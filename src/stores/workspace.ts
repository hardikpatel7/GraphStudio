import { create } from "zustand";

export type ItemType =
  | "dataview"
  | "shared_pipeline"
  | "core_service"
  | "dimension"
  | "filter_config"
  | "source"
  | "connection"
  | "graph"
  | "query"
  | "cross_filter"
  | "memory"
  | "feedback";

export interface SelectedItem {
  type: ItemType;
  id: string;
}

interface WorkspaceState {
  /* ---- active top-level tab ---- */
  activeTab: ItemType;
  setActiveTab: (tab: ItemType) => void;

  /* ---- sidebar / workspace ---- */
  selected: SelectedItem | null;
  select: (item: SelectedItem | null) => void;

  /* ---- inspector ---- */
  inspectorOpen: boolean;
  inspectorPayload: any;
  openInspector: (payload: any) => void;
  closeInspector: () => void;

  /* ---- right panel: activity ---- */
  activityOpen: boolean;
  toggleActivity: () => void;

  /* ---- notification badge ---- */
  unreadCount: number;
  setUnreadCount: (n: number) => void;

  /* ---- sidebar search ---- */
  sidebarSearch: string;
  setSidebarSearch: (q: string) => void;
}

export const useWorkspaceStore = create<WorkspaceState>((set, get) => ({
  activeTab: "dataview",
  setActiveTab: (tab) => {
    if (tab === "query") {
      // Query tab: clear sidebar selection, show query workspace
      set({ activeTab: tab, selected: null, sidebarSearch: "" });
    } else {
      set({ activeTab: tab, sidebarSearch: "" });
    }
  },

  selected: null,
  select: (item) => set({ selected: item, inspectorOpen: false, inspectorPayload: null }),

  inspectorOpen: false,
  inspectorPayload: null,
  openInspector: (payload) => set({ inspectorOpen: true, inspectorPayload: payload }),
  closeInspector: () => set({ inspectorOpen: false, inspectorPayload: null }),

  activityOpen: false,
  toggleActivity: () => set({ activityOpen: !get().activityOpen }),

  unreadCount: 0,
  setUnreadCount: (n) => set({ unreadCount: n }),

  sidebarSearch: "",
  setSidebarSearch: (q) => set({ sidebarSearch: q }),
}));
