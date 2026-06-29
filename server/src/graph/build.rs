//! Orchestrate a `Graph` build from a validated `GraphSpec`.
//!
//! Reads source data through [`SourceReader`], populates the kind +
//! metric registries from spec, walks each hierarchy to build the
//! spine, attaches per-source metric rows, then post-order rolls
//! everything up.
//!
//! ## What Phase 2 covers
//!
//! - Single primary hierarchy + any number of auxiliary hierarchies
//!   (each is a separate spine under the synthetic root).
//! - Metric sources with a single `attaches_at = "<kind>"` declaration.
//! - All scalar/collection/bool rollup operators (the engine handles
//!   them uniformly; the bealls case happens to use only `Sum`).
//!
//! ## Deferred to Phase 3
//!
//! - Composite-attach metrics (cube storage in
//!   `Graph::composite_metrics`).
//! - Bridge sources → cross-edge materialization.
//! - `split = "<sep>"` / `unnest = true` level expansion.
//! - Relation-bridged identity column lookup (when the metric source's
//!   identity column doesn't share a name with the attach kind's
//!   identifying column).

use anyhow::{Result, anyhow};
use std::collections::HashMap;
use std::time::Instant;

use super::graph::{
    CrossEdgeMeta, CrossEdgeRegistry, Graph, KindId, KindRegistry, MetricMeta,
    MetricRegistry, MetricValue,
};
use super::rollup::{merge, post_order_rollup};
use super::source::{Row, SourceReader};
use super::spec::{
    AttachesAt, GraphSpec, HierarchySpec, MetricSpec, Severity, SourceSpec, validate,
};

/// Counts emitted at the end of every build. Logged at info level so
/// telemetry can confirm the graph's shape without re-querying it.
#[derive(Debug, Clone, Default)]
pub struct BuildStats {
    /// Kind name → node count. Includes `__root__: 1`.
    pub nodes_by_kind: HashMap<String, usize>,
    pub total_nodes: usize,
    pub primary_metric_count: usize,
    pub composite_metric_count: usize,
    pub strings_interned: usize,
    pub elapsed_ms: u128,
}

