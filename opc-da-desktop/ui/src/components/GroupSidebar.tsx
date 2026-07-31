/**
 * GroupSidebar — left rail listing subscription groups.
 *
 * Create / select / delete groups. The selected group drives the main
 * pane (its `GroupEditor` + `TagTable`). A filled dot marks a group
 * that is currently streaming (cookie != null).
 */

import { useSubscriptionStore } from "../stores/subscription";
import { useConnectionStore } from "../stores/connection";

export function GroupSidebar() {
    const connLoading = useConnectionStore((s) => s.loading);
    const groups = useSubscriptionStore((s) => s.groups);
    const activeGroupId = useSubscriptionStore((s) => s.activeGroupId);
    const addGroup = useSubscriptionStore((s) => s.addGroup);
    const removeGroup = useSubscriptionStore((s) => s.removeGroup);
    const setActive = useSubscriptionStore((s) => s.setActive);

    const list = Array.from(groups.values());

    return (
        <div className="sidebar">
            <div className="sidebar-header">
                <span>Groups</span>
                <button
                    className="sidebar-add"
                    onClick={() => addGroup()}
                    title="New group"
                    disabled={connLoading}
                >
                    +
                </button>
            </div>
            <div className="sidebar-list">
                {list.length === 0 && (
                    <div className="sidebar-empty">No groups yet. Click + to create one.</div>
                )}
                {list.map((g) => (
                    <div
                        key={g.id}
                        className={`sidebar-item ${g.id === activeGroupId ? "active" : ""}`}
                        onClick={() => {
                            if (!connLoading) setActive(g.id);
                        }}
                    >
                        <span className="sidebar-item-name">
                            {g.cookie !== null && <span className="sidebar-dot" title="streaming">●</span>}
                            {g.name}
                        </span>
                        <span className="sidebar-item-meta">{g.tagIds.length} tags</span>
                        <button
                            className="sidebar-del"
                            title="Delete group"
                            disabled={connLoading}
                            onClick={(e) => {
                                e.stopPropagation();
                                void removeGroup(g.id);
                            }}
                        >
                            ×
                        </button>
                    </div>
                ))}
            </div>
        </div>
    );
}
