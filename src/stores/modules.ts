import { create } from "zustand";
import { api } from "@/api/client";

interface ModulesState {
  modules: any[];
  loading: boolean;
  error: string | null;
  fetchModules: () => Promise<void>;
  createModule: (data: any) => Promise<any>;
  updateModule: (id: string, data: any) => Promise<void>;
  deleteModule: (id: string) => Promise<void>;
}

export const useModulesStore = create<ModulesState>((set, get) => ({
  modules: [],
  loading: false,
  error: null,

  fetchModules: async () => {
    set({ loading: true, error: null });
    try {
      const modules = await api.getModules();
      set({ modules, loading: false });
    } catch (err: any) {
      set({ error: err.message, loading: false });
    }
  },

  createModule: async (data) => {
    const mod = await api.createModule(data);
    set({ modules: [...get().modules, mod] });
    return mod;
  },

  updateModule: async (id, data) => {
    const updated = await api.updateModule(id, data);
    set({ modules: get().modules.map((m) => (m.id === id ? updated : m)) });
  },

  deleteModule: async (id) => {
    await api.deleteModule(id);
    set({ modules: get().modules.filter((m) => m.id !== id) });
  },
}));
