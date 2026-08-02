//! OPC DA server COM 对象（Server / Group / ConnectionPoint）。
//!
//! 阶段 0：`ServerObj` 空壳 spike（多接口共存验证）。
//! 阶段 1（进行中）：`ConnectionPoint<T>` 通用连接点（Group/Server 订阅推送的 sink 表）。

mod connection_point;
mod server;

pub use connection_point::ConnectionPoint;
pub use server::ServerObj;
