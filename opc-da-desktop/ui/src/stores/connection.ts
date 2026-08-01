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
  /** DCOM credentials (empty user = current logged-in user). */
  user: string;
  password: string;
  domain: string;
  /** Signature of host+creds the backend client is bound to (rebuild dedup). */
  connectedSig: string;
  /** Host the backend client is currently bound to (for display). */
  connectedHost: string;
  servers: ServerInfo[];
  progId: string | null;
  loading: boolean;
  error: string | null;

  setHost: (host: string) => void;
  setUser: (user: string) => void;
  setPassword: (password: string) => void;
  setDomain: (domain: string) => void;
  refresh: () => Promise<void>;
  bind: (progId: string) => Promise<void>;
  unbind: () => Promise<void>;
}

export const useConnectionStore = create<ConnectionState>((set, get) => ({
  host: "localhost",
  user: "",
  password: "",
  domain: "",
  connectedSig: "localhost",
  connectedHost: "localhost",
  servers: [],
  progId: null,
  loading: false,
  error: null,

  setHost: (host) => set({ host }),
  setUser: (user) => set({ user }),
  setPassword: (password) => set({ password }),
  setDomain: (domain) => set({ domain }),

  refresh: async () => {
    set({ loading: true, error: null });
    try {
      const { host, user, password, domain } = get();
      const sig = `${host}\0${user}\0${password}\0${domain}`;
      // Host OR credential change: rebuild the backend client (which tears
      // down its subscriptions), then clear the local subscription store.
      // Unchanged signature skips both and keeps active subscriptions.
      if (sig !== get().connectedSig) {
        await setHostApi(host, user, password, domain);
        await useSubscriptionStore.getState().clearAll();
        // Clear servers too: if listServers fails on the new host/creds, the
        // user must NOT see the old (cross-host-invalid) server list.
        set({ connectedSig: sig, connectedHost: host, progId: null, servers: [] });
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