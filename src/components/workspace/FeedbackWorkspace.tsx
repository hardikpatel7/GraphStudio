import { useEffect, useState } from "react";
import { MessageSquare, RefreshCw, Loader2, ChevronDown, ChevronRight } from "lucide-react";

/// Feedback queue — entries filed by the MCP's `submit_feedback` tool.
/// Each row has a lifecycle status (pending / partial / addressed) that can be
/// cycled inline. Newest first.

type Status = "pending" | "partial" | "addressed";
const STATUS_ORDER: Status[] = ["pending", "partial", "addressed"];

interface FeedbackRow {
  id: string;
  created_at: string;
  category: string;
  summary: string;
  example_question: string | null;
  attempted_path: string | null;
  what_was_painful: string | null;
  workaround: string | null;
  proposed_solution: string | null;
  status: Status;
}

interface ListResponse {
  feedback: FeedbackRow[];
  count: number;
}

const CATEGORY_STYLES: Record<string, string> = {
  missing_endpoint: "bg-blue-900/40 text-blue-200 border-blue-800",
  data_gap:         "bg-rose-900/40 text-rose-200 border-rose-800",
  ergonomics:       "bg-amber-900/40 text-amber-200 border-amber-800",
  perf:             "bg-purple-900/40 text-purple-200 border-purple-800",
  new_graph:        "bg-emerald-900/40 text-emerald-200 border-emerald-800",
  bug:              "bg-red-900/40 text-red-200 border-red-800",
};

const STATUS_STYLES: Record<Status, string> = {
  pending:   "bg-gray-800 text-gray-300 border-gray-700 hover:bg-gray-700",
  partial:   "bg-amber-900/40 text-amber-200 border-amber-800 hover:bg-amber-900/60",
  addressed: "bg-emerald-900/40 text-emerald-200 border-emerald-800 hover:bg-emerald-900/60",
};

function categoryClass(c: string): string {
  return CATEGORY_STYLES[c] ?? "bg-gray-800 text-gray-300 border-gray-700";
}

function nextStatus(s: Status): Status {
  return STATUS_ORDER[(STATUS_ORDER.indexOf(s) + 1) % STATUS_ORDER.length];
}

function parseAttemptedPath(raw: string | null): string[] {
  if (!raw) return [];
  try {
    const v = JSON.parse(raw);
    if (Array.isArray(v)) return v.filter((s) => typeof s === "string");
  } catch {
    // not JSON — treat as a comma-separated fallback
  }
  return raw.split(",").map((s) => s.trim()).filter(Boolean);
}

function FeedbackRowView({
  row,
  onStatusChange,
}: {
  row: FeedbackRow;
  onStatusChange: (id: string, status: Status) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const [updating, setUpdating] = useState(false);
  const path = parseAttemptedPath(row.attempted_path);
  // The expander is always available — even on a thin row it reveals the id
  // and (if present) example_question. Consistent visual rhythm beats the
  // older "sometimes the chevron is missing" mode.
  const hasExtraDetails =
    !!row.what_was_painful || !!row.workaround || !!row.proposed_solution || path.length > 0;

  async function cycleStatus(e: React.MouseEvent) {
    // Don't toggle the expand chevron when the chip is clicked.
    e.stopPropagation();
    if (updating) return;
    const next = nextStatus(row.status);
    setUpdating(true);
    try {
      const res = await fetch(`/api/feedback/${row.id}`, {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ status: next }),
      });
      if (!res.ok) throw new Error(await res.text());
      onStatusChange(row.id, next);
    } catch (err) {
      console.error("[feedback] status update failed", err);
    } finally {
      setUpdating(false);
    }
  }

  return (
    <div className="border border-gray-800 rounded bg-gray-900/40">
      <button
        onClick={() => setExpanded((v) => !v)}
        className="w-full flex items-start gap-3 px-4 py-3 text-left hover:bg-gray-900/60"
      >
        <div className="pt-0.5 text-gray-500">
          {expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        </div>
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2 flex-wrap">
            <span
              className={`text-[10px] uppercase tracking-wide font-medium px-1.5 py-0.5 rounded border ${categoryClass(
                row.category
              )}`}
            >
              {row.category}
            </span>
            <span
              role="button"
              tabIndex={0}
              onClick={cycleStatus}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  cycleStatus(e as unknown as React.MouseEvent);
                }
              }}
              title={`Click to cycle status (currently ${row.status})`}
              aria-label={`Status: ${row.status}. Click to cycle.`}
              className={`text-[10px] uppercase tracking-wide font-medium px-1.5 py-0.5 rounded border cursor-pointer transition-colors ${
                STATUS_STYLES[row.status]
              } ${updating ? "opacity-50" : ""}`}
            >
              {updating ? "…" : row.status}
            </span>
            <span className="text-sm text-gray-100 font-medium">{row.summary}</span>
          </div>
          {row.example_question && (
            <div className="mt-1 text-xs text-gray-400 italic line-clamp-2">
              “{row.example_question}”
            </div>
          )}
        </div>
        <div className="text-[11px] text-gray-500 font-mono whitespace-nowrap pt-0.5">
          {row.created_at?.slice(0, 19).replace("T", " ")}
        </div>
      </button>

      {expanded && (
        <div className="px-4 pb-3 pt-1 border-t border-gray-800/60 space-y-3 text-xs">
          {!hasExtraDetails && (
            <div className="text-gray-500 italic">
              No additional details captured for this entry.
            </div>
          )}
          {path.length > 0 && (
            <div>
              <div className="text-[10px] uppercase tracking-wide text-gray-500 mb-1">
                Attempted path
              </div>
              <div className="flex items-center gap-1 flex-wrap">
                {path.map((tool, i) => (
                  <span key={i} className="flex items-center gap-1">
                    <code className="px-1.5 py-0.5 rounded bg-gray-800 text-gray-300 font-mono">
                      {tool}
                    </code>
                    {i < path.length - 1 && <span className="text-gray-600">→</span>}
                  </span>
                ))}
              </div>
            </div>
          )}
          {row.what_was_painful && (
            <div>
              <div className="text-[10px] uppercase tracking-wide text-gray-500 mb-1">
                What was painful
              </div>
              <div className="text-gray-300 whitespace-pre-wrap">{row.what_was_painful}</div>
            </div>
          )}
          {row.workaround && (
            <div>
              <div className="text-[10px] uppercase tracking-wide text-gray-500 mb-1">
                Workaround
              </div>
              <div className="text-gray-300 whitespace-pre-wrap">{row.workaround}</div>
            </div>
          )}
          {row.proposed_solution && (
            <div>
              <div className="text-[10px] uppercase tracking-wide text-gray-500 mb-1">
                Proposed solution
              </div>
              <div className="text-gray-300 whitespace-pre-wrap">{row.proposed_solution}</div>
            </div>
          )}
          <div className="text-[10px] text-gray-600 font-mono">{row.id}</div>
        </div>
      )}
    </div>
  );
}

