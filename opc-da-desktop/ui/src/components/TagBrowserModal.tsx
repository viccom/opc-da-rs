/**
 * TagBrowserModal — left tree + right leaf list, multi-select.
 *
 * Backed by `browse_tags` IPC. We don't fully implement the tree
 * recursion client-side (the backend returns a flat vec); instead we
 * provide a single root "Tags" node and let the user select leaves.
 *
 * For a real product we'd cache hierarchical walks per ProgID; this
 * minimal implementation matches the "Add Tags" UX the user described.
 */

import { useEffect, useMemo, useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import type { TagDescriptor } from "../api/tauri";
import { browseTagsInvoke } from "../api/tauri";
import { useConnectionStore } from "../stores/connection";

interface Props {
  onAdd: (tagIds: string[]) => void;
  onClose: () => void;
}

export function TagBrowserModal({ onAdd, onClose }: Props) {
  const progId = useConnectionStore((s) => s.progId);
  const [tags, setTags] = useState<string[]>([]);
  const [filter, setFilter] = useState("");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const channel = new Channel<TagDescriptor>();
    const collected: string[] = [];
    channel.onmessage = (t: TagDescriptor) => {
      collected.push(t.item_id);
      setTags([...collected]);
    };
    browseTagsInvoke(channel, 5000).catch((e) => {
      setError(String(e));
    });
    return () => {
      // Tauri Channel has no explicit close; replacing the handler
      // with a noop drops incoming messages so they don't pile up
      // when the effect re-runs.
      channel.onmessage = () => {};
    };
  }, [progId]);

  const visible = useMemo(() => {
    const f = filter.trim().toLowerCase();
    if (!f) return tags;
    return tags.filter((t) => t.toLowerCase().includes(f));
  }, [tags, filter]);

  const toggle = (id: string) => {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    setSelected(next);
  };

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-header">Add Tags from {progId ?? "(not connected)"}</div>
        <div style={{ padding: "4px 8px", borderBottom: "1px solid #3c3c3c" }}>
          <input
            type="text"
            placeholder="filter…"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            style={{ width: "100%", padding: "2px 6px" }}
          />
        </div>
        {error && (
          <div style={{ padding: 8, color: "#f48771" }}>{error}</div>
        )}
        <div className="modal-body">
          <div className="tree">
            <div className="tree-node selected">{progId ?? "(root)"}</div>
            <div style={{ paddingLeft: 16, color: "#858585", fontSize: 11 }}>
              {tags.length} leaf tags discovered
            </div>
          </div>
          <div className="leaf-list">
            {visible.map((id) => (
              <div
                key={id}
                className={`leaf ${selected.has(id) ? "selected" : ""}`}
                onClick={() => toggle(id)}
              >
                {selected.has(id) ? "☑" : "☐"} {id}
              </div>
            ))}
          </div>
        </div>
        <div className="modal-footer">
          <span style={{ marginRight: "auto", alignSelf: "center", fontSize: 11 }}>
            {selected.size} selected
          </span>
          <button onClick={onClose}>Cancel</button>
          <button onClick={() => onAdd(Array.from(selected))} disabled={selected.size === 0}>
            Add {selected.size > 0 ? `(${selected.size})` : ""}
          </button>
        </div>
      </div>
    </div>
  );
}