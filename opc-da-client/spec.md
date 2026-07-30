# 📋 spec.md — opc-da-client

> **Behavioral Source of Truth** for the `opc-da-client` library crate.
> Defines *what* each module should do — independent of current implementation.
>
> Last verified against: e74ee22

---

## 1. Module / Component Contracts

### 1.1 `provider` — Core Trait & Data Types

**Purpose:** Define the async trait that all OPC DA backends must implement, plus the canonical data model for tag values.

#### Public API

##### `trait OpcProvider: Send + Sync`

All methods use `#[async_trait]`.

| Method | Signature | Description |
| :--- | :--- | :--- |
| `list_servers` | `async fn list_servers(&self, host: &str) -> OpcResult<Vec<String>>` | Enumerate OPC DA servers on `host` (local or remote DCOM). |
| `browse_tags` | `async fn browse_tags(&self, server: &str, max_tags: usize, progress: Arc<AtomicUsize>, tags_sink: Arc<Mutex<Vec<String>>>, data_type: u16, access_rights: u32) -> OpcResult<Vec<String>>` | Recursively discover tags with optional data-type/access-rights filter. |
| `read_tag_values` | `async fn read_tag_values(&self, server: &str, tag_ids: Vec<String>) -> OpcResult<Vec<TagValue>>` | Read current value, quality, and timestamp (`IOPCSyncIO::Read`). |
| `write_tag_value` | `async fn write_tag_value(&self, server: &str, tag_id: &str, value: OpcValue) -> OpcResult<WriteResult>` | Write a typed value to a single tag (`IOPCSyncIO::Write`). |
| `get_server_status` | `async fn get_server_status(&self, server: &str) -> OpcResult<ServerStatus>` | Query server status — vendor info, state, group count (`IOPCServer::GetStatus`). |
| `write_tag_values` | `async fn write_tag_values(&self, server: &str, items: Vec<(String, OpcValue)>) -> OpcResult<Vec<WriteResult>>` | Batch write multiple tags in one operation. |
| `get_item_properties` | `async fn get_item_properties(&self, server: &str, tag_id: &str) -> OpcResult<Vec<ItemProperty>>` | Query item properties — EU, data type, access rights (`IOPCItemProperties`). |
| `read_tag_values_max_age` | `async fn read_tag_values_max_age(&self, server: &str, tag_ids: Vec<String>, max_age_ms: u32) -> OpcResult<Vec<TagValue>>` | Read with maximum-age constraint (`IOPCSyncIO2::ReadMaxAge`, DA 3.0). |
| `write_tag_value_vqt` | `async fn write_tag_value_vqt(&self, server: &str, tag_id: &str, value: OpcValue, quality: Option<u16>, timestamp: Option<SystemTime>) -> OpcResult<WriteResult>` | Write value + quality + timestamp (`IOPCSyncIO2::WriteVQT`, DA 3.0). |
| `get_error_string` | `async fn get_error_string(&self, server: &str, hresult: i32) -> OpcResult<String>` | Server-localized error description (`IOPCCommon::GetErrorString`). |
| `subscribe` | `async fn subscribe(&self, server: &str, tag_ids: Vec<String>, update_rate: u32) -> OpcResult<SubscriptionHandle>` | Subscribe to data-change notifications (`IOPCDataCallback` + `Advise`). Returns a `SubscriptionHandle{cookie, rx}`. |
| `unsubscribe` | `async fn unsubscribe(&self, cookie: u32) -> OpcResult<()>` | Tear down a subscription by cookie (`Unadvise`). |
| `subscribe_shutdown` | `async fn subscribe_shutdown(&self, server: &str) -> OpcResult<ShutdownHandle>` | Subscribe to server-shutdown notifications (`IOPCShutdown`). |
| `unsubscribe_shutdown` | `async fn unsubscribe_shutdown(&self, cookie: u32) -> OpcResult<()>` | Tear down a shutdown subscription. |
| `set_subscription_rate` | `async fn set_subscription_rate(&self, cookie: u32, update_rate: u32) -> OpcResult<u32>` | Change update rate at runtime (`IOPCGroupStateMgt::SetState`). |
| `set_keep_alive` | `async fn set_keep_alive(&self, cookie: u32, keep_alive_ms: u32) -> OpcResult<u32>` | Set keep-alive interval (`IOPCGroupStateMgt2::SetKeepAlive`, DA 3.0). |
| `set_locale_id` | `async fn set_locale_id(&self, server: &str, locale_id: u32) -> OpcResult<()>` | Set server locale for string localization (`IOPCCommon::SetLocaleID`). |
| `set_client_name` | `async fn set_client_name(&self, server: &str, name: &str) -> OpcResult<()>` | Set client application name (`IOPCCommon::SetClientName`). |

