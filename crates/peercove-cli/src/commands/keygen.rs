use std::path::Path;

use anyhow::{bail, Context};
use peercove_core::keys::{PresharedKey, PrivateKey};
use peercove_ops::secret::write_secret;

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
        write_secret(out, &format!("{}\n", key.to_base64())).context("PSK の保存に失敗しました")?;
        println!("事前共有鍵(PSK)を {} に保存しました", out.display());
    } else {
        let private = PrivateKey::generate();
        write_secret(out, &format!("{}\n", private.to_base64()))
            .context("秘密鍵の保存に失敗しました")?;
        println!("秘密鍵を {} に保存しました", out.display());
        println!("公開鍵: {}", private.public_key());
    }
    Ok(())
}
