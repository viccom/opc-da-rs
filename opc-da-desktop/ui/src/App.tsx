/**
 * App — main shell.
 *
 * Layout: a top strip (server picker + connection info) over a row that
 * splits into a left `GroupSidebar` and a main pane showing the selected
 * group's editor + filter + live tag table.
 */

import { ServerPanel } from "./components/ServerPanel";
import { GroupEditor } from "./components/GroupEditor";
import { FilterBar } from "./components/FilterBar";
import { TagTable } from "./components/TagTable";
import { GroupSidebar } from "./components/GroupSidebar";
import { useConnectionStore } from "./stores/connection";
import { useSubscriptionStore } from "./stores/subscription";

export default function App() {
    const progId = useConnectionStore((s) => s.progId);
    const error = useConnectionStore((s) => s.error);
    const activeGroup = useSubscriptionStore((s) =>
        s.activeGroupId ? s.groups.get(s.activeGroupId) : undefined,
    );

    return (
        <div className="app">
            <div className="topbar">
                <ServerPanel />
                <div className="panel">
                    <h3>2. Connection</h3>
                    <div className="field">
                        <label>Server</label>
                        <span style={{ flex: 1, color: progId ? "#4ec9b0" : "#858585" }}>
                            {progId ?? "(not connected)"}
                        </span>
                    </div>
                </div>
            </div>
            <div className="main">
                <GroupSidebar />
                <div className="main-content">
                    {activeGroup ? (
                        <>
                            <GroupEditor />
                            <FilterBar />
                            <div style={{ flex: 1, overflow: "hidden" }}>
                                <TagTable />
                            </div>
                        </>
                    ) : (
                        <div className="empty">
                            No group selected. Create one in the left sidebar.
                        </div>
                    )}
                </div>
            </div>
            {error && <div className="statusbar error">{error}</div>}
        </div>
    );
}
