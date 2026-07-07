//! ホスト側: メンバー招待トークン(pcv1)の発行(ADR-0005 案 B)。
//!
//! メンバーの鍵ペアと仮想 IP をこの場で生成して host.toml に登録し、
//! 参加に必要な情報一式をトークンにまとめる。トークンは秘密情報のため
//! 既定ではファイルへ保存し、画面表示(--print / --qr)は明示オプトイン。

use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::Path;

use anyhow::{bail, Context};
use peercove_core::config::{Config, DEFAULT_LISTEN_PORT};
use peercove_core::ipalloc::next_free_ip;
use peercove_core::keys::{write_secret_file, PresharedKey, PrivateKey};
use peercove_core::token::{InviteToken, MAX_NAME_LEN};

use super::add_peer::{append_peer, used_ips, NewPeer};

pub struct InviteOptions<'a> {
    pub config_path: &'a Path,
    /// 省略時 `member-<IP 第4オクテット>`
    pub name: Option<&'a str>,
    /// 省略時は空き IP を自動割当
    pub ip: Option<Ipv4Addr>,
    /// 追加のエンドポイント(外部 IP:ポート等)。LAN は自動で先頭に入る
    pub extra_endpoints: &'a [SocketAddrV4],
    /// メンバー用の事前共有鍵を発行する
    pub psk: bool,
    /// トークンの保存先
    pub out: &'a Path,
    pub force: bool,
    pub print: bool,
    pub qr: bool,
}

