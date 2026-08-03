//! 统一推送调度器（规模化方案 §4 P0）。
//!
//! 替代旧 `publisher::spawn` 的 per-group `thread::spawn`：全局一个 [`Scheduler`]，
//! 按 `update_rate` 分桶的时间轮（1 ms tick）+ 固定 N worker 线程池。1 万组 = 1 万个
//! 轻量 [`PublishJob`]（数据），而非 1 万个 OS 线程。
//!
//! ## 线程
//!
//! - **tick 线程**（1）：1 ms 周期检查各桶 `next_tick`，到期则把该桶所有 job 派发到队列。
//! - **worker 线程**（N，默认 = 核数）：MTA 下从队列取 job，调 `publisher::enumerate_sinks`
//!   + `publisher::push_data_change` 推送 `OnDataChange`。
//!
//! 线程 daemon（进程退出强杀）。
//!
//! ## 集成
//!
//! - `bin run_server` 调 [`init`]（启动线程 + 设全局单例）。
//! - `GroupObj::new` 调 [`global`]().register（未 init 时返 `None`，register 跳过——兼容单测）。
//! - `GroupObj::Drop` 调 [`global`]().unregister。
//!
//! ## 竞态（R1）
//!
//! `unregister` 仅从 registry/bucket 移除引用；worker 手里已取出的 `Arc<PublishJob>` 引用
//! 计数保证 job 存活到推送完成（不会访问已释放内存）。

#![allow(
    clippy::non_send_fields_in_send_ty,    // PublishJob 含 IConnectionPoint raw ptr；MTA 下跨线程安全（见 unsafe impl SAFETY）
    clippy::significant_drop_tightening    // 锁作用域已用 block/显式 drop 限定；nursery lint 对 MutexGuard 持有期常误报
)]

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use std::cell::RefCell;
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Com::{CoIncrementMTAUsage, IConnectionPoint};
use windows::Win32::System::Variant::VARIANT;

use crate::data_source::DataSource;
use crate::objects::group::GroupInner;
use crate::objects::publisher;

/// 全局调度器（`bin run_server` 经 [`init`] 设置；进程级单例）。
static SCHEDULER: OnceLock<Arc<Scheduler>> = OnceLock::new();

/// 全局调度器句柄（未 init 返 `None`——单测/未启动环境 [`Scheduler::register`] 跳过）。
#[must_use]
pub(crate) fn global() -> Option<&'static Arc<Scheduler>> {
    SCHEDULER.get()
}

/// 启动全局调度器：创建 + 启动 tick/worker 线程 + 设单例。幂等（重复调 no-op）。
///
/// `workers` = worker 线程数（建议 = 核数）；tick 线程固定 1。单例存于 [`SCHEDULER`]，
/// 线程持各自 `Arc<Scheduler>` clone 存活；调用方无需持句柄。
pub fn init(workers: usize) {
    SCHEDULER.get_or_init(|| {
        let s = Arc::new(Scheduler::new());
        let n = workers.max(1);
        // tick 线程（1）。
        let s_tick = Arc::clone(&s);
        thread::spawn(move || tick_loop(&s_tick));
        // worker 线程（N）。
        for _ in 0..n {
            let s_w = Arc::clone(&s);
            thread::spawn(move || worker_loop(&s_w));
        }
        s
    });
}

/// 取锁；mutex poison 时返回 guard（不 panic）。同 group.rs/publisher.rs 模式。
fn locked<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 单个组的推送任务（数据，非线程）。
///
/// `data_cp` 含 COM raw ptr，默认非 `Send`；server free-threaded（MTA），跨线程安全
///（见下 `unsafe impl`）。
pub(crate) struct PublishJob {
    /// GroupKey = `h_server_group`（`GroupInner` 已有，全局唯一）。
    pub(crate) key: u32,
    pub(crate) inner: Arc<Mutex<GroupInner>>,
    pub(crate) data_source: Arc<dyn DataSource>,
    pub(crate) data_cp: IConnectionPoint,
}

// SAFETY: server free-threaded（MTA）——所有线程经 `CoIncrementMTAUsage` 加入 MTA；
// COM 接口指针（`data_cp`）在 MTA 下可跨线程传递/调用。其余字段（Arc/Mutex/u32/Duration）
// 皆 Send+Sync。
unsafe impl Send for PublishJob {}
// SAFETY: 同上；tick/worker 多线程经 `Arc<PublishJob>` 共享只读访问（字段构造后不变）。
unsafe impl Sync for PublishJob {}

/// 全局推送调度器。
pub(crate) struct Scheduler {
    /// 按 `update_rate` 分桶（rate 离散值有限：10/50/100/250/500/1000ms…）。
    buckets: Mutex<HashMap<Duration, Bucket>>,
    /// job 派发队列（tick → worker）。MPMC：N worker 竞争消费。
    queue: JobQueue,
    /// `key(h_server_group)` → `(job, rate)`，注销时定位 bucket 用。
    registry: Mutex<HashMap<u32, (Arc<PublishJob>, Duration)>>,
}

struct Bucket {
    /// 该 rate 下所有 job（注册 push，注销 retain）。由 `Scheduler::buckets` 锁保护。
    jobs: Vec<Arc<PublishJob>>,
    /// 下次到期时间。由 `Scheduler::buckets` 锁保护。
    next_tick: Instant,
}

/// MPMC job 队列：`Mutex<VecDeque>` + `Condvar`（纯 std，无外部依赖）。
struct JobQueue {
    inner: Mutex<VecDeque<Arc<PublishJob>>>,
    notify: Condvar,
    /// 仅供 [`JobQueue::pop`] 在 poison/调试时观察；当前未用于停机（进程退出强杀线程）。
    _shutdown: AtomicBool,
}

impl JobQueue {
    fn new() -> Self {
        Self {
            inner: Mutex::new(VecDeque::new()),
            notify: Condvar::new(),
            _shutdown: AtomicBool::new(false),
        }
    }

    fn push(&self, job: Arc<PublishJob>) {
        let mut q = locked(&self.inner);
        q.push_back(job);
        self.notify.notify_one();
    }

    fn pop(&self) -> Option<Arc<PublishJob>> {
        let mut q = locked(&self.inner);
        loop {
            if let Some(job) = q.pop_front() {
                return Some(job);
            }
            // 无 job：阻塞等 push 唤醒。进程退出时线程 daemon 强杀，无需 shutdown 信号。
            q = self.notify.wait(q).unwrap_or_else(PoisonError::into_inner);
        }
    }
}

impl Scheduler {
    pub(crate) fn new() -> Self {
        Self {
            buckets: Mutex::new(HashMap::new()),
            queue: JobQueue::new(),
            registry: Mutex::new(HashMap::new()),
        }
    }

    /// 注册一个组的推送任务（`GroupObj::new` 调）。
    pub(crate) fn register(
        &self,
        key: u32,
        inner: Arc<Mutex<GroupInner>>,
        data_source: Arc<dyn DataSource>,
        data_cp: IConnectionPoint,
        rate_ms: u32,
    ) {
        let rate = Duration::from_millis(u64::from(rate_ms.max(1)));
        let job = Arc::new(PublishJob {
            key,
            inner,
            data_source,
            data_cp,
        });
        locked(&self.registry).insert(key, (Arc::clone(&job), rate));
        let mut buckets = locked(&self.buckets);
        let bucket = buckets.entry(rate).or_insert_with(|| Bucket {
            jobs: Vec::new(),
            next_tick: Instant::now() + rate,
        });
        bucket.jobs.push(job);
    }

    /// 注销一个组的推送任务（`GroupObj::Drop` 调）。key 不存在则 no-op（幂等）。
    pub(crate) fn unregister(&self, key: u32) {
        // 先取 removed（registry guard 在此语句末释放），再 if let——避免锁持有延长到 if body
        //（clippy significant_drop_in_scrutinee）。
        let removed = locked(&self.registry).remove(&key);
        if let Some((_, rate)) = removed {
            let mut buckets = locked(&self.buckets);
            if let Some(bucket) = buckets.get_mut(&rate) {
                bucket.jobs.retain(|j| j.key != key);
            }
        }
    }