**Error Conditions:**

| Method | Error Condition | Meaning |
| :--- | :--- | :--- |
| `list_servers` | COM init failure | Windows COM subsystem unavailable. |
| `list_servers` | Registry enumeration failure | OPC Core Components not installed or registry corrupt. |
| `browse_tags` | ProgID resolution failure | `server` string does not map to a registered CLSID. |
| `browse_tags` | Server connection failure | DCOM permissions, server offline, or licensing error. |
| `browse_tags` | Namespace walk failure | Browse position corrupted (failed `UP` navigation). |
| `read_tag_values` | ProgID resolution failure | Same as `browse_tags`. |
| `read_tag_values` | No valid items | None of the requested `tag_ids` could be added to the OPC group. |
| `read_tag_values` | Sync read failure | Server-side read error on all items. |
| `write_tag_value` | ProgID resolution failure | Same as `browse_tags`. |
| `write_tag_value` | Item add failure | The `tag_id` could not be added to the OPC group. |
| `write_tag_value` | Sync write failure | Server-side write error (e.g., read-only tag). |

**Invariants:**

*   All methods are `Send + Sync` safe; they are safe to call from an async context.
*   `list_servers` returns a **sorted, deduplicated** list of ProgID strings.
*   `browse_tags` **never** collects more than `max_tags` items.
*   `browse_tags` pushes tags to `tags_sink` incrementally; on timeout the caller can harvest partial results.
*   `browse_tags` updates `progress` atomically for each discovered tag.
*   `read_tag_values` returns a `TagValue` entry for all requested tags, preserving the original array length and order. Items that fail to be added to the group or read will have their `value` set to `"Error"` and `quality` set to `"Bad — <hint>"`.
*   `write_tag_value` returns `Ok(WriteResult)` in all non-fatal cases; per-tag success/error is reported inside `WriteResult`.


---

##### `struct TagValue`

**Purpose:** Canonical representation of a single OPC DA tag read result.

| Field | Type | Required | Description | Constraints |
| :--- | :--- | :--- | :--- | :--- |
| `tag_id` | `String` | Yes | Fully qualified tag identifier. | Non-empty. |
| `value` | `String` | Yes | Current value as a display string. | May be `"Empty"`, `"Null"`, or formatted number/string. |
| `quality` | `String` | Yes | OPC quality label. | One of `"Good"`, `"Bad"`, `"Uncertain"`, or `"Unknown(0xNNNN)"`. |
| `timestamp` | `String` | Yes | Last-change timestamp as local time. | Format `YYYY-MM-DD HH:MM:SS`, or `"N/A"` / `"Invalid"`. |

**Derives:** `Debug`, `Clone`.

---

##### `enum OpcValue`

**Purpose:** Typed representation of a value to be written to an OPC DA tag.

| Variant | Data Type | Description | COM VT Type |
| :--- | :--- | :--- | :--- |
| `String(String)` | `String` | Raw string value. | `VT_BSTR` |
| `Int(i32)` | `i32` | 32-bit signed integer. | `VT_I4` |
| `Float(f64)` | `f64` | 64-bit float. | `VT_R8` |
| `Bool(bool)` | `bool` | Boolean value. | `VT_BOOL` |

**Derives:** `Debug`, `Clone`, `PartialEq`.

---

##### `struct WriteResult`

**Purpose:** Canonical representation of an OPC DA tag write result.

