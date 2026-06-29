import { useState, useEffect } from "react";
import { Settings, X, Table2, GitBranch, Cpu, Layers, Filter, FileSearch, Database, Bell, Activity, Sun, Moon, ChevronLeft, ChevronRight, Search, MemoryStick, Package, Network, MessageSquare } from "lucide-react";
import { useWorkspaceStore, type ItemType } from "@/stores/workspace";
import Sidebar from "@/components/Sidebar";
import { InspectorPanel } from "@/components/InspectorPanel";
import { SettingsModal } from "@/components/SettingsModal";
import { ActivityPanel } from "@/components/ActivityPanel";
import { BundleModal } from "@/components/BundleModal";
import { useActivitySSE } from "@/hooks/useActivitySSE";
import type { Identity } from "@/api/client";

interface WorkspaceLayoutProps {
  tenantId: string;
  identity: Identity;
  workspace: React.ReactNode;
}

const SECTION_TABS: { key: ItemType; label: string; icon: React.ComponentType<{ size?: number; className?: string }> }[] = [
  { key: "dataview", label: "DataViews", icon: Table2 },
  { key: "shared_pipeline", label: "Pipelines", icon: GitBranch },
  { key: "core_service", label: "Services", icon: Cpu },
  { key: "dimension", label: "Dimensions", icon: Layers },
  { key: "filter_config", label: "Filter Configurations", icon: Filter },
  { key: "source", label: "Sources", icon: FileSearch },
  { key: "connection", label: "Connections", icon: Database },
  { key: "graph", label: "Graphs", icon: Network },
  { key: "query", label: "Query", icon: Database },
  { key: "cross_filter", label: "Cross Filter", icon: Search },
  { key: "memory", label: "Memory", icon: MemoryStick },
  { key: "feedback", label: "Feedback", icon: MessageSquare },
];

