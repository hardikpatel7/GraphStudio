//! `UamStore` — cold-load + lookup.
//!
//! Loaded once at boot via `cold_load`. Reads
//! `global.user_access_hierarchy_mapping` directly (independent of the
//! V7 extracts pipeline so UAM doesn't depend on the article_selection
//! pipeline graph being current). Each row's `filters` jsonb is
//! parsed into our [`crate::cross_filter::Filter`] shape and resolved
//! against the live graph via [`crate::cross_filter::apply_filters`]
//! to produce a concrete article NodeId set.
//!
//! Phase A: cold-load only. CDC-driven refresh lands in Phase B.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use arc_swap::ArcSwap;
use tokio_postgres::NoTls;

use crate::graph::legacy::ArticleGraph;
use crate::cross_filter::{apply_filters, EntitledSet, Filter};

/// Composite key matching the upstream
/// `user_access_hierarchy_mapping_un` unique constraint
/// `(user_code, acl_code)`.
pub type UamLookupKey = (i32, i32);

/// In-process UAM store. Reads are lock-free via the inner Arc; the
/// only mutator is `cold_load`, which builds a new map and swaps it
/// in atomically.
pub struct UamStore {
    /// Live entitlements snapshot. `None` for a (user, acl) pair
    /// means the row had empty filters (= unrestricted access).
    /// Absent from the map entirely = the user has no row in
    /// user_access_hierarchy_mapping for that acl_code; treat as
    /// unauthorized.
    inner: ArcSwap<HashMap<UamLookupKey, Arc<EntitlementEntry>>>,
}

/// One row of the UAM table after resolution.
#[derive(Debug, Clone)]
pub struct EntitlementEntry {
    pub user_code: i32,
    pub acl_code: i32,
    /// `None` = unrestricted access. `Some(set)` = candidate articles
    /// the user is allowed to see — pre-resolved against the v1
    /// `ArticleGraph` at cold-load time.
    pub entitled: Option<EntitledSet>,
    /// How many filter expressions the row carried before resolution.
    /// Useful for diagnostics; surfaced in the boot log.
    pub raw_filter_count: usize,
    /// Original filter expressions, kept after resolution so the v2
    /// `graph::uam_adapter::entitled_set_for` path can re-resolve
    /// the same filters against a v2 `Graph` snapshot (whose NodeIds
    /// differ from v1's). Empty for rows with no filters.
    pub raw_filters: Vec<Filter>,
}

impl UamStore {
    /// New empty store. Cold-load is async and happens on demand
    /// (typically once at boot via `start`).
    pub fn new() -> Self {
        Self {
            inner: ArcSwap::from(Arc::new(HashMap::new())),
        }
    }

    /// Look up a user's entitled set. `Some(None)` = explicit
    /// "unrestricted" (empty filters). `Some(Some(_))` = the
    /// candidate set. `None` = unknown user/acl pair (caller decides
    /// whether to deny or allow).
    pub fn lookup(&self, user: i32, acl: i32) -> Option<Arc<EntitlementEntry>> {
        self.inner.load().get(&(user, acl)).cloned()
    }

    /// Number of (user, acl) entries cached.
    pub fn entry_count(&self) -> usize {
        self.inner.load().len()
    }

    /// Snapshot of every entry as cloned `EntitlementEntry`s. Used by
    /// the UAM-as-DataView projection (`handlers::dataview_source`).
    /// Allocates — fine for the ~221-entry bealls dataset; if the
    /// cache grows large, swap to an iterator over the inner Arc.
    pub fn snapshot_entries(&self) -> Vec<EntitlementEntry> {
        self.inner
            .load()
            .values()
            .map(|arc| (**arc).clone())
            .collect()
    }

    /// Number of users with at least one restrictive (non-empty
    /// filters) entry. Diagnostic only.
    pub fn restrictive_user_count(&self) -> usize {
        let snap = self.inner.load();
        let mut users: std::collections::HashSet<i32> = std::collections::HashSet::new();
        for entry in snap.values() {
            if entry.entitled.is_some() {
                users.insert(entry.user_code);
            }
        }
        users.len()
    }

    /// Cold-load every UAM row from PG, resolve filters against the
    /// graph, swap the result in. Idempotent: a second call rebuilds
    /// the cache from scratch.
    pub async fn cold_load(&self, dsn: &str, graph: Arc<ArticleGraph>) -> Result<()> {
        let started = std::time::Instant::now();
        let raw_rows = fetch_rows(dsn).await.context("UAM cold-load PG read")?;
        let raw_count = raw_rows.len();
        let mut next: HashMap<UamLookupKey, Arc<EntitlementEntry>> = HashMap::with_capacity(raw_count);
        let mut restrictive = 0usize;
        for row in raw_rows {
            let key = (row.user_code, row.acl_code);
            // Empty filters (or all filter values empty) = unrestricted.
            let entitled = if row.filters.is_empty() {
                None
            } else {
                let candidates = apply_filters(&graph, &row.filters, None);
                if candidates.is_empty() {
                    // Filters that resolve to zero candidates are
                    // honored as-is — the user explicitly has empty
                    // entitlements, surfaced separately from the
                    // unrestricted case so callers can detect it.
                    Some(EntitledSet {
                        articles: Some(std::collections::HashSet::new()),
                        store_codes: None,
                    })
                } else {
                    let articles: std::collections::HashSet<_> = candidates.into_iter().collect();
                    Some(EntitledSet {
                        articles: Some(articles),
                        store_codes: None,
                    })
                }
            };
            if entitled.is_some() {
                restrictive += 1;
            }
            next.insert(
                key,
                Arc::new(EntitlementEntry {
                    user_code: row.user_code,
                    acl_code: row.acl_code,
                    entitled,
                    raw_filter_count: row.filters.len(),
                    raw_filters: row.filters,
                }),
            );
        }

        self.inner.store(Arc::new(next));
        tracing::info!(
            "[uam] cold-loaded {} (user,acl) entries ({} restrictive) in {}ms",
            raw_count,
            restrictive,
            started.elapsed().as_millis()
        );
        Ok(())
    }

