import { create } from "zustand";
import { api, type Identity } from "@/api/client";

/**
 * Identity store. Replaces the old multi-tenant `apps` store.
 *
 * The backend now serves exactly one tenant per server, so we just load
 * `/api/identity` once on mount and expose it. Components that previously
 * read `currentApp.id` / `currentApp.display_name` etc. can keep doing so
 * via the `currentApp` compatibility shim — it points at the identity
 * record (with an `id` field that maps to `tenant_id`).
 */
interface AppsState {
  identity: Identity | null;
  /** Compatibility shim: old code reads `currentApp.id`, `.display_name`, `.client`. */
  currentApp: any | null;
  loading: boolean;
  error: string | null;
  fetchIdentity: () => Promise<void>;
  setIdentity: (identity: Identity | null) => void;
}

function shimCurrentApp(identity: Identity | null): any | null {
  if (!identity) return null;
  return {
    id: identity.id,
    display_name: identity.display_name,
    client: identity.client,
    app_type: identity.app_type,
    environment: identity.environment,
    config: {},
  };
}

export const useAppsStore = create<AppsState>((set) => ({
  identity: null,
  currentApp: null,
  loading: false,
  error: null,

  fetchIdentity: async () => {
    set({ loading: true, error: null });
    try {
      const identity = await api.getIdentity();
      set({ identity, currentApp: shimCurrentApp(identity), loading: false });
    } catch (err: any) {
      set({ error: err.message, loading: false });
    }
  },

  setIdentity: (identity) =>
    set({ identity, currentApp: shimCurrentApp(identity) }),
}));