| Field | Type | Required | Description |
| :--- | :--- | :--- | :--- |
| `tag_id` | `String` | Yes | The tag identifier that was written to. |
| `success` | `bool` | Yes | Whether the write operation succeeded. |
| `error` | `Option<String>` | No | Error message or hint if `success` is `false`. |

**Derives:** `Debug`, `Clone`, `PartialEq`.


---

##### `MockOpcProvider` *(feature = `test-support`)*

**Purpose:** Auto-generated mock of `OpcProvider` via `mockall`, exported when the `test-support` feature is enabled.

**Invariants:**
*   Provides `expect_*` methods for each trait method.
*   Must be fully compatible with `#[tokio::test]` async test harnesses.

---

### 1.2 `helpers` — COM Utility Functions

**Purpose:** Provide reusable helpers for COM error mapping, data conversion, and OPC data formatting.

#### Public API

##### `fn friendly_com_hint(error: &OpcError) -> Option<&'static str>`

**Description:** Inspects the `OpcError` instance for known COM/DCOM HRESULT patterns and returns a human-readable hint.

**Inputs:** An `OpcError` reference.
**Output:** `Some(hint)` if a known code is found, `None` otherwise.

**Known Mappings:**

| HRESULT | Hint |
| :--- | :--- |
| `0x80040112` | Server license does not permit OPC client connections |
| `0x80080005` | Server process failed to start — check if it is installed and running |
| `0x80070005` | Access denied — DCOM launch/activation permissions not configured for this user |
| `0x800706BA` | RPC server unavailable — the target host may be offline or blocking RPC |
| `0x800706F4` | COM marshalling error — try restarting the OPC server |
| `0x80040154` | Server is not registered on this machine |
| `0x80004003` | Invalid pointer (E_POINTER) |
| `0xC0040004` | Server rejected write — the item may be read-only (OPC_E_BADRIGHTS) |
| `0xC0040006` | Data type mismatch — server cannot convert the written value (OPC_E_BADTYPE) |
| `0xC0040007` | Item ID not found in server address space (OPC_E_UNKNOWNITEMID) |
| `0xC0040008` | Item ID syntax is invalid for this server (OPC_E_INVALIDITEMID) |


**Invariants:**
*   Pure function — no side effects, no I/O, no panics.
*   Pattern matching is case-sensitive on the hex string.

---

##### `fn format_hresult(hr: windows::core::HRESULT) -> String`

**Description:** Formats a COM `HRESULT` for user-facing error messages, appending a friendly hint if one is mapped.

**Inputs:** A `windows::core::HRESULT` (passed by value).
**Output:** Format `0xHHHHHHHH: <hint>` if a hint exists, otherwise just `0xHHHHHHHH`.

**Invariants:**
*   Returns a consistently formatted upper-case hex string.

---

#### Internal API (crate-visible only, documented for completeness)

| Function | Signature | Purpose |
| :--- | :--- | :--- |
| `guid_to_progid` | `fn(guid: &GUID) -> Result<String>` | Converts a COM GUID to its registered ProgID string. |
| `variant_to_string` | `fn(variant: &VARIANT) -> String` | Formats a COM VARIANT as a display string. Handles VT_EMPTY, VT_NULL, VT_I2, VT_I4, VT_R4, VT_R8, VT_CY, VT_DATE, VT_BSTR, VT_ERROR, VT_BOOL, VT_I1, VT_UI1, VT_UI2, VT_UI4, VT_I8, VT_UI8, and VT_ARRAY composites. |
| `quality_to_string` | `fn(quality: u16) -> String` | Maps OPC quality bitmask to `"Good"` / `"Bad"` / `"Uncertain"`. |
| `filetime_to_string` | `fn(ft: &FILETIME) -> String` | Converts Win32 FILETIME to local `YYYY-MM-DD HH:MM:SS` string. |
| `opc_value_to_variant` | `fn(value: &OpcValue) -> VARIANT` | Converts an `OpcValue` to a COM `VARIANT`. |


---

### 1.3 `backend::opc_da` — Default OPC DA Backend

**Purpose:** Concrete `OpcProvider` implementation using the `opc_da` crate. Handles COM MTA initialization, server connection, namespace browsing, and synchronous I/O reads.

