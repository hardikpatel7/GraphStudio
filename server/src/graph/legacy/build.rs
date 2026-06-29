//! Build an [`ArticleGraph`] from a [`GraphSourceReader`].
//!
//! Single entry point: [`build_graph`]. Reads everything the graph needs
//! through the reader, threads the hierarchy spine, attaches leaf
//! metrics, and rolls metrics up bottom-up. RCL rule_pointers are bound
//! by [`crate::graph::legacy::selector_trie::bind_rules`] (called from
//! the same orchestration step in the pipeline assembly handler).
//!
//! No backend-specific code lives here — the reader trait abstracts
//! DuckDB / parquet / PG / BQ.

use anyhow::Result;
use std::time::Instant;

use crate::graph::legacy::graph::{ArticleGraph, MetricKind, NodeKind, StrId};
use crate::graph::legacy::rollup::post_order_rollup;
use crate::graph::legacy::rows::GraphSourceReader;

/// Build counts emitted in the build log so callers can confirm the
/// shape of the graph without re-querying it.
#[derive(Debug, Clone, Copy, Default)]
pub struct BuildStats {
    pub articles: usize,
    pub product_codes: usize,
    pub hierarchy_nodes: usize,
    pub stores: usize,
    pub channels: usize,
    pub strings: usize,
    pub elapsed_ms: u128,
}

