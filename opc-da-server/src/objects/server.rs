//! Server 对象——OPC DA Server COM 对象。
//!
//! 实现 client（`opc-da-client` 的 `v2::Server::try_from`）connect 时强制 cast 的 4 接口：
//! `IOPCServer` / `IOPCCommon` / `IConnectionPointContainer` / `IOPCItemProperties`。
//!
//! `IOPCServer`：`AddGroup` / `RemoveGroup` / `GetStatus`（group_count 接实际计数）已实装；
//! `GetGroupByName` / `CreateGroupEnumerator` / `GetErrorString` 暂 `E_NOTIMPL`（后续）。
//! `IOPCCommon` / `IConnectionPointContainer` / `IOPCItemProperties` 当前 stub（`E_NOTIMPL`），
//! 仅满足 QI——M5/M7 逐步实装。

// `#[implement]` 展开的 COM 胶水触发若干 pedantic lints；与 `subscription.rs` 同模式 allow。
#![allow(
    clippy::ref_as_ptr,
    clippy::inline_always,
    clippy::undocumented_unsafe_blocks,
    clippy::not_unsafe_ptr_arg_deref
)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use windows::Win32::Foundation::{
    E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER, S_OK,
};
use windows::Win32::System::Com::{
    CoTaskMemAlloc, CoTaskMemFree, IConnectionPoint, IConnectionPointContainer,
    IConnectionPointContainer_Impl, IEnumConnectionPoints,
};
use windows::Win32::System::Variant::VARIANT;
use windows::core::{
    BOOL, GUID, HRESULT, IUnknown, Interface, OutRef, PCWSTR, PWSTR, Result, implement,
};

use opc_da_client::bindings::comn::{IOPCCommon, IOPCCommon_Impl, IOPCShutdown};
use opc_da_client::bindings::da::{
    IOPCItemProperties, IOPCItemProperties_Impl, IOPCServer, IOPCServer_Impl, OPC_STATUS_RUNNING,
    tagOPCENUMSCOPE, tagOPCSERVERSTATUS,
};

use crate::data_source::{DataSource, SimDataSource, now_filetime};
use crate::objects::{ConnectionPoint, GroupObj, pwstr_to_string};

/// OPC DA Server COM 对象。
///
/// 持有 group 注册表（`inner`）+ 数据源（`data_source`，`AddGroup` 时克隆给新建 Group）+
/// shutdown 连接点（`shutdown_cp`，client `Advise` `IOPCShutdown` 于此——M5 实装暴露）。
#[implement(IOPCServer, IOPCCommon, IConnectionPointContainer, IOPCItemProperties)]
pub struct ServerObj {
    inner: Mutex<ServerInner>,
    data_source: Arc<dyn DataSource>,
    /// shutdown sink 连接点。后续 `FindConnectionPoint(IOPCShutdown)` 返回它（M5）。
    #[allow(dead_code)]
    shutdown_cp: ConnectionPoint<IOPCShutdown>,
}

impl ServerObj {
    /// 新建 Server（空 group 注册表 + `SimDataSource` + 空 shutdown cp）。
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(ServerInner::new()),
            data_source: Arc::new(SimDataSource::new()),
            shutdown_cp: ConnectionPoint::new(),
        }
    }
}

/// Server 的可变状态（group 注册表 + handle 分配器）。`Mutex` 守护，跨 COM 调用线程。
struct ServerInner {
    /// `hServerGroup -> GroupObj` 的 IUnknown（server 持引用，防 client 释放后过早 drop）。
    groups: HashMap<u32, IUnknown>,
    next_handle: u32,
}

impl ServerInner {
    fn new() -> Self {
        Self {
            groups: HashMap::new(),
            next_handle: 1,
        }
    }

    /// 分配下一个未用的 server group handle（0 永不返回——0 = 无效）。
    fn alloc_handle(&mut self) -> u32 {
        loop {
            let h = self.next_handle;
            self.next_handle = self.next_handle.wrapping_add(1);
            if h != 0 && !self.groups.contains_key(&h) {
                return h;
            }
        }
    }
}

