// The right-side column inside the workspace panel: lists dashboards for
// the current workspace, lets the user create / open / edit / delete.
//
// Sibling of the `SessionsPanel` column. Same visual language: header
// with a count, an inline "create" row at the top, then one card per
// dashboard with hover-revealed Edit + Delete actions.

import { useEffect, useState } from "react";
import {
  ChevronRight, Clock, LayoutDashboard, Loader2, Pencil, Plus, Trash2,
} from "lucide-react";
import { dashboardsApi } from "./api";
import type { DashboardSummary } from "./types";

export function DashboardsColumn(props: {
  workspaceId: string;
  onOpen: (id: string) => void;
  onEdit: (id: string) => void;
}) {
  const [items, setItems] = useState<DashboardSummary[]>([]);
  const [loading, setLoading] = useState(false);

  const refresh = () => {
    dashboardsApi.list(props.workspaceId)
      .then(setItems)
      .catch(console.error);
  };
  useEffect(refresh, [props.workspaceId]);

  return (
    <div>
      <div className="flex items-baseline gap-3 mb-3">
        <h2 className="text-xs font-semibold text-slate-500 uppercase tracking-wider">Dashboards</h2>
        <span className="text-xs text-slate-400">{items.length} saved</span>
      </div>

      <NewDashboard
        workspaceId={props.workspaceId}
        loading={loading}
        setLoading={setLoading}
        onCreated={(d) => {
          refresh();
          // Open the new dashboard in edit mode immediately — most users
          // create then immediately start designing.
          props.onEdit(d.id);
        }}
      />

      <div className="mt-4 grid gap-2">
        {items.length === 0 ? (
          <div className="text-sm text-slate-400 py-6 text-center border border-dashed border-slate-200 rounded-2xl bg-white/60">
            No dashboards yet — create one above.
          </div>
        ) : (
          items.map((d) => (
            <DashboardRow
              key={d.id}
              dashboard={d}
              onOpen={() => props.onOpen(d.id)}
              onEdit={() => props.onEdit(d.id)}
              onDeleted={refresh}
            />
          ))
        )}
      </div>
    </div>
  );
}

function NewDashboard(props: {
  workspaceId: string;
  loading: boolean;
  setLoading: (b: boolean) => void;
  onCreated: (d: DashboardSummary) => void;
}) {
  const [name, setName] = useState("");
  const submit = async () => {
    const trimmed = name.trim();
    if (!trimmed) return;
    props.setLoading(true);
    try {
      const d = await dashboardsApi.create(props.workspaceId, { name: trimmed });
      setName("");
      props.onCreated(d);
    } catch (e) {
      console.error(e);
    } finally {
      props.setLoading(false);
    }
  };
  return (
    <div className="border border-slate-200 rounded-xl bg-white p-2 flex items-center gap-2 shadow-sm">
      <input
        value={name}
        onChange={(e) => setName(e.target.value)}
        onKeyDown={(e) => { if (e.key === "Enter") void submit(); }}
        placeholder="Dashboard name"
        className="flex-1 px-3 py-2 rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-indigo-200 bg-slate-50/60"
      />
      <button
        onClick={submit}
        disabled={!name.trim() || props.loading}
        className="inline-flex items-center gap-1.5 px-4 py-2 bg-gradient-to-br from-indigo-500 to-blue-600 text-white rounded-lg text-sm font-medium hover:from-indigo-600 hover:to-blue-700 disabled:opacity-50 shadow-sm transition"
      >
        {props.loading ? <Loader2 className="w-4 h-4 animate-spin" /> : <Plus className="w-4 h-4" />}
        New
      </button>
    </div>
  );
}

function DashboardRow(props: {
  dashboard: DashboardSummary;
  onOpen: () => void;
  onEdit: () => void;
  onDeleted: () => void;
}) {
  const d = props.dashboard;
  const [confirming, setConfirming] = useState(false);
  const [deleting, setDeleting] = useState(false);

  const onDelete = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (!confirming) { setConfirming(true); return; }
    setDeleting(true);
    try {
      await dashboardsApi.delete(d.id);
      props.onDeleted();
    } catch (err) {
      console.error(err);
    } finally {
      setDeleting(false);
      setConfirming(false);
    }
  };

  const onClick = () => {
    if (confirming) { setConfirming(false); return; }
    props.onOpen();
  };

  return (
    <div className="group w-full border border-slate-200 rounded-xl bg-white hover:border-slate-300 hover:shadow-sm transition flex items-center gap-3 pl-3 pr-2">
      <button
        onClick={onClick}
        className="flex-1 min-w-0 py-3.5 flex items-center gap-3 cursor-pointer text-left"
      >
        <div className="w-9 h-9 rounded-lg bg-slate-100 text-slate-500 flex items-center justify-center flex-shrink-0">
          <LayoutDashboard className="w-4 h-4" />
        </div>
        <div className="flex-1 min-w-0">
          <div className="font-medium text-slate-900 truncate">{d.name}</div>
          <div className="text-xs text-slate-500 mt-0.5 flex items-center gap-2 flex-wrap">
            {d.description && <span className="truncate">{d.description}</span>}
            <span className="inline-flex items-center gap-1">
              <Clock className="w-3 h-3" /> {timeAgo(d.updated_at)}
            </span>
          </div>
        </div>
      </button>

      {!confirming && (
        <button
          onClick={(e) => { e.stopPropagation(); props.onEdit(); }}
          className="rounded-md px-2 py-1 text-xs text-slate-400 hover:text-indigo-600 hover:bg-indigo-50 opacity-0 group-hover:opacity-100 transition flex-shrink-0"
          title="Edit dashboard layout"
        >
          <Pencil className="w-3.5 h-3.5" />
        </button>
      )}

      <button
        onClick={onDelete}
        disabled={deleting}
        className={[
          "rounded-md px-2 py-1 text-xs transition flex items-center gap-1 flex-shrink-0",
          confirming
            ? "bg-rose-600 text-white hover:bg-rose-700"
            : "text-slate-400 hover:text-rose-600 hover:bg-rose-50 opacity-0 group-hover:opacity-100",
        ].join(" ")}
        title={confirming ? "Click again to confirm delete" : "Delete dashboard"}
      >
        {deleting ? <Loader2 className="w-3.5 h-3.5 animate-spin" /> : <Trash2 className="w-3.5 h-3.5" />}
        {confirming && <span>delete?</span>}
      </button>

      {!confirming && <ChevronRight className="w-4 h-4 text-slate-300 flex-shrink-0" />}
    </div>
  );
}

function timeAgo(ms: number): string {
  const diff = Date.now() - ms;
  if (diff < 60_000) return "just now";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m ago`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)}h ago`;
  return `${Math.floor(diff / 86_400_000)}d ago`;
}
