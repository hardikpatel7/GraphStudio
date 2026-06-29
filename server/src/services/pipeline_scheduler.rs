//! Pipeline scheduler — Phase 2 of misty-hinton.
//!
//! Reads each row in `pipelines`, parses its `trigger` JSON, and spawns a
//! per-pipeline subscriber task that listens for the events its trigger
//! depends on (CDC source changes, RCL store replacements). Events are
//! debounced (default 5s) so a burst of CDC changes coalesces into a single
//! pipeline run rather than N runs.
//!
//! Trigger kinds handled here:
//!   - `Manual`     → skipped; runs only via `POST /api/pipelines/{id}/tree-stream`
//!   - `Cdc`        → subscribe to `state.cdc_change_tx`, filter by source_ids
//!   - `RclChange`  → subscribe to `RuleStore::subscribe()` watch
//!   - `Composed`   → union of the above
//!   - `Scheduled`  → not implemented yet (logged + skipped)
//!
//! Manual `POST /api/article-selection/materialize` and the SSE handler
//! remain available; the scheduler does not displace them. Per-tenant the
//! existing `pipeline_run_lock` already serializes runs across all sources
//! (manual + scheduled).

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::broadcast;

use crate::AppState;
use crate::handlers::pipeline_v2::{PipelineRunOptions, execute_pipeline_run};

/// Default debounce — a burst of CDC events within this window coalesces to
/// one run. Conservative; the operator may want to tune via environment.toml
/// later. Hardcoded for Phase 2.
const DEFAULT_DEBOUNCE: Duration = Duration::from_secs(5);

/// Wait this long for `state.rcl_store` to be populated before giving up on
/// `RclChange` triggers. RCL is started in the background so the scheduler
/// races boot.
const RCL_BOOT_WAIT: Duration = Duration::from_secs(30);

/// Published by `handlers::sources::cdc_start` whenever a CDC consumer
/// applies a change. Subscribers filter by `source_id` and use it to wake
/// their debounce timer.
#[derive(Debug, Clone)]
pub struct CdcChangeEvent {
    pub source_id: String,
    pub lsn: String,
}

/// Boot entry point. Reads pipelines, spawns one task per non-Manual trigger,
/// and returns once tasks are scheduled. The tasks themselves run for the
/// process lifetime.
pub fn start(state: Arc<AppState>) {
    let pipelines = match load_pipeline_triggers(&state) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "[scheduler] failed to load pipelines; scheduler not started");
            return;
        }
    };

    let mut spawned = 0;
    for (pipeline_id, trigger) in pipelines {
        if matches!(trigger, pipeline::PipelineTrigger::Manual) {
            continue;
        }
        let cdc_ids: Vec<String> = trigger.cdc_source_ids().into_iter().map(String::from).collect();
        let listens_for_rcl = trigger.listens_for_rcl();
        if cdc_ids.is_empty() && !listens_for_rcl {
            // E.g. a Scheduled-only trigger — not implemented yet.
            tracing::warn!(
                pipeline = %pipeline_id, trigger = ?trigger,
                "[scheduler] trigger has no CDC sources or RclChange and Scheduled isn't implemented yet — skipping"
            );
            continue;
        }
        let st = state.clone();
        tokio::spawn(run_pipeline_subscriber(st, pipeline_id, cdc_ids, listens_for_rcl));
        spawned += 1;
    }
    tracing::info!("[scheduler] started {} pipeline subscriber task(s)", spawned);
}

/// Pull `(id, trigger)` pairs from SQLite. Triggers parse from the `trigger`
/// JSON column; rows that fail to parse default to `Manual` (with a warning)
/// so a malformed trigger doesn't take down the scheduler.
fn load_pipeline_triggers(state: &AppState) -> anyhow::Result<Vec<(String, pipeline::PipelineTrigger)>> {
    let rows = state.db.query("SELECT id, trigger FROM pipelines", &[])?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let id = match row.get("id").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let trigger_json = row.get("trigger").cloned().unwrap_or_else(|| Value::Null);
        let trigger = parse_trigger(&id, trigger_json);
        out.push((id, trigger));
    }
    Ok(out)
}

fn parse_trigger(pipeline_id: &str, raw: Value) -> pipeline::PipelineTrigger {
    let raw = match raw {
        Value::String(s) => match serde_json::from_str::<Value>(&s) {
            Ok(v) => v,
            Err(_) => return pipeline::PipelineTrigger::Manual,
        },
        Value::Null => return pipeline::PipelineTrigger::Manual,
        other => other,
    };
    match serde_json::from_value::<pipeline::PipelineTrigger>(raw) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(pipeline = %pipeline_id, error = %e, "[scheduler] malformed trigger JSON; defaulting to Manual");
            pipeline::PipelineTrigger::Manual
        }
    }
}