/// 取锁；mutex poison 时返回 guard（不 panic）。poison 表示持锁线程曾 panic；本模块锁内
/// 不执行会 panic 的操作，故继续而非传播 panic（遵循 CLAUDE.md "禁止 panic"）。
fn locked<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 通用"未实装"错误：尚未实装的方法返回 `E_NOTIMPL`。
fn nyi<T>() -> Result<T> {
    Err(E_NOTIMPL.into())
}

impl IOPCServer_Impl for ServerObj_Impl {
    fn AddGroup(
        &self,
        szname: &PCWSTR,
        bactive: BOOL,
        dwrequestedupdaterate: u32,
        hclientgroup: u32,
        ptimebias: *const i32,
        ppercentdeadband: *const f32,
        dwlcid: u32,
        phservergroup: *mut u32,
        previsedupdaterate: *mut u32,
        riid: *const GUID,
        ppunk: OutRef<'_, IUnknown>,
    ) -> Result<()> {
        if phservergroup.is_null() || ppunk.is_null() || riid.is_null() {
            return Err(E_POINTER.into());
        }
        // group state 初值：name/active 取参数；time_bias/percent_deadband 可选（非 null 取值）。
        let name = pwstr_to_string(PWSTR(szname.as_ptr().cast_mut()));
        let active = bactive.as_bool();
        // SAFETY: ptimebias / ppercentdeadband 为 COM in 指针（调用方契约：非 null 时有效）。
        let (time_bias, percent_deadband) = unsafe {
            (
                ptimebias.as_ref().copied().unwrap_or(0),
                ppercentdeadband.as_ref().copied().unwrap_or(0.0),
            )
        };
        let mut raw: *mut core::ffi::c_void = core::ptr::null_mut();
        {
            let mut inner = locked(&self.inner);
            let h = inner.alloc_handle();
            // 创建 GroupObj（DataSource + group state 初值，含 h_server=h）。
            let group_obj = GroupObj::new(
                self.data_source.clone(),
                name,
                active,
                dwrequestedupdaterate,
                time_bias,
                percent_deadband,
                dwlcid,
                hclientgroup,
                h,
            );
            let group_unk: IUnknown = group_obj.into();
            // client 传 riid（通常 IID_IOPCItemMgt）——QI 到该接口返回对应 vtable 指针。
            // SAFETY: riid 调用方提供；query 成功填 raw + AddRef，失败 raw 不改。
            let hr = unsafe { group_unk.query(riid, &raw mut raw) };
            if hr != S_OK || raw.is_null() {
                // QI 失败：group_unk drop（未 insert），不占注册表槽。
                return Err(E_NOINTERFACE.into());
            }
            // 注册 group：server 持 group_unk（client 释放后仍存活到 RemoveGroup）。
            inner.groups.insert(h, group_unk);
            drop(inner);
            // SAFETY: phservergroup 非空（上面校验）；h 为 u32 副本，锁已释放仍有效。
            unsafe { *phservergroup = h };
        }
        // SAFETY: raw 非 null（上面校验）；from_raw 包成 IUnknown，ABI 指向 riid vtable。
        let requested = unsafe { IUnknown::from_raw(raw) };
        // OutRef::write 用 transmute_copy + forget：把 requested 的 ABI（riid 指针）写入
        // ppunk，forget 避免局部 Release——该引用转交给 client。
        ppunk.write(Some(requested))?;
        if !previsedupdaterate.is_null() {
            // SAFETY: previsedupdaterate 非空时为调用方 out 值。
            unsafe { *previsedupdaterate = dwrequestedupdaterate };
        }
        Ok(())
    }

    fn GetErrorString(&self, _dwerror: HRESULT, _dwlocale: u32) -> Result<PWSTR> {
        nyi()
    }

    fn GetGroupByName(&self, _szname: &PCWSTR, _riid: *const GUID) -> Result<IUnknown> {
        nyi()
    }

    fn GetStatus(&self) -> Result<*mut tagOPCSERVERSTATUS> {
        const VENDOR: &str = "opc-da-server (Rust)";
        let group_count = u32::try_from(locked(&self.inner).groups.len()).unwrap_or(u32::MAX);
        // SAFETY: CoTaskMemAlloc 分配的内存按 COM 所有权约定转移给 client（client 经
        // CoTaskMemFree 释放结构本身与 szVendorInfo）。wide string 与结构都在 CoTaskMem 堆。
        unsafe {
            let vendor_wide: Vec<u16> = VENDOR.encode_utf16().chain(std::iter::once(0)).collect();
            let vendor_ptr = CoTaskMemAlloc(vendor_wide.len() * 2).cast::<u16>();
            if vendor_ptr.is_null() {
                return Err(E_OUTOFMEMORY.into());
            }
            std::ptr::copy_nonoverlapping(vendor_wide.as_ptr(), vendor_ptr, vendor_wide.len());

            let status = CoTaskMemAlloc(std::mem::size_of::<tagOPCSERVERSTATUS>())
                .cast::<tagOPCSERVERSTATUS>();
            if status.is_null() {
                CoTaskMemFree(Some(vendor_ptr as *const _));
                return Err(E_OUTOFMEMORY.into());
            }
            // 当前时间（client 解析 FILETIME 需 >= UNIX_EPOCH；零值会被判 before-epoch）。
            let now = now_filetime();
            *status = tagOPCSERVERSTATUS {
                ftStartTime: now,
                ftCurrentTime: now,
                ftLastUpdateTime: now,
                dwServerState: OPC_STATUS_RUNNING,
                dwGroupCount: group_count,
                dwBandWidth: 0,
                wMajorVersion: 0,
                wMinorVersion: 1,
                wBuildNumber: 0,
                wReserved: 0,
                szVendorInfo: PWSTR(vendor_ptr),
            };
            Ok(status)
        }
    }

    fn RemoveGroup(&self, hservergroup: u32, _bforce: BOOL) -> Result<()> {
        if locked(&self.inner).groups.remove(&hservergroup).is_some() {
            Ok(())
        } else {
            Err(E_INVALIDARG.into())
        }
    }

    fn CreateGroupEnumerator(
        &self,
        _dwscope: tagOPCENUMSCOPE,
        _riid: *const GUID,
    ) -> Result<IUnknown> {
        nyi()
    }
}

impl IOPCCommon_Impl for ServerObj_Impl {
    fn SetLocaleID(&self, _dwlcid: u32) -> Result<()> {
        nyi()
    }

    fn GetLocaleID(&self) -> Result<u32> {
        nyi()
    }

    fn QueryAvailableLocaleIDs(&self, _pdwcount: *mut u32, _pdwlcid: *mut *mut u32) -> Result<()> {
        nyi()
    }

    fn GetErrorString(&self, _dwerror: HRESULT) -> Result<PWSTR> {
        nyi()
    }

    fn SetClientName(&self, _szname: &PCWSTR) -> Result<()> {
        nyi()
    }
}

impl IConnectionPointContainer_Impl for ServerObj_Impl {
    fn EnumConnectionPoints(&self) -> Result<IEnumConnectionPoints> {
        nyi()
    }

    fn FindConnectionPoint(&self, _riid: *const GUID) -> Result<IConnectionPoint> {
        nyi()
    }
}

impl IOPCItemProperties_Impl for ServerObj_Impl {
    fn QueryAvailableProperties(
        &self,
        _szitemid: &PCWSTR,
        _pdwcount: *mut u32,
        _pppropertyids: *mut *mut u32,
        _ppdescriptions: *mut *mut PWSTR,
        _ppvtdatatypes: *mut *mut u16,
    ) -> Result<()> {
        nyi()
    }

