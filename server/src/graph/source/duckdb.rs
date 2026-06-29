//! DuckDB implementation of [`SourceReader`].
//!
//! Synthesizes `SELECT "col1", "col2", … FROM "table" [WHERE filter]`
//! from the spec'd table + columns + filter and maps each row into the
//! generic [`Row`] / [`CellValue`] pair. The reader assumes the spec
//! has already been through `validate()` — it does basic identifier
//! safety checks (no semicolons, no embedded quotes) as a defense in
//! depth, but trusts the input format.
//!
//! Schema-qualified names (`schema.table`) are split on `.` and each
//! half is double-quoted independently, matching DuckDB's identifier
//! rules. Bare identifiers go through as-is.

use anyhow::{Result, anyhow};
use duckdb::Connection;
use duckdb::types::{Value, ValueRef};

use super::{CellValue, Row, SourceReader};

/// Owns a `duckdb::Connection`. The lifetime model mirrors
/// `article_graph::source::duckdb::DuckDbReader`: one connection per
/// build, reused across every `read` call in that build.
pub struct DuckDbSourceReader {
    conn: Connection,
}

impl DuckDbSourceReader {
    /// Open a fresh DuckDB connection at `path`. For tests / when a
    /// caller already has a connection in hand, see `from_connection`.
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        Ok(Self { conn })
    }

    pub fn from_connection(conn: Connection) -> Self {
        Self { conn }
    }
}

/// Cheap identifier safety check. Validate already rejects garbage at
/// definition time; this catches programmer errors (table/column names
/// hand-built at runtime with embedded `;` or `"`). Rejecting at the
/// reader rather than the SQL layer turns "syntax error near …" into a
/// readable Rust error.
fn assert_safe_identifier(s: &str, what: &str) -> Result<()> {
    if s.is_empty() {
        return Err(anyhow!("{what} is empty"));
    }
    if s.contains(';') || s.contains('"') || s.contains('\n') {
        return Err(anyhow!(
            "{what} `{s}` contains characters that aren't valid in a DuckDB identifier (`;`, `\"`, newline)",
        ));
    }
    Ok(())
}

/// Wrap an identifier in double quotes, handling schema-qualified
/// names (`schema.table`) by quoting each part.
fn quote_ident(s: &str) -> Result<String> {
    if s.contains('.') {
        let parts: Vec<&str> = s.split('.').collect();
        for p in &parts {
            assert_safe_identifier(p, "identifier component")?;
        }
        Ok(parts.iter().map(|p| format!("\"{p}\"")).collect::<Vec<_>>().join("."))
    } else {
        assert_safe_identifier(s, "identifier")?;
        Ok(format!("\"{s}\""))
    }
}

impl SourceReader for DuckDbSourceReader {
    fn read(
        &self,
        table: &str,
        columns: &[String],
        filter: Option<&str>,
    ) -> Result<Vec<Row>> {
        if columns.is_empty() {
            return Err(anyhow!("read called with empty `columns` slice"));
        }

        let cols_sql = columns
            .iter()
            .map(|c| quote_ident(c))
            .collect::<Result<Vec<_>>>()?
            .join(", ");
        let table_sql = quote_ident(table)?;
        let sql = match filter {
            Some(f) if !f.trim().is_empty() => {
                format!("SELECT {cols_sql} FROM {table_sql} WHERE {f}")
            }
            _ => format!("SELECT {cols_sql} FROM {table_sql}"),
        };

        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query([])?;
        let col_count = columns.len();

        let mut out: Vec<Row> = Vec::new();
        while let Some(r) = rows.next()? {
            let mut cells = Vec::with_capacity(col_count);
            for i in 0..col_count {
                cells.push(cell_from_ref(r.get_ref(i)?)?);
            }
            out.push(Row { cells });
        }
        Ok(out)
    }
}

/// Convert a borrowed DuckDB value into our owned `CellValue`. The
/// branch coverage here is "the types we actually expect to see in
/// graph specs" — numerics, text, null. Exotic types (intervals,
/// blobs, decimals, lists, structs) collapse to Text via the owned
/// `Value`'s Debug impl; the engine never operates on them in Phase 2,
/// and falling back to text means an unfamiliar column type doesn't
/// crash the build.
///
/// LIST<…> handling for `unnest = true` lands in Phase 3 — that code
/// will replace the wildcard branch with a recursive List converter.
fn cell_from_ref(v: ValueRef<'_>) -> Result<CellValue> {
    Ok(match v {
        ValueRef::Null => CellValue::Null,
        ValueRef::Boolean(b) => CellValue::Int(if b { 1 } else { 0 }),
        ValueRef::TinyInt(i) => CellValue::Int(i as i64),
        ValueRef::SmallInt(i) => CellValue::Int(i as i64),
        ValueRef::Int(i) => CellValue::Int(i as i64),
        ValueRef::BigInt(i) => CellValue::Int(i),
        ValueRef::HugeInt(i) => CellValue::Int(i as i64),
        ValueRef::UTinyInt(i) => CellValue::Int(i as i64),
        ValueRef::USmallInt(i) => CellValue::Int(i as i64),
        ValueRef::UInt(i) => CellValue::Int(i as i64),
        ValueRef::UBigInt(i) => CellValue::Int(i as i64),
        ValueRef::Float(f) => CellValue::Float(f as f64),
        ValueRef::Double(f) => CellValue::Float(f),
        ValueRef::Text(bytes) => {
            CellValue::Text(std::str::from_utf8(bytes).unwrap_or("").to_string())
        }
        other => {
            // Unknown / exotic / structural type — fall back to a string
            // representation. Loses semantic typing but doesn't crash
            // the build.
            let owned: Value = other.into();
            CellValue::Text(format!("{owned:?}"))
        }
    })
}
