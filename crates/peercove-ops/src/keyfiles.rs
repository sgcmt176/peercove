//! デバイス鍵ローテーションの鍵ファイル操作(ADR-0020)。
//!
//! デスクトップ(peercove-cli/rotate.rs)とモバイル(peercove-mobile)で共用:
//! - `member.key` … 確定済みの鍵(設定 `private_key_file` が指すファイル)
//! - `member.key.new` … 更新待ちの新鍵。**依頼を送る前に必ず書く**。
//!   ホストへの反映が確認できるまで消さない(締め出し防止)

use std::path::{Path, PathBuf};

use anyhow::Context;
use peercove_core::config::Config;
use peercove_core::keys::{read_private_key_file, write_secret_file, PrivateKey};

/// 更新待ちの新鍵ファイル(`<private_key_file>.new`)。
pub fn pending_path(key_path: &Path) -> PathBuf {
    let mut path = key_path.as_os_str().to_owned();
    path.push(".new");
    PathBuf::from(path)
}

/// 更新待ちの新鍵を読む(無い・読めないなら None)。
pub fn load_pending(key_path: &Path) -> Option<PrivateKey> {
    read_private_key_file(&pending_path(key_path)).ok()
}

/// 更新待ちの新鍵を用意する(あれば再利用 — 依頼の再送を冪等にする)。
pub fn ensure_pending(key_path: &Path) -> anyhow::Result<PrivateKey> {
    if let Some(key) = load_pending(key_path) {
        return Ok(key);
    }
    let key = PrivateKey::generate();
    let path = pending_path(key_path);
    write_secret_file(&path, &format!("{}\n", key.to_base64()))
        .with_context(|| format!("{} を書き込めません", path.display()))?;
    // root デーモンが作るファイルの所有者を、既存の鍵ファイルに合わせる
    // (ユーザーが join し直すときに消せなくならないように)
    #[cfg(unix)]
    if let Ok(meta) = std::fs::metadata(key_path) {
        use std::os::unix::fs::MetadataExt as _;
        let _ = std::os::unix::fs::chown(&path, Some(meta.uid()), Some(meta.gid()));
    }
    Ok(key)
}

/// 更新を確定する: `member.key` を新鍵で上書き → `.new` を削除 →
/// member.toml の key_source を "self" へ。
///
/// 既存ファイルは中身の上書き(所有権・権限を保つ)。key_source の更新に
/// 失敗しても鍵ファイルは一貫しているため警告のみ(次のセッションで
/// もう一度ローテーションが走るだけで、締め出しは起きない)。
pub fn commit_pending(config_path: &Path, key_path: &Path) -> anyhow::Result<()> {
    let pending = pending_path(key_path);
    let key = read_private_key_file(&pending)
        .with_context(|| format!("{} を読めません", pending.display()))?;
    write_secret_file(key_path, &format!("{}\n", key.to_base64()))
        .with_context(|| format!("{} を書き込めません", key_path.display()))?;
    if let Err(e) = std::fs::remove_file(&pending) {
        tracing::warn!("{} の削除に失敗しました: {e}", pending.display());
    }
    if let Err(e) = mark_key_self(config_path) {
        tracing::warn!("member.toml の key_source 更新に失敗しました: {e:#}");
    }
    Ok(())
}

/// 拒否されたときに更新待ちの新鍵を破棄する。
pub fn discard_pending(key_path: &Path) {
    let _ = std::fs::remove_file(pending_path(key_path));
}

/// member.toml の `[interface] key_source` を "self" にする(コメント保持)。
pub fn mark_key_self(config_path: &Path) -> anyhow::Result<()> {
    // 設定保存(UI)など他プロセスの書き込みと直列化する(peercove-ops と同じロック)。
    let _lock = crate::peers::lock_config(config_path)?;
    let text = std::fs::read_to_string(config_path)
        .with_context(|| format!("{} の読み込みに失敗しました", config_path.display()))?;
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .context("member.toml の解析に失敗しました(手編集の構文エラー?)")?;
    doc.get_mut("interface")
        .and_then(|item| item.as_table_mut())
        .context("[interface] が見つかりません")?
        .insert("key_source", toml_edit::value("self"));
    let updated = doc.to_string();
    let _: Config = toml::from_str(&updated).context("編集結果の TOML が不正です")?;
    std::fs::write(config_path, updated)
        .with_context(|| format!("{} の書き込みに失敗しました", config_path.display()))
}
