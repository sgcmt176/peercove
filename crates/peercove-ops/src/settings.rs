//! 設定ファイルの `[interface]` と、メンバー設定のホスト endpoint の編集(M2-G5)。
//!
//! `[[peer]]` の追加・削除・改名は [`crate::peers`] の担当。ここで扱うのは
//! 「自分側の設定」だけ:
//!
//! | 項目 | 反映タイミング |
//! |---|---|
//! | `display_name` | 次の supervisor 周期(約 5 秒)で台帳に載る |
//! | `listen_port` / `mtu` | **トンネルの再起動が必要**(インターフェース生成時に決まる) |
//! | ホストの `endpoint`(メンバー設定) | **トンネルの再起動が必要**(自分のピア設定は再読込しない) |
//!
//! 呼び出し側は [`Settings::restart_required`] を見て、その旨を利用者に伝えること。
//! コメントと整形を保つため書き換えは `toml_edit` で行う。

use std::net::SocketAddr;
use std::path::Path;

use anyhow::{bail, Context};
use peercove_core::config::Config;

use crate::peers::{load_doc, write_validated};

/// MTU の許容範囲。下限は [`Config::validate`] と揃える。上限はイーサネットの
/// ペイロード上限(1500)。WG のヘッダ分を引いた 1420 が既定値。
const MTU_RANGE: std::ops::RangeInclusive<u16> = 576..=1500;

/// 画面に出す現在値。
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub interface_name: String,
    /// 台帳に載る自分の表示名。
    pub display_name: Option<String>,
    /// (host のみ)自分の DNS 名(ADR-0021、M3-14a)。未設定なら従来どおり
    /// 表示名から導出(実質 `host`)。メンバーの DNS 名は host.toml が正本の
    /// ためここには出ない(変更はデーモン IPC の set_dns_name 経由)。
    /// 反映は次の supervisor 周期(約 5 秒)— 再起動不要。
    pub dns_name: Option<String>,
    /// 自分の仮想 IP とサブネット(表示のみ。変更は init/join のやり直し)。
    pub address: String,
    /// UDP 待受ポート。メンバーでは未指定(OS 任せ)が普通。
    pub listen_port: Option<u16>,
    pub mtu: u16,
    /// メンバー設定のときだけ Some(ホストの `IP:ポート`)。
    pub host_endpoint: Option<String>,
    /// この設定がメンバー設定か(= ピアが 1 つで endpoint を持つ)。
    pub is_member: bool,
    /// メンバー間直接通信を試すか(ADR-0013、既定 true)。
    /// 反映は次の supervisor 周期(約 5 秒)— 再起動不要。
    pub direct: bool,
    /// 受信するファイルサイズの上限(MB、ADR-0015 / M3-9)。0 で無制限。
    /// 反映は次の supervisor 周期(約 5 秒)— 再起動不要。
    pub max_recv_file_mb: u64,
    /// (host のみ)新規招待を管理者承認まで隔離する。
    pub require_invite_approval: bool,
}

impl Settings {
    /// この設定を変えたときトンネルの再起動が必要か(`display_name` 以外は必要)。
    pub fn restart_required(&self, update: &Update) -> bool {
        self.listen_port != update.listen_port
            || self.mtu != update.mtu
            || (self.is_member && self.host_endpoint != update.host_endpoint)
    }
}

/// 書き戻す値。UI は現在値を読んでから、全項目を埋めて渡す。
#[derive(Debug, Clone, PartialEq)]
pub struct Update {
    /// `None` または空文字で削除。
    pub display_name: Option<String>,
    /// (host のみ)自分の DNS 名(ADR-0021)。`None` または空文字で削除
    /// (従来導出に戻る)。メンバー設定では無視する。
    pub dns_name: Option<String>,
    /// `None` で削除(ホストでは既定の 51820、メンバーでは OS 任せになる)。
    pub listen_port: Option<u16>,
    pub mtu: u16,
    /// メンバー設定のときだけ意味を持つ。ホスト設定では無視する。
    pub host_endpoint: Option<String>,
    /// メンバー間直接通信を試すか(ADR-0013)。
    pub direct: bool,
    /// 受信するファイルサイズの上限(MB)。0 で無制限。
    pub max_recv_file_mb: u64,
    pub require_invite_approval: bool,
}

