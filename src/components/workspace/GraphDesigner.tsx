import { useEffect, useMemo, useState } from "react";
import {
  Network,
  Save,
  CheckCircle2,
  Hammer,
  RefreshCw,
  Loader2,
  AlertCircle,
  CircleAlert,
  Trash2,
} from "lucide-react";
import { useWorkspaceStore } from "@/stores/workspace";
import { SchemaSketch } from "./GraphDesigner/SchemaSketch";
import { ExplorePane } from "./GraphDesigner/ExplorePane";
import { FormView } from "./GraphDesigner/FormView";

// ─── Wire shapes ───────────────────────────────────────────────────────────

interface GraphRow {
  id: string;
  display_name: string;
  toml_text: string;
  last_validated_at: string | null;
  error_log: string | null;
  created_at?: string;
  updated_at?: string;
}

interface ValidationIssue {
  severity: "error" | "warning";
  code: string;
  message: string;
  location: string | null;
}

interface ValidateResponse {
  id: string;
  ok: boolean;
  issues: ValidationIssue[];
}

interface BuildResponse {
  id: string;
  ok: boolean;
  stats: {
    total_nodes: number;
    primary_metric_count: number;
    composite_metric_count: number;
    strings_interned: number;
    elapsed_ms: number;
    nodes_by_kind: Record<string, number>;
  };
}

interface StatsResponse {
  id: string;
  graph_version: number;
  node_count: number;
  string_count: number;
  kinds: { name: string; hierarchy: string; node_count: number }[];
  metrics: { name: string; source: string; column: string; rollup: string; is_composite: boolean }[];
  cross_edges?: { alias: string; kind_a: string; kind_b: string }[];
}

// ─── Component ─────────────────────────────────────────────────────────────

interface Props {
  graphId: string;
}

// Right-pane width bounds. Default fits the schema sketch + explore
// drill comfortably. User drag-resizes; persisted per-browser.
const RIGHT_MIN_PX = 320;
const RIGHT_MAX_PX = 900;
const RIGHT_DEFAULT_PX = 520;

