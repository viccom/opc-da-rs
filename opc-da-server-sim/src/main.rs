//! `opc-da-server-sim` —— 基于 opc-da-server 库的示例 OPC DA Simulation Server。
//!
//! Windows-only（依赖 opc-da-server 的 COM 实现）。非 Windows 由 opc-da-server 的
//! `compile_error!` 直接拒编译（与库一致，无需额外 stub）。

#[cfg(windows)]
mod waveform;

#[cfg(windows)]
mod tags;

#[cfg(windows)]
mod data_source;

fn main() {
    eprintln!("opc-da-server-sim: skeleton (Task 1) — 后续 task 填充 COM 编排");
}
