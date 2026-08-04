//! 订阅推送数据函数（规模化方案 §4 P0）。
//!
//! 旧 per-group `thread::spawn`（`spawn`/`publisher_loop`）已废，统一调度见 `scheduler.rs`。
//! 本模块保留推送的纯数据函数：[`enumerate_sinks`]（取 `data_cp` 的 `IOPCDataCallback`
//! sink 快照）+ [`push_data_change`]（打包 5 数组 + 遍历 sink 调 `OnDataChange`）。
//! 由 `scheduler.rs` 的 worker 线程与 `group.rs` 的 `Refresh2` 复用。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use windows::Win32::Foundation::{FILETIME, S_OK};
use windows::Win32::System::Com::{CONNECTDATA, IConnectionPoint};
use windows::Win32::System::Variant::VARIANT;
use windows::core::{HRESULT, Interface};

use opc_da_client::bindings::da::IOPCDataCallback;

use crate::objects::connection_point::global_git;

/// 取锁；mutex poison 时返回 guard（不 panic）。同模块内 `typed_sinks` 用。
fn locked<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 从 `ConnectionPoint` 共享的订阅表（`cookie → GIT cookie`）取 sink proxy 快照。
///
/// 经 GIT `GetInterfaceFromGlobal` 取**可用于当前线程**的 `IOPCDataCallback` proxy——解决
/// STA client（PsOPCClient 等）的 sink 绑定 client STA 线程、MTA worker 直接调报
/// `RPC_E_WRONG_THREAD`（0x8001010E）的问题。GIT 在 Advise 线程注册（sink 正确上下文），
/// 任意线程 GetInterfaceFromGlobal 取回的 proxy 可直接调 `OnDataChange`。
///
/// GIT 不可用时返空（降级——不推送，避免崩溃）。
pub fn typed_sinks(sinks: &Arc<Mutex<HashMap<u32, u32>>>) -> Vec<IOPCDataCallback> {
    let Some(git) = global_git() else {
        return Vec::new();
    };
    // 锁内仅 copy 出 git cookies（u32，廉价），立即释放锁——避免在 N 次 GIT 往返
    //（GetInterfaceFromGlobal 是跨 apartment COM 调用）期间持 `sinks` 锁，阻塞并发
    // Advise/Unadvise（规模化场景 10w sinks × 单次快照可能阻塞上百 ms → client RPC 超时）。
    let cookies: Vec<u32> = locked(sinks).values().copied().collect();
    cookies
        .iter()
        .filter_map(|git_cookie| {
            // SAFETY: git 有效（进程级 GIT）；git_cookie 由 Advise 经同一 GIT 注册；riid 匹配。
            let mut raw: *mut core::ffi::c_void = core::ptr::null_mut();
            unsafe { git.GetInterfaceFromGlobal(*git_cookie, &IOPCDataCallback::IID, &raw mut raw) }
                .ok()
                // SAFETY: GetInterfaceFromGlobal 成功返回 AddRef 过的 proxy；本线程可调。
                .map(|()| unsafe { IOPCDataCallback::from_raw(raw) })
        })
        .collect()
}

