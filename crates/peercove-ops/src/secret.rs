//! 秘密ファイル(鍵・PSK)の保存とアクセス権制限。

use std::path::Path;

/// 秘密情報をファイルへ保存し、所有者(+ デーモンサービス)のみ読めるようにする。
///
/// Unix はパーミッション 600(`write_secret_file` が付ける)、
/// Windows は ACL を「現在のユーザー + SYSTEM」に制限する。
pub fn write_secret(path: &Path, contents: &str) -> anyhow::Result<()> {
    peercove_core::keys::write_secret_file(path, contents)?;
    restrict_acl(path);
    Ok(())
}

/// Windows では秘密ファイルの ACL を「現在のユーザー + SYSTEM のみ」に
/// 制限する(Unix の 600 相当)。失敗しても処理は続行し、手動対処を促す。
///
/// SYSTEM(SID S-1-5-18)を含めるのは、デーモンを Windows サービス
/// (LocalSystem)として動かすため(M2-G7、ADR-0010)。これが無いと
/// サービスが秘密鍵を読めずトンネル起動に失敗する。SID 表記なのは
/// 「SYSTEM」というアカウント名が OS の言語でローカライズされるため。
#[cfg(windows)]
fn restrict_acl(path: &Path) {
    let username = match std::env::var("USERNAME") {
        Ok(name) => name,
        Err(_) => {
            tracing::warn!("USERNAME が取得できないため ACL を制限できませんでした");
            return;
        }
    };
    let result = std::process::Command::new("icacls")
        .arg(path)
        .args([
            "/inheritance:r",
            "/grant:r",
            &format!("{username}:F"),
            "/grant:r",
            "*S-1-5-18:F",
        ])
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
fn restrict_acl(_path: &Path) {}
