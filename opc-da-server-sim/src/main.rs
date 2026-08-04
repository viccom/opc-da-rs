//! `opc-da-server-sim` —— 基于 opc-da-server 库的示例 OPC DA Simulation Server。
//!
//! 命令行：
//! - `/RegServer`   写 HKCR 注册项（需管理员），注册后退出。
//! - `/UnregServer` 清注册项（幂等）。
//! - 无参          启动服务循环。tag 数量由 env `OPC_DA_SIM_COUNT` 控制（默认 100）。
//!
//! Windows-only（依赖 opc-da-server 的 COM 实现）。非 Windows 由库 compile_error! 拒编译。

#[cfg(windows)]
mod data_source;
#[cfg(windows)]
mod runtime;
#[cfg(windows)]
mod tags;
#[cfg(windows)]
mod waveform;

#[cfg(windows)]
fn main() -> windows::core::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a.eq_ignore_ascii_case("/RegServer")) {
        return runtime::run_register();
    }
    if args.iter().any(|a| a.eq_ignore_ascii_case("/UnregServer")) {
        return runtime::run_unregister();
    }
    runtime::run_server()
}

#[cfg(not(windows))]
fn main() {}