export function WorkspaceLayout({ tenantId, identity, workspace }: WorkspaceLayoutProps) {
  const activeTab = useWorkspaceStore((s) => s.activeTab);
  const setActiveTab = useWorkspaceStore((s) => s.setActiveTab);
  const activityOpen = useWorkspaceStore((s) => s.activityOpen);
  const toggleActivity = useWorkspaceStore((s) => s.toggleActivity);
  const unreadCount = useWorkspaceStore((s) => s.unreadCount);
  const inspectorOpen = useWorkspaceStore((s) => s.inspectorOpen);
  const closeInspector = useWorkspaceStore((s) => s.closeInspector);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [bundleOpen, setBundleOpen] = useState(false);

  // Collapse the leftmost sidebar (DataViews / Pipelines / etc list).
  // Persisted so it survives reloads. Hidden on the Query tab anyway.
  const [sidebarCollapsed, setSidebarCollapsed] = useState<boolean>(() => {
    if (typeof window === "undefined") return false;
    return window.localStorage.getItem("ss.sidebar.collapsed") === "1";
  });
  useEffect(() => {
    window.localStorage.setItem("ss.sidebar.collapsed", sidebarCollapsed ? "1" : "0");
  }, [sidebarCollapsed]);

  // Sidebar width — drag-to-resize via the splitter on the right edge.
  // Persisted in localStorage. Bounded so it can't grow off-screen or
  // collapse past the point where the search input is unusable.
  const SIDEBAR_MIN_PX = 160;
  const SIDEBAR_MAX_PX = 600;
  const SIDEBAR_DEFAULT_PX = 224; // matches the original w-56
  const [sidebarWidth, setSidebarWidth] = useState<number>(() => {
    if (typeof window === "undefined") return SIDEBAR_DEFAULT_PX;
    const saved = parseInt(
      window.localStorage.getItem("ss.sidebar.width") || "",
      10,
    );
    if (Number.isFinite(saved) && saved >= SIDEBAR_MIN_PX && saved <= SIDEBAR_MAX_PX) {
      return saved;
    }
    return SIDEBAR_DEFAULT_PX;
  });
  useEffect(() => {
    window.localStorage.setItem("ss.sidebar.width", String(sidebarWidth));
  }, [sidebarWidth]);

  // Splitter drag — register window-level listeners so the drag continues
  // even if the cursor leaves the splitter element. End on mouseup.
  const startResize = (e: React.MouseEvent) => {
    e.preventDefault();
    const startX = e.clientX;
    const startWidth = sidebarWidth;
    const onMove = (ev: MouseEvent) => {
      const next = Math.min(
        SIDEBAR_MAX_PX,
        Math.max(SIDEBAR_MIN_PX, startWidth + (ev.clientX - startX)),
      );
      setSidebarWidth(next);
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
    // Prevent the browser from selecting text while dragging.
    document.body.style.userSelect = "none";
  };

  // Theme: persisted in localStorage. Default 'dark' to match the existing
  // top-bar/sidebar shell which still uses fixed dark Tailwind classes; the
  // CSS in index.css overrides the rest of the UI through the `.dark` class
  // on <html>.
  const [theme, setTheme] = useState<"light" | "dark">(() => {
    if (typeof window === "undefined") return "dark";
    const saved = window.localStorage.getItem("ss.theme");
    return saved === "light" ? "light" : "dark";
  });
  useEffect(() => {
    document.documentElement.classList.toggle("dark", theme === "dark");
    window.localStorage.setItem("ss.theme", theme);
  }, [theme]);

  // SSE hook — always connected, manages notifications + unread count
  const activitySSE = useActivitySSE(tenantId);

  return (
    <div className="flex flex-col h-screen bg-gray-900 text-gray-100">
      {/* ---- Top bar ---- */}
      <header className="flex items-center justify-between h-11 px-4 border-b border-gray-800 bg-gray-900 shrink-0">
        {/* Left */}
        <div className="flex items-center gap-3">
          <span className="text-blue-500 font-semibold text-sm tracking-wide">
            GraphStudio
          </span>
          <span className="px-1.5 py-0.5 text-[10px] font-medium rounded bg-green-900/60 text-green-400 leading-none">
            {identity.environment}
          </span>
          <span className="text-xs text-gray-400">{identity.display_name || identity.client}</span>
        </div>

        {/* Right */}
        <div className="flex items-center gap-1">
          <button
            onClick={() => setTheme((t) => (t === "dark" ? "light" : "dark"))}
            title={`Switch to ${theme === "dark" ? "light" : "dark"} theme`}
            className="p-2 rounded hover:bg-gray-800 text-gray-400 transition-colors"
          >
            {theme === "dark" ? <Sun size={16} /> : <Moon size={16} />}
          </button>
          <button
            onClick={toggleActivity}
            className={`p-2 rounded hover:bg-gray-800 transition-colors relative ${
              activityOpen ? "text-blue-400" : "text-gray-400"
            }`}
            title="Activity & Notifications"
          >
            <Bell size={16} />
            {unreadCount > 0 && (
              <span className="absolute -top-0.5 -right-0.5 min-w-[16px] h-4 bg-red-500 text-white text-[9px] font-bold rounded-full flex items-center justify-center px-1">
                {unreadCount > 99 ? "99+" : unreadCount}
              </span>
            )}
          </button>
          <button
            onClick={() => setBundleOpen(true)}
            title="Bundle export / import"
            className={`p-2 rounded hover:bg-gray-800 transition-colors ${
              bundleOpen ? "text-blue-400" : "text-gray-400"
            }`}
          >
            <Package size={16} />
          </button>
          <button
            onClick={() => setSettingsOpen(true)}
            className={`p-2 rounded hover:bg-gray-800 transition-colors ${
              settingsOpen ? "text-blue-400" : "text-gray-400"
            }`}
          >
            <Settings size={16} />
          </button>
        </div>
      </header>

      {/* ---- Section tabs ---- */}
      <div className="flex items-center gap-0.5 px-4 h-9 border-b border-gray-800 bg-gray-900/80 shrink-0">
        {SECTION_TABS.map(({ key, label, icon: Icon }) => (
          <button
            key={key}
            onClick={() => setActiveTab(key)}
            className={`flex items-center gap-1.5 px-3 py-1 text-xs font-medium rounded whitespace-nowrap transition-colors ${
              activeTab === key
                ? "bg-gray-800 text-gray-900"
                : "text-gray-500 hover:text-gray-300 hover:bg-gray-800/50"
            }`}
          >
            <Icon size={13} />
            {label}
          </button>
        ))}
      </div>

      {/* ---- Body ---- */}
      <div className="flex flex-1 min-h-0">
        {/* Left sidebar (hidden for Query and Cross Filter tabs —
            both are singleton tools with no list). When collapsed, the
            aside shrinks to a 24px strip with an expand chevron; the
            current section's items hide entirely so the workspace
            stretches across. */}
        {activeTab !== "cross_filter" && activeTab !== "memory" && activeTab !== "feedback" && (
          sidebarCollapsed ? (
            <aside className="w-6 bg-gray-900 border-r border-gray-800 shrink-0 flex flex-col items-center pt-2">
              <button
                onClick={() => setSidebarCollapsed(false)}
                title="Expand sidebar"
                className="text-gray-400 hover:text-gray-100 hover:bg-gray-800 p-1 rounded"
              >
                <ChevronRight size={13} />
              </button>
            </aside>
          ) : (
            <aside
              style={{ width: `${sidebarWidth}px` }}
              className="relative bg-gray-900 border-r border-gray-800 overflow-y-auto shrink-0 flex flex-col"
            >
              <div className="flex items-center justify-end px-1 py-1 border-b border-gray-800 shrink-0">
                <button
                  onClick={() => setSidebarCollapsed(true)}
                  title="Collapse sidebar"
                  className="text-gray-400 hover:text-gray-100 hover:bg-gray-800 p-1 rounded"
                >
                  <ChevronLeft size={13} />
                </button>
              </div>
              <div className="flex-1 overflow-y-auto">
                <Sidebar />
              </div>
              {/* Splitter — 4px wide hit zone on the right edge. The visible
                  marker is a 1px line that lights up on hover/active. Window-
                  level listeners (see startResize) keep the drag alive even
                  when the cursor moves outside the strip. Double-click resets
                  to the default width. */}
              <div
                onMouseDown={startResize}
                onDoubleClick={() => setSidebarWidth(SIDEBAR_DEFAULT_PX)}
                title="Drag to resize · double-click to reset"
                className="absolute top-0 right-0 h-full w-1 cursor-col-resize bg-transparent hover:bg-blue-500/40 active:bg-blue-500/60 transition-colors"
              />
            </aside>
          )
        )}

        {/* Center workspace */}
        <main className="flex-1 bg-gray-950 overflow-y-auto">
          {activeTab === "cross_filter" || activeTab === "memory" || activeTab === "graph" || activeTab === "feedback" ? (
            <div className="h-full">{workspace}</div>
          ) : (
            <div className={activeTab === "query" ? "h-full" : "p-6"}>{workspace}</div>
          )}
        </main>

        {/* Right panel: Activity or Inspector */}
        {(activityOpen || inspectorOpen) && (
          <aside className="w-80 bg-gray-900 border-l border-gray-800 shrink-0 flex flex-col">
            {/* Panel header */}
            <div className="flex items-center justify-between px-3 h-9 border-b border-gray-800 shrink-0">
              <div className="flex items-center gap-1.5">
                {activityOpen ? (
                  <>
                    <Activity size={13} className="text-gray-400" />
                    <span className="text-xs font-medium text-gray-300">Activity</span>
                    {unreadCount > 0 && (
                      <span className="text-[9px] bg-red-500/80 text-white px-1.5 py-0.5 rounded-full font-medium leading-none">
                        {unreadCount}
                      </span>
                    )}
                  </>
                ) : (
                  <span className="text-xs font-medium text-gray-400">Inspector</span>
                )}
              </div>
              <button
                onClick={() => {
                  if (activityOpen) toggleActivity();
                  else if (inspectorOpen) closeInspector();
                }}
                className="p-1 rounded hover:bg-gray-800 text-gray-500 transition-colors"
                title="Close panel"
              >
                <X size={14} />
              </button>
            </div>

            {/* Panel content */}
            <div className="flex-1 overflow-y-auto min-h-0">
              {activityOpen ? (
                <ActivityPanel
                  notifications={activitySSE.notifications}
                  setNotifications={activitySSE.setNotifications}
                  readIdsRef={activitySSE.readIdsRef}
                  saveReadIds={activitySSE.saveReadIds}
                  reload={activitySSE.reload}
                />
              ) : inspectorOpen ? (
                <InspectorPanel />
              ) : null}
            </div>
          </aside>
        )}
      </div>

      {/* ---- Settings modal ---- */}
      {settingsOpen && (
        <SettingsModal onClose={() => setSettingsOpen(false)} />
      )}

      {/* ---- Bundle export/import modal ---- */}
      <BundleModal open={bundleOpen} onClose={() => setBundleOpen(false)} />
    </div>
  );
}