/// Build a graph snapshot from `reader`. Emits a `tracing::info!` line
/// at completion with the build stats. The caller is responsible for
/// wrapping the result in `ArcSwapOption<ArticleGraph>` and publishing
/// it on `AppState`.
///
/// `graph_version` is bumped by the caller — typically a monotonic
/// counter from `AppState`. CDC delta paths produce graphs with the
/// next version; full rebuilds bump too.
pub fn build_graph(
    reader: &dyn GraphSourceReader,
    graph_version: u64,
    cancel: &tokio_util::sync::CancellationToken,
    ruleset: Option<&rcl::RuleSet>,
) -> Result<(ArticleGraph, BuildStats)> {
    let started = Instant::now();
    let mut graph = ArticleGraph::new(graph_version);

    // Each phase boundary here lets the user abort a long graph build
    // without waiting for the rollup or PSM passes to run on data they
    // already decided to throw away.
    let bail_if_cancelled = || -> Result<()> {
        if cancel.is_cancelled() {
            Err(anyhow::anyhow!("article_graph build: cancelled"))
        } else {
            Ok(())
        }
    };

    // ── Read source data via the reader. Each call materializes a Rust
    // collection; nothing is streamed into the graph build below.
    let read_start = Instant::now();
    let ph_master = reader.read_ph_master()?;
    bail_if_cancelled()?;
    let paf_by_pc = reader.read_paf()?;
    bail_if_cancelled()?;
    let inv_by_ph = reader.read_inventory()?;
    bail_if_cancelled()?;
    let txs_by_ph = reader.read_txs_metrics()?;
    bail_if_cancelled()?;
    let product_dc = reader.read_product_dc()?;
    bail_if_cancelled()?;
    let store_dc = reader.read_store_dc()?;
    let dc_names = reader.read_distribution_centres()?;
    let store_channels = reader.read_store_channels()?;
    let store_to_sgs = reader.read_store_to_sgs()?;
    let active_stores = reader.read_active_store_codes()?;
    bail_if_cancelled()?;
    let _ = dc_names; // surfaced via cross_indices in a later phase
    tracing::info!(
        "[article_graph] reads done in {}ms: {} ph, {} paf, {} inv, {} txs, {} product_dc, {} store_dc, {} active_stores",
        read_start.elapsed().as_millis(),
        ph_master.len(),
        paf_by_pc.len(),
        inv_by_ph.len(),
        txs_by_ph.len(),
        product_dc.len(),
        store_dc.len(),
        active_stores.len(),
    );

    // ── Pre-intern channel names — small set (≤ 5) so a single pass
    // through ph_master + store_channels is enough.
    // ── Build the product-side hierarchy spine. For each ph_master row:
    // l0 → l1 → l2 → l3 → l4 → l5 → article → product_code.
    let build_start = Instant::now();
    let mut articles = 0usize;
    let mut product_codes = 0usize;
    for ph in &ph_master {
        let l0 = graph.intern(&ph.l0_name);
        let l1 = graph.intern(&ph.l1_name);
        let l2 = graph.intern(&ph.l2_name);
        let l3 = graph.intern(&ph.l3_name);
        let l4 = graph.intern(&ph.l4_name);
        let l5 = graph.intern(&ph.l5_name);
        let article = graph.intern(&ph.article);

        let l0_id = graph.upsert_node(NodeKind::L0, l0, graph.root);
        let l1_id = graph.upsert_node(NodeKind::L1, l1, l0_id);
        let l2_id = graph.upsert_node(NodeKind::L2, l2, l1_id);
        let l3_id = graph.upsert_node(NodeKind::L3, l3, l2_id);
        let l4_id = graph.upsert_node(NodeKind::L4, l4, l3_id);
        let l5_id = graph.upsert_node(NodeKind::L5, l5, l4_id);
        let article_id = graph.upsert_node(NodeKind::Article, article, l5_id);
        articles += 1;

        // Brand cross-edge — brand is not on the spine but is queried
        // alongside articles for filter/aggregate. Two indices:
        //   brand → [article_ids]  (for "all articles for brand X")
        //   article → brand        (for per-article projections; O(1))
        // The inverse map is load-bearing for `project_rows(ARTICLE)`,
        // which would otherwise scan brand_to_articles per article.
        if !ph.brand.is_empty() {
            let brand = graph.intern(&ph.brand);
            graph
                .cross_indices
                .brand_to_articles
                .entry(brand)
                .or_default()
                .push(article_id);
            graph
                .cross_indices
                .article_to_brand
                .insert(article_id, brand);
        }
        // Channel cross-edge — `ph_master.channel` is a comma-separated
        // text. Each article may belong to multiple channels; we record
        // only the first (V7 `compute_constraints` only filters by the
        // PH's primary channel string).
        if !ph.channel.is_empty() {
            let primary = ph.channel.split(',').next().unwrap_or("").trim();
            if !primary.is_empty() {
                let ch = graph.intern(primary);
                graph.cross_indices.article_to_channel.insert(article_id, ch);
            }
        }
        // product_codes string: V7's extract joins them with `|`.
        for pc in ph
            .product_codes
            .split(|c: char| c == '|' || c == ',')
            .filter(|s| !s.is_empty())
        {
            let pc_str = graph.intern(pc);
            let _pc_id = graph.upsert_node(NodeKind::ProductCode, pc_str, article_id);
            product_codes += 1;
        }
    }

    // ── product_code → DCs cross-index (separate from spine).
    for (pc, dcs) in &product_dc {
        if dcs.is_empty() {
            continue;
        }
        let pc_str = graph.intern(pc);
        let mut interned = smallvec::SmallVec::<[StrId; 4]>::new();
        for dc in dcs {
            interned.push(graph.intern(dc));
        }
        graph
            .cross_indices
            .product_code_to_dcs
            .insert(pc_str, interned);
    }

    // ── Store-side spine: Root → Channel → StoreCode. Stores are
    // filtered to the active set per `store_master`.
    let mut store_count = 0usize;
    let mut channels_seen: std::collections::HashSet<StrId> = std::collections::HashSet::new();
    for (store, channel) in &store_channels {
        if !active_stores.contains(store) {
            continue;
        }
        let ch = graph.intern(channel);
        channels_seen.insert(ch);
        let ch_id = graph.upsert_node(NodeKind::Channel, ch, graph.root);
        let store_str = graph.intern(store);
        let _store_id = graph.upsert_node(NodeKind::StoreCode, store_str, ch_id);
        store_count += 1;
    }

    // store_code → DCs cross-index.
    for (store, dcs) in &store_dc {
        if !active_stores.contains(store) {
            continue;
        }
        let store_str = graph.intern(store);
        let mut interned = smallvec::SmallVec::<[StrId; 4]>::new();
        for dc in dcs {
            interned.push(graph.intern(dc));
        }
        graph
            .cross_indices
            .store_code_to_dcs
            .insert(store_str, interned);
    }
    // store_code → store_groups cross-index.
    for (store, sgs) in &store_to_sgs {
        if !active_stores.contains(store) {
            continue;
        }
        let store_str = graph.intern(store);
        let mut interned = smallvec::SmallVec::<[StrId; 4]>::new();
        for sg in sgs {
            interned.push(graph.intern(sg));
        }
        graph
            .cross_indices
            .store_code_to_sgs
            .insert(store_str, interned);
    }

    tracing::info!(
        "[article_graph] spine built in {}ms: {} articles, {} product_codes, {} stores, {} channels",
        build_start.elapsed().as_millis(),
        articles,
        product_codes,
        store_count,
        channels_seen.len(),
    );
    bail_if_cancelled()?;

    // ── Attach leaf metrics. Every PH's inventory + txs aggregates land
    // on its article node. Stored in the metric slots defined by
    // `MetricKind`. Bottom-up rollup will propagate up the spine.
    let metrics_start = Instant::now();
    let mut articles_with_metrics = 0usize;
    for ph in &ph_master {
        let article = graph.intern(&ph.article);
        let Some(article_id) = graph.find(NodeKind::Article, article) else {
            continue;
        };
        if let Some(inv) = inv_by_ph.get(&ph.ph_code) {
            let m = &mut graph.node_mut(article_id).metrics;
            m[MetricKind::Oh.idx()] += inv.oh as f64;
            m[MetricKind::Oo.idx()] += inv.oo as f64;
            m[MetricKind::It.idx()] += inv.it as f64;
            m[MetricKind::ReserveQuantity.idx()] += inv.reserve_quantity as f64;
            m[MetricKind::AllocatedUnits.idx()] += inv.allocated_units as f64;
        }
        if let Some(txs) = txs_by_ph.get(&ph.ph_code) {
            let m = &mut graph.node_mut(article_id).metrics;
            m[MetricKind::LwUnits.idx()] += txs.lw_units as f64;
            m[MetricKind::LwRevenue.idx()] += txs.lw_revenue as f64;
            m[MetricKind::LwMargin.idx()] += txs.lw_margin as f64;
        }
        articles_with_metrics += 1;
    }
    tracing::info!(
        "[article_graph] leaf metrics attached in {}ms: {} articles",
        metrics_start.elapsed().as_millis(),
        articles_with_metrics,
    );
    bail_if_cancelled()?;

    // ── Bottom-up rollup. Single post-order DFS.
    let rollup_start = Instant::now();
    post_order_rollup(&mut graph);
    tracing::info!(
        "[article_graph] rollup done in {}ms",
        rollup_start.elapsed().as_millis()
    );
    bail_if_cancelled()?;

    // ── PSM resolver. Reads three small tables from the same DuckDB
    // pull and stores them in the graph for the gRPC `ResolveRcl`
    // path. Each call is best-effort — missing extracts (e.g. a fresh
    // tenant) leave the resolver empty; downstream surfaces "no PSM
    // match" instead of erroring.
    // On-the-fly PSM resolver. Reads `(rcl_code, rule_code, dim_json)`
    // rows from the extract; PsmResolver::build parses each dim_json
    // and groups rules into per-rcl-code buckets keyed by the
    // dimension field tuple. No per-product hash table.
    let psm_start = Instant::now();
    let priorities = reader.read_psm_priorities().unwrap_or_default();
    let raw_rules = reader.read_psm_rule_dim().unwrap_or_default();
    let raw_rule_count = raw_rules.len();
    graph.psm =
        crate::graph::legacy::psm_resolver::PsmResolver::build(priorities, raw_rules);
    let bucket_count: usize = graph.psm.by_rcl.values().map(|i| i.buckets.len()).sum();
    let rule_count: usize = graph
        .psm
        .by_rcl
        .values()
        .flat_map(|i| i.buckets.iter())
        .map(|b| b.by_tuple.len())
        .sum();
    tracing::info!(
        "[article_graph] PSM resolver built in {}ms: {} priorities, {} rcl_codes, {} buckets, {} rules ({} raw rows in), ready={}",
        psm_start.elapsed().as_millis(),
        graph.psm.priorities.len(),
        graph.psm.by_rcl.len(),
        bucket_count,
        rule_count,
        raw_rule_count,
        graph.psm.is_ready(),
    );

    bail_if_cancelled()?;

    // ── Pre-bind RCL rule pointers per Article. Replaces per-projection
    // `explain_dc_policy` / `explain_constraints` walks (each O(n_rules)
    // priority + O(n_dim_rules) specificity) with a node-side O(1)
    // pointer lookup. When the ruleset changes, the host triggers a
    // rebuild and the new graph version starts fresh; partial rebinds
    // aren't supported (see `rule_pointers_version` on graph).
    if let Some(rules) = ruleset {
        let bind_start = Instant::now();
        let bound = bind_rule_pointers(&mut graph, rules);
        graph.rule_pointers_version = rules.version;
        tracing::info!(
            "[article_graph] rule_pointers bound in {}ms: {} articles bound (ruleset v{})",
            bind_start.elapsed().as_millis(),
            bound,
            rules.version,
        );
    } else {
        tracing::info!(
            "[article_graph] rule_pointers binding skipped — no ruleset supplied"
        );
    }

    // Drop the build-time string-index — readers don't need it.
    let strings = graph.string_pool.len();
    graph.finalize_strings();

    let elapsed_ms = started.elapsed().as_millis();
    let stats = BuildStats {
        articles,
        product_codes,
        hierarchy_nodes: graph.count_kind(NodeKind::L0)
            + graph.count_kind(NodeKind::L1)
            + graph.count_kind(NodeKind::L2)
            + graph.count_kind(NodeKind::L3)
            + graph.count_kind(NodeKind::L4)
            + graph.count_kind(NodeKind::L5),
        stores: store_count,
        channels: channels_seen.len(),
        strings,
        elapsed_ms,
    };
    tracing::info!(
        "[article_graph] built v{} in {}ms: {} articles, {} product_codes, {} hierarchy nodes, {} stores, {} strings",
        graph_version,
        elapsed_ms,
        stats.articles,
        stats.product_codes,
        stats.hierarchy_nodes,
        stats.stores,
        stats.strings,
    );
    // PafRow + paf_by_pc are not used by Phase 1 build — they'll feed
    // the selector-trie binding step (Task #42), which uses each
    // product's PAF hierarchy to look up matching RCL rules. Reading
    // them up front so the trait surface stabilizes now.
    let _ = paf_by_pc;
    Ok((graph, stats))
}

