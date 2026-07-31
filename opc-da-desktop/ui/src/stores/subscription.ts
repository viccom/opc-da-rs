/**
 * Subscription state: multiple named subscription groups.
 *
 * Each group owns its own tag set, refresh rate, OPC cookie, and a
 * `Map<tag_id, TagUpdate>` rows table updated in real time by the group's
 * own Tauri `Channel<TagUpdate>`. The backend (`subscribe_tags`) supports
 * concurrent subscriptions (one OPC group per cookie); this store just
 * keeps them separated by a client-generated group id and exposes the
 * currently-selected group for the main pane.
 */

import { create } from "zustand";
import type { TagUpdate } from "../api/tauri";
import {
    subscribeTags as subscribeApi,
    unsubscribeTags as unsubscribeApi,
    subscribeTagsChannel,
} from "../api/tauri";

export interface GroupState {
    id: string;
    name: string;
    /** Refresh rate in ms. */
    rate: number;
    /** Tag IDs chosen for this group (not yet necessarily subscribed). */
    tagIds: string[];
    /** Backend cookie once subscribed, `null` when stopped. */
    cookie: number | null;
    /** Live values, keyed by tag id. */
    rows: Map<string, TagUpdate>;
    busy: boolean;
    error: string | null;
}

interface SubscriptionState {
    groups: Map<string, GroupState>;
    activeGroupId: string | null;
    filter: string;

    setFilter: (filter: string) => void;

    /** Create a new empty group, select it, return its id. */
    addGroup: () => string;
    /** Delete a group (stopping it first if active). */
    removeGroup: (id: string) => Promise<void>;
    setActive: (id: string) => void;
    setGroupName: (id: string, name: string) => void;
    setGroupRate: (id: string, rate: number) => void;
    setGroupTags: (id: string, tagIds: string[]) => void;

    /** Subscribe the group's tags (replaces any prior subscription for it). */
    startGroup: (id: string) => Promise<void>;
    /** Tear down the group's subscription. */
    stopGroup: (id: string) => Promise<void>;
}

let groupSeq = 0;
function nextGroupId(): string {
    groupSeq += 1;
    return `g${groupSeq}`;
}

// Tauri `Channel` 对象必须在前端保持 JS 引用——一旦被 GC，channel 就关闭，
// 后端 `run_subscription` 的 `channel.send` 送不到任何 listener，订阅表格会
// 一直空。每个订阅组的 channel 在此常驻，直到 stop/remove。
const groupChannels = new Map<string, ReturnType<typeof subscribeTagsChannel>>();

export const useSubscriptionStore = create<SubscriptionState>((set, get) => {
    // Patch one group immutably (new Map → new group object).
    const patchGroup = (id: string, patch: Partial<GroupState>) => {
        set((state) => {
            const cur = state.groups.get(id);
            if (!cur) return {};
            const next = new Map(state.groups);
            next.set(id, { ...cur, ...patch });
            return { groups: next };
        });
    };

    return {
        groups: new Map(),
        activeGroupId: null,
        filter: "",

        setFilter: (filter) => set({ filter }),

        addGroup: () => {
            const id = nextGroupId();
            const group: GroupState = {
                id,
                name: `Group ${get().groups.size + 1}`,
                rate: 1000,
                tagIds: [],
                cookie: null,
                rows: new Map(),
                busy: false,
                error: null,
            };
            set((state) => {
                const next = new Map(state.groups);
                next.set(id, group);
                return { groups: next, activeGroupId: id };
            });
            return id;
        },

        removeGroup: async (id) => {
            const g = get().groups.get(id);
            if (g && g.cookie !== null) {
                await unsubscribeApi(g.cookie).catch(() => undefined);
            }
            groupChannels.delete(id);
            set((state) => {
                const next = new Map(state.groups);
                next.delete(id);
                const active =
                    state.activeGroupId === id
                        ? (next.keys().next().value ?? null)
                        : state.activeGroupId;
                return { groups: next, activeGroupId: active };
            });
        },

        setActive: (id) => set({ activeGroupId: id }),

        setGroupName: (id, name) => patchGroup(id, { name }),
        setGroupRate: (id, rate) => patchGroup(id, { rate }),
        setGroupTags: (id, tagIds) => patchGroup(id, { tagIds }),

        startGroup: async (id) => {
            const g = get().groups.get(id);
            if (!g || g.tagIds.length === 0) return;
            patchGroup(id, { busy: true, error: null });
            try {
                // Replace any prior subscription for this group first.
                if (g.cookie !== null) {
                    await unsubscribeApi(g.cookie).catch(() => undefined);
                    patchGroup(id, { cookie: null, rows: new Map() });
                }
                // Each group gets its own Channel; onmessage routes by id.
                // 必须存进 groupChannels 常驻，否则函数返回后 GC 关闭 channel。
                const channel = subscribeTagsChannel();
                groupChannels.set(id, channel);
                channel.onmessage = (update: TagUpdate) => {
                    const cur = get().groups.get(id);
                    if (!cur) return;
                    const rows = new Map(cur.rows);
                    rows.set(update.tag_id, update);
                    patchGroup(id, { rows });
                };
                const { cookie } = await subscribeApi(g.tagIds, g.rate, channel);
                patchGroup(id, { cookie, busy: false });
            } catch (e) {
                patchGroup(id, { busy: false, error: String(e) });
            }
        },

        stopGroup: async (id) => {
            const g = get().groups.get(id);
            if (!g || g.cookie === null) return;
            patchGroup(id, { busy: true });
            try {
                await unsubscribeApi(g.cookie).catch(() => undefined);
                groupChannels.delete(id);
                patchGroup(id, { cookie: null, rows: new Map(), busy: false });
            } catch (e) {
                patchGroup(id, { busy: false, error: String(e) });
            }
        },
    };
});
