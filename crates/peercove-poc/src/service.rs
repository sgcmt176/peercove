//! デーモンの OS サービス統合(M2-G7、ADR-0010)。
//!
//! - Windows: `windows-service` crate によるサービス実装 + SCM への登録。
//!   サービスは **LocalSystem / Session 0** で動く。UI(非特権)からの操作は
//!   既存の名前付きパイプ + SDDL(ADR-0007)がそのまま通る
//! - Linux: systemd ユニットの設置(`packaging/systemd/peercove-daemon.service`
//!   のテンプレートを使用)。デーモン本体に特別なモードは不要で、ユニットが
//!   `daemon run` を起動するだけ
//!
//! MSI / deb はそれぞれのインストーラ機構(WiX ServiceInstall / systemd unit 同梱)
//! で登録するため、ここの install/uninstall は **ZIP 配布(上級者向け)と
//! 開発時の PoC 用**。どちらの経路でも同じサービス名・同じ引数になるよう、
//! 定数はこのモジュールに集約する。

/// サービス名(Windows SCM / systemd で共通)。
pub const SERVICE_NAME: &str = "peercove-daemon";

// ---- Windows ----

#[cfg(windows)]
mod windows_impl {
    use std::ffi::OsString;
    use std::time::Duration;

    use anyhow::Context;
    use windows_service::service::{
        ServiceAccess, ServiceErrorControl, ServiceExitCode, ServiceInfo, ServiceStartType,
        ServiceState, ServiceStatus, ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
    use windows_service::{define_windows_service, service_dispatcher};

    use super::SERVICE_NAME;

    const DISPLAY_NAME: &str = "PeerCove Daemon";
    const DESCRIPTION: &str =
        "PeerCove の常駐デーモン。トンネルの作成・維持・破棄を行います(UI/CLI からローカル IPC で操作)。";
    /// ファイアウォールの受信許可ルール名。install/uninstall で対に管理する。
    /// MSI(G7b)も同じ名前でルールを作る・消すこと(手動追加とも互換)。
    const FIREWALL_RULE: &str = "PeerCove Daemon";

    // SCM が呼ぶ FFI エントリポイントを生成する
    define_windows_service!(ffi_service_main, service_main);

    /// `daemon service`: SCM の制御下でサービスとして動く(SCM 以外から呼ぶと失敗する)。
    pub fn run_dispatch() -> anyhow::Result<()> {
        service_dispatcher::start(SERVICE_NAME, ffi_service_main).context(
            "サービスディスパッチャを開始できません。このコマンドは Windows の\
             サービス制御マネージャーから起動されるものです(手動で常駐させるには\
             `daemon run` を使ってください)",
        )
    }

    fn service_main(_arguments: Vec<OsString>) {
        if let Err(e) = run_service() {
            // サービスの stderr はどこにも繋がっていない。リングバッファには残る
            tracing::error!("サービスの実行に失敗しました: {e:#}");
        }
    }

    /// SCM への状態報告。
    fn report(
        handle: service_control_handler::ServiceStatusHandle,
        state: ServiceState,
        wait_hint: Duration,
        exit_code: u32,
    ) -> windows_service::Result<()> {
        use windows_service::service::ServiceControlAccept;
        handle.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: if state == ServiceState::Running {
                ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
            } else {
                ServiceControlAccept::empty()
            },
            exit_code: ServiceExitCode::Win32(exit_code),
            checkpoint: 0,
            wait_hint,
            process_id: None,
        })
    }

    fn run_service() -> anyhow::Result<()> {
        use std::sync::OnceLock;

        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);

        // ハンドラ(別スレッドで呼ばれる)からも状態報告できるよう、登録後に
        // ハンドルを置く。登録直後〜設定前に Stop が来た場合は PENDING 報告を
        // 諦めるだけで、停止シグナル自体は届く
        static STATUS: OnceLock<service_control_handler::ServiceStatusHandle> = OnceLock::new();

        // SCM からの制御(停止・シャットダウン)を受ける。ハンドラ内では
        // 長い処理をせず、シグナル送信と STOP_PENDING の報告だけを行う。
        // クリーンアップ(トンネル破棄 + UPnP 解放)に数秒かかるため、
        // SCM に「止まろうとしている」と伝えて待ってもらう
        let status_handle = service_control_handler::register(SERVICE_NAME, move |control| {
            use windows_service::service::ServiceControl;
            match control {
                ServiceControl::Stop | ServiceControl::Shutdown => {
                    let _ = stop_tx.send(true);
                    if let Some(handle) = STATUS.get() {
                        let _ = report(
                            *handle,
                            ServiceState::StopPending,
                            Duration::from_secs(30),
                            0,
                        );
                    }
                    ServiceControlHandlerResult::NoError
                }
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                _ => ServiceControlHandlerResult::NotImplemented,
            }
        })
        .context("サービス制御ハンドラの登録に失敗しました")?;
        let _ = STATUS.set(status_handle);

        report(status_handle, ServiceState::Running, Duration::default(), 0)
            .context("サービス状態(Running)の報告に失敗しました")?;
        tracing::info!("Windows サービスとして起動しました");

        // 本体。SCM の Stop / IPC の shutdown どちらでも抜ける
        let result = crate::daemon::serve(Some(stop_rx));

        let exit_code = if result.is_ok() { 0 } else { 1 };
        let _ = report(
            status_handle,
            ServiceState::Stopped,
            Duration::default(),
            exit_code,
        );
        result
    }

    /// サービスを SCM に登録して起動する(要管理者)。
    pub fn install() -> anyhow::Result<()> {
        let manager = ServiceManager::local_computer(
            None::<&str>,
            ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
        )
        .map_err(|e| access_denied_hint(e, "サービスの登録"))?;

        let exe = std::env::current_exe().context("実行ファイルのパスを特定できません")?;
        let info = ServiceInfo {
            name: SERVICE_NAME.into(),
            display_name: DISPLAY_NAME.into(),
            service_type: ServiceType::OWN_PROCESS,
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: exe.clone(),
            launch_arguments: vec!["daemon".into(), "service".into()],
            dependencies: vec![],
            account_name: None, // LocalSystem
            account_password: None,
        };
        let service = manager
            .create_service(&info, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START)
            .map_err(|e| match e {
                windows_service::Error::Winapi(ref io)
                    if io.raw_os_error() == Some(1073) /* ERROR_SERVICE_EXISTS */ =>
                {
                    anyhow::anyhow!(
                        "サービス {SERVICE_NAME} は既に登録されています。\
                         登録し直すには先に `daemon service-uninstall` を実行してください"
                    )
                }
                other => anyhow::anyhow!(other).context("サービスの登録に失敗しました"),
            })?;
        service
            .set_description(DESCRIPTION)
            .context("サービスの説明の設定に失敗しました")?;

        // ファイアウォールの受信許可(UDP)。Session 0 のサービスには
        // 「アクセスを許可しますか?」のダイアログが出ないため、明示的に
        // ルールを作らないと WG のハンドシェイクが黙って遮断される(PoC で発覚)
        add_firewall_rule(&exe);

        let no_args: [&std::ffi::OsStr; 0] = [];
        service.start(&no_args).context(
            "サービスの起動に失敗しました(登録は完了しています。\
             wintun.dll が実行ファイルと同じフォルダにあるか確認してください)",
        )?;
        println!("サービス {SERVICE_NAME} を登録して起動しました(自動起動: 有効)");
        println!("  実行ファイル: {}", exe.display());
        // PowerShell では `sc` が Set-Content のエイリアスなので、必ず sc.exe と書く
        println!("  状態確認: Get-Service {SERVICE_NAME}(または sc.exe query {SERVICE_NAME})");
        println!("  停止: sc.exe stop {SERVICE_NAME}");
        Ok(())
    }

    /// 受信許可ルールを追加する(このプログラム宛の UDP。待受ポートは設定で
    /// 変わるためポート指定はしない)。再インストールで重複しないよう、
    /// 同名ルールを消してから追加する。
    fn add_firewall_rule(exe: &std::path::Path) {
        let _ = std::process::Command::new("netsh")
            .args(["advfirewall", "firewall", "delete", "rule"])
            .arg(format!("name={FIREWALL_RULE}"))
            .output();
        let result = std::process::Command::new("netsh")
            .args(["advfirewall", "firewall", "add", "rule"])
            .arg(format!("name={FIREWALL_RULE}"))
            .args(["dir=in", "action=allow", "protocol=UDP"])
            .arg(format!("program={}", exe.display()))
            .output();
        match result {
            Ok(output) if output.status.success() => {
                println!("ファイアウォールの受信許可(UDP)を追加しました: {FIREWALL_RULE}");
            }
            _ => eprintln!(
                "警告: ファイアウォールルールの追加に失敗しました。メンバーからの接続が\
                 通らない場合は手動で追加してください:\n  netsh advfirewall firewall add rule \
                 name=\"{FIREWALL_RULE}\" dir=in action=allow protocol=UDP program=\"{}\"",
                exe.display()
            ),
        }
    }

    /// install が追加した受信許可ルールを削除する。
    fn remove_firewall_rule() {
        let result = std::process::Command::new("netsh")
            .args(["advfirewall", "firewall", "delete", "rule"])
            .arg(format!("name={FIREWALL_RULE}"))
            .output();
        match result {
            Ok(output) if output.status.success() => {
                println!("ファイアウォールの受信許可ルールを削除しました: {FIREWALL_RULE}");
            }
            // 元々無い場合も delete は失敗コードを返す。残骸ではないので黙る
            _ => {}
        }
    }

    /// サービスを停止して登録解除する(要管理者)。
    pub fn uninstall() -> anyhow::Result<()> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .map_err(|e| access_denied_hint(e, "サービスの登録解除"))?;
        let service = manager
            .open_service(
                SERVICE_NAME,
                ServiceAccess::STOP | ServiceAccess::DELETE | ServiceAccess::QUERY_STATUS,
            )
            .map_err(|e| match e {
                windows_service::Error::Winapi(ref io)
                    if io.raw_os_error() == Some(1060) /* ERROR_SERVICE_DOES_NOT_EXIST */ =>
                {
                    anyhow::anyhow!("サービス {SERVICE_NAME} は登録されていません")
                }
                other => access_denied_hint(other, "サービスの登録解除"),
            })?;

        // 動いていれば止める(トンネルのクリーンアップが走る)
        if service.query_status()?.current_state != ServiceState::Stopped {
            println!("サービスを停止しています(トンネルをクリーンアップ中)…");
            let _ = service.stop();
            let deadline = std::time::Instant::now() + Duration::from_secs(30);
            loop {
                if service.query_status()?.current_state == ServiceState::Stopped {
                    break;
                }
                if std::time::Instant::now() > deadline {
                    anyhow::bail!(
                        "サービスが 30 秒以内に停止しませんでした。\
                         `sc.exe stop {SERVICE_NAME}` で停止してから再実行してください"
                    );
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        }
        service.delete().context("サービスの削除に失敗しました")?;
        remove_firewall_rule();
        println!("サービス {SERVICE_NAME} を登録解除しました");
        Ok(())
    }

    /// アクセス拒否(os error 5)に管理者実行の案内を添える。
    fn access_denied_hint(e: windows_service::Error, action: &str) -> anyhow::Error {
        if let windows_service::Error::Winapi(ref io) = e {
            if io.raw_os_error() == Some(5) {
                return anyhow::anyhow!(
                    "{action}には管理者権限が必要です。管理者として実行した\
                     PowerShell / ターミナルから実行してください"
                );
            }
        }
        anyhow::anyhow!(e).context(format!("{action}に失敗しました"))
    }
}

