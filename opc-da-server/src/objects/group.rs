//! Group 对象骨架——阶段 1。
//!
//! 验证 `#[implement(IOPCItemMgt, IOPCGroupStateMgt, IOPCSyncIO, IOPCAsyncIO2,
//! IConnectionPointContainer)]` **5 接口共存**在 windows-rs 0.61 + 冻结 bindings
//! 下可行（比阶段 0 `ServerObj` 的 2 接口 spike 更严苛——vtable offset 数量翻倍）。
//! 通过后阶段 1 后续在此扩展真实实装（`IOPCItemMgt::AddItems` / `IOPCSyncIO::Read` /
//! 订阅推送引擎等，对应 RUNBOOK §5 第 4-9 项）。
//!
//! 当前所有方法返回 `E_NOTIMPL`——仅验证 QI 到 5 个接口 + `IUnknown` 都成功。
//! `data_cp` 字段已持有 [`ConnectionPoint<IOPCDataCallback>`]（订阅 sink 表），
//! 供后续 `IOPCServer::AddGroup` 注入 container + publisher 引擎遍历推送。

// `#[implement]` 展开的 COM 胶水（`_Impl`/`_Vtbl`）触发若干 pedantic lints；与
// `class_factory.rs` / `server.rs` 同模式 allow。
#![allow(
    clippy::ref_as_ptr,
    clippy::inline_always,
    clippy::undocumented_unsafe_blocks,
    clippy::not_unsafe_ptr_arg_deref
)]

use windows::Win32::Foundation::E_NOTIMPL;
use windows::Win32::System::Com::{
    IConnectionPoint, IConnectionPointContainer, IConnectionPointContainer_Impl,
    IEnumConnectionPoints,
};
use windows::Win32::System::Variant::VARIANT;
use windows::core::{BOOL, GUID, HRESULT, IUnknown, PCWSTR, PWSTR, Result, implement};

use opc_da_client::bindings::da::{
    IOPCAsyncIO2, IOPCAsyncIO2_Impl, IOPCDataCallback, IOPCGroupStateMgt, IOPCGroupStateMgt_Impl,
    IOPCItemMgt, IOPCItemMgt_Impl, IOPCSyncIO, IOPCSyncIO_Impl, tagOPCDATASOURCE, tagOPCITEMDEF,
    tagOPCITEMRESULT, tagOPCITEMSTATE,
};

use crate::objects::ConnectionPoint;

/// OPC DA Group COM 对象（5 接口骨架）。
///
/// 一个 Group 持有一组 item（client 配置的标签）+ 一个订阅连接点（`data_cp`）。
/// client 对 Group 的操作：`IOPCItemMgt`（增删 item）/ `IOPCSyncIO`（同步读写）/
/// `IOPCAsyncIO2`（异步读写，结果走 `data_cp` 的 `IOPCDataCallback`）/ 状态管理。
#[implement(
    IOPCItemMgt,
    IOPCGroupStateMgt,
    IOPCSyncIO,
    IOPCAsyncIO2,
    IConnectionPointContainer
)]
pub struct GroupObj {
    /// 订阅推送的 sink 连接点（client `Advise` `IOPCDataCallback` 于此）。后续 publisher
    /// 引擎（§10）周期遍历此 cp 的 sink 调 `OnDataChange`。
    #[allow(dead_code)] // 骨架：后续 IOPCServer::AddGroup 注入 container + publisher 遍历
    pub(crate) data_cp: ConnectionPoint<IOPCDataCallback>,
}

impl GroupObj {
    /// 新建 Group 骨架（空 `data_cp`，未关联 container）。
    #[allow(dead_code)] // 骨架：后续 IOPCServer::AddGroup 构造 Group
    pub(crate) fn new() -> Self {
        Self {
            data_cp: ConnectionPoint::new(),
        }
    }
}

/// 通用"未实装"错误：骨架阶段所有方法暂返回 `E_NOTIMPL`。
fn nyi<T>() -> Result<T> {
    Err(E_NOTIMPL.into())
}

impl IOPCItemMgt_Impl for GroupObj_Impl {
    fn AddItems(
        &self,
        _dwcount: u32,
        _pitemarray: *const tagOPCITEMDEF,
        _ppaddresults: *mut *mut tagOPCITEMRESULT,
        _pperrors: *mut *mut HRESULT,
    ) -> Result<()> {
        nyi()
    }

    fn ValidateItems(
        &self,
        _dwcount: u32,
        _pitemarray: *const tagOPCITEMDEF,
        _bblobupdate: BOOL,
        _ppvalidationresults: *mut *mut tagOPCITEMRESULT,
        _pperrors: *mut *mut HRESULT,
    ) -> Result<()> {
        nyi()
    }

    fn RemoveItems(
        &self,
        _dwcount: u32,
        _phserver: *const u32,
        _pperrors: *mut *mut HRESULT,
    ) -> Result<()> {
        nyi()
    }

    fn SetActiveState(
        &self,
        _dwcount: u32,
        _phserver: *const u32,
        _bactive: BOOL,
        _pperrors: *mut *mut HRESULT,
    ) -> Result<()> {
        nyi()
    }

