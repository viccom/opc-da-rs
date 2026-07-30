/**
 * TagTable — high-frequency tag table with name filter.
 *
 * Columns (5, per the agreed spec):
 *   tag id | data type (placeholder) | value | timestamp | quality
 *
 * Uses `@tanstack/react-table` for sorting + `@tanstack/react-virtual`
 * for windowing; `useMemo` ensures only the filter change re-iterates.
 */

import { useMemo } from "react";
import {
  createColumnHelper,
  flexRender,
  getCoreRowModel,
  getSortedRowModel,
  useReactTable,
  type SortingState,
} from "@tanstack/react-table";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useRef, useState } from "react";
import { useSubscriptionStore } from "../stores/subscription";
import type { TagUpdate } from "../api/tauri";

const helper = createColumnHelper<TagUpdate>();

const columns = [
  helper.accessor("tag_id", { header: "Tag ID" }),
  helper.accessor(() => "(unknown)", { id: "data_type", header: "Data Type" }),
  helper.accessor("value", { header: "Value" }),
  helper.accessor("timestamp", { header: "Timestamp" }),
  helper.accessor("quality", { header: "Quality" }),
];

export function TagTable() {
  const { rows, filter, cookie } = useSubscriptionStore();
  const list = useMemo(() => Array.from(rows.values()), [rows]);
  const visible = useMemo(() => {
    const f = filter.trim().toLowerCase();
    if (!f) return list;
    return list.filter((r) => r.tag_id.toLowerCase().includes(f));
  }, [list, filter]);

  const [sorting, setSorting] = useState<SortingState>([]);
  const table = useReactTable({
    data: visible,
    columns,
    state: { sorting },
    onSortingChange: setSorting,
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
  });

  const rows2 = table.getRowModel().rows;
  const parentRef = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: rows2.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 24,
    overscan: 12,
  });

  if (cookie === null) {
    return (
      <div style={{ padding: 16, color: "#858585" }}>
        No active subscription. Use “Add Tags…” then “Start” to begin streaming.
      </div>
    );
  }

  return (
    <div
      ref={parentRef}
      style={{ height: "100%", overflow: "auto", position: "relative" }}
    >
      <table>
        <thead>
          {table.getHeaderGroups().map((hg) => (
            <tr key={hg.id}>
              {hg.headers.map((h) => (
                <th
                  key={h.id}
                  onClick={h.column.getToggleSortingHandler()}
                  style={{ cursor: "pointer" }}
                >
                  {flexRender(h.column.columnDef.header, h.getContext())}
                  {h.column.getIsSorted() === "asc" ? " ▲" : h.column.getIsSorted() === "desc" ? " ▼" : ""}
                </th>
              ))}
            </tr>
          ))}
        </thead>
        <tbody style={{ height: `${virtualizer.getTotalSize()}px`, position: "relative" }}>
          {virtualizer.getVirtualItems().map((vi) => {
            const row = rows2[vi.index];
            return (
              <tr
                key={row.id}
                style={{
                  position: "absolute",
                  top: 0,
                  left: 0,
                  width: "100%",
                  height: `${vi.size}px`,
                  transform: `translateY(${vi.start}px)`,
                }}
              >
                {row.getVisibleCells().map((cell) => (
                  <td key={cell.id}>{flexRender(cell.column.columnDef.cell, cell.getContext())}</td>
                ))}
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}