//! 订阅推送数据函数（规模化方案 §4 P0）。
//!
//! 旧 per-group `thread::spawn`（`spawn`/`publisher_loop`）已废，统一调度见 `scheduler.rs`。
//! 本模块保留推送的纯数据函数：[`enumerate_sinks`]（取 `data_cp` 的 `IOPCDataCallback`
//! sink 快照）+ [`push_data_change`]（打包 5 数组 + 遍历 sink 调 `OnDataChange`）。
//! 由 `scheduler.rs` 的 worker 线程与 `group.rs` 的 `Refresh2` 复用。

use windows::Win32::Foundation::{FILETIME, S_OK};
use windows::Win32::System::Com::{CONNECTDATA, IConnectionPoint};
use windows::Win32::System::Variant::VARIANT;
use windows::core::{HRESULT, Interface};

use opc_da_client::bindings::da::IOPCDataCallback;

use crate::data_source::DataSource;

/// 枚举 `data_cp` 当前所有 sink（`IOPCDataCallback`）：`EnumConnections` + `Next` → pUnk cast。
pub fn enumerate_sinks(cp: &IConnectionPoint) -> Vec<IOPCDataCallback> {
    let mut sinks = Vec::new();
    // SAFETY: cp 为 IConnectionPoint 接口；EnumConnections 返回快照枚举器。
    let Ok(en) = (unsafe { cp.EnumConnections() }) else {
        return sinks;
    };
    let mut buf: Vec<CONNECTDATA> = vec![CONNECTDATA::default(); 64];
    let mut fetched = 0u32;
    // SAFETY: en 枚举器接口；Next 写入 buf（容量 64）。
    let _ = unsafe { en.Next(&mut buf, &raw mut fetched) };
    for mut cd in buf.into_iter().take(fetched as usize) {
        // pUnk: ManuallyDrop<Option<IUnknown>>。take 取出 owned Option<IUnknown>，
        // cd drop 时 pUnk 已空（ManuallyDrop no-op），不 double-free。
        // SAFETY: take 后 cd.pUnk 空；cd owned（into_iter 消费）。
        let unk_opt: Option<windows::core::IUnknown> =
            unsafe { std::mem::ManuallyDrop::take(&mut cd.pUnk) };
        if let Some(unk) = unk_opt {
            // cast = QI + AddRef，新 IOPCDataCallback ref；unk drop Release EnumConnections 给的 ref。
            if let Ok(cb) = unk.cast::<IOPCDataCallback>() {
                sinks.push(cb);
            }
        }
    }
    sinks
}

/// 打包 frames 为 `OnDataChange` 5 数组 + 遍历 sink 推送。
///
/// `trans_id`：周期推送传 `0`（非事务）；`Refresh2` 传 client 的 `dwTransactionID`（client
/// 据此区分"主动刷新"回调与周期推送）。
pub fn push_data_change(
    sinks: &[IOPCDataCallback],
    h_group: u32,
    frames: &[(u32, String)],
    data_source: &dyn DataSource,
    trans_id: u32,
) {
    let n = frames.len();
    let mut hclients: Vec<u32> = Vec::with_capacity(n);
    let mut values: Vec<VARIANT> = Vec::with_capacity(n);
    let mut qualities: Vec<u16> = Vec::with_capacity(n);
    let mut timestamps: Vec<FILETIME> = Vec::with_capacity(n);
    let errors: Vec<HRESULT> = vec![S_OK; n];
    for (hc, id) in frames {
        let (v, q, ts) = data_source.read(id);
        hclients.push(*hc);
        values.push(v);
        qualities.push(q);
        timestamps.push(ts);
    }
    let count = u32::try_from(n).unwrap_or(u32::MAX);
    for sink in sinks {
        // SAFETY: sink 是 IOPCDataCallback 接口（MTA 下跨线程/跨进程可调）；数组指针在本函数
        // 栈存活期间有效（OnDataChange 同步返回前不释放）。
        let _ = unsafe {
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
    }
}
