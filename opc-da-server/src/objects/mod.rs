//! OPC DA server COM 对象（Server / Group / ConnectionPoint）。
//!
//! 阶段 0：仅 `ServerObj` 空壳 spike。后续阶段在此扩展 Group、ConnectionPoint、
//! 各接口方法的真实实装。

mod server;

pub use server::ServerObj;
