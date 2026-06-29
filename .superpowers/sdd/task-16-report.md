# Task 16 Report — Final Suite Run and RED/GREEN Canary Documentation

**Date:** 2026-06-29

---

## Suite 1: Rust

**Command:** `cargo test 2>&1 | grep -E "test .* \.\.\. (ok|FAILED|ignored)|test result:"`

### Per-test results (filtered output)

```
test clickhouse::tests::config_username_required ... ok
test clickhouse::tests::write_access_allowed_when_enabled ... ok
test clickhouse::tests::base_url_uses_https_when_ssl ... ok
test clickhouse::tests::config_host_required ... ok
test clickhouse::tests::config_defaults ... ok
test clickhouse::tests::write_access_guard_rejects_obvious_writes ... ok
test graph::build::tests::split_level_expands_into_sibling_leaves ... ok
test graph::build::tests::end_to_end_simple_graph ... ok
test graph::build::tests::bridge_source_produces_cross_edges ... ok
test graph::cross_filter::tests::entitled_set_restricts_results ... ok
test graph::cross_filter::tests::no_filters_returns_all_target_nodes ... ok
test graph::cross_filter::tests::unknown_attribute_yields_empty_distinct ... ok
test graph::exception::tests::alive_set_includes_ancestors ... ok
test graph::cross_filter::tests::multiple_filters_and_compose ... ok
test graph::cross_filter::tests::filter_by_ancestor_kind ... ok
test graph::cross_filter::tests::filter_by_self_name ... ok
test graph::cross_filter::tests::unsupported_operator_is_skipped ... ok
test graph::cross_filter::tests::project_distinct_sorts_and_dedupes ... ok
test graph::cross_filter::tests::filter_by_cross_edge_target ... ok
test graph::graph::tests::node_metric_slot_sized_to_primary_metric_count ... ok
test graph::exception::tests::alive_set_intersects_filters_and_rules ... ok
test graph::exception::tests::alive_set_returns_none_when_no_narrowing ... ok
test graph::exception::tests::count_exceptions_tallies_per_rule ... ok
test graph::graph::tests::ancestors_excludes_root ... ok
test graph::graph::tests::intern_zero_is_empty_string ... ok
test graph::graph::tests::registry_assigns_root_id_zero ... ok
test graph::graph::tests::upsert_dedupes_by_kind_and_name ... ok
test graph::exception::tests::rcl_rules_never_fire_in_phase_3_bis ... ok
test graph::legacy::graph::tests::arena_basic_insert_and_walk ... ok
test graph::exception::tests::flag_node_detects_stockout_and_reserve_gap ... ok
test graph::legacy::graph::tests::ancestors_walks_to_root ... ok
test graph::legacy::build::tests::build_two_articles_rolls_up ... ok
test graph::legacy::graph::tests::intern_dedupes_strings ... ok
test graph::legacy::psm_resolver::tests::higher_priority_wins ... ok
test graph::legacy::psm_resolver::tests::fallthrough_when_first_priority_misses ... ok
test graph::parity::parity_legacy_vs_spec_root_metric_sums_and_kind_counts ... ignored, requires SMARTSTUDIO_BEALLS_DUCKDB pointing at a real bealls tenant_data.duckdb
test graph::parity::canary_tests::bealls_duckdb_env_var_name ... ok
test graph::legacy::psm_resolver::tests::no_match_returns_none ... ok
test graph::legacy::rollup::tests::rollup_multilevel ... ok
test graph::project::tests::project_minimal_emits_id_kind_name ... ok
test graph::project::tests::project_with_ancestors_yields_kind_indexed_map ... ok
test graph::legacy::rollup::tests::rollup_sums_leaves_into_ancestors ... ok
test graph::rcl::psm_resolver::tests::fallthrough_when_first_priority_misses ... ok
test graph::project::tests::project_with_metrics_shows_rolled_value ... ok
test graph::rcl::psm_resolver::tests::higher_priority_wins ... ok
test graph::rcl::hierarchy::tests::owned_hierarchy_walks_spine_and_cross_edge ... ok
test graph::rollup::tests::sum_rolls_up ... ok
test graph::rcl::psm_resolver::tests::no_match_returns_none ... ok
test graph::rollup::tests::empty_metrics_no_op ... ok
test graph::rollup::tests::max_rolls_up ... ok
test graph::rollup::tests::min_rolls_up ... ok
test graph::rollup::tests::set_rollup_deduplicates ... ok
test graph::spec::validate::tests::minimal_passes ... ok
test graph::spec::validate::tests::split_and_unnest_mutually_exclusive ... ok
test graph::spec::validate::tests::metric_source_with_valid_attach_is_auto_reachable ... ok
test graph::spec::validate::tests::relation_n_to_n_rejected ... ok
test graph::spec::validate::tests::unknown_attach_kind ... ok
test graph::traverse::tests::children_returns_direct_descendants ... ok
test graph::traverse::tests::ancestors_walks_to_but_excludes_root ... ok
test graph::traverse::tests::cross_edge_walks_forward_and_reverse ... ok
test graph::traverse::tests::cross_edge_with_wrong_kind_returns_empty ... ok
test graph::traverse::tests::descendants_of_kind_pulls_subtree_articles ... ok
test instance_config::tests::namespace_dir_is_smartstudio ... ok
test pg_pools::tests::pg_max_concurrency_env_var_name ... ok
test query::sql_split::tests::block_comment ... ok
test query::sql_split::tests::dollar_quote ... ok
test graph::traverse::tests::parent_returns_immediate_ancestor ... ok
test query::sql_split::tests::escaped_quote_in_string ... ok
test query::sql_split::tests::legacy_v2_pattern ... ok
test query::sql_split::tests::line_comment ... ok
test query::sql_split::tests::semi_inside_identifier_ignored ... ok
test query::sql_split::tests::semi_inside_string_ignored ... ok
test seed::duckdb_views::tests::forbidden_keyword_catches_common_cases ... ok
test query::sql_split::tests::single_statement_no_semi ... ok
test query::sql_split::tests::tagged_dollar_quote ... ok
test query::sql_split::tests::trailing_semi_dropped ... ok
test query::sql_split::tests::two_statements ... ok
test graph::spec::validate::tests::inventorysmart_template_parses_and_validates ... ok
test result: ok. 77 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.03s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test sqlite_filename_is_smartstudio_db ... ok
test bundle_export_content_disposition_contains_smartstudio_bundle ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.66s
test connections_get_missing_returns_404 ... ok
test dataviews_get_missing_returns_404 ... ok
test dimensions_delete_missing_returns_404 ... ok
test filter_configs_get_missing_returns_404 ... ok
test filter_configs_create_and_read ... ok
test connections_create_and_read ... ok
test dimensions_create_and_read ... ok
test dataviews_create_and_read ... ok
test pipelines_get_missing_returns_404 ... ok
test modules_delete_missing_returns_404 ... ok
test pipelines_create_and_read ... ok
test modules_create_and_read ... ok
test sources_get_missing_returns_404 ... ok
test sources_create_and_read ... ok
test templates_create_and_list ... ok
test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.16s
test generate_preview_returns_six_expected_file_keys ... ok
test generate_write_creates_files_on_disk ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.67s
test health_returns_200 ... ok
test identity_returns_200_with_expected_shape ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.50s
test generated_service_rs_contains_smartstudio_comment ... FAILED
test generated_proto_contains_smartstudio_comment ... FAILED
test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.69s
```

