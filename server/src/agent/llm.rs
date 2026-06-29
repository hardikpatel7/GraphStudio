//! LLM facade. Everything outside this module talks to a `LlmRunner` trait so
//! the underlying client (Rig in v1, async-openai later for specific models)
//! is swappable per model.
//!
//! v1: Rig backend (`rig_backend::Runner`). The async-openai backend lives
//! behind a stub that returns `Unimplemented` so the matrix of
//! (provider, model, backend) compiles end-to-end.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;

use super::meter::hook::SseEvent;
use super::meter::writer::MeterTx;
use crate::AppState;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Backend {
    Rig,
    AsyncOpenAi,
}

impl Backend {
    pub fn from_str(s: &str) -> Self {
        match s {
            "async_openai" => Backend::AsyncOpenAi,
            _              => Backend::Rig,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ModelEntry {
    pub provider: String,
    pub model: String,
    pub display_name: String,
    pub backend: Backend,
}

#[derive(Clone, Debug, Default)]
pub struct TurnSummary {
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub latency_ms: i64,
    pub final_text: String,
}

/// Inputs every backend needs. Held by value because Rig's
/// `PromptHook` is `Clone`-bound and Tokio mpsc senders clone trivially.
pub struct RunnerInput {
    pub state: Arc<AppState>,
    pub workspace_kind: super::tools::WorkspaceKind,
    pub model: ModelEntry,
    pub prompt_id: String,
    pub sse_tx: mpsc::Sender<SseEvent>,
    pub meter_tx: MeterTx,
    /// Pre-discovered schema overview (DataView ids / ClickHouse table
    /// names / etc.) injected into the system prompt so the model doesn't
    /// burn turns on schema exploration. Empty string when discovery
    /// hasn't run or yielded nothing.
    pub schema_hint: String,
    /// Per-call addendum appended to the system prompt AFTER the catalog.
    /// Used by the dashboard widget runner to enforce an output contract
    /// like "Reply with exactly one ```chart block of type bar." Empty
    /// for the normal chat path.
    pub addendum: String,
}

#[async_trait]
pub trait LlmRunner: Send + Sync {
    async fn run_turn(&self, input: RunnerInput, user_text: &str) -> Result<TurnSummary>;
}

pub mod rig_backend {
    use std::time::Instant;

    use anyhow::{anyhow, Context, Result};
    use async_trait::async_trait;
    use rig::client::{CompletionClient, ProviderClient};
    use rig::completion::Prompt;
    use rig::providers::{anthropic, openai};

    use super::super::meter::hook::MeteringHook;
    use super::super::tools::for_kind;
    use super::{LlmRunner, ModelEntry, RunnerInput, TurnSummary};

    pub struct Runner;

    pub fn build() -> Runner { Runner }

    /// System prompt the model sees on every Inventory-workspace turn. Frames
    /// the available tools and how to use them. Other workspace kinds get a
    /// "Backend not yet configured" reply via the routes layer, not here.
    fn preamble_for(kind: super::super::tools::WorkspaceKind) -> &'static str {
        match kind {
            super::super::tools::WorkspaceKind::Inventory =>
                "# CRITICAL RULE — read this first\n\
                 Every message you emit is exactly one of:\n\
                 - **TOOL TURN**: contains at least one tool call.\n\
                 - **FINAL TURN**: contains a complete answer with concrete numbers; NO tool calls.\n\
                 \n\
                 A message that says *\"I will…\"*, *\"Let me try…\"*, *\"Here's the SQL you could run…\"*, \
                 *\"Could you tell me…\"*, *\"Based on typical conventions…\"*, or that shows the user code/SQL \
                 to run themselves — without calling a tool — is a SILENT FAILURE. The user sees only \
                 \"…\" and your conversation ends.\n\
                 \n\
                 If a tool returned an error, your NEXT message MUST contain a corrected tool call. \
                 If a query failed because a table/column name was wrong, your NEXT message MUST run \
                 a discovery query (e.g. `duckdb_query` with `SELECT table_schema, table_name FROM \
                 information_schema.tables WHERE table_name ILIKE '%<keyword>%' LIMIT 50`). Never \
                 hand the user the SQL to run; you run it.\n\
                 \n\
                 ---\n\
                 \n\
                 You are a GraphStudio retail-inventory planning assistant. You have access to tools \
                 that read this tenant's metadata (DataViews, Sources, Graphs, Connections) and that \
                 query the underlying data stores (DuckDB, ClickHouse). \
                 \n\n## How to answer\n\
                 1. **Keep reasoning across tool calls.** Each result should inform your next move — don't stop after one call if more data is needed.\n\
                 2. **Use the pre-discovered catalog below — don't re-discover.** A section starting with `# This tenant's catalog` is appended after the instructions. It already lists every DataView / graph / ClickHouse database / table available, AND for the most-likely-to-be-relevant ClickHouse tables it includes a `sample:` row with real values from that table.\n\
                    The sample rows are EVIDENCE about what values to filter on. If you see `sample: { ..., \"l1_name\"=\"JEWELRY\", \"fiscal_year\"=2026, ... }` in a table's entry, that table HAS rows matching those values — pick THAT table, and filter with the exact casing shown (`'JEWELRY'`, not `'Jewelry'`). Conversely, if no cataloged table's sample shows the values from the user's question, the data genuinely may not exist for that combination — report this honestly.\n\
                    Do NOT call `list_dataviews` / `SHOW DATABASES` / `SHOW TABLES` unless the catalog section is empty or missing the name you need. Use `DESCRIBE <db>.<table>` only when a sample row in the catalog has fewer columns than the question requires.\n\
                 3. **Confirm the shape.** `describe_dataview` / `introspect_dataview` tell you the columns and source binding — use them before crafting filters or group_by clauses.\n\
                 4. **Prefer the structured read.** `dataview_read` returns rows + total + columns and accepts `limit`, `filters`, `group_by`, `aggregates`, `having`, `node_kind`. Fall back to `duckdb_query` only when you need SQL the read tool can't express (cross-DataView joins, ad-hoc reshaping).\n\
                 5. **Keep each tool result tight.** Set a small `limit` (10-50 usually), use `group_by`+`aggregates` to summarize instead of pulling raw rows, and filter to the user's actual scope. The system will truncate oversize results and tell you (`truncated: true` + a `truncation_note`) — when that happens, refine your query and try again rather than answering with partial data.\n\
                 6. **Budget.** You have 16 turns per question — plenty for multi-step reasoning, but not infinite. Spend them on discovery + targeted reads, not on describing every DataView in sight.\n\
                 7. **Final answer.** Be concise. Lead with the concrete numbers, cite the table/column names you queried, and STOP. DO NOT add a paragraph of meta-commentary about what couldn't be done, what \"further inspection would reveal\", or what \"adjustments are being made\". Those phrases read as cop-outs. If part of the request genuinely failed, mention it in ONE sentence at the end (\"Couldn't break down by category — GROUP BY threw a syntax error; ask me to retry that piece specifically.\") and stop there.\n\
                 \n\
                 ## CRITICAL — keep going until you're done\n\
                 Every message you emit must be exactly one of:\n\
                 - **Tool turn**: at least one tool call. The runtime executes it, feeds the result back, and asks you for the next message. You may add a short note alongside the call but it is OPTIONAL.\n\
                 - **Final turn**: a complete answer to the user, with NO tool calls.\n\
                 \n\
                 There is no third option. A message with neither a tool call NOR a complete answer silently ends the conversation and the user sees only \"…\".\n\
                 \n\
                 ### Banned phrasing on a non-final turn\n\
                 If your message contains any of these, you MUST also emit a tool call in the same message; otherwise you have ended the conversation prematurely:\n\
                 - \"I will…\", \"I'll…\", \"Let me…\", \"Let's…\", \"Next, I'll…\", \"I'm going to…\"\n\
                 - \"I'll review/examine/check/explore…\"\n\
                 - \"Based on typical naming conventions…\"\n\
                 - \"If you have any additional information…\" / \"could you tell me…\" / \"would you like me to…\" — DO NOT ask the user clarifying questions mid-task. Make a best-effort attempt with what you have. The user gave you a question and tools; use them.\n\
                 \n\
                 ### When a tool errors or returns a truncated result\n\
                 - SQL syntax error → re-issue a corrected query in the SAME turn.\n\
                 - \"table not found\" / \"schema not found\" → discover before retrying. Examples:\n\
                   * `duckdb_query` with `SELECT table_schema, table_name FROM information_schema.tables WHERE table_name ILIKE '%sales%' LIMIT 50`\n\
                   * `duckdb_query` with `SHOW TABLES FROM <schema>`\n\
                 - Truncated result (`truncated: true`) → re-call with a tighter `limit`, a `WHERE` filter, or a `GROUP BY` rollup. Don't ask the user — refine yourself.\n\
                 - Discovery yields too many tables → narrow with `SHOW TABLES FROM <db> LIKE '%<keyword>%'` so the result fits without truncation.\n\
                 - **Some columns NULL while others have values** → don't conclude \"sales not reported\". The column you SUM'd is probably the wrong one. Run `DESCRIBE <db>.<table>` to enumerate columns and look for alternatives: try `sum_sales_dollars`, `written_sales`, `sales_amount`, `total_sales`, `net_sales`, `gross_sales`. Or the data may live in a sibling table — check `SHOW TABLES FROM <db>` for one with a related name (e.g. `<base>_sales`, `<base>_finance`). Re-query with the corrected column or table. Always report the column/table you actually used.\n\
                 - **Empty/null in the chosen table** → before concluding \"no data\", verify by trying another candidate. THIS IS NOT OPTIONAL. The catalog above lists multiple tables; if your first pick returns zero rows, your next tool call MUST be a query against a different table from the catalog whose name suggests recorded/historical data. Only after 2-3 candidate tables all return empty are you allowed to say \"no data\". Two checks:\n\
                   * **Other periods**: run `SELECT DISTINCT fiscal_year FROM <table> ORDER BY 1 DESC LIMIT 5`. If FY2026 isn't in the list but FY2025 is, the data simply hasn't been loaded for the current period — say so explicitly.\n\
                   * **Other tables**: enumerate `SHOW TABLES FROM <db>` looking for related names (e.g. `<base>_actual`, `<base>_historical`, `<base>_daily`, `<base>_weekly`) vs work-in-progress / forecast tables (`<base>_wp`, `<base>_forecast`, `<base>_expected`). Prefer the table whose name suggests recorded data over one that suggests planning data.\n\
                   Re-run the same question against the alternative table or period before concluding.\n\
                 - **Suspiciously uniform results** (e.g. \"16 items in every class\") → you probably have an unintended `LIMIT` or are counting rows in a sampled subset. Re-run without `LIMIT` and with `COUNT(*)` directly, scoped to the actual filter. Verify the result varies before reporting.\n\
                 - **Empty result / all NULLs** → do NOT immediately conclude \"no data\". Your filter values may be wrong. BEFORE saying \"no data\":\n\
                   * Run a counts probe without filters: `SELECT COUNT(*) FROM <table>` to confirm the table is populated.\n\
                   * Run a distinct probe for each filter column: `SELECT DISTINCT <column> FROM <table> LIMIT 20`. Maybe `l1_name` is stored as `'JEWELRY'`, `'jewelry'`, or `'Jewelry & Accessories'` — not `'Jewelry'`. Maybe `fiscal_year` uses `2026` or `'FY2026'` or `202601`.\n\
                   * Re-run the original query with the actual values you found.\n\
                   Only after these probes confirm the values are absent should you report \"no matching rows\". Always cite which values you tried and what the column actually contains.\n\
                 - Permission / 403 / not configured → that error IS your final answer; relay it to the user.\n\
                 \n\
                 ### Correct vs incorrect example\n\
                 User: *\"How many distinct l1_name values are there?\"*\n\
                 \n\
                 ✅ Correct sequence:\n\
                 - Turn 1: call `list_dataviews`\n\
                 - Turn 2: from the response pick the most likely DataView; call `describe_dataview` on it\n\
                 - Turn 3: call `dataview_read` with a `group_by` aggregation, or `duckdb_query` with `SELECT COUNT(DISTINCT l1_name) FROM ...`\n\
                 - Turn 4: final answer with the number\n\
                 \n\
                 ❌ Incorrect (this is what you must NOT do):\n\
                 - Turn 1: call `list_dataviews`\n\
                 - Turn 2: emit text *\"Let me look for a relevant DataView and run a query. Based on typical conventions there should be...\"* with no tool call → ends the conversation; user sees only your narration.\n\
                 \n\
                 ## Output formatting (the UI renders Markdown)\n\
                 - When you return more than ~3 rows of data, format them as a **Markdown table** (pipe syntax with a `---` header row).\n\
                 - Keep tables tight: <= ~10 rows by default; sort/filter to surface what the user asked for and mention how many were truncated.\n\
                 - Use bullet lists for enumerations (e.g. \"top 5 reasons …\").\n\
                 - Wrap identifiers — table names, column names, DataView ids, SQL fragments — in backticks: `dv_articles_woc`, `l1_name`.\n\
                 - For multi-line SQL or JSON you ran, use a fenced code block tagged with the language (```sql … ``` or ```json … ```).\n\
                 - Use **bold** to highlight a single key number when answering a quantitative question.\n\
                 \n\
                 ## Charts (preferred over a table when the shape fits)\n\
                 Emit a fenced block tagged `chart` containing a JSON object. Always add a one-sentence \
                 lede *above* the chart explaining what it shows. Available shapes:\n\
                 \n\
                 - **kpi** — single headline number.\n\
                 ```chart\n\
                 { \"type\": \"kpi\", \"label\": \"Articles below reorder\", \"value\": 38, \"hint\": \"store 1042, today\" }\n\
                 ```\n\
                 \n\
                 - **bar** — ranked categories (3-10 items). Sort descending unless the user asked otherwise.\n\
                 ```chart\n\
                 { \"type\": \"bar\", \"title\": \"Articles per l1_name (top 5)\", \"data\": [\n\
                   { \"label\": \"Apparel\",  \"value\": 1248 },\n\
                   { \"label\": \"Footwear\", \"value\": 942 },\n\
                   { \"label\": \"Home\",     \"value\": 711 }\n\
                 ] }\n\
                 ```\n\
                 \n\
                 - **line** — time series / trend. `x` is usually a date/week, `y` a number.\n\
                 ```chart\n\
                 { \"type\": \"line\", \"title\": \"Weekly receipts\", \"data\": [\n\
                   { \"x\": \"W01\", \"y\": 1200 }, { \"x\": \"W02\", \"y\": 1340 }\n\
                 ] }\n\
                 ```\n\
                 \n\
                 - **pie** — composition / share (<= 6 slices).\n\
                 ```chart\n\
                 { \"type\": \"pie\", \"title\": \"Stock status\", \"data\": [\n\
                   { \"label\": \"In stock\", \"value\": 80 }, { \"label\": \"Low\", \"value\": 15 }, { \"label\": \"Out\", \"value\": 5 }\n\
                 ] }\n\
                 ```\n\
                 \n\
                 Pick the chart that matches the question shape. Use a Markdown table when the data is dense (>10 rows) or doesn't fit any shape. Don't both chart and table the same data.\n\
                 \n\
                 Do not invent tool names or dataview ids — discover them via the listing tools.",
            _ => "Backend for this workspace is not yet configured.",
        }
    }

    /// Translate a Rig `PromptError` to the string shape our SSE route maps
    /// to a friendly UI message. Used by both the initial turn and the
    /// narration-rescue retry below.
    fn map_rig_err(e: rig::completion::request::PromptError, max_turns: usize) -> anyhow::Error {
        // Walk the std::error::Error source chain so reqwest-layer details
        // ("connection reset", "timeout", "invalid certificate") surface
        // instead of getting flattened to the generic "Http client error".
        // Each frame is logged at WARN so cargo-watch shows the trail.
        use std::error::Error as _; // bring `.source()` into scope
        let mut chain: Vec<String> = vec![e.to_string()];
        {
            let err: &dyn std::error::Error = &e;
            let mut current = err.source();
            while let Some(src) = current {
                chain.push(src.to_string());
                current = src.source();
            }
        }
        let joined = chain.join("  ↘  ");
        tracing::warn!("[llm] Rig prompt error chain: {joined}");

        if joined.contains("MaxTurnsError") || joined.contains("max turn limit") {
            anyhow!(
                "max_turns_reached: model didn't converge within {max_turns} turns. \
                 Try a more specific question, or break it into smaller asks."
            )
        } else {
            anyhow!("rig prompt failed: {joined}")
        }
    }

    /// Heuristic for "the model gave up without doing anything". Looks for
    /// the telltale narration patterns alongside a *lack* of concrete
    /// numeric content. Tuned to favor false negatives over false positives —
    /// we'd rather miss a few premature stops than re-prompt on every
    /// answer.
    fn looks_like_premature_stop(output: &str) -> bool {
        let t = output.trim();
        if t.is_empty() { return true; }
        let lower = t.to_lowercase();

        // Tier 1 — strong "I'm giving up / planning instead of doing"
        // signals. Fires regardless of incidental digits.
        let gives_up = [
            // direct user-asks
            "could you ", "could you tell", "could you specify", "could you provide",
            "would you ", "would you like",
            "should i ", "should we ",
            "do you have", "if you have",
            "let me know if",
            "please specify", "please tell", "please provide", "please confirm",
            "can you tell", "can you specify", "can you provide",
            // apologetic give-ups
            "i apologize", "apologies for", "i'm sorry",
            // "let me describe what I would do" patterns
            "let's take a step back", "take a step back",
            "let's reevaluate", "let's re-evaluate", "let's re evaluate",
            "we need to ensure", "we need to verify", "we need to confirm",
            "outline the approach", "outline the steps", "outline what would",
            "best action is",
            "the typical approach", "typical approach",
            "ensure access to", "with correct table",
            "if possible, fetching", "if possible, providing",
            "invite other methods", "at your request", "for your request",
            "this would typically", "would typically work",
            "needs further refinement", "need further refinement",
            "might need further", "further refinement",
            "would be appreciated", "would help",
            "would be pivotal", "will be pivotal",
            "if you're aware", "if you are aware",
            "this information will", "this information would",
            "may rectify", "would rectify",
            "any specific schema", "any insight into",
            "further clarification",
            // trailing-off / hedging patterns: model has partial result
            // but bails on investigating further
            "may not be available or not reported",
            "might be an issue",
            "future steps might", "future steps could",
            "further examination", "further investigation",
            "verifying constraints",
            "if these specifics", "if specifics",
            "utilizing other tables", "utilizing other resources",
            "may not be reported", "not reported in this",
            "might need updating", "might need confirming",
            "suggests that there", "suggests there might",
            "indicating either", "either an absence",
            "potential delay", "delay in data updates",
            "actual sales and metric values are missing",
            "absence of recorded activity",
            "values are missing in this dataset",
            "within the selected timeline",
            "data is not updated", "data is not populated",
            "no items currently categorized", "no items currently",
            "no sales recorded",
            "for a more detailed analysis",
            "if there are other related",
            "provide additional context",
            "please ensure that the data",
            "appropriately populated",
            "implies either", "this implies either",
            "no distinct items recorded",
            "not available or recorded",
            "no specific item sales data",
            "no recorded key, secondary",
            "if further analysis",
            "different approach is needed",
            "please let me know",
            "no items recorded",
            "no records were returned",
            "no current records",
            "might be a discrepancy",
            "verify data input",
            "check alternate databases",
            "may need to verify",
            "have been misconfigured",
            "if you believe there",
            // open-ended planning-prose
            "let's reevaluate what", "let's evaluate", "let's examine",
            "by examining if",
            "to successfully form",
        ];
        if gives_up.iter().any(|p| lower.contains(p)) {
            return true;
        }
        // Catch-all: messages that read like a plan ("we need to", "we should",
        // "let's …") but contain neither a code block nor a tabular structure
        // — those are unambiguously planning without doing.
        let planning_starts = ["we need to", "we should ", "let's ", "lets "];
        let plans = planning_starts.iter().any(|p| lower.contains(p));
        let shows_real_output = t.contains("```") || t.contains("|---") || t.contains("| ---") || t.contains("\n| ");
        if plans && !shows_real_output {
            return true;
        }

        // Tier 3 — "concluded empty without diagnosing". Phrases like
        // "no concrete numbers are available", "no records matching",
        // "null values indicating no records" surface when the model
        // accepts an empty query result without first probing whether
        // the filter values exist. Rescue ONCE with a diagnostic nudge.
        // (Legitimate empty answers can still happen — but the second
        // attempt's prompt forces the DISTINCT probe before concluding.)
        let empty_without_probe = [
            "no concrete numbers are available",
            "no records matching",
            "no data found",
            "no matching records",
            "null values, indicating",
            "may not be records matching",
            "there may not be records",
        ];
        if empty_without_probe.iter().any(|p| lower.contains(p)) {
            return true;
        }
        // Trailing question mark on a long message also reads as a
        // give-up question to the user. Skip very short replies (could
        // be a legit clarification in a question-shaped greeting).
        if t.ends_with('?') && t.len() > 80 {
            return true;
        }

        // Tier 2 — pure intent narration without action. Combine with
        // the "no concrete digits" check so a real answer that uses
        // "I'll mention" or similar isn't flagged.
        let narration = [
            "let me try", "let's try", "let me check", "let me look",
            "i'll try", "i'll check", "i'll attempt", "i'll explore", "i'll review", "i'll run",
            "i will try", "i will attempt", "i will check", "i will explore",
            "next, i'll", "next, i will",
            "based on typical", "typical naming",
            "you could run", "you can run", "please ensure",
        ];
        if narration.iter().any(|p| lower.contains(p)) {
            // Need at least a small number sequence (3+ digits) to count
            // as a real answer. Years like "2026" alone don't qualify.
            let has_real_number = t
                .split_whitespace()
                .any(|w| {
                    let digits: String = w.chars().filter(|c| c.is_ascii_digit()).collect();
                    // 3+ digits OR contains a comma-formatted number OR contains a $.
                    digits.len() >= 3
                        || w.contains(',') && w.chars().any(|c| c.is_ascii_digit())
                        || w.contains('$') && w.chars().any(|c| c.is_ascii_digit())
                });
            return !has_real_number;
        }

        false
    }

    #[async_trait]
    impl LlmRunner for Runner {
        async fn run_turn(&self, input: RunnerInput, user_text: &str) -> Result<TurnSummary> {
            let tools = for_kind(input.state.clone(), input.workspace_kind);
            // Combine the static preamble with: (1) the pre-discovered
            // schema overview, (2) an optional per-call addendum (used by
            // the dashboard widget runner to enforce an output contract).
            // Each section is appended only when non-empty so the
            // preamble doesn't carry empty headers.
            let base = preamble_for(input.workspace_kind);
            let mut preamble = String::from(base);
            if !input.schema_hint.trim().is_empty() {
                preamble.push_str("\n\n---\n\n# This tenant's catalog (pre-discovered — use these names, don't re-list)\n\n");
                preamble.push_str(&input.schema_hint);
            }
            if !input.addendum.trim().is_empty() {
                preamble.push_str("\n\n---\n\n# Output contract for this turn\n\n");
                preamble.push_str(&input.addendum);
            }
            let hook = MeteringHook::new(
                input.prompt_id.clone(),
                input.sse_tx.clone(),
                input.meter_tx.clone(),
            );
            // `max_turns` bounds the model+tool feedback loop. A "turn" is one
            // model call: the model emits assistant text + zero-or-more tool
            // calls, every tool runs, the loop tops up the history and asks
            // for another turn. 16 gives room for legit multi-step questions
            // (list → describe several → introspect → read) without letting
            // the model spiral on dead-ends. Tool concurrency lets the model
            // request several reads in one turn and have them run in parallel.
            const MAX_TURNS: usize = 16;
            const TOOL_CONCURRENCY: usize = 4;
            const RESCUE_ATTEMPTS: usize = 2;
            const NUDGES: [&str; 2] = [
                // First nudge: clarify the contract + give a discovery
                // recipe. Most premature stops happen because the model
                // hasn't sampled what's actually in the data.
                "Your previous message wasn't a complete answer. You MUST act now, not narrate. \
                 Do not show SQL you didn't run. Do not ask me for clarification.\n\n\
                 If a prior query returned NO ROWS or NULLs, the filter values are probably \
                 wrong — not the data missing. Before saying \"no data\":\n\
                 1. Run `SELECT COUNT(*) FROM <table>` to confirm the table is populated.\n\
                 2. Run `SELECT DISTINCT <filter_column> FROM <table> LIMIT 20` for each filter \
                    column you used. The literal `'Jewelry'` may actually be `'JEWELRY'`, \
                    `'jewelry'`, or `'Jewelry & Accessories'` in the data.\n\
                 3. Re-run with the actual values you discovered.\n\n\
                 If you genuinely haven't run any tool yet, start with `SHOW DATABASES` (clickhouse) \
                 or `SELECT table_schema, table_name FROM information_schema.tables LIMIT 50` (duckdb).",
                // Second nudge: final chance, narrow further.
                "STILL no complete answer with numbers. Final attempt.\n\
                 - Pick the SINGLE most likely table from prior results.\n\
                 - Run `DESCRIBE <db>.<table>` (or `SELECT * FROM <table> LIMIT 3`) to confirm its columns and a row sample.\n\
                 - Run ONE final query that the user's question maps to — even if partial.\n\
                 - Return whatever number you got, citing the exact table + columns + filter values you used. \
                 If you confirmed via DISTINCT that the filter values aren't present, say so explicitly and list what IS present.\n\
                 Do not apologize. Do not ask the user. This is your final message — make it useful.",
            ];
            let t0 = Instant::now();

            // The prompt-loop body is identical for OpenAI / Anthropic /
            // any future Rig provider — only the `Agent<M>` concrete
            // type differs. A local macro lets us share the body
            // without writing a generic helper (the trait bounds for
            // CompletionClient + CompletionModel + the hook generics
            // get verbose).
            macro_rules! run_loop {
                ($agent:expr) => {{
                    let agent = $agent;
                    let mut resp = agent
                        .prompt(user_text)
                        .with_hook(hook.clone())
                        .max_turns(MAX_TURNS)
                        .with_tool_concurrency(TOOL_CONCURRENCY)
                        .extended_details()
                        .await
                        .map_err(|e| map_rig_err(e, MAX_TURNS))?;
                    // "Narration / give-up" rescue. Some weaker models
                    // (especially gpt-4o-mini) end a multi-step task by
                    // apologizing and outlining what they would do instead
                    // of doing it. Detect and re-prompt up to
                    // RESCUE_ATTEMPTS times with progressively more
                    // directive nudges.
                    for attempt in 0..RESCUE_ATTEMPTS {
                        if !looks_like_premature_stop(&resp.output) {
                            break;
                        }
                        let history_carry: Vec<rig::message::Message> =
                            resp.messages.clone().unwrap_or_default();
                        tracing::info!("[agent] narration rescue attempt {} of {}", attempt + 1, RESCUE_ATTEMPTS);
                        resp = agent
                            .prompt(NUDGES[attempt])
                            .with_history(history_carry)
                            .with_hook(hook.clone())
                            .max_turns(MAX_TURNS)
                            .with_tool_concurrency(TOOL_CONCURRENCY)
                            .extended_details()
                            .await
                            .map_err(|e| map_rig_err(e, MAX_TURNS))?;
                    }
                    resp
                }};
            }

            // Dispatch on `provider`. OpenAI uses `.completions_api()`
            // to pin Chat Completions (avoids Rig's Responses-API
            // `strict: true` clash with our `#[serde(default)]` tool
            // schemas); Anthropic only has Messages API so no such
            // toggle. Both call `client.agent(model)` which is the
            // same `CompletionClient` trait method.
            let resp = match input.model.provider.as_str() {
                "openai" => {
                    let client = openai::Client::from_env()
                        .map_err(|e| anyhow!("OpenAI client init (env): {e}"))?
                        .completions_api();
                    let agent = client
                        .agent(&input.model.model)
                        .preamble(&preamble)
                        .tools(tools)
                        .build();
                    run_loop!(&agent)
                }
                "anthropic" => {
                    let client = anthropic::Client::from_env()
                        .map_err(|e| anyhow!("Anthropic client init (env): {e}"))?;
                    let agent = client
                        .agent(&input.model.model)
                        .preamble(&preamble)
                        .tools(tools)
                        .build();
                    run_loop!(&agent)
                }
                other => return Err(anyhow!("unsupported provider `{other}` — expected `openai` or `anthropic`")),
            };
            Ok(TurnSummary {
                tokens_in: resp.usage.input_tokens as i64,
                tokens_out: resp.usage.output_tokens as i64,
                latency_ms: t0.elapsed().as_millis() as i64,
                final_text: resp.output,
            })
        }
    }
}

pub mod async_openai_backend {
    use anyhow::Result;
    use async_trait::async_trait;

    use super::{LlmRunner, RunnerInput, TurnSummary};

    pub struct Runner;
    pub fn build() -> Runner { Runner }

    #[async_trait]
    impl LlmRunner for Runner {
        async fn run_turn(&self, _input: RunnerInput, _user_text: &str) -> Result<TurnSummary> {
            anyhow::bail!("async_openai backend not implemented in v1")
        }
    }
}

pub fn build_runner(backend: &Backend) -> Box<dyn LlmRunner> {
    match backend {
        Backend::Rig         => Box::new(rig_backend::build()),
        Backend::AsyncOpenAi => Box::new(async_openai_backend::build()),
    }
}
