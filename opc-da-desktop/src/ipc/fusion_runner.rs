//! Fusion 订阅 runner：drain `FusionReader` 的 `rx`（`FusionEvent`）并推给
//! Tauri `Channel<FusionEventDto>`。
//!
//! 与 [`crate::ipc::subscription_runner`] 不同：本 runner **不调 `unsubscribe`**——
//! `FusionReader` 的 `Drop` 自带优雅退订（shutdown 信号 → 显式 unsubscribe，见库
//! `fusion_reader.rs`）。前端 `unsubscribe_fusion_tags` 直接 drop reader 即可。

use std::time::Duration;

use tauri::ipc::Channel;

use opc_da_client::FusionEvent;

use crate::ipc::{FusionEventDto, TagUpdate};

/// 推送一个 fusion 订阅的事件流。`rx` 结束（reader drop）或 channel 关闭时返回。
pub async fn run_fusion_subscription(
    mut rx: tauri::async_runtime::Receiver<FusionEvent>,
    channel: Channel<FusionEventDto>,
) {
    tracing::info!("fusion subscription runner started");
    loop {
        let next = tokio::select! {
            biased;
            next = rx.recv() => next,
            () = tokio::time::sleep(Duration::from_millis(50)) => continue,
        };
        match next {
            Some(ev) => {
                let dto = match ev {
                    FusionEvent::Data(tv) => FusionEventDto::Data(TagUpdate::from(tv)),
                    FusionEvent::Subscribed => FusionEventDto::Subscribed,
                    FusionEvent::Fallback(e) => FusionEventDto::Fallback {
                        message: e.to_string(),
                    },
                };
                if channel.send(dto).is_err() {
                    tracing::warn!("fusion channel send failed; WebView likely disconnected");
                    break;
                }
            }
            None => {
                tracing::info!("fusion stream ended");
                break;
            }
        }
    }
    tracing::info!("fusion subscription runner exited");
}