pub fn run(options: &InviteOptions) -> anyhow::Result<()> {
    let config = Config::load(options.config_path)?;
    let subnet = config.interface.address.trunc();
    let listen_port = config.interface.listen_port.unwrap_or(DEFAULT_LISTEN_PORT);

    if options.out.exists() && !options.force {
        bail!(
            "{} は既に存在します。上書きするには --force を指定してください",
            options.out.display()
        );
    }

    // 割当 IP
    let used: Vec<Ipv4Addr> = used_ips(&config).collect();
    let ip = match options.ip {
        Some(ip) => ip, // 妥当性は append_peer が検証する
        None => next_free_ip(subnet, &used)
            .with_context(|| format!("サブネット {subnet} に空き IP がありません"))?,
    };

    // 表示名
    let name = match options.name {
        Some(name) => name.to_string(),
        None => format!("member-{}", ip.octets()[3]),
    };
    validate_name(&name)?;
    if config
        .peers
        .iter()
        .any(|p| p.name.as_deref() == Some(name.as_str()))
    {
        bail!("名前 {name} のピアは既に存在します(--name で別名を指定してください)");
    }

    // エンドポイント一覧: LAN(自動)→ 追加指定(外部など)の順
    let mut endpoints: Vec<SocketAddrV4> = Vec::new();
    if let Some(std::net::IpAddr::V4(lan_ip)) = crate::upnp::default_route_local_ip() {
        endpoints.push(SocketAddrV4::new(lan_ip, listen_port));
    }
    for ep in options.extra_endpoints {
        if !endpoints.contains(ep) {
            endpoints.push(*ep);
        }
    }
    if endpoints.is_empty() {
        bail!(
            "エンドポイントを決定できませんでした。--endpoint <IP:ポート> で\
             ホストへの到達先を指定してください"
        );
    }

    // メンバーの鍵と(任意)PSK を発行
    let member_private_key = PrivateKey::generate();
    let member_public_key = member_private_key.public_key();
    let preshared_key = options.psk.then(PresharedKey::generate);
    let psk_file_name = format!("peer-{ip}.psk");
    if let Some(psk) = &preshared_key {
        let psk_path = options
            .config_path
            .parent()
            .unwrap_or(Path::new("."))
            .join(&psk_file_name);
        write_secret_file(&psk_path, &format!("{}\n", psk.to_base64()))
            .context("ホスト側 PSK ファイルの保存に失敗しました")?;
        super::restrict_secret_file_acl(&psk_path);
    }

    // host.toml へ登録(実行中の host には約 5 秒で自動反映)
    append_peer(
        options.config_path,
        &NewPeer {
            public_key: member_public_key,
            ip,
            name: Some(&name),
            preshared_key_file: preshared_key.as_ref().map(|_| psk_file_name.as_str()),
        },
    )?;

    // トークンを生成して保存
    let token = InviteToken {
        member_private_key,
        host_public_key: host_public_key(&config)?,
        preshared_key,
        member_address: ipnet::Ipv4Net::new(ip, subnet.prefix_len()).expect("検証済み"),
        host_virtual_ip: config.interface.address.addr(),
        endpoints: endpoints.clone(),
        name: name.clone(),
    };
    let encoded = token.encode()?;
    write_secret_file(options.out, &format!("{encoded}\n"))
        .context("トークンファイルの保存に失敗しました")?;
    super::restrict_secret_file_acl(options.out);

    println!(
        "メンバー {name} を登録し、招待トークンを {} に保存しました",
        options.out.display()
    );
    println!("  割当 IP: {ip}");
    println!(
        "  エンドポイント候補: {}",
        endpoints
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("  PSK: {}", if options.psk { "あり" } else { "なし" });
    println!("トークンは秘密情報です。メンバー本人以外へ渡さず、受け渡し後は削除してください");
    println!("取り消すには remove-peer を使います(M1-G3 で実装予定)");
    if options.print {
        println!();
        println!("{encoded}");
    }
    if options.qr {
        let qr = fast_qr::QRBuilder::new(encoded.as_str())
            .build()
            .map_err(|e| anyhow::anyhow!("QR コードの生成に失敗しました: {e:?}"))?;
        println!();
        println!("{}", qr.to_str());
    }
    Ok(())
}

fn host_public_key(config: &Config) -> anyhow::Result<peercove_core::keys::PublicKey> {
    let private = peercove_core::keys::read_private_key_file(&config.interface.private_key_file)
        .context("ホストの秘密鍵の読み込みに失敗しました(keygen で生成してください)")?;
    Ok(private.public_key())
}

fn validate_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() || name.len() > MAX_NAME_LEN {
        bail!(
            "名前は 1〜{MAX_NAME_LEN} バイトにしてください(実際 {} バイト)",
            name.len()
        );
    }
    // TOML 追記と表示の安全のため制御文字と引用符を拒否する
    if name
        .chars()
        .any(|c| c.is_control() || c == '"' || c == '\\')
    {
        bail!("名前に制御文字・引用符・バックスラッシュは使えません");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST_TOML: &str = r#"
[interface]
private_key_file = "host.key"
address = "100.100.42.1/24"
listen_port = 51820
"#;

    fn setup(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("peercove-invite-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("host.toml");
        std::fs::write(&config, HOST_TOML).unwrap();
        let host_key = PrivateKey::generate();
        write_secret_file(&dir.join("host.key"), &host_key.to_base64()).unwrap();
        config
    }

    fn options<'a>(config: &'a Path, out: &'a Path) -> InviteOptions<'a> {
        InviteOptions {
            config_path: config,
            name: None,
            ip: None,
            extra_endpoints: &[],
            psk: false,
            out,
            force: false,
            print: false,
            qr: false,
        }
    }

    #[test]
    fn invite_registers_peer_and_writes_token() {
        let config_path = setup("basic");
        let out = config_path.parent().unwrap().join("invite.token");
        run(&options(&config_path, &out)).unwrap();

        // host.toml にピアが追加され、名前と IP が入っている
        let config = Config::load(&config_path).unwrap();
        assert_eq!(config.peers.len(), 1);
        assert_eq!(config.peers[0].name.as_deref(), Some("member-2"));
        assert_eq!(
            config.peers[0].allowed_ips[0].to_string(),
            "100.100.42.2/32"
        );

        // トークンが解析でき、登録された公開鍵と一致する
        let token = InviteToken::parse(&std::fs::read_to_string(&out).unwrap()).unwrap();
        assert_eq!(
            token.member_private_key.public_key(),
            config.peers[0].public_key
        );
        assert_eq!(token.member_address.addr().to_string(), "100.100.42.2");
        assert_eq!(token.name, "member-2");
        assert!(!token.endpoints.is_empty());

        // 2 人目は次の空き IP になる
        let out2 = config_path.parent().unwrap().join("invite2.token");
        run(&options(&config_path, &out2)).unwrap();
        let config = Config::load(&config_path).unwrap();
        assert_eq!(
            config.peers[1].allowed_ips[0].addr().to_string(),
            "100.100.42.3"
        );
    }

    #[test]
    fn invite_with_psk_writes_host_side_psk_file() {
        let config_path = setup("psk");
        let out = config_path.parent().unwrap().join("invite.token");
        let mut opts = options(&config_path, &out);
        opts.psk = true;
        run(&opts).unwrap();

        let config = Config::load(&config_path).unwrap();
        let psk_path = config.peers[0].preshared_key_file.as_ref().unwrap();
        let host_psk = peercove_core::keys::read_preshared_key_file(psk_path).unwrap();
        let token = InviteToken::parse(&std::fs::read_to_string(&out).unwrap()).unwrap();
        assert_eq!(
            host_psk.as_bytes(),
            token.preshared_key.unwrap().as_bytes(),
            "ホスト側 PSK とトークン内 PSK が一致しない"
        );
    }

    #[test]
    fn invite_rejects_duplicate_name_and_existing_token_file() {
        let config_path = setup("dup");
        let out = config_path.parent().unwrap().join("invite.token");
        let mut opts = options(&config_path, &out);
        opts.name = Some("alice");
        run(&opts).unwrap();

        // 同名は拒否
        let out2 = config_path.parent().unwrap().join("invite2.token");
        let mut opts2 = options(&config_path, &out2);
        opts2.name = Some("alice");
        assert!(run(&opts2).is_err());

        // 既存トークンファイルへの上書きは --force が必要
        let mut opts3 = options(&config_path, &out);
        opts3.name = Some("bob");
        assert!(run(&opts3).is_err());
        opts3.force = true;
        run(&opts3).unwrap();
    }

    #[test]
    fn validate_name_rules() {
        assert!(validate_name("たろう").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name(&"x".repeat(65)).is_err());
        assert!(validate_name("a\"b").is_err());
        assert!(validate_name("a\nb").is_err());
    }
}