> [!NOTE]
> Only compiled when feature `opc-da-backend` is enabled (default).

#### Public API

##### `struct OpcDaClient`

| Method | Signature | Description |
| :--- | :--- | :--- |
| `new(connector: C)` | `fn new(connector: C) -> OpcResult<Self>` | Constructs a new wrapper, launching the dedicated COM worker thread. |

Implements `OpcProvider` for all four trait methods by dispatching to the `ComWorker`.

**Invariants:**
*   All COM work runs on a dedicated, long-lived `ComWorker` thread, avoiding repeated initialization overhead and solving COM thread-affinity constraints.
*   Connections are pooled and cached automatically inside the worker, mapped by ProgID.
*   Stale connections are transparently evicted and retried during request dispatch.
*   GUID filtering: zeroed GUIDs are skipped during server enumeration.
*   Server list is sorted and deduplicated before returning.
*   OPC groups created by `read_tag_values` and `write_tag_value` are **always** removed via `remove_group` — even on error paths — to prevent resource leaks.

#### Internal: `browse_recursive`

**Signature:**
```rust
fn browse_recursive(
    server: &Server,
    tags: &mut Vec<String>,
    max_tags: usize,
    progress: &Arc<AtomicUsize>,
    tags_sink: &Arc<Mutex<Vec<String>>>,
    depth: usize,
) -> Result<()>
```

**Behavior:**
1.  Terminates if `depth > 50` (MAX_DEPTH) or `tags.len() >= max_tags`.
2.  Enumerates `OPC_BRANCH` items, descends into each via `change_browse_position(DOWN)`.
3.  **Always** navigates back `UP` after recursing — even if recursion itself fails — to prevent position corruption. Failure to navigate `UP` is a hard error.
4.  Enumerates `OPC_LEAF` items (soft-fail: errors logged and skipped).
5.  Converts browse names to fully-qualified item IDs via `get_item_id()`; falls back to browse name on failure.
6.  Each discovered tag is pushed to both `tags` and `tags_sink`, and `progress` is incremented.

#### Internal: OPC_FLAT Fast Path

Before calling `browse_recursive`, `browse_tags` attempts `BrowseOPCItemIDs(OPC_FLAT)` at root. If the server returns items, they are collected directly as fully-qualified IDs — skipping recursion and `get_item_id()` entirely. Falls back to `browse_recursive` on error, empty results, or first-item failure.

---

### 1.4 `com_guard` — RAII COM Initialization

**Purpose:** Provide a drop guard that ensures `CoUninitialize` is called exactly once per successful `CoInitializeEx`, even on early returns or panics.

#### Internal API

##### `struct ComGuard`

| Method | Signature | Description |
| :--- | :--- | :--- |
| `new()` | `fn new() -> anyhow::Result<Self>` | Initialize COM in Multi-Threaded Apartment (MTA) mode. Returns `Ok` on success or if already initialized (`S_FALSE`). |

**Drop behavior:** Calls `CoUninitialize` only if `CoInitializeEx` returned `Ok`.

**Error Conditions:**

| Error | Meaning |
| :--- | :--- |
| Fatal HRESULT from `CoInitializeEx` | Windows COM subsystem is unavailable or misconfigured. |

**Invariants:**
*   Must be used on the **same thread** that called `new()`.
*   `S_FALSE` (already initialized) is treated as success — the guard will still call `CoUninitialize` on drop.
*   The guard is **not** `Send` or `Sync` — it must remain on the thread that created it.

**Required Test Coverage:**
- [x] Doctest: `ComGuard::new()?` is ignored (internal only).

---

### 1.5 `opc_da` — Internal OPC DA Module

**Purpose:** Provide raw COM wrapping and lifetime management for interacting with OPC DA server, group, and item COM objects. Inherited inline from the previously vendored `opc_da` code (Phase 2), with `actix` support excluded.

#### Public API (Crate-Internal)

##### `client::traits::*`

Contains the core definitions and abstractions representing OPC DA concepts:
- `ClientTrait`: Enumerate and connect to OPC DA servers.
- `ServerTrait`: Create OPC items and groups, introspect namespaces.
- `ItemMgtTrait`: Manage items natively via the server-internal arrays.
- `SyncIoTrait`: Blocking read and write operations.
- `BrowseServerAddressSpaceTrait`: Navigation properties.

##### `def::*`

Definitions representing domain structures mapping to the `tagOPC*` struct data:
- `ServerStatus` / `ServerState`: Enums detailing health and run-state.
- `ItemDef` / `ItemResult`: Types detailing an item added/returned individually.
- `GroupState`: Metadata bounding an OPC Group Object.
- `BrowseType` / `NamespaceType`: Classification rules to discern flat from tiered namespaces.

##### `utils::*`

- `RemoteArray` / `RemotePointer`: Safely wrap `CoTaskMemAlloc` structures and properly route through `Drop`.
- `LocalPointer`: Handle local allocations moving into `CoTaskMem`.

**Invariants:**
- Relies exclusively on `TryFromNative` and `ToNative` bridging to interop efficiently without unsafe footprints bleeding out.

---

## 2. Data Models

### `TagValue`

Defined in § 1.1. See table above.

### Feature Flags

| Flag | Default | Effect |
| :--- | :--- | :--- |
| `opc-da-backend` | ✅ Yes | Compiles the `backend::opc_da` module and exports `OpcDaClient`. |
| `test-support` | ❌ No | Enables `mockall` and exports `MockOpcProvider`. |
| `e2e` | ❌ No | Enables end-to-end integration tests against a real OPC DA server (`tests/e2e.rs`). |

---

## 3. Integration Points

### 3.1 Internal: `opc_da` Module (merged)

**Boundary:** `OpcDaClient` → `opc_da::client::v2::Client` / `Server`.

| Operation | `opc_da` API Used |
| :--- | :--- |
| Server enumeration | `Client.get_servers()` |
| Server connection | `Client.create_server()` |
| Namespace detection | `Server.query_organization()` |
| Tag browsing | `Server.browse_opc_item_ids()` (OPC_LEAF, OPC_BRANCH, OPC_FLAT), `Server.change_browse_position()`, `Server.get_item_id()` |
| Tag reading | `Server.add_group()`, group `.add_items()`, group `.read()`, `Server.remove_group()` |
| Tag writing | `Server.add_group()`, group `.add_items()`, group `.write()`, `Server.remove_group()` |
| String iteration | `StringIterator::new()` |

**Error Handling at Boundary:**
*   All `opc_da` errors are wrapped with `anyhow::Context` to add operation context.
*   `create_server` failures additionally log a `friendly_com_hint` before propagating.
*   `E_POINTER` errors from `StringIterator` are now handled internally by the iterator (null-PWSTR skip + `debug!` log).

**Known Upstream Bugs:**

| ID | Bug | Workaround |
| :--- | :--- | :--- |
| OPC-BUG-001 | `StringIterator` produces 16 phantom `E_POINTER` errors per iterator | **FIXED**: cache zeroing + null-PWSTR skip in `StringIterator::next()` |

### 3.2 Downstream: `opc-cli` (Consumer)

**Boundary:** `opc-cli` → `dyn OpcProvider`.

*   The CLI crate depends on the `OpcProvider` trait, never on `OpcDaClient` directly in its core logic.
*   Tests use `MockOpcProvider` (via `test-support` feature).
*   `friendly_com_hint()` is called by the CLI to enrich error messages displayed in the TUI status bar.

---

## 4. Required Test Coverage

### Unit Tests (in `helpers.rs`)

