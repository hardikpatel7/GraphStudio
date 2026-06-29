# PG Extract Performance: Investigation & Optimization

## Context

SmartStudio pipelines extract data from PostgreSQL into Parquet files via the Rust backend. The **Article Selection v2** pipeline is the most demanding — it extracts 21 tables from PG, loads them into DuckDB, runs a complex materialize query joining everything, then writes final Parquet output.

The largest tables:

| Table | Rows | PG Size | Columns Extracted |
|-------|------|---------|-------------------|
| `article_inventory_dashboard` | 28M | 25 GB (77 cols total) | 11 |
| `woc_master` | 21.2M | 3.8 GB | ~10 |
| `product_mapping_product_dc` | 10M | — | 2 |
| `product_attributes_filter` | 1.4M | — | ~15 |
| `ph_master` | 535K | 384 MB | 17 |

---

## Option 1: ConnectorX Binary Protocol (Original)

### How it works

[ConnectorX](https://github.com/sfu-db/connector-x) uses PostgreSQL's **binary protocol** to fetch data directly into Arrow2 columnar arrays, which are then written to Parquet via `arrow2::io::parquet`. This skips CSV serialization/deserialization entirely — data goes from PG wire format → Arrow columnar → Parquet columnar.

```
PG binary protocol → Arrow2 arrays (in memory) → Parquet file
```

### Implementation

```rust
use connectorx::prelude::*;

let source = SourceConn::try_from(pg_url)?;
let queries = &[CXQuery::naked(query)];
let dest = get_arrow2(source, None, queries)?;
// dest.arrow() returns Vec<Chunk<Box<dyn Array>>>
// Write chunks to parquet via arrow2::io::parquet::write
```

### Results

| Table | Time | Notes |
|-------|------|-------|
| `sku_dc_available` (1.4M rows) | 1.7 min | Acceptable |
| `ph_master` (535K rows) | **5.3 min** | Unexpectedly slow |
| `article_inventory_dashboard` (28M rows) | **8+ min** | Very slow |
| `woc_master` (21.2M rows) | **7.8 min** | Very slow |

### Problems discovered

1. **ConnectorX panics on `::text` casts (JSONB columns)**

   `ph_master` has a column `product_code_size_map::text AS product_code_size_map` — a JSONB column cast to text. ConnectorX's binary protocol parser doesn't handle this type transition. The CX worker thread **panics** (Rust `panic!`), and because CX spawns its own threads internally, the panic is unrecoverable from the caller's perspective.

   The result: the pipeline hangs for 2-3 minutes waiting for the CX thread to die, then falls back to CSV. This explains why `ph_master` (535K rows) was slower than `sku_dc_available` (1.4M rows) — it wasn't the data volume, it was the **wasted time on a doomed CX attempt**.

2. **No partition support for VARCHAR keys**

   ConnectorX can partition a query across multiple threads for parallelism, but it requires an **integer column** to partition on (e.g., `WHERE id BETWEEN 0 AND 1000000`). These PG tables use VARCHAR composite keys (`article`, `store_code`, `product_code`), so CX partitioning is not available.

3. **Single-threaded network bottleneck**

   Without partitioning, CX fetches the entire result set through a single PG connection. For `article_inventory_dashboard` (28M rows, ~950 MB of data), PG executes the query in ~14 seconds but network transfer takes minutes. The binary protocol doesn't help here — the bottleneck is single-connection throughput.

### Verdict

ConnectorX binary protocol is fast for **small-to-medium tables** with simple column types. It falls apart on:
- JSONB / `::text` casts (panics)
- Large tables without integer partition keys (single-threaded)
- Any query that needs CSV fallback (wasted time on failed attempt)

---

## Option 2: Single-Stream COPY CSV

### How it works

PostgreSQL's `COPY ... TO STDOUT` streams the entire result set as CSV over a single connection. The Rust client reads the stream into memory, writes it to a temp CSV file, then uses DuckDB's `read_csv_auto` to convert CSV → Parquet.

```
PG COPY CSV → memory buffer → temp .csv file → DuckDB read_csv_auto → Parquet
```

### Implementation

```rust
let copy_sql = format!("COPY ({}) TO STDOUT WITH (FORMAT CSV, HEADER true)", query);
let stream = client.copy_out(&copy_sql).await?;

// Collect all bytes
let mut data = Vec::new();
while let Some(chunk) = stream.next().await {
    data.extend_from_slice(&chunk?);
}

// Write CSV to disk, then convert
std::fs::write(&csv_path, &data)?;
let db = DuckDbConn::open_in_memory()?;
db.execute_batch(&format!(
    "COPY (SELECT * FROM read_csv_auto('{}', sample_size=-1)) TO '{}' (FORMAT PARQUET, COMPRESSION SNAPPY)",
    csv_path, out_file
))?;
```

### Results

| Table | Time | Notes |
|-------|------|-------|
| `ph_master` (535K rows) | ~60s | After CX panic fallback was eliminated |
| `article_inventory_dashboard` (28M rows) | **~98s** | Benchmarked single COPY |

### Problems discovered

1. **Still single-connection throughput**

   Even though COPY CSV is more efficient than CX binary for these tables (no type parsing overhead, no thread panics), it's still bounded by single PG connection throughput. For 28M rows (~950 MB), a single COPY takes ~98 seconds.

2. **DuckDB CSV type auto-detection failure**

   DuckDB's `read_csv_auto` samples the first N rows (default `sample_size=20480`) to infer column types. For `product_mapping_product_dc`, the `product_code` column looks like integers for the first ~2M rows, but row 2,040,132 contains `"57335772-00002"` (a VARCHAR). DuckDB commits to BIGINT from the sample, then fails at row 2M:

   ```
   CSV Error on Line: 2040132
   Error when converting column "product_code".
   Could not convert string "57335772-00002" to 'BIGINT'
   ```

   **Fix**: `sample_size=-1` tells DuckDB to scan the **entire file** for type inference before committing to column types. This adds marginal overhead (DuckDB streams through the file once for types, once for data) but guarantees correct type detection.

### Verdict

Single COPY CSV is reliable and handles all PG types correctly (PG does the serialization). But for tables > 1M rows, it's still slow due to single-connection throughput.

---

## Option 3: Parallel Hash-Partitioned COPY CSV (Final Solution)

### How it works

Instead of one COPY stream, we split the table into N partitions using PostgreSQL's `hashtext()` function on the first two columns, then run N independent COPY streams in parallel — each on its own PG connection, in its own OS thread with its own tokio runtime. DuckDB merges the N CSV chunks into a single Parquet file.

```
                    ┌─ Thread 0: COPY (WHERE abs(hash) % 4 = 0) → chunk_0.csv ─┐
PG table ──────────►├─ Thread 1: COPY (WHERE abs(hash) % 4 = 1) → chunk_1.csv ─┤─► DuckDB UNION ALL → Parquet
(N connections)     ├─ Thread 2: COPY (WHERE abs(hash) % 4 = 2) → chunk_2.csv ─┤
                    └─ Thread 3: COPY (WHERE abs(hash) % 4 = 3) → chunk_3.csv ─┘
```

### Hash partitioning strategy

PostgreSQL's `hashtext()` function returns a 32-bit signed integer hash of a text value. We concatenate the first two columns of the SELECT list as the hash key:

```sql
-- Original query:
SELECT article, store_code, oh, oo, it FROM inventory_smart.article_inventory_dashboard

-- Partitioned query for stream i of N:
COPY (
  SELECT article, store_code, oh, oo, it
  FROM inventory_smart.article_inventory_dashboard
  WHERE abs(hashtext(article::text || store_code::text)) % 4 = 0
) TO STDOUT WITH (FORMAT CSV, HEADER true)
```

**Why two columns?** Using two columns as the hash key provides better distribution than one. If the first column has low cardinality (e.g., 5 DCs), hashing on it alone would create very uneven partitions. Concatenating two columns ensures even distribution even with skewed data.

**Why `hashtext()` and not `hash_record()`?** `hashtext()` operates on text, works with any column type (via `::text` cast), and is available in all PG versions. It returns a stable 32-bit hash suitable for modulo partitioning.

### Implementation

```rust
fn parallel_csv_pg_to_parquet(
    &self, dsn: &str, query: &str, out_file: &str,
    num_streams: usize, progress: &dyn Fn(&str),
) -> Result<i64> {
    // 1. Parse the query to extract table name and column list
    //    SELECT col1, col2, ... FROM schema.table [WHERE ...]
    let (table_part, existing_where) = parse_query(query);

    // 2. Build hash expression from first two columns
    let hash_expr = format!("hashtext({}::text || {}::text)", col1, col2);

    // 3. Spawn N OS threads, each with own tokio runtime + PG connection
    let handles: Vec<_> = (0..num_streams).map(|i| {
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all().build()?;
            rt.block_on(async {
                let (client, conn) = tokio_postgres::connect(&dsn, NoTls).await?;
                tokio::spawn(async move { conn.await.ok(); });

                let partition_where = format!(
                    "abs({}) % {} = {}",
                    hash_expr, num_streams, i
                );
                let full_query = if let Some(existing) = &existing_where {
                    format!("SELECT {} FROM {} WHERE ({}) AND ({})",
                        cols, table, existing, partition_where)
                } else {
                    format!("SELECT {} FROM {} WHERE {}",
                        cols, table, partition_where)
                };

                let copy_sql = format!(
                    "COPY ({}) TO STDOUT WITH (FORMAT CSV, HEADER true)",
                    full_query
                );
                let stream = client.copy_out(&copy_sql).await?;
                // ... collect bytes, write to chunk file ...
            })
        })
    }).collect();

    // 4. Wait for all threads, collect chunk paths
    for handle in handles { handle.join()?; }

    // 5. Merge chunks via DuckDB UNION ALL → single Parquet
    let union_sql = chunk_paths.iter()
        .map(|p| format!("SELECT * FROM read_csv_auto('{}', sample_size=-1)", p))
        .collect::<Vec<_>>()
        .join(" UNION ALL ");

    let db = DuckDbConn::open_in_memory()?;
    db.execute_batch(&format!(
        "COPY ({}) TO '{}' (FORMAT PARQUET, COMPRESSION SNAPPY)",
        union_sql, out_file
    ))?;

    Ok(row_count)
}
```

### Why separate OS threads with own tokio runtimes?

The pipeline executor runs on crossbeam threads (sync), not inside a tokio async context. We can't use `tokio::spawn` directly. Each parallel stream needs:
- Its own **PG connection** (PG connections are not multiplexed)
- Its own **tokio runtime** for the async `tokio-postgres` client
- **True parallelism** (OS threads), not async concurrency on a shared runtime

Using `std::thread::spawn` with `Runtime::new()` per thread guarantees:
1. Each stream gets dedicated CPU time
2. Each PG connection transfers data independently
3. No contention on the tokio executor
4. If one stream fails, others continue (we collect errors after join)

### Results

| Table | Single COPY | 4x Parallel COPY | Speedup |
|-------|-------------|-------------------|---------|
| `article_inventory_dashboard` (28M rows) | ~98s | **~25s fetch + merge** | **~4x** |
| `woc_master` (21.2M rows) | — | **279s total** | — |
| `ph_master` (535K rows, large columns) | ~60s | **133s total** | * |
| `product_mapping_product_dc` (10M rows) | FAILED | **88s total** | Fixed |
| `sku_dc_available` (117K rows) | ~100s (via CX) | **6.4s** | **~16x** |

\* `ph_master` total time includes the `sample_size=-1` scan of 291 MB CSV. The raw data has wide JSONB columns serialized as text, so 535K rows produce 291 MB of CSV — the bottleneck is data volume, not row count.

### Full pipeline comparison (Article Selection v2)

| Step | Before (CX + single CSV) | After (parallel CSV) |
|------|--------------------------|----------------------|
| All 21 extracts | ~20 min (with failures) | **~5 min** |
| `extract_product_dc` | **FAILED** (type error) | 88s |
| `extract_ph_master` | 5.3 min (CX panic + fallback) | 133s |
| `extract_aid` | 8+ min (CX single-threaded) | 308s |
| `extract_woc_master` | 7.8 min (CX single-threaded) | 279s |

---

## Routing Strategy

The final implementation uses a three-tier routing strategy based on query characteristics:

```
┌─────────────────────────────────┐
│ Does query contain ::text cast? │──Yes──► Direct to single CSV
│ (JSONB columns cause CX panic) │        (parallel not safe for
└──────────────┬──────────────────┘         ::text in hash partition)
               │ No
┌──────────────▼──────────────────┐
│ Is it a simple table scan?      │──Yes──► 4x parallel CSV
│ (single FROM, no JOINs,        │        (hash-partition on first
│  no subqueries)                 │         two columns)
└──────────────┬──────────────────┘
               │ No (complex query)
┌──────────────▼──────────────────┐
│ Try ConnectorX binary protocol  │──Success──► Use CX result
│ (with panic catch_unwind)       │
└──────────────┬──────────────────┘
               │ Panic/Failure
               ▼
         Single CSV fallback
```

### Detection logic

```rust
let has_text_cast = query.contains("::text") || query.contains(":: text");

let is_simple_scan = !query.contains(" JOIN ")
    && !query.to_lowercase().contains(" join ")
    && query.matches(" FROM ").count() + query.matches(" from ").count() == 1
    && !query.contains("(SELECT")
    && !query.contains("(select");

if has_text_cast || is_simple_scan {
    // Direct to CSV (parallel for simple scans, single for ::text)
    csv_pg_to_parquet(...)
} else {
    // Complex query → try ConnectorX, fall back to CSV
    match catch_unwind(|| cx_pg_to_parquet(...)) {
        Ok(Ok(count)) => count,
        _ => csv_pg_to_parquet(...)
    }
}
```

### Why not always use parallel CSV?

1. **Complex queries with JOINs** can't be trivially hash-partitioned — the WHERE clause injection changes the query plan and may produce incorrect results if the join condition interacts with the partition predicate.

2. **Stored procedure calls** (`SELECT * FROM some_func(...)`) can't be partitioned.

3. **ConnectorX binary protocol is genuinely faster** for complex queries with integer keys — it avoids CSV serialization overhead entirely. The routing keeps CX as an option for cases where it works well.

---

## DuckDB CSV Type Detection Fix

### The problem

DuckDB's `read_csv_auto` uses a sampling strategy to infer column types. The default `sample_size=20480` reads 20K rows to determine types. For `product_mapping_product_dc`:

- First 2M rows: `product_code` values are pure integers (`57335772`, `57335773`, ...)
- Row 2,040,132: `product_code = "57335772-00002"` (VARCHAR with hyphen)

DuckDB commits to BIGINT after sampling, then fails 2M rows later when it encounters a non-integer value.

### The fix

```sql
-- Before (default sample_size=20480):
SELECT * FROM read_csv_auto('data.csv')

-- After (scan entire file for type inference):
SELECT * FROM read_csv_auto('data.csv', sample_size=-1)
```

`sample_size=-1` tells DuckDB to scan **every row** in the file before committing to column types. If any value in a column is non-numeric, the column becomes VARCHAR.

### Performance impact

| File Size | Type Scan Overhead |
|-----------|--------------------|
| < 10 MB | Negligible (< 100ms) |
| 100 MB | ~1-2 seconds |
| 900 MB (28M rows) | ~5-10 seconds |

The overhead is acceptable because:
1. DuckDB streams through the file — it doesn't load it all into memory
2. The alternative (pipeline failure) is infinitely worse
3. The scan happens once per file, not per query

### Where it's applied

All `read_csv_auto` calls in the pipeline executor use `sample_size=-1`:
- `single_csv_pg_to_parquet` — single stream CSV → Parquet
- `parallel_csv_pg_to_parquet` — each chunk in the UNION ALL merge
- `exec_pg_extract_to_memory` — CDC / in-memory table loads

---

## ConnectorX Panic Safety

### The problem

ConnectorX spawns its own internal threads for data fetching. When it encounters an unsupported type (like JSONB→text cast), the worker thread **panics**. Because the panic happens inside CX's thread pool (not our code), it's not caught by normal Rust error handling. The calling thread hangs waiting for a response that never comes, until the OS reaps the dead thread (2-3 minutes).

### The fix

```rust
use std::panic::{catch_unwind, AssertUnwindSafe};

let result = catch_unwind(AssertUnwindSafe(|| {
    cx_pg_to_parquet(&url, &query, &out_file)
}));

match result {
    Ok(Ok(count)) => count,           // CX succeeded
    Ok(Err(e)) => {                    // CX returned an error
        tracing::warn!("CX error: {}", e);
        csv_pg_to_parquet(...)         // fall back to CSV
    }
    Err(panic_info) => {               // CX thread panicked
        tracing::error!("CX panicked, falling back to CSV");
        csv_pg_to_parquet(...)         // fall back to CSV
    }
}
```

`catch_unwind` captures panics from CX's internal threads and converts them to `Result::Err`, allowing graceful fallback. `AssertUnwindSafe` is required because the closure captures references that aren't `UnwindSafe` by default.

**Note**: With the routing strategy change, `::text` queries now skip CX entirely, so the panic scenario is largely avoided. The `catch_unwind` remains as a safety net for any other unexpected CX panics on complex queries.

---

## SSE Progress Events

Each extraction step reports real-time progress to the frontend via Server-Sent Events:

```
Phase 1: "connecting"              — PG connection established
Phase 2: "fetching (4× parallel CSV)"  — or "fetching (binary)" for CX
Phase 3: "fetched 953.4 MB (4× parallel)" — total data received
Phase 4: "writing parquet"         — DuckDB CSV→Parquet conversion
Phase 5: "fallback: single CSV"    — if parallel failed (shown before retry)
```

The frontend displays these phases in a fixed-width column next to each step, along with a live elapsed timer that ticks every 200ms.

---

## Summary of Changes

### Files modified

| File | Changes |
|------|---------|
| `server/src/pipeline/tree_executor.rs` | Parallel CSV extraction, routing strategy, `sample_size=-1`, `catch_unwind`, progress events |
| `server/src/handlers/pipeline_handler.rs` | `phase` field in SSE node_event JSON |
| `src/components/workspace/DataViewWorkspace.tsx` | Live timer hook, phase display, fixed-width step columns |

### Key decisions

1. **Parallel CSV over ConnectorX for simple scans** — CX binary protocol has no advantage when the bottleneck is network throughput, and it introduces risk (panics, type issues). Parallel CSV uses proven PG COPY protocol with guaranteed type safety.

2. **4 streams as default parallelism** — Matches typical PG `max_connections` headroom. Each stream opens its own connection. 4× provides near-linear speedup for network-bound transfers without overwhelming the PG server.

3. **`sample_size=-1` everywhere** — Small cost (~5-10s for 900MB files), eliminates an entire class of type detection failures. Worth it for pipeline reliability.

4. **`::text` queries → single CSV, not parallel** — The `::text` cast is already in the SELECT list. Adding `hashtext(col::text)` to the WHERE clause works, but these queries are typically small (ph_master is 535K rows). Single CSV is fast enough and simpler.

5. **ConnectorX kept for complex queries** — JOINs, subqueries, and stored procedures still go through CX (with `catch_unwind` safety). CX's binary protocol is genuinely faster for these cases when it works, and the routing ensures it only gets queries it can handle.
