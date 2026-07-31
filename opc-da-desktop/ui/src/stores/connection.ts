/**
 * Connection state: host, server list, currently bound ProgID.
 */

import { create } from "zustand";
import type { ServerInfo } from "../api/tauri";
import {
    listServers as listServersApi,
    connect as connectApi,
    disconnect as disconnectApi,
    setHost as setHostApi,
} from "../api/tauri";
import { useSubscriptionStore } from "./subscription";

interface ConnectionState {
  host: string;
  /** Host the backend client is currently bound to (drives rebuild + clearAll dedup). */
  connectedHost: string;
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
  connectedHost: "localhost",
  servers: [],
  progId: null,
  loading: false,
  error: null,

  setHost: (host) => set({ host }),

  refresh: async () => {
    set({ loading: true, error: null });
    try {
      const host = get().host;
      // Host change: rebuild the backend client (which tears down its
      // subscriptions), then clear the local subscription store. Same-host
      // refresh skips both and keeps active subscriptions.
      if (host !== get().connectedHost) {
        await setHostApi(host);
        await useSubscriptionStore.getState().clearAll();
        // Clear servers too: if listServers fails on the new host, the user
        // must NOT see the old host's (cross-host-invalid) server list.
        set({ connectedHost: host, progId: null, servers: [] });
      }
      const servers = await listServersApi(host);
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