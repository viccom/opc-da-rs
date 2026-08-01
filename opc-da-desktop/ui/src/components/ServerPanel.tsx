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
    user,
    password,
    domain,
    servers,
    progId,
    loading,
    error,
    setHost,
    setUser,
    setPassword,
    setDomain,
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
          disabled={loading}
        />
        <button onClick={refresh} disabled={loading}>
          {loading ? "…" : "Refresh"}
        </button>
      </div>
      <div className="field">
        <label>User</label>
        <input
          type="text"
          value={user}
          onChange={(e) => setUser(e.target.value)}
          placeholder="空=当前登录用户"
          disabled={loading}
        />
      </div>
      <div className="field">
        <label>Password</label>
        <input
          type="password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          placeholder="空=当前登录用户"
          disabled={loading}
        />
      </div>
      <div className="field">
        <label>Domain</label>
        <input
          type="text"
          value={domain}
          onChange={(e) => setDomain(e.target.value)}
          placeholder="可选（域/机器名）"
          disabled={loading}
        />
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
          disabled={loading}
          onChange={(e) => {
            const v = e.target.value;
            if (v) void bind(v);
          }}
        >
          <option value="" disabled>
            {servers.length === 0 ? "(no servers)" : "— pick one —"}
          </option>
          {servers.map((s) => (
            <option key={s.prog_id} value={s.prog_id} title={s.clsid}>
              {s.prog_id}
              {s.user_type ? ` — ${s.user_type}` : ""}
            </option>
          ))}
        </select>
      </div>
      {progId && (
        <div className="field">
          <label>Connected</label>
          <span style={{ color: "#4ec9b0", flex: 1 }}>{progId}</span>
          <button className="danger" onClick={unbind} disabled={loading}>
            Disconnect
          </button>
        </div>
      )}
    </div>
  );
}