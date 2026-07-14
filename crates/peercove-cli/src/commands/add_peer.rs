//! `add-peer`: 公開鍵を指定してピアを追加する(M0 互換)。
//! ロジックは `peercove-ops::peers`(ADR-0008)。

use std::net::Ipv4Addr;
use std::path::Path;

use anyhow::Context;
use peercove_core::keys::PublicKey;
use peercove_ops::peers::{append_peer, NewPeer};

pub fn run(config_path: &Path, pubkey: &str, ip: Ipv4Addr) -> anyhow::Result<()> {
    let public_key = PublicKey::from_base64(pubkey)
        .context("--pubkey が不正です(base64 の X25519 公開鍵を指定してください)")?;
    append_peer(
        config_path,
        &NewPeer {
            public_key,
            ip,
            name: None,
            dns_name: None,
            preshared_key_file: None,
            invite_id: None,
            invite_issued_at: None,
            invite_expires_at: None,
        },
    )?;
    println!("ピアを追加しました: {public_key} -> {ip}/32");
    println!("実行中の host プロセスには約 5 秒で自動反映されます(再起動不要)");
    Ok(())
}
