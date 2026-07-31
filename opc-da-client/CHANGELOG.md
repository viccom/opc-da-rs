# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **Lazy hierarchical browse** (`browse_children`): one-round-trip branch/leaf enumeration per tree-node click, instead of a flat recursive dump. New `BranchNode` / `LeafNode` / `BrowseChildren` types.
- **Per-tag data type** on `TagValue` (`data_type: String`), populated from the VARIANT type via `variant_type_name`.
- **`OpcError::hr_code()`**: access the raw HRESULT without coupling to `windows::core::Error`.
- **Cross-target guard**: non-Windows targets now fail with a single friendly `compile_error!` instead of a wall of unresolved imports.

### Fixed
- **Subscription crash (heap corruption `0xc0000374`)**: `VariantClear` was freeing borrowed `SafeArray` BSTR/`IUnknown` elements; the crash a few seconds into streaming is eliminated.
- **Multi-subscription `OPC_E_DUPLICATENAME`**: subscription groups now get unique names (was a static name) — multiple concurrent subscription groups work.
- **`variant_to_string` array-branch stack overflow**: clamped the copy length to the VARIANT union size (was `min(16)` into an 8-byte union).
- **Latent `Clone` double-free**: removed `Clone` from `RemotePointer`/`RemoteArray` (free-on-drop types) and deleted the dead `into_vec`.
- **`--no-default-features` build**: the `provider` module no longer unconditionally imports the cfg-gated `opc_da` backend.
- **Server-side OPC group leak on `add_items` failure**: the read/write handlers (`read_tag_values`, `read_tag_values_max_age`, `write_tag_value`, `write_tag_value_vqt`, `write_tag_values`) now `remove_group` before returning when `add_items` itself fails, instead of leaking an orphan group on the server.

### Changed
- `authors` / `repository` metadata now reflect the current fork maintainer (original author retained); README links updated.
- README: added **Platform Support** matrix, **Acknowledgements**, **Improvements in this Fork**, and **Third-Party Licenses** sections (vendored bindings attribution).
- `categories` now includes `os::windows-apis`.

## [0.3.0] - 2026-07-30

### Added — OPC DA Subscription & DA 3.0
- **Data-change subscription** (`subscribe`/`unsubscribe`): `IOPCDataCallback` sink via `#[implement]`, `Advise`/`Unadvise`, returns `SubscriptionHandle{cookie, rx}` for async stream consumption.
- **Server-shutdown notification** (`subscribe_shutdown`/`unsubscribe_shutdown`): `IOPCShutdown` sink.
- **MaxAge read** (`read_tag_values_max_age`): `IOPCSyncIO2::ReadMaxAge` (DA 3.0).
- **VQT write** (`write_tag_value_vqt`): value + quality + timestamp (`IOPCSyncIO2::WriteVQT`, DA 3.0).
- **Batch write** (`write_tag_values`): write multiple tags in one COM group.
- **Keep-alive** (`set_keep_alive`): `IOPCGroupStateMgt2::SetKeepAlive` (DA 3.0).
- **Subscription rate** (`set_subscription_rate`): runtime `IOPCGroupStateMgt::SetState`.
- **Item properties** (`get_item_properties`): EU, data type, access rights (`IOPCItemProperties`).
- **Server status** (`get_server_status`): vendor info, state, group count (`IOPCServer::GetStatus`).
- **Error string** (`get_error_string`): server-localized HRESULT text (`IOPCCommon::GetErrorString`).
- **Locale / client name** (`set_locale_id` / `set_client_name`): `IOPCCommon`.
- **Browse filter**: `browse_tags` now accepts `data_type` + `access_rights` filter parameters.
- **DA 3.0 interface wiring**: `ComServer`/`ComGroup` connected to all DA 3.0 traits (Browse, ItemIO, GroupStateMgt2, SyncIO2, AsyncIO3, ItemDeadbandMgt, ItemSamplingMgt).
- **Remote DCOM**: `ComConnector` carries a target `host`; `create_server2`/`CoCreateInstanceEx` for remote server instantiation; `IOPCServerList` remote enumeration.
- **Connection lifecycle**: `disconnect`/`reconnect` inherent methods + `dispatch_with_retry` exponential backoff (max 3, 50/100/200 ms).
- **End-to-end test suite** (`e2e` feature, `tests/e2e.rs`): 19 local + 4 remote tests against Matrikon.OPC.Simulation.1.
- **Diagnostic CLI** (`examples/remote_list.rs`): minimal remote server enumeration tool.

### Fixed
- **VARIANT memory leak** (P0): `clear_variant_array` / `clear_item_states` release embedded BSTR/SafeArray in COM-allocated arrays before `RemotePointer::drop`. All read paths (`handle_read`, `read_max_age`, `get_item_properties`) and write paths (`handle_write`, `handle_write_vqt`, `handle_write_values`) now `VariantClear` local VARIANTs.
- **AuthInfo Bridge dangling pointer**: `try_to_native()` returned a temporary `COAUTHINFO` whose address was taken for `COSERVERINFO.pAuthInfo` — caused `0x800703E6` on 32-bit. Fixed by using `COSERVERINFO{pAuthInfo: null}` (DCOM default auth = current user).
- **`.cargo/config.toml`**: replaced machine-specific portable-msvc linker path with BuildTools path (x64 + i686 targets).
- **`write_tag_value` error message**: copy-paste "Browse error" → "Write error" in `poll_write_result`.

### Changed — Breaking
- `OpcProvider` trait expanded from 4 to 18 methods. Downstream `impl OpcProvider` must implement all new methods (or use `MockOpcProvider` via `test-support`).
- `browse_tags` signature: added `data_type: u16` and `access_rights: u32` parameters.
- `ServerConnector::enumerate_servers` signature: added `host: &str` parameter.
- `ComConnector` is no longer a unit struct; use `ComConnector::new(host)` or `ComConnector::default()` (localhost).

