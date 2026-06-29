import { create } from "zustand";
import { api } from "@/api/client";

interface FilterConfigsState {
  filterConfigs: any[];
  loading: boolean;
  error: string | null;
  fetchFilterConfigs: () => Promise<void>;
  createFilterConfig: (data: any) => Promise<any>;
  updateFilterConfig: (id: string, data: any) => Promise<void>;
  deleteFilterConfig: (id: string) => Promise<void>;
}

export const useFilterConfigsStore = create<FilterConfigsState>((set, get) => ({
  filterConfigs: [],
  loading: false,
  error: null,

  fetchFilterConfigs: async () => {
    set({ loading: true, error: null });
    try {
      const filterConfigs = await api.getFilterConfigs();
      set({ filterConfigs, loading: false });
    } catch (err: any) {
      set({ error: err.message, loading: false });
    }
  },

  createFilterConfig: async (data) => {
    const fc = await api.createFilterConfig(data);
    set({ filterConfigs: [...get().filterConfigs, fc] });
    return fc;
  },

  updateFilterConfig: async (id, data) => {
    const updated = await api.updateFilterConfig(id, data);
    set({ filterConfigs: get().filterConfigs.map((f) => (f.id === id ? updated : f)) });
  },

  deleteFilterConfig: async (id) => {
    await api.deleteFilterConfig(id);
    set({ filterConfigs: get().filterConfigs.filter((f) => f.id !== id) });
  },
}));
