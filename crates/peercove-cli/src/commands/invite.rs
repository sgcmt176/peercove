//! `invite`: 招待トークンの発行コマンド。ロジックは `peercove-ops`(ADR-0008)。
//!
//! トークンは秘密情報のため、既定ではファイルへ保存し、画面表示(--print / --qr)は
//! 明示オプトインにする。

use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::Path;

use anyhow::bail;
use peercove_ops::invite::{invite, InviteOptions};
use peercove_ops::secret::write_secret;

pub struct CliOptions<'a> {
    pub config_path: &'a Path,
    pub name: Option<&'a str>,
    pub ip: Option<Ipv4Addr>,
    pub extra_endpoints: &'a [SocketAddrV4],
    pub psk: bool,
    pub expires_in_secs: Option<u64>,
    /// トークンの保存先
    pub out: &'a Path,
    pub force: bool,
    pub print: bool,
    pub qr: bool,
}

pub fn run(options: &CliOptions) -> anyhow::Result<()> {
    if options.out.exists() && !options.force {
        bail!(
            "{} は既に存在します。上書きするには --force を指定してください",
            options.out.display()
        );
    }

    let result = invite(&InviteOptions {
        config_path: options.config_path,
        name: options.name,
        ip: options.ip,
        extra_endpoints: options.extra_endpoints,
        psk: options.psk,
        expires_in_secs: options.expires_in_secs,
    })?;

    write_secret(options.out, &format!("{}\n", result.token))?;

    println!(
        "メンバー {} を登録し、招待トークンを {} に保存しました",
        result.name,
        options.out.display()
    );
    println!("  割当 IP: {}", result.ip);
    println!(
        "  エンドポイント候補: {}",
        result
            .endpoints
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("  PSK: {}", if result.psk { "あり" } else { "なし" });
    println!(
        "  有効期限: {}",
        result
            .expires_at
            .map(|value| format!("UNIX {value}"))
            .unwrap_or_else(|| "無期限".to_string())
    );
    println!("トークンは秘密情報です。メンバー本人以外へ渡さず、受け渡し後は削除してください");
    println!("取り消すには remove-peer を使います");

    if options.print {
        println!();
        println!("{}", result.token);
    }
    if options.qr {
        let qr = fast_qr::QRBuilder::new(result.token.as_str())
            .build()
            .map_err(|e| anyhow::anyhow!("QR コードの生成に失敗しました: {e:?}"))?;
        println!();
        println!("{}", qr.to_str());
    }
    Ok(())
}
