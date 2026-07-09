//! 複数ネットワークの設定配置(ADR-0012 §2)。
//!
//! 各ネットワークは `<基準ディレクトリ>/networks/<スラッグ>/` に置く。
//! ディレクトリ内のファイル名は従来どおり(host.toml / member.toml と鍵)で、
//! **どちらのファイルがあるかで役割を判別する**(config.toml へ改名しない —
//! 移行が純粋な移動で済み、既存ツール・ドキュメントとの互換も保てるため)。
//!
//! 旧配置(基準ディレクトリ直下の host.toml / member.toml)は
//! [`migrate_legacy`] が networks/ へ移す。

use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use ipnet::Ipv4Net;
use peercove_core::config::Config;
use peercove_core::names::{self, DEFAULT_NETWORK_NAME};

pub const NETWORKS_DIR: &str = "networks";
pub const HOST_FILE: &str = "host.toml";
pub const MEMBER_FILE: &str = "member.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Host,
    Member,
}

/// networks/ 配下の 1 ネットワーク。
#[derive(Debug, Clone)]
pub struct NetworkEntry {
    /// ディレクトリ名でもある正規化済みの名前
    pub slug: String,
    /// 設定に書かれたネットワーク名(通常 slug と一致)
    pub name: String,
    pub role: Role,
    pub dir: PathBuf,
    pub config_path: PathBuf,
    pub address: Ipv4Net,
}

/// networks/ ディレクトリの場所。
pub fn networks_dir(base: &Path) -> PathBuf {
    base.join(NETWORKS_DIR)
}

/// ネットワーク用ディレクトリ(`networks/<スラッグ>/`)。名前は表示名のままで
/// よく、ここで正規化する。作成はしない。
pub fn network_dir(base: &Path, name: &str) -> anyhow::Result<(String, PathBuf)> {
    let Some(slug) = names::dns_label(name) else {
        bail!(
            "ネットワーク名 \"{name}\" から有効な名前を作れませんでした。\
             半角英数字を含む名前にしてください"
        );
    };
    let dir = networks_dir(base).join(&slug);
    Ok((slug, dir))
}

/// join の書き込み先(`networks/<スラッグ>/`)。
///
/// 同じスラッグのディレクトリに**ホスト設定が既に居る**場合(自分がホストする
/// ネットワークと同名のネットワークへ参加するケース)は `-2`, `-3`, …で
/// 別ディレクトリに逃がす。member.toml が既に居る場合はそのまま返し、
/// 上書きの可否は join 側のガード(force)に委ねる(再参加のため)。
pub fn join_dir(base: &Path, name: &str) -> anyhow::Result<(String, PathBuf)> {
    let (slug, dir) = network_dir(base, name)?;
    if !dir.join(HOST_FILE).exists() {
        return Ok((slug, dir));
    }
    for i in 2.. {
        let candidate = format!("{slug}-{i}");
        let dir = networks_dir(base).join(&candidate);
        if !dir.join(HOST_FILE).exists() {
            return Ok((candidate, dir));
        }
    }
    unreachable!("上のループは必ず返る");
}

/// networks/ 配下を走査して一覧を返す(スラッグ順)。
///
/// 設定が壊れているディレクトリは飛ばす(結果に含めない)。壊れた設定で
/// 一覧全体が失敗すると UI が何も出せなくなるため。
pub fn list(base: &Path) -> Vec<NetworkEntry> {
    let mut entries = Vec::new();
    let Ok(read_dir) = std::fs::read_dir(networks_dir(base)) else {
        return entries; // networks/ 自体が無い = ネットワークなし
    };
    for entry in read_dir.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(slug) = dir.file_name().and_then(|n| n.to_str()).map(String::from) else {
            continue;
        };
        let (config_path, role) = match (dir.join(HOST_FILE), dir.join(MEMBER_FILE)) {
            (host, _) if host.exists() => (host, Role::Host),
            (_, member) if member.exists() => (member, Role::Member),
            _ => continue,
        };
        let Ok(config) = Config::load(&config_path) else {
            continue;
        };
        entries.push(NetworkEntry {
            name: config.network_name().to_string(),
            address: config.interface.address,
            slug,
            dir,
            config_path,
            role,
        });
    }
    entries.sort_by(|a, b| a.slug.cmp(&b.slug));
    entries
}

