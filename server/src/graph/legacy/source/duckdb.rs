//! DuckDB-backed [`GraphSourceReader`].
//!
//! Reads from the same `asv2_*` / `raw_*` tables that V7 reads in
//! [`crate::article_selection::extractor`]. The SQL is intentionally
//! mirrored here (not imported) so V7 and V8 evolve independently —
//! each module owns its read path.
//!
//! Phase 1 source. Subsequent phases will add `parquet`, `pg`, `bq`
//! readers behind the same trait.

use anyhow::Result;
use duckdb::Connection;
use std::collections::{HashMap, HashSet};

use crate::graph::legacy::rows::{
    GraphSourceReader, InventoryAgg, PafRow, PhMasterRow, TxsMetrics,
};

/// Reader that opens the tenant DuckDB at `duckdb_path` once per build
/// and serves all queries off that connection.
pub struct DuckDbReader {
    conn: Connection,
}

impl DuckDbReader {
    pub fn open(duckdb_path: &str) -> Result<Self> {
        let conn = Connection::open(duckdb_path)?;
        Ok(Self { conn })
    }

    /// For tests / when a caller already has a Connection in hand.
    pub fn from_connection(conn: Connection) -> Self {
        Self { conn }
    }
}

/// NULL-safe string read: PG NULL → empty string. Mirrors the helper
/// used in `article_selection::extractor` reader fns.
fn s(r: &duckdb::Row<'_>, i: usize) -> duckdb::Result<String> {
    r.get::<_, Option<String>>(i).map(|o| o.unwrap_or_default())
}

