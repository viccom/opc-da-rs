/**
 * FilterBar — substring filter on tag id; live local filter (no IPC).
 */

import { useSubscriptionStore } from "../stores/subscription";

export function FilterBar() {
  const filter = useSubscriptionStore((s) => s.filter);
  const setFilter = useSubscriptionStore((s) => s.setFilter);
  return (
    <div style={{ padding: "4px 8px", borderBottom: "1px solid #3c3c3c" }}>
      <input
        type="text"
        placeholder="Filter by tag id…"
        value={filter}
        onChange={(e) => setFilter(e.target.value)}
        style={{ width: 240 }}
      />
    </div>
  );
}