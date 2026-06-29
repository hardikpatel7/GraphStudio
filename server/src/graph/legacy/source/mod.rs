//! Concrete [`GraphSourceReader`](super::rows::GraphSourceReader) implementations.
//!
//! Phase 1 ships [`duckdb::DuckDbReader`]. Later phases add `parquet`,
//! `pg`, `bq` modules behind the same trait.

pub mod duckdb;