- [x] `friendly_com_hint` returns correct hint for known HRESULT codes.
- [x] `friendly_com_hint` returns `None` for unknown errors.
- [x] `format_hresult` returns `0xHHHHHHHH: <hint>` for known codes.
- [x] `format_hresult` returns `0xHHHHHHHH` for unknown codes.
- [x] `filetime_to_string` returns `"N/A"` for zero FILETIME.
- [x] `filetime_to_string` produces valid date string for non-zero FILETIME.
- [x] `StringIterator` skips null PWSTR entries without producing `E_POINTER`.
- [x] `StringIterator` handles empty enumeration (0 items).
- [x] `opc_value_to_variant` correctly converts `Int` variant.
- [x] `variant_to_string` roundtrips through `VT_I4` and `VT_R4`.
- [x] `variant_to_string` handles `VT_EMPTY` and `VT_NULL`.
- [x] `variant_to_string` handles `VT_CY` (currency).
- [x] `variant_to_string` handles `VT_ERROR` with known and unknown HRESULTs.
- [x] `variant_to_string` returns `(VT ...)` for unknown variant types.
- [x] `quality_to_string` returns `"Good"` for `0xC0`.
- [x] `quality_to_string` returns `"Bad"` for `0x00`.
- [x] `quality_to_string` returns `"Uncertain"` for `0x40`.
- [x] `quality_to_string` returns `"Unknown(…)"` for unrecognized bitmask.

### ComWorker Thread Dispatch Unit Tests (in `com_worker.rs`)

- [x] `test_worker_starts_and_stops` — worker thread start & stop.
- [x] `test_worker_list_servers` — server listing dispatch.
- [x] `test_worker_write_tag_value` — write path dispatch & `WriteResult`.
- [x] `test_connection_cache_reuse` — server connection pooling across requests (`connect_count == 1`).
- [x] `test_stale_connection_eviction` — auto-eviction & transparent reconnect on COM/RPC error (`connect_count == 2`).
- [x] `test_worker_panic_propagation` — worker thread panic safety & error propagation to caller.
- [x] `test_drop_during_active_request` — graceful worker thread shutdown.
- [x] `test_worker_init_failure` — initialization error handling.

### Mock-Backend Integration Tests (in `opc_da.rs`)

- [x] `test_mock_list_servers` returns expected mock server enumeration.
- [x] `test_mock_read_tags_happy` — all tags valid, correct values returned.
- [x] `test_mock_read_tags_partial_reject` — 1 of 3 tags rejected, returns length-3 array with error sentinel.
- [x] `test_mock_read_tags_all_reject` — all tags rejected, returns `Ok` with all error sentinels.
- [x] `test_mock_write_tag_happy` — tag valid, `success=true`.
- [x] `test_mock_write_tag_add_fail` — tag rejected, `success=false`, group cleaned up.
- [x] `test_browse_tags_flat_server` — flat namespace browse collects tags correctly.
- [x] `test_browse_tags_opc_flat_success` — OPC_FLAT fast path collects all tags.
- [x] `test_browse_tags_opc_flat_error_fallback` — OPC_FLAT error falls back to recursive.
- [x] `test_browse_tags_opc_flat_empty_fallback` — OPC_FLAT empty falls back to recursive.
- [x] `test_browse_tags_hierarchical_recursive` — recursive browse traverses hierarchy.
- [x] `test_browse_tags_max_tags_limit` — max_tags cap works on all paths.

### Mock-Based Tests (in `opc-cli`)

- [x] `MockOpcProvider` returns expected server list.
- [x] `MockOpcProvider` returns expected browse results.
- [x] `MockOpcProvider` returns expected tag values.
- [x] `MockOpcProvider` simulates error conditions for UI error handling.

### Doc Tests

- [x] `friendly_com_hint` — runnable doctest in `helpers.rs`.
- [x] `ComGuard` — internal-only ignored doctest in `com_guard.rs`.

### Integration / Manual Tests

- [ ] `list_servers("localhost")` returns non-empty list on a machine with OPC servers installed.
- [ ] `browse_tags` correctly discovers tags on a flat-namespace server.
- [ ] `browse_tags` correctly discovers tags on a hierarchical-namespace server.
- [ ] `browse_tags` respects `max_tags` cap.
- [ ] `browse_tags` populates `tags_sink` incrementally (observable via progress counter).
- [ ] `read_tag_values` returns correct value/quality/timestamp for known tags.
- [ ] `read_tag_values` gracefully handles tags that fail `add_items`.
- [ ] `write_tag_value` returns success for a valid write to a simulation tag.
- [ ] `write_tag_value` returns error (with hint) when writing to a read-only tag.
- [ ] `opc_value_to_variant` correctly converts all `OpcValue` variants.

