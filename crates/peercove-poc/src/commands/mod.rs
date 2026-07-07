pub mod add_peer;
pub mod invite;
pub mod join;
pub mod keygen;
pub mod remove_peer;
pub mod status;
pub mod tunnel;
pub mod udp;

/// Windows では秘密ファイルの ACL を「現在のユーザーのみフルコントロール」に
/// 制限する(Unix の 600 相当。Unix 側は `write_secret_file` が mode 600 を付ける)。
/// 失敗しても処理は続行し、手動対処を促す。
#[cfg(windows)]
pub(crate) fn restrict_secret_file_acl(path: &std::path::Path) {
    let username = match std::env::var("USERNAME") {
        Ok(name) => name,
        Err(_) => {
            tracing::warn!("USERNAME が取得できないため ACL を制限できませんでした");
            return;
        }
    };
    let result = std::process::Command::new("icacls")
        .arg(path)
        .args(["/inheritance:r", "/grant:r", &format!("{username}:F")])
        .output();
    match result {
        Ok(output) if output.status.success() => {}
        _ => tracing::warn!(
            "{} の ACL 制限に失敗しました。エクスプローラーのプロパティ > セキュリティで \
             他ユーザーのアクセス権を削除してください",
            path.display()
        ),
    }
}

#[cfg(not(windows))]
pub(crate) fn restrict_secret_file_acl(_path: &std::path::Path) {}
