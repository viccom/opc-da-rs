/**
 * App — main shell. Three-column top strip (server / connect-info /
 * group-editor) over the full-width tag table.
 *
 * The middle "connect info" panel is left empty for now; the live
 * server status payload could be rendered there once we add a
 * `get_server_status` channel.
 */

import { ServerPanel } from "./components/ServerPanel";
import { GroupEditor } from "./components/GroupEditor";
import { FilterBar } from "./components/FilterBar";
import { TagTable } from "./components/TagTable";
import { useConnectionStore } from "./stores/connection";

export default function App() {
  const progId = useConnectionStore((s) => s.progId);
  const error = useConnectionStore((s) => s.error);

  return (
    <div className="app">
      <div className="strips">
        <ServerPanel />
        <div className="panel">
          <h3>2. Connection</h3>
          <div className="field">
            <label>Server</label>
            <span style={{ flex: 1, color: progId ? "#4ec9b0" : "#858585" }}>
              {progId ?? "(not connected)"}
            </span>
          </div>
          <div className="field" style={{ color: "#858585", fontSize: 11 }}>
            Live `get_server_status` could populate this strip — left as a follow-up.
          </div>
        </div>
        <GroupEditor />
      </div>
      <FilterBar />
      <div style={{ flex: 1, overflow: "hidden" }}>
        <TagTable />
      </div>
      {error && <div className="statusbar error">{error}</div>}
    </div>
  );
}