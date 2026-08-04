//! COM 编排：CLSID/ProgID 常量 + build_registration + run（复制库 bin 模板）。
//!
//! 本模块所有 pub 接口（`run_register`/`run_unregister`/`run_server`）尚未被 main.rs
//! 消费（main.rs 仍为 Task 1 skeleton）。后续 task 接线后移除 `#[allow(dead_code)]`。

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
#[allow(dead_code)] // main.rs 接线前仅测试消费。
pub const CLSID_OPC_DA_SIM: GUID = GUID::from_u128(0xb1c2_d3e4_f5a6_0718_293a_4b5c_5d6e_7f80);
#[allow(dead_code)] // main.rs 接线前仅 build_registration 消费。
const PROG_ID: &str = "opc-da-rs.Sim.1";
#[allow(dead_code)] // 同上。
const VIPROG_ID: &str = "opc-da-rs.Sim";
#[allow(dead_code)] // 同上。
const DESCRIPTION: &str = "opc-da-rs OPC DA Simulation Server";

#[allow(dead_code)] // 同上。
const CATIDS: [GUID; 3] = [
    CATID_OPCDAServer10::IID,
    CATID_OPCDAServer20::IID,
    CATID_OPCDAServer30::IID,
];

#[allow(dead_code)] // main.rs 接线前仅测试消费。
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

#[allow(dead_code)] // 仅 run_server 消费；后者当前未接线。
fn read_count() -> usize {
    std::env::var("OPC_DA_SIM_COUNT")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| (1..=100_000).contains(&n))
        .unwrap_or(100)
}

#[allow(dead_code)] // main.rs 接线前无 caller。
pub fn run_register() -> Result<()> {
    let exe_path = std::env::current_exe()?;
    let reg = build_registration(&exe_path);
    register(&reg)?;
    eprintln!("opc-da-server-sim: registered (ProgID={})", reg.prog_id);
    Ok(())
}

#[allow(dead_code)] // 同上。
pub fn run_unregister() -> Result<()> {
    let exe_path = std::env::current_exe()?;
    let reg = build_registration(&exe_path);
    unregister(&reg)?;
    eprintln!("opc-da-server-sim: unregistered (ProgID={})", reg.prog_id);
    Ok(())
}

#[allow(dead_code)] // 同上。
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
        eprintln!(
            "opc-da-server-sim: serving (ProgID={}, {} tags, Ctrl+C 退出)",
            PROG_ID,
            8 * count + 1
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