    /// 注册表当前 job 数（测试/观测用）。
    #[cfg(test)]
    pub(crate) fn registered_count(&self) -> usize {
        locked(&self.registry).len()
    }
}

/// tick 主循环（独立线程）：1 ms 周期派发到期桶的 job 到队列。
fn tick_loop(s: &Scheduler) {
    // tick 线程当前不调 COM（只派发），加入 MTA 保险（未来若 tick 读 COM 状态）。
    // SAFETY: CoIncrementMTAUsage 幂等，让线程加入 MTA；返回 handle 忽略（线程生命周期内常驻）。
    let _ = unsafe { CoIncrementMTAUsage() };
    loop {
        tick_once(s);
        thread::sleep(Duration::from_millis(1));
    }
}

/// 单次 tick：检查所有桶，到期则派发。
fn tick_once(s: &Scheduler) {
    let now = Instant::now();
    // buckets 锁内收集到期 job（推进 next_tick + clone Arc）；锁释放后再 push 队列，
    // 避免 buckets 锁与 queue 锁嵌套（clippy significant_drop_tightening）。
    let due: Vec<Arc<PublishJob>> = {
        let mut buckets = locked(&s.buckets);
        let mut due = Vec::new();
        for (rate, bucket) in buckets.iter_mut() {
            if now < bucket.next_tick {
                continue;
            }
            bucket.next_tick += *rate;
            for job in &bucket.jobs {
                due.push(Arc::clone(job));
            }
        }
        due
    };
    for job in due {
        s.queue.push(job);
    }
}

/// worker 主循环（N 个线程）：MTA 下从队列取 job 推送。
fn worker_loop(s: &Scheduler) {
    // worker 调 COM sink（OnDataChange），必须 MTA。
    // SAFETY: CoIncrementMTAUsage 幂等，让 worker 线程加入 MTA；返回 handle 忽略。
    let _ = unsafe { CoIncrementMTAUsage() };
    while let Some(job) = s.queue.pop() {
        push_one(&job);
    }
}

thread_local! {
    static PUSH_BUF: RefCell<PushBuf> = RefCell::new(PushBuf::default());
}

#[derive(Default)]
struct PushBuf {
    hc: Vec<u32>,
    v: Vec<VARIANT>,
    q: Vec<u16>,
    ts: Vec<FILETIME>,
}

/// 推送单个 job（worker 线程调）：取 sink 快照 → 锁内 read + deadband 过滤 + 更新
/// `last_pushed` + 收集变化帧（复用 [`PUSH_BUF`]）→ 锁外 push `OnDataChange`。
fn push_one(job: &PublishJob) {
    let sinks = publisher::enumerate_sinks(&job.data_cp);
    if sinks.is_empty() {
        return;
    }
    PUSH_BUF.with(|cell| {
        let mut buf = cell.borrow_mut();
        buf.hc.clear();
        buf.v.clear();
        buf.q.clear();
        buf.ts.clear();
        let h_group = {
            let mut g = locked(&job.inner);
            let deadband = g.percent_deadband;
            let h_group = g.h_client_group;
            for entry in g.items.values_mut() {
                if !entry.active {
                    continue;
                }
                let (val, qual, t) = job.data_source.read(&entry.item_id);
                let nv = crate::data_source::normalize_variant(&val);
                let range = job.data_source.item_range(&entry.item_id);
                if crate::data_source::should_push(entry.last_pushed, nv, qual, deadband, range) {
                    entry.last_pushed = Some(crate::data_source::PushState {
                        value: nv.unwrap_or(0.0),
                        quality: qual,
                    });
                    buf.hc.push(entry.h_client);
                    buf.v.push(val);
                    buf.q.push(qual);
                    buf.ts.push(t);
                }
            }
            h_group
        };
        if !buf.hc.is_empty() {
            publisher::push_data_change(&sinks, h_group, &buf.hc, &buf.v, &buf.q, &buf.ts, 0);
        }
    });
}
