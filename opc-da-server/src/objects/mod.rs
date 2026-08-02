//! OPC DA server COM 对象（Server / Group / ConnectionPoint）。
//!
//! 阶段 0：`ServerObj` 空壳 spike（2 接口共存验证）。
//! 阶段 1（进行中）：
//! - `ConnectionPoint<T>` 通用连接点（Group/Server 订阅推送的 sink 表）。
//! - `GroupObj` 5 接口骨架（IOPCItemMgt/IOPCGroupStateMgt/IOPCSyncIO/IOPCAsyncIO2/
//!   IConnectionPointContainer）。

mod connection_point;
mod group;
mod server;

pub use connection_point::ConnectionPoint;
pub use group::GroupObj;
pub use server::ServerObj;
