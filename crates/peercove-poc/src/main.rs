mod backend;
mod commands;
mod control;
mod daemon;
mod dns;
mod logbuf;
mod service;
mod upnp;

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "peercove-poc", version, about = "PeerCove CLI / デーモン")]
struct Cli {
    /// ログの詳細度(error/warn/info/debug/trace)。RUST_LOG より優先する
    #[arg(long, global = true, value_name = "LEVEL")]
    log_level: Option<String>,

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
    /// ホストを初期化する(host.key と host.toml を生成、サブネットは自動選択)
    Init {
        /// 出力先ディレクトリ
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        /// ネットワーク名(DNS のサブドメインと表示に使う。英数字とハイフン)
        #[arg(long, default_value = peercove_core::names::DEFAULT_NETWORK_NAME)]
        name: String,
        /// UDP 待受ポート
        #[arg(long, default_value_t = peercove_core::config::DEFAULT_LISTEN_PORT)]
        port: u16,
        /// 既存ファイルを上書きする
        #[arg(long)]
        force: bool,
    },
    /// メンバー招待トークン(pcv1)を発行する(鍵と IP を自動生成して登録)
    Invite {
        #[arg(long)]
        config: PathBuf,
        /// メンバーの表示名(省略時 member-<IP第4オクテット>)
        #[arg(long)]
        name: Option<String>,
        /// 割り当てる仮想 IP(省略時は空きを自動割当)
        #[arg(long)]
        ip: Option<Ipv4Addr>,
        /// 追加のエンドポイント候補(外部 IP:ポート等。複数指定可。LAN は自動)
        #[arg(long = "endpoint")]
        endpoints: Vec<std::net::SocketAddrV4>,
        /// メンバー用の事前共有鍵も発行する
        #[arg(long)]
        psk: bool,
        /// トークンの保存先ファイル
        #[arg(long, default_value = "invite.token")]
        out: PathBuf,
        /// トークン文字列を画面にも表示する(秘密情報なので注意)
        #[arg(long)]
        print: bool,
        /// トークンの QR コードをターミナルに表示する(秘密情報なので注意)
        #[arg(long)]
        qr: bool,
        /// 既存のトークンファイルを上書きする
        #[arg(long)]
        force: bool,
    },
    /// 招待トークンから参加用の鍵と設定を生成する
    Join {
        /// トークン文字列(pcv1.…)
        #[arg(long)]
        token: Option<String>,
        /// トークンが保存されたファイル
        #[arg(long)]
        token_file: Option<PathBuf>,
        /// 出力先ディレクトリ(member.key / member.toml を生成)
        #[arg(long, default_value = ".")]
        out_dir: PathBuf,
        /// 既存ファイルを上書きする
        #[arg(long)]
        force: bool,
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
    /// メンバーを削除する(--name / --pubkey / --ip のいずれかで指定)
    RemovePeer {
        #[arg(long)]
        config: PathBuf,
        /// 表示名で指定
        #[arg(long)]
        name: Option<String>,
        /// 公開鍵(base64)で指定
        #[arg(long)]
        pubkey: Option<String>,
        /// 仮想 IP で指定
        #[arg(long)]
        ip: Option<Ipv4Addr>,
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
    /// デーモン(UI/CLI から IPC で操作する常駐プロセス)の管理
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
}

#[derive(Subcommand)]
enum DaemonAction {
    /// デーモンを起動して常駐する(トンネル操作に管理者/root 権限が必要)
    Run,
    /// (内部用)Windows サービスとして動く。SCM から起動される
    #[command(hide = true)]
    Service,
    /// デーモンを OS サービスとして登録し起動する(Windows サービス / systemd。要管理者/root)
    ServiceInstall,
    /// OS サービスを停止して登録解除する(要管理者/root)
    ServiceUninstall,
    /// デーモンとトンネルの状態を表示する
    Status,
    /// ホストとしてトンネルを開始する
    StartHost {
        #[arg(long)]
        config: PathBuf,
        /// UPnP IGD によるポート自動開放を試行する
        #[arg(long)]
        upnp: bool,
    },
    /// メンバーとしてトンネルを開始する
    StartMember {
        #[arg(long)]
        config: PathBuf,
    },
    /// トンネルを停止する(デーモンは常駐継続)
    Stop {
        /// 停止するネットワークの設定ファイル(1 本だけ稼働中なら省略可)
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// デーモンが保持する直近のログを表示する
    Logs {
        /// 新しい行を待ち続ける(Ctrl+C で終了)
        #[arg(long, short)]
        follow: bool,
    },
    /// デーモンを終了する
    Shutdown,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.log_level.as_deref());
    run(cli.command)
}

/// ログの詳細度: `--log-level` > `RUST_LOG` > 既定(info)。
///
/// パケット 1 個ごとのログは trace なので、既定の info では静かに動く。
///
/// 標準エラー出力に加えてリングバッファへも複製する([`logbuf`])。デーモンの
/// ログを UI から読むための唯一の経路なので、フィルタは両者で共通にする
/// (`--log-level warn` にすると UI のログビューも warn 以上だけになる)。
fn init_tracing(log_level: Option<&str>) {
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::EnvFilter;

    let filter = match log_level {
        Some(level) => EnvFilter::new(level),
        None => EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
    };
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .with(logbuf::RingLayer)
        .init();
}

fn run(command: Command) -> anyhow::Result<()> {
    match command {
        Command::Keygen { out, psk, force } => commands::keygen::run(&out, psk, force),
        Command::Host { config, upnp } => {
            commands::tunnel::run_up(&config, commands::tunnel::Role::Host, upnp)
        }
        Command::Member { config } => {
            commands::tunnel::run_up(&config, commands::tunnel::Role::Member, false)
        }
        Command::Init {
            dir,
            name,
            port,
            force,
        } => commands::init::run(&dir, &name, port, force),
        Command::Invite {
            config,
            name,
            ip,
            endpoints,
            psk,
            out,
            print,
            qr,
            force,
        } => commands::invite::run(&commands::invite::CliOptions {
            config_path: &config,
            name: name.as_deref(),
            ip,
            extra_endpoints: &endpoints,
            psk,
            out: &out,
            force,
            print,
            qr,
        }),
        Command::Join {
            token,
            token_file,
            out_dir,
            force,
        } => commands::join::run(&commands::join::CliOptions {
            token: token.as_deref(),
            token_file: token_file.as_deref(),
            out_dir: &out_dir,
            force,
        }),
        Command::AddPeer { config, pubkey, ip } => commands::add_peer::run(&config, &pubkey, ip),
        Command::RemovePeer {
            config,
            name,
            pubkey,
            ip,
        } => {
            use commands::remove_peer::Selector;
            let selector = match (&name, &pubkey, ip) {
                (Some(name), None, None) => Selector::Name(name),
                (None, Some(key), None) => Selector::PublicKey(key),
                (None, None, Some(ip)) => Selector::Ip(ip),
                _ => {
                    anyhow::bail!("--name / --pubkey / --ip のいずれか 1 つだけを指定してください")
                }
            };
            commands::remove_peer::run(&config, &selector)
        }
        Command::UdpEcho { listen } => commands::udp::run_echo(listen),
        Command::UdpPing { target, count } => commands::udp::run_ping(target, count),
        Command::Status { config } => commands::status::run(&config),
        Command::Down { config } => commands::tunnel::run_down(&config),
        Command::Daemon { action } => run_daemon_action(action),
    }
}

fn run_daemon_action(action: DaemonAction) -> anyhow::Result<()> {
    use peercove_core::ipc::{IpcRequest, IpcResponse};
    // デーモンとクライアントで作業ディレクトリが違うため、パスは絶対にして送る
    let canon = |path: PathBuf| -> anyhow::Result<PathBuf> {
        std::fs::canonicalize(&path)
            .map_err(|e| anyhow::anyhow!("{} が見つかりません: {e}", path.display()))
    };
    match action {
        DaemonAction::Run => daemon::run_server(),
        DaemonAction::Service => {
            #[cfg(windows)]
            {
                service::run_dispatch()
            }
            #[cfg(not(windows))]
            {
                // Linux では systemd が `daemon run` を直接起動する(特別なモード不要)
                anyhow::bail!(
                    "`daemon service` は Windows 専用です。Linux では \
                     `daemon service-install` が systemd に `daemon run` を登録します"
                )
            }
        }
        DaemonAction::ServiceInstall => service::install(),
        DaemonAction::ServiceUninstall => service::uninstall(),
        DaemonAction::Status => {
            if let IpcResponse::Status(status) = daemon::request(IpcRequest::Status)? {
                daemon::print_status(&status);
            }
            Ok(())
        }
        DaemonAction::StartHost { config, upnp } => {
            daemon::request(IpcRequest::StartHost {
                config: canon(config)?,
                upnp,
            })?;
            println!("ホストとしてトンネルを開始しました(daemon status で確認できます)");
            Ok(())
        }
        DaemonAction::StartMember { config } => {
            daemon::request(IpcRequest::StartMember {
                config: canon(config)?,
            })?;
            println!("メンバーとしてトンネルを開始しました(daemon status で確認できます)");
            Ok(())
        }
        DaemonAction::Stop { config } => {
            let config = config.map(canon).transpose()?;
            daemon::request(IpcRequest::Stop { config })?;
            println!("トンネルを停止しました");
            Ok(())
        }
        DaemonAction::Logs { follow } => daemon::print_logs(follow),
        DaemonAction::Shutdown => {
            daemon::request(IpcRequest::Shutdown)?;
            println!("デーモンを終了しました");
            Ok(())
        }
    }
}
