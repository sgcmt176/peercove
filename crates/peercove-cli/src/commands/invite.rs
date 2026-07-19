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

    // 稼働中デーモンが UPnP で外部エンドポイントを知っていれば自動で候補に足す
    // (M4 E-C)。デーモン停止・UPnP 無効なら黙って何もしない(手動指定で代替可)
    let mut extra: Vec<SocketAddrV4> = options.extra_endpoints.to_vec();
    if let Some(external) = daemon_external_endpoint(options.config_path) {
        if !extra.contains(&external) {
            println!("デーモンが観測した外部エンドポイント {external} を候補に追加します");
            extra.push(external);
        }
    }

    let result = invite(&InviteOptions {
        config_path: options.config_path,
        name: options.name,
        ip: options.ip,
        extra_endpoints: &extra,
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

/// 稼働中デーモンからこのネットワークの外部エンドポイントを取り出す(ベストエフォート)。
fn daemon_external_endpoint(config_path: &Path) -> Option<SocketAddrV4> {
    let canonical = config_path
        .canonicalize()
        .unwrap_or_else(|_| config_path.to_path_buf());
    match peercove_ipc::request(peercove_core::ipc::IpcRequest::Status) {
        Ok(peercove_core::ipc::IpcResponse::Status(status)) => status
            .tunnels
            .iter()
            .find(|t| {
                t.config == canonical
                    || t.config.canonicalize().ok().as_deref() == Some(canonical.as_path())
            })
            .and_then(|t| t.external_endpoint),
        _ => None,
    }
}
