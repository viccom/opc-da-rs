//! `opc-da-server.exe` —— OPC DA LocalServer EXE 入口。
//!
//! 命令行：
//! - `/RegServer`   写 HKCR 注册项（CLSID/ProgID/CATID/AppID），需管理员，注册后退出。
//! - `/UnregServer` 清注册项（阶段 0 占位）。
//! - 无参          启动服务循环：注册类对象 + 阻塞（被 SCM 经 `-Embedding` 拉起时）。

use std::time::Duration;

use opc_da_client::bindings::da::{CATID_OPCDAServer10, CATID_OPCDAServer20, CATID_OPCDAServer30};
use opc_da_server::class_factory::{CLSID_OPC_DA_SERVER, Factory};
use opc_da_server::registry::{ServerRegistration, register, unregister};
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, CoIncrementMTAUsage, CoInitializeSecurity, CoRegisterClassObject,
    CoResumeClassObjects, EOAC_NONE, IClassFactory, REGCLS_MULTIPLEUSE, REGCLS_SUSPENDED,
    RPC_C_AUTHN_LEVEL_CONNECT, RPC_C_IMP_LEVEL_IDENTIFY,
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
    let catids = [
        CATID_OPCDAServer10::IID,
        CATID_OPCDAServer20::IID,
        CATID_OPCDAServer30::IID,
    ];
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
    let catids = [
        CATID_OPCDAServer10::IID,
        CATID_OPCDAServer20::IID,
        CATID_OPCDAServer30::IID,
    ];
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
        // DCOM 安全：注册类对象前调 CoInitializeSecurity（全进程一次，COM 初始化后 + 首次
        // 激活前）。CONNECT 认证级 + IDENTIFY 模拟 + EOAC_NONE——标准 OPC DA server 配置，
        // 本机/远程 client 均可连接（本机不受影响，远程 DCOM 需匹配 client 认证级）。
        // SAFETY: CoInitializeSecurity 在 CoInitialize 后 + 任何 COM 激活前调；cauthn=-1
        // 让 COM 选择认证服务。
        CoInitializeSecurity(
            None,
            -1,
            None,
            None,
            RPC_C_AUTHN_LEVEL_CONNECT,
            RPC_C_IMP_LEVEL_IDENTIFY,
            None,
            EOAC_NONE,
            None,
        )?;
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