    /// Materialize the per-(user, acl) summary into the tenant DuckDB
    /// so DataViews bound to `kind = duckdb_table` (target = uam_summary)
    /// can read it through the standard read path. Idempotent: drops
    /// and rewrites the rows on every call. Call this after every
    /// `cold_load` (and any future incremental refresh) so the
    /// inspection view stays in sync with the live policy store.
    ///
    /// Schema (one row per (user_code, acl_code)):
    ///   user_code        INTEGER
    ///   acl_code         INTEGER
    ///   restricted       BOOLEAN
    ///   article_count    BIGINT
    ///   store_count      BIGINT
    ///   raw_filter_count INTEGER
    ///
    /// `article_count` for unrestricted entries is the caller-supplied
    /// `universe_articles` (typically `graph.count_kind(Article)`); for
    /// restricted entries it's the size of `entitled.articles`. Same
    /// shape the previous in-memory projection emitted.
    pub fn materialize_to_duckdb(&self, duckdb_path: &str, universe_articles: i64) -> Result<()> {
        let conn = duckdb::Connection::open(duckdb_path)
            .with_context(|| format!("opening DuckDB at {duckdb_path}"))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS uam_summary (\
                user_code        INTEGER, \
                acl_code         INTEGER, \
                restricted       BOOLEAN, \
                article_count    BIGINT, \
                store_count      BIGINT, \
                raw_filter_count INTEGER\
             );\
             DELETE FROM uam_summary;",
        )
        .context("uam_summary table setup")?;

        let entries = self.snapshot_entries();
        let mut stmt = conn
            .prepare("INSERT INTO uam_summary VALUES (?, ?, ?, ?, ?, ?)")
            .context("uam_summary insert prepare")?;
        for entry in &entries {
            let restricted = entry.entitled.is_some();
            let article_count: i64 = match entry.entitled.as_ref() {
                Some(ent) => ent
                    .articles
                    .as_ref()
                    .map(|s| s.len() as i64)
                    .unwrap_or(0),
                None => universe_articles,
            };
            let store_count: i64 = entry
                .entitled
                .as_ref()
                .and_then(|e| e.store_codes.as_ref())
                .map(|s| s.len() as i64)
                .unwrap_or(0);
            stmt.execute(duckdb::params![
                entry.user_code,
                entry.acl_code,
                restricted,
                article_count,
                store_count,
                entry.raw_filter_count as i32,
            ])
            .context("uam_summary insert")?;
        }
        tracing::info!(
            entries = entries.len(),
            "[uam] materialized summary to DuckDB"
        );
        Ok(())
    }
}

impl Default for UamStore {
    fn default() -> Self {
        Self::new()
    }
}

/// One raw row from `global.user_access_hierarchy_mapping`. We keep
/// only the fields used downstream — `access_hierarchy` is denormalized
/// from `filters`, so we ignore it.
#[derive(Debug)]
struct RawRow {
    user_code: i32,
    acl_code: i32,
    filters: Vec<Filter>,
}

async fn fetch_rows(dsn: &str) -> Result<Vec<RawRow>> {
    let (client, conn) = tokio_postgres::connect(dsn, NoTls)
        .await
        .context("PG connect")?;
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            tracing::warn!(error = %e, "[uam] PG connection lost");
        }
    });

    let rows = client
        .query(
            "SELECT user_code, acl_code, filters::text \
             FROM global.user_access_hierarchy_mapping \
             WHERE user_code IS NOT NULL AND acl_code IS NOT NULL",
            &[],
        )
        .await
        .context("query user_access_hierarchy_mapping")?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let user_code: i32 = r.get(0);
        let acl_code: i32 = r.get(1);
        let filters_text: String = r.get::<_, Option<String>>(2).unwrap_or_default();
        let filters: Vec<Filter> = if filters_text.is_empty() || filters_text == "[]" {
            Vec::new()
        } else {
            // Some operators in PG arrive as upper-case strings the
            // upstream service maps differently. Lower-case the
            // operator field before parsing so our serde
            // `rename_all = "lowercase"` lands correctly.
            let normalized = lowercase_operators(&filters_text);
            match serde_json::from_str::<Vec<Filter>>(&normalized) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(
                        "[uam] failed to parse filters for user_code={} acl_code={}: {} — treating as unrestricted",
                        user_code, acl_code, e
                    );
                    Vec::new()
                }
            }
        };
        out.push(RawRow {
            user_code,
            acl_code,
            filters,
        });
    }
    Ok(out)
}

/// Lowercase every JSON `"operator": "FOO"` value in place. The
/// production data uses lowercase already, but legacy rows surface
/// `"In"` / `"NotIn"` etc. — normalize so serde's `rename_all =
/// "lowercase"` accepts them.
fn lowercase_operators(s: &str) -> String {
    // Cheap regex-free pass: find `"operator":` followed by a quoted
    // string and lowercase its contents.
    let needle = "\"operator\":";
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let bytes = s.as_bytes();
    while i < bytes.len() {
        if bytes[i..].starts_with(needle.as_bytes()) {
            out.push_str(needle);
            i += needle.len();
            // skip whitespace
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                out.push(bytes[i] as char);
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'"' {
                out.push('"');
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    out.push((bytes[i] as char).to_ascii_lowercase());
                    i += 1;
                }
                if i < bytes.len() {
                    out.push('"');
                    i += 1;
                }
            }
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}
