//! OPC DA server COM 对象（Server / Group / ConnectionPoint / 调度器）。
//!
//! - `ConnectionPoint<T>`：通用连接点（Group/Server 订阅推送的 sink 表）。
//! - `GroupObj`：`IOPCItemMgt` / `IOPCGroupStateMgt` / `IOPCSyncIO` / `IOPCAsyncIO2`（Refresh2）
//!   / `IConnectionPointContainer`。
//! - `ServerObj`：`IOPCServer` / `IOPCCommon` / `IConnectionPointContainer` / `IOPCItemProperties`
//!   / `IOPCBrowseServerAddressSpace`（flat + hierarchical）。
//! - `scheduler`：统一推送调度（时间轮 + worker 池，替 per-group 线程）。
//! - `publisher`：推送纯数据函数（`enumerate_sinks` + `push_data_change`）。
//! - `browse`：`IEnumString`（browse item id 枚举器）。

mod browse;
pub(crate) mod connection_point;
mod group;
mod publisher;
pub mod scheduler;
mod server;

pub use browse::StringEnum;
pub use connection_point::ConnectionPoint;
pub use group::GroupObj;
pub use group::pwstr_to_string;
pub use server::ServerObj;
