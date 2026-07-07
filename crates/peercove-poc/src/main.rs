mod backend;
mod commands;
mod upnp;

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;

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
        Command::Host { config, upnp } => {
            commands::tunnel::run_up(&config, commands::tunnel::Role::Host, upnp)
        }
        Command::Member { config } => {
            commands::tunnel::run_up(&config, commands::tunnel::Role::Member, false)
        }
        Command::AddPeer { config, pubkey, ip } => commands::add_peer::run(&config, &pubkey, ip),
        Command::UdpEcho { listen } => commands::udp::run_echo(listen),
        Command::UdpPing { target, count } => commands::udp::run_ping(target, count),
        Command::Status { config } => commands::status::run(&config),
        Command::Down { config } => commands::tunnel::run_down(&config),
    }
}