/// Build a graph snapshot from `spec` via `reader`. Bumping
/// `graph_version` per build is the caller's responsibility — usually a
/// monotonic counter on `AppState`.
pub fn build_graph(
    spec: &GraphSpec,
    reader: &dyn SourceReader,
    graph_version: u64,
) -> Result<(Graph, BuildStats)> {
    let started = Instant::now();

    // ── 1. Validation gate. Even though Phase 1's UI save path already
    // validates, the underlying source schema or another tenant's edit
    // could have drifted since then. Cheap to re-run; the cost of
    // building a graph against an invalid spec is high.
    let issues = validate(spec);
    let errors: Vec<_> = issues
        .iter()
        .filter(|i| matches!(i.severity, Severity::Error))
        .collect();
    if !errors.is_empty() {
        return Err(anyhow!(
            "graph spec `{}` failed validation with {} error(s): {}",
            spec.id,
            errors.len(),
            errors
                .iter()
                .map(|i| format!("[{}] {}", i.code, i.message))
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }

    // ── 2. Build the source-alias → SourceSpec map. Used by every
    // later pass when we need to look up a source by name.
    let sources_by_alias: HashMap<String, &SourceSpec> = spec
        .sources
        .iter()
        .map(|s| (s.alias.clone(), s))
        .collect();

    // ── 3. Build registries. Kinds: root + every level. Metrics: every
    // entry under `[metrics.<src>]`, flagged composite if the source's
    // `attaches_at` is composite.
    let (kinds, kind_of_level) = build_kind_registry(spec);
    let (metrics, metric_slot_index) = build_metric_registry(spec, &sources_by_alias, &kind_of_level);
    // Cross-edge registry is empty in Phase 2 — bridge support is a
    // Phase 3 add. Allocated so the field shape on `Graph` is stable.
    let cross_edges = CrossEdgeRegistry::default();

    let mut graph = Graph::new_with_registries(kinds, metrics, cross_edges, graph_version);

    // ── 4. Build each hierarchy's spine.
    for (hname, h) in &spec.hierarchy {
        let source = sources_by_alias
            .get(&h.source)
            .ok_or_else(|| anyhow!("hierarchy `{hname}` source `{}` not found", h.source))?;
        build_hierarchy_spine(&mut graph, hname, h, source, &kind_of_level, reader)?;
    }

    // ── 5. Attach metrics. Source-by-source so we read each table at
    // most once per role.
    for (src_alias, metrics_block) in &spec.metrics {
        let source = match sources_by_alias.get(src_alias) {
            Some(s) => *s,
            None => continue, // validate flagged this as METRIC_SOURCE_UNKNOWN
        };
        // Composite-attach metrics live in `composite_metrics`, not on
        // `Node`. Skip in Phase 2; surfaces in BuildStats.
        let attach = match &source.attaches_at {
            Some(AttachesAt::Single(k)) => k.clone(),
            Some(AttachesAt::Composite(_)) => continue,
            None => continue, // bridge source — handled by cross-edge pass below
        };

        attach_metrics_from_source(
            &mut graph,
            src_alias,
            &attach,
            source,
            metrics_block,
            &kind_of_level,
            &metric_slot_index,
            reader,
        )?;
    }

    // ── 5b. Materialize cross-edges from bridge sources. A bridge is
    // a source with no `attaches_at` that has relations connecting it
    // to ≥ 2 distinct hierarchies; its rows become edges between the
    // resolved node pairs. The bealls product→DC and store→DC links
    // ride this path once the corresponding hierarchies are declared.
    let bridges = detect_bridges(spec, &sources_by_alias, &kind_of_level);
    materialize_cross_edges(&mut graph, &bridges, &sources_by_alias, reader)?;

    // ── 5c. Optional RCL/PSM build. Tries `raw_rcl_psm_priorities` +
    // `raw_rcl_psm_rule_dim`; absence is non-fatal (matches v1's
    // `unwrap_or_default` on the two reads). Tenants without PSM data
    // get `graph.psm.is_ready() == false` and skip RCL-dependent paths.
    match super::rcl::build_psm_resolver(reader) {
        Ok(Some(resolver)) => graph.psm = resolver,
        Ok(None) => {
            tracing::info!("graph: PSM tables absent; RCL disabled for graph `{}`", spec.id);
        }
        Err(e) => {
            tracing::warn!(error = %e, "graph: PSM build failed; RCL disabled for graph `{}`", spec.id);
        }
    }

    // ── 6. Roll up. Spine first (parent ← children within each
    // hierarchy), then cross-edges (push metrics from the natively-
    // attached side to the bridge-connected side). The order matters:
    // spine populates intermediate ancestors that aren't themselves
    // the cross-edge endpoint, and cross-edge then carries the
    // already-aggregated values across hierarchies.
    post_order_rollup(&mut graph);
    crate::graph::rollup::cross_edge_rollup(&mut graph);

    // ── 7. Finalize: drop the build-time string index now that no more
    // interning will happen. Halves the resident memory of a finished
    // graph on bealls (~715 K strings ≈ 100 MB).
    graph.finalize_strings();

    // ── 8. Assemble stats.
    let mut nodes_by_kind: HashMap<String, usize> = HashMap::new();
    for (id, meta) in graph.kinds.iter() {
        nodes_by_kind.insert(meta.name.clone(), graph.count_kind(id));
    }
    let composite_count = graph
        .metrics
        .iter()
        .filter(|(_, m)| m.is_composite)
        .count();
    let stats = BuildStats {
        total_nodes: graph.node_count(),
        primary_metric_count: graph.metrics.len() - composite_count,
        composite_metric_count: composite_count,
        strings_interned: graph.string_pool.len(),
        elapsed_ms: started.elapsed().as_millis(),
        nodes_by_kind,
    };

    tracing::info!(
        graph_id = %spec.id,
        nodes = stats.total_nodes,
        metrics = stats.primary_metric_count + stats.composite_metric_count,
        strings = stats.strings_interned,
        elapsed_ms = stats.elapsed_ms,
        "graph::build_graph done"
    );

    Ok((graph, stats))
}

// ──────────────────────────────────────────────────────────────────────────
// Registry construction
// ──────────────────────────────────────────────────────────────────────────

/// Build the kind registry and the spec-level → KindId reverse map. The
/// returned map is keyed on the bare level id; level ids are globally
/// unique across hierarchies by Decision 28's validation rule, so we
/// don't need a compound key.
fn build_kind_registry(spec: &GraphSpec) -> (KindRegistry, HashMap<String, KindId>) {
    let mut kinds = KindRegistry::default();
    kinds.register("__root__".to_string(), "".to_string());

    let mut kind_of_level: HashMap<String, KindId> = HashMap::new();
    for (hname, h) in &spec.hierarchy {
        for level_id in h.levels.keys() {
            let kid = kinds.register(level_id.clone(), hname.clone());
            kind_of_level.insert(level_id.clone(), kid);
        }
    }
    (kinds, kind_of_level)
}

/// Build the metric registry. Returns the registry plus a
/// `(source_alias, metric_name) → slot_index` lookup so the attach
/// pass can find the right slot on `Node.metrics`. Slot index =
/// position among non-composite metrics in declaration order; the same
/// derivation that `Graph::insert_node` uses to size each node's
/// metric box, kept in lock-step.
fn build_metric_registry(
    spec: &GraphSpec,
    sources_by_alias: &HashMap<String, &SourceSpec>,
    kind_of_level: &HashMap<String, KindId>,
) -> (MetricRegistry, HashMap<(String, String), usize>) {
    let mut metrics = MetricRegistry::default();
    let mut slot_index: HashMap<(String, String), usize> = HashMap::new();
    let mut next_slot: usize = 0;

    for (src_alias, block) in &spec.metrics {
        let source = sources_by_alias.get(src_alias);
        let is_composite = source
            .and_then(|s| s.attaches_at.as_ref())
            .map(|a| matches!(a, AttachesAt::Composite(_)))
            .unwrap_or(false);
        // Resolve the single-attach kind for non-composite sources.
        // Cross-edge rollup uses this to decide which side of a bridge
        // naturally owns the metric (and therefore which direction to
        // push it across).
        let attach_kind: Option<KindId> = source
            .and_then(|s| match s.attaches_at.as_ref()? {
                AttachesAt::Single(k) => kind_of_level.get(k).copied(),
                AttachesAt::Composite(_) => None,
            });

        for (mid, m) in block {
            let column = m.column.clone().unwrap_or_else(|| mid.clone());
            metrics.register(MetricMeta {
                name: mid.clone(),
                source_alias: src_alias.clone(),
                column,
                rollup: m.rollup,
                is_composite,
                attach_kind,
            });
            if !is_composite {
                slot_index.insert((src_alias.clone(), mid.clone()), next_slot);
                next_slot += 1;
            }
        }
    }
    (metrics, slot_index)
}

// ──────────────────────────────────────────────────────────────────────────
// Hierarchy spine
// ──────────────────────────────────────────────────────────────────────────

/// Walk the source rows for one hierarchy, threading each row through
/// the level chain to build the spine. `upsert_node` dedupes shared
/// prefixes — multiple ph_master rows under the same l0 collapse to
/// one l0 node.
///
/// Levels with `split = "<sep>"` (Decision 21, legacy delimited
/// strings) expand one cell into many sibling nodes — for the bealls
/// case, that's `product_codes = "P1|P2|P3"` producing three
/// `product_code` leaves under one article. When a split occurs
/// mid-chain (rare; bealls only splits at the leaf), the LAST token
/// is taken as the continuation parent for deeper levels — full
/// cartesian-product walking would require a re-shape of the loop and
/// has no use case today.
///
/// `unnest = true` falls back to scalar handling in Phase 3 because
/// our DuckDB reader doesn't yet emit `CellValue::List` for native
/// LIST columns; once it does, this path will mirror the split
/// branch.
fn build_hierarchy_spine(
    graph: &mut Graph,
    hname: &str,
    h: &HierarchySpec,
    source: &SourceSpec,
    kind_of_level: &HashMap<String, KindId>,
    reader: &dyn SourceReader,
) -> Result<()> {
    if h.levels.is_empty() {
        return Ok(());
    }
    let level_ids: Vec<String> = h.levels.keys().cloned().collect();
    let columns: Vec<String> = h.levels.values().map(|l| l.column.clone()).collect();

    let rows = reader.read(&source.table, &columns, source.filter.as_deref())?;
    tracing::debug!(
        hierarchy = hname,
        source = source.alias,
        rows = rows.len(),
        "graph::build_hierarchy_spine read"
    );

    let root = graph.root;
    for row in &rows {
        let mut parent = root;
        for (i, level_id) in level_ids.iter().enumerate() {
            let kid = *kind_of_level
                .get(level_id)
                .ok_or_else(|| anyhow!("level `{level_id}` missing from kind index"))?;
            let level = h
                .levels
                .get(level_id)
                .ok_or_else(|| anyhow!("level `{level_id}` missing from hierarchy"))?;
            let raw = row.cells.get(i);
            let value = raw.map(|c| c.as_text()).unwrap_or_default();

            if let Some(sep) = level.split.as_deref() {
                // Multi-value column. Split, trim, upsert each
                // non-empty token; the LAST token becomes the
                // continuation parent for any deeper levels.
                let mut last = parent;
                let mut emitted_any = false;
                for tok in value.split(sep) {
                    let t = tok.trim();
                    if t.is_empty() {
                        continue;
                    }
                    let sid = graph.intern(t);
                    last = graph.upsert_node(kid, sid, parent);
                    emitted_any = true;
                }
                if !emitted_any {
                    // All tokens were empty — collapse to a single
                    // empty-string node so the chain still threads.
                    // Matches the V1 behavior where a missing
                    // product_codes value attaches under article as ''.
                    let sid = graph.intern("");
                    last = graph.upsert_node(kid, sid, parent);
                }
                parent = last;
            } else {
                let sid = graph.intern(&value);
                parent = graph.upsert_node(kid, sid, parent);
            }
        }
    }
    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────
// Metric attachment
// ──────────────────────────────────────────────────────────────────────────

/// Attach metric values from one source to the nodes its rows resolve
/// to. `attach_kind` is the level id from `source.attaches_at`; the
/// identity column on the source is the same name as the kind's
/// identifying column (Decision 28 — the simple case Phase 2 supports).
///
/// Each row: find the matching node by `(attach_kind_id, intern(identity_value))`,
/// then for each metric in the block, read its column value and merge
/// it into the node's slot using the metric's rollup operator. The
/// initial slot value is the operator's identity element (set by
/// `Graph::insert_node`), so merge yields the correct value even for
/// the first attached row.
fn attach_metrics_from_source(
    graph: &mut Graph,
    src_alias: &str,
    attach_kind: &str,
    source: &SourceSpec,
    metrics_block: &indexmap::IndexMap<String, MetricSpec>,
    kind_of_level: &HashMap<String, KindId>,
    metric_slot_index: &HashMap<(String, String), usize>,
    reader: &dyn SourceReader,
) -> Result<()> {
    let attach_kind_id = *kind_of_level.get(attach_kind).ok_or_else(|| {
        anyhow!(
            "source `{src_alias}` attaches_at `{attach_kind}` but no such kind is registered",
        )
    })?;

    // Identity column on the source equals the attach kind's identifying
    // column. Phase 2 reads from the spec under the assumption that the
    // source has a column with that exact name; Phase 3 will resolve
    // through a `[[relation]]` when names differ.
    let identity_col = identity_column_for_kind(attach_kind, &graph_hierarchy_levels(graph));
    // `graph_hierarchy_levels` is a placeholder — we don't actually
    // have the LevelSpec available here, so identity_col falls back to
    // the kind's own name. That's correct when level.column == level_id.
    let identity_col = identity_col.unwrap_or_else(|| attach_kind.to_string());

    // Build the column list: identity first, then each metric's column.
    let mut columns: Vec<String> = vec![identity_col.clone()];
    let mut metric_ops: Vec<(usize, super::spec::Rollup)> = Vec::new(); // (slot_index, op)
    let mut metric_kinds: Vec<MetricKind> = Vec::new();
    for (mid, m) in metrics_block {
        let column = m.column.clone().unwrap_or_else(|| mid.clone());
        columns.push(column);
        let slot = *metric_slot_index
            .get(&(src_alias.to_string(), mid.clone()))
            .ok_or_else(|| anyhow!("metric `{src_alias}.{mid}` missing from slot index"))?;
        metric_ops.push((slot, m.rollup));
        metric_kinds.push(MetricKind::from_rollup(m.rollup));
    }

    let rows = reader.read(&source.table, &columns, source.filter.as_deref())?;
    tracing::debug!(
        source = src_alias,
        attach = attach_kind,
        rows = rows.len(),
        metrics = metrics_block.len(),
        "graph::attach_metrics_from_source read"
    );

    for row in &rows {
        attach_row(graph, attach_kind_id, &metric_ops, &metric_kinds, row)?;
    }
    Ok(())
}

/// Per-metric storage shape, derived from the rollup operator. Used by
/// `attach_row` to wrap the row's scalar/text value into the matching
/// `MetricValue` variant before calling `merge`.
#[derive(Clone, Copy)]
enum MetricKind {
    Scalar,
    SetMember,
    ListMember,
    BoolFlag,
}

impl MetricKind {
    fn from_rollup(r: super::spec::Rollup) -> Self {
        use super::spec::Rollup::*;
        match r {
            Sum | Min | Max | Count | CountDistinct | Avg => MetricKind::Scalar,
            // CountDistinct uses Set storage; the engine treats the cell
            // as a set and surfaces len() at read time. Attach builds a
            // single-element Set wrapping the row's value.
            Set => MetricKind::SetMember,
            List => MetricKind::ListMember,
            Any | All => MetricKind::BoolFlag,
        }
    }
}

fn attach_row(
    graph: &mut Graph,
    attach_kind_id: KindId,
    metric_ops: &[(usize, super::spec::Rollup)],
    metric_kinds: &[MetricKind],
    row: &Row,
) -> Result<()> {
    let identity_text = row
        .cells
        .get(0)
        .map(|c| c.as_text())
        .unwrap_or_default();
    if identity_text.is_empty() {
        // Null/empty identity column — the row attaches to nothing.
        // Skipping is safer than crashing on a single dirty row.
        return Ok(());
    }
    let identity_str = graph.intern(&identity_text);
    let node_id = match graph.find(attach_kind_id, identity_str) {
        Some(id) => id,
        None => {
            // Row refers to a node the spine never built — typically a
            // source-data inconsistency (metrics for an article that
            // doesn't appear in ph_master). Drop quietly; build still
            // produces a coherent graph for the rest of the data.
            return Ok(());
        }
    };

    for (i, (slot, op)) in metric_ops.iter().enumerate() {
        let raw = match row.cells.get(i + 1) {
            Some(c) => c,
            None => continue,
        };
        let src_value = match metric_kinds[i] {
            MetricKind::Scalar => {
                // Count rolls up as +1 per row regardless of the column's
                // value. Other scalars take the numeric value.
                let v = if matches!(*op, super::spec::Rollup::Count) {
                    1.0
                } else {
                    raw.as_f64()
                };
                MetricValue::Scalar(v)
            }
            MetricKind::SetMember => {
                let id = graph.intern(&raw.as_text());
                let mut s = indexmap::IndexSet::with_capacity(1);
                s.insert(id);
                MetricValue::Set(s)
            }
            MetricKind::ListMember => {
                let id = graph.intern(&raw.as_text());
                let mut v: smallvec::SmallVec<[super::graph::StrId; 4]> =
                    smallvec::SmallVec::new();
                v.push(id);
                MetricValue::List(v)
            }
            MetricKind::BoolFlag => {
                let b = raw.as_f64().abs() > f64::EPSILON;
                MetricValue::Bool(b)
            }
        };
        merge(*op, &mut graph.node_mut(node_id).metrics[*slot], &src_value);
    }
    Ok(())
}

/// Resolve the kind's identifying column. Phase 2 has no access to the
/// `LevelSpec` from this layer, so returns `None` and the caller falls
/// back to the kind's own name (correct when `column == level_id`).
/// Phase 3 will plumb the `HierarchySpec` map through so this can do a
/// proper `LevelSpec.key.or(LevelSpec.column)` lookup.
fn identity_column_for_kind(
    _attach_kind: &str,
    _hierarchy_levels: &(),
) -> Option<String> {
    None
}

/// Placeholder for the Phase 3 hierarchy-level lookup (see above). Kept
/// as a no-op so the call-site in `attach_metrics_from_source` doesn't
/// have to change when Phase 3 lands.
fn graph_hierarchy_levels(_graph: &Graph) -> () {}

// ──────────────────────────────────────────────────────────────────────────
// Cross-edges (bridge sources)
// ──────────────────────────────────────────────────────────────────────────

/// One side of a bridge: the kind whose nodes a row resolves to, plus
/// the column on the bridge source whose value identifies the node.
#[derive(Debug, Clone)]
struct BridgeEndpoint {
    kind: KindId,
    /// Column on the *bridge source* (`self_side` of the relation) that
    /// carries the node identifier. Distinct from the column on the
    /// hierarchy source — relations let the two sides use different
    /// names, paired positionally (Decision 25).
    bridge_column: String,
    /// Hierarchy name the kind belongs to. Used for diagnostics +
    /// dedup when multiple relations land on the same hierarchy.
    hierarchy: String,
}

/// A bridge source's plan: which two kinds it connects, by which
/// columns on the source. Phase 3-bis materializes 2-way bridges
/// (sources with edges to exactly two hierarchies); multi-way bridges
/// (3+) are valid spec but require fan-out logic that's deferred.
#[derive(Debug, Clone)]
struct BridgePlan {
    source_alias: String,
    a: BridgeEndpoint,
    b: BridgeEndpoint,
}

/// Walk every source with no `attaches_at` and figure out which two
/// hierarchies it bridges by inspecting its relations. A relation
/// counts as "bridging hierarchy H" when the relation's other side
/// references H's source AND the other side's join column is the
/// identifying column of some level in H. The bridge source's own
/// join column becomes the BridgeEndpoint's `bridge_column`.
fn detect_bridges(
    spec: &GraphSpec,
    sources_by_alias: &HashMap<String, &SourceSpec>,
    kind_of_level: &HashMap<String, KindId>,
) -> Vec<BridgePlan> {
    // Build (hierarchy_source_alias, identifying_column) → (KindId,
    // hierarchy_name) so we can resolve a relation's other side to a
    // specific kind. Match by `key` if set on the level, else by
    // `column`.
    let mut alias_col_to_kind: HashMap<(String, String), (KindId, String)> = HashMap::new();
    for (hname, h) in &spec.hierarchy {
        for (level_id, level) in &h.levels {
            if let Some(kid) = kind_of_level.get(level_id) {
                let id_col = level.key.clone().unwrap_or_else(|| level.column.clone());
                alias_col_to_kind.insert(
                    (h.source.clone(), id_col),
                    (*kid, hname.clone()),
                );
            }
        }
    }

    let mut plans: Vec<BridgePlan> = Vec::new();

    for source in &spec.sources {
        if source.attaches_at.is_some() {
            continue;
        }
        // Collect one BridgeEndpoint per hierarchy this source reaches.
        // Use IndexMap so the endpoint order matches relation declaration
        // order — keeps multi-relation bridges deterministic across
        // builds, which helps when the same input data should produce
        // identical CrossEdgeId assignments.
        let mut by_hierarchy: indexmap::IndexMap<String, BridgeEndpoint> =
            indexmap::IndexMap::new();

        for r in &spec.relations {
            let (self_side, other_side) = if r.from.alias == source.alias {
                (&r.from, &r.to)
            } else if r.to.alias == source.alias {
                (&r.to, &r.from)
            } else {
                continue;
            };

            // The other side must be a hierarchy source AND its first
            // join column must match a level's identifying column.
            // Composite-column joins (Decision 18) extend this; Phase
            // 3-bis takes the first column only.
            let other_col = match other_side.columns.first() {
                Some(c) => c.clone(),
                None => continue,
            };
            let (kid, hname) =
                match alias_col_to_kind.get(&(other_side.alias.clone(), other_col)) {
                    Some(t) => t.clone(),
                    None => continue,
                };

            let self_col = self_side.columns.first().cloned().unwrap_or_default();
            if self_col.is_empty() {
                continue;
            }

            // First relation wins per hierarchy — later ones to the
            // same hierarchy are likely refining the same join and
            // would over-count edges if we processed both.
            by_hierarchy.entry(hname.clone()).or_insert(BridgeEndpoint {
                kind: kid,
                bridge_column: self_col,
                hierarchy: hname,
            });
        }

        // Phase 3-bis: only 2-way bridges. A source bridging 3+
        // hierarchies is a valid spec; the runtime needs fan-out
        // logic before it can produce edges for it (cartesian or
        // per-pair?), so we surface the case via tracing and skip.
        if by_hierarchy.len() < 2 {
            continue;
        }
        if by_hierarchy.len() > 2 {
            tracing::warn!(
                source = source.alias,
                hierarchies = by_hierarchy.len(),
                "graph::detect_bridges: 3+ hierarchy bridges are deferred; skipping"
            );
            continue;
        }

        let mut iter = by_hierarchy.into_iter();
        let (_, a) = iter.next().unwrap();
        let (_, b) = iter.next().unwrap();
        plans.push(BridgePlan { source_alias: source.alias.clone(), a, b });
    }

    plans
}

/// Read each bridge source's rows and write one cross-edge per row.
/// Registers a fresh `CrossEdgeId` per bridge so callers can ask
/// "which bridge produced this edge" via `CrossEdgeMeta`.
fn materialize_cross_edges(
    graph: &mut Graph,
    bridges: &[BridgePlan],
    sources_by_alias: &HashMap<String, &SourceSpec>,
    reader: &dyn SourceReader,
) -> Result<()> {
    for plan in bridges {
        let source = match sources_by_alias.get(&plan.source_alias) {
            Some(s) => *s,
            None => continue,
        };
        let columns = vec![plan.a.bridge_column.clone(), plan.b.bridge_column.clone()];
        let rows = reader.read(&source.table, &columns, source.filter.as_deref())?;
        tracing::debug!(
            source = plan.source_alias,
            rows = rows.len(),
            from_kind = plan.a.hierarchy,
            to_kind = plan.b.hierarchy,
            "graph::materialize_cross_edges read"
        );

        let edge_id = graph.cross_edges.register(CrossEdgeMeta {
            kind_a: plan.a.kind,
            kind_b: plan.b.kind,
            bridge_source: plan.source_alias.clone(),
        });

        for row in &rows {
            let val_a = row.cells.get(0).map(|c| c.as_text()).unwrap_or_default();
            let val_b = row.cells.get(1).map(|c| c.as_text()).unwrap_or_default();
            if val_a.is_empty() || val_b.is_empty() {
                continue;
            }
            // intern() takes &mut self; we capture both StrIds before
            // borrowing cross_edges mutably to keep the borrow non-
            // overlapping with the immutable `find()` calls below.
            let str_a = graph.intern(&val_a);
            let str_b = graph.intern(&val_b);
            let Some(node_a) = graph.find(plan.a.kind, str_a) else {
                continue;
            };
            let Some(node_b) = graph.find(plan.b.kind, str_b) else {
                continue;
            };
            let idx = graph.cross_edges.get_mut(edge_id);
            idx.forward.entry(node_a).or_default().push(node_b);
            idx.reverse.entry(node_b).or_default().push(node_a);
        }
    }
    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::source::CellValue;
    use std::cell::RefCell;

    /// In-memory `SourceReader` for tests. Owns a `table → rows`
    /// dictionary; each `read` returns the rows projected to the
    /// requested columns. No SQL synthesis, no DuckDB.
    struct MockReader {
        // RefCell so `read` (immutable self) can record call counts
        // without forcing the trait to take `&mut self`.
        tables: HashMap<String, MockTable>,
        reads: RefCell<usize>,
    }

    struct MockTable {
        columns: Vec<String>,
        rows: Vec<Vec<CellValue>>,
    }

    impl SourceReader for MockReader {
        fn read(
            &self,
            table: &str,
            columns: &[String],
            _filter: Option<&str>,
        ) -> Result<Vec<Row>> {
            *self.reads.borrow_mut() += 1;
            let t = self
                .tables
                .get(table)
                .ok_or_else(|| anyhow!("mock: no table `{table}`"))?;
            // Project requested columns by name. Missing column → Null.
            let col_idx: Vec<Option<usize>> =
                columns.iter().map(|c| t.columns.iter().position(|x| x == c)).collect();
            Ok(t.rows
                .iter()
                .map(|raw| {
                    let cells = col_idx
                        .iter()
                        .map(|idx| match idx {
                            Some(i) => raw[*i].clone(),
                            None => CellValue::Null,
                        })
                        .collect();
                    Row { cells }
                })
                .collect())
        }
    }

    fn ts(s: &str) -> CellValue { CellValue::Text(s.to_string()) }
    fn ti(i: i64) -> CellValue { CellValue::Int(i) }

    /// Smallest end-to-end build: 1 hierarchy (product, levels l0/article),
    /// 1 source (ph_master) for the spine, 1 metric source (inv) attached
    /// at `article`. Verifies kind counts, articles' attached values, and
    /// rollup totals at l0 and root.
    #[test]
    fn end_to_end_simple_graph() {
        let toml = r#"
id = "g"
display_name = "G"

[[sources]]
alias = "ph_master"
table = "ph_master"
attaches_at = "article"

[[sources]]
alias = "inv"
table = "inv"
attaches_at = "article"

[hierarchy.product]
source = "ph_master"

[hierarchy.product.l0]
column = "l0"

[hierarchy.product.article]
column = "article"

[metrics.inv]
oh = { rollup = "sum" }
"#;
        let spec = crate::graph::spec::from_toml(toml).unwrap();

        let mut tables = HashMap::new();
        tables.insert(
            "ph_master".to_string(),
            MockTable {
                columns: vec!["l0".to_string(), "article".to_string()],
                rows: vec![
                    vec![ts("L0_A"), ts("A1")],
                    vec![ts("L0_A"), ts("A2")],
                    vec![ts("L0_B"), ts("B1")],
                ],
            },
        );
        tables.insert(
            "inv".to_string(),
            MockTable {
                columns: vec!["article".to_string(), "oh".to_string()],
                rows: vec![
                    vec![ts("A1"), ti(10)],
                    vec![ts("A2"), ti(25)],
                    vec![ts("B1"), ti(7)],
                ],
            },
        );
        let reader = MockReader { tables, reads: RefCell::new(0) };

        let (g, stats) = build_graph(&spec, &reader, 1).expect("build");

        assert_eq!(stats.primary_metric_count, 1);
        // 1 root + 2 l0 nodes + 3 article nodes = 6.
        assert_eq!(stats.total_nodes, 6);

        let art_kind = g.kinds.id_of("article").unwrap();
        let l0_kind = g.kinds.id_of("l0").unwrap();
        assert_eq!(g.count_kind(art_kind), 3);
        assert_eq!(g.count_kind(l0_kind), 2);

        // L0_A rollup = 10 + 25 = 35; L0_B = 7; root = 42.
        let l0_a = g.find(l0_kind, find_strid(&g, "L0_A")).unwrap();
        let l0_b = g.find(l0_kind, find_strid(&g, "L0_B")).unwrap();
        assert!(matches!(g.node(l0_a).metrics[0], MetricValue::Scalar(v) if (v - 35.0).abs() < 1e-9));
        assert!(matches!(g.node(l0_b).metrics[0], MetricValue::Scalar(v) if (v - 7.0).abs() < 1e-9));
        assert!(matches!(g.node(g.root).metrics[0], MetricValue::Scalar(v) if (v - 42.0).abs() < 1e-9));
    }

    /// Helper: find a `StrId` by string content. Build phase clears the
    /// reverse index (`finalize_strings`), so this re-scans the pool.
    /// Cheap enough for tests; production reads come in with `StrId`s
    /// already in hand.
    fn find_strid(g: &Graph, s: &str) -> crate::graph::graph::StrId {
        let i = g
            .string_pool
            .iter()
            .position(|a| a.as_ref() == s)
            .expect("string not interned");
        crate::graph::graph::StrId(i as u32)
    }

    /// Bridge source materializes cross-edges between two hierarchies.
    ///
    /// Setup: two single-level hierarchies (`product → article` and
    /// `brand → brand`), sourced from `products_tbl` and `brands_tbl`.
    /// A bridge source `pb_link` (table `pb_link_tbl`) has rows
    /// `(article_id, brand_id)` and two relations — one to each
    /// hierarchy's spine source. After build, `graph.cross_edges`
    /// should contain a single edge type whose forward map yields
    /// each article's brand and whose reverse map collects articles
    /// per brand.
    #[test]
    fn bridge_source_produces_cross_edges() {
        let toml = r#"
id = "g"
display_name = "G"

[[sources]]
alias = "products"
table = "products_tbl"
attaches_at = "article"

[[sources]]
alias = "brands"
table = "brands_tbl"
attaches_at = "brand"

[[sources]]
alias = "pb_link"
table = "pb_link_tbl"
# No attaches_at -> bridge.

[[relation]]
from = { alias = "pb_link",  columns = ["article_id"], cardinality = "*" }
to   = { alias = "products", columns = ["article"],    cardinality = "1" }

[[relation]]
from = { alias = "pb_link", columns = ["brand_id"],   cardinality = "*" }
to   = { alias = "brands",  columns = ["brand_name"], cardinality = "1" }

[hierarchy.product]
source = "products"

[hierarchy.product.article]
column = "article"

[hierarchy.brand]
source = "brands"

[hierarchy.brand.brand]
column = "brand_name"
"#;
        let spec = crate::graph::spec::from_toml(toml).unwrap();

        let mut tables = HashMap::new();
        tables.insert(
            "products_tbl".to_string(),
            MockTable {
                columns: vec!["article".to_string()],
                rows: vec![vec![ts("A1")], vec![ts("A2")], vec![ts("A3")]],
            },
        );
        tables.insert(
            "brands_tbl".to_string(),
            MockTable {
                columns: vec!["brand_name".to_string()],
                rows: vec![vec![ts("B1")], vec![ts("B2")]],
            },
        );
        tables.insert(
            "pb_link_tbl".to_string(),
            MockTable {
                columns: vec!["article_id".to_string(), "brand_id".to_string()],
                rows: vec![
                    vec![ts("A1"), ts("B1")],
                    vec![ts("A2"), ts("B1")],
                    vec![ts("A3"), ts("B2")],
                ],
            },
        );
        let reader = MockReader { tables, reads: RefCell::new(0) };

        let (g, _stats) = build_graph(&spec, &reader, 1).expect("build");

        // Exactly one cross-edge type registered (article ↔ brand).
        assert_eq!(g.cross_edges.metas.len(), 1);
        let edge_meta = &g.cross_edges.metas[0];
        let art_kind = g.kinds.id_of("article").unwrap();
        let brand_kind = g.kinds.id_of("brand").unwrap();
        // Endpoint kinds match — order depends on declaration sequence
        // (`product` first → article is `kind_a`, brand `kind_b`).
        assert_eq!(edge_meta.kind_a, art_kind);
        assert_eq!(edge_meta.kind_b, brand_kind);

        let idx = &g.cross_edges.indices[0];

        // Forward: article → [brand].
        let a1 = g.find(art_kind, find_strid(&g, "A1")).unwrap();
        let a2 = g.find(art_kind, find_strid(&g, "A2")).unwrap();
        let a3 = g.find(art_kind, find_strid(&g, "A3")).unwrap();
        let b1 = g.find(brand_kind, find_strid(&g, "B1")).unwrap();
        let b2 = g.find(brand_kind, find_strid(&g, "B2")).unwrap();
        assert_eq!(idx.forward.get(&a1).unwrap().as_slice(), &[b1]);
        assert_eq!(idx.forward.get(&a2).unwrap().as_slice(), &[b1]);
        assert_eq!(idx.forward.get(&a3).unwrap().as_slice(), &[b2]);

        // Reverse: brand → [article…] (B1 has A1+A2, B2 has A3).
        let b1_articles = idx.reverse.get(&b1).unwrap();
        assert_eq!(b1_articles.len(), 2);
        assert!(b1_articles.contains(&a1));
        assert!(b1_articles.contains(&a2));
        assert_eq!(idx.reverse.get(&b2).unwrap().as_slice(), &[a3]);
    }

    /// Verifies `split = "|"` at a leaf level: one source row with a
    /// pipe-delimited column produces N sibling leaf nodes under the
    /// row's article. Mirrors the bealls product_codes case.
    #[test]
    fn split_level_expands_into_sibling_leaves() {
        let toml = r#"
id = "g"
display_name = "G"

[[sources]]
alias = "ph_master"
table = "ph_master"
attaches_at = "article"

[hierarchy.product]
source = "ph_master"

[hierarchy.product.article]
column = "article"

[hierarchy.product.product_code]
column = "product_codes"
split = "|"
"#;
        let spec = crate::graph::spec::from_toml(toml).unwrap();

        let mut tables = HashMap::new();
        tables.insert(
            "ph_master".to_string(),
            MockTable {
                columns: vec!["article".to_string(), "product_codes".to_string()],
                rows: vec![
                    vec![ts("A1"), ts("P1|P2|P3")],
                    vec![ts("A2"), ts("P4")],
                    // Empty cell — collapses to a single empty-string leaf
                    // so the article still has a product_code child.
                    vec![ts("A3"), ts("")],
                ],
            },
        );
        let reader = MockReader { tables, reads: RefCell::new(0) };

        let (g, _stats) = build_graph(&spec, &reader, 1).expect("build");

        let art_kind = g.kinds.id_of("article").unwrap();
        let pc_kind = g.kinds.id_of("product_code").unwrap();
        assert_eq!(g.count_kind(art_kind), 3);
        // P1, P2, P3, P4, "" → 5 distinct product_code leaves.
        assert_eq!(g.count_kind(pc_kind), 5);

        // A1 should have three product_code children (P1, P2, P3).
        let a1 = g.find(art_kind, find_strid(&g, "A1")).unwrap();
        assert_eq!(g.node(a1).children.len(), 3);
    }
}
