/**
 * Connection state: host, server list, currently bound ProgID.
 */

import { create } from "zustand";
import type { ServerInfo } from "../api/tauri";
import { listServers as listServersApi, connect as connectApi, disconnect as disconnectApi } from "../api/tauri";

interface ConnectionState {
  host: string;
  servers: ServerInfo[];
  progId: string | null;
  loading: boolean;
  error: string | null;

  setHost: (host: string) => void;
  refresh: () => Promise<void>;
  bind: (progId: string) => Promise<void>;
  unbind: () => Promise<void>;
}

export const useConnectionStore = create<ConnectionState>((set, get) => ({
  host: "localhost",
  servers: [],
  progId: null,
  loading: false,
  error: null,

  setHost: (host) => set({ host }),

  refresh: async () => {
    set({ loading: true, error: null });
    try {
      const servers = await listServersApi(get().host);
      set({ servers, loading: false });
    } catch (e) {
      set({ error: String(e), loading: false });
    }
  },

  bind: async (progId) => {
    try {
      await connectApi(progId);
      set({ progId, error: null });
    } catch (e) {
      set({ error: String(e) });
    }
  },

  unbind: async () => {
    try {
      await disconnectApi();
      set({ progId: null });
    } catch (e) {
      set({ error: String(e) });
    }
  },
}));