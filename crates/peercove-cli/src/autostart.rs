//! 起動時の自動接続復旧の台帳(ADR-0057、M6 H-8e)。
//!
//! デーモンは LocalSystem / root で動き、利用者の `networks/` ディレクトリの
//! 場所を知らない(IPC で設定の絶対パスを渡される受け身の設計)。そこで
//! 「一度でも接続に成功した(または UI が明示登録した)設定パスの台帳」を
//! 自分の状態ディレクトリに持ち、次回起動時にそれを読んで**設定ファイル側の
//! `auto_connect` フラグ**を見る(実際に自動接続するかどうかの判断は
//! [`crate::daemon`] 側)。
//!
//! **台帳に秘密は一切書かない**(設定ファイルの絶対パス・role・upnp のみ)。
//! 壊れていても・書き込めなくても致命的エラーにはしない(警告ログを出して
//! 機能だけ無効になる)。開発時にデーモンを非特権で動かすことがあるため。

use std::path::{Path, PathBuf};

use peercove_core::ipc::TunnelRole;
use serde::{Deserialize, Serialize};

/// 台帳ファイル名(状態ディレクトリ直下)。
const FILE_NAME: &str = "autostart.json";

/// 台帳の現行フォーマットバージョン。
const CURRENT_VERSION: u32 = 1;

/// 台帳 1 件。設定ファイルのパスがキー。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutostartEntry {
    pub config: PathBuf,
    pub role: TunnelRole,
    #[serde(default)]
    pub upnp: bool,
}

/// ディスク上の JSON 表現。`deny_unknown_fields` は付けない(将来の
/// additive な拡張を旧デーモンが黙って無視できるようにするため)。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AutostartFile {
    #[serde(default = "current_version")]
    version: u32,
    #[serde(default)]
    entries: Vec<AutostartEntry>,
}

fn current_version() -> u32 {
    CURRENT_VERSION
}

/// 状態ディレクトリ(Windows: `%ProgramData%\PeerCove\` / Unix:
/// `/var/lib/peercove/`)。
#[cfg(windows)]
fn state_dir() -> PathBuf {
    let base = std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
    base.join("PeerCove")
}

#[cfg(unix)]
fn state_dir() -> PathBuf {
    PathBuf::from("/var/lib/peercove")
}

/// 台帳ファイルの既定パス。
pub fn default_path() -> PathBuf {
    state_dir().join(FILE_NAME)
}

/// 台帳を読む。無ければ空。壊れていても警告ログを出すだけで空として扱う
/// (デーモンの起動は止めない)。
pub fn load() -> Vec<AutostartEntry> {
    load_at(&default_path())
}

fn load_at(path: &Path) -> Vec<AutostartEntry> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            tracing::warn!(
                "自動接続台帳を読めません({}): {e}(自動接続は無効になります)",
                path.display()
            );
            return Vec::new();
        }
    };
    match serde_json::from_slice::<AutostartFile>(&bytes) {
        Ok(file) => file.entries,
        Err(e) => {
            tracing::warn!(
                "自動接続台帳の形式が不正です({}): {e}(自動接続は無効になります)",
                path.display()
            );
            Vec::new()
        }
    }
}

/// 接続成功のたびに呼ぶ。同じパスの既存エントリは role/upnp を上書きする。
pub fn upsert(config: &Path, role: TunnelRole, upnp: bool) {
    upsert_at(&default_path(), config, role, upnp);
}

fn upsert_at(path: &Path, config: &Path, role: TunnelRole, upnp: bool) {
    let mut entries = load_at(path);
    match entries.iter_mut().find(|e| e.config == config) {
        Some(entry) => {
            entry.role = role;
            entry.upnp = upnp;
        }
        None => entries.push(AutostartEntry {
            config: config.to_path_buf(),
            role,
            upnp,
        }),
    }
    save_at(path, &entries);
}

/// UI からの明示登録用(IPC `RegisterAutostart`)。一度も接続していない
/// ネットワークでも、設定ダイアログで `auto_connect` を ON にした時点で
/// デーモンが場所を知れるようにする(ADR-0057 決定 2)。upnp は接続実績が
/// ないので既存値を保つ(無ければ false)。
pub fn upsert_keep_upnp(config: &Path, role: TunnelRole) {
    upsert_keep_upnp_at(&default_path(), config, role);
}

fn upsert_keep_upnp_at(path: &Path, config: &Path, role: TunnelRole) {
    let mut entries = load_at(path);
    match entries.iter_mut().find(|e| e.config == config) {
        Some(entry) => entry.role = role,
        None => entries.push(AutostartEntry {
            config: config.to_path_buf(),
            role,
            upnp: false,
        }),
    }
    save_at(path, &entries);
}

/// 設定ファイルが存在しないエントリを台帳から削除する。戻り値は削除後の一覧
/// (呼び出し側がそのまま復旧処理に使えるように)。
pub fn prune_missing() -> Vec<AutostartEntry> {
    prune_missing_at(&default_path())
}

