import { create } from "zustand";
import { api } from "@/api/client";

interface SubModulesState {
  submodulesByModule: Record<string, any[]>; // keyed by module_id
  loading: boolean;
  error: string | null;
  fetchSubModules: (modId: string) => Promise<void>;
  createSubModule: (modId: string, data: any) => Promise<any>;
  updateSubModule: (modId: string, id: string, data: any) => Promise<void>;
  deleteSubModule: (modId: string, id: string) => Promise<void>;
  fetchAllSubModules: (moduleIds: string[]) => Promise<void>;
}

export const useSubModulesStore = create<SubModulesState>((set, get) => ({
  submodulesByModule: {},
  loading: false,
  error: null,

  fetchSubModules: async (modId) => {
    set({ loading: true, error: null });
    try {
      const subs = await api.getSubModules(modId);
      set({
        submodulesByModule: { ...get().submodulesByModule, [modId]: subs },
        loading: false,
      });
    } catch (err: any) {
      set({ error: err.message, loading: false });
    }
  },

  fetchAllSubModules: async (moduleIds) => {
    set({ loading: true, error: null });
    try {
      const results: Record<string, any[]> = {};
      for (const modId of moduleIds) {
        results[modId] = await api.getSubModules(modId);
      }
      set({ submodulesByModule: results, loading: false });
    } catch (err: any) {
      set({ error: err.message, loading: false });
    }
  },

  createSubModule: async (modId, data) => {
    const sub = await api.createSubModule(modId, data);
    const existing = get().submodulesByModule[modId] || [];
    set({
      submodulesByModule: { ...get().submodulesByModule, [modId]: [...existing, sub] },
    });
    return sub;
  },

  updateSubModule: async (modId, id, data) => {
    const updated = await api.updateSubModule(id, data);
    const existing = get().submodulesByModule[modId] || [];
    set({
      submodulesByModule: {
        ...get().submodulesByModule,
        [modId]: existing.map((s) => (s.id === id ? updated : s)),
      },
    });
  },

  deleteSubModule: async (modId, id) => {
    await api.deleteSubModule(id);
    const existing = get().submodulesByModule[modId] || [];
    set({
      submodulesByModule: {
        ...get().submodulesByModule,
        [modId]: existing.filter((s) => s.id !== id),
      },
    });
  },
}));
