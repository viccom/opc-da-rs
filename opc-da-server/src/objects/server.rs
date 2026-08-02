//! Server 对象——阶段 0 spike（空壳）。
//!
//! 目的：验证 `#[implement(IOPCServer, IOPCCommon)]` **多接口共存**在 windows-rs
//! 0.61 + 冻结 bindings 下可行。现有 `opc_da_client::subscription` 只验证过**单接口**
//! sink（`#[implement(IOPCDataCallback)]`）；server 对象需同时挂多个 OPC 接口，必须
//! 实测。通过后阶段 0 后续才在此扩展真实实装。
//!
//! 当前所有方法返回 `E_NOTIMPL`——仅验证 QI 到 `IOPCServer` / `IOPCCommon` /
//! `IUnknown` 三个接口都成功（vtable offset 正确）。

// `#[implement]` 展开的 COM 胶水触发若干 pedantic lints；与 `subscription.rs` 同模式 allow。
#![allow(
    clippy::ref_as_ptr,
    clippy::inline_always,
    clippy::undocumented_unsafe_blocks,
    clippy::not_unsafe_ptr_arg_deref
)]

use windows::Win32::Foundation::{E_NOTIMPL, E_OUTOFMEMORY};
use windows::Win32::System::Com::{
    CoTaskMemAlloc, CoTaskMemFree, IConnectionPoint, IConnectionPointContainer,
    IConnectionPointContainer_Impl, IEnumConnectionPoints,
};
use windows::Win32::System::Variant::VARIANT;
use windows::core::{BOOL, GUID, HRESULT, IUnknown, OutRef, PCWSTR, PWSTR, Result, implement};

use opc_da_client::bindings::comn::{IOPCCommon, IOPCCommon_Impl};
use opc_da_client::bindings::da::{
    IOPCItemProperties, IOPCItemProperties_Impl, IOPCServer, IOPCServer_Impl, OPC_STATUS_RUNNING,
    tagOPCENUMSCOPE, tagOPCSERVERSTATUS,
};

use crate::data_source::now_filetime;

/// OPC DA Server COM 对象。
///
/// 实现 client（`opc-da-client` 的 `v2::Server::try_from`）connect 时强制 cast 的 4 接口：
/// `IOPCServer`（`GetStatus` 已实装，其余 nyi）/ `IOPCCommon` / `IConnectionPointContainer`
/// / `IOPCItemProperties`。后两者当前 stub（`E_NOTIMPL`），仅满足 QI——M2/M5/M7 逐步实装。
#[implement(IOPCServer, IOPCCommon, IConnectionPointContainer, IOPCItemProperties)]
pub struct ServerObj;

/// 通用"未实装"错误：spike 阶段所有方法暂返回 `E_NOTIMPL`。
fn nyi<T>() -> Result<T> {
    Err(E_NOTIMPL.into())
}

impl IOPCServer_Impl for ServerObj_Impl {
    fn AddGroup(
        &self,
        _szname: &PCWSTR,
        _bactive: BOOL,
        _dwrequestedupdaterate: u32,
        _hclientgroup: u32,
        _ptimebias: *const i32,
        _ppercentdeadband: *const f32,
        _dwlcid: u32,
        _phservergroup: *mut u32,
        _previsedupdaterate: *mut u32,
        _riid: *const GUID,
        _ppunk: OutRef<'_, IUnknown>,
    ) -> Result<()> {
        nyi()
    }

    fn GetErrorString(&self, _dwerror: HRESULT, _dwlocale: u32) -> Result<PWSTR> {
        nyi()
    }

    fn GetGroupByName(&self, _szname: &PCWSTR, _riid: *const GUID) -> Result<IUnknown> {
        nyi()
    }

    fn GetStatus(&self) -> Result<*mut tagOPCSERVERSTATUS> {
        const VENDOR: &str = "opc-da-server (Rust)";
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
                dwGroupCount: 0,
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

    fn RemoveGroup(&self, _hservergroup: u32, _bforce: BOOL) -> Result<()> {
        nyi()
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
    use windows::core::{IUnknown, Interface};

    /// spike：验证多接口 `#[implement(IOPCServer, IOPCCommon)]` 共存——QI 到三个
    /// 接口都成功，证明 windows-rs 0.61 + 冻结 bindings 的 vtable offset 正确。
    /// 这是阶段 0 最高风险点的前置验证；失败则需调整 server 对象架构。
    #[test]
    fn multi_interface_qi_succeeds() {
        let obj: IUnknown = ServerObj.into();
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
}
