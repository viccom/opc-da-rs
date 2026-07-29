#![allow(unused_imports)]
/// OPC DA client trait definitions.
///
/// Trait definitions for interacting with OPC DA servers, organized by functionality
/// and OPC DA version compatibility.
///
/// > **实现状态（2026-07-28 审计）**：trait 定义完整覆盖规范，但**生产路径只用到一小部分**。
/// > 经 `OpcProvider` 对外暴露、被 `ComWorker` 实际调用的仅：`ServerTrait`（AddGroup/
/// > RemoveGroup/GetStatus）、`BrowseServerAddressSpaceTrait`、`ItemMgtTrait`、`SyncIoTrait`，
/// > 以及 `ClientTrait`（服务器枚举）。`CommonTrait`/`ItemPropertiesTrait`/`GroupStateMgtTrait`
/// > 已在 `ComServer`/`ComGroup` 实现，部分正逐步暴露（见 `ROADMAP.md`）。
/// > `AsyncIo*Trait` 与订阅回调（`IOPCDataCallback`）尚未接线。DA 3.0 trait
/// > （`BrowseTrait`/`ItemIoTrait`/`GroupStateMgt2Trait`/`SyncIo2Trait`/`AsyncIo3Trait`/
/// > `ItemDeadbandMgtTrait`/`ItemSamplingMgtTrait`）仅在未接线的 `v3` 模块有示例实现。
///
/// # Trait 清单
///
/// Version independent: `CommonTrait`, `ConnectionPointContainerTrait`, `DataObjectTrait`
///
/// DA 1.0: `AsyncIoTrait`, `SyncIoTrait`
/// DA 2.0: `AsyncIo2Trait`, `SyncIo2Trait`, `BrowseServerAddressSpaceTrait`
/// DA 3.0: `AsyncIo3Trait`, `BrowseTrait`, `ItemDeadbandMgtTrait`, `ItemIoTrait`,
///         `ItemSamplingMgtTrait`, `GroupStateMgt2Trait`
///
/// # Types
///
/// - `GroupHandle`: type-safe handle for OPC groups
/// - `ItemHandle`: type-safe handle for OPC items
mod async_io;
mod async_io2;
mod async_io3;
mod browse;
mod browse_server_address_space;
mod client;
mod common;
mod connection_point_container;
mod data_object;
mod group_state_mgt;
mod group_state_mgt2;
mod item_deadband_mgt;
mod item_io;
mod item_mgt;
mod item_properties;
mod item_sampling_mgt;
mod public_group_state_mgt;
mod server;
mod server_public_groups;
mod sync_io;
mod sync_io2;

pub use async_io::*;
pub use async_io2::*;
pub use async_io3::*;
pub use browse::*;
pub use browse_server_address_space::*;
pub use client::*;
pub use common::*;
pub use connection_point_container::*;
pub use data_object::*;
pub use group_state_mgt::*;
pub use group_state_mgt2::*;
pub use item_deadband_mgt::*;
pub use item_io::*;
pub use item_mgt::*;
pub use item_properties::*;
pub use item_sampling_mgt::*;
pub use public_group_state_mgt::*;
pub use server::*;
pub use server_public_groups::*;
pub use sync_io::*;
pub use sync_io2::*;

pub use crate::opc_da::typedefs::{GroupHandle, ItemHandle};