    fn SetClientHandles(
        &self,
        _dwcount: u32,
        _phserver: *const u32,
        _phclient: *const u32,
        _pperrors: *mut *mut HRESULT,
    ) -> Result<()> {
        nyi()
    }

    fn SetDatatypes(
        &self,
        _dwcount: u32,
        _phserver: *const u32,
        _prequesteddatatypes: *const u16,
        _pperrors: *mut *mut HRESULT,
    ) -> Result<()> {
        nyi()
    }

    fn CreateEnumerator(&self, _riid: *const GUID) -> Result<IUnknown> {
        nyi()
    }
}

impl IOPCGroupStateMgt_Impl for GroupObj_Impl {
    fn GetState(
        &self,
        _pupdaterate: *mut u32,
        _pactive: *mut BOOL,
        _ppname: *mut PWSTR,
        _ptimebias: *mut i32,
        _ppercentdeadband: *mut f32,
        _plcid: *mut u32,
        _phclientgroup: *mut u32,
        _phservergroup: *mut u32,
    ) -> Result<()> {
        nyi()
    }

    fn SetState(
        &self,
        _prequestedupdaterate: *const u32,
        _previsedupdaterate: *mut u32,
        _pactive: *const BOOL,
        _ptimebias: *const i32,
        _ppercentdeadband: *const f32,
        _plcid: *const u32,
        _phclientgroup: *const u32,
    ) -> Result<()> {
        nyi()
    }

    fn SetName(&self, _szname: &PCWSTR) -> Result<()> {
        nyi()
    }

    fn CloneGroup(&self, _szname: &PCWSTR, _riid: *const GUID) -> Result<IUnknown> {
        nyi()
    }
}

impl IOPCSyncIO_Impl for GroupObj_Impl {
    fn Read(
        &self,
        _dwsource: tagOPCDATASOURCE,
        _dwcount: u32,
        _phserver: *const u32,
        _ppitemvalues: *mut *mut tagOPCITEMSTATE,
        _pperrors: *mut *mut HRESULT,
    ) -> Result<()> {
        nyi()
    }

    fn Write(
        &self,
        _dwcount: u32,
        _phserver: *const u32,
        _pitemvalues: *const VARIANT,
        _pperrors: *mut *mut HRESULT,
    ) -> Result<()> {
        nyi()
    }
}

impl IOPCAsyncIO2_Impl for GroupObj_Impl {
    fn Read(
        &self,
        _dwcount: u32,
        _phserver: *const u32,
        _dwtransactionid: u32,
        _pdwcancelid: *mut u32,
        _pperrors: *mut *mut HRESULT,
    ) -> Result<()> {
        nyi()
    }

    fn Write(
        &self,
        _dwcount: u32,
        _phserver: *const u32,
        _pitemvalues: *const VARIANT,
        _dwtransactionid: u32,
        _pdwcancelid: *mut u32,
        _pperrors: *mut *mut HRESULT,
    ) -> Result<()> {
        nyi()
    }

    fn Refresh2(&self, _dwsource: tagOPCDATASOURCE, _dwtransactionid: u32) -> Result<u32> {
        nyi()
    }

    fn Cancel2(&self, _dwcancelid: u32) -> Result<()> {
        nyi()
    }

    fn SetEnable(&self, _benable: BOOL) -> Result<()> {
        nyi()
    }

    fn GetEnable(&self) -> Result<BOOL> {
        nyi()
    }
}

impl IConnectionPointContainer_Impl for GroupObj_Impl {
    fn EnumConnectionPoints(&self) -> Result<IEnumConnectionPoints> {
        nyi()
    }

    fn FindConnectionPoint(&self, _riid: *const GUID) -> Result<IConnectionPoint> {
        nyi()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::{IUnknown, Interface};

    /// 骨架核心验证：5 接口 `#[implement]` 共存——QI 到 5 个 OPC/COM 接口 +
    /// `IUnknown` 都成功，证明 windows-rs 0.61 + 冻结 bindings 的 vtable offset 全对。
    /// 这是 Group（接口最多的对象）的前置验证；失败需调整对象架构。
    #[test]
    fn multi_interface_qi_succeeds() {
        let obj: IUnknown = GroupObj::new().into();
        assert!(obj.cast::<IOPCItemMgt>().is_ok(), "QI IOPCItemMgt 失败");
        assert!(
            obj.cast::<IOPCGroupStateMgt>().is_ok(),
            "QI IOPCGroupStateMgt 失败"
        );
        assert!(obj.cast::<IOPCSyncIO>().is_ok(), "QI IOPCSyncIO 失败");
        assert!(obj.cast::<IOPCAsyncIO2>().is_ok(), "QI IOPCAsyncIO2 失败");
        assert!(
            obj.cast::<IConnectionPointContainer>().is_ok(),
            "QI IConnectionPointContainer 失败"
        );
        assert!(obj.cast::<IUnknown>().is_ok(), "QI IUnknown 失败");
    }

    /// `data_cp` 字段已就位且可用（白盒）：初始无订阅。后续 publisher 引擎依赖此字段。
    #[test]
    fn data_cp_initially_empty() {
        let g = GroupObj::new();
        assert_eq!(
            g.data_cp.advise_count(),
            0,
            "新建 Group 的 data_cp 应无订阅"
        );
    }
}
