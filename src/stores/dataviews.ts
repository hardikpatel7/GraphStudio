import { create } from "zustand";
import { api } from "@/api/client";

interface DataViewsState {
  dataviews: any[];
  currentDataView: any | null;
  loading: boolean;
  error: string | null;
  fetchDataViews: () => Promise<void>;
  fetchDataView: (id: string) => Promise<void>;
  createDataView: (data: any) => Promise<any>;
  updateDataView: (id: string, data: any) => Promise<void>;
  deleteDataView: (id: string) => Promise<void>;
  setCurrentDataView: (dv: any | null) => void;
}

export const useDataViewsStore = create<DataViewsState>((set, get) => ({
  dataviews: [],
  currentDataView: null,
  loading: false,
  error: null,

  fetchDataViews: async () => {
    set({ loading: true, error: null });
    try {
      const dataviews = await api.getDataViews();
      set({ dataviews, loading: false });
    } catch (err: any) {
      set({ error: err.message, loading: false });
    }
  },

  fetchDataView: async (id) => {
    set({ loading: true, error: null });
    try {
      const dv = await api.getDataView(id);
      set({ currentDataView: dv, loading: false });
    } catch (err: any) {
      set({ error: err.message, loading: false });
    }
  },

  createDataView: async (data) => {
    const dv = await api.createDataView(data);
    set({ dataviews: [...get().dataviews, dv] });
    return dv;
  },

  updateDataView: async (id, data) => {
    const updated = await api.updateDataView(id, data);
    set({
      dataviews: get().dataviews.map((d) => (d.id === id ? updated : d)),
      currentDataView: get().currentDataView?.id === id ? updated : get().currentDataView,
    });
  },

  deleteDataView: async (id) => {
    await api.deleteDataView(id);
    set({
      dataviews: get().dataviews.filter((d) => d.id !== id),
      currentDataView: get().currentDataView?.id === id ? null : get().currentDataView,
    });
  },

  setCurrentDataView: (dv) => set({ currentDataView: dv }),
}));
