# opc-da-server

Native OPC DA **Custom Server** library for Rust — implement OPC DA servers as out-of-process (LocalServer) COM servers, the reverse of [`opc-da-client`](../opc-da-client/) (which consumes those interfaces).

> **Status**: Phase 0 (COM foundation) in progress. Design: [`docs/superpowers/specs/2026-08-02-opc-da-server-design.md`](../docs/superpowers/specs/2026-08-02-opc-da-server-design.md).
>
> **Platform**: Windows only (COM/DCOM). Reuses `opc-da-client` frozen bindings / `com_utils` / `typedefs` (zero changes to client logic).

## License

MIT — see workspace [`LICENSE`](../LICENSE).
