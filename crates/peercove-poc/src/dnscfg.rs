//! OS のスプリット DNS 設定(ADR-0011 §4、M3-1b)。
//!
//! `*.peercove.internal` のクエリだけを内蔵リゾルバ(トンネル IP の :53)へ
//! 向ける。他ドメインの解決には一切干渉しない。
//!
//! - **Windows**: NRPT(Name Resolution Policy Table)にルールを 1 つ登録する。
//!   ルールは**マシン設定として永続**するため、デーモン起動時と終了時に必ず
//!   自前ルール(Comment = "PeerCove")を掃除する(異常終了の残骸対策)
//! - **Linux**: systemd-resolved の per-link 設定(`resolvectl`)。リンク
//!   (トンネル IF)の消滅と同時に resolved 側が忘れるので、残骸は残らない
//!
//! 失敗は警告ログに留める(名前解決は付加機能。トンネル自体は動かし続ける)。

#![cfg_attr(test, allow(dead_code))]

use std::net::Ipv4Addr;

/// NRPT / resolvectl に登録するサフィックス(先頭ドット = サフィックス一致)。
pub const NAMESPACE: &str = ".peercove.internal";

#[cfg(windows)]
mod imp {
    use super::*;

    /// 自前の NRPT ルールを識別するコメント。
    const RULE_COMMENT: &str = "PeerCove";

    /// NRPT ルールを「現在の内蔵リゾルバ一覧」に同期する(空なら削除のみ)。
    /// トンネルの開始・停止のたびに呼ばれる。ブロッキング(要 spawn_blocking)。
    pub fn apply_servers(servers: &[Ipv4Addr]) {
        // 既存の自前ルールを削除(重複登録・残骸の防止)
        run(&format!(
            "Get-DnsClientNrptRule | Where-Object {{ $_.Comment -eq '{RULE_COMMENT}' }} \
             | Remove-DnsClientNrptRule -Force"
        ));
        if servers.is_empty() {
            return;
        }
        let list = servers
            .iter()
            .map(|ip| format!("'{ip}'"))
            .collect::<Vec<_>>()
            .join(",");
        if run(&format!(
            "Add-DnsClientNrptRule -Namespace '{NAMESPACE}' -NameServers @({list}) \
             -Comment '{RULE_COMMENT}'"
        )) {
            tracing::info!("NRPT に {NAMESPACE} → {servers:?} を登録しました");
        } else {
            tracing::warn!(
                "NRPT ルールの登録に失敗しました。{NAMESPACE} の名前解決は使えません\
                 (トンネル自体は動作します)"
            );
        }
    }

    /// Linux 用 API との対称性のためのダミー(Windows は per-link 設定不要)。
    pub fn register_link(_if_name: &str, _server: Ipv4Addr) {}

    fn run(command: &str) -> bool {
        let result = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", command])
            .output();
        match result {
            Ok(output) if output.status.success() => true,
            Ok(output) => {
                tracing::debug!(
                    "PowerShell 失敗({}): {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                );
                false
            }
            Err(e) => {
                tracing::debug!("PowerShell を起動できません: {e}");
                false
            }
        }
    }
}

#[cfg(unix)]
mod imp {
    use super::*;

    /// per-link のスプリット DNS を登録する(トンネル起動直後に 1 回)。
    /// リンク消滅時に resolved が自動で忘れるため、解除処理は不要。
    /// ブロッキング(要 spawn_blocking)。
    pub fn register_link(if_name: &str, server: Ipv4Addr) {
        let ok = run(&["dns", if_name, &server.to_string()])
            && run(&[
                "domain",
                if_name,
                &format!("~{}", NAMESPACE.trim_start_matches('.')),
            ]);
        if ok {
            tracing::info!("{if_name} に {NAMESPACE} → {server} のスプリット DNS を設定しました");
        } else {
            tracing::warn!(
                "resolvectl によるスプリット DNS 設定に失敗しました。{NAMESPACE} の\
                 名前解決は使えません(systemd-resolved が無効の環境では未対応)"
            );
        }
    }

    /// Windows 用 API との対称性のためのダミー(Linux は per-link で完結)。
    pub fn apply_servers(_servers: &[Ipv4Addr]) {}

    fn run(args: &[&str]) -> bool {
        let result = std::process::Command::new("resolvectl").args(args).output();
        match result {
            Ok(output) if output.status.success() => true,
            Ok(output) => {
                tracing::debug!(
                    "resolvectl {args:?} 失敗({}): {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                );
                false
            }
            Err(e) => {
                tracing::debug!("resolvectl を起動できません: {e}");
                false
            }
        }
    }
}

pub use imp::{apply_servers, register_link};
