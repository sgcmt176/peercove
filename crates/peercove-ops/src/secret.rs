//! 秘密ファイル(鍵・PSK)の保存とアクセス権制限。

use std::path::Path;

/// 秘密情報をファイルへ保存し、所有者(+ デーモンサービス)のみ読めるようにする。
///
/// Unix はパーミッション 600(`write_secret_file` が付ける)、
/// Windows は ACL を「現在のユーザー + SYSTEM」に制限する。
pub fn write_secret(path: &Path, contents: &str) -> anyhow::Result<()> {
    write_secret_bytes(path, contents.as_bytes())
}

/// バイナリの秘密情報を保存する。暗号化バックアップにも同じ権限制限を適用する。
pub fn write_secret_bytes(path: &Path, contents: &[u8]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        // 保存先が既にシンボリックリンクなら拒否(リンク先へ秘密を書かない)。
        if let Ok(meta) = std::fs::symlink_metadata(path) {
            if meta.file_type().is_symlink() {
                anyhow::bail!(
                    "秘密ファイルの保存先がシンボリックリンクです: {}",
                    path.display()
                );
            }
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(path)?;
        // 既存ファイルを上書きした場合に緩い権限が残らないよう 0600 を再適用。
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        std::io::Write::write_all(&mut file, contents)?;
        restrict_acl(path);
    }
    #[cfg(not(unix))]
    {
        // Windows: 先に空ファイルを作って ACL を制限し、その後で中身を書く。
        // 「継承 ACL のまま秘密内容がディスクに存在する窓」を無くす。icacls が
        // 失敗した場合は緩い権限のまま秘密を残さないよう、ファイルごと削除する。
        std::fs::File::create(path)?;
        if !restrict_acl(path) {
            let _ = std::fs::remove_file(path);
            anyhow::bail!(
                "{} の ACL を制限できなかったため保存を中止しました",
                path.display()
            );
        }
        std::fs::write(path, contents)?;
    }
    Ok(())
}

/// Windows では秘密ファイルの ACL を「現在のユーザー + SYSTEM のみ」に
/// 制限する(Unix の 600 相当)。失敗しても処理は続行し、手動対処を促す。
///
/// SYSTEM(SID S-1-5-18)を含めるのは、デーモンを Windows サービス
/// (LocalSystem)として動かすため(M2-G7、ADR-0010)。これが無いと
/// サービスが秘密鍵を読めずトンネル起動に失敗する。SID 表記なのは
/// 「SYSTEM」というアカウント名が OS の言語でローカライズされるため。
/// ACL 制限に成功したら `true`。失敗時は警告を出し `false` を返す
/// (呼び出し側は秘密を緩い権限のまま残さない判断に使う)。
#[cfg(all(windows, not(test)))]
fn restrict_acl(path: &Path) -> bool {
    let username = match std::env::var("USERNAME") {
        Ok(name) => name,
        Err(_) => {
            tracing::warn!("USERNAME が取得できないため ACL を制限できませんでした");
            return false;
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
        Ok(output) if output.status.success() => true,
        _ => {
            tracing::warn!(
                "{} の ACL 制限に失敗しました。エクスプローラーのプロパティ > セキュリティで \
                 他ユーザーのアクセス権を削除してください",
                path.display()
            );
            false
        }
    }
}

#[cfg(any(not(windows), test))]
fn restrict_acl(_path: &Path) -> bool {
    true
}