    fn GetItemProperties(
        &self,
        _szitemid: &PCWSTR,
        _dwcount: u32,
        _pdwpropertyids: *const u32,
        _ppvdata: *mut *mut VARIANT,
        _pperrors: *mut *mut HRESULT,
    ) -> Result<()> {
        nyi()
    }

    fn LookupItemIDs(
        &self,
        _szitemid: &PCWSTR,
        _dwcount: u32,
        _pdwpropertyids: *const u32,
        _ppsznewitemids: *mut *mut PWSTR,
        _pperrors: *mut *mut HRESULT,
    ) -> Result<()> {
        nyi()
    }
}

#[cfg(test)]
mod tests {
    use super::ServerObj;
    use opc_da_client::bindings::comn::IOPCCommon;
    use opc_da_client::bindings::da::{IOPCItemProperties, IOPCServer};
    use windows::Win32::System::Com::IConnectionPointContainer;
    use windows::core::{IUnknown, Interface, PCWSTR};

    /// 验证 4 接口 `#[implement]` 共存——QI 到 client connect 强制 cast 的 4 接口 +
    /// `IUnknown` 都成功。
    #[test]
    fn multi_interface_qi_succeeds() {
        let obj: IUnknown = ServerObj::new().into();
        assert!(obj.cast::<IOPCServer>().is_ok(), "QI IOPCServer 失败");
        assert!(obj.cast::<IOPCCommon>().is_ok(), "QI IOPCCommon 失败");
        assert!(
            obj.cast::<IConnectionPointContainer>().is_ok(),
            "QI IConnectionPointContainer 失败"
        );
        assert!(
            obj.cast::<IOPCItemProperties>().is_ok(),
            "QI IOPCItemProperties 失败"
        );
        assert!(obj.cast::<IUnknown>().is_ok(), "QI IUnknown 失败");
    }

    /// AddGroup/RemoveGroup/GetStatus 联动：add 后 group_count 增，remove 后减。
    #[test]
    fn add_remove_group_updates_count() {
        let server: IOPCServer = ServerObj::new().into();
        // GetStatus 初始 group_count = 0。
        // SAFETY: GetStatus 返回 CoTaskMem 结构，调用方释放。
        let status_ptr = unsafe { server.GetStatus().expect("GetStatus") };
        unsafe {
            assert_eq!((*status_ptr).dwGroupCount, 0);
            windows::Win32::System::Com::CoTaskMemFree(Some(status_ptr.cast()));
        }
        // AddGroup → group_count = 1。
        // SAFETY: AddGroup 同进程 vtable 调用；riid=IOPCItemMgt::IID；out 指针有效。
        let mut hserver = 0u32;
        let mut revised = 0u32;
        let mut group: Option<IUnknown> = None;
        unsafe {
            server
                .AddGroup(
                    PCWSTR::null(),
                    true,
                    500,
                    0,
                    core::ptr::null(),
                    core::ptr::null(),
                    0,
                    &raw mut hserver,
                    &raw mut revised,
                    &<opc_da_client::bindings::da::IOPCItemMgt as Interface>::IID,
                    &raw mut group,
                )
                .expect("AddGroup");
        }
        assert_ne!(hserver, 0, "hServer 非 0");
        assert_eq!(revised, 500, "revised = requested");
        assert!(group.is_some(), "ppunk 非 None");
        let status_ptr = unsafe { server.GetStatus().expect("GetStatus 2") };
        unsafe {
            assert_eq!((*status_ptr).dwGroupCount, 1, "add 后 group_count=1");
            windows::Win32::System::Com::CoTaskMemFree(Some(status_ptr.cast()));
        }
        // RemoveGroup → group_count = 0。
        unsafe { server.RemoveGroup(hserver, false).expect("RemoveGroup") };
        let status_ptr = unsafe { server.GetStatus().expect("GetStatus 3") };
        unsafe {
            assert_eq!((*status_ptr).dwGroupCount, 0, "remove 后 group_count=0");
            windows::Win32::System::Com::CoTaskMemFree(Some(status_ptr.cast()));
        }
    }
}
