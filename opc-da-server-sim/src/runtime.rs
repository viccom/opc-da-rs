//! COM 编排：CLSID/ProgID 常量 + build_registration + run（复制库 bin 模板）。

use std::path::Path;
use std::time::Duration;

use opc_da_client::bindings::da::{CATID_OPCDAServer10, CATID_OPCDAServer20, CATID_OPCDAServer30};
use opc_da_server::class_factory::Factory;
use opc_da_server::data_source::DataSource;
use opc_da_server::objects::scheduler;
use opc_da_server::registry::{ServerRegistration, register, unregister};
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, CoIncrementMTAUsage, CoInitializeSecurity, CoRegisterClassObject,
    CoResumeClassObjects, EOAC_NONE, IClassFactory, REGCLS_MULTIPLEUSE, REGCLS_SUSPENDED,
    RPC_C_AUTHN_LEVEL_CONNECT, RPC_C_IMP_LEVEL_IDENTIFY,
};
use windows::core::{GUID, Interface, Result};

use crate::data_source::SimDataSource;

/// sim 的独立 CLSID（与库 CLSID_OPC_DA_SERVER 0x9a7b_3c2d_... 不同）。
pub const CLSID_OPC_DA_SIM: GUID = GUID::from_u128(0xb1c2_d3e4_f5a6_0718_293a_4b5c_5d6e_7f80);
const PROG_ID: &str = "opc-da-rs.Sim.1";
const VIPROG_ID: &str = "opc-da-rs.Sim";
const DESCRIPTION: &str = "opc-da-rs OPC DA Simulation Server";

const CATIDS: [GUID; 3] = [
    CATID_OPCDAServer10::IID,
    CATID_OPCDAServer20::IID,
    CATID_OPCDAServer30::IID,
];

fn build_registration(exe_path: &Path) -> ServerRegistration<'_> {
    ServerRegistration {
        clsid: CLSID_OPC_DA_SIM,
        prog_id: PROG_ID,
        version_independent_prog_id: VIPROG_ID,
        exe_path,
        catids: &CATIDS,
        app_id: CLSID_OPC_DA_SIM,
        description: DESCRIPTION,
    }
}

fn read_count() -> usize {
    let parsed = read_count_from_config().or_else(read_count_from_env);
    parsed
        .filter(|&n| (1..=100_000).contains(&n))
        .unwrap_or(100)
}

/// exe 同目录 `opc-da-server-sim.ini`：首条 `count = <N>` 行。
/// SCM 启动 exe 时不继承 shell env，env `OPC_DA_SIM_COUNT` 失效；exe 读自己的配置文件不受影响。
fn read_count_from_config() -> Option<usize> {
    let path = std::env::current_exe()
        .ok()?
        .with_file_name("opc-da-server-sim.ini");
    let content = std::fs::read_to_string(&path).ok()?;
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("count")
            && let Some(num_str) = rest.trim_start().strip_prefix('=')
            && let Ok(n) = num_str.trim().parse::<usize>()
        {
            return Some(n);
        }
    }
    None
}

fn read_count_from_env() -> Option<usize> {
    std::env::var("OPC_DA_SIM_COUNT")
        .ok()
        .and_then(|s| s.parse().ok())
}

pub fn run_register() -> Result<()> {
    let exe_path = std::env::current_exe()?;
    let reg = build_registration(&exe_path);
    register(&reg)?;
    eprintln!("opc-da-server-sim: registered (ProgID={})", reg.prog_id);
    Ok(())
}

pub fn run_unregister() -> Result<()> {
    let exe_path = std::env::current_exe()?;
    let reg = build_registration(&exe_path);
    unregister(&reg)?;
    eprintln!("opc-da-server-sim: unregistered (ProgID={})", reg.prog_id);
    Ok(())
}

/// exe 同目录（SCM 启动时 cwd 不可靠——system32；配置 ini / 日志目录均以 exe 相对）。
// 模块 `runtime` 在 main.rs 中私有声明 → `pub` 仅 crate 内可见（clippy redundant_pub_crate）。
pub fn exe_dir() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

pub fn run_server() -> Result<()> {
    let count = read_count();
    // SAFETY: 标准 EXE server 启动序列（复制 opc-da-server/src/bin/opc-da-server.rs:83-119）。
    unsafe {
        CoIncrementMTAUsage()?;
        let workers = std::thread::available_parallelism().map_or(4, std::num::NonZeroUsize::get);
        scheduler::init(workers);
        // SAFETY: CoInitializeSecurity 在 COM 初始化后 + 首次激活前调；cauthn=-1 让 COM 选认证服务。
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
        let ds: std::sync::Arc<dyn DataSource> = std::sync::Arc::new(SimDataSource::new(count));
        let factory: IClassFactory = Factory::new(ds).into();
        let _cookie = CoRegisterClassObject(
            &CLSID_OPC_DA_SIM,
            &factory,
            CLSCTX_LOCAL_SERVER,
            REGCLS_MULTIPLEUSE | REGCLS_SUSPENDED,
        )?;
        CoResumeClassObjects()?;
        tracing::info!(
            "opc-da-server-sim: serving (ProgID={}, {} tags, count={}, Ctrl+C 退出)",
            PROG_ID,
            8 * count + 1,
            count
        );
        tracing::info!(
            "详细日志: {}\\logs\\opc-da-server-sim.log（debug 级，每日滚动）",
            exe_dir().display()
        );
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_fields() {
        let reg = build_registration(Path::new("C:\\test.exe"));
        assert_eq!(reg.prog_id, "opc-da-rs.Sim.1");
        assert_eq!(reg.version_independent_prog_id, "opc-da-rs.Sim");
        assert_eq!(reg.clsid, CLSID_OPC_DA_SIM);
        assert_ne!(
            reg.clsid,
            opc_da_server::class_factory::CLSID_OPC_DA_SERVER,
            "必须与库 CLSID 不同"
        );
        assert_eq!(reg.description, DESCRIPTION);
        assert_eq!(reg.catids.len(), 3);
    }
}
