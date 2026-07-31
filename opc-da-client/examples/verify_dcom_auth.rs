//! 验证 DCOM 凭据接通（想法1）与融合读取（想法2：订阅优先 + 同步兜底）。
//!
//! **阶段A — 凭据验证**：先用 null 凭据（当前登录用户）连远程，预期失败；再用显式
//! `user`/`password` 连，预期成功。直观体现"手动指定 DCOM 凭据"的价值。
//!
//! **阶段B — 融合读取**：订阅优先（`OnDataChange` 推送）；订阅建立失败 / 超时 / 推送流关闭，
//! 或回调静默超过 `fallback_timeout` → 自动退回同步轮询（应对 NAT/防火墙/client 端
//! DCOM 难配的场景）。订阅与兜底读取用各自独立的 `OpcDaClient`（独立 COM worker），
//! 避免订阅失败/超时阻塞兜底读取。
//!
//! # Usage
//!
//! ```sh
//! cargo run -p opc-da-client --example verify_dcom_auth -- \
//!     --host <host> --user <user> --pass <password> \
//!     --server Matrikon.OPC.Simulation.1 --tag Random.Real4
//! ```
//!
//! 不带参数则用上述默认值。域帐户加 `--domain 域名`；32-bit 客户端加 `--target i686-pc-windows-msvc`。

use std::time::{Duration, Instant};

use opc_da_client::{
    AuthCredentials, ComConnector, OpcDaClient, OpcProvider, SubscriptionHandle, friendly_com_hint,
};

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let host = arg_or(&args, "--host", "192.168.199.155");
    let user = arg_or(&args, "--user", "viccom");
    let pass = arg_or(&args, "--pass", "");
    let server = arg_or(&args, "--server", "Matrikon.OPC.Simulation.1");
    let tag = arg_or(&args, "--tag", "Random.Real4");
    let domain = arg_or(&args, "--domain", "");
    let me = std::env::var("USERNAME").unwrap_or_else(|_| "?".to_string());

    eprintln!(">>> verify_dcom_auth: host={host} server={server} tag={tag} (run as {me})");

    println!("\n==== 阶段A：DCOM 凭据验证（想法1）====");

    // A1: null 凭据（当前登录用户）
    println!("\n[A1] null 凭据（当前登录用户 {me}）");
    match OpcDaClient::new(ComConnector::new(&host)) {
        Ok(null_client) => {
            match null_client.list_servers(&host).await {
                Ok(list) => println!("[A1] list_servers 成功（{} 个）", list.len()),
                Err(e) => println!("[A1] list_servers 失败：{e}"),
            }
            match null_client
                .read_tag_values(&server, vec![tag.clone()])
                .await
            {
                Ok(vs) => {
                    for tv in vs {
                        println!(
                            "[A1] read {server}/{tag} = {} ({}, {})",
                            tv.value, tv.quality, tv.timestamp
                        );
                    }
                }
                Err(e) => println!("[A1] read 失败：{e}"),
            }
        }
        Err(e) => println!("[A1] client 初始化失败：{e}"),
    }

    // A2: 显式凭据（user/password）
    println!("\n[A2] 显式凭据 {user}@{host}（domain={domain}）");
    let creds = AuthCredentials {
        user: user.clone(),
        password: pass.clone(),
        domain: domain.clone(),
    };
    let client = match OpcDaClient::with_credentials(&host, creds.clone()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[A2] client 初始化失败：{e}");
            return;
        }
    };
    match client.list_servers(&host).await {
        Ok(list) => {
            println!("[A2] list_servers 成功（{} 个）：", list.len());
            for s in &list {
                println!("     - {s}");
            }
        }
        Err(e) => {
            eprintln!("[A2] list_servers 失败：{e}");
            if let Some(h) = friendly_com_hint(&e) {
                eprintln!("       HINT: {h}");
            }
        }
    }
    match client.read_tag_values(&server, vec![tag.clone()]).await {
        Ok(vs) => {
            for tv in vs {
                println!(
                    "[A2] read {server}/{tag} = {} ({}, {})",
                    tv.value, tv.quality, tv.timestamp
                );
            }
        }
        Err(e) => eprintln!("[A2] read 失败：{e}"),
    }

    println!("\n==== 阶段B：融合读取（想法2：订阅优先 + 同步兜底）====");
    fusion_reader(
        &host,
        creds,
        &server,
        &tag,
        1000,
        Duration::from_secs(10),
        Duration::from_secs(20),
    )
    .await;
}

