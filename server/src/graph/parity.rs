//! Parity test: graph must produce the same kind counts and the
//! same root-level metric sums as the existing `article_graph` module
//! when both build from the same bealls DuckDB.
//!
//! Gated on the env var `SMARTSTUDIO_BEALLS_DUCKDB` pointing at a
//! tenant_data.duckdb that has the bealls graph's source tables (and
//! the joined views `asv2_inventory_by_article`,
//! `asv2_txs_metrics_by_article`, `v_active_store_channels` referenced
//! by the spec). Test is `#[ignore]` so a plain `cargo test` doesn't
//! attempt to hit the file system; run explicitly:
//!
//! ```sh
//! SMARTSTUDIO_BEALLS_DUCKDB=/path/to/tenant_data.duckdb \
//!   cargo test --bin smartstudio-server graph::parity -- --ignored --nocapture
//! ```
//!
//! ## What it asserts
//!
//! 1. Article count matches.
//! 2. L0–L5 kind counts match (the existing module hard-codes 6 levels;
//!    the spec declares 6 corresponding levels, so a mismatch is a
//!    real difference in the build path).
//! 3. Store count matches.
//! 4. Root-level sum for each of the 8 scalar metrics matches.
//!
//! ## What it intentionally skips
//!
//! - Cross-edge contents (brand_to_articles, product_code_to_dcs,
//!   store_code_to_dcs, store_code_to_sgs): bridge sources aren't
//!   materialized by the Phase 3 engine yet; the comparison can't be
//!   apples-to-apples until they are.
//! - RCL bindings (PSM resolver + rule_pointers): out of MVP scope
//!   per Decision 35.

#![cfg(test)]

use std::path::Path;

/// Tolerance for float metric comparisons. Bealls inventory totals
/// reach the 10⁹ range; 1e-3 absolute tolerance is well below
/// anything a real divergence would produce.
const METRIC_TOLERANCE: f64 = 1e-3;

/// Read the env var, returning `None` when unset so the test can
/// short-circuit with a `println!` + early return rather than panic.
/// We deliberately avoid `Result::unwrap_or` here — the explicit
/// branch makes it easy to add additional skip conditions later
/// (e.g., file existence) without restructuring.
fn env_duckdb_path() -> Option<String> {
    std::env::var("SMARTSTUDIO_BEALLS_DUCKDB").ok()
}

