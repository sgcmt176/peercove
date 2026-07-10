//! `send-file`: 稼働中トンネルのメンバーへファイルを送る(ADR-0015、M3-9)。
//!
//! 送信自体はデーモンが行う(IPC の SendFile)。この CLI は宛先の解決
//! (表示名 → 仮想 IP)と進捗表示だけを受け持つ。

use std::io::Write as _;
use std::net::Ipv4Addr;
use std::path::Path;

use anyhow::{bail, Context};
use peercove_core::ipc::{IpcRequest, IpcResponse, TransferInfo, TunnelInfo};

use crate::daemon;

pub fn run(config: &Path, to: &str, file: &Path) -> anyhow::Result<()> {
    // デーモンとは作業ディレクトリが違うため、パスは絶対にして送る
    let config = std::fs::canonicalize(config)
        .with_context(|| format!("{} が見つかりません", config.display()))?;
    let file = std::fs::canonicalize(file)
        .with_context(|| format!("{} が見つかりません", file.display()))?;

    let tunnel = find_tunnel(&config)?;
    let peer = resolve_peer(&tunnel, to)?;

    let response = daemon::request(IpcRequest::SendFile {
        config: config.clone(),
        peer,
        path: file,
    })?;
    let IpcResponse::Transfer { id } = response else {
        bail!("デーモンから想定外の応答が返りました");
    };

    // 進捗を status 経由でポーリング表示する
    loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let tunnel = find_tunnel(&config)?;
        let Some(transfer) = tunnel.transfers.iter().find(|t| t.id == id) else {
            bail!("転送がデーモンの進捗一覧から消えました");
        };
        print_progress(transfer);
        if transfer.done {
            println!();
            match &transfer.error {
                Some(error) => bail!("送信に失敗しました: {error}"),
                None => {
                    println!("送信が完了しました(相手の受信ボックスに入りました)");
                    return Ok(());
                }
            }
        }
    }
}

pub(crate) fn find_tunnel(config: &Path) -> anyhow::Result<TunnelInfo> {
    let IpcResponse::Status(status) = daemon::request(IpcRequest::Status)? else {
        bail!("デーモンから想定外の応答が返りました");
    };
    status
        .tunnels
        .into_iter()
        .find(|t| t.config == config)
        .with_context(|| format!("この設定のトンネルは動いていません({})", config.display()))
}

/// 宛先の解決: 仮想 IP 直指定、または台帳の表示名。
pub(crate) fn resolve_peer(tunnel: &TunnelInfo, to: &str) -> anyhow::Result<Ipv4Addr> {
    if let Ok(ip) = to.parse::<Ipv4Addr>() {
        return Ok(ip);
    }
    tunnel
        .ledger
        .iter()
        .find(|entry| entry.name.as_deref() == Some(to))
        .map(|entry| entry.ip)
        .with_context(|| {
            let known: Vec<&str> = tunnel
                .ledger
                .iter()
                .filter_map(|e| e.name.as_deref())
                .collect();
            format!(
                "メンバー \"{to}\" が見つかりません(登録名: {})",
                known.join(", ")
            )
        })
}

fn print_progress(transfer: &TransferInfo) {
    // 空ファイル(size 0)は 100% 扱い
    let percent = (transfer.transferred * 100)
        .checked_div(transfer.size)
        .unwrap_or(100);
    print!(
        "\r送信中… {percent:3}%({} / {} バイト)",
        transfer.transferred, transfer.size
    );
    let _ = std::io::stdout().flush();
}