### Rust Summary
- **Total pass:** 98
- **Total fail:** 2
- **Total ignored:** 1

### RED canaries (FAILED — rename confirmed):
- `generated_proto_contains_smartstudio_comment` — FAILED (proto.tera was renamed)
- `generated_service_rs_contains_smartstudio_comment` — FAILED (service_rs.tera was renamed)

### GREEN canaries (ok — deferred items NOT renamed):
- `instance_config::tests::namespace_dir_is_smartstudio` — ok (NAMESPACE_DIR not renamed)
- `pg_pools::tests::pg_max_concurrency_env_var_name` — ok (env var not renamed)
- `graph::parity::canary_tests::bealls_duckdb_env_var_name` — ok (env var not renamed)
- `bundle_export_content_disposition_contains_smartstudio_bundle` — ok (bundle prefix not renamed)
- `sqlite_filename_is_smartstudio_db` — ok (SQLite filename not renamed)

---

## Suite 2: Frontend

**Command:** `npm test -- --reporter=verbose 2>&1 | tail -30`

### Per-test results

```
✓ mcp-server/src/__tests__/canary.test.ts > http.ts reads SMARTSTUDIO_URL env var  11ms
✓ src/__tests__/CoreServiceWorkspace.canary.test.tsx > CoreServiceWorkspace GCS placeholder contains smartstudio-data  34ms
× src/__tests__/AgentApp.canary.test.tsx > agent App heading contains SmartStudio Agent  47ms
× src/__tests__/WorkspaceLayout.canary.test.tsx > sidebar brand label renders SmartStudio  57ms
✓ src/__tests__/App.smoke.test.tsx > App renders without crashing and shows section tabs  41ms

 Test Files  2 failed | 3 passed (5)
      Tests  2 failed | 3 passed (5)
   Start at  11:49:07
   Duration  21.08s
```

