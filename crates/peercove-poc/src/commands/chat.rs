//! `chat` / `chat-log`: 稼働中トンネルのメンバーとチャットする(ADR-0016、
//! M3-13a の検証用 CLI)。送受信・履歴はデーモンが行い、この CLI は宛先の解決
//! (表示名 → 仮想 IP)と表示だけを受け持つ。

use std::net::Ipv4Addr;
use std::path::Path;

use anyhow::{bail, Context};
use peercove_core::ipc::{ChatMessageInfo, IpcRequest, IpcResponse, TunnelInfo};
use peercove_core::msg::ChatScope;

use crate::commands::send_file::{find_tunnel, resolve_peer};
use crate::daemon;

/// `chat`: 1 通送る(`--to <名前|IP>` または `--all`)。
pub fn send(config: &Path, to: Option<&str>, all: bool, text: &str) -> anyhow::Result<()> {
    // デーモンとは作業ディレクトリが違うため、パスは絶対にして送る
    let config = std::fs::canonicalize(config)
        .with_context(|| format!("{} が見つかりません", config.display()))?;
    let tunnel = find_tunnel(&config)?;
    let (scope, peer) = match (to, all) {
        (Some(to), false) => (ChatScope::Direct, Some(resolve_peer(&tunnel, to)?)),
        (None, true) => (ChatScope::Network, None),
        _ => bail!("--to <名前|IP> か --all のどちらか 1 つを指定してください"),
    };
    let response = daemon::request(IpcRequest::ChatSend {
        config,
        scope,
        peer,
        text: text.to_string(),
    })?;
    let IpcResponse::Chat { .. } = response else {
        bail!("デーモンから想定外の応答が返りました");
    };
    match scope {
        ChatScope::Direct => println!("送信しました(相手へ配送中)"),
        ChatScope::Network => println!("送信しました(オンラインのメンバー全員へ配送中)"),
    }
    println!("配送に失敗した場合は chat-log に(送信失敗)と表示されます");
    Ok(())
}

/// `chat-log`: 履歴を表示する。`--follow` は 1 秒ごとに新着を取りに行く。
pub fn log(config: &Path, follow: bool) -> anyhow::Result<()> {
    let config = std::fs::canonicalize(config)
        .with_context(|| format!("{} が見つかりません", config.display()))?;
    let mut after_seq = 0u64;
    loop {
        let tunnel = find_tunnel(&config)?;
        // 1 応答に載る件数には上限があるため、最新 seq に届くまで取り切る
        loop {
            let response = daemon::request(IpcRequest::ChatFetch {
                config: config.clone(),
                after_seq,
            })?;
            let IpcResponse::Chat { seq, messages } = response else {
                bail!("デーモンから想定外の応答が返りました");
            };
            for message in &messages {
                println!("{}", format_message(message, &tunnel));
                after_seq = message.seq;
            }
            if after_seq >= seq {
                break;
            }
        }
        if !follow {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

/// `12:34:56 [全体] alice: こんにちは` の形式(時刻は UTC — daemon logs と同じ)。
fn format_message(message: &ChatMessageInfo, tunnel: &TunnelInfo) -> String {
    let secs_of_day = (message.sent_at / 1000) % 86_400;
    let who = if message.from == tunnel.address {
        "自分".to_string()
    } else {
        display_name(tunnel, message.from)
    };
    let scope = match message.scope {
        ChatScope::Direct => {
            if message.from == tunnel.address {
                let to = message
                    .to
                    .map(|ip| display_name(tunnel, ip))
                    .unwrap_or_else(|| "?".to_string());
                format!("[→ {to}] ")
            } else {
                String::new()
            }
        }
        ChatScope::Network => "[全体] ".to_string(),
    };
    format!(
        "{:02}:{:02}:{:02} {scope}{who}: {}{}",
        secs_of_day / 3600,
        (secs_of_day / 60) % 60,
        secs_of_day % 60,
        message.text,
        if message.failed { "(送信失敗)" } else { "" }
    )
}

fn display_name(tunnel: &TunnelInfo, ip: Ipv4Addr) -> String {
    tunnel
        .ledger
        .iter()
        .find(|entry| entry.ip == ip)
        .and_then(|entry| entry.name.clone())
        .unwrap_or_else(|| ip.to_string())
}
