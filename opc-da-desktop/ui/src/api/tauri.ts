/**
 * Tauri IPC bridge — type-safe wrappers around every backend command.
 *
 * Naming mirrors the Rust `#[tauri::command]` functions 1:1 so the
 * backend stays the source of truth. Channel-based subscription is
 * modeled via the `Channel<T>` API from `@tauri-apps/api/core`.
 */

import { invoke, Channel } from "@tauri-apps/api/core";

/** A server returned by `list_servers`. */
export interface ServerInfo {
  prog_id: string;
}

/** One leaf tag from `browse_tags`. */
export interface TagDescriptor {
  item_id: string;
}

/** Result of a synchronous read (`read_tag_values`). */
export interface TagRow {
  tag_id: string;
  value: string;
  timestamp: string;
  quality: string;
}

/** One update pushed through the subscription channel. */
export interface TagUpdate {
  tag_id: string;
  value: string;
  timestamp: string;
  quality: string;
}

/** Result of a write (`write_tag_value`). */
export interface WriteResult {
  tag_id: string;
  success: boolean;
  error: string | null;
}

/** Result of a subscribe (`subscribe_tags`). */
export interface SubscriptionCreated {
  cookie: number;
  tag_count: number;
}

export async function listServers(host: string): Promise<ServerInfo[]> {
  return invoke<ServerInfo[]>("list_servers", { host });
}

export async function connect(progId: string): Promise<void> {
  return invoke<void>("connect", { progId });
}

export async function disconnect(): Promise<void> {
  return invoke<void>("disconnect");
}

/**
 * Stream tags from the server namespace. `maxTags` caps the total
 * number of tags the backend will push through `channel`.
 */
export async function browseTagsInvoke(
  channel: Channel<TagDescriptor>,
  maxTags: number,
): Promise<void> {
  return invoke<void>("browse_tags", { maxTags, channel });
}

export async function readTagValues(tagIds: string[]): Promise<TagRow[]> {
  return invoke<TagRow[]>("read_tag_values", { tagIds });
}

export async function writeTagValue(
  itemId: string,
  value: unknown,
): Promise<WriteResult> {
  return invoke<WriteResult>("write_tag_value", {
    request: { item_id: itemId, value },
  });
}

/**
 * Open a subscription channel. The `Channel<TagUpdate>` will receive
 * one message per `OnDataChange` callback from the OPC server.
 */
export function subscribeTagsChannel(): Channel<TagUpdate> {
  return new Channel<TagUpdate>();
}

export async function subscribeTags(
  tagIds: string[],
  updateRateMs: number,
  channel: Channel<TagUpdate>,
): Promise<SubscriptionCreated> {
  return invoke<SubscriptionCreated>("subscribe_tags", {
    tagIds,
    updateRateMs,
    channel,
  });
}

export async function unsubscribeTags(cookie: number): Promise<void> {
  return invoke<void>("unsubscribe_tags", { cookie });
}

// ── browse_children: lazy single-level namespace browse (tree browser) ──

/** One child branch of a namespace node (expandable). */
export interface BranchNode {
    /** Fully-qualified branch path (e.g. `"Random"` or `"Bucket Brigade"`). */
    id: string;
    /** Branch browse name, relative to its parent. */
    name: string;
}

/** One child leaf (data tag) of a namespace node. */
export interface LeafNode {
    /** Fully-qualified item ID (the value passed to `subscribe`/`read`/`write`). */
    item_id: string;
    /** Leaf browse name, relative to its parent. */
    name: string;
}

/** Direct children of one namespace node — one lazy browse level. */
export interface BrowseChildren {
    branches: BranchNode[];
    leaves: LeafNode[];
}

/**
 * Browse one namespace level: the direct child branches + leaves under
 * `branchPath` (`null` = root). One round-trip per tree-node click.
 */
export async function browseChildren(
    branchPath: string | null,
): Promise<BrowseChildren> {
    return invoke<BrowseChildren>("browse_children", { branchPath });
}