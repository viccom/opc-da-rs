/**
 * ServerPanel — top-left strip (function 1+2+3).
 *
 * - Host input
 * - Refresh button → calls `list_servers`
 * - Server list with Connect button
 * - Connected status
 */

import { useConnectionStore } from "../stores/connection";

export function ServerPanel() {
  const {
    host,
    servers,
    progId,
    loading,
    error,
    setHost,
    refresh,
    bind,
    unbind,
  } = useConnectionStore();

  return (
    <div className="panel">
      <h3>1. Select the OPC Server</h3>
      <div className="field">
        <label>Host</label>
        <input
          type="text"
          value={host}
          onChange={(e) => setHost(e.target.value)}
          placeholder="localhost or IP"
        />
        <button onClick={refresh} disabled={loading}>
          {loading ? "…" : "Refresh"}
        </button>
      </div>
      {error && (
        <div className="field" style={{ color: "#f48771" }}>
          {error}
        </div>
      )}
      <div className="field">
        <label>Servers</label>
        <select
          size={6}
          style={{ flex: 1, minHeight: 80 }}
          value={progId ?? ""}
          onChange={(e) => {
            const v = e.target.value;
            if (v) void bind(v);
          }}
        >
          <option value="" disabled>
            {servers.length === 0 ? "(no servers)" : "— pick one —"}
          </option>
          {servers.map((s) => (
            <option key={s.prog_id} value={s.prog_id}>
              {s.prog_id}
            </option>
          ))}
        </select>
      </div>
      {progId && (
        <div className="field">
          <label>Connected</label>
          <span style={{ color: "#4ec9b0", flex: 1 }}>{progId}</span>
          <button className="danger" onClick={unbind}>
            Disconnect
          </button>
        </div>
      )}
    </div>
  );
}