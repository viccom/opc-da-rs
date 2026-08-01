/**
 * App — main shell.
 *
 * Layout: a top strip (server picker + connection info) over a row that
 * splits into a left `GroupSidebar` and a main pane showing the selected
 * group's editor + filter + live tag table.
 */

import { useState } from "react";
import { ServerPanel } from "./components/ServerPanel";
import { GroupEditor } from "./components/GroupEditor";
import { FilterBar } from "./components/FilterBar";
import { TagTable } from "./components/TagTable";
import { GroupSidebar } from "./components/GroupSidebar";
import { useConnectionStore } from "./stores/connection";
import { useSubscriptionStore } from "./stores/subscription";
import { getServerStatus, type ServerStatus } from "./api/tauri";

export default function App() {
    const progId = useConnectionStore((s) => s.progId);
    const servers = useConnectionStore((s) => s.servers);
    const error = useConnectionStore((s) => s.error);
    const activeGroup = useSubscriptionStore((s) =>
        s.activeGroupId ? s.groups.get(s.activeGroupId) : undefined,
    );
    const connectedServer = progId
        ? servers.find((s) => s.prog_id === progId)
        : undefined;

    const [status, setStatus] = useState<ServerStatus | null>(null);
    const [statusBusy, setStatusBusy] = useState(false);
    const refreshStatus = async () => {
        if (!progId) return;
        setStatusBusy(true);
        try {
            setStatus(await getServerStatus());
        } catch {
            setStatus(null);
        } finally {
            setStatusBusy(false);
        }
    };

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
                    {connectedServer && (
                        <>
                            <div className="field">
                                <label>CLSID</label>
                                <span
                                    style={{
                                        flex: 1,
                                        fontFamily: "monospace",
                                        fontSize: "0.85em",
                                        color: "#858585",
                                        wordBreak: "break-all",
                                    }}
                                >
                                    {connectedServer.clsid}
                                </span>
                            </div>
                            <div className="field">
                                <label>Type</label>
                                <span style={{ flex: 1, color: "#858585" }}>
                                    {connectedServer.user_type ?? "—"}
                                </span>
                            </div>
                        </>
                    )}
                    {progId && (
                        <div className="field">
                            <label>Status</label>
                            <button
                                onClick={refreshStatus}
                                disabled={statusBusy}
                                style={{ flex: 1 }}
                            >
                                {statusBusy ? "…" : "Refresh Status"}
                            </button>
                        </div>
                    )}
                    {status && (
                        <>
                            <div className="field">
                                <label>State</label>
                                <span style={{ flex: 1, color: "#4ec9b0" }}>
                                    {status.server_state}
                                </span>
                            </div>
                            <div className="field">
                                <label>Vendor</label>
                                <span style={{ flex: 1, color: "#858585" }}>
                                    {status.vendor_info}
                                </span>
                            </div>
                            <div className="field">
                                <label>Version</label>
                                <span style={{ flex: 1, color: "#858585" }}>
                                    {status.version}
                                </span>
                            </div>
                            <div className="field">
                                <label>Start</label>
                                <span style={{ flex: 1, color: "#858585" }}>
                                    {status.start_time}
                                </span>
                            </div>
                            <div className="field">
                                <label>Current</label>
                                <span style={{ flex: 1, color: "#858585" }}>
                                    {status.current_time}
                                </span>
                            </div>
                            <div className="field">
                                <label>LastUpdate</label>
                                <span style={{ flex: 1, color: "#858585" }}>
                                    {status.last_update_time}
                                </span>
                            </div>
                            <div className="field">
                                <label>Groups</label>
                                <span style={{ flex: 1, color: "#858585" }}>
                                    {status.group_count}
                                </span>
                            </div>
                            <div className="field">
                                <label>BandWidth</label>
                                <span style={{ flex: 1, color: "#858585" }}>
                                    {status.band_width}
                                </span>
                            </div>
                        </>
                    )}
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
