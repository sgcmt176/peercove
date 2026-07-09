use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Context;
use peercove_core::proto::LedgerEntry;

use crate::backend::PeerStats;

/// この時間より古いステータスファイルは「停止中の残骸」とみなす。
/// 書き込み周期(5 秒)の 3 倍。
const STALE_AFTER: Duration = Duration::from_secs(15);

/// host/member プロセスが書き出すステータスファイルのパス(ADR-0002)。
pub fn status_file_path(config_path: &Path) -> PathBuf {
    config_path.with_extension("status.txt")
}

/// ステータスファイルを表示する。
pub fn run(config_path: &Path) -> anyhow::Result<()> {
    // 設定の妥当性確認を兼ねる(設定が壊れていれば分かりやすく失敗させる)
    let config = peercove_core::config::Config::load(config_path)?;
    let path = status_file_path(config_path);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!(
                "ステータスファイル {} がありません。この設定で host / member \
                 プロセスが起動しているか確認してください",
                path.display()
            );
        }
        Err(e) => {
            return Err(
                anyhow::anyhow!(e).context(format!("{} の読み込みに失敗しました", path.display()))
            )
        }
    };
    let age = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok());
    println!(
        "interface: {}({})",
        config.interface.name, config.interface.address
    );
    print!("{text}");
    if let Some(age) = age {
        if age > STALE_AFTER {
            println!();
            println!(
                "警告: この情報は {} 秒前のものです。host / member プロセスが停止して\
                 いる可能性があります",
                age.as_secs()
            );
        }
    }
    Ok(())
}

/// 統計と台帳をステータスファイル・画面共通の形式に整形する。
pub fn render(stats: &[PeerStats], ledger: Option<&[LedgerEntry]>) -> String {
    let mut out = String::new();
    // 台帳(コントロールチャネル経由。host は自前、member は受信したもの)
    if let Some(ledger) = ledger {
        out.push_str("members:\n");
        for entry in ledger {
            out.push_str(&format!(
                "  {} {}({}){}\n",
                if entry.online { "●" } else { "○" },
                entry.name.as_deref().unwrap_or("(名前なし)"),
                entry.ip,
                if entry.is_host { " [host]" } else { "" }
            ));
        }
        out.push('\n');
    }
    if stats.is_empty() {
        out.push_str("peers: なし\n");
        return out;
    }
    for peer in stats {
        out.push_str(&format!("peer: {}\n", peer.public_key));
        out.push_str(&format!(
            "  endpoint: {}\n",
            peer.endpoint
                .map(|e| e.to_string())
                .unwrap_or_else(|| "(未接続)".to_string())
        ));
        let allowed = peer
            .allowed_ips
            .iter()
            .map(|net| net.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("  allowed_ips: {allowed}\n"));
        let handshake = match peer.last_handshake {
            Some(t) => match SystemTime::now().duration_since(t) {
                Ok(elapsed) => format!("{} 秒前", elapsed.as_secs()),
                Err(_) => "たった今".to_string(),
            },
            None => "なし".to_string(),
        };
        out.push_str(&format!("  latest handshake: {handshake}\n"));
        out.push_str(&format!(
            "  transfer: rx {}, tx {}\n",
            human_bytes(peer.rx_bytes),
            human_bytes(peer.tx_bytes)
        ));
    }
    out
}

/// ステータスファイルへ書き出す。失敗は呼び出し側で警告ログにする。
pub fn write_status_file(
    path: &Path,
    stats: &[PeerStats],
    ledger: Option<&[LedgerEntry]>,
) -> anyhow::Result<()> {
    std::fs::write(path, render(stats, ledger))
        .with_context(|| format!("{} の書き込みに失敗しました", path.display()))
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peercove_core::keys::PublicKey;

    #[test]
    fn renders_peer_with_and_without_handshake() {
        let stats = vec![
            PeerStats {
                public_key: PublicKey::from_bytes([1; 32]),
                endpoint: Some("203.0.113.5:51820".parse().unwrap()),
                last_handshake: Some(SystemTime::now() - Duration::from_secs(12)),
                tx_bytes: 1536,
                rx_bytes: 42,
                allowed_ips: vec!["100.100.42.0/24".parse().unwrap()],
            },
            PeerStats {
                public_key: PublicKey::from_bytes([2; 32]),
                endpoint: None,
                last_handshake: None,
                tx_bytes: 0,
                rx_bytes: 0,
                allowed_ips: vec!["100.100.42.3/32".parse().unwrap()],
            },
        ];
        let text = render(&stats, None);
        assert!(text.contains("endpoint: 203.0.113.5:51820"));
        assert!(text.contains("latest handshake: 12 秒前"));
        assert!(text.contains("transfer: rx 42 B, tx 1.50 KiB"));
        assert!(text.contains("endpoint: (未接続)"));
        assert!(text.contains("latest handshake: なし"));
        assert!(!text.contains("members:"));
    }

    #[test]
    fn renders_ledger_section() {
        use peercove_core::proto::LedgerEntry;
        let ledger = vec![
            LedgerEntry {
                name: Some("host".to_string()),
                ip: "100.100.42.1".parse().unwrap(),
                public_key: PublicKey::from_bytes([3; 32]),
                online: true,
                is_host: true,
                endpoint: None,
                endpoint_age_secs: None,
            },
            LedgerEntry {
                name: Some("alice".to_string()),
                ip: "100.100.42.2".parse().unwrap(),
                public_key: PublicKey::from_bytes([4; 32]),
                online: false,
                is_host: false,
                endpoint: None,
                endpoint_age_secs: None,
            },
        ];
        let text = render(&[], Some(&ledger));
        assert!(text.contains("members:"));
        assert!(text.contains("● host(100.100.42.1) [host]"));
        assert!(text.contains("○ alice(100.100.42.2)"));
    }

    #[test]
    fn human_bytes_units() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(1024), "1.00 KiB");
        assert_eq!(human_bytes(1024 * 1024 * 3 / 2), "1.50 MiB");
    }

    #[test]
    fn status_path_replaces_extension() {
        assert_eq!(
            status_file_path(Path::new("dir/host.toml")),
            Path::new("dir/host.status.txt")
        );
    }
}