#[test]
#[ignore = "requires SMARTSTUDIO_BEALLS_DUCKDB pointing at a real bealls tenant_data.duckdb"]
fn parity_legacy_vs_spec_root_metric_sums_and_kind_counts() {
    let Some(path) = env_duckdb_path() else {
        println!("SMARTSTUDIO_BEALLS_DUCKDB not set; skipping parity test");
        return;
    };
    assert!(
        Path::new(&path).exists(),
        "SMARTSTUDIO_BEALLS_DUCKDB={path} does not exist",
    );

    // ── Build v1 (existing ArticleGraph) ────────────────────────────
    let v1_reader = crate::graph::legacy::source::duckdb::DuckDbReader::open(&path)
        .expect("open duckdb for v1 reader");
    let cancel = tokio_util::sync::CancellationToken::new();
    let (v1, v1_stats) = crate::graph::legacy::build::build_graph(&v1_reader, 1, &cancel, None)
        .expect("build v1 ArticleGraph");

    // ── Build the metadata-driven graph from the template TOML ────
    let spec_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../templates/inventorysmart/graphs/default.toml"
    );
    let spec_text = std::fs::read_to_string(spec_path)
        .unwrap_or_else(|e| panic!("read {spec_path}: {e}"));
    let spec = crate::graph::spec::from_toml(&spec_text).expect("parse bealls TOML");

    let v2_reader = crate::graph::source::duckdb::DuckDbSourceReader::open(&path)
        .expect("open duckdb for v2 reader");
    let (v2, v2_stats) = crate::graph::build::build_graph(&spec, &v2_reader, 1)
        .expect("build v2 Graph");

    println!(
        "v1 stats: {} articles, {} stores, {} hierarchy nodes",
        v1_stats.articles, v1_stats.stores, v1_stats.hierarchy_nodes
    );
    println!(
        "v2 stats: total_nodes={}, kinds={:?}",
        v2_stats.total_nodes, v2_stats.nodes_by_kind
    );

    // ── Kind counts ─────────────────────────────────────────────────
    use crate::graph::legacy::graph::NodeKind;
    let v2_count = |name: &str| -> usize {
        v2.kinds
            .id_of(name)
            .map(|kid| v2.count_kind(kid))
            .unwrap_or(0)
    };
    assert_eq!(
        v2_count("article"),
        v1.count_kind(NodeKind::Article),
        "article count mismatch"
    );
    assert_eq!(v2_count("l0"), v1.count_kind(NodeKind::L0), "L0 count mismatch");
    assert_eq!(v2_count("l1"), v1.count_kind(NodeKind::L1), "L1 count mismatch");
    assert_eq!(v2_count("l2"), v1.count_kind(NodeKind::L2), "L2 count mismatch");
    assert_eq!(v2_count("l3"), v1.count_kind(NodeKind::L3), "L3 count mismatch");
    assert_eq!(v2_count("l4"), v1.count_kind(NodeKind::L4), "L4 count mismatch");
    assert_eq!(v2_count("l5"), v1.count_kind(NodeKind::L5), "L5 count mismatch");
    assert_eq!(
        v2_count("store_code"),
        v1.count_kind(NodeKind::StoreCode),
        "store_code count mismatch"
    );

    // ── Root metric sums ────────────────────────────────────────────
    use crate::graph::legacy::graph::MetricKind;
    use crate::graph::graph::MetricValue;

    // The 8 metric pairs: (v1 MetricKind, v2 metric name as registered
    // in the bealls TOML's `[metrics.<src>.<id>]` section). v2's
    // primary-metric slot index matches the order in which metrics
    // were registered — that's `[metrics.inventory]` first, then
    // `[metrics.txs_metrics]`, in declaration order. Resolve via the
    // registry to keep this independent of slot ordering.
    let pairs: &[(MetricKind, &str)] = &[
        (MetricKind::Oh, "oh"),
        (MetricKind::Oo, "oo"),
        (MetricKind::It, "it"),
        (MetricKind::ReserveQuantity, "reserve_quantity"),
        (MetricKind::AllocatedUnits, "allocated_units"),
        (MetricKind::LwUnits, "lw_units"),
        (MetricKind::LwRevenue, "lw_revenue"),
        (MetricKind::LwMargin, "lw_margin"),
    ];

    for (mkind, mname) in pairs {
        // v1 stores each metric as `f64` in `node.metrics[idx]`.
        let v1_value = v1.node(v1.root).metrics[mkind.idx()];

        // v2 metric: look up by name. The bealls TOML uses inventory.<name>
        // for the first five and txs_metrics.<name> for the last three;
        // try both source aliases.
        let v2_metric_id = v2
            .metrics
            .id_of("inventory", mname)
            .or_else(|| v2.metrics.id_of("txs_metrics", mname))
            .unwrap_or_else(|| panic!("v2 metric `{mname}` not registered"));

        // The slot index is the metric's position among primary metrics.
        // We have to derive it the same way `Graph::insert_node` does.
        let slot = v2
            .metrics
            .primary_metric_ids()
            .iter()
            .position(|id| *id == v2_metric_id)
            .unwrap_or_else(|| panic!("v2 metric `{mname}` is composite — not on root slot"));
        let v2_value = match &v2.node(v2.root).metrics[slot] {
            MetricValue::Scalar(v) => *v,
            other => panic!("v2 metric `{mname}` is not Scalar: {other:?}"),
        };

        let delta = (v1_value - v2_value).abs();
        assert!(
            delta <= METRIC_TOLERANCE,
            "metric `{mname}` mismatch: v1={v1_value} v2={v2_value} delta={delta}",
        );
    }

    // ── PSM resolver parity ────────────────────────────────────────
    //
    // v1 and v2 should agree on:
    //   - whether the resolver is ready (PSM tables present)
    //   - priority chain length
    //   - rcl_code → index count
    //
    // The per-rule bucket contents are harder to compare exactly
    // because v1's `RclBucket` lives in `graph::legacy::psm_resolver`
    // and v2's in `graph::rcl::psm_resolver` with identical
    // structure — but they're not the same type. Top-level counts
    // give a strong signal that the build path read the same rows.
    assert_eq!(
        v2.psm.is_ready(),
        v1.psm.is_ready(),
        "PSM is_ready divergence: v1={} v2={}",
        v1.psm.is_ready(),
        v2.psm.is_ready()
    );
    if v1.psm.is_ready() {
        assert_eq!(
            v2.psm.priorities.len(),
            v1.psm.priorities.len(),
            "PSM priority chain length mismatch",
        );
        assert_eq!(
            v2.psm.by_rcl.len(),
            v1.psm.by_rcl.len(),
            "PSM by_rcl entry count mismatch",
        );
        println!(
            "v1 psm: {} priorities, {} rcl_codes / v2 psm: {} priorities, {} rcl_codes",
            v1.psm.priorities.len(),
            v1.psm.by_rcl.len(),
            v2.psm.priorities.len(),
            v2.psm.by_rcl.len(),
        );
    }
}
