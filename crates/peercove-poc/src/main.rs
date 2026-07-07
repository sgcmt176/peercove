mod commands;

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use anyhow::bail;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "peercove-poc", version, about = "PeerCove M0 技術検証 CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// X25519 鍵ペア(または --psk で事前共有鍵)を生成してファイルへ保存する
    Keygen {
        /// 保存先ファイル
        #[arg(long, default_value = "peercove.key")]
        out: PathBuf,
        /// 鍵ペアではなく事前共有鍵(PSK)を生成する
        #[arg(long)]
        psk: bool,
        /// 既存ファイルを上書きする
        #[arg(long)]
        force: bool,
    },
    /// ホストとしてトンネルを作成し、メンバーの接続を待ち受ける
    Host {
        #[arg(long)]
        config: PathBuf,
        /// UPnP IGD によるポート自動開放を試行する
        #[arg(long)]
        upnp: bool,
    },
    /// メンバーとしてトンネルを作成し、ホストへ接続する
    Member {
        #[arg(long)]
        config: PathBuf,
    },
    /// ホスト設定にメンバーピアを追加する(AllowedIPs = <ip>/32)
    AddPeer {
        #[arg(long)]
        config: PathBuf,
        /// メンバーの公開鍵(base64)
        #[arg(long)]
        pubkey: String,
        /// メンバーへ割り当てる仮想 IP
        #[arg(long)]
        ip: Ipv4Addr,
    },
    /// UDP echo サーバー(G-5 検証用)
    UdpEcho {
        #[arg(long, default_value = "0.0.0.0:9999")]
        listen: SocketAddr,
    },
    /// UDP 疎通クライアント。RTT を表示する(G-5 検証用)
    UdpPing {
        #[arg(long)]
        target: SocketAddr,
        /// 送信回数
        #[arg(long, default_value_t = 5)]
        count: u32,
    },
    /// ピア一覧・最終ハンドシェイク・転送量を表示する
    Status {
        #[arg(long)]
        config: PathBuf,
    },
    /// トンネル・ルート・フォワーディング設定をクリーンアップする
    Down {
        #[arg(long)]
        config: PathBuf,
    },
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Keygen { out, psk, force } => commands::keygen::run(&out, psk, force),
        Command::Host { config, upnp: _ } => not_yet(&config, "host", "G-1"),
        Command::Member { config } => not_yet(&config, "member", "G-1"),
        Command::AddPeer { .. } => bail!("add-peer は G-2 で実装予定です"),
        Command::UdpEcho { .. } | Command::UdpPing { .. } => {
            bail!("udp-echo / udp-ping は G-5 で実装予定です")
        }
        Command::Status { config } => not_yet(&config, "status", "G-2"),
        Command::Down { config } => not_yet(&config, "down", "G-1"),
    }
}

/// 未実装コマンドの仮実装。設定の読み込み・検証だけ行い、内容を報告して終了する。
fn not_yet(config_path: &Path, name: &str, goal: &str) -> anyhow::Result<()> {
    let config = peercove_core::config::Config::load(config_path)?;
    println!(
        "設定 OK: interface={} address={} mtu={} peers={}",
        config.interface.name,
        config.interface.address,
        config.interface.mtu,
        config.peers.len()
    );
    bail!("{name} のトンネル操作は {goal} で実装予定です(設定の検証のみ行いました)");
}