function StatusFilterPill({
  status,
  count,
  active,
  onToggle,
}: {
  status: Status;
  count: number;
  active: boolean;
  onToggle: () => void;
}) {
  return (
    <button
      onClick={onToggle}
      aria-pressed={active}
      title={active ? `Click to hide ${status} entries` : `Click to show ${status} entries`}
      className={`text-[10px] uppercase tracking-wide font-medium px-1.5 py-0.5 rounded border transition-colors ${
        active
          ? STATUS_STYLES[status]
          : "bg-transparent text-gray-500 border-gray-800 hover:border-gray-700 hover:text-gray-400"
      }`}
    >
      {count} {status}
    </button>
  );
}

export function FeedbackWorkspace() {
  const [rows, setRows] = useState<FeedbackRow[]>([]);
  const [count, setCount] = useState(0);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  // All three statuses visible by default. Multi-select — toggle off any
  // status to hide its rows. Persisting across reloads would be the next
  // step (localStorage) but isn't worth it until a session is long enough
  // to care.
  const [visible, setVisible] = useState<Set<Status>>(new Set(STATUS_ORDER));

  async function load() {
    setLoading(true);
    setError(null);
    try {
      const res = await fetch("/api/feedback");
      if (!res.ok) {
        const body = await res.json().catch(() => ({ error: res.statusText }));
        throw new Error(body.error ?? res.statusText);
      }
      const data: ListResponse = await res.json();
      setRows(data.feedback ?? []);
      setCount(data.count ?? 0);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    load();
  }, []);

  // Per-status counts over the full set (not filtered). The pills always
  // show the full-set count so toggling one off doesn't make the other
  // pills' counts shift around — the totals are stable while you triage.
  const statusCounts: Record<Status, number> = { pending: 0, partial: 0, addressed: 0 };
  for (const r of rows) statusCounts[r.status] = (statusCounts[r.status] ?? 0) + 1;

  const filteredRows = rows.filter((r) => visible.has(r.status));
  const allVisible = visible.size === STATUS_ORDER.length;

  function toggleStatus(s: Status) {
    setVisible((curr) => {
      const next = new Set(curr);
      if (next.has(s)) next.delete(s);
      else next.add(s);
      return next;
    });
  }

  return (
    <div className="h-full flex flex-col">
      <header className="flex items-center justify-between px-6 py-3 border-b border-gray-800 shrink-0">
        <div className="flex items-center gap-3">
          <div className="flex items-center gap-2">
            <MessageSquare size={16} className="text-gray-400" />
            <h2 className="text-sm font-medium text-gray-100">Feedback</h2>
            <span className="text-xs text-gray-500">
              {allVisible
                ? `${count} ${count === 1 ? "entry" : "entries"}`
                : `${filteredRows.length} of ${count}`}
            </span>
          </div>
          <div className="flex items-center gap-1.5">
            {STATUS_ORDER.map((s) => (
              <StatusFilterPill
                key={s}
                status={s}
                count={statusCounts[s]}
                active={visible.has(s)}
                onToggle={() => toggleStatus(s)}
              />
            ))}
          </div>
        </div>
        <button
          onClick={load}
          disabled={loading}
          className="flex items-center gap-1.5 px-2.5 py-1 text-xs text-gray-300 hover:text-gray-100 hover:bg-gray-800 rounded disabled:opacity-50"
        >
          {loading ? <Loader2 size={12} className="animate-spin" /> : <RefreshCw size={12} />}
          Refresh
        </button>
      </header>

      <div className="flex-1 overflow-y-auto px-6 py-4">
        {error && (
          <div className="text-xs text-red-400 font-mono mb-3">Error: {error}</div>
        )}
        {!loading && rows.length === 0 && !error && (
          <div className="text-xs text-gray-500 italic text-center py-12">
            No feedback yet. The MCP files entries here via <code>submit_feedback</code>.
          </div>
        )}
        {!loading && rows.length > 0 && filteredRows.length === 0 && !error && (
          <div className="text-xs text-gray-500 italic text-center py-12">
            No entries match the current status filter.
          </div>
        )}
        <div className="space-y-2">
          {filteredRows.map((r) => (
            <FeedbackRowView
              key={r.id}
              row={r}
              onStatusChange={(id, status) =>
                setRows((curr) => curr.map((x) => (x.id === id ? { ...x, status } : x)))
              }
            />
          ))}
        </div>
      </div>
    </div>
  );
}
