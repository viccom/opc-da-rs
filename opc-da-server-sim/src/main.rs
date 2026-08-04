//! `opc-da-server-sim` —— 基于 opc-da-server 库的示例 OPC DA Simulation Server。
//!
//! 命令行：
//! - `/RegServer`   写 HKCR 注册项（需管理员），注册后退出。
//! - `/UnregServer` 清注册项（幂等）。
//! - 无参          启动服务循环。tag 数量优先读 exe 同目录 `opc-da-server-sim.ini`
//!   的 `count = <N>` 行；文件缺失时 fallback env `OPC_DA_SIM_COUNT`；再 fallback 默认 100。
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
    // 初始化 tracing subscriber，让库 opc-da-server 内部的 tracing 日志输出到 stderr。
    // SCM 启动 exe 时不继承 shell env，但 RUST_LOG 可由注册项/启动配置注入；缺失则 EnvFilter 默认（WARN）。
    // try_init 返 Err 仅当全局 subscriber 已设（重复初始化），`let _` 吞掉，不 panic。
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init();
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