#[cfg(windows)]
pub use windows_impl::{install, run_dispatch, uninstall};

// ---- Linux(systemd) ----

#[cfg(unix)]
mod unix_impl {
    use anyhow::Context;

    use super::SERVICE_NAME;

    /// ユニットのテンプレート。deb はこのファイルをそのまま同梱する。
    const UNIT_TEMPLATE: &str = include_str!("../../../packaging/systemd/peercove-daemon.service");
    /// テンプレート内のバイナリパス(deb の配置先)。CLI インストール時は
    /// 実行中の exe のパスへ置換する。
    const TEMPLATE_EXEC: &str = "/usr/bin/peercove-poc";

    fn unit_path() -> std::path::PathBuf {
        // /etc は管理者の設置場所(deb は /usr/lib/systemd/system を使い、
        // /etc が優先されるため衝突しても CLI 側が勝つ)
        std::path::PathBuf::from(format!("/etc/systemd/system/{SERVICE_NAME}.service"))
    }

    fn require_root(action: &str) -> anyhow::Result<()> {
        // SAFETY: geteuid は常に安全に呼べる(引数なし・失敗しない)
        if unsafe { libc::geteuid() } != 0 {
            anyhow::bail!("{action}には root 権限が必要です(sudo で実行してください)");
        }
        Ok(())
    }