fn prune_missing_at(path: &Path) -> Vec<AutostartEntry> {
    let mut entries = load_at(path);
    let before = entries.len();
    entries.retain(|entry| entry.config.is_file());
    if entries.len() != before {
        save_at(path, &entries);
    }
    entries
}

/// 台帳を書き戻す。ディレクトリ作成・書き込みのどちらが失敗しても警告ログの
/// みで、呼び出し側には伝播しない(非特権実行時に状態ディレクトリへ書けない
/// ことがあるが、機能を無効にするだけで致命的ではないため)。
fn save_at(path: &Path, entries: &[AutostartEntry]) {
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::warn!(
                "自動接続台帳のディレクトリを作れません({}): {e}(自動接続は無効になります)",
                parent.display()
            );
            return;
        }
    }
    let file = AutostartFile {
        version: CURRENT_VERSION,
        entries: entries.to_vec(),
    };
    let json = match serde_json::to_vec_pretty(&file) {
        Ok(json) => json,
        Err(e) => {
            tracing::warn!("自動接続台帳をシリアライズできません: {e}");
            return;
        }
    };
    if let Err(e) = std::fs::write(path, json) {
        tracing::warn!(
            "自動接続台帳を書き込めません({}): {e}(自動接続は無効になります)",
            path.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("peercove-autostart-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        // ディレクトリ自体は save_at が作るので、ここでは掃除だけ
        dir.join("autostart.json")
    }

    #[test]
    fn json_round_trips() {
        let path = temp_path("roundtrip");
        upsert_at(
            &path,
            Path::new("C:/net/a/host.toml"),
            TunnelRole::Host,
            true,
        );
        let entries = load_at(&path);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].config, Path::new("C:/net/a/host.toml"));
        assert_eq!(entries[0].role, TunnelRole::Host);
        assert!(entries[0].upnp);

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("\"role\": \"host\""),
            "role は snake_case: {text}"
        );
        assert!(!text.contains("private"), "秘密は書かない: {text}");
    }

    #[test]
    fn upsert_same_path_does_not_duplicate() {
        let path = temp_path("upsert-dedup");
        upsert_at(&path, Path::new("member.toml"), TunnelRole::Member, false);
        upsert_at(&path, Path::new("member.toml"), TunnelRole::Member, true);
        let entries = load_at(&path);
        assert_eq!(entries.len(), 1, "同じパスは重複しない");
        assert!(entries[0].upnp, "後勝ちで上書きされる");
    }

    #[test]
    fn upsert_keep_upnp_preserves_existing_upnp() {
        let path = temp_path("keep-upnp");
        upsert_at(&path, Path::new("host.toml"), TunnelRole::Host, true);
        upsert_keep_upnp_at(&path, Path::new("host.toml"), TunnelRole::Host);
        let entries = load_at(&path);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].upnp, "upsert_keep_upnp は upnp を変えない");
    }

    #[test]
    fn upsert_keep_upnp_new_entry_defaults_upnp_false() {
        let path = temp_path("keep-upnp-new");
        upsert_keep_upnp_at(&path, Path::new("member.toml"), TunnelRole::Member);
        let entries = load_at(&path);
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].upnp, "接続実績がないので既定 false");
    }

    #[test]
    fn prune_missing_drops_entries_without_files() {
        let dir = std::env::temp_dir().join("peercove-autostart-prune");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let existing = dir.join("exists.toml");
        std::fs::write(&existing, "").unwrap();
        let missing = dir.join("missing.toml");
        let path = dir.join("autostart.json");

        upsert_at(&path, &existing, TunnelRole::Host, false);
        upsert_at(&path, &missing, TunnelRole::Member, false);
        assert_eq!(load_at(&path).len(), 2);

        let remaining = prune_missing_at(&path);
        assert_eq!(
            remaining.len(),
            1,
            "存在しない設定ファイルのエントリは落ちる"
        );
        assert_eq!(remaining[0].config, existing);
        assert_eq!(load_at(&path).len(), 1, "ディスクにも反映される");
    }

    #[test]
    fn load_missing_file_returns_empty() {
        let path = temp_path("no-file");
        assert!(load_at(&path).is_empty());
    }

    #[test]
    fn load_corrupt_file_returns_empty_without_panicking() {
        let path = temp_path("corrupt");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not json").unwrap();
        assert!(load_at(&path).is_empty());
    }

    /// `load()` / `default_path()`(引数なしの公開 API)は読み取りのみで
    /// 副作用がないので、実環境の状態ディレクトリを指していても実害はない
    /// (呼べることの確認 = dead_code 抑止を兼ねる)。
    #[test]
    fn public_load_and_default_path_do_not_panic() {
        let _ = default_path();
        let _ = load();
    }
}
