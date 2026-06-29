//! Backend-agnostic source reading.
//!
//! `build_graph` reads every byte through `SourceReader::read`, so
//! swapping DuckDB for parquet / PG / a mock is a one-line change at
//! the call site. The trait is `Send` only — Phase 2 calls `build` from
//! a single `spawn_blocking` task and never shares the reader across
//! threads.
//!
//! ## Why column-oriented `CellValue` rather than `serde_json::Value`?
//!
//! Two reasons. First, `CellValue` keeps integer columns as `i64`
//! without the round-trip-through-f64 precision loss that
//! `serde_json::Number` enforces. Second, `CellValue::List` is needed
//! for the `unnest = true` LIST<…> column path (Decision 21); JSON
//! handling that case as a `Value::Array` of mixed-typed entries
//! conflates "this column holds a list" with "this row produced a
//! nested structure".

use anyhow::Result;

pub mod duckdb;

/// One cell from a source query. `Null` is preserved (mapped to empty
/// string by `as_text` for the common "treat null as empty" hierarchy
/// case) so callers can distinguish "missing" from "empty string" when
/// it matters.
#[derive(Debug, Clone)]
pub enum CellValue {
    Null,
    Int(i64),
    Float(f64),
    Text(String),
    /// Native `LIST<…>` columns — the engine unnests these per the
    /// level's `unnest = true` directive. Delimited-string columns
    /// stay as `Text` and get split by the engine.
    List(Vec<CellValue>),
}

impl CellValue {
    /// Cell → string, using empty-string for `Null`. The common path
    /// for hierarchy spine reading: callers rarely care about null vs
    /// empty when interning a node name.
    pub fn as_text(&self) -> String {
        match self {
            CellValue::Null => String::new(),
            CellValue::Int(i) => i.to_string(),
            CellValue::Float(f) => f.to_string(),
            CellValue::Text(s) => s.clone(),
            // Lists are serialized for diagnostic logging only; the
            // engine never calls `as_text` on a LIST cell — it inspects
            // `CellValue::List` directly when `unnest` is set.
            CellValue::List(_) => "<list>".to_string(),
        }
    }

    /// Cell → f64 for numeric metrics. `Null` and `Text` collapse to 0.0
    /// to keep sum-style rollups going on dirty data; callers that need
    /// strict typing should branch on the variant directly.
    pub fn as_f64(&self) -> f64 {
        match self {
            CellValue::Int(i) => *i as f64,
            CellValue::Float(f) => *f,
            CellValue::Text(s) => s.parse().unwrap_or(0.0),
            CellValue::Null | CellValue::List(_) => 0.0,
        }
    }
}

/// One row from a source query. Cells are positional, matching the
/// `columns` slice passed to `SourceReader::read`. Heavy reads (bealls
/// PH master ≈ 50 K rows) build a `Vec<Row>` outright — streaming would
/// complicate the build code without buying anything (every reader
/// today is single-machine and the rows fit comfortably in RAM).
#[derive(Debug, Clone)]
pub struct Row {
    pub cells: Vec<CellValue>,
}

/// Backend-agnostic source reader. Each impl owns its own connection /
/// table catalog / SQL dialect; the `build` code never sees those
/// details.
pub trait SourceReader: Send {
    /// Read every row of `table` matching `filter` (optional WHERE
    /// clause body, no `WHERE` keyword), returning only `columns` in
    /// the order requested.
    ///
    /// The reader is free to synthesize whatever SQL its backend needs
    /// — quote `table` per the dialect's rules, project only the listed
    /// columns, etc. Callers should not pre-escape anything in the
    /// strings they pass; that's the reader's responsibility.
    fn read(
        &self,
        table: &str,
        columns: &[String],
        filter: Option<&str>,
    ) -> Result<Vec<Row>>;
}