/// 枚举 `data_cp` 当前所有 sink（`IOPCDataCallback`）：`EnumConnections` + `Next` → pUnk cast。
///
/// ⚠ 生产推送已改走 [`typed_sinks`]（本函数对 sink 做 QI——STA client 跨线程 sink 报
/// `RPC_E_WRONG_THREAD`）。保留供测试（验证 `EnumConnections` 多轮 Next 不截断）与
/// 潜在的 client 查询路径。
#[allow(dead_code)]
pub fn enumerate_sinks(cp: &IConnectionPoint) -> Vec<IOPCDataCallback> {
    let mut sinks = Vec::new();
    // SAFETY: cp 为 IConnectionPoint 接口；EnumConnections 返回快照枚举器。
    let Ok(en) = (unsafe { cp.EnumConnections() }) else {
        return sinks;
    };
    // COM 枚举器一次 Next 最多返回 buf.len() 个；循环到取不满（到尾）或错误，避免单次 Next
    // 截断（旧实现固定 64 单次 Next，sink > 64 会静默丢失后续订阅者的推送）。
    let mut buf: Vec<CONNECTDATA> = vec![CONNECTDATA::default(); 64];
    loop {
        let mut fetched = 0u32;
        // SAFETY: en 枚举器接口；Next 写入 buf（容量 64）。
        let hr = unsafe { en.Next(&mut buf, &raw mut fetched) };
        // 真正的错误码（非 S_OK/S_FALSE）→ 停（防错误枚举器死循环）。S_FALSE=1 是成功码，不停。
        if hr.is_err() {
            break;
        }
        for cd in buf.iter_mut().take(fetched as usize) {
            // SAFETY: Next 写入 fetched 个有效 CONNECTDATA。pUnk 是 ManuallyDrop<Option<IUnknown>>
            // ——take 取出 owned Option<IUnknown>；buf 复用前已 take 空，下轮 Next 覆盖时不
            // double-free（ManuallyDrop drop 为 no-op）。
            let unk_opt: Option<windows::core::IUnknown> =
                unsafe { std::mem::ManuallyDrop::take(&mut cd.pUnk) };
            if let Some(unk) = unk_opt {
                // cast = QI + AddRef，新 IOPCDataCallback ref；unk drop Release 枚举器给的 ref。
                if let Ok(cb) = unk.cast::<IOPCDataCallback>() {
                    sinks.push(cb);
                }
            }
        }
        // 取不满（到尾，Next 返 S_FALSE）→ 停。
        if (fetched as usize) < buf.len() {
            break;
        }
    }
    sinks
}

/// 遍历 sink 调 `OnReadComplete`（`AsyncIO2::Read` 的结果回调，5 数组由 caller 打包）。
///
/// `trans_id` = client 的 `dwTransactionID`（AsyncIO2::Read 传入）；`h_group` = client 的
/// `hClientGroup`。`hrMasterQuality/hrMasterError` 恒 `S_OK`（per-item 错误在 `pErrors`）。
pub fn push_read_complete(
    sinks: &[IOPCDataCallback],
    h_group: u32,
    trans_id: u32,
    hclients: &[u32],
    values: &[VARIANT],
    qualities: &[u16],
    timestamps: &[FILETIME],
) {
    let n = hclients.len();
    // 防御性断言：5 数组必须等长（COM OnReadComplete 按 count 读各数组）。caller（read_frames
    // / PUSH_BUF）保证等长，debug 构建捕获调用方错误，release 零成本。
    debug_assert_eq!(values.len(), n, "values 长度须 == hclients");
    debug_assert_eq!(qualities.len(), n, "qualities 长度须 == hclients");
    debug_assert_eq!(timestamps.len(), n, "timestamps 长度须 == hclients");
    let errors: Vec<HRESULT> = vec![S_OK; n];
    let count = u32::try_from(n).unwrap_or(u32::MAX);
    for sink in sinks {
        // SAFETY: sink 是 GIT 取的 proxy（当前线程可调）；数组指针在调用期间存活。
        let hr = unsafe {
            sink.OnReadComplete(
                trans_id,
                h_group,
                S_OK, // hrMasterQuality
                S_OK, // hrMasterError
                count,
                hclients.as_ptr(),
                values.as_ptr(),
                qualities.as_ptr(),
                timestamps.as_ptr(),
                errors.as_ptr(),
            )
        };
        // 诊断：回调失败（跨进程 marshaling 断、client 已死等）。
        if let Err(e) = &hr {
            tracing::warn!(
                method = "OnReadComplete",
                h_group,
                trans_id,
                hr = ?e.code(),
                "回调失败"
            );
        }
    }
}

