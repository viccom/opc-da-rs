//! # opc-da-server
//!
//! OPC DA **Custom Server** 库——实现 OPC DA 服务端 COM 接口，作为 out-of-process
//! COM server（LocalServer EXE）被客户端连接、暴露数据。
//!
//! 与 [`opc_da_client`]（消费 COM 接口）方向相反：本 crate **实现** OPC DA 接口
//!（`IOPCServer` / `IOPCItemMgt` / `IOPCSyncIO` / …）+ `IClassFactory` + EXE 生命周期
//! + 反向回调（`IOPCDataCallback` / `IOPCShutdown`）+ 注册表注册。
//!
//! 复用 [`opc_da_client`] 的冻结 `bindings`（接口定义）、`com_utils`（COM 内存工具）、
//! `typedefs`（类型）——省掉最危险的 ABI/vtable 手写层。
//!
//! ## 平台
//!
//! **仅 Windows**——OPC DA server 基于 COM/DCOM。非 Windows 编译给出单条
//! `compile_error!`。
//!
//! ## 状态
//!
//! COM 地基 + Server/Group 多接口 + flat/hierarchical 浏览 + 订阅推送（统一调度）+ 注册
//! 已实装；部分 client 用的接口方法仍为 `E_NOTIMPL`（见各处 `TODO(后续阶段)`）。设计参见
//! `docs/superpowers/specs/2026-08-02-opc-da-server-design.md`。

#![allow(unsafe_code)]
// `#[implement]` 宏展开的 COM 胶水（`_Impl`/`_Vtbl`）触发若干 pedantic lints——crate 级统一
// allow（原散布于 class_factory / browse / connection_point / server / group 各模块级，去重上提）。
#![allow(
    clippy::ref_as_ptr,
    clippy::inline_always,
    clippy::undocumented_unsafe_blocks,
    clippy::not_unsafe_ptr_arg_deref
)]

// 非 Windows 目标：单条友好错误，而非一串 unresolved-import。
#[cfg(not(target_os = "windows"))]
compile_error!(
    "opc-da-server requires Windows (COM/DCOM). It cannot be built on non-Windows targets."
);

#[cfg(target_os = "windows")]
pub mod class_factory;
#[cfg(target_os = "windows")]
pub mod data_source;
#[cfg(target_os = "windows")]
pub mod objects;
#[cfg(target_os = "windows")]
pub mod registry;
