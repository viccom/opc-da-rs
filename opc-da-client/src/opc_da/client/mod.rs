//! OPC DA client implementation.
mod iterator;
mod traits;

#[doc(hidden)]
pub mod v1; // DA 1.0 server/group — 未接线，保留作历史参考
pub mod v2; // 生产路径使用 v2::Client 作 CoCreateInstance 包装
#[doc(hidden)]
pub mod v3; // DA 3.0 server/group — 未接线，保留作未来 P3-01 接线参考

pub use iterator::*;
pub use traits::*;