/// 遍历 sink 调 `OnDataChange`（5 数组由 caller 打包；P1 起 push_data_change 不再 read）。
///
/// `trans_id`：周期推送传 `0`（非事务）；`Refresh2` 传 client 的 `dwTransactionID`。
/// caller 负责 read + deadband 过滤（[`crate::objects::scheduler::push_one`]）或全推（`Refresh2`）。
pub fn push_data_change(
    sinks: &[IOPCDataCallback],
    h_group: u32,
    hclients: &[u32],
    values: &[VARIANT],
    qualities: &[u16],
    timestamps: &[FILETIME],
    trans_id: u32,
) {
    let n = hclients.len();
    // 防御性断言：5 数组必须等长（COM OnDataChange 按 count 读各数组）。见 push_read_complete。
    debug_assert_eq!(values.len(), n, "values 长度须 == hclients");
    debug_assert_eq!(qualities.len(), n, "qualities 长度须 == hclients");
    debug_assert_eq!(timestamps.len(), n, "timestamps 长度须 == hclients");
    let errors: Vec<HRESULT> = vec![S_OK; n];
    let count = u32::try_from(n).unwrap_or(u32::MAX);
    for sink in sinks {
        // SAFETY: sink 是 IOPCDataCallback 接口（MTA 下跨线程/跨进程可调）；数组指针在本函数
        // 栈存活期间有效（OnDataChange 同步返回前不释放）。
        let hr = unsafe {
            sink.OnDataChange(
                trans_id, // dwTransID：周期推送=0，Refresh2=client dwTransactionID
                h_group,
                S_OK, // hrMasterQuality
                S_OK, // hrMasterError
                count,
                hclients.as_ptr(),
                values.as_ptr(),
                qualities.as_ptr(),
                timestamps.as_ptr(),
                errors.as_ptr(),
            )
        };
        // 诊断：OnDataChange 返回非 S_OK → 回调失败（跨进程 marshaling 断、client 已死等）。
        // 周期推送 trans_id=0；失败时记 hr 帮助定位"scheduler 推了但 client 没收到"。
        if let Err(e) = &hr {
            tracing::warn!(
                method = "OnDataChange",
                h_group,
                count,
                hr = ?e.code(),
                "回调失败"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::ConnectionPoint;
    use opc_da_client::bindings::da::IOPCDataCallback_Impl;
    use windows::Win32::System::Com::CoIncrementMTAUsage;
    use windows::core::{IUnknown, implement};

    /// 空 sink（仅作 Advise 目标，验证 `enumerate_sinks` 取尽 >buf 容量的 sink 不截断）。
    #[implement(IOPCDataCallback)]
    struct NopSink;

    #[allow(clippy::too_many_arguments)] // COM vtable 签名固定（bindings 生成）
    impl IOPCDataCallback_Impl for NopSink_Impl {
        fn OnDataChange(
            &self,
            _dwtransid: u32,
            _hgroup: u32,
            _hrmasterquality: HRESULT,
            _hrmastererror: HRESULT,
            _dwcount: u32,
            _phclientitems: *const u32,
            _pvvalues: *const VARIANT,
            _pwqualities: *const u16,
            _pfttimestamps: *const FILETIME,
            _perrors: *const HRESULT,
        ) -> windows::core::Result<()> {
            Ok(())
        }

        fn OnReadComplete(
            &self,
            _dwtransid: u32,
            _hgroup: u32,
            _hrmasterquality: HRESULT,
            _hrmastererror: HRESULT,
            _dwcount: u32,
            _phclientitems: *const u32,
            _pvvalues: *const VARIANT,
            _pwqualities: *const u16,
            _pfttimestamps: *const FILETIME,
            _perrors: *const HRESULT,
        ) -> windows::core::Result<()> {
            Ok(())
        }

        fn OnWriteComplete(
            &self,
            _dwtransid: u32,
            _hgroup: u32,
            _hrmastererr: HRESULT,
            _dwcount: u32,
            _pclienthandles: *const u32,
            _perrors: *const HRESULT,
        ) -> windows::core::Result<()> {
            Ok(())
        }

        fn OnCancelComplete(&self, _dwtransid: u32, _hgroup: u32) -> windows::core::Result<()> {
            Ok(())
        }
    }

    /// 验证 `enumerate_sinks` 取尽 >buf(64) 个 sink——多轮 `Next` 迭代路径（首次返 64 + S_OK，
    /// 二次返剩余 + S_FALSE），防 A2 回归（旧单次 Next 固定 64 会静默截断第 65+ 个订阅者）。
    #[test]
    fn enumerate_sinks_returns_all_above_buf_size() {
        unsafe { CoIncrementMTAUsage() }.expect("CoIncrementMTAUsage");
        const N: usize = 70; // > buf.len()(64)，触发第二次 Next
        let cp: IConnectionPoint = ConnectionPoint::<IOPCDataCallback>::new().into();
        for _ in 0..N {
            let sink: IUnknown = NopSink.into();
            // SAFETY: 同进程 implement 对象 Advise（直接走 vtable，不经 SCM）。
            unsafe { cp.Advise(&sink) }.expect("Advise");
        }
        let sinks = enumerate_sinks(&cp);
        assert_eq!(sinks.len(), N, ">64 sink 应全部返回（不截断）");
    }
}