/// GraphDesigner — TOML-edit + validate + build workflow for one v2 graph.
///
/// Three back-end ops:
///   - PUT /api/graphs/:id        — persist toml_text
///   - POST /api/graphs/:id/validate — metadata-only checks (Decisions 23, 26, 28, 31-36)
///   - POST /api/graphs/:id/build    — run build_graph against the tenant DuckDB
/// plus GET /api/graphs/:id/stats for the live snapshot inventory.
///
/// Save invalidates the previous validation (the server NULLs
/// last_validated_at on PUT); the user must re-run Validate to confirm
/// the edit. Build runs validation again internally — if the spec is
/// broken, the build step fails with a clear error.
export function GraphDesigner({ graphId }: Props) {
  const [row, setRow] = useState<GraphRow | null>(null);
  const [toml, setToml] = useState<string>("");
  const [stats, setStats] = useState<StatsResponse | null>(null);
  const [issues, setIssues] = useState<ValidationIssue[] | null>(null);
  const [busy, setBusy] = useState<
    "load" | "save" | "validate" | "build" | "delete" | null
  >(null);
  const [error, setError] = useState<string | null>(null);
  const [buildLog, setBuildLog] = useState<string | null>(null);
  const select = useWorkspaceStore((s) => s.select);

  // Right-pane tab. Per-graph persistence isn't worth the
  // localStorage churn for now; reset to Status on every reload.
  type RightTab = "status" | "schema" | "explore";
  const [rightTab, setRightTab] = useState<RightTab>("status");

  // Right-pane width — drag handle between editor (left) and the
  // Status/Schema/Explore tabs (right). Persisted so the user's
  // preferred split survives reloads. Bounded so neither pane
  // collapses past usability.
  const [rightWidth, setRightWidth] = useState<number>(() => {
    if (typeof window === "undefined") return RIGHT_DEFAULT_PX;
    const saved = parseInt(
      window.localStorage.getItem("ss.graph.right.width") || "",
      10,
    );
    if (Number.isFinite(saved) && saved >= RIGHT_MIN_PX && saved <= RIGHT_MAX_PX) {
      return saved;
    }
    return RIGHT_DEFAULT_PX;
  });
  useEffect(() => {
    window.localStorage.setItem("ss.graph.right.width", String(rightWidth));
  }, [rightWidth]);

  // Mirror of FormView's tree-splitter logic. Dragging LEFT grows
  // the right pane (so we subtract the delta, not add). Body cursor
  // + userSelect are stashed/restored to prevent text-selection
  // during the drag.
  const startRightResize = (e: React.MouseEvent) => {
    e.preventDefault();
    const startX = e.clientX;
    const startWidth = rightWidth;
    const onMove = (ev: MouseEvent) => {
      const next = Math.min(
        RIGHT_MAX_PX,
        Math.max(RIGHT_MIN_PX, startWidth - (ev.clientX - startX)),
      );
      setRightWidth(next);
    };
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
  };

  // Left-pane editing mode. Form is the default — TOML kept as
  // an escape hatch for power users (and for editing graph
  // sections whose form inspectors haven't shipped yet). Persisted
  // per-browser so it survives reloads.
  type LeftMode = "form" | "toml";
  const [leftMode, setLeftMode] = useState<LeftMode>(() => {
    if (typeof window === "undefined") return "form";
    const saved = window.localStorage.getItem("ss.graph.editor.mode");
    return saved === "toml" ? "toml" : "form";
  });
  useEffect(() => {
    window.localStorage.setItem("ss.graph.editor.mode", leftMode);
  }, [leftMode]);

  // Local dirty flag — true when the editor diverges from `row.toml_text`
  // (the last persisted shape). Saving clears it. Validation / build
  // when dirty warns the user that they're operating on the unsaved
  // text (the server reads the row, not the editor buffer).
  const dirty = useMemo(() => row != null && toml !== row.toml_text, [toml, row]);

  const loadRow = async () => {
    setBusy("load");
    setError(null);
    try {
      const r = await fetch(`/api/graphs/${encodeURIComponent(graphId)}`);
      if (!r.ok) throw new Error(await r.text());
      const data: GraphRow = await r.json();
      setRow(data);
      setToml(data.toml_text ?? "");
      // Parse any persisted validation issues so we re-render them
      // without forcing the user to re-validate after navigating away.
      if (data.error_log) {
        try {
          const parsed = JSON.parse(data.error_log) as ValidationIssue[];
          setIssues(parsed);
        } catch {
          setIssues(null);
        }
      } else {
        setIssues(null);
      }
    } catch (e: any) {
      setError(e?.message ?? String(e));
    } finally {
      setBusy(null);
    }
  };

  const loadStats = async () => {
    try {
      const r = await fetch(`/api/graphs/${encodeURIComponent(graphId)}/stats`);
      if (!r.ok) {
        setStats(null);
        return;
      }
      setStats(await r.json());
    } catch {
      setStats(null);
    }
  };

  useEffect(() => {
    void loadRow();
    void loadStats();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [graphId]);

  const doSave = async () => {
    setBusy("save");
    setError(null);
    try {
      const r = await fetch(`/api/graphs/${encodeURIComponent(graphId)}`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ toml_text: toml }),
      });
      if (!r.ok) throw new Error(await r.text());
      const data: GraphRow = await r.json();
      setRow(data);
      // PUT clears server-side last_validated_at / error_log — match locally.
      setIssues(null);
    } catch (e: any) {
      setError(e?.message ?? String(e));
    } finally {
      setBusy(null);
    }
  };

  const doValidate = async () => {
    setBusy("validate");
    setError(null);
    try {
      // If the editor diverges from the persisted row, save first so
      // validation runs on the user's intent — not the stale row.
      if (dirty) await doSave();
      const r = await fetch(`/api/graphs/${encodeURIComponent(graphId)}/validate`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
      });
      if (!r.ok) throw new Error(await r.text());
      const data: ValidateResponse = await r.json();
      setIssues(data.issues);
    } catch (e: any) {
      setError(e?.message ?? String(e));
    } finally {
      setBusy(null);
    }
  };

  /// Destructive — confirm() guards the call. On success we clear
  /// the workspace selection (drops us back to the empty state)
  /// and dispatch a `graphs-changed` event so the sidebar refetches
  /// without a full page reload.
  const doDelete = async () => {
    const ok = window.confirm(
      `Delete graph "${row?.display_name ?? graphId}"? This drops the SQLite row and the in-memory snapshot. Cannot be undone.`,
    );
    if (!ok) return;
    setBusy("delete");
    setError(null);
    try {
      const r = await fetch(`/api/graphs/${encodeURIComponent(graphId)}`, {
        method: "DELETE",
      });
      if (!r.ok) throw new Error(await r.text());
      // Tell the sidebar to refetch.
      window.dispatchEvent(new CustomEvent("graphs-changed"));
      // Drop the selection — the workspace renders the empty-state
      // hint, and the user can pick another graph from the sidebar.
      select(null);
    } catch (e: any) {
      setError(e?.message ?? String(e));
      setBusy(null);
    }
  };

  const doBuild = async () => {
    setBusy("build");
    setError(null);
    setBuildLog(null);
    try {
      if (dirty) await doSave();
      const r = await fetch(`/api/graphs/${encodeURIComponent(graphId)}/build`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
      });
      if (!r.ok) throw new Error(await r.text());
      const data: BuildResponse = await r.json();
      const stat = data.stats;
      setBuildLog(
        `built ${stat.total_nodes.toLocaleString()} nodes (${stat.primary_metric_count} primary metrics, ${stat.strings_interned.toLocaleString()} strings) in ${stat.elapsed_ms}ms`,
      );
      void loadStats();
    } catch (e: any) {
      setError(e?.message ?? String(e));
    } finally {
      setBusy(null);
    }
  };

  const errorCount = issues?.filter((i) => i.severity === "error").length ?? 0;
  const warningCount = issues?.filter((i) => i.severity === "warning").length ?? 0;

  return (
    <div className="h-full flex flex-col bg-gray-950 text-gray-200">
      {/* Header */}
      <div className="px-4 py-3 border-b border-gray-800 flex items-center gap-3">
        <Network size={14} className="text-blue-400" />
        <GraphTitleEditor
          graphId={graphId}
          displayName={row?.display_name ?? graphId}
          onChanged={() => { void loadRow(); }}
        />
        <span className="text-[10px] px-1.5 py-0.5 rounded bg-blue-900/40 text-blue-300 font-mono">
          graph
        </span>
        {dirty && (
          <span className="text-[10px] text-amber-400">unsaved</span>
        )}
        {row?.last_validated_at && !dirty && issues != null && (
          <span className="text-[10px] text-gray-500">
            validated{" "}
            {new Date(row.last_validated_at + "Z").toLocaleString(undefined, {
              dateStyle: "short",
              timeStyle: "short",
            })}
          </span>
        )}

        <div className="ml-auto flex items-center gap-2">
          {/* Form ⇄ TOML mode toggle. Two-segment pill — clicking
              either segment switches modes. Form is the default;
              TOML is the escape hatch for unsupported edit types
              (hierarchies/relations/metrics in Phase 1) and for
              comment-preserving hand edits. */}
          <div className="flex rounded border border-gray-800 overflow-hidden">
            {(["form", "toml"] as const).map((m) => (
              <button
                key={m}
                onClick={() => setLeftMode(m)}
                className={
                  "text-[11px] px-2.5 py-1 transition-colors capitalize " +
                  (leftMode === m
                    ? "bg-gray-800 text-gray-100"
                    : "bg-gray-900 text-gray-500 hover:text-gray-300")
                }
              >
                {m}
              </button>
            ))}
          </div>

          <button
            onClick={doSave}
            disabled={busy != null || !dirty}
            className="flex items-center gap-1 text-[11px] px-2.5 py-1 rounded bg-gray-900 border border-gray-800 text-gray-300 hover:bg-gray-800 disabled:opacity-40 disabled:cursor-not-allowed"
          >
            {busy === "save" ? <Loader2 size={11} className="animate-spin" /> : <Save size={11} />}
            Save
          </button>
          <button
            onClick={doValidate}
            disabled={busy != null}
            className="flex items-center gap-1 text-[11px] px-2.5 py-1 rounded bg-gray-900 border border-gray-800 text-gray-300 hover:bg-gray-800 disabled:opacity-40"
          >
            {busy === "validate" ? <Loader2 size={11} className="animate-spin" /> : <CheckCircle2 size={11} />}
            Validate
          </button>
          <button
            onClick={doBuild}
            disabled={busy != null || (issues != null && errorCount > 0)}
            className="flex items-center gap-1 text-[11px] px-2.5 py-1 rounded bg-blue-600 border border-blue-500 text-white hover:bg-blue-500 disabled:opacity-40 disabled:cursor-not-allowed"
            title={errorCount > 0 ? "Fix validation errors first" : "Build the v2 snapshot into state.graphs"}
          >
            {busy === "build" ? <Loader2 size={11} className="animate-spin" /> : <Hammer size={11} />}
            Build
          </button>
          <button
            onClick={() => {
              void loadRow();
              void loadStats();
            }}
            disabled={busy != null}
            className="flex items-center gap-1 text-[11px] px-2.5 py-1 rounded bg-gray-900 border border-gray-800 text-gray-300 hover:bg-gray-800 disabled:opacity-40"
            title="Re-fetch the graph row + live snapshot stats from the server"
          >
            <RefreshCw size={11} />
            Reload
          </button>
          {/* Destructive — solid red, labeled "Delete" to match
              DataView / SharedPipeline workspace patterns. confirm()
              in doDelete is the actual guard. */}
          <button
            onClick={doDelete}
            disabled={busy != null}
            title="Delete this graph"
            className="flex items-center gap-1.5 px-3 py-1.5 text-xs font-medium rounded bg-red-700 hover:bg-red-600 text-white disabled:opacity-50 transition-colors shrink-0"
          >
            {busy === "delete" ? (
              <Loader2 size={12} className="animate-spin" />
            ) : (
              <Trash2 size={12} />
            )}
            Delete
          </button>
        </div>
      </div>

      {/* Body — split left (editor) / drag handle / right (tabbed
          visualization). Editor gets the flex 1fr; right pane is
          a fixed pixel width the user can drag-resize.

          `grid-rows-1` (= `repeat(1, minmax(0, 1fr))`) is load-bearing:
          without it the implicit row defaults to `auto` and sizes to
          whichever pane's content is taller (Status sections on the
          right), starving the TOML editor of vertical space.

          `gridTemplateColumns` is inline so the splitter can update
          it without going through Tailwind. The middle column is a
          4px strip with cursor:col-resize. */}
      <div
        className="flex-1 grid grid-rows-1 overflow-hidden"
        style={{ gridTemplateColumns: `1fr 4px ${rightWidth}px` }}
      >
        {/* Left pane — Form or TOML depending on the mode toggle.
            Both edit the same underlying `toml` state. FormView
            owns parsing + serialization; the textarea binds directly.

            `min-h-0 h-full` on the cell wrapper is the load-bearing
            pair: the implicit grid row defaults to `auto` (content-
            based), which collapses to 0 when the form's `h-full`
            chains through. Explicit `h-full` makes the cell fill
            the body row regardless of which mode is rendered. */}
        <div className="flex flex-col overflow-hidden min-h-0 h-full">
          {leftMode === "form" ? (
            <FormView tomlText={toml} onTomlChange={setToml} />
          ) : (
            <>
              <textarea
                value={toml}
                onChange={(e) => setToml(e.target.value)}
                spellCheck={false}
                className="flex-1 min-h-0 w-full px-3 py-2 bg-gray-950 text-gray-200 font-mono text-[12px] leading-[1.4] resize-none focus:outline-none placeholder-gray-700"
                placeholder='id = "..."&#10;display_name = "..."&#10;&#10;[[sources]]&#10;…'
              />
              <div className="px-3 py-1 border-t border-gray-800 text-[10px] text-gray-600 font-mono flex items-center justify-end gap-3 shrink-0">
                <span>{toml.length.toLocaleString()} chars</span>
                <span>·</span>
                <span>{toml.split("\n").length} lines</span>
              </div>
            </>
          )}
        </div>

        {/* Splitter — drag to resize the right pane. Mirrors the
            in-FormView Tree splitter; window listeners on mousedown
            keep the drag alive when the cursor leaves the 4px strip.
            Double-click resets to the default width. */}
        <div
          onMouseDown={startRightResize}
          onDoubleClick={() => setRightWidth(RIGHT_DEFAULT_PX)}
          title="Drag to resize · double-click to reset"
          className="cursor-col-resize bg-transparent hover:bg-blue-500/40 active:bg-blue-500/60 transition-colors border-l border-r border-gray-800"
        />

        {/* Right pane: tabbed (Status / Schema / Explore) */}
        <div className="flex flex-col overflow-hidden bg-gray-950">
          {/* Tab strip */}
          <div className="px-2 pt-2 border-b border-gray-800 flex items-center gap-1">
            {(["status", "schema", "explore"] as const).map((t) => (
              <button
                key={t}
                onClick={() => setRightTab(t)}
                className={
                  "text-[11px] px-3 py-1.5 rounded-t border-b-2 transition-colors capitalize " +
                  (rightTab === t
                    ? "text-gray-100 border-blue-500 bg-gray-900"
                    : "text-gray-400 border-transparent hover:text-gray-200 hover:bg-gray-900/40")
                }
              >
                {t}
              </button>
            ))}
          </div>

          {/* Schema tab: live SVG diagram from the editor's TOML.
              Renders pre-build, no snapshot needed. */}
          {rightTab === "schema" && (
            <div className="flex-1 overflow-auto">
              <SchemaSketch tomlText={toml} />
            </div>
          )}

          {/* Explore tab: live drill against /traverse. Requires
              a built snapshot; the pane surfaces a "not built" hint
              when stats are absent. */}
          {rightTab === "explore" && (
            <div className="flex-1 overflow-auto">
              <ExplorePane graphId={graphId} stats={stats} />
            </div>
          )}

          {/* Status tab: validation issues + live snapshot inventory. */}
          {rightTab === "status" && (
            <div className="flex-1 overflow-auto">
          {error && (
            <div className="m-3 rounded border border-red-900/60 bg-red-950/30 p-2.5 text-[11px] text-red-300">
              <div className="flex items-start gap-1.5">
                <CircleAlert size={12} className="mt-0.5 shrink-0" />
                <pre className="font-mono whitespace-pre-wrap break-words">{error}</pre>
              </div>
            </div>
          )}

          {buildLog && (
            <div className="m-3 rounded border border-blue-900/60 bg-blue-950/30 p-2.5 text-[11px] text-blue-300 flex items-start gap-1.5">
              <Hammer size={12} className="mt-0.5 shrink-0" />
              <span>{buildLog}</span>
            </div>
          )}

          {/* Validation issues */}
          <section className="px-3 py-2 border-b border-gray-800">
            <div className="flex items-baseline gap-2 mb-1.5">
              <h2 className="text-[10px] uppercase tracking-wider font-semibold text-gray-400">
                Validation
              </h2>
              {issues == null ? (
                <span className="text-[11px] text-gray-500">not run yet</span>
              ) : (
                <span className="text-[11px] text-gray-500">
                  {errorCount > 0 ? (
                    <span className="text-red-400">{errorCount} error{errorCount === 1 ? "" : "s"}</span>
                  ) : (
                    <span className="text-green-400">no errors</span>
                  )}
                  {warningCount > 0 && (
                    <>
                      {" · "}
                      <span className="text-amber-400">{warningCount} warning{warningCount === 1 ? "" : "s"}</span>
                    </>
                  )}
                </span>
              )}
            </div>
            {issues != null && issues.length > 0 && (
              <ul className="space-y-1 text-[11px]">
                {issues.map((iss, i) => (
                  <li key={i} className="flex items-start gap-1.5">
                    {iss.severity === "error" ? (
                      <AlertCircle size={11} className="mt-0.5 shrink-0 text-red-400" />
                    ) : (
                      <CircleAlert size={11} className="mt-0.5 shrink-0 text-amber-400" />
                    )}
                    <div className="min-w-0">
                      <div className="font-mono text-gray-400">
                        [{iss.code}]
                        {iss.location && (
                          <span className="text-gray-600"> at {iss.location}</span>
                        )}
                      </div>
                      <div className="text-gray-300">{iss.message}</div>
                    </div>
                  </li>
                ))}
              </ul>
            )}
          </section>

          {/* Live snapshot inventory */}
          <section className="px-3 py-2 border-b border-gray-800">
            <div className="flex items-baseline gap-2 mb-1.5">
              <h2 className="text-[10px] uppercase tracking-wider font-semibold text-gray-400">
                Live snapshot
              </h2>
              {stats == null ? (
                <span className="text-[11px] text-gray-500">not built — click Build</span>
              ) : (
                <span className="text-[11px] text-gray-500">
                  v{stats.graph_version} · {stats.node_count.toLocaleString()} nodes · {stats.string_count.toLocaleString()} strings
                </span>
              )}
            </div>
            {stats != null && (
              <>
                <div className="text-[10px] uppercase tracking-wider text-gray-500 mt-2 mb-1">Kinds</div>
                <table className="w-full text-[11px]">
                  <tbody className="divide-y divide-gray-900/60">
                    {stats.kinds
                      .filter((k) => k.name !== "__root__")
                      .sort((a, b) => b.node_count - a.node_count)
                      .map((k) => (
                        <tr key={k.name}>
                          <td className="py-0.5 font-mono text-gray-300 truncate">
                            {k.name}
                            {k.hierarchy && (
                              <span className="text-gray-600"> · {k.hierarchy}</span>
                            )}
                          </td>
                          <td className="py-0.5 text-right tabular-nums text-gray-400">
                            {k.node_count.toLocaleString()}
                          </td>
                        </tr>
                      ))}
                  </tbody>
                </table>

                <div className="text-[10px] uppercase tracking-wider text-gray-500 mt-3 mb-1">Metrics</div>
                <table className="w-full text-[11px]">
                  <tbody className="divide-y divide-gray-900/60">
                    {stats.metrics.map((m) => (
                      <tr key={`${m.source}.${m.name}`}>
                        <td className="py-0.5 font-mono text-gray-300 truncate">
                          {m.source}.{m.name}
                        </td>
                        <td className="py-0.5 text-right text-gray-500 font-mono">{m.rollup}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </>
            )}
          </section>

          {/* Helpful hint when nothing's built */}
          {stats == null && issues == null && (
            <div className="px-3 py-3 text-[11px] text-gray-500">
              Edit the TOML, click <span className="text-gray-300">Validate</span> to check the spec, then{" "}
              <span className="text-gray-300">Build</span> to materialize the in-memory snapshot.
            </div>
          )}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

/// Click-to-rename graph title. Same pattern as
/// `PipelineTitleEditor` in SharedPipelineWorkspace — renders the
/// display_name as a header; click switches to an input. Enter or
/// blur saves via PUT /api/graphs/:id, Esc cancels. Empty / unchanged
/// values revert silently.
///
/// Fires a `graphs-changed` window event after a successful rename
/// so the sidebar list updates without a manual refresh.
function GraphTitleEditor({
  graphId,
  displayName,
  onChanged,
}: {
  graphId: string;
  displayName: string;
  onChanged: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(displayName);
  const [busy, setBusy] = useState(false);

  // Keep the draft in sync when the row reloads with a fresh name —
  // but never overwrite while the user is mid-edit.
  useEffect(() => {
    if (!editing) setDraft(displayName);
  }, [displayName, editing]);

  const commit = async () => {
    const trimmed = draft.trim();
    setEditing(false);
    if (!trimmed || trimmed === displayName) {
      setDraft(displayName);
      return;
    }
    setBusy(true);
    try {
      const r = await fetch(`/api/graphs/${encodeURIComponent(graphId)}`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ display_name: trimmed }),
      });
      if (!r.ok) throw new Error(await r.text());
      window.dispatchEvent(new CustomEvent("graphs-changed"));
      onChanged();
    } catch (e: any) {
      alert("Rename failed: " + (e?.message || "Unknown error"));
      setDraft(displayName);
    } finally {
      setBusy(false);
    }
  };

  if (editing) {
    return (
      <input
        autoFocus
        type="text"
        value={draft}
        disabled={busy}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit();
          else if (e.key === "Escape") {
            setDraft(displayName);
            setEditing(false);
          }
        }}
        className="text-sm font-medium text-gray-100 bg-gray-900 border border-blue-700 rounded px-1.5 py-0.5 focus:outline-none focus:ring-1 focus:ring-blue-500 min-w-[200px]"
      />
    );
  }
  return (
    <h1
      className="text-sm font-medium text-gray-100 truncate cursor-text hover:bg-gray-900 rounded px-1.5 py-0.5 -mx-1.5 -my-0.5"
      title="Click to rename"
      onClick={() => setEditing(true)}
    >
      {displayName}
    </h1>
  );
}