    fn systemctl(args: &[&str]) -> anyhow::Result<()> {
        let status = std::process::Command::new("systemctl")
            .args(args)
            .status()
            .context("systemctl の実行に失敗しました(systemd の環境ですか?)")?;
        if !status.success() {
            anyhow::bail!("systemctl {} が失敗しました", args.join(" "));
        }
        Ok(())
    }

    /// systemd ユニットを設置して有効化・起動する(要 root)。
    pub fn install() -> anyhow::Result<()> {
        require_root("サービスの登録")?;
        let exe = std::env::current_exe().context("実行ファイルのパスを特定できません")?;
        let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
        let unit = UNIT_TEMPLATE.replace(TEMPLATE_EXEC, &exe.display().to_string());
        let path = unit_path();
        std::fs::write(&path, unit)
            .with_context(|| format!("{} の書き込みに失敗しました", path.display()))?;
        systemctl(&["daemon-reload"])?;
        systemctl(&["enable", "--now", SERVICE_NAME])?;
        println!("サービス {SERVICE_NAME} を登録して起動しました(自動起動: 有効)");
        println!("  ユニット: {}", path.display());
        println!("  実行ファイル: {}", exe.display());
        println!(
            "  状態確認: systemctl status {SERVICE_NAME} / ログ: journalctl -u {SERVICE_NAME}"
        );
        Ok(())
    }

    /// systemd ユニットを停止・無効化して撤去する(要 root)。
    pub fn uninstall() -> anyhow::Result<()> {
        require_root("サービスの登録解除")?;
        let path = unit_path();
        if !path.exists() {
            anyhow::bail!(
                "サービス {SERVICE_NAME} は登録されていません({} が無い)",
                path.display()
            );
        }
        // 停止でトンネルのクリーンアップが走る。失敗しても撤去は続ける
        if let Err(e) = systemctl(&["disable", "--now", SERVICE_NAME]) {
            tracing::warn!("サービスの停止・無効化に失敗しました(撤去は続行): {e:#}");
        }
        std::fs::remove_file(&path)
            .with_context(|| format!("{} の削除に失敗しました", path.display()))?;
        systemctl(&["daemon-reload"])?;
        println!("サービス {SERVICE_NAME} を登録解除しました");
        Ok(())
    }
}

#[cfg(unix)]
pub use unix_impl::{install, uninstall};