/// 現在の設定を読む。
pub fn read(config_path: &Path) -> anyhow::Result<Settings> {
    let config = Config::load(config_path)?;
    // join が書く member.toml は「endpoint を持つピアがちょうど 1 つ」。
    // host.toml のピア(メンバー)は endpoint を持たない
    let host_endpoint = match config.peers.as_slice() {
        [peer] => peer.endpoint.map(|e| e.to_string()),
        _ => None,
    };
    Ok(Settings {
        interface_name: config.interface.name.clone(),
        display_name: config.interface.display_name.clone(),
        dns_name: config.interface.dns_name.clone(),
        address: config.interface.address.to_string(),
        listen_port: config.interface.listen_port,
        mtu: config.interface.mtu,
        is_member: host_endpoint.is_some(),
        host_endpoint,
        direct: config.interface.direct,
        max_recv_file_mb: config.interface.max_recv_file_mb,
        require_invite_approval: config.interface.require_invite_approval,
    })
}

/// 設定を書き戻す。書く前に、結果が設定として妥当かを検証する。
pub fn update(config_path: &Path, update: &Update) -> anyhow::Result<()> {
    if !MTU_RANGE.contains(&update.mtu) {
        bail!(
            "MTU は {}〜{} の範囲で指定してください(既定 {})",
            MTU_RANGE.start(),
            MTU_RANGE.end(),
            peercove_core::config::DEFAULT_MTU
        );
    }
    if update.listen_port == Some(0) {
        bail!("待受ポートに 0 は指定できません");
    }
    let display_name = update
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty());
    if let Some(name) = display_name {
        crate::invite::validate_name(name)?;
    }
    let endpoint = update
        .host_endpoint
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            value.parse::<SocketAddr>().with_context(|| {
                format!("エンドポイント {value} は IP:ポート形式で指定してください")
            })
        })
        .transpose()?;

    let current = read(config_path)?;
    // (host のみ)DNS 名(ADR-0021)。正規化 → 予約語・重複を検証してから書く
    let dns_name = update
        .dns_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .filter(|_| !current.is_member)
        .map(|input| -> anyhow::Result<String> {
            let label = peercove_core::names::normalize_dns_name(input, true)?;
            let config = Config::load(config_path)?;
            if crate::peers::taken_dns_labels(&config, crate::peers::DnsExclude::Host)
                .contains(&label)
            {
                bail!("DNS 名「{label}」はこのネットワークで既に使用されています");
            }
            Ok(label)
        })
        .transpose()?;
    let mut doc = load_doc(config_path)?;
    let interface = doc
        .get_mut("interface")
        .and_then(|item| item.as_table_like_mut())
        .context("[interface] が見つかりません")?;

    match display_name {
        Some(name) => interface.insert("display_name", toml_edit::value(name)),
        None => interface.remove("display_name"),
    };
    if !current.is_member {
        match &dns_name {
            Some(label) => interface.insert("dns_name", toml_edit::value(label)),
            None => interface.remove("dns_name"),
        };
    }
    match update.listen_port {
        Some(port) => interface.insert("listen_port", toml_edit::value(i64::from(port))),
        None => interface.remove("listen_port"),
    };
    interface.insert("mtu", toml_edit::value(i64::from(update.mtu)));
    // direct は既定(true)なら書かない(設定ファイルを汚さない)
    match update.direct {
        true => interface.remove("direct"),
        false => interface.insert("direct", toml_edit::value(false)),
    };
    // 受信サイズ上限(ADR-0015)も既定(100 MB)なら書かない。
    // 既定以外を書いた設定は旧バージョンの peercove では読めなくなる点に注意
    if update.max_recv_file_mb == peercove_core::config::DEFAULT_MAX_RECV_FILE_MB {
        interface.remove("max_recv_file_mb");
    } else {
        interface.insert(
            "max_recv_file_mb",
            toml_edit::value(i64::try_from(update.max_recv_file_mb).unwrap_or(i64::MAX)),
        );
    }
    if !current.is_member {
        match update.require_invite_approval {
            true => interface.insert("require_invite_approval", toml_edit::value(true)),
            false => interface.remove("require_invite_approval"),
        };
    }

    // endpoint はメンバー設定(ピア 1 つ)のときだけ触る。ホスト設定のピアは
    // メンバーの登録なので、ここから書き換えてはいけない
    if let (true, Some(endpoint)) = (current.is_member, endpoint) {
        let peers = doc
            .get_mut("peer")
            .and_then(|item| item.as_array_of_tables_mut())
            .context("[[peer]] が見つかりません")?;
        let peer = peers.get_mut(0).context("[[peer]] が空です")?;
        peer["endpoint"] = toml_edit::value(endpoint.to_string());
    }

    write_validated(config_path, &doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEMBER: &str = r#"
# 参加設定(コメントは保持されること)
[interface]
private_key_file = "member.key"
display_name = "alice"
address = "10.119.96.2/24"
mtu = 1420

[[peer]]
control_host = "10.119.96.1"
public_key = "hSDwCYkwp1R0i33ctD73Wg2/Og0mOBr06uSpB6ipTmo="
endpoint = "203.0.113.5:51820"
allowed_ips = ["10.119.96.0/24"]
persistent_keepalive = 25
"#;

    const HOST: &str = r#"
[interface]
private_key_file = "host.key"
address = "10.119.96.1/24"
listen_port = 51820
mtu = 1420

[[peer]]
name = "bob"
public_key = "hSDwCYkwp1R0i33ctD73Wg2/Og0mOBr06uSpB6ipTmo="
allowed_ips = ["10.119.96.2/32"]
"#;

    fn write(name: &str, text: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("peercove-settings-{name}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, text).unwrap();
        // private_key_file の実体は read/update では読まないので作らない
        path
    }

    #[test]
    fn reads_member_and_host_shapes() {
        let member = read(&write("read-member", MEMBER)).unwrap();
        assert!(member.is_member);
        assert_eq!(member.host_endpoint.as_deref(), Some("203.0.113.5:51820"));
        assert_eq!(member.display_name.as_deref(), Some("alice"));
        assert_eq!(member.listen_port, None);
        assert!(member.direct, "未指定なら既定 true");

        let host = read(&write("read-host", HOST)).unwrap();
        assert!(!host.is_member, "ホストのピアは endpoint を持たない");
        assert_eq!(host.host_endpoint, None);
        assert_eq!(host.listen_port, Some(51820));
    }

    #[test]
    fn update_member_rewrites_endpoint_and_keeps_comments() {
        let path = write("update-member", MEMBER);
        update(
            &path,
            &Update {
                display_name: Some("  alice2 ".to_string()),
                dns_name: None,
                listen_port: Some(51900),
                mtu: 1380,
                host_endpoint: Some("198.51.100.7:51820".to_string()),
                direct: false,
                max_recv_file_mb: 500,
                require_invite_approval: false,
            },
        )
        .unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# 参加設定"), "コメントが保持される");
        assert!(text.contains("persistent_keepalive = 25"), "他項目は不変");
        assert!(text.contains("direct = false"), "false なら書かれる");
        assert!(
            text.contains("max_recv_file_mb = 500"),
            "既定以外なら書かれる"
        );

        let after = read(&path).unwrap();
        assert_eq!(after.display_name.as_deref(), Some("alice2"), "trim される");
        assert_eq!(after.listen_port, Some(51900));
        assert_eq!(after.mtu, 1380);
        assert_eq!(after.host_endpoint.as_deref(), Some("198.51.100.7:51820"));
        assert!(!after.direct);
        assert_eq!(after.max_recv_file_mb, 500);

        // 既定値に戻すと行ごと消える(既定なので書かない)
        update(
            &path,
            &Update {
                display_name: Some("alice2".to_string()),
                dns_name: None,
                listen_port: Some(51900),
                mtu: 1380,
                host_endpoint: None,
                direct: true,
                max_recv_file_mb: peercove_core::config::DEFAULT_MAX_RECV_FILE_MB,
                require_invite_approval: false,
            },
        )
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("direct"), "既定値なら書かない");
        assert!(!text.contains("max_recv_file_mb"), "既定値なら書かない");
        let after = read(&path).unwrap();
        assert!(after.direct);
        assert_eq!(
            after.max_recv_file_mb,
            peercove_core::config::DEFAULT_MAX_RECV_FILE_MB
        );
    }

    /// ホスト自身の DNS 名(ADR-0021): 正規化して書き込み、空で削除。
    /// メンバー設定では無視される。
    #[test]
    fn update_host_dns_name() {
        let path = write("host-dns", HOST);
        let base = Update {
            display_name: None,
            dns_name: None,
            listen_port: None,
            mtu: 1420,
            host_endpoint: None,
            direct: true,
            max_recv_file_mb: peercove_core::config::DEFAULT_MAX_RECV_FILE_MB,
            require_invite_approval: false,
        };
        update(
            &path,
            &Update {
                dns_name: Some("Game Room".to_string()),
                ..base.clone()
            },
        )
        .unwrap();
        assert_eq!(read(&path).unwrap().dns_name.as_deref(), Some("game-room"));

        // 予約語・メンバー名との重複は拒否(bob は従来導出ラベル)
        assert!(update(
            &path,
            &Update {
                dns_name: Some("localhost".to_string()),
                ..base.clone()
            }
        )
        .is_err());
        assert!(update(
            &path,
            &Update {
                dns_name: Some("bob".to_string()),
                ..base.clone()
            }
        )
        .is_err());

        // 空で削除(従来導出に戻る)
        update(&path, &base).unwrap();
        assert_eq!(read(&path).unwrap().dns_name, None);

        // メンバー設定では無視される(書き込まれない)
        let member = write("member-dns", MEMBER);
        update(
            &member,
            &Update {
                dns_name: Some("my-pc".to_string()),
                mtu: 1420,
                ..base
            },
        )
        .unwrap();
        assert_eq!(read(&member).unwrap().dns_name, None);
    }

    /// ホスト設定では endpoint を渡されても `[[peer]]`(= メンバー登録)を触らない。
    #[test]
    fn update_host_never_touches_peer_endpoint() {
        let path = write("update-host", HOST);
        update(
            &path,
            &Update {
                display_name: None,
                dns_name: None,
                listen_port: None,
                mtu: 1400,
                host_endpoint: Some("198.51.100.7:51820".to_string()),
                direct: true,
                max_recv_file_mb: peercove_core::config::DEFAULT_MAX_RECV_FILE_MB,
                require_invite_approval: false,
            },
        )
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("endpoint"), "ピアに endpoint が生えていない");
        assert!(!text.contains("listen_port"), "None なら削除される");
        assert_eq!(read(&path).unwrap().mtu, 1400);
    }

    #[test]
    fn rejects_bad_values_without_writing() {
        let path = write("reject", MEMBER);
        let base = Update {
            display_name: None,
            dns_name: None,
            listen_port: None,
            mtu: 1420,
            host_endpoint: None,
            direct: true,
            max_recv_file_mb: peercove_core::config::DEFAULT_MAX_RECV_FILE_MB,
            require_invite_approval: false,
        };

        let bad_mtu = Update {
            mtu: 100,
            ..base.clone()
        };
        assert!(update(&path, &bad_mtu)
            .unwrap_err()
            .to_string()
            .contains("MTU"));

        let bad_port = Update {
            listen_port: Some(0),
            ..base.clone()
        };
        assert!(update(&path, &bad_port).is_err());

        let bad_endpoint = Update {
            host_endpoint: Some("203.0.113.5".to_string()),
            ..base.clone()
        };
        assert!(update(&path, &bad_endpoint)
            .unwrap_err()
            .to_string()
            .contains("IP:ポート"));

        // どれも書き込まれていない
        assert_eq!(
            read(&path).unwrap(),
            read(&write("reject-ref", MEMBER)).unwrap()
        );
    }

    #[test]
    fn restart_required_only_for_interface_and_endpoint() {
        let current = read(&write("restart", MEMBER)).unwrap();
        let same = Update {
            display_name: current.display_name.clone(),
            dns_name: current.dns_name.clone(),
            listen_port: current.listen_port,
            mtu: current.mtu,
            host_endpoint: current.host_endpoint.clone(),
            direct: current.direct,
            max_recv_file_mb: current.max_recv_file_mb,
            require_invite_approval: current.require_invite_approval,
        };
        assert!(!current.restart_required(&same));
        assert!(
            !current.restart_required(&Update {
                direct: false,
                ..same.clone()
            }),
            "direct は約 5 秒で反映されるので再起動不要"
        );
        assert!(
            !current.restart_required(&Update {
                max_recv_file_mb: 500,
                ..same.clone()
            }),
            "受信サイズ上限も約 5 秒で反映されるので再起動不要"
        );
        assert!(!current.restart_required(&Update {
            display_name: Some("bob".to_string()),
            ..same.clone()
        }));
        assert!(current.restart_required(&Update {
            mtu: 1380,
            ..same.clone()
        }));
        assert!(current.restart_required(&Update {
            host_endpoint: Some("198.51.100.7:51820".to_string()),
            ..same.clone()
        }));
    }
}