/// 订阅优先；订阅失败 / 超时 / 回调静默超 `fallback_timeout` → 退回同步轮询。
/// 订阅与兜底读取用各自独立的 `OpcDaClient`，避免订阅阻塞兜底读取。跑满 `run_for`。
#[allow(clippy::too_many_lines)]
async fn fusion_reader(
    host: &str,
    creds: AuthCredentials,
    server: &str,
    tag: &str,
    update_rate: u32,
    fallback_timeout: Duration,
    run_for: Duration,
) {
    eprintln!("[B] 建立独立 sub_client + read_client（各自 COM worker，互不阻塞）");
    let sub_client = match OpcDaClient::with_credentials(host, creds.clone()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[B] sub_client 初始化失败：{e}");
            return;
        }
    };
    let read_client = match OpcDaClient::with_credentials(host, creds) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[B] read_client 初始化失败：{e}");
            return;
        }
    };

    let tags = vec![tag.to_string()];
    let start = Instant::now();
    let mut last_data = Instant::now();
    let mut mode_subscribe;

    // subscribe 限 8s：远程订阅反向回调不通时，server 端 Advise 会长时间 RPC 超时，
    // 必须截断，否则会吃掉整个 run_for 且阻塞 worker。
    let (mut rx, mut errors, cookie) = match tokio::time::timeout(
        Duration::from_secs(8),
        sub_client.subscribe(server, tags.clone(), update_rate),
    )
    .await
    {
        Ok(Ok(SubscriptionHandle { cookie, rx, errors })) => {
            println!(
                "[B] 订阅建立成功 → 推送模式（update_rate={update_rate}ms，fallback_timeout={}s）",
                fallback_timeout.as_secs()
            );
            mode_subscribe = true;
            (Some(rx), Some(errors), Some(cookie))
        }
        Ok(Err(e)) => {
            println!("[B] 订阅建立失败 → 直接同步兜底 | 原因：{e}");
            mode_subscribe = false;
            (None, None, None)
        }
        Err(_) => {
            println!("[B] 订阅 8s 未完成（反向回调可能不通）→ 直接同步兜底");
            mode_subscribe = false;
            (None, None, None)
        }
    };

    while start.elapsed() < run_for {
        if mode_subscribe {
            // 非阻塞 drain 订阅级错误（重建失败等）→ 触发 fallback
            if let Some(err_rx) = errors.as_mut()
                && let Ok(err) = err_rx.try_recv()
            {
                println!("[B][SUBSCRIBE ERROR] {err} → 切同步兜底");
                mode_subscribe = false;
            }
            if !mode_subscribe {
                continue;
            }
            let since_data = last_data.elapsed();
            if since_data >= fallback_timeout {
                println!(
                    "[B][CALLBACK_SILENT] {}s 内无回调 → 切同步兜底",
                    fallback_timeout.as_secs()
                );
                mode_subscribe = false;
                continue;
            }
            let left = fallback_timeout.saturating_sub(since_data);
            let Some(rx_recv) = rx.as_mut() else {
                mode_subscribe = false;
                continue;
            };
            match tokio::time::timeout(left, rx_recv.recv()).await {
                Ok(Some(tv)) => {
                    last_data = Instant::now();
                    println!(
                        "[B][SUBSCRIBE]     {} = {} ({})",
                        tv.tag_id, tv.value, tv.quality
                    );
                }
                Ok(None) => {
                    println!("[B][SUBSCRIBE] 推送流关闭 → 切同步兜底");
                    mode_subscribe = false;
                }
                Err(_) => {
                    println!(
                        "[B][CALLBACK_SILENT] {}s 内无回调 → 切同步兜底",
                        fallback_timeout.as_secs()
                    );
                    mode_subscribe = false;
                }
            }
        } else {
            // 兜底读取用独立 read_client，不被订阅 worker 阻塞。
            match tokio::time::timeout(
                Duration::from_secs(5),
                read_client.read_tag_values(server, tags.clone()),
            )
            .await
            {
                Ok(Ok(vs)) => {
                    for tv in vs {
                        println!(
                            "[B][SYNC_FALLBACK] {} = {} ({})",
                            tv.tag_id, tv.value, tv.quality
                        );
                    }
                }
                Ok(Err(e)) => println!("[B][SYNC_FALLBACK] read 失败：{e}"),
                Err(_) => println!("[B][SYNC_FALLBACK] read 超时（5s）"),
            }
            tokio::time::sleep(Duration::from_millis(u64::from(update_rate))).await;
        }
    }

    if let Some(c) = cookie {
        let _ = sub_client.unsubscribe(c).await;
    }
    println!("[B] 完成（{}s）", run_for.as_secs());
}

fn arg_or(args: &[String], key: &str, default: &str) -> String {
    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        if a == key
            && let Some(v) = iter.next()
        {
            return v.clone();
        }
    }
    default.to_string()
}
