import { create } from "zustand";
import { api } from "@/api/client";

interface ComponentsState {
  componentsBySub: Record<string, any[]>; // keyed by submodule_id
  loading: boolean;
  error: string | null;
  fetchComponents: (subId: string) => Promise<void>;
  fetchAllComponents: (subIds: string[]) => Promise<void>;
  createComponent: (subId: string, data: any) => Promise<any>;
  updateComponent: (subId: string, id: string, data: any) => Promise<void>;
  deleteComponent: (subId: string, id: string) => Promise<void>;
}

export const useComponentsStore = create<ComponentsState>((set, get) => ({
  componentsBySub: {},
  loading: false,
  error: null,

  fetchComponents: async (subId) => {
    set({ loading: true, error: null });
    try {
      const comps = await api.getComponents(subId);
      set({ componentsBySub: { ...get().componentsBySub, [subId]: comps }, loading: false });
    } catch (err: any) {
      set({ error: err.message, loading: false });
    }
  },

  fetchAllComponents: async (subIds) => {
    set({ loading: true, error: null });
    try {
      const results: Record<string, any[]> = {};
      for (const subId of subIds) {
        results[subId] = await api.getComponents(subId);
      }
      set({ componentsBySub: results, loading: false });
    } catch (err: any) {
      set({ error: err.message, loading: false });
    }
  },

  createComponent: async (subId, data) => {
    const comp = await api.createComponent(subId, data);
    const existing = get().componentsBySub[subId] || [];
    set({ componentsBySub: { ...get().componentsBySub, [subId]: [...existing, comp] } });
    return comp;
  },

  updateComponent: async (subId, id, data) => {
    const updated = await api.updateComponent(id, data);
    const existing = get().componentsBySub[subId] || [];
    set({ componentsBySub: { ...get().componentsBySub, [subId]: existing.map((c) => (c.id === id ? updated : c)) } });
  },

  deleteComponent: async (subId, id) => {
    await api.deleteComponent(id);
    const existing = get().componentsBySub[subId] || [];
    set({ componentsBySub: { ...get().componentsBySub, [subId]: existing.filter((c) => c.id !== id) } });
  },
}));