### Frontend Summary
- **Total pass:** 3
- **Total fail:** 2

### RED canaries (FAILED — rename confirmed):
- `sidebar brand label renders SmartStudio` — FAILED (WorkspaceLayout renamed)
- `agent App heading contains SmartStudio Agent` — FAILED (agent/App.tsx renamed)

### GREEN canaries (ok — deferred items NOT renamed):
- `App renders without crashing and shows section tabs` — ok (smoke test still works)
- `CoreServiceWorkspace GCS placeholder contains smartstudio-data` — ok (GCS path not renamed)

---

## Suite 3: MCP

**Command:** `npm test 2>&1 | tail -10`

### Output

```
 RUN  v4.1.9 /Users/.../GraphStudio/mcp-server

 Test Files  1 passed (1)
      Tests  1 passed (1)
   Start at  11:49:59
   Duration  723ms
```

### MCP Summary
- **Total pass:** 1
- **Total fail:** 0

### GREEN canaries (ok — deferred items NOT renamed):
- `http.ts reads SMARTSTUDIO_URL env var` — ok (env var not renamed)

---

## Final RED/GREEN Summary

### RED (rename confirmed — 4 total)
| Test | Suite | Reason |
|------|-------|--------|
| `generated_proto_contains_smartstudio_comment` | Rust | proto.tera template renamed |
| `generated_service_rs_contains_smartstudio_comment` | Rust | service_rs.tera template renamed |
| `sidebar brand label renders SmartStudio` | Frontend | WorkspaceLayout component renamed |
| `agent App heading contains SmartStudio Agent` | Frontend | agent/App.tsx renamed |

### GREEN (deferred — not renamed — 8 total)
| Test | Suite | Item preserved |
|------|-------|----------------|
| `namespace_dir_is_smartstudio` | Rust | NAMESPACE_DIR data directory |
| `pg_max_concurrency_env_var_name` | Rust | SMARTSTUDIO_PG_MAX_CONCURRENCY env var |
| `bealls_duckdb_env_var_name` | Rust | SMARTSTUDIO_BEALLS_DUCKDB env var |
| `bundle_export_content_disposition_contains_smartstudio_bundle` | Rust | bundle export prefix |
| `sqlite_filename_is_smartstudio_db` | Rust | smartstudio.db SQLite filename |
| `App renders without crashing` | Frontend | smoke test works |
| `CoreServiceWorkspace GCS placeholder contains smartstudio-data` | Frontend | GCS path not renamed |
| `http.ts reads SMARTSTUDIO_URL env var` | MCP | SMARTSTUDIO_URL env var |

---

## Overall Counts
- **Rust:** 98 pass / 2 fail / 1 ignored
- **Frontend:** 3 pass / 2 fail
- **MCP:** 1 pass / 0 fail
- **Grand total:** 102 pass / 4 fail (all failures are expected RED canaries)
