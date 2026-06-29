import { useState, useCallback } from "react";
import { api } from "@/api/client";
import { useWorkspaceStore } from "@/stores/workspace";
import {
  Bell, BellOff, Check, CheckCheck, AlertCircle,
  Loader2, RefreshCw, Filter, Bookmark,
} from "lucide-react";

interface Props {
  notifications: any[];
  setNotifications: (updater: any[] | ((prev: any[]) => any[])) => void;
  readIdsRef: React.MutableRefObject<Set<string>>;
  saveReadIds: (ids: Set<string>) => void;
  reload: () => Promise<void>;
}

export function ActivityPanel({ notifications, setNotifications, readIdsRef, saveReadIds, reload }: Props) {
  const setUnreadCount = useWorkspaceStore((s) => s.setUnreadCount);
  const [loading, setLoading] = useState(false);
  const [showUnreadOnly, setShowUnreadOnly] = useState(false);
  const [categoryFilter, setCategoryFilter] = useState<string | null>(null);

  const readIds = readIdsRef.current;

  const handleReload = useCallback(async () => {
    setLoading(true);
    await reload();
    setLoading(false);
  }, [reload]);

  const markRead = (id: string) => {
    const next = new Set(readIds);
    next.add(id);
    saveReadIds(next);
  };

  const markUnread = (id: string) => {
    const next = new Set(readIds);
    next.delete(id);
    saveReadIds(next);
  };

  const markAllRead = () => {
    const next = new Set(readIds);
    notifications.forEach((n) => next.add(n._id));
    saveReadIds(next);
    setUnreadCount(0);
  };

  const toggleFollowUp = async (n: any) => {
    try {
      await api.toggleFollowUp(n.rowid);
      setNotifications((prev: any[]) =>
        prev.map((item) =>
          item._id === n._id ? { ...item, follow_up: !item.follow_up } : item
        )
      );
    } catch {}
  };

  const filtered = notifications.filter((n) => {
    if (showUnreadOnly && !readIds.has(n._id) === false) return false;
    if (showUnreadOnly && readIds.has(n._id)) return false;
    if (categoryFilter && n.category !== categoryFilter) return false;
    return true;
  });

  const categories = [...new Set(notifications.map((n) => n.category).filter(Boolean))];

  return (
    <div className="flex flex-col h-full">
      {/* Toolbar */}
      <div className="flex items-center justify-between px-3 py-2 border-b border-gray-800 shrink-0">
        <div className="flex items-center gap-1">
          <button
            onClick={() => setShowUnreadOnly(!showUnreadOnly)}
            title={showUnreadOnly ? "Show all" : "Show unread only"}
            className={`p-1 rounded transition-colors ${
              showUnreadOnly ? "text-blue-400 bg-blue-900/30" : "text-gray-500 hover:text-gray-300"
            }`}
          >
            {showUnreadOnly ? <BellOff size={13} /> : <Filter size={13} />}
          </button>
          <select
            value={categoryFilter || ""}
            onChange={(e) => setCategoryFilter(e.target.value || null)}
            className="text-[10px] bg-gray-800 border border-gray-700 text-gray-300 rounded px-1.5 py-0.5"
          >
            <option value="">All</option>
            {categories.map((c) => (
              <option key={c} value={c}>{c}</option>
            ))}
          </select>
        </div>
        <div className="flex items-center gap-0.5">
          {notifications.some((n) => !readIds.has(n._id)) && (
            <button onClick={markAllRead} title="Mark all read"
              className="p-1 rounded text-gray-500 hover:text-green-400 hover:bg-green-900/20">
              <CheckCheck size={13} />
            </button>
          )}
          <button onClick={handleReload} disabled={loading}
            className="p-1 rounded text-gray-500 hover:text-gray-300 disabled:opacity-50">
            {loading ? <Loader2 size={13} className="animate-spin" /> : <RefreshCw size={13} />}
          </button>
        </div>
      </div>

      {/* Notification list */}
      <div className="flex-1 overflow-y-auto">
        {filtered.length === 0 && (
          <div className="flex flex-col items-center justify-center py-12 text-gray-600">
            <Bell size={20} className="mb-2 text-gray-700" />
            <p className="text-xs">{showUnreadOnly ? "No unread notifications" : "No activity yet"}</p>
          </div>
        )}
        {filtered.map((n) => {
          const isRead = readIds.has(n._id);
          const isError = n.status === "failed" || n.category === "error";
          const isPipeline = n.category === "pipeline";
          return (
            <div
              key={n._id}
              onClick={() => !isRead && markRead(n._id)}
              className={`px-3 py-2.5 border-b border-gray-800/50 cursor-pointer transition-colors ${
                isRead ? "bg-transparent" : "bg-blue-900/10"
              } hover:bg-gray-800/50`}
            >
              <div className="flex items-start gap-2">
                {/* Status indicator */}
                <div className="mt-0.5 shrink-0">
                  {isError ? (
                    <AlertCircle size={12} className="text-red-400" />
                  ) : isPipeline ? (
                    <div className={`w-2 h-2 rounded-full mt-0.5 ${n.status === "success" ? "bg-green-500" : "bg-amber-500"}`} />
                  ) : (
                    <div className={`w-2 h-2 rounded-full mt-0.5 ${isRead ? "bg-gray-600" : "bg-blue-500"}`} />
                  )}
                </div>

                {/* Content */}
                <div className="flex-1 min-w-0">
                  <div className="flex items-center gap-1 mb-0.5">
                    <span className={`text-[9px] px-1 py-0.5 rounded font-medium ${
                      isError ? "bg-red-900/40 text-red-400"
                        : isPipeline ? "bg-amber-900/40 text-amber-400"
                        : "bg-gray-800 text-gray-400"
                    }`}>
                      {n.category}
                    </span>
                    {n.action && <span className="text-[9px] text-gray-500">{n.action}</span>}
                    {!isRead && <span className="w-1.5 h-1.5 rounded-full bg-blue-500 shrink-0" />}
                  </div>
                  <p className={`text-[11px] leading-relaxed ${isError ? "text-red-300" : "text-gray-300"}`}>
                    {n.message}
                  </p>
                  {n.detail && n.detail.length > 0 && n.detail !== "0" && (
                    <p className="text-[9px] text-gray-500 mt-0.5 font-mono truncate">{n.detail}</p>
                  )}
                  <div className="flex items-center gap-2 mt-0.5">
                    <span className="text-[9px] text-gray-400">
                      {formatTimestamp(n.timestamp)}
                    </span>
                    {n.duration_ms > 0 && (
                      <span className="text-[9px] text-gray-400">{n.duration_ms}ms</span>
                    )}
                  </div>
                </div>

                {/* Actions */}
                <div className="flex items-center gap-0.5 shrink-0">
                  <button
                    onClick={(e) => { e.stopPropagation(); toggleFollowUp(n); }}
                    title="Toggle follow-up"
                    className={`p-0.5 rounded transition-colors ${
                      n.follow_up ? "text-amber-400" : "text-gray-700 hover:text-amber-400"
                    }`}
                  >
                    <Bookmark size={11} />
                  </button>
                  <button
                    onClick={(e) => { e.stopPropagation(); isRead ? markUnread(n._id) : markRead(n._id); }}
                    title={isRead ? "Mark as unread" : "Mark as read"}
                    className="p-0.5 rounded text-gray-700 hover:text-blue-400"
                  >
                    {isRead ? <Check size={11} /> : <div className="w-1.5 h-1.5 rounded-full bg-blue-500" />}
                  </button>
                </div>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}

function formatTimestamp(ts: string): string {
  if (!ts) return "";
  try {
    const d = new Date(ts);
    const now = new Date();
    const diff = now.getTime() - d.getTime();
    if (diff < 60000) return "just now";
    if (diff < 3600000) return `${Math.floor(diff / 60000)}m ago`;
    if (diff < 86400000) return `${Math.floor(diff / 3600000)}h ago`;
    return d.toLocaleDateString();
  } catch {
    return ts;
  }
}
