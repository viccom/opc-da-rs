//! 融合读取：默认订阅（`OnDataChange` 推送），订阅失败 / 超时 / 回调静默时自动 fallback
//! 同步轮询。
//!
//! 专为 NAT / 防火墙 / client 端 DCOM 回调难配等场景：远程订阅反向回调不通时，不报错
//! 中断，而是自动切同步轮询兜底，保证数据持续可得。
//!
//! 订阅与兜底读取用各自独立的 [`OpcDaClient`]（独立 COM worker），避免订阅失败 / 超时
//! 阻塞兜底读取（经真机验证：同 worker 会被 server 端 Advise 的长 RPC 超时卡死）。
//!
//! # Example
//!
//! ```no_run
//! # use std::time::Duration;
//! # use opc_da_client::{FusionReader, FusionReaderOptions, AuthCredentials};
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let (reader, mut rx) = FusionReader::start(
//!     "192.168.1.10",
//!     None,                       // None=当前登录用户；Some(AuthCredentials)=显式凭据
//!     "Matrikon.OPC.Simulation.1",
//!     vec!["Random.Real4".into()],
//!     &FusionReaderOptions::default(),
//! )?;
//! while let Some(ev) = rx.recv().await {
//!     match ev {
//!         opc_da_client::FusionEvent::Data(tv) => println!("{} = {}", tv.tag_id, tv.value),
//!         opc_da_client::FusionEvent::Subscribed => println!("[推送模式]"),
//!         opc_da_client::FusionEvent::Fallback(e) => println!("[同步兜底] {e}"),
//!     }
//! }
//! # Ok(()) }
//! ```

use std::mem::ManuallyDrop;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::opc_da::errors::{OpcError, OpcResult};
use crate::{
    AuthCredentials, ComConnector, OpcDaClient, OpcProvider, SubscriptionHandle, TagValue,
};

/// 远程订阅反向回调不通时，server 端 Advise 会长时间 RPC 超时，必须截断。经验值：
/// 太短会误判正常订阅，太长会阻塞首次兜底（8s 覆盖绝大多数正常 Advise 往返）。
const SUBSCRIBE_TIMEOUT: Duration = Duration::from_secs(8);

/// 同步兜底读取的单次超时。read 正常 <1s；超时说明 worker 卡在 RPC，放弃本次（下一轮
/// check shutdown 后退出），避免 task 永远卡在 read 上无法响应拆除。
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// 退出时显式退订 cookie 的超时。unadvise 正常 <1s；超时放弃，退而靠 server lease 回收，
/// 不让退订卡住清理线程。
const UNSUB_TIMEOUT: Duration = Duration::from_secs(3);

/// 融合读取事件。
#[derive(Debug)]
pub enum FusionEvent {
    /// 一条标签值（订阅推送或同步轮询产生）。
    Data(TagValue),
    /// 订阅建立成功，进入推送模式。
    Subscribed,
    /// 切换到同步兜底（订阅失败 / 超时 / 回调静默 / 推送流关闭），携带原因。
    Fallback(OpcError),
}

/// 融合读取选项。
#[derive(Clone)]
pub struct FusionReaderOptions {
    /// 订阅 `update_rate`，兼作同步兜底的轮询周期（毫秒）。
    pub update_rate: u32,
    /// 订阅建立成功后，多久收不到回调即判静默、切同步兜底。
    pub fallback_timeout: Duration,
    /// 事件 channel 容量。`Data` 在满时丢弃（保 `Subscribed`/`Fallback` 必达）。
    pub buffer: usize,
}

impl Default for FusionReaderOptions {
    fn default() -> Self {
        Self {
            update_rate: 1000,
            fallback_timeout: Duration::from_secs(10),
            buffer: 256,
        }
    }
}

/// 融合读取器：默认订阅 + 同步兜底。
///
/// 用 [`FusionReader::start`] 启动（**必须在 tokio runtime 内**），返回事件接收端。
/// Drop 时取消后台 task 并释放订阅。
pub struct FusionReader {
    task: Option<JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
}

impl FusionReader {
    /// 启动融合读取。
    ///
    /// - `creds = None`：用当前登录用户（DCOM 默认认证）。
    /// - `creds = Some`：用显式 user/password（跨工作组远程推荐，绕开当前用户 token 的
    ///   SID 跨机不匹配问题，详见 `DCOM_GUIDE.md`）。
    ///
    /// # Errors
    /// 仅当 client（COM worker）初始化失败时返回 `Err`。运行期订阅/读取失败通过
    /// `FusionEvent::Fallback` 事件报告，不中断流。
    pub fn start(
        host: &str,
        creds: Option<AuthCredentials>,
        server: &str,
        tags: Vec<String>,
        opts: &FusionReaderOptions,
    ) -> OpcResult<(Self, mpsc::Receiver<FusionEvent>)> {
        let sub_client = build_client(host, creds.clone())?;
        let read_client = build_client(host, creds)?;
        let update_rate = opts.update_rate;
        let fallback_timeout = opts.fallback_timeout;
        let (tx, rx) = mpsc::channel(opts.buffer);
        let server_owned = server.to_string();
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();
        let task = tokio::spawn(async move {
            run_fusion(
                sub_client,
                read_client,
                server_owned,
                tags,
                update_rate,
                fallback_timeout,
                tx,
                shutdown_clone,
            )
            .await;
        });
        Ok((
            Self {
                task: Some(task),
                shutdown,
            },
            rx,
        ))
    }
}

impl Drop for FusionReader {
    fn drop(&mut self) {
        // 通知 task 优雅退出（走完末尾 unsubscribe），不 abort——避免 abort 跳过 server 端
        // 显式退订（group 只能靠 lease 延迟回收）。task 各 await 均有 timeout 兜底，几秒内退出。
        self.shutdown.store(true, Ordering::Relaxed);
        // detach：不 join 也不 abort。task 在后台自行退出，client 由 DetachingClient 移到
        // 清理线程释放。JoinHandle drop 不会 cancel task。
        self.task.take();
    }
}

/// 包装 `OpcDaClient`，使其 `Drop`（含 `ComWorker::join`）在独立 OS 线程上执行。
///
/// 远程订阅反向回调不通时，worker 线程可能卡在 `Advise` 同步 RPC 直到 DCOM 超时
/// （数分钟）。若 client 在 tokio runtime 线程上 drop，`ComWorker::drop::join()` 会
/// 阻塞该 runtime 线程：单线程 runtime（current_thread）整体冻结，多线程 runtime
/// 损失一个 worker。这里把真正的释放移到专用清理线程，堵清理线程而非 runtime。
struct DetachingClient {
    inner: ManuallyDrop<OpcDaClient<ComConnector>>,
}

impl DetachingClient {
    fn new(client: OpcDaClient<ComConnector>) -> Self {
        Self {
            inner: ManuallyDrop::new(client),
        }
    }

    /// 被包装 client 的引用（`OpcProvider` 方法均为 `&self`，无需可变访问）。
    fn client(&self) -> &OpcDaClient<ComConnector> {
        &self.inner
    }
}

impl Drop for DetachingClient {
    fn drop(&mut self) {
        // SAFETY: `inner` 由 ManuallyDrop 持有，仅在此 take 一次；take 后结构体即被
        // 丢弃，不会再访问 `inner`，故无 double-drop。
        let client = unsafe { ManuallyDrop::take(&mut self.inner) };
        // spawn 失败（资源耗尽，极罕见）时 `client` 随闭包在此原地 drop，退回原同步
        // join 行为，不比修复前更差。
        let _ = std::thread::Builder::new()
            .name("opc-da-fusion-cleanup".into())
            .spawn(move || drop(client));
    }
}

fn build_client(host: &str, creds: Option<AuthCredentials>) -> OpcResult<DetachingClient> {
    let client = match creds {
        Some(c) => OpcDaClient::with_credentials(host, c),
        None => OpcDaClient::new(ComConnector::new(host)),
    }?;
    Ok(DetachingClient::new(client))
}

/// 后台 task：维护订阅/同步模式，向 `tx` 发事件。`rx` 关闭（上层 drop 接收端）时退出。
#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
async fn run_fusion(
    sub_client: DetachingClient,
    read_client: DetachingClient,
    server: String,
    tags: Vec<String>,
    update_rate: u32,
    fallback_timeout: Duration,
    tx: mpsc::Sender<FusionEvent>,
    shutdown: Arc<AtomicBool>,
) {
    let mut mode_subscribe;
    let (mut rx_sub, mut errors, cookie) = match tokio::time::timeout(
        SUBSCRIBE_TIMEOUT,
        sub_client
            .client()
            .subscribe(&server, tags.clone(), update_rate),
    )
    .await
    {
        Ok(Ok(SubscriptionHandle { cookie, rx, errors })) => {
            // Subscribed 必达；若接收端已关则直接退出。
            if tx.send(FusionEvent::Subscribed).await.is_err() {
                let _ = sub_client.client().unsubscribe(cookie).await;
                return;
            }
            mode_subscribe = true;
            (Some(rx), Some(errors), Some(cookie))
        }
        Ok(Err(e)) => {
            let _ = tx.send(FusionEvent::Fallback(e)).await;
            mode_subscribe = false;
            (None, None, None)
        }
        Err(_) => {
            let _ = tx
                .send(FusionEvent::Fallback(OpcError::Connection(
                    "订阅 8s 未完成（反向回调可能不通）".to_string(),
                )))
                .await;
            mode_subscribe = false;
            (None, None, None)
        }
    };

    let mut last_data = Instant::now();
    loop {
        if shutdown.load(Ordering::Relaxed) || tx.is_closed() {
            break;
        }
        if mode_subscribe {
            // 非阻塞 drain 订阅级错误（重建失败等）→ fallback
            if let Some(err_rx) = errors.as_mut()
                && let Ok(err) = err_rx.try_recv()
            {
                let _ = tx.send(FusionEvent::Fallback(err)).await;
                mode_subscribe = false;
                continue;
            }
            let Some(rx_recv) = rx_sub.as_mut() else {
                mode_subscribe = false;
                continue;
            };
            let since = last_data.elapsed();
            if since >= fallback_timeout {
                let _ = tx
                    .send(FusionEvent::Fallback(OpcError::Connection(format!(
                        "回调静默 {}s",
                        fallback_timeout.as_secs()
                    ))))
                    .await;
                mode_subscribe = false;
                continue;
            }
            let left = fallback_timeout.saturating_sub(since);
            match tokio::time::timeout(left, rx_recv.recv()).await {
                Ok(Some(tv)) => {
                    last_data = Instant::now();
                    // Data 用 try_send：满则丢，避免 task 阻塞。
                    let _ = tx.try_send(FusionEvent::Data(tv));
                }
                Ok(None) => {
                    let _ = tx
                        .send(FusionEvent::Fallback(OpcError::Connection(
                            "推送流关闭".to_string(),
                        )))
                        .await;
                    mode_subscribe = false;
                }
                Err(_) => {
                    let _ = tx
                        .send(FusionEvent::Fallback(OpcError::Connection(format!(
                            "回调静默 {}s",
                            fallback_timeout.as_secs()
                        ))))
                        .await;
                    mode_subscribe = false;
                }
            }
        } else {
            match tokio::time::timeout(
                READ_TIMEOUT,
                read_client.client().read_tag_values(&server, tags.clone()),
            )
            .await
            {
                Ok(Ok(vs)) => {
                    for tv in vs {
                        let _ = tx.try_send(FusionEvent::Data(tv));
                    }
                }
                Ok(Err(e)) => {
                    let _ = tx.send(FusionEvent::Fallback(e)).await;
                }
                Err(_) => {
                    let _ = tx
                        .send(FusionEvent::Fallback(OpcError::Connection(format!(
                            "read 超时 {}s",
                            READ_TIMEOUT.as_secs()
                        ))))
                        .await;
                }
            }
            tokio::time::sleep(Duration::from_millis(u64::from(update_rate))).await;
        }
    }

    if let Some(c) = cookie {
        // 显式退订；超时放弃（server 端 lease 兜底回收），避免卡住清理线程。
        let _ = tokio::time::timeout(UNSUB_TIMEOUT, sub_client.client().unsubscribe(c)).await;
    }
}