impl GraphSourceReader for DuckDbReader {
    fn read_ph_master(&self) -> Result<Vec<PhMasterRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT CAST(ph_code AS VARCHAR), CAST(article AS VARCHAR), \
                    CAST(l0_name AS VARCHAR), CAST(l1_name AS VARCHAR), \
                    CAST(l2_name AS VARCHAR), CAST(l3_name AS VARCHAR), \
                    CAST(l4_name AS VARCHAR), CAST(l5_name AS VARCHAR), \
                    CAST(brand AS VARCHAR), CAST(channel AS VARCHAR), \
                    CAST(product_codes AS VARCHAR) \
             FROM asv2_ph_master",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(PhMasterRow {
                    ph_code: s(r, 0)?,
                    article: s(r, 1)?,
                    l0_name: s(r, 2)?,
                    l1_name: s(r, 3)?,
                    l2_name: s(r, 4)?,
                    l3_name: s(r, 5)?,
                    l4_name: s(r, 6)?,
                    l5_name: s(r, 7)?,
                    brand: s(r, 8)?,
                    channel: s(r, 9)?,
                    product_codes: s(r, 10)?,
                })
            })?
            .collect::<duckdb::Result<Vec<_>>>()?;
        Ok(rows)
    }

    fn read_paf(&self) -> Result<HashMap<String, PafRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT CAST(product_code AS VARCHAR), CAST(article AS VARCHAR), \
                    CAST(l0_name AS VARCHAR), CAST(l1_name AS VARCHAR), \
                    CAST(l2_name AS VARCHAR), CAST(l3_name AS VARCHAR), \
                    CAST(l4_name AS VARCHAR), CAST(l5_name AS VARCHAR), \
                    CAST(brand AS VARCHAR) \
             FROM asv2_paf",
        )?;
        let pairs: Vec<(String, PafRow)> = stmt
            .query_map([], |r| {
                let pc = s(r, 0)?;
                Ok((
                    pc.clone(),
                    PafRow {
                        product_code: pc,
                        article: s(r, 1)?,
                        l0_name: s(r, 2)?,
                        l1_name: s(r, 3)?,
                        l2_name: s(r, 4)?,
                        l3_name: s(r, 5)?,
                        l4_name: s(r, 6)?,
                        l5_name: s(r, 7)?,
                        brand: s(r, 8)?,
                    },
                ))
            })?
            .collect::<duckdb::Result<Vec<_>>>()?;
        Ok(pairs.into_iter().collect())
    }

    fn read_inventory(&self) -> Result<HashMap<String, InventoryAgg>> {
        let mut stmt = self.conn.prepare(
            "SELECT CAST(ph_code AS VARCHAR), \
                    CAST(oh AS BIGINT), CAST(oo AS BIGINT), CAST(it AS BIGINT), \
                    CAST(reserve_quantity AS BIGINT), CAST(allocated_units AS BIGINT) \
             FROM asv2_inventory",
        )?;
        let pairs: Vec<(String, InventoryAgg)> = stmt
            .query_map([], |r| {
                Ok((
                    s(r, 0)?,
                    InventoryAgg {
                        oh: r.get::<_, i64>(1).unwrap_or(0),
                        oo: r.get::<_, i64>(2).unwrap_or(0),
                        it: r.get::<_, i64>(3).unwrap_or(0),
                        reserve_quantity: r.get::<_, i64>(4).unwrap_or(0),
                        allocated_units: r.get::<_, i64>(5).unwrap_or(0),
                    },
                ))
            })?
            .collect::<duckdb::Result<Vec<_>>>()?;
        Ok(pairs.into_iter().collect())
    }

    fn read_txs_metrics(&self) -> Result<HashMap<String, TxsMetrics>> {
        let mut stmt = self.conn.prepare(
            "SELECT CAST(ph_code AS VARCHAR), \
                    CAST(lw_units AS BIGINT), CAST(lw_margin AS BIGINT), CAST(lw_revenue AS BIGINT) \
             FROM asv2_txs_metrics",
        )?;
        let pairs: Vec<(String, TxsMetrics)> = stmt
            .query_map([], |r| {
                Ok((
                    s(r, 0)?,
                    TxsMetrics {
                        lw_units: r.get::<_, i64>(1).unwrap_or(0),
                        lw_margin: r.get::<_, i64>(2).unwrap_or(0),
                        lw_revenue: r.get::<_, i64>(3).unwrap_or(0),
                    },
                ))
            })?
            .collect::<duckdb::Result<Vec<_>>>()?;
        Ok(pairs.into_iter().collect())
    }

    fn read_product_dc(&self) -> Result<HashMap<String, Vec<String>>> {
        let mut stmt = self.conn.prepare(
            "SELECT CAST(product_code AS VARCHAR), CAST(dc_codes AS VARCHAR) FROM asv2_product_dc",
        )?;
        let pairs: Vec<(String, Vec<String>)> = stmt
            .query_map([], |r| {
                let pc = s(r, 0)?;
                let dcs_str = s(r, 1)?;
                let dcs: Vec<String> = dcs_str
                    .split('|')
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect();
                Ok((pc, dcs))
            })?
            .collect::<duckdb::Result<Vec<_>>>()?;
        Ok(pairs.into_iter().collect())
    }

    fn read_store_dc(&self) -> Result<HashMap<String, Vec<String>>> {
        let mut stmt = self.conn.prepare(
            "SELECT CAST(store_code AS VARCHAR), CAST(dc_code AS VARCHAR) FROM raw_store_dc_mapping",
        )?;
        let pairs: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((s(r, 0)?, s(r, 1)?)))?
            .collect::<duckdb::Result<Vec<_>>>()?;
        let mut out: HashMap<String, Vec<String>> = HashMap::new();
        for (store, dc) in pairs {
            if store.is_empty() || dc.is_empty() {
                continue;
            }
            out.entry(store).or_default().push(dc);
        }
        Ok(out)
    }

    fn read_distribution_centres(&self) -> Result<HashMap<String, String>> {
        let mut stmt = self.conn.prepare(
            "SELECT CAST(dc_code AS VARCHAR), CAST(name AS VARCHAR) FROM raw_distribution_centres",
        )?;
        let pairs: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((s(r, 0)?, s(r, 1)?)))?
            .collect::<duckdb::Result<Vec<_>>>()?;
        Ok(pairs.into_iter().collect())
    }

    fn read_store_channels(&self) -> Result<HashMap<String, String>> {
        // raw_store_channels rows are filtered to active=true on extract.
        let mut stmt = self.conn.prepare(
            "SELECT CAST(store_code AS VARCHAR), CAST(channel AS VARCHAR) \
             FROM raw_store_channels WHERE active = true",
        )?;
        let pairs: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((s(r, 0)?, s(r, 1)?)))?
            .collect::<duckdb::Result<Vec<_>>>()?;
        Ok(pairs.into_iter().collect())
    }

    fn read_store_to_sgs(&self) -> Result<HashMap<String, Vec<String>>> {
        // V7 reads (sg_code, store_code); we invert to store_code → sg_codes
        // since the graph keys store nodes by store_code.
        let mut stmt = self.conn.prepare(
            "SELECT CAST(store_code AS VARCHAR), CAST(sg_code AS VARCHAR) \
             FROM raw_store_groups_mapping",
        )?;
        let pairs: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((s(r, 0)?, s(r, 1)?)))?
            .collect::<duckdb::Result<Vec<_>>>()?;
        let mut out: HashMap<String, Vec<String>> = HashMap::new();
        for (store, sg) in pairs {
            if store.is_empty() || sg.is_empty() {
                continue;
            }
            out.entry(store).or_default().push(sg);
        }
        Ok(out)
    }

    fn read_active_store_codes(&self) -> Result<HashSet<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT CAST(store_code AS VARCHAR) FROM raw_store_master \
             WHERE active = true AND is_deleted = false",
        )?;
        let rows: HashSet<String> = stmt
            .query_map([], |r| s(r, 0))?
            .collect::<duckdb::Result<HashSet<_>>>()?;
        Ok(rows)
    }

    fn read_psm_priorities(&self) -> Result<Vec<(String, i32)>> {
        let mut stmt = self.conn.prepare(
            "SELECT CAST(rcl_code AS VARCHAR), CAST(priority AS INTEGER) \
             FROM raw_rcl_psm_priorities ORDER BY priority ASC",
        )?;
        let rows: Vec<(String, i32)> = stmt
            .query_map([], |r| {
                Ok((
                    s(r, 0)?,
                    r.get::<_, Option<i32>>(1)?.unwrap_or(0),
                ))
            })?
            .collect::<duckdb::Result<Vec<_>>>()?;
        Ok(rows)
    }

    fn read_psm_rule_dim(&self) -> Result<Vec<(String, String, String)>> {
        // Returns (rcl_code, rule_code, dim_json). The on-the-fly
        // resolver parses dim_json once per row at build time and
        // stores the parsed map in PsmResolver's per-rcl-code index.
        // No md5 round-trip — the resolver matches the product's
        // hierarchy fields directly against each rule's dim map.
        let mut stmt = self.conn.prepare(
            "SELECT CAST(rcl_code AS VARCHAR), CAST(rule_code AS VARCHAR), \
                    CAST(dim_json AS VARCHAR) \
             FROM raw_rcl_psm_rule_dim",
        )?;
        let rows: Vec<(String, String, String)> = stmt
            .query_map([], |r| Ok((s(r, 0)?, s(r, 1)?, s(r, 2)?)))?
            .collect::<duckdb::Result<Vec<_>>>()?;
        Ok(rows)
    }
}
