import { useEffect, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import {
  ArrowLeft,
  BarChart3,
  Blocks,
  Boxes,
  Check,
  CheckCircle2,
  LayoutDashboard,
  ChevronRight,
  ClipboardList,
  Clock,
  Copy,
  Database,
  DollarSign,
  Loader2,
  MessageSquare,
  Package2,
  Plus,
  RefreshCw,
  Send,
  Pencil,
  Sparkles,
  Tag,
  Trash2,
  User as UserIcon,
  Wrench,
  X,
  XCircle,
  Zap,
} from "lucide-react";
import { api, submitPrompt, type ModelEntry, type Prompt, type PromptDetail, type Session, type SseEvent, type Workspace, type WorkspaceStats } from "./api";
import { ChartBlock } from "./charts";
import { DashboardView } from "./dashboards/DashboardView";
import { DashboardEdit } from "./dashboards/DashboardEdit";
import { DashboardsColumn } from "./dashboards/DashboardsColumn";
import { ComponentsTab } from "./components/ComponentsTab";

// Single-file v1 UI. Three view states driven by what's selected:
//   - workspace: no session picked yet → workspace picker + session list
//   - session:   a session is open → message thread + composer
// Detail drawer (tokens/api calls/cost) overlays when a prompt is clicked.

type Tab = "ws-picker" | "session" | "dashboard-view" | "dashboard-edit";

/** Inner tabs inside the workspace panel. Reset to `summary` when the
 *  active workspace changes. */
type WsTab = "summary" | "sessions" | "dashboards" | "components";

type ToolChip = {
  call_id: string;
  tool: string;
  status: "running" | "ok" | "error" | string;
  duration_ms?: number;
  args_preview?: string;
};

type ThreadMessage =
  | {
      role: "user";
      text: string;
      /** ms-epoch when the user submitted the prompt. For replayed history
       *  this is `prompt.started_at`. */
      timestamp?: number;
    }
  | {
      role: "assistant";
      text: string;
      chips: ToolChip[];
      usage?: { tokens_in: number; tokens_out: number };
      promptId?: string;
      /** Model that produced this response. Captured from `turn_started`
       *  during a live run; populated from `prompt.model` on replay.
       *  Surfaced in the assistant footer so the user can attribute
       *  output to a specific model, especially after switching mid-
       *  session. */
      model?: string;
      /** End-to-end model + tool-call latency for this turn (ms). Set when
       *  `turn_finished` arrives. */
      latencyMs?: number;
      /** Derived cost in USD. Filled in via a follow-up `promptDetail` call
       *  ~300ms after `turn_finished` so the meter writer has flushed. */
      costUsd?: number;
      /** ms-epoch when the response completed. For replayed history this
       *  is `prompt.finished_at`. */
      timestamp?: number;
    };

// Visual palette per workspace kind. Each kind picks an icon + a soft
// gradient tint so the picker reads as a row of distinct product lines
// rather than five identical squares.
const KIND_THEME: Record<
  Workspace["kind"],
  { icon: typeof Boxes; gradient: string; ring: string; iconBg: string; iconColor: string }
> = {
  // Wired kinds use a -100 start / -50 end so the tile reads as
  // distinctly tinted rather than nearly-white-with-a-hint.
  inventory: { icon: Boxes,          gradient: "from-sky-100    to-cyan-50",     ring: "ring-sky-300/60",     iconBg: "bg-sky-200",     iconColor: "text-sky-800" },
  item:      { icon: Tag,            gradient: "from-violet-100 to-fuchsia-50",  ring: "ring-violet-300/60",  iconBg: "bg-violet-200",  iconColor: "text-violet-800" },
  // Pending kinds keep the lighter palette; the card stays white in
  // practice because `isWired` gates the gradient, but the theme entries
  // are still defined for future wiring.
  pricing:   { icon: DollarSign,     gradient: "from-emerald-50 to-teal-50",    ring: "ring-emerald-300/60", iconBg: "bg-emerald-100", iconColor: "text-emerald-700" },
  assort:    { icon: Package2,       gradient: "from-amber-50  to-orange-50",   ring: "ring-amber-300/60",   iconBg: "bg-amber-100",   iconColor: "text-amber-700" },
  plan:      { icon: ClipboardList,  gradient: "from-rose-50   to-pink-50",     ring: "ring-rose-300/60",    iconBg: "bg-rose-100",    iconColor: "text-rose-700" },
};

export function App() {
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [models, setModels] = useState<ModelEntry[]>([]);
  const [activeWs, setActiveWs] = useState<Workspace | null>(null);
  const [sessions, setSessions] = useState<Session[]>([]);
  const [activeSession, setActiveSession] = useState<Session | null>(null);
  const [activeDashboardId, setActiveDashboardId] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>("ws-picker");
  const [wsTab, setWsTab] = useState<WsTab>("summary");
  const [bootError, setBootError] = useState<string | null>(null);
  const [bootLoading, setBootLoading] = useState(true);

  useEffect(() => {
    setBootLoading(true);
    Promise.all([api.listWorkspaces(), api.listModels()])
      .then(([ws, ms]) => { setWorkspaces(ws); setModels(ms); setBootError(null); })
      .catch((e: unknown) => setBootError(e instanceof Error ? e.message : String(e)))
      .finally(() => setBootLoading(false));
  }, []);
  useEffect(() => {
    if (!activeWs) return;
    api.listSessions(activeWs.id).then(setSessions).catch(console.error);
    // Default to Summary whenever the user enters a workspace. Pending
    // workspaces (no tools wired) skip Summary since it has nothing to
    // show; jump them straight to Sessions instead.
    const isWired = (activeWs.tool_count ?? 0) > 0;
    setWsTab(isWired ? "summary" : "sessions");
  }, [activeWs?.id]);

  const refreshSessions = () => {
    if (activeWs) api.listSessions(activeWs.id).then(setSessions).catch(console.error);
  };

  return (
    <div className="min-h-screen flex flex-col">
      <Header
        activeWs={activeWs}
        activeSession={activeSession}
        models={models}
        onChangeSessionModel={async (next) => {
          if (!activeSession || next === activeSession.model) return;
          const prev = activeSession.model;
          setActiveSession({ ...activeSession, model: next });
          try {
            const updated = await api.updateSession(activeSession.id, { model: next });
            setActiveSession(updated);
          } catch (e) {
            setActiveSession({ ...activeSession, model: prev });
            alert(`Couldn't change model: ${e instanceof Error ? e.message : String(e)}`);
          }
        }}
        onBack={() => {
          // Dashboard edit → view; view → workspace; session → workspace;
          // workspace → picker.
          if (tab === "dashboard-edit") {
            setTab("dashboard-view");
          } else if (tab === "dashboard-view") {
            setActiveDashboardId(null);
            setTab("ws-picker");
          } else if (tab === "session") {
            setActiveSession(null);
            setTab("ws-picker");
          } else if (activeWs) {
            setActiveWs(null);
          }
        }}
      />
      <div className="flex-1 flex">
        {bootLoading && (
          <div className="flex-1 flex items-center justify-center text-sm text-slate-400 gap-2">
            <Loader2 className="w-4 h-4 animate-spin" /> Loading…
          </div>
        )}
        {!bootLoading && bootError && (
          <div className="flex-1 p-6 max-w-3xl mx-auto w-full">
            <div className="border border-rose-300 bg-rose-50 rounded-xl p-4 text-sm text-rose-800 shadow-sm">
              <div className="font-medium mb-1 flex items-center gap-1.5">
                <XCircle className="w-4 h-4" /> Couldn't reach the agent backend
              </div>
              <div className="font-mono text-xs whitespace-pre-wrap">{bootError}</div>
              <div className="mt-2 text-xs text-rose-700">
                Check that the Rust server is running with the new <code>agent/</code> module
                (restart <code>cargo run</code>), and that Vite's <code>/api</code> proxy
                target matches the server's port (see <code>vite.config.ts</code>).
              </div>
            </div>
          </div>
        )}
        {!bootLoading && !bootError && tab === "ws-picker" && (
          <WorkspacePicker
            workspaces={workspaces}
            activeWs={activeWs}
            sessions={sessions}
            models={models}
            onPickWorkspace={(w) => setActiveWs(w)}
            onPickSession={(s) => {
              setActiveSession(s);
              setTab("session");
            }}
            onCreated={(s) => {
              refreshSessions();
              setActiveSession(s);
              setTab("session");
            }}
            onSessionDeleted={() => refreshSessions()}
            onPickDashboard={(id, edit) => {
              setActiveDashboardId(id);
              setTab(edit ? "dashboard-edit" : "dashboard-view");
            }}
            wsTab={wsTab}
            setWsTab={setWsTab}
          />
        )}
        {!bootLoading && !bootError && tab === "session" && activeSession && (
          <SessionView
            session={activeSession}
            onBack={() => setTab("ws-picker")}
          />
        )}
        {!bootLoading && !bootError && tab === "dashboard-view" && activeDashboardId && (
          <DashboardView
            dashboardId={activeDashboardId}
            models={models}
            onBack={() => { setActiveDashboardId(null); setTab("ws-picker"); }}
            onEdit={() => setTab("dashboard-edit")}
          />
        )}
        {!bootLoading && !bootError && tab === "dashboard-edit" && activeDashboardId && (
          <DashboardEdit
            dashboardId={activeDashboardId}
            onDone={() => setTab("dashboard-view")}
          />
        )}
      </div>
    </div>
  );
}

function Header(props: {
  activeWs: Workspace | null;
  activeSession: Session | null;
  models: ModelEntry[];
  /** Called when the user picks a different model from the badge
   *  dropdown. The handler does the server PATCH + local state
   *  refresh; the badge is just the trigger. */
  onChangeSessionModel: (next: string) => void;
  onBack: () => void;
}) {
  return (
    <header className="border-b border-slate-200/70 bg-white/70 backdrop-blur supports-[backdrop-filter]:bg-white/60 px-6 py-3 flex items-center gap-3 sticky top-0 z-20">
      <div className="flex items-center gap-2">
        <div className="w-8 h-8 rounded-lg bg-gradient-to-br from-indigo-500 to-blue-500 flex items-center justify-center text-white shadow-sm">
          <Sparkles className="w-4 h-4" />
        </div>
        <h1 className="text-base font-semibold tracking-tight text-slate-900">GraphStudio Agent</h1>
      </div>
      {(props.activeWs || props.activeSession) && (
        <button
          onClick={props.onBack}
          className="ml-2 inline-flex items-center gap-1 text-xs text-slate-500 hover:text-slate-800 transition"
          title="Back"
        >
          <ArrowLeft className="w-3.5 h-3.5" /> back
        </button>
      )}
      <div className="flex items-center gap-1.5 ml-3 text-sm">
        {props.activeWs ? (
          <span className="text-slate-700 font-medium">{props.activeWs.name}</span>
        ) : (
          <span className="text-slate-400">Choose a workspace</span>
        )}
        {props.activeSession && (
          <>
            <ChevronRight className="w-3.5 h-3.5 text-slate-300" />
            <span className="text-slate-700">{props.activeSession.title}</span>
            <SessionModelBadge
              value={props.activeSession.model}
              models={props.models}
              onChange={props.onChangeSessionModel}
            />
          </>
        )}
      </div>
    </header>
  );
}

/** Inline model dropdown styled to look like the static badge it
 *  replaces. Wraps the native <select> so the user sees the current
 *  model at a glance and can swap it in one click. */
function SessionModelBadge(props: {
  value: string;
  models: ModelEntry[];
  onChange: (next: string) => void;
}) {
  const inList = props.models.some((m) => m.model === props.value);
  return (
    <label
      className="ml-2 inline-flex items-center px-1.5 py-0.5 rounded-md text-[11px] font-medium bg-slate-100 text-slate-600 ring-1 ring-inset ring-slate-200 hover:bg-slate-200 cursor-pointer transition"
      title="Model used for new turns in this session"
    >
      <select
        value={props.value}
        onChange={(e) => props.onChange(e.target.value)}
        className="bg-transparent outline-none cursor-pointer"
      >
        {!inList && (
          <option value={props.value} disabled>
            {props.value} (disabled)
          </option>
        )}
        {props.models.map((m) => (
          <option key={m.model} value={m.model}>
            {m.display_name}
          </option>
        ))}
      </select>
    </label>
  );
}

function WorkspacePicker(props: {
  workspaces: Workspace[];
  activeWs: Workspace | null;
  sessions: Session[];
  models: ModelEntry[];
  onPickWorkspace: (w: Workspace) => void;
  onPickSession: (s: Session) => void;
  onCreated: (s: Session) => void;
  onSessionDeleted: () => void;
  onPickDashboard: (id: string, edit?: boolean) => void;
  wsTab: WsTab;
  setWsTab: (t: WsTab) => void;
}) {
  return (
    <div className="flex-1 p-6 max-w-6xl mx-auto w-full">
      <div className="mb-6">
        <h2 className="text-xs font-semibold text-slate-500 uppercase tracking-wider mb-1">Workspaces</h2>
        <p className="text-sm text-slate-500">Pick a product line to start a conversation.</p>
      </div>
      <div className="grid grid-cols-5 gap-3 mb-10">
        {props.workspaces.map((w) => {
          const isWired = (w.tool_count ?? 0) > 0;
          const isActive = props.activeWs?.id === w.id;
          const theme = KIND_THEME[w.kind];
          const Icon = theme.icon;
          return (
            <button
              key={w.id}
              onClick={() => props.onPickWorkspace(w)}
              className={[
                "group relative border rounded-2xl p-4 text-left transition-all duration-200",
                "shadow-sm hover:-translate-y-0.5 hover:shadow-md",
                isWired ? `bg-gradient-to-br ${theme.gradient}` : "bg-white",
                isActive
                  ? `border-transparent ring-2 ${theme.ring}`
                  : "border-slate-200/80 hover:border-slate-300",
              ].join(" ")}
            >
              <div className="flex items-start justify-between">
                <div className={`w-10 h-10 rounded-xl ${theme.iconBg} ${theme.iconColor} flex items-center justify-center ring-1 ring-inset ring-white/60 shadow-sm`}>
                  <Icon className="w-5 h-5" />
                </div>
                {isWired ? (
                  <span className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded-md text-[10px] font-medium bg-emerald-100 text-emerald-700 ring-1 ring-inset ring-emerald-200">
                    <CheckCircle2 className="w-3 h-3" /> wired
                  </span>
                ) : (
                  <span className="inline-flex items-center px-1.5 py-0.5 rounded-md text-[10px] font-medium bg-slate-100 text-slate-500 ring-1 ring-inset ring-slate-200">
                    pending
                  </span>
                )}
              </div>
              <div className="mt-3 font-semibold text-slate-900">{w.name}</div>
              <div className="mt-1 flex items-center gap-2 text-[11px] text-slate-500">
                <span className="inline-flex items-center gap-1">
                  <Wrench className="w-3 h-3" /> {w.tool_count ?? 0} tools
                </span>
                <span className="text-slate-300">·</span>
                <span className="inline-flex items-center gap-1">
                  <MessageSquare className="w-3 h-3" /> {w.session_count}
                </span>
              </div>
            </button>
          );
        })}
      </div>

      {props.activeWs && (
        <>
          <WsTabs
            kind={props.activeWs.kind}
            wired={(props.activeWs.tool_count ?? 0) > 0}
            active={props.wsTab}
            onChange={props.setWsTab}
          />
          <div className="mt-5">
            {props.wsTab === "summary" && (props.activeWs.tool_count ?? 0) > 0 && (
              <StatsDashboard workspaceId={props.activeWs.id} />
            )}
            {props.wsTab === "sessions" && (
              <SessionsPanel
                workspace={props.activeWs}
                sessions={props.sessions}
                models={props.models}
                onPickSession={props.onPickSession}
                onCreated={props.onCreated}
                onDeleted={props.onSessionDeleted}
              />
            )}
            {props.wsTab === "dashboards" && (props.activeWs.tool_count ?? 0) > 0 && (
              <DashboardsColumn
                workspaceId={props.activeWs.id}
                onOpen={(id) => props.onPickDashboard(id, false)}
                onEdit={(id) => props.onPickDashboard(id, true)}
              />
            )}
            {props.wsTab === "components" && (props.activeWs.tool_count ?? 0) > 0 && (
              <ComponentsTab workspaceId={props.activeWs.id} />
            )}
          </div>
        </>
      )}
    </div>
  );
}

/**
 * Three-tab strip inside the workspace panel. Summary + Dashboards are
 * disabled on pending workspaces (no tools wired) since they have
 * nothing to show; Sessions is always available so the user can still
 * see their conversation history even on un-wired kinds.
 */
function WsTabs(props: {
  kind: Workspace["kind"];
  wired: boolean;
  active: WsTab;
  onChange: (t: WsTab) => void;
}) {
  const items: Array<{ id: WsTab; label: string; icon: typeof BarChart3; disabled?: boolean }> = [
    { id: "summary",    label: "Summary",    icon: BarChart3,       disabled: !props.wired },
    { id: "sessions",   label: "Sessions",   icon: MessageSquare },
    { id: "dashboards", label: "Dashboards", icon: LayoutDashboard, disabled: !props.wired },
    { id: "components", label: "Components", icon: Blocks,          disabled: !props.wired },
  ];
  return (
    <div className="border-b border-slate-200 flex items-center gap-1">
      {items.map((it) => {
        const Icon = it.icon;
        const active = props.active === it.id;
        return (
          <button
            key={it.id}
            onClick={() => !it.disabled && props.onChange(it.id)}
            disabled={it.disabled}
            className={[
              "relative inline-flex items-center gap-1.5 px-3 py-2 text-sm font-medium transition",
              it.disabled
                ? "text-slate-300 cursor-not-allowed"
                : active
                  ? "text-indigo-700"
                  : "text-slate-500 hover:text-slate-800",
            ].join(" ")}
            title={it.disabled ? `${it.label} unavailable — workspace not wired` : undefined}
          >
            <Icon className="w-3.5 h-3.5" />
            {it.label}
            {active && (
              <span className="absolute left-2 right-2 -bottom-px h-0.5 bg-gradient-to-r from-indigo-500 to-blue-500 rounded" />
            )}
          </button>
        );
      })}
      <span className="ml-auto text-[11px] text-slate-400 mr-1">{props.kind}</span>
    </div>
  );
}

function SessionsPanel(props: {
  workspace: Workspace;
  sessions: Session[];
  models: ModelEntry[];
  onPickSession: (s: Session) => void;
  onCreated: (s: Session) => void;
  onDeleted: () => void;
}) {
  const isWired = (props.workspace.tool_count ?? 0) > 0;
  if (!isWired) {
    return (
      <div className="border border-dashed border-slate-300 rounded-2xl p-10 text-center bg-white/60 backdrop-blur-sm">
        <div className="w-12 h-12 rounded-full bg-slate-100 mx-auto flex items-center justify-center mb-3">
          <Wrench className="w-5 h-5 text-slate-400" />
        </div>
        <div className="text-slate-700 font-medium">Backend not yet configured</div>
        <div className="text-slate-400 text-xs mt-1 max-w-md mx-auto">
          Add rows to <code className="bg-slate-100 px-1 py-0.5 rounded">workspace_kind_tools</code> for
          kind <span className="font-mono text-slate-600">{props.workspace.kind}</span> to wire it.
        </div>
      </div>
    );
  }
  // Split the list into chat sessions (what the user opens manually)
  // and dashboard-backed sessions (the synthetic ones each dashboard
  // owns, plus component-preview holders). The split is server-
  // derived via `session.kind`. Dashboard sessions render collapsed
  // by default — they're useful for audit / cost tracking but
  // shouldn't take visual room in the main Sessions list.
  const chatSessions = props.sessions.filter((s) => (s.kind ?? "chat") === "chat");
  const dashboardSessions = props.sessions.filter((s) => s.kind === "dashboard");
  return (
    <div>
      <div className="flex items-baseline gap-3 mb-3">
        <h2 className="text-xs font-semibold text-slate-500 uppercase tracking-wider">Sessions</h2>
        <span className="text-xs text-slate-400">{chatSessions.length} active</span>
      </div>
      <NewSession workspace={props.workspace} models={props.models} onCreated={props.onCreated} />
      <div className="mt-4 grid gap-2">
        {chatSessions.length === 0 ? (
          <div className="text-sm text-slate-400 py-6 text-center border border-dashed border-slate-200 rounded-2xl bg-white/60">
            No sessions yet — start one above.
          </div>
        ) : (
          chatSessions.map((s) => (
            <SessionRow
              key={s.id}
              session={s}
              onPick={() => props.onPickSession(s)}
              onDeleted={props.onDeleted}
              onRenamed={props.onDeleted /* same refresh path */}
            />
          ))
        )}
      </div>

      {dashboardSessions.length > 0 && (
        <DashboardSessionsGroup
          sessions={dashboardSessions}
          onPickSession={props.onPickSession}
          onDeleted={props.onDeleted}
        />
      )}
    </div>
  );
}

/** Collapsed-by-default group that lists the synthetic sessions
 *  backing each dashboard (and the workspace's component-preview
 *  session). Useful for cost / audit / debug, but kept out of the
 *  primary chat-sessions area where it was noise. */
function DashboardSessionsGroup(props: {
  sessions: Session[];
  onPickSession: (s: Session) => void;
  onDeleted: () => void;
}) {
  const [open, setOpen] = useState(false);
  return (
    <div className="mt-6 border-t border-slate-200 pt-4">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="flex items-baseline gap-2 text-xs font-semibold text-slate-500 uppercase tracking-wider hover:text-slate-700 transition"
      >
        <ChevronRight className={`w-3 h-3 transition-transform ${open ? "rotate-90" : ""}`} />
        Dashboard sessions
        <span className="text-slate-400 normal-case tracking-normal">{props.sessions.length}</span>
      </button>
      {open && (
        <div className="mt-3 grid gap-2">
          {props.sessions.map((s) => (
            <SessionRow
              key={s.id}
              session={s}
              onPick={() => props.onPickSession(s)}
              onDeleted={props.onDeleted}
              onRenamed={props.onDeleted}
            />
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * One row in the sessions list. Three interactions:
 * - Body click → open the session
 * - Pencil icon → inline rename (input replaces the title; Enter saves, Esc cancels)
 * - Trash icon → two-step delete (first click confirms, second click commits)
 *
 * Inline rename + delete buttons fade in on row hover so the resting view
 * stays clean.
 */
function SessionRow(props: {
  session: Session;
  onPick: () => void;
  onDeleted: () => void;
  onRenamed: () => void;
}) {
  const s = props.session;
  const [confirming, setConfirming] = useState(false);
  const [deleting,   setDeleting]   = useState(false);
  const [editing,    setEditing]    = useState(false);
  const [draftTitle, setDraftTitle] = useState(s.title);
  const [saving,     setSaving]     = useState(false);

  const onDelete = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (!confirming) { setConfirming(true); return; }
    setDeleting(true);
    try {
      await api.deleteSession(s.id);
      props.onDeleted();
    } catch (err) {
      console.error(err);
    } finally {
      setDeleting(false);
      setConfirming(false);
    }
  };

  const startEdit = (e: React.MouseEvent) => {
    e.stopPropagation();
    setEditing(true);
    setDraftTitle(s.title);
  };

  const saveEdit = async () => {
    const next = draftTitle.trim();
    if (!next || next === s.title) {
      setEditing(false);
      return;
    }
    setSaving(true);
    try {
      await api.updateSession(s.id, { title: next });
      props.onRenamed();
    } catch (err) {
      console.error(err);
    } finally {
      setSaving(false);
      setEditing(false);
    }
  };

  const cancelEdit = () => {
    setEditing(false);
    setDraftTitle(s.title);
  };

  // Cancel pending states when the body is clicked.
  const onPick = () => {
    if (confirming) { setConfirming(false); return; }
    if (editing)    { cancelEdit(); return; }
    props.onPick();
  };

  return (
    <div className="group w-full text-left border border-slate-200 rounded-xl bg-white hover:border-slate-300 hover:shadow-sm transition flex items-center gap-3 pl-3 pr-2">
      <button
        onClick={onPick}
        className="flex-1 min-w-0 py-3.5 flex items-center gap-3 cursor-pointer text-left"
      >
        <div className="w-9 h-9 rounded-lg bg-slate-100 text-slate-500 flex items-center justify-center flex-shrink-0">
          <MessageSquare className="w-4 h-4" />
        </div>
        <div className="flex-1 min-w-0">
          {editing ? (
            <input
              autoFocus
              value={draftTitle}
              onChange={(e) => setDraftTitle(e.target.value)}
              onClick={(e) => e.stopPropagation()}
              onKeyDown={(e) => {
                e.stopPropagation();
                if (e.key === "Enter")  { e.preventDefault(); void saveEdit(); }
                if (e.key === "Escape") { e.preventDefault(); cancelEdit(); }
              }}
              onBlur={() => void saveEdit()}
              disabled={saving}
              className="w-full font-medium text-slate-900 bg-white border border-indigo-300 rounded-md px-2 py-0.5 focus:outline-none focus:ring-2 focus:ring-indigo-200"
            />
          ) : (
            <div className="font-medium text-slate-900 truncate">{s.title}</div>
          )}
          <div className="text-xs text-slate-500 mt-0.5 flex items-center gap-2 flex-wrap">
            <span className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded bg-slate-100 text-slate-600 font-mono text-[10px]">
              {s.model}
            </span>
            <span>{s.prompt_count ?? 0} prompts</span>
            <span className="text-slate-300">·</span>
            <span className="inline-flex items-center gap-1">
              <Clock className="w-3 h-3" /> {timeAgo(s.last_active_at)}
            </span>
          </div>
        </div>
      </button>

      {/* Rename — hidden while editing (the input itself is the affordance). */}
      {!editing && !confirming && (
        <button
          onClick={startEdit}
          className="rounded-md px-2 py-1 text-xs text-slate-400 hover:text-indigo-600 hover:bg-indigo-50 opacity-0 group-hover:opacity-100 transition flex-shrink-0"
          title="Rename session"
        >
          <Pencil className="w-3.5 h-3.5" />
        </button>
      )}

      {/* Delete — hidden during edit. */}
      {!editing && (
        <button
          onClick={onDelete}
          disabled={deleting}
          className={[
            "rounded-md px-2 py-1 text-xs transition flex items-center gap-1 flex-shrink-0",
            confirming
              ? "bg-rose-600 text-white hover:bg-rose-700"
              : "text-slate-400 hover:text-rose-600 hover:bg-rose-50 opacity-0 group-hover:opacity-100",
          ].join(" ")}
          title={confirming ? "Click again to confirm delete" : "Delete session"}
        >
          {deleting ? <Loader2 className="w-3.5 h-3.5 animate-spin" /> : <Trash2 className="w-3.5 h-3.5" />}
          {confirming && <span>delete?</span>}
        </button>
      )}

      {!confirming && !editing && (
        <ChevronRight className="w-4 h-4 text-slate-300 flex-shrink-0" />
      )}
    </div>
  );
}

/**
 * Workspace summary dashboard. Pulls `GET /api/agent/workspaces/{id}/stats`
 * on mount and renders a row of KPI tiles + a top-tools chart. Falls back
 * to a tight skeleton while loading; hides itself on fetch failure so a
 * blip in stats doesn't push the sessions panel down.
 */
function StatsDashboard({ workspaceId }: { workspaceId: string }) {
  const [stats, setStats] = useState<WorkspaceStats | null>(null);
  const [err, setErr] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setStats(null); setErr(false);
    api.workspaceStats(workspaceId)
      .then((s) => { if (!cancelled) setStats(s); })
      .catch((e) => { if (!cancelled) { console.error(e); setErr(true); } });
    return () => { cancelled = true; };
  }, [workspaceId]);

  if (err) return null;
  if (!stats) {
    return (
      <div className="mb-6">
        <h2 className="text-xs font-semibold text-slate-500 uppercase tracking-wider mb-3">Summary</h2>
        <div className="grid grid-cols-2 md:grid-cols-5 gap-2">
          {Array.from({ length: 5 }).map((_, i) => (
            <div key={i} className="h-[68px] rounded-xl border border-slate-200 bg-slate-50/60 animate-pulse" />
          ))}
        </div>
      </div>
    );
  }

  const successRate = stats.prompts.total > 0
    ? (stats.prompts.done / stats.prompts.total) * 100
    : 0;
  const cacheRate = stats.api_calls.total > 0
    ? (stats.api_calls.cache_hits / stats.api_calls.total) * 100
    : 0;

  return (
    <div className="mb-6">
      <div className="flex items-baseline gap-3 mb-3">
        <h2 className="text-xs font-semibold text-slate-500 uppercase tracking-wider">Summary</h2>
        <span className="text-xs text-slate-400">{stats.sessions_total} sessions · {stats.prompts.total} prompts</span>
      </div>
      <div className="grid grid-cols-2 md:grid-cols-5 gap-2">
        <StatTile
          icon={MessageSquare}
          label="Prompts"
          value={stats.prompts.total.toLocaleString()}
          sub={`${stats.prompts.done} ok · ${stats.prompts.errored} err`}
          tint="from-slate-50 to-white"
        />
        <StatTile
          icon={CheckCircle2}
          label="Success rate"
          value={`${successRate.toFixed(0)}%`}
          sub={`${stats.prompts.errored} failed`}
          tint="from-emerald-50 to-green-50"
        />
        <StatTile
          icon={DollarSign}
          label="Total cost"
          value={fmtCost(stats.cost_usd_total)}
          sub={`avg ${fmtCost(stats.cost_usd_avg)}/prompt`}
          tint="from-indigo-50 to-blue-50"
        />
        <StatTile
          icon={Zap}
          label="Tool calls"
          value={stats.api_calls.total.toLocaleString()}
          sub={
            stats.api_calls.cache_hits > 0
              ? `${cacheRate.toFixed(0)}% cache · ${stats.api_calls.errors} err`
              : `${stats.api_calls.errors} err`
          }
          tint="from-amber-50 to-orange-50"
        />
        <StatTile
          icon={Clock}
          label="Avg latency"
          value={fmtDuration(stats.tokens.avg_latency_ms)}
          sub={`${(stats.tokens.tokens_in_total + stats.tokens.tokens_out_total).toLocaleString()} tokens`}
          tint="from-violet-50 to-fuchsia-50"
        />
      </div>

      {stats.top_tools.length > 0 && (
        <div className="mt-3 border border-slate-200 rounded-xl bg-white p-3">
          <div className="text-[11px] uppercase tracking-wider text-slate-500 font-medium mb-2">
            Top tools by call count
          </div>
          <div className="space-y-1.5">
            {stats.top_tools.map((t) => {
              const max = stats.top_tools[0]?.count || 1;
              const pct = (t.count / max) * 100;
              return (
                <div key={t.tool} className="flex items-center gap-2 text-xs">
                  <div className="font-mono text-slate-700 w-44 truncate" title={t.tool}>{t.tool}</div>
                  <div className="flex-1 h-4 bg-slate-100 rounded-full relative overflow-hidden">
                    <div
                      className="h-full rounded-full bg-indigo-300/70"
                      style={{ width: `${pct}%` }}
                    />
                  </div>
                  <div className="tabular-nums text-slate-600 w-12 text-right font-medium">{t.count}</div>
                </div>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}

function StatTile({
  icon: Icon, label, value, sub, tint,
}: {
  icon: typeof MessageSquare;
  label: string;
  value: string;
  sub?: string;
  tint: string;
}) {
  return (
    <div className={`rounded-xl border border-slate-200 bg-gradient-to-br ${tint} p-3`}>
      <div className="flex items-start justify-between gap-2">
        <div className="text-[10px] uppercase tracking-wider text-slate-500 font-medium">{label}</div>
        <Icon className="w-3.5 h-3.5 text-slate-400 flex-shrink-0" />
      </div>
      <div className="mt-0.5 text-xl font-semibold text-slate-900 tabular-nums leading-tight">{value}</div>
      {sub && <div className="text-[11px] text-slate-500 mt-0.5">{sub}</div>}
    </div>
  );
}

function NewSession(props: {
  workspace: Workspace;
  models: ModelEntry[];
  onCreated: (s: Session) => void;
}) {
  const [title, setTitle] = useState("");
  const [model, setModel] = useState(props.models[0]?.model ?? "");
  useEffect(() => {
    if (!model && props.models[0]) setModel(props.models[0].model);
  }, [props.models]);

  const submit = async () => {
    if (!model) return;
    const s = await api.createSession(props.workspace.id, {
      model,
      title: title.trim() || undefined,
    });
    setTitle("");
    props.onCreated(s);
  };

  return (
    <div className="border border-slate-200 rounded-xl bg-white p-2 flex items-center gap-2 shadow-sm">
      <input
        value={title}
        onChange={(e) => setTitle(e.target.value)}
        placeholder="Session title (optional)"
        className="flex-1 px-3 py-2 rounded-lg text-sm focus:outline-none focus:ring-2 focus:ring-indigo-200 bg-slate-50/60"
        onKeyDown={(e) => { if (e.key === "Enter") submit(); }}
      />
      <select
        value={model}
        onChange={(e) => setModel(e.target.value)}
        className="px-3 py-2 rounded-lg text-sm bg-slate-50/60 border border-transparent focus:outline-none focus:ring-2 focus:ring-indigo-200"
      >
        {props.models.map((m) => (
          <option key={m.model} value={m.model}>
            {m.display_name}
          </option>
        ))}
      </select>
      <button
        onClick={submit}
        disabled={!model}
        className="inline-flex items-center gap-1.5 px-4 py-2 bg-gradient-to-br from-indigo-500 to-blue-600 text-white rounded-lg text-sm font-medium hover:from-indigo-600 hover:to-blue-700 disabled:opacity-50 shadow-sm transition"
      >
        <Plus className="w-4 h-4" /> New session
      </button>
    </div>
  );
}

function SessionView(props: { session: Session; onBack: () => void }) {
  const [messages, setMessages] = useState<ThreadMessage[]>([]);
  const [draft, setDraft] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [openDetail, setOpenDetail] = useState<string | null>(null);
  const bottomRef = useRef<HTMLDivElement | null>(null);

  // Replay past prompts when opening an existing session. Each row becomes
  // a user+assistant pair; tool-call chips aren't replayed (not in the
  // prompt row — the detail drawer still has the full trace).
  useEffect(() => {
    let cancelled = false;
    api.listPrompts(props.session.id)
      .then((rows: Prompt[]) => {
        if (cancelled) return;
        const replayed: ThreadMessage[] = [];
        for (const p of rows) {
          replayed.push({ role: "user", text: p.user_text, timestamp: p.started_at });
          replayed.push({
            role: "assistant",
            text: p.response_text
              ?? (p.status === "error"
                ? (p.error ? `⚠ ${p.error}` : "⚠ this prompt errored")
                : "…"),
            chips: [],
            promptId: p.id,
            model: p.model,
            usage: p.tokens_in != null && p.tokens_out != null
              ? { tokens_in: p.tokens_in, tokens_out: p.tokens_out }
              : undefined,
            latencyMs: p.latency_ms ?? undefined,
            costUsd:   p.cost_usd   ?? undefined,
            timestamp: p.finished_at ?? undefined,
          });
        }
        setMessages(replayed);
      })
      .catch(console.error);
    return () => { cancelled = true; };
  }, [props.session.id]);

  useEffect(() => { bottomRef.current?.scrollIntoView({ behavior: "smooth" }); }, [messages]);

  const runText = async (raw: string) => {
    const text = raw.trim();
    if (!text || submitting) return;
    setSubmitting(true);
    const now = Date.now();
    let assistantIndex = -1;
    setMessages((m) => {
      const next = [
        ...m,
        { role: "user" as const, text, timestamp: now },
        { role: "assistant" as const, text: "", chips: [] as ToolChip[] },
      ];
      assistantIndex = next.length - 1;
      return next;
    });

    const updateAssistant = (mut: (a: ThreadMessage & { role: "assistant" }) => void) => {
      setMessages((prev) => {
        const next = prev.slice();
        const a = next[assistantIndex];
        if (a && a.role === "assistant") {
          const updated = { ...a, chips: [...a.chips] };
          mut(updated);
          next[assistantIndex] = updated;
        }
        return next;
      });
    };

    await submitPrompt(props.session.id, text, (ev: SseEvent) => {
      switch (ev.type) {
        case "turn_started":
          updateAssistant((a) => { a.promptId = ev.prompt_id; a.model = ev.model; });
          break;
        case "tool_call_started":
          updateAssistant((a) => {
            a.chips.push({ call_id: ev.call_id, tool: ev.tool, status: "running", args_preview: ev.args_preview });
          });
          break;
        case "tool_call_finished":
          updateAssistant((a) => {
            const c = a.chips.find((x) => x.call_id === ev.call_id);
            if (c) { c.status = ev.status; c.duration_ms = ev.duration_ms; }
          });
          break;
        case "text_delta":
          updateAssistant((a) => { a.text += ev.text; });
          break;
        case "usage":
          updateAssistant((a) => { a.usage = { tokens_in: ev.tokens_in, tokens_out: ev.tokens_out }; });
          break;
        case "turn_finished":
          updateAssistant((a) => {
            if (!a.text) a.text = ev.final_text;
            a.latencyMs = ev.latency_ms;
            a.timestamp = Date.now();
          });
          // Cost is derived on read from `pricing_config`; the meter writer
          // flushes every ~100ms, so wait a beat before fetching so we see
          // a complete view (LLM tokens + all api_call rows priced in).
          if (ev.prompt_id) {
            const pid = ev.prompt_id;
            setTimeout(() => {
              api.promptDetail(pid)
                .then((d) => {
                  if (d.cost_usd == null) return;
                  updateAssistant((a) => { a.costUsd = d.cost_usd!; });
                })
                .catch(console.error);
            }, 300);
          }
          break;
        case "error":
          updateAssistant((a) => {
            // The server tags MaxTurnsError messages with `max_turns_reached:`
            // so we can render a friendlier explanation than the raw Rig
            // message. Everything else passes through verbatim with a ⚠.
            const isMaxTurns = ev.message.startsWith("max_turns_reached:");
            const human = isMaxTurns
              ? "The agent didn't converge within the turn budget. Try a more specific question, or break it into smaller parts."
              : ev.message;
            const tag = isMaxTurns ? "⏱ Max turns reached" : "⚠ Error";
            a.text = (a.text ? a.text + "\n\n" : "") + `${tag}\n\n${human}`;
          });
          break;
      }
    });
    setSubmitting(false);
  };

  const send = async () => {
    const text = draft.trim();
    if (!text || submitting) return;
    setDraft("");
    await runText(text);
  };

  return (
    <div className="flex-1 flex flex-col max-w-4xl mx-auto w-full p-6 gap-4">
      <div className="flex-1 overflow-y-auto space-y-4 pr-1">
        {messages.length === 0 && (
          <div className="text-center py-16">
            <div className="w-12 h-12 rounded-2xl bg-gradient-to-br from-indigo-100 to-blue-100 mx-auto flex items-center justify-center mb-3 ring-1 ring-inset ring-indigo-200/60">
              <Sparkles className="w-5 h-5 text-indigo-600" />
            </div>
            <div className="text-slate-700 font-medium">Ask anything about inventory</div>
            <div className="text-xs text-slate-400 mt-1">
              Try: <em>“list the dataviews available”</em> or
              <em> “how many articles are below reorder threshold?”</em>
            </div>
          </div>
        )}
        {messages.map((m, i) => (
          <Message
            key={i}
            msg={m}
            onOpenDetail={() => m.role === "assistant" && m.promptId && setOpenDetail(m.promptId)}
            onRerun={m.role === "user" ? () => runText(m.text) : undefined}
            rerunDisabled={submitting}
          />
        ))}
        <div ref={bottomRef} />
      </div>

      <Composer
        draft={draft}
        setDraft={setDraft}
        submitting={submitting}
        onSubmit={send}
        model={props.session.model}
      />

      {openDetail && (
        <PromptDetailDrawer promptId={openDetail} onClose={() => setOpenDetail(null)} />
      )}
    </div>
  );
}

function Message(props: {
  msg: ThreadMessage;
  onOpenDetail: () => void;
  onRerun?: () => void;
  rerunDisabled?: boolean;
}) {
  const m = props.msg;
  if (m.role === "user") {
    return (
      <div className="flex items-start gap-3 group">
        <div className="w-7 h-7 rounded-full bg-slate-200 text-slate-600 flex items-center justify-center flex-shrink-0">
          <UserIcon className="w-3.5 h-3.5" />
        </div>
        <div className="flex-1 rounded-2xl rounded-tl-md border border-slate-200 bg-white p-3 shadow-sm relative">
          <div className="whitespace-pre-wrap text-slate-900 text-[15px] leading-relaxed">{m.text}</div>
          <div className="mt-1.5 flex items-center justify-between gap-2">
            {m.timestamp != null ? (
              <div className="text-[11px] text-slate-400 inline-flex items-center gap-1">
                <Clock className="w-3 h-3" /> {fmtTimestamp(m.timestamp)}
              </div>
            ) : <span />}
            {props.onRerun && (
              <button
                onClick={props.onRerun}
                disabled={!!props.rerunDisabled}
                className="text-[11px] text-slate-500 hover:text-indigo-600 inline-flex items-center gap-1 px-2 py-0.5 rounded-md hover:bg-slate-100 transition disabled:opacity-40 disabled:cursor-not-allowed opacity-0 group-hover:opacity-100 focus:opacity-100"
                title="Re-run this prompt as a new turn"
              >
                <RefreshCw className="w-3 h-3" /> Rerun
              </button>
            )}
          </div>
        </div>
      </div>
    );
  }
  return (
    <div className="flex items-start gap-3">
      <div className="w-7 h-7 rounded-full bg-gradient-to-br from-indigo-500 to-blue-500 text-white flex items-center justify-center flex-shrink-0 shadow-sm">
        <Sparkles className="w-3.5 h-3.5" />
      </div>
      <div className="flex-1 rounded-2xl rounded-tl-md border border-indigo-100 bg-gradient-to-br from-indigo-50/40 to-blue-50/30 p-3 shadow-sm">
        {m.chips.length > 0 && (
          <div className="flex flex-wrap gap-1.5 mb-2">
            {m.chips.map((c) => (
              <ToolChipPill key={c.call_id} chip={c} />
            ))}
          </div>
        )}
        <div className="text-slate-900 text-[15px] leading-relaxed">
          {m.text ? (
            <MarkdownBody text={m.text} />
          ) : (
            <span className="text-slate-400 inline-flex items-center gap-1.5">
              {m.chips.length
                ? <><Loader2 className="w-3.5 h-3.5 animate-spin" /> thinking…</>
                : "…"}
            </span>
          )}
        </div>
        <div className="flex items-center gap-3 mt-2 text-xs text-slate-500 flex-wrap">
          {m.model && (
            <span
              className="inline-flex items-center px-1.5 py-0.5 rounded-md text-[11px] font-medium bg-slate-100 text-slate-600 ring-1 ring-inset ring-slate-200"
              title="Model that produced this response"
            >
              {m.model}
            </span>
          )}
          {m.usage && (
            <span className="inline-flex items-center gap-1">
              <Zap className="w-3 h-3" /> {m.usage.tokens_in.toLocaleString()} in · {m.usage.tokens_out.toLocaleString()} out
            </span>
          )}
          {m.latencyMs != null && (
            <span className="inline-flex items-center gap-1">
              <Clock className="w-3 h-3" /> {fmtDuration(m.latencyMs)}
            </span>
          )}
          {m.costUsd != null && (
            <span className="inline-flex items-center gap-1">
              <DollarSign className="w-3 h-3" /> {fmtCost(m.costUsd)}
            </span>
          )}
          {m.timestamp != null && (
            <span className="inline-flex items-center gap-1 text-slate-400">
              {fmtTimestamp(m.timestamp)}
            </span>
          )}
          {m.promptId && (
            <button onClick={props.onOpenDetail} className="text-indigo-600 hover:text-indigo-800 hover:underline">
              detail
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

/**
 * Renders an assistant turn's text as GitHub-flavored Markdown. We override
 * every element react-markdown emits with explicit Tailwind classes so the
 * output sits naturally inside the agent bubble — no default browser
 * margins, tables clipped to the bubble width with horizontal scroll, code
 * blocks visually distinct. `remark-gfm` is what enables pipe-syntax tables
 * + task lists + strikethrough + autolinks.
 */
function MarkdownBody({ text }: { text: string }) {
  return (
    <div className="space-y-2.5 [&>*:first-child]:mt-0 [&>*:last-child]:mb-0">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          // Headings — bumped down a level from default sizing to fit inside
          // a message bubble (the user's prompt is the implicit h1).
          h1: ({ children }) => <h2 className="text-base font-semibold text-slate-900 mt-3 mb-1">{children}</h2>,
          h2: ({ children }) => <h3 className="text-sm font-semibold text-slate-900 mt-3 mb-1">{children}</h3>,
          h3: ({ children }) => <h4 className="text-sm font-semibold text-slate-800 mt-2 mb-1">{children}</h4>,
          h4: ({ children }) => <h5 className="text-xs font-semibold uppercase tracking-wide text-slate-600 mt-2 mb-1">{children}</h5>,

          p:  ({ children }) => <p className="leading-relaxed">{children}</p>,

          // Lists. Use proper bullets/numbers and tight spacing.
          ul: ({ children }) => <ul className="list-disc pl-5 space-y-1">{children}</ul>,
          ol: ({ children }) => <ol className="list-decimal pl-5 space-y-1">{children}</ol>,
          li: ({ children }) => <li className="leading-relaxed">{children}</li>,

          // Tables — wrap in a scroll container so wide ones don't blow the
          // bubble width. Header gets a tinted background and the rest
          // alternates row colors for legibility.
          table: ({ children }) => (
            <div className="overflow-x-auto rounded-lg border border-slate-200 bg-white">
              <table className="min-w-full text-xs">{children}</table>
            </div>
          ),
          thead: ({ children }) => <thead className="bg-slate-50 text-slate-700">{children}</thead>,
          tbody: ({ children }) => <tbody className="divide-y divide-slate-100">{children}</tbody>,
          tr:    ({ children }) => <tr className="even:bg-slate-50/40">{children}</tr>,
          th:    ({ children }) => <th className="px-3 py-2 text-left font-semibold border-b border-slate-200">{children}</th>,
          td:    ({ children }) => <td className="px-3 py-2 align-top text-slate-700 font-mono text-[12px]">{children}</td>,

          // Code. Three cases:
          //   1. ```chart  → ChartBlock dispatcher (kpi / bar / line / pie)
          //   2. fenced    → dark code card (any other language tag)
          //   3. inline    → subtle pill
          code: (props) => {
            const { children, className } = props as { children?: React.ReactNode; className?: string };
            if (className === "language-chart") {
              return <ChartBlock raw={String(children ?? "").trim()} />;
            }
            const isBlock = !!className; // language-* class is only on fenced blocks
            if (isBlock) {
              return (
                <pre className="rounded-lg bg-slate-900 text-slate-100 text-[12px] leading-snug p-3 overflow-x-auto my-2">
                  <code className="font-mono">{children}</code>
                </pre>
              );
            }
            return <code className="px-1.5 py-0.5 rounded bg-slate-100 text-slate-800 font-mono text-[12px]">{children}</code>;
          },
          pre: ({ children }) => <>{children}</>,

          blockquote: ({ children }) => (
            <blockquote className="border-l-2 border-slate-200 pl-3 italic text-slate-600">
              {children}
            </blockquote>
          ),

          a: ({ children, href }) => (
            <a href={href} target="_blank" rel="noreferrer" className="text-indigo-600 hover:text-indigo-800 underline underline-offset-2">
              {children}
            </a>
          ),

          hr: () => <hr className="my-3 border-slate-200" />,
          strong: ({ children }) => <strong className="font-semibold text-slate-900">{children}</strong>,
        }}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
}

function ToolChipPill({ chip }: { chip: ToolChip }) {
  // Map raw status string → a UI category. `cache_hit` from the meter
  // hook is a special "served from cache" state worth highlighting; any
  // other non-ok status is collapsed to "fail" with a red treatment.
  const cat =
    chip.status === "running"     ? "running" :
    chip.status === "ok"          ? "ok" :
    chip.status === "cache_hit"   ? "cache" :
                                    "fail";
  const color =
    cat === "running" ? "bg-amber-50 text-amber-800 border-amber-200" :
    cat === "ok"      ? "bg-emerald-50 text-emerald-800 border-emerald-200" :
    cat === "cache"   ? "bg-sky-50 text-sky-800 border-sky-200" :
                        "bg-rose-50 text-rose-800 border-rose-200";
  const Icon =
    cat === "running" ? Loader2 :
    cat === "ok"      ? CheckCircle2 :
    cat === "cache"   ? Database :
                        XCircle;
  return (
    <span className={`inline-flex items-center gap-1.5 px-2 py-0.5 border rounded-full text-[11px] font-medium ${color}`}>
      <Icon className={`w-3 h-3 ${cat === "running" ? "animate-spin" : ""}`} />
      <span className="font-mono">{chip.tool}</span>
      {chip.duration_ms != null && <span className="opacity-70">{chip.duration_ms}ms</span>}
    </span>
  );
}

function Composer(props: {
  draft: string;
  setDraft: (v: string) => void;
  submitting: boolean;
  onSubmit: () => void;
  model: string;
}) {
  return (
    <div className="rounded-2xl bg-white border border-slate-200 shadow-sm focus-within:border-indigo-300 focus-within:ring-2 focus-within:ring-indigo-100 transition">
      <textarea
        value={props.draft}
        onChange={(e) => props.setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
            e.preventDefault();
            props.onSubmit();
          }
        }}
        placeholder="Ask anything about inventory…   (Ctrl/⌘+Enter to send)"
        rows={3}
        className="w-full px-4 pt-3 pb-1 text-[15px] focus:outline-none resize-none rounded-2xl bg-transparent"
        disabled={props.submitting}
      />
      <div className="flex items-center justify-between px-3 py-2 border-t border-slate-100">
        <span className="text-[11px] text-slate-400 inline-flex items-center gap-1">
          <Zap className="w-3 h-3" /> {props.model}
        </span>
        <button
          onClick={props.onSubmit}
          disabled={!props.draft.trim() || props.submitting}
          className="inline-flex items-center gap-1.5 px-4 py-1.5 bg-gradient-to-br from-indigo-500 to-blue-600 text-white rounded-lg text-sm font-medium hover:from-indigo-600 hover:to-blue-700 disabled:opacity-50 disabled:cursor-not-allowed shadow-sm transition"
        >
          {props.submitting ? (
            <Loader2 className="w-4 h-4 animate-spin" />
          ) : (
            <>
              <Send className="w-3.5 h-3.5" /> Send
            </>
          )}
        </button>
      </div>
    </div>
  );
}

/** Minimum + initial widths for the resizable drawer. Persisted in
 *  localStorage so reloads keep the user's last size. */
const DRAWER_MIN_PX = 320;
const DRAWER_MAX_PX = 1100;
const DRAWER_DEFAULT_PX = 420;
const DRAWER_WIDTH_KEY = "agent.detailDrawer.widthPx";

function PromptDetailDrawer({ promptId, onClose }: { promptId: string; onClose: () => void }) {
  const [data, setData] = useState<Awaited<ReturnType<typeof api.promptDetail>> | null>(null);
  const [width, setWidth] = useState<number>(() => {
    try {
      const stored = parseInt(localStorage.getItem(DRAWER_WIDTH_KEY) || "", 10);
      if (Number.isFinite(stored) && stored >= DRAWER_MIN_PX && stored <= DRAWER_MAX_PX) return stored;
    } catch {/* ignore */}
    return DRAWER_DEFAULT_PX;
  });
  const dragging = useRef(false);

  useEffect(() => {
    api.promptDetail(promptId).then(setData).catch(console.error);
  }, [promptId]);

  // Drag-to-resize. Left-edge grip listens for mousedown; while held, we
  // track mousemove on the window to update width. Mouseup releases.
  // Persist on release so we don't thrash localStorage during the drag.
  useEffect(() => {
    const onMove = (e: MouseEvent) => {
      if (!dragging.current) return;
      const next = Math.min(DRAWER_MAX_PX, Math.max(DRAWER_MIN_PX, window.innerWidth - e.clientX));
      setWidth(next);
    };
    const onUp = () => {
      if (!dragging.current) return;
      dragging.current = false;
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
      try { localStorage.setItem(DRAWER_WIDTH_KEY, String(width)); } catch {/* ignore */}
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, [width]);

  const startDrag = (e: React.MouseEvent) => {
    e.preventDefault();
    dragging.current = true;
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
  };

  return (
    <div
      className="fixed inset-y-0 right-0 bg-white border-l border-slate-200 shadow-2xl overflow-hidden z-30 flex"
      style={{ width: `${width}px` }}
    >
      {/* Left-edge resize grip. Thin invisible interactive strip with a
          subtle visible affordance on hover. */}
      <div
        onMouseDown={startDrag}
        className="w-1.5 cursor-col-resize hover:bg-indigo-200/60 active:bg-indigo-300/70 transition-colors flex-shrink-0"
        title="Drag to resize"
      />
      <div className="flex-1 min-w-0 p-5 overflow-y-auto">
      <div className="flex items-center justify-between mb-4">
        <div className="flex items-center gap-2">
          <div className="w-7 h-7 rounded-lg bg-gradient-to-br from-indigo-500 to-blue-500 text-white flex items-center justify-center">
            <Sparkles className="w-4 h-4" />
          </div>
          <h3 className="font-semibold text-slate-900">Prompt detail</h3>
        </div>
        <div className="flex items-center gap-1">
          {data && <CopyDetailButton data={data} />}
          <button
            onClick={onClose}
            className="text-slate-400 hover:text-slate-700 p-1 rounded-md hover:bg-slate-100 transition"
            title="Close"
          >
            <X className="w-4 h-4" />
          </button>
        </div>
      </div>
      {!data ? (
        <div className="text-sm text-slate-400 inline-flex items-center gap-2">
          <Loader2 className="w-4 h-4 animate-spin" /> Loading…
        </div>
      ) : (
        <>
          <div className="rounded-xl bg-gradient-to-br from-slate-50 to-white border border-slate-200 p-3 mb-4">
            <div className="text-[11px] uppercase tracking-wider text-slate-500 font-medium">Summary</div>
            <div className="mt-1 text-sm text-slate-700">
              <span className="font-mono">{data.prompt.model}</span> · {data.usage?.latency_ms ?? "?"}ms ·{" "}
              <span className="font-medium text-slate-900">${(data.cost_usd ?? 0).toFixed(4)}</span>
            </div>
            {data.cost_breakdown && (
              <CostBreakdownPanel cb={data.cost_breakdown} />
            )}
          </div>

          {data.prompt.status === "error" && (
            <div className="rounded-xl bg-red-50 border border-red-200 p-3 mb-4">
              <div className="text-[11px] uppercase tracking-wider text-red-600 font-medium mb-1">
                Error
              </div>
              <pre className="text-xs text-red-800 font-mono whitespace-pre-wrap break-words">
                {data.prompt.error ?? "(no error message captured — this prompt errored before the error column shipped)"}
              </pre>
            </div>
          )}

          {data.usage && (
            <div className="mb-5">
              <div className="text-[11px] uppercase tracking-wider text-slate-500 font-medium mb-2">Tokens</div>
              <div className="grid grid-cols-2 gap-2">
                <TokenStat label="input"  value={data.usage.tokens_in}  tint="from-sky-50 to-cyan-50" />
                <TokenStat label="output" value={data.usage.tokens_out} tint="from-violet-50 to-fuchsia-50" />
              </div>
            </div>
          )}

          <div className="text-[11px] uppercase tracking-wider text-slate-500 font-medium mb-2">
            API calls ({data.api_calls.length})
          </div>
          <div className="space-y-1.5">
            {data.api_calls.map((c) => (
              <ApiCallCard key={c.id} call={c} />
            ))}
            {data.api_calls.length === 0 && (
              <div className="text-xs text-slate-400">No tool calls.</div>
            )}
          </div>
        </>
      )}
      </div>
    </div>
  );
}

/**
 * Expandable "how was this cost arrived at" panel. Shows the model token
 * line items, every tool call's base + ms + bytes × multiplier, the
 * weights from the active pricing_config row, and subtotal/total. Renders
 * only inside the prompt-detail Summary card.
 */
function CostBreakdownPanel({ cb }: { cb: NonNullable<PromptDetail["cost_breakdown"]> }) {
  const [open, setOpen] = useState(false);
  const fmt = (n: number) => `$${n.toFixed(6)}`;
  return (
    <div className="mt-2">
      <button
        onClick={() => setOpen(!open)}
        className="text-[11px] text-slate-500 hover:text-indigo-600 inline-flex items-center gap-1"
      >
        <ChevronRight className={`w-3 h-3 transition-transform ${open ? "rotate-90" : ""}`} />
        How was this calculated?
      </button>
      {open && (
        <div className="mt-2 space-y-2.5 text-[12px] text-slate-700">
          {/* Tokens */}
          {cb.tokens && (
            <div className="bg-white rounded-lg border border-slate-200 p-2.5">
              <div className="text-[10px] uppercase tracking-wider text-slate-400 mb-1">Tokens</div>
              <div className="space-y-0.5 font-mono text-[11px]">
                <div className="flex justify-between">
                  <span className="text-slate-500">model</span>
                  <span>{cb.tokens.model}{cb.tokens.rate_found ? "" : " (no rate ⚠)"}</span>
                </div>
                <div className="flex justify-between">
                  <span className="text-slate-500">{cb.tokens.tokens_in.toLocaleString()} in × ${cb.tokens.in_per_1k_usd}/1K</span>
                  <span>{fmt(cb.tokens.input_cost_usd)}</span>
                </div>
                <div className="flex justify-between">
                  <span className="text-slate-500">{cb.tokens.tokens_out.toLocaleString()} out × ${cb.tokens.out_per_1k_usd}/1K</span>
                  <span>{fmt(cb.tokens.output_cost_usd)}</span>
                </div>
                <div className="flex justify-between border-t border-slate-100 pt-1 mt-1 font-medium text-slate-800">
                  <span>token subtotal</span>
                  <span>{fmt(cb.tokens.subtotal_usd)}</span>
                </div>
              </div>
            </div>
          )}
          {/* Calls */}
          {cb.calls.length > 0 && (
            <div className="bg-white rounded-lg border border-slate-200 p-2.5">
              <div className="text-[10px] uppercase tracking-wider text-slate-400 mb-1">
                Tool calls · base ${cb.weights.per_call_usd} + ${cb.weights.per_ms_usd}/ms + ${cb.weights.per_byte_out_usd.toExponential(0)}/byte
              </div>
              <div className="space-y-1.5 font-mono text-[11px]">
                {cb.calls.map((c) => (
                  <div key={c.api_call_id} className="border-b border-slate-100 last:border-b-0 pb-1.5 last:pb-0">
                    <div className="flex justify-between text-slate-800">
                      <span className="font-medium">{c.tool}{c.status !== "ok" && ` (${c.status})`}</span>
                      <span>{fmt(c.post_multiplier_usd)}</span>
                    </div>
                    <div className="ml-2 text-slate-500 text-[10.5px]">
                      <div>base {fmt(c.base_call_usd)} + {c.duration_ms}ms·{fmt(c.ms_cost_usd)} + {c.bytes_out.toLocaleString()}B·{fmt(c.bytes_cost_usd)} = {fmt(c.pre_multiplier_usd)}</div>
                      {c.multiplier !== 1 && (
                        <div>× {c.multiplier} ({c.multiplier_key}) = {fmt(c.post_multiplier_usd)}</div>
                      )}
                    </div>
                  </div>
                ))}
                <div className="flex justify-between pt-1 mt-1 font-medium text-slate-800">
                  <span>calls subtotal ({cb.calls.length})</span>
                  <span>{fmt(cb.calls_subtotal_usd)}</span>
                </div>
              </div>
            </div>
          )}
          {/* Total */}
          <div className="flex justify-between font-medium text-slate-900 px-1">
            <span>Total</span>
            <span>{fmt(cb.total_usd)}</span>
          </div>
        </div>
      )}
    </div>
  );
}

/**
 * Copies the prompt detail (summary, tokens, every tool call's args + response)
 * to the clipboard as Markdown — handy for pasting into a debug conversation.
 * Shows a brief "Copied" confirmation after a successful copy.
 */
function CopyDetailButton({ data }: { data: PromptDetail }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    const md = renderDetailAsMarkdown(data);
    try {
      await navigator.clipboard.writeText(md);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch (e) {
      console.error("[copy-detail]", e);
    }
  };
  return (
    <button
      onClick={copy}
      className="text-slate-400 hover:text-slate-700 p-1 rounded-md hover:bg-slate-100 transition flex items-center gap-1 text-xs"
      title="Copy detail as Markdown"
    >
      {copied ? <Check className="w-4 h-4 text-emerald-600" /> : <Copy className="w-4 h-4" />}
      <span className="hidden sm:inline">{copied ? "copied" : "copy"}</span>
    </button>
  );
}

function renderDetailAsMarkdown(data: PromptDetail): string {
  const lines: string[] = [];
  const p = data.prompt;
  const u = data.usage;
  lines.push(`# Prompt detail`);
  lines.push("");
  lines.push(`**model** \`${p.model}\` · **latency** ${u?.latency_ms ?? "?"}ms · **cost** $${(data.cost_usd ?? 0).toFixed(4)}`);
  lines.push("");
  if (p.user_text) {
    lines.push(`## User prompt`);
    lines.push("");
    lines.push(p.user_text);
    lines.push("");
  }
  if (p.response_text) {
    lines.push(`## Assistant response`);
    lines.push("");
    lines.push(p.response_text);
    lines.push("");
  }
  if (u) {
    lines.push(`## Tokens`);
    lines.push("");
    lines.push(`- input: ${u.tokens_in.toLocaleString()}`);
    lines.push(`- output: ${u.tokens_out.toLocaleString()}`);
    lines.push("");
  }
  lines.push(`## API calls (${data.api_calls.length})`);
  lines.push("");
  for (const c of data.api_calls) {
    lines.push(`### \`${c.tool_name}\` — ${c.duration_ms}ms · ${fmtBytes(c.bytes_out)} · ${c.status}`);
    if (c.args_preview != null) {
      lines.push("");
      lines.push(`**args**`);
      lines.push("");
      lines.push("```json");
      lines.push(stringifyMaybeJson(c.args_preview));
      lines.push("```");
    }
    if (c.response_preview != null && c.status !== "error") {
      lines.push("");
      lines.push(`**response**`);
      lines.push("");
      lines.push("```json");
      lines.push(stringifyMaybeJson(c.response_preview));
      lines.push("```");
    }
    if (c.error) {
      lines.push("");
      lines.push(`**error**`);
      lines.push("");
      lines.push("```");
      lines.push(c.error);
      lines.push("```");
    }
    lines.push("");
  }
  return lines.join("\n");
}

/** Same dual-shape handling as `Section`: body might arrive as string or
 *  pre-parsed object. Pretty-prints either. */
function stringifyMaybeJson(v: unknown): string {
  if (v == null) return "";
  if (typeof v === "string") {
    try { return JSON.stringify(JSON.parse(v), null, 2); } catch { return v; }
  }
  try { return JSON.stringify(v, null, 2); } catch { return String(v); }
}

/**
 * One row in the prompt-detail API-calls list. Header is always visible
 * (tool name + status icon + duration + bytes). Body expands on click to
 * show the args the model passed and the (truncated) response — useful for
 * auditing what the agent actually ran.
 */
function ApiCallCard({ call: c }: { call: PromptDetail["api_calls"][number] }) {
  const [open, setOpen] = useState(false);
  const isErr   = c.status === "error" || c.status === "timeout";
  const isCache = c.status === "cache_hit";
  const StatusIcon = isErr ? XCircle : isCache ? Database : CheckCircle2;
  const hasBody = !!(c.args_preview || c.response_preview) || (isErr && !!c.error);
  return (
    <div
      className={[
        "border rounded-lg text-xs",
        isErr   ? "border-rose-200 bg-rose-50"      :
        isCache ? "border-sky-200  bg-sky-50/50"    :
                  "border-slate-200 bg-white",
      ].join(" ")}
    >
      <button
        type="button"
        disabled={!hasBody}
        onClick={() => setOpen(o => !o)}
        className={`w-full p-2.5 flex items-center justify-between gap-2 ${hasBody ? "cursor-pointer hover:bg-black/[0.02]" : "cursor-default"}`}
      >
        <span className="font-mono text-slate-800 flex items-center gap-1.5 min-w-0 truncate">
          <StatusIcon className={`w-3.5 h-3.5 flex-shrink-0 ${isErr ? "text-rose-600" : isCache ? "text-sky-600" : "text-emerald-600"}`} />
          {hasBody && (
            <ChevronRight
              className={`w-3 h-3 flex-shrink-0 text-slate-400 transition-transform ${open ? "rotate-90" : ""}`}
            />
          )}
          {c.tool_name}
        </span>
        <span className={`flex-shrink-0 tabular-nums ${isErr ? "text-rose-700" : "text-slate-500"}`}>
          {c.duration_ms}ms · {fmtBytes(c.bytes_out)}
        </span>
      </button>
      {open && hasBody && (
        <div className="border-t border-slate-200 px-2.5 py-2 space-y-2 bg-white">
          {c.args_preview != null && (
            <Section title="args" body={c.args_preview as unknown} />
          )}
          {c.response_preview != null && !isErr && (
            <Section title="response" body={c.response_preview as unknown} />
          )}
          {isErr && c.error && (
            <Section title="error" body={c.error} tint="rose" />
          )}
        </div>
      )}
    </div>
  );
}

function Section({ title, body, tint }: { title: string; body: unknown; tint?: "rose" }) {
  // The detail endpoint returns previews as JSON-text in SQLite, but the
  // backend's `row_value_to_json` auto-parses TEXT columns that happen to
  // be valid JSON. So `body` can arrive as either a string or a
  // pre-parsed object. Render either uniformly:
  //   - object/array → pretty-print
  //   - string that parses as JSON → pretty-print
  //   - anything else → toString
  let display: string;
  if (body == null) {
    display = "";
  } else if (typeof body === "string") {
    try {
      display = JSON.stringify(JSON.parse(body), null, 2);
    } catch {
      display = body;
    }
  } else {
    try {
      display = JSON.stringify(body, null, 2);
    } catch {
      display = String(body);
    }
  }
  const bodyColor = tint === "rose" ? "text-rose-700" : "text-slate-700";
  return (
    <div>
      <div className="text-[10px] uppercase tracking-wider text-slate-500 font-medium mb-0.5">{title}</div>
      <pre className={`font-mono text-[11px] whitespace-pre-wrap break-words leading-snug ${bodyColor} bg-slate-50/80 rounded px-2 py-1.5 max-h-72 overflow-y-auto`}>
        {display}
      </pre>
    </div>
  );
}

function TokenStat({ label, value, tint }: { label: string; value: number; tint: string }) {
  return (
    <div className={`rounded-lg border border-slate-200 p-2.5 bg-gradient-to-br ${tint}`}>
      <div className="text-[10px] uppercase tracking-wider text-slate-500 font-medium">{label}</div>
      <div className="text-lg font-semibold text-slate-900 mt-0.5 leading-none">{value.toLocaleString()}</div>
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

/** Human-friendly duration: ms under a second, s with one decimal under a
 *  minute, mm:ss above. Avoids the "10847ms" wall-of-digits look in the
 *  message footer. */
function fmtDuration(ms: number): string {
  if (ms < 1000)       return `${ms}ms`;
  if (ms < 60_000)     return `${(ms / 1000).toFixed(1)}s`;
  const m = Math.floor(ms / 60_000);
  const s = Math.floor((ms % 60_000) / 1000);
  return `${m}m${s.toString().padStart(2, "0")}s`;
}

/** Cost display: 4 decimal places under $1 (typical agent prompts),
 *  2 decimal places above. Always prefixed with `$`. */
function fmtCost(usd: number): string {
  if (usd < 0.01) return `$${usd.toFixed(4)}`;
  if (usd < 1)    return `$${usd.toFixed(3)}`;
  return `$${usd.toFixed(2)}`;
}

/** Show a date/time the user would actually want to scan. Same-day =
 *  HH:MM:SS local. Otherwise prepend the date. */
function fmtTimestamp(ms: number): string {
  const d = new Date(ms);
  const today = new Date();
  const sameDay =
    d.getFullYear() === today.getFullYear() &&
    d.getMonth() === today.getMonth() &&
    d.getDate() === today.getDate();
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  const ss = String(d.getSeconds()).padStart(2, "0");
  if (sameDay) return `${hh}:${mm}:${ss}`;
  const mon = String(d.getMonth() + 1).padStart(2, "0");
  const dd  = String(d.getDate()).padStart(2, "0");
  return `${mon}/${dd} ${hh}:${mm}`;
}

function fmtBytes(n: number): string {
  if (n < 1024) return `${n}B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)}K`;
  return `${(n / 1024 / 1024).toFixed(1)}M`;
}
