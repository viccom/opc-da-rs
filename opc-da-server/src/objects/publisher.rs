//! 订阅推送引擎——周期读 DataSource + 遍历 `data_cp` 的 sink 调
//! `IOPCDataCallback::OnDataChange`（设计 §10）。
//!
//! publisher 是每个 group 的后台 `std::thread`（由 `GroupObj::new` 启动），按 `update_rate`
//! 周期读取，把数据经连接点反向回调给已 `Advise` 的客户端 sink。无 sink 时跳过读取（省 CPU）。
//! 线程 daemon（进程退出停）；MTA 下跨线程/跨进程调 sink。

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::{FILETIME, S_OK};
use windows::Win32::System::Com::{CONNECTDATA, CoIncrementMTAUsage, IConnectionPoint};
use windows::Win32::System::Variant::VARIANT;
use windows::core::{HRESULT, Interface};

use opc_da_client::bindings::da::IOPCDataCallback;

use crate::data_source::DataSource;
use crate::objects::group::GroupInner;

/// 取锁；mutex poison 时返回 guard（不 panic）。同 group.rs/server.rs 模式。
fn locked<T>(lock: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    lock.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// COM 接口指针的 `Send` wrapper。
///
/// `IConnectionPoint`（含 raw `NonNull<c_void>`）默认非 `Send`（windows-rs 保守，因 STA 线程
/// 亲和性）。本 server free-threaded（MTA），`CoIncrementMTAUsage` 让 publisher 线程加入 MTA，
/// 接口指针可跨线程传递与调用。
#[allow(clippy::non_send_fields_in_send_ty)] // IConnectionPoint 含 raw ptr；MTA 下跨线程安全（见下 unsafe impl SAFETY）
struct SendCp(IConnectionPoint);
// SAFETY: publisher 线程经 CoIncrementMTAUsage 加入 MTA；COM 接口指针在 MTA 下可跨线程传递/调用。
unsafe impl Send for SendCp {}

/// 启动 publisher 线程：周期（`update_rate`）读 DataSource + 遍历 `data_cp` 的 sink 调 OnDataChange。
///
/// 线程 daemon（进程退出停）；无 sink 时跳过读取。每个 group 一个线程（`GroupObj::new` 启动）。
pub fn spawn(
    inner: Arc<Mutex<GroupInner>>,
    data_source: Arc<dyn DataSource>,
    data_cp: IConnectionPoint,
    update_rate: u32,
) {
    let cp = SendCp(data_cp);
    thread::spawn(move || publisher_loop(inner, data_source, cp, update_rate));
}

/// publisher 主循环（独立线程）。MTA + 周期 sleep + 取 sink 快照 + 读 + 推送。
#[allow(clippy::needless_pass_by_value)] // 参数由 thread closure move 传入，需 owned
fn publisher_loop(
    inner: Arc<Mutex<GroupInner>>,
    data_source: Arc<dyn DataSource>,
    data_cp: SendCp,
    update_rate: u32,
) {
    // 线程进 MTA（跨线程/跨进程 COM 调用 sink 所需）。
    // SAFETY: CoIncrementMTAUsage 幂等，让线程加入 MTA；返回的 handle 忽略（线程生命周期内常驻）。
    let _ = unsafe { CoIncrementMTAUsage() };
    let rate = Duration::from_millis(u64::from(update_rate.max(1)));
    loop {
        thread::sleep(rate);
        // 无 sink 跳过（省 read）。
        let sinks = enumerate_sinks(&data_cp.0);
        if sinks.is_empty() {
            continue;
        }
        // 锁内取 items 快照（h_client, item_id）+ h_client_group；锁外读+推送（避免长持锁）。
        let (h_group, frames) = {
            let g = locked(&inner);
            g.snapshot_for_publish()
        };
        if frames.is_empty() {
            continue;
        }
        push_data_change(&sinks, h_group, &frames, &*data_source);
    }
}

/// 枚举 `data_cp` 当前所有 sink（`IOPCDataCallback`）：`EnumConnections` + `Next` → pUnk cast。
fn enumerate_sinks(cp: &IConnectionPoint) -> Vec<IOPCDataCallback> {
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
fn push_data_change(
    sinks: &[IOPCDataCallback],
    h_group: u32,
    frames: &[(u32, String)],
    data_source: &dyn DataSource,
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
                0, // dwTransID（subscription 推送，非 async 事务）
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
