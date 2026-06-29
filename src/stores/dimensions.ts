import { create } from "zustand";
import { api } from "@/api/client";

interface DimensionsState {
  dimensions: any[];
  loading: boolean;
  error: string | null;
  fetchDimensions: () => Promise<void>;
  createDimension: (data: any) => Promise<any>;
  updateDimension: (id: string, data: any) => Promise<void>;
  deleteDimension: (id: string) => Promise<void>;
}

export const useDimensionsStore = create<DimensionsState>((set, get) => ({
  dimensions: [],
  loading: false,
  error: null,

  fetchDimensions: async () => {
    set({ loading: true, error: null });
    try {
      const dimensions = await api.getDimensions();
      set({ dimensions, loading: false });
    } catch (err: any) {
      set({ error: err.message, loading: false });
    }
  },

  createDimension: async (data) => {
    const dim = await api.createDimension(data);
    set({ dimensions: [...get().dimensions, dim] });
    return dim;
  },

  updateDimension: async (id, data) => {
    const updated = await api.updateDimension(id, data);
    set({ dimensions: get().dimensions.map((d) => (d.id === id ? updated : d)) });
  },

  deleteDimension: async (id) => {
    await api.deleteDimension(id);
    set({ dimensions: get().dimensions.filter((d) => d.id !== id) });
  },
}));
