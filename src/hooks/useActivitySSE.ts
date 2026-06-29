import { useState, useEffect, useCallback, useRef } from "react";
import { api } from "@/api/client";
import { useWorkspaceStore } from "@/stores/workspace";

/**
 * Hook that maintains an always-on SSE connection for activity events.
 * Mount this once at the app/layout level so the badge updates even when
 * the Activity panel is closed.
 */
export function useActivitySSE(tenantId: string) {
  const setUnreadCount = useWorkspaceStore((s) => s.setUnreadCount);
  const [notifications, setNotificationsState] = useState<any[]>([]);
  const readIdsRef = useRef<Set<string>>(new Set());

  // Load read IDs from localStorage on init
  useEffect(() => {
    try {
      const stored = localStorage.getItem(`notif-read-${tenantId}`);
      if (stored) readIdsRef.current = new Set(JSON.parse(stored));
    } catch {}
  }, [tenantId]);

  const computeUnread = useCallback((notifs: any[]) => {
    const unread = notifs.filter((r) => !readIdsRef.current.has(r._id)).length;
    setUnreadCount(unread);
  }, [setUnreadCount]);

  const setNotifications = useCallback((updater: any[] | ((prev: any[]) => any[])) => {
    setNotificationsState((prev) => {
      const next = typeof updater === "function" ? updater(prev) : updater;
      computeUnread(next);
      return next;
    });
  }, [computeUnread]);

  // Initial load from REST API
  useEffect(() => {
    if (!tenantId) return;
    (async () => {
      try {
        const result = await api.getActivity({ limit: 200, hours_ago: 24 });
        const rows = (result.rows || []).map((r: any, i: number) => ({
          ...r,
          _id: `${r.timestamp}-${i}`,
        }));
        setNotifications(rows);
      } catch (e) {
        console.warn("Failed to load activity:", e);
      }
    })();
  }, [tenantId, setNotifications]);

  // SSE connection — always on, auto-reconnect
  useEffect(() => {
    if (!tenantId) return;
    let es: EventSource | null = null;
    let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
    let closed = false;

    const connect = () => {
      if (closed) return;
      es = new EventSource(`/api/activity/stream`);

      es.onmessage = (event) => {
        try {
          const data = JSON.parse(event.data);
          const newNotif = {
            ...data,
            _id: `sse-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
            timestamp: new Date().toISOString(),
          };
          setNotifications((prev) => [newNotif, ...prev]);
        } catch {}
      };

      es.onerror = () => {
        es?.close();
        if (!closed) {
          reconnectTimer = setTimeout(connect, 3000);
        }
      };
    };

    connect();

    return () => {
      closed = true;
      es?.close();
      if (reconnectTimer) clearTimeout(reconnectTimer);
    };
  }, [tenantId, setNotifications]);

  // Methods for the panel to call
  const saveReadIds = useCallback((ids: Set<string>) => {
    readIdsRef.current = ids;
    localStorage.setItem(`notif-read-${tenantId}`, JSON.stringify([...ids]));
    // Force recompute unread
    setNotificationsState((prev) => {
      const unread = prev.filter((r) => !ids.has(r._id)).length;
      setUnreadCount(unread);
      return prev;
    });
  }, [tenantId, setUnreadCount]);

  const reload = useCallback(async () => {
    try {
      const result = await api.getActivity({ limit: 200, hours_ago: 24 });
      const rows = (result.rows || []).map((r: any, i: number) => ({
        ...r,
        _id: `${r.timestamp}-${i}`,
      }));
      setNotifications(rows);
    } catch {}
  }, [tenantId, setNotifications]);

  return { notifications, setNotifications, readIdsRef, saveReadIds, reload };
}
