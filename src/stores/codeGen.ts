import { create } from "zustand";
import { api } from "@/api/client";

/**
 * Code generation store. The new backend exposes per-DataView gRPC service
 * generation via `previewDataViewService(dvId)` and `writeDataViewService(dvId)`.
 * The old per-app `previewApp` / `generateApp` endpoints are gone.
 */
interface CodeGenState {
  previewResult: any | null;
  generateResult: any | null;
  languagePacks: any[];
  loading: boolean;
  error: string | null;
  fetchLanguagePacks: () => Promise<void>;
  preview: (dvId: string) => Promise<void>;
  generate: (dvId: string, outputDir?: string) => Promise<void>;
}

export const useCodeGenStore = create<CodeGenState>((set) => ({
  previewResult: null,
  generateResult: null,
  languagePacks: [],
  loading: false,
  error: null,

  fetchLanguagePacks: async () => {
    try {
      const packs = await api.getLanguagePacks();
      set({ languagePacks: packs });
    } catch (err: any) {
      set({ error: err.message });
    }
  },

  preview: async (dvId) => {
    set({ loading: true, error: null, previewResult: null });
    try {
      const result = await api.previewDataViewService(dvId);
      set({ previewResult: result, loading: false });
    } catch (err: any) {
      set({ error: err.message, loading: false });
    }
  },

  generate: async (dvId, outputDir) => {
    set({ loading: true, error: null, generateResult: null });
    try {
      const result = await api.writeDataViewService(dvId, outputDir);
      set({ generateResult: result, loading: false });
    } catch (err: any) {
      set({ error: err.message, loading: false });
    }
  },
}));
