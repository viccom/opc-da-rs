//! `IClassFactory` —— COM 类工厂，让 SCM / `CoCreateInstance` 能实例化 Server 对象。

use std::sync::Arc;

use windows::Win32::Foundation::CLASS_E_NOAGGREGATION;
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};
use windows::core::{BOOL, Error, GUID, IUnknown, Interface, Ref, Result, implement};

use crate::data_source::DataSource;
use crate::objects::ServerObj;

/// COM 类工厂——为 `CoCreateInstance` 提供 `ServerObj` 实例。
///
/// 持 `data_source`（bin 启动时注入；`CreateInstance` 用它构造每个 `ServerObj`）。
/// 默认 `SimDataSource`；env `OPC_DA_DATASOURCE=generated` 时为 `GeneratedDataSource`。
#[implement(IClassFactory)]
pub struct Factory {
    data_source: Arc<dyn DataSource>,
}

impl Factory {
    /// 新建 Factory（注入数据源）。bin `run_server` 调。
    pub fn new(data_source: Arc<dyn DataSource>) -> Self {
        Self { data_source }
    }

    /// 默认 Factory（SimDataSource）。单元测试用。
    #[cfg(test)]
    pub(crate) fn default_sim() -> Self {
        Self {
            data_source: Arc::new(crate::data_source::SimDataSource::new()),
        }
    }
}

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<'_, IUnknown>,
        riid: *const GUID,
        object: *mut *mut core::ffi::c_void,
    ) -> Result<()> {
        // 不支持 aggregation——规范返回 CLASS_E_NOAGGREGATION（COM 约定，不 panic）。
        if !outer.is_null() {
            return Err(Error::from(CLASS_E_NOAGGREGATION));
        }
        let unknown: IUnknown = ServerObj::with_data_source(self.data_source.clone()).into();
        // SAFETY: `riid`（COM 运行时提供）为有效 GUID；`object` 为调用方提供的 out 指针。
        // query 成功则写入请求接口指针，失败则不改 `*object`（COM QueryInterface 语义）。
        unsafe { unknown.query(riid, object).ok() }
    }

    fn LockServer(&self, lock: BOOL) -> Result<()> {
        // TODO(后续阶段): 实装 LockServer——调 CoAddRefServerProcess/CoReleaseServerProcess
        // 维持进程存活（配合 run_server 的 CoReleaseServerProcess()==0 优雅退出）。当前 no-op。
        let _ = lock;
        Ok(())
    }
}

/// server COM CLSID（`/RegServer` 写 HKCR 用；e2e/压测经此 CLSID 的 ProgID 连接）。
#[allow(dead_code)] // 生产路径经 bin（不同 crate）按 ProgID 连接；本 crate 内仅测试直接用。
pub const CLSID_OPC_DA_SERVER: GUID = GUID::from_u128(0x9a7b_3c2d_4e5f_6789_abcd_ef01_2345_6789);

#[cfg(test)]
mod tests {
    use super::*;
    use opc_da_client::bindings::comn::IOPCCommon;
    use opc_da_client::bindings::da::{IOPCServer, OPC_STATUS_RUNNING};
    use windows::Win32::System::Com::{
        CLSCTX_LOCAL_SERVER, CoCreateInstance, CoIncrementMTAUsage, CoRegisterClassObject,
        CoResumeClassObjects, CoRevokeClassObject, CoTaskMemFree, REGCLS_MULTIPLEUSE,
        REGCLS_SUSPENDED,
    };

    /// 自激活测试：同进程 `CoRegisterClassObject` + `CoCreateInstance` 命中，
    /// 验证 IClassFactory + ServerObj + COM 激活整条链。QI 到 IOPCServer / IOPCCommon。
    #[test]
    fn self_activate_via_coregister() {
        unsafe {
            CoIncrementMTAUsage().expect("CoIncrementMTAUsage");

            let factory: IClassFactory = Factory::default_sim().into();
            let cookie = CoRegisterClassObject(
                &CLSID_OPC_DA_SERVER,
                &factory,
                CLSCTX_LOCAL_SERVER,
                REGCLS_MULTIPLEUSE | REGCLS_SUSPENDED,
            )
            .expect("CoRegisterClassObject");
            CoResumeClassObjects().expect("CoResumeClassObjects");

            // 同进程 CoCreateInstance 应命中上面注册的工厂。
            let server: IOPCServer =
                CoCreateInstance(&CLSID_OPC_DA_SERVER, None, CLSCTX_LOCAL_SERVER)
                    .expect("CoCreateInstance");
            assert!(server.cast::<IOPCCommon>().is_ok(), "QI IOPCCommon 失败");

            // 退出验证：GetStatus 返回有效结构（state / group_count / vendor）。
            // SAFETY: GetStatus（unsafe）+ 解引用返回结构 + CoTaskMemFree 均在 self_activate
            // 外层 unsafe 内。返回的 CoTaskMem 结构由调用方（client）释放。
            let status_ptr = server.GetStatus().expect("GetStatus");
            let status = &*status_ptr;
            assert_eq!(status.dwServerState, OPC_STATUS_RUNNING, "server state");
            assert_eq!(status.dwGroupCount, 0, "group count");
            assert!(!status.szVendorInfo.is_null(), "vendor 非空");
            CoTaskMemFree(Some(status_ptr as *const _));

            CoRevokeClassObject(cookie).expect("CoRevokeClassObject");
        }
    }

    /// aggregation 请求（outer 非 null）返回 CLASS_E_NOAGGREGATION（不 panic）。
    #[test]
    fn create_instance_rejects_aggregation() {
        unsafe {
            CoIncrementMTAUsage().expect("CoIncrementMTAUsage");
            let factory: IClassFactory = Factory::default_sim().into();
            // 任意 COM 对象作 outer（聚合控制 unknown）；outer 非 null 即拒。
            let outer: IUnknown = Factory::default_sim().into();
            // SAFETY: factory 同进程 implement；outer 有效。windows-rs 的 CreateInstance 是泛型
            // 便利封装（内部 CreateInstance(outer, &T::IID, out)），outer 非 null 时返
            // CLASS_E_NOAGGREGATION（不 panic、不写出指针）。
            let err: windows::core::Result<IUnknown> = factory.CreateInstance(&outer);
            assert_eq!(
                err.expect_err("aggregation 应返错误").code(),
                CLASS_E_NOAGGREGATION
            );
        }
    }
}
