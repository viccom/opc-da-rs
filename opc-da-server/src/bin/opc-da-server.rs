//! `opc-da-server.exe` —— OPC DA LocalServer EXE 入口。
//!
//! 命令行：
//! - `/RegServer`   写 HKCR 注册项（CLSID/ProgID/CATID/AppID），需管理员，注册后退出。
//! - `/UnregServer` 清注册项（阶段 0 占位）。
//! - 无参          启动服务循环：注册类对象 + 阻塞（被 SCM 经 `-Embedding` 拉起时）。

use std::time::Duration;

use opc_da_client::bindings::da::CATID_OPCDAServer20;
use opc_da_server::class_factory::{CLSID_OPC_DA_SERVER, Factory};
use opc_da_server::registry::{ServerRegistration, register, unregister};
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, CoIncrementMTAUsage, CoRegisterClassObject, CoResumeClassObjects,
    IClassFactory, REGCLS_MULTIPLEUSE, REGCLS_SUSPENDED,
};
use windows::core::{Interface, Result};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a.eq_ignore_ascii_case("/RegServer")) {
        return run_register();
    }
    if args.iter().any(|a| a.eq_ignore_ascii_case("/UnregServer")) {
        return run_unregister();
    }
    run_server()
}

/// /RegServer：写注册表后退出。
fn run_register() -> Result<()> {
    let exe_path = std::env::current_exe()?;
    let catids = [CATID_OPCDAServer20::IID];
    let reg = ServerRegistration {
        clsid: CLSID_OPC_DA_SERVER,
        prog_id: "opc-da-rs.Server.1",
        version_independent_prog_id: "opc-da-rs.Server",
        exe_path: &exe_path,
        catids: &catids,
        // 阶段 0 简化：AppID 复用 CLSID（阶段 3 DCOM 再独立分配）。
        app_id: CLSID_OPC_DA_SERVER,
    };
    register(&reg)?;
    eprintln!(
        "opc-da-server: registered (ProgID={}, CLSID={{...}})",
        reg.prog_id
    );
    Ok(())
}

/// /UnregServer：清注册表（阶段 0 占位）。
fn run_unregister() -> Result<()> {
    let exe_path = std::env::current_exe()?;
    let catids = [CATID_OPCDAServer20::IID];
    let reg = ServerRegistration {
        clsid: CLSID_OPC_DA_SERVER,
        prog_id: "opc-da-rs.Server.1",
        version_independent_prog_id: "opc-da-rs.Server",
        exe_path: &exe_path,
        catids: &catids,
        app_id: CLSID_OPC_DA_SERVER,
    };
    unregister(&reg)?;
    eprintln!(
        "opc-da-server: unregistered (ProgID={}, CLSID={{...}})",
        reg.prog_id
    );
    Ok(())
}

/// 服务循环：注册类对象 + 阻塞。
///
/// 阶段 0 占位——主循环仅 `sleep` 保持进程存活（让 worker 线程服务 `CoCreateInstance`
/// 激活）。后续阶段按 `CoReleaseServerProcess()==0` 或 `IOPCShutdown` 优雅
/// `CoRevokeClassObject` 退出。
fn run_server() -> Result<()> {
    // SAFETY: COM 注册/恢复为标准 EXE server 启动序列。
    unsafe {
        CoIncrementMTAUsage()?;
        let factory: IClassFactory = Factory.into();
        let _cookie = CoRegisterClassObject(
            &CLSID_OPC_DA_SERVER,
            &factory,
            CLSCTX_LOCAL_SERVER,
            REGCLS_MULTIPLEUSE | REGCLS_SUSPENDED,
        )?;
        CoResumeClassObjects()?;
        eprintln!("opc-da-server: serving (Ctrl+C 退出)");
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}
