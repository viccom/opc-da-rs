/**
 * Subscription state: active subscription cookie + tag rows updated
 * in real time by the Tauri `Channel<TagUpdate>`.
 */

import { create } from "zustand";
import type { TagUpdate } from "../api/tauri";
import {
  subscribeTags as subscribeApi,
  unsubscribeTags as unsubscribeApi,
  subscribeTagsChannel,
} from "../api/tauri";

interface SubscriptionState {
  cookie: number | null;
  filter: string;
  rows: Map<string, TagUpdate>;

  setFilter: (filter: string) => void;

  /** Subscribe to a set of tags. Returns the cookie. */
  start: (tagIds: string[], updateRateMs: number) => Promise<number>;

  /** Tear down the active subscription. */
  stop: () => Promise<void>;

  /** Internal: apply one update from the channel. */
  applyUpdate: (update: TagUpdate) => void;
}

export const useSubscriptionStore = create<SubscriptionState>((set, get) => {
  // One channel per app instance, created lazily on first `start`.
  let channel: ReturnType<typeof subscribeTagsChannel> | null = null;

  return {
    cookie: null,
    filter: "",
    rows: new Map(),

    setFilter: (filter) => set({ filter }),

    start: async (tagIds, updateRateMs) => {
      // Tear down any prior subscription first.
      const prior = get().cookie;
      if (prior !== null) {
        await unsubscribeApi(prior).catch(() => undefined);
        set({ cookie: null, rows: new Map() });
      }
      channel = subscribeTagsChannel();
      channel.onmessage = (update) => {
        get().applyUpdate(update);
      };
      const { cookie } = await subscribeApi(tagIds, updateRateMs, channel);
      set({ cookie });
      return cookie;
    },

    stop: async () => {
      const cookie = get().cookie;
      if (cookie !== null) {
        await unsubscribeApi(cookie).catch(() => undefined);
        set({ cookie: null, rows: new Map() });
        channel = null;
      }
    },

    applyUpdate: (update) => {
      const next = new Map(get().rows);
      next.set(update.tag_id, update);
      set({ rows: next });
    },
  };
});