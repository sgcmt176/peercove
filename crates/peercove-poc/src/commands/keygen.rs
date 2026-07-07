use std::path::Path;

use anyhow::{bail, Context};
use peercove_core::keys::{write_secret_file, PresharedKey, PrivateKey};

pub fn run(out: &Path, psk: bool, force: bool) -> anyhow::Result<()> {
    if out.exists() && !force {
        bail!(
            "{} は既に存在します。上書きするには --force を指定してください",
            out.display()
        );
    }

    // 秘密鍵・PSK の base64 はファイルへのみ書き、標準出力・ログへは出さない。
    if psk {
        let key = PresharedKey::generate();
        write_secret_file(out, &format!("{}\n", key.to_base64()))
            .context("PSK の保存に失敗しました")?;
        restrict_windows_acl(out);
        println!("事前共有鍵(PSK)を {} に保存しました", out.display());
    } else {
        let private = PrivateKey::generate();
        write_secret_file(out, &format!("{}\n", private.to_base64()))
            .context("秘密鍵の保存に失敗しました")?;
        restrict_windows_acl(out);
        println!("秘密鍵を {} に保存しました", out.display());
        println!("公開鍵: {}", private.public_key());
    }
    Ok(())
}

/// Windows では作成したファイルの ACL を「現在のユーザーのみフルコントロール」に
/// 制限する(Unix の 600 相当)。失敗しても処理は続行し、手動対処を促す。
#[cfg(windows)]
fn restrict_windows_acl(path: &Path) {
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
fn restrict_windows_acl(_path: &Path) {}