/// Per-pipeline event loop. Subscribes to its triggers, debounces, and fires
/// `execute_pipeline_run` when the debounce expires.
async fn run_pipeline_subscriber(
    state: Arc<AppState>,
    pipeline_id: String,
    cdc_source_ids: Vec<String>,
    listens_for_rcl: bool,
) {
    let cdc_set: std::collections::HashSet<String> = cdc_source_ids.iter().cloned().collect();
    let mut cdc_rx: broadcast::Receiver<CdcChangeEvent> = state.cdc_change_tx.subscribe();

    // RCL watch is set up best-effort. If RCL never publishes (disabled or
    // failed to start), the scheduler still handles CDC triggers.
    let mut rcl_rx = if listens_for_rcl {
        wait_for_rcl_subscribe(&state).await
    } else {
        None
    };

    tracing::info!(
        pipeline = %pipeline_id,
        cdc_sources = ?cdc_source_ids,
        rcl = listens_for_rcl,
        rcl_active = rcl_rx.is_some(),
        "[scheduler] subscriber online"
    );

    loop {
        let mut pending_trigger: Option<String> = None;

        // Wait for the first triggering event.
        tokio::select! {
            evt = cdc_rx.recv() => {
                match evt {
                    Ok(e) if cdc_set.contains(&e.source_id) => {
                        pending_trigger = Some(format!("cdc:{}", e.source_id));
                    }
                    Ok(_) => continue,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(pipeline = %pipeline_id, lagged = n, "[scheduler] cdc broadcast lagged");
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::info!(pipeline = %pipeline_id, "[scheduler] cdc channel closed; subscriber exiting");
                        return;
                    }
                }
            }
            changed = wait_for_rcl_change(rcl_rx.as_mut()) => {
                if changed {
                    pending_trigger = Some("rcl_change".to_string());
                }
            }
        }

        let Some(trigger_source) = pending_trigger else { continue };

        // Debounce window: keep absorbing additional events until quiet for DEFAULT_DEBOUNCE.
        let mut deadline = tokio::time::Instant::now() + DEFAULT_DEBOUNCE;
        loop {
            let sleep = tokio::time::sleep_until(deadline);
            tokio::pin!(sleep);
            tokio::select! {
                _ = &mut sleep => break,
                evt = cdc_rx.recv() => {
                    if let Ok(e) = evt {
                        if cdc_set.contains(&e.source_id) {
                            deadline = tokio::time::Instant::now() + DEFAULT_DEBOUNCE;
                        }
                    }
                }
                changed = wait_for_rcl_change(rcl_rx.as_mut()) => {
                    if changed {
                        deadline = tokio::time::Instant::now() + DEFAULT_DEBOUNCE;
                    }
                }
            }
        }

        // Run the pipeline. Hands off to a dedicated OS thread (same pattern
        // as the SSE handler) so the !Send DuckDB connection stays pinned.
        let report_rx = spawn_run(state.clone(), pipeline_id.clone(), trigger_source.clone());
        match report_rx.await {
            Ok(report) if report.success => {
                tracing::info!(
                    pipeline = %pipeline_id, trigger = %trigger_source,
                    time_to_cook_ms = report.total_ms, rows = report.total_rows,
                    "[scheduler] pipeline run complete"
                );
            }
            Ok(report) => {
                tracing::warn!(
                    pipeline = %pipeline_id, trigger = %trigger_source,
                    error = %report.error.unwrap_or_default(),
                    "[scheduler] pipeline run failed"
                );
            }
            Err(_) => {
                tracing::error!(pipeline = %pipeline_id, "[scheduler] pipeline run task dropped");
            }
        }
    }
}

/// Returns true if the watch receiver yielded a new value. None receiver →
/// pends forever (so `tokio::select!` works without an extra arm).
async fn wait_for_rcl_change(
    rx: Option<&mut tokio::sync::watch::Receiver<Arc<rcl::RuleSet>>>,
) -> bool {
    match rx {
        Some(rx) => match rx.changed().await {
            Ok(()) => true,
            Err(_) => {
                // Sender dropped — pend forever instead of looping hot.
                std::future::pending::<()>().await;
                false
            }
        },
        None => {
            std::future::pending::<()>().await;
            false
        }
    }
}

/// Wait up to `RCL_BOOT_WAIT` for `state.rcl_store` to be populated, then
/// return its watch receiver. None if RCL never came online.
async fn wait_for_rcl_subscribe(
    state: &AppState,
) -> Option<tokio::sync::watch::Receiver<Arc<rcl::RuleSet>>> {
    let deadline = tokio::time::Instant::now() + RCL_BOOT_WAIT;
    loop {
        {
            let guard = state.rcl_store.read().await;
            if let Some(store) = guard.as_ref() {
                return Some(store.subscribe());
            }
        }
        if tokio::time::Instant::now() >= deadline {
            tracing::warn!("[scheduler] RCL store not available within {:?}; RclChange triggers will not fire", RCL_BOOT_WAIT);
            return None;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Spawn an OS thread + private tokio runtime to execute the run, so the
/// !Send DuckDB connection stays pinned. Returns a oneshot the caller can
/// await for the report.
fn spawn_run(
    state: Arc<AppState>,
    pipeline_id: String,
    trigger_source: String,
) -> tokio::sync::oneshot::Receiver<crate::handlers::pipeline_v2::PipelineRunReport> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("build scheduler pipeline runtime");
        rt.block_on(async move {
            let opts = PipelineRunOptions {
                pipeline_id,
                skip_ids: Default::default(),
                mode_progress_interval: None,
                mode_quantify: false,
                trigger_source,
                // CDC-driven key extraction (the WAL row → ph_code lookup) is
                // future work; the cdc crate's on_lsn_update only surfaces
                // LSN today. Until that lands, scheduler-triggered runs do
                // a full recompute.
                partial_recompute_keys: Vec::new(),
                // Scheduler-triggered runs default to sequence; if a pipeline
                // is parallel-friendly, the operator can either rely on a
                // future per-pipeline default or trigger via the SSE path.
                parallel: false,
            };
            // Drop step events on the floor — scheduler runs are headless.
            let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
            let drain = tokio::spawn(async move { while event_rx.recv().await.is_some() {} });
            let report = execute_pipeline_run(state, opts, event_tx).await;
            let _ = drain.await;
            let _ = tx.send(report);
        });
    });
    rx
}