/// 旧配置(base 直下の host.toml / member.toml)を networks/ へ移す。
/// 移した設定ファイルの新パスを返す(何もなければ空)。
///
/// - 移動先スラッグは設定の network_name(旧設定は既定名)。既に同名の
///   ディレクトリがある場合は `-2`, `-3`, …で一意化する
/// - 鍵・PSK は設定からの相対参照なので、**ディレクトリごと純粋に移動**すれば
///   参照は壊れない(host.key / member.key / member.psk / peer-*.psk)
/// - 稼働中のトンネルがある状態で移すと、デーモンの定期再読込が旧パスを
///   見失う。呼び出し側は切断中に呼ぶこと(README に記載)
pub fn migrate_legacy(base: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut moved = Vec::new();
    for (config_file, companions) in [
        (HOST_FILE, vec!["host.key"]),
        (MEMBER_FILE, vec!["member.key", "member.psk"]),
    ] {
        let old_config = base.join(config_file);
        if !old_config.exists() {
            continue;
        }
        let config = Config::load(&old_config).with_context(|| {
            format!("{} の読み込みに失敗しました(移行前)", old_config.display())
        })?;
        let slug = unique_slug(base, config.network_name());
        let dir = networks_dir(base).join(&slug);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("{} の作成に失敗しました", dir.display()))?;

        // 設定本体 + 付随ファイル(鍵・PSK)を移動
        let new_config = dir.join(config_file);
        rename(&old_config, &new_config)?;
        for name in companions {
            let old = base.join(name);
            if old.exists() {
                rename(&old, &dir.join(name))?;
            }
        }
        if config_file == HOST_FILE {
            // invite が発行したメンバー PSK(peer-<IP>.psk)も一緒に移す
            for entry in std::fs::read_dir(base).into_iter().flatten().flatten() {
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                if name.starts_with("peer-") && name.ends_with(".psk") {
                    rename(&entry.path(), &dir.join(name))?;
                }
            }
        }
        moved.push(new_config);
    }
    Ok(moved)
}

/// 既存の networks/ ディレクトリと重複しないスラッグを選ぶ。
fn unique_slug(base: &Path, name: &str) -> String {
    let slug = names::dns_label(name).unwrap_or_else(|| DEFAULT_NETWORK_NAME.to_string());
    if !networks_dir(base).join(&slug).exists() {
        return slug;
    }
    for i in 2.. {
        let candidate = format!("{slug}-{i}");
        if !networks_dir(base).join(&candidate).exists() {
            return candidate;
        }
    }
    unreachable!("上のループは必ず返る");
}