## [0.2.0] - 2026-02-23

### Added
- Natively integrated and froze raw `windows-bindgen` outputs into an internal `bindings/` module, dropping required build-time codegen dependencies.
- Added defensive array length validation checks on the return values of `group.add_items` in `ComWorker` to prevent silent zip truncation during per-item tag processing.
- Documented panicking behavior of `OpcDaClient::default()` in rustdocs.

### Changed
- **Major Architectural Refactor**: Completely eliminated the external `opc_da`, `opc_da_bindings`, and `opc_comn_bindings` crate dependencies. The `opc-da-client` is now entirely self-contained for OPC COM communication natively.
  - **Pros vs v0.1.3**: 
    - **Build Velocity**: Eliminates the intermediate `windows-bindgen` code-generation step entirely.
    - **Security & Maintenance**: Severs reliance on abandoned upstream crates. All `unsafe` COM pointers are now directly subject to workspace native `clippy` gating.
    - **Testability**: Enabled the injection of `MockServerConnector`, allowing CI pipelines to organically test application logic without requiring a physical Windows DCOM environment.
- Extracted the primary `OpcDaWrapper` logic and stabilized its initialization footprint beneath the unified `OpcDaClient` identifier.
- Synchronized architectural documentation, replacing obsolete `anyhow::Result` references with the active `OpcResult` type.
  - **Context:** The library migrated away from `anyhow` (an application-level, type-erased error) to `thiserror` (`OpcError`) to adhere to standard Rust library best practices. 
  - **Compatibility:** Because `OpcError` implements `std::error::Error`, downstream consumers (like `opc-cli`) using `anyhow` can still perfectly propagate `OpcResult` using the standard `?` operator.
- Refactored server cache lookup logic in `dispatch_with_retry` to use Rust's `Entry` API, removing double lookup hash penalties and unsafe `.unwrap()` calls.
- **API Simplification**: Internalized the `ComGuard` struct and constructor to be crate-private (`pub(crate)`). Downstream consumers no longer need to handle manual COM initialization; the MTA COM apartment lifecycle is managed transparently on a dedicated background worker thread pool.

### Fixed
- Patched integration doc-tests within `README.md` with `no_run` attributes.
- Fixed potential resource leaks during array length mismatch failures in `ComWorker` by ensuring COM groups are cleaned up immediately.

## [0.1.3] - 2026-02-22

### Fixed
- **docs.rs build failure**: Added `[package.metadata.docs.rs]` with `default-target = "x86_64-pc-windows-msvc"` so documentation builds correctly on crates.io.

### Changed
- Enabled `all-features = true` for docs.rs builds, making `MockOpcProvider` visible in the rendered API docs.
- Synced all documentation with OPC_FLAT browse optimization from v0.1.2 audit.

## [0.1.2] - 2026-02-22

### Changed
- Updated `friendly_com_hint` E_POINTER text — removed stale iterator blame.
- Fixed broken `architecture.md`/`spec.md` links in README for crates.io.
- Added 3 missing HRESULT mappings (`OPC_E_BADTYPE`, `OPC_E_UNKNOWNITEMID`, `OPC_E_INVALIDITEMID`) to documentation.

## [0.1.1] - 2026-02-22

### Fixed
- **OPC-BUG-001**: Eliminated `StringIterator` phantom `E_POINTER` errors at the source (cache zeroing + null-PWSTR skip).
- Removed `is_known_iterator_bug()` workaround — no longer needed.

### Changed
- Write rejection log downgraded from `error!` to `warn!` (handled failure).
- Added `operation` context field to OPC group cleanup warnings.
- Added `info!`-level success logs to `list_servers`, `browse_tags`, `read_tag_values`.

## [0.1.0] - 2026-02-22

### Added

- **`OpcProvider` trait** — async trait with `list_servers`, `browse_tags`, `read_tag_values`, and `write_tag_value`.
- **`OpcDaClient`** — native OPC DA backend via `windows-rs` COM calls. Generic over `ServerConnector` for testability.
- **`ComGuard`** — RAII guard for COM MTA initialization/teardown.
- **`friendly_com_hint()`** — maps 11 known COM/DCOM HRESULT codes to actionable user hints.
- **`format_hresult()`** — formats `HRESULT` as `0xHHHHHHHH: <hint>` for consistent error messages.
- **`TagValue` / `OpcValue` / `WriteResult`** — core data types for read/write operations.
- **`MockOpcProvider`** — optional mock via `test-support` feature flag for downstream testing.
- **`VT_ERROR` handling** — `variant_to_string` now correctly parses and displays error-type VARIANTs.
- **Resource cleanup guarantees** — OPC groups are always removed via `remove_group`, even on error paths.
- **1-to-1 read contract** — `read_tag_values` returns exactly one `TagValue` per requested tag, with error sentinels for failed items.
- **SafeArray display** — array VARIANTs show up to 20 element values instead of opaque `Array[N]`.
- **Comprehensive variant support** — VT_EMPTY, VT_NULL, VT_I2, VT_I4, VT_R4, VT_R8, VT_CY, VT_DATE, VT_BSTR, VT_ERROR, VT_BOOL, VT_I1, VT_UI1, VT_UI2, VT_UI4, VT_I8, VT_UI8, and VT_ARRAY composites.

### Fixed

- Silent tag dropping when `add_items` partially fails during read operations.
- Resource leaks when group creation succeeds but subsequent operations fail.
- Opaque HRESULT formatting in user-facing error messages.
