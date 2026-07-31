/**
 * TagTable — high-frequency tag table with name filter + column sort.
 *
 * Rendered as div + CSS grid (NOT a <table>): react-virtual positions each
 * data row with `position:absolute`, which forces `display:table-row` to
 * compute as `block` (CSS 2.1 §9.7). Under `table-layout:auto` every
 * absolute row then sized its own columns from its content, so the header
 * grid and body grids never agreed → columns misaligned. A shared
 * `grid-template-columns` on every row (header + body) makes alignment
 * structural, independent of content.
 *
 * Columns (5): tag id | data type | value | timestamp | quality.
 * Value cell is double-click to write (inline input → Enter commits).
 * `@tanstack/react-virtual` still drives windowing.
 */

import { useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useSubscriptionStore } from "../stores/subscription";
import { writeTagValue } from "../api/tauri";

type SortKey = "tag_id" | "data_type" | "value" | "timestamp" | "quality";
type SortDir = "asc" | "desc";

interface Column {
    key: SortKey;
    label: string;
    sortable: boolean;
}

const COLUMNS: Column[] = [
    { key: "tag_id", label: "Tag ID", sortable: true },
    { key: "data_type", label: "Data Type", sortable: true },
    { key: "value", label: "Value", sortable: true },
    { key: "timestamp", label: "Timestamp", sortable: true },
    { key: "quality", label: "Quality", sortable: true },
];

/** Parse a typed edit string into a JSON value for `write_tag_value`:
 *  integer → number, float → number, true/false → boolean, else string. */
function parseValue(s: string): unknown {
    const t = s.trim();
    if (t === "true") return true;
    if (t === "false") return false;
    if (/^-?\d+$/.test(t)) return Number.parseInt(t, 10);
    if (/^-?\d*\.\d+$/.test(t)) return Number.parseFloat(t);
    return t;
}

export function TagTable() {
    const group = useSubscriptionStore((s) =>
        s.activeGroupId ? s.groups.get(s.activeGroupId) : undefined,
    );
    const filter = useSubscriptionStore((s) => s.filter);
    const rowsMap = group?.rows;
    const cookie = group?.cookie ?? null;

    const list = useMemo(() => (rowsMap ? Array.from(rowsMap.values()) : []), [rowsMap]);
    const [sortKey, setSortKey] = useState<SortKey | null>(null);
    const [sortDir, setSortDir] = useState<SortDir>("asc");

    // Inline write state: which row is being edited + the buffer + last error.
    const [editingId, setEditingId] = useState<string | null>(null);
    const [editValue, setEditValue] = useState("");
    const [writeError, setWriteError] = useState<string | null>(null);

    const visible = useMemo(() => {
        const f = filter.trim().toLowerCase();
        const arr = f ? list.filter((r) => r.tag_id.toLowerCase().includes(f)) : list;
        if (!sortKey) return arr;
        const sorted = [...arr].sort((a, b) => {
            const cmp = a[sortKey].localeCompare(b[sortKey]);
            return sortDir === "asc" ? cmp : -cmp;
        });
        return sorted;
    }, [list, filter, sortKey, sortDir]);

    const parentRef = useRef<HTMLDivElement>(null);
    const virtualizer = useVirtualizer({
        count: visible.length,
        getScrollElement: () => parentRef.current,
        estimateSize: () => 24,
        overscan: 12,
    });

    const commitWrite = async (tagId: string) => {
        const value = parseValue(editValue);
        setEditingId(null);
        try {
            const r = await writeTagValue(tagId, value);
            setWriteError(r.success ? null : r.error ?? "write failed");
        } catch (e) {
            setWriteError(String(e));
        }
    };

    if (cookie === null) {
        return (
            <div style={{ padding: 16, color: "#858585" }}>
                No active subscription. Use “Add Tags…” then “Start” to begin streaming.
            </div>
        );
    }

    const toggleSort = (key: SortKey) => {
        if (sortKey === key) {
            setSortDir(sortDir === "asc" ? "desc" : "asc");
        } else {
            setSortKey(key);
            setSortDir("asc");
        }
    };

    return (
        <div className="tag-scroll-wrap">
            {writeError && <div className="write-error">{writeError}</div>}
            <div ref={parentRef} className="tag-scroll">
                <div className="tag-grid">
                    <div className="tag-row tag-header">
                        {COLUMNS.map((c) => (
                            <div
                                key={c.key}
                                className="tag-cell"
                                onClick={() => toggleSort(c.key)}
                                style={{ cursor: "pointer" }}
                            >
                                {c.label}
                                {sortKey === c.key ? (sortDir === "asc" ? " ▲" : " ▼") : ""}
                            </div>
                        ))}
                    </div>
                    <div
                        className="tag-body"
                        style={{ height: virtualizer.getTotalSize(), position: "relative" }}
                    >
                        {virtualizer.getVirtualItems().map((vi) => {
                            const row = visible[vi.index];
                            const editing = editingId === row.tag_id;
                            return (
                                <div
                                    key={row.tag_id}
                                    className="tag-row"
                                    style={{
                                        position: "absolute",
                                        top: 0,
                                        left: 0,
                                        width: "100%",
                                        height: `${vi.size}px`,
                                        transform: `translateY(${vi.start}px)`,
                                    }}
                                >
                                    <div className="tag-cell">{row.tag_id}</div>
                                    <div className="tag-cell tag-muted">
                                        {row.data_type || "—"}
                                    </div>
                                    <div className="tag-cell tag-value">
                                        {editing ? (
                                            <input
                                                className="cell-input"
                                                value={editValue}
                                                autoFocus
                                                onChange={(e) => setEditValue(e.target.value)}
                                                onKeyDown={(e) => {
                                                    if (e.key === "Enter")
                                                        void commitWrite(row.tag_id);
                                                    else if (e.key === "Escape") setEditingId(null);
                                                }}
                                                onBlur={() => setEditingId(null)}
                                            />
                                        ) : (
                                            <span
                                                onDoubleClick={() => {
                                                    setEditingId(row.tag_id);
                                                    setEditValue(row.value);
                                                }}
                                                title="Double-click to write a new value"
                                            >
                                                {row.value || "—"}
                                            </span>
                                        )}
                                    </div>
                                    <div className="tag-cell">{row.timestamp}</div>
                                    <div className="tag-cell">{row.quality}</div>
                                </div>
                            );
                        })}
                    </div>
                </div>
            </div>
        </div>
    );
}