fn rename(from: &Path, to: &Path) -> anyhow::Result<()> {
    std::fs::rename(from, to)
        .with_context(|| format!("{} → {} の移動に失敗しました", from.display(), to.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("peercove-ops-networks-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn list_returns_networks_with_roles() {
        let base = base("list");
        let (_, host_dir) = network_dir(&base, "Game LAN").unwrap();
        crate::init::init_host(&host_dir, "Game LAN", 51820, false).unwrap();

        let entries = list(&base);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].slug, "game-lan");
        assert_eq!(entries[0].name, "game-lan");
        assert_eq!(entries[0].role, Role::Host);
        assert!(entries[0].config_path.ends_with(HOST_FILE));

        // 空ディレクトリや壊れた設定は無視される
        std::fs::create_dir_all(networks_dir(&base).join("empty")).unwrap();
        let broken = networks_dir(&base).join("broken");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(broken.join(HOST_FILE), "not toml [").unwrap();
        assert_eq!(list(&base).len(), 1);
    }

    #[test]
    fn migrate_moves_legacy_host() {
        let base = base("migrate");
        // 旧配置: base 直下に host 一式(network_name なし = 旧設定)
        let result = crate::init::init_host(&base, DEFAULT_NETWORK_NAME, 51820, false).unwrap();
        // 旧設定を再現するため network_name 行を消す
        let text = std::fs::read_to_string(&result.config_path).unwrap();
        std::fs::write(
            &result.config_path,
            text.replace(&format!("network_name = \"{DEFAULT_NETWORK_NAME}\"\n"), ""),
        )
        .unwrap();
        // invite の PSK ファイルも置いておく(移動されることの確認用)
        std::fs::write(base.join("peer-10.68.1.2.psk"), "x\n").unwrap();

        let moved = migrate_legacy(&base).unwrap();
        assert_eq!(moved.len(), 1);
        assert!(!base.join(HOST_FILE).exists());
        assert!(!base.join("host.key").exists());
        assert!(!base.join("peer-10.68.1.2.psk").exists());

        let entries = list(&base);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].slug, DEFAULT_NETWORK_NAME);
        // 鍵の相対参照が壊れていない(load が解決に成功する)
        let config = Config::load(&entries[0].config_path).unwrap();
        assert!(config.interface.private_key_file.exists());
        assert!(entries[0].dir.join("peer-10.68.1.2.psk").exists());

        // 何も残っていない 2 回目は no-op
        assert!(migrate_legacy(&base).unwrap().is_empty());
    }

    #[test]
    fn migrate_moves_legacy_member() {
        let base = base("migrate-member");
        // 旧バイナリの join が書いた member 一式を再現(network_name 行なし)
        let token = peercove_core::token::InviteToken {
            member_private_key: peercove_core::keys::PrivateKey::generate(),
            host_public_key: peercove_core::keys::PrivateKey::generate().public_key(),
            preshared_key: Some(peercove_core::keys::PresharedKey::generate()),
            member_address: "10.100.42.5/24".parse().unwrap(),
            host_virtual_ip: "10.100.42.1".parse().unwrap(),
            endpoints: vec!["203.0.113.5:51820".parse().unwrap()],
            name: "carol".to_string(),
            network: None,
        };
        let result = crate::join::join(&token.encode().unwrap(), &base, false).unwrap();
        let text = std::fs::read_to_string(&result.config_path).unwrap();
        std::fs::write(
            &result.config_path,
            text.replace(&format!("network_name = \"{DEFAULT_NETWORK_NAME}\"\n"), ""),
        )
        .unwrap();

        migrate_legacy(&base).unwrap();
        assert!(!base.join(MEMBER_FILE).exists());
        assert!(!base.join("member.key").exists());
        assert!(!base.join("member.psk").exists());

        let entries = list(&base);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].role, Role::Member);
        assert_eq!(entries[0].slug, DEFAULT_NETWORK_NAME);
        let config = Config::load(&entries[0].config_path).unwrap();
        assert!(config.interface.private_key_file.exists());
        assert!(config.peers[0]
            .preshared_key_file
            .as_ref()
            .unwrap()
            .exists());
    }

    #[test]
    fn join_dir_avoids_hosted_directory() {
        let base = base("join-dir");
        // 未使用なら素直にそのまま
        let (slug, dir) = join_dir(&base, "Game LAN").unwrap();
        assert_eq!(slug, "game-lan");
        assert!(dir.ends_with(Path::new("networks").join("game-lan")));

        // 同名でホスト中なら -2 に逃がす。member 既存なら再利用(再参加)
        crate::init::init_host(&dir, "game-lan", 51820, false).unwrap();
        let (slug2, dir2) = join_dir(&base, "game-lan").unwrap();
        assert_eq!(slug2, "game-lan-2");
        std::fs::create_dir_all(&dir2).unwrap();
        std::fs::write(dir2.join(MEMBER_FILE), "").unwrap();
        assert_eq!(join_dir(&base, "game-lan").unwrap().0, "game-lan-2");
    }

    #[test]
    fn migrate_uniquifies_slug_collision() {
        let base = base("collide");
        // 既に networks/home がある状態で、旧配置の host.toml(既定名)を移行する
        let (_, dir) = network_dir(&base, DEFAULT_NETWORK_NAME).unwrap();
        crate::init::init_host(&dir, DEFAULT_NETWORK_NAME, 51820, false).unwrap();
        crate::init::init_host(&base, DEFAULT_NETWORK_NAME, 51821, false).unwrap();

        let moved = migrate_legacy(&base).unwrap();
        assert_eq!(moved.len(), 1);
        assert!(moved[0]
            .display()
            .to_string()
            .contains(&format!("{DEFAULT_NETWORK_NAME}-2")));
        assert_eq!(list(&base).len(), 2);
    }
}
