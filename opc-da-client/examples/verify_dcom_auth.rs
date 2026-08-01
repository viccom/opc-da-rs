//! 验证 DCOM 凭据（想法1）+ `FusionReader` 融合读取（想法2：默认订阅 + 同步兜底）。
//!
//! **阶段A** — 凭据验证：当前登录用户（ncpepc）vs 显式凭据（viccom）读同一个 tag。
//! **阶段B** — 融合读取：直接用库的 [`FusionReader`]（订阅优先，订阅失败/静默时自动
//! 同步兜底）。
//!
//! # Usage
//!
//! ```sh
//! cargo run -p opc-da-client --example verify_dcom_auth -- \
//!     --host <host> --user <user> --pass <password> \
//!     --server <ProgID> --tag <ItemID>   # 域帐户加 --domain <域名>
//! ```

use std::time::{Duration, Instant};

use opc_da_client::{
    AuthCredentials, ComConnector, FusionEvent, FusionReader, FusionReaderOptions, OpcDaClient,
    OpcProvider, friendly_com_hint,
};

#[tokio::main]
#[allow(clippy::too_many_lines, clippy::ignored_unit_patterns)]
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

    // A2: 显式凭据
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
    drop(client);

    println!("\n==== 阶段B：FusionReader 融合读取（想法2：默认订阅 + 同步兜底）====");
    let opts = FusionReaderOptions {
        update_rate: 1000,
        fallback_timeout: Duration::from_secs(10),
        buffer: 256,
    };
    let (reader, mut rx) =
        match FusionReader::start(&host, Some(creds), &server, vec![tag.clone()], &opts) {
            Ok(x) => x,
            Err(e) => {
                eprintln!("[B] FusionReader 启动失败：{e}");
                return;
            }
        };
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        let left = deadline.saturating_duration_since(Instant::now());
        tokio::select! {
            ev = rx.recv() => match ev {
                Some(FusionEvent::Data(tv)) => {
                    println!("[B][DATA]       {} = {} ({})", tv.tag_id, tv.value, tv.quality);
                }
                Some(FusionEvent::Subscribed) => println!("[B][SUBSCRIBED] 进入推送模式"),
                Some(FusionEvent::Fallback(e)) => println!("[B][FALLBACK]   切同步兜底 | {e}"),
                None => break,
            },
            _ = tokio::time::sleep(left) => break,
        }
    }
    let drop_start = Instant::now();
    drop(reader);
    eprintln!("[B] reader 拆除耗时 {:?}", drop_start.elapsed());
    println!("[B] 完成（20s）");
}

fn arg_or(args: &[String], key: &str, default: &str) -> String {
    // 跳过 args[0]（程序名），否则 flag/value 配对会整体错位一位。
    let mut iter = args.iter().skip(1);
    while let Some(a) = iter.next()
        && let Some(v) = iter.next()
    {
        if a == key {
            return v.clone();
        }
    }
    default.to_string()
}