/// Walk every Article node and bind RCL pointers for the DcPolicy and
/// Constraints flavors. Per-article cost ≈ two priority-walks of the
/// ruleset (≤ 16 rules × 7 selector-field comparisons each), then two
/// HashMap lookups for the rule_code; total wall-clock ~1-2s for the
/// full Bealls dataset (535K articles). Pays off as long as Live View
/// projections happen more than ~5 times per build cycle.
///
/// PSM is not bound here — that's served by the on-the-fly resolver
/// in `psm_resolver.rs` which has a different lookup shape (per-rcl
/// bucket keyed by dimension tuple, not per-article).
///
/// Returns the number of articles successfully bound (those for which
/// at least one of DcPolicy / Constraints resolved). An article with
/// no matching rules gets an empty `rule_pointers` vec — projection
/// falls through to the same null-emit path it uses today.
///
/// Two-pass split so the explain calls run in parallel:
///   1. **Parallel pass** (rayon): for every article, walk the
///      hierarchy, run `explain_dc_policy` + `explain_constraints`,
///      and collect the owned `(rcl_code, rule_code)` String pairs.
///      Reads only — `&ArticleGraph` is `Sync`.
///   2. **Sequential pass**: intern each unique code into the graph's
///      string pool and write the resolved `RulePtr`s onto each node.
///      Has to be serial because `intern` and `node_mut` both take
///      `&mut self`.
///
/// Pass 1 dominates (the explain walk is `O(n_rules)` per article;
/// pass 2 is `O(matched_articles)` and trivially fast). On the bealls
/// dataset (46K articles, 16 rules) this drops the sequential ~9.4s
/// down to ~1–2s on a multi-core machine, with no change to the
/// resulting graph state.
fn bind_rule_pointers(graph: &mut ArticleGraph, rules: &rcl::RuleSet) -> usize {
    use crate::graph::legacy::graph::{RuleKind, RulePtr};
    use crate::graph::legacy::resolver::{explain_constraints, explain_dc_policy};
    use rayon::prelude::*;

    // Snapshot the article NodeIds. We take ownership so we can mutate
    // each node's `rule_pointers` without holding a borrow on the
    // arena while iterating.
    let article_ids: Vec<crate::graph::legacy::NodeId> =
        graph.by_kind[NodeKind::Article.idx()].values().copied().collect();

    // Helper: walk `node`'s parents to recover the hierarchy + brand
    // strings (same pattern `project_single` uses). Returns owned
    // String for each level — needed because rcl::ProductHierarchy
    // takes `&str` and we can't borrow from `graph` while interning.
    fn hierarchy_strings(
        graph: &ArticleGraph,
        article_id: crate::graph::legacy::NodeId,
    ) -> [String; 8] {
        let mut levels: [String; 6] = Default::default();
        let mut cur = graph.node(article_id).parent;
        while !cur.is_none() {
            let p = graph.node(cur);
            let name = graph.get_str(p.name).to_string();
            match p.kind {
                NodeKind::L0 => levels[0] = name,
                NodeKind::L1 => levels[1] = name,
                NodeKind::L2 => levels[2] = name,
                NodeKind::L3 => levels[3] = name,
                NodeKind::L4 => levels[4] = name,
                NodeKind::L5 => levels[5] = name,
                NodeKind::Root => break,
                _ => {}
            }
            cur = p.parent;
        }
        let brand = graph
            .cross_indices
            .article_to_brand
            .get(&article_id)
            .map(|s| graph.get_str(*s).to_string())
            .unwrap_or_default();
        // Article name as the product_code stand-in for selector matching
        // (the resolver doesn't dispatch on product_code, but the type
        // requires a non-empty string).
        let article_name = graph.get_str(graph.node(article_id).name).to_string();
        [
            levels[0].clone(), levels[1].clone(), levels[2].clone(),
            levels[3].clone(), levels[4].clone(), levels[5].clone(),
            brand,
            article_name,
        ]
    }

    // Owned result per article — kept compact so pass-1 → pass-2
    // doesn't pay a large heap-allocation tax.
    type BoundEntries = smallvec::SmallVec<[(RuleKind, String, String); 3]>;

    // ── Pass 1 — parallel explain. Filters out articles with no
    // resolved DcPolicy AND no resolved Constraints so pass 2 only
    // touches nodes that need pointer assignments.
    let graph_ref: &ArticleGraph = graph;
    let pending: Vec<(crate::graph::legacy::NodeId, BoundEntries)> = article_ids
        .par_iter()
        .filter_map(|&article_id| {
            let h = hierarchy_strings(graph_ref, article_id);
            let p = rcl::ProductHierarchy {
                product_code: h[7].as_str(),
                l0_name: h[0].as_str(),
                l1_name: h[1].as_str(),
                l2_name: h[2].as_str(),
                l3_name: h[3].as_str(),
                l4_name: h[4].as_str(),
                l5_name: h[5].as_str(),
                brand: h[6].as_str(),
            };
            let dc = explain_dc_policy(rules, &p);
            let cn = explain_constraints(rules, &p);
            if dc.is_none() && cn.is_none() {
                return None;
            }
            let mut entries: BoundEntries = smallvec::SmallVec::new();
            if let Some(dc) = dc {
                entries.push((RuleKind::DcPolicy, dc.rcl_code, dc.rule_code));
            }
            if let Some(cn) = cn {
                entries.push((RuleKind::Constraints, cn.rcl_code, cn.rule_code));
            }
            Some((article_id, entries))
        })
        .collect();

    // ── Pass 2 — sequential intern + assign. Can't parallelize:
    // `intern` mutates `string_pool` + `string_index` (mutable
    // borrow), and the resulting StrId must be stable across all
    // writers. Cost is tiny in practice: ~46K interns of strings
    // drawn from a small set of unique codes (so HashMap hits hot
    // after the first few thousand articles).
    let mut bound_count = 0usize;
    for (article_id, entries) in pending {
        let mut ptrs = smallvec::SmallVec::<[RulePtr; 3]>::new();
        for (kind, rcl_code, rule_code) in entries {
            let rcl_id = graph.intern(&rcl_code);
            let rule_id = graph.intern(&rule_code);
            ptrs.push(RulePtr { kind, rcl_code: rcl_id, rule_code: rule_id });
        }
        graph.node_mut(article_id).rule_pointers = ptrs;
        bound_count += 1;
    }
    bound_count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::legacy::graph::MetricKind;
    use crate::graph::legacy::rows::{
        GraphSourceReader, InventoryAgg, PafRow, PhMasterRow, TxsMetrics,
    };
    use std::collections::{HashMap, HashSet};

    /// Tiny in-memory reader for unit tests. Lets us build a graph from
    /// hand-coded rows without any DuckDB.
    struct MockReader {
        ph: Vec<PhMasterRow>,
        paf: HashMap<String, PafRow>,
        inv: HashMap<String, InventoryAgg>,
        txs: HashMap<String, TxsMetrics>,
        product_dc: HashMap<String, Vec<String>>,
        store_dc: HashMap<String, Vec<String>>,
        dc_names: HashMap<String, String>,
        store_channels: HashMap<String, String>,
        store_to_sgs: HashMap<String, Vec<String>>,
        active_stores: HashSet<String>,
    }

    impl GraphSourceReader for MockReader {
        fn read_ph_master(&self) -> Result<Vec<PhMasterRow>> {
            Ok(self.ph.clone())
        }
        fn read_paf(&self) -> Result<HashMap<String, PafRow>> {
            Ok(self.paf.clone())
        }
        fn read_inventory(&self) -> Result<HashMap<String, InventoryAgg>> {
            Ok(self.inv.clone())
        }
        fn read_txs_metrics(&self) -> Result<HashMap<String, TxsMetrics>> {
            Ok(self.txs.clone())
        }
        fn read_product_dc(&self) -> Result<HashMap<String, Vec<String>>> {
            Ok(self.product_dc.clone())
        }
        fn read_store_dc(&self) -> Result<HashMap<String, Vec<String>>> {
            Ok(self.store_dc.clone())
        }
        fn read_distribution_centres(&self) -> Result<HashMap<String, String>> {
            Ok(self.dc_names.clone())
        }
        fn read_store_channels(&self) -> Result<HashMap<String, String>> {
            Ok(self.store_channels.clone())
        }
        fn read_store_to_sgs(&self) -> Result<HashMap<String, Vec<String>>> {
            Ok(self.store_to_sgs.clone())
        }
        fn read_active_store_codes(&self) -> Result<HashSet<String>> {
            Ok(self.active_stores.clone())
        }
        fn read_psm_priorities(&self) -> Result<Vec<(String, i32)>> {
            Ok(Vec::new())
        }
        fn read_psm_rule_dim(&self) -> Result<Vec<(String, String, String)>> {
            Ok(Vec::new())
        }
    }

    fn ph(
        ph_code: &str,
        article: &str,
        l0: &str,
        l1: &str,
        l2: &str,
        product_codes: &str,
    ) -> PhMasterRow {
        PhMasterRow {
            ph_code: ph_code.into(),
            article: article.into(),
            l0_name: l0.into(),
            l1_name: l1.into(),
            l2_name: l2.into(),
            l3_name: "".into(),
            l4_name: "".into(),
            l5_name: "".into(),
            brand: "".into(),
            channel: "bls".into(),
            product_codes: product_codes.into(),
        }
    }

    /// Two articles under the same l0/l1, different l2. After build +
    /// rollup, l1's OH should equal sum of both articles' OH.
    #[test]
    fn build_two_articles_rolls_up() {
        let mut inv = HashMap::new();
        inv.insert(
            "ph1".into(),
            InventoryAgg {
                oh: 10,
                oo: 0,
                it: 0,
                reserve_quantity: 0,
                allocated_units: 0,
            },
        );
        inv.insert(
            "ph2".into(),
            InventoryAgg {
                oh: 25,
                oo: 0,
                it: 0,
                reserve_quantity: 0,
                allocated_units: 0,
            },
        );
        let reader = MockReader {
            ph: vec![
                ph("ph1", "A1", "30", "3510", "3548", "p1|p2"),
                ph("ph2", "A2", "30", "3510", "3549", "p3"),
            ],
            paf: HashMap::new(),
            inv,
            txs: HashMap::new(),
            product_dc: HashMap::new(),
            store_dc: HashMap::new(),
            dc_names: HashMap::new(),
            store_channels: HashMap::new(),
            store_to_sgs: HashMap::new(),
            active_stores: HashSet::new(),
        };
        let cancel = tokio_util::sync::CancellationToken::new();
        let (graph, stats) = build_graph(&reader, 1, &cancel, None).unwrap();
        assert_eq!(stats.articles, 2);
        assert_eq!(stats.product_codes, 3);
        // 1 l0 ('30') + 1 l1 ('3510') + 2 l2 ('3548','3549') + 1 each
        // for the empty-string l3/l4/l5 (interned once, shared).
        assert_eq!(stats.hierarchy_nodes, 1 + 1 + 2 + 1 + 1 + 1);

        let l1_name = {
            // recreate the StrId
            graph
                .string_pool
                .iter()
                .position(|s| s.as_ref() == "3510")
                .map(|i| crate::graph::legacy::graph::StrId(i as u32))
                .expect("l1 string interned")
        };
        let l1_id = graph.find(NodeKind::L1, l1_name).expect("l1 node exists");
        assert_eq!(graph.node(l1_id).metrics[MetricKind::Oh.idx()], 35.0);
    }
}
