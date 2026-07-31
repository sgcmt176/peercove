//! `backup`: 暗号化バックアップの CLI 化(ADR-0059 決定 5、M7 J-1a)。
//!
//! VPS 上のホストを headless で常時稼働させる運用を想定し、cron からの
//! 定期実行を主な用途とする。ロジックは既存の `peercove_ops::backup`
//! (ADR-0034)をそのまま呼ぶだけで、この module は引数解決と表示だけを持つ。
//!
//! パスフレーズはコマンドライン引数では受け取らない(`ps` やシェル履歴に
//! 露出するため)。環境変数 `PEERCOVE_BACKUP_PASSPHRASE` か
//! `--passphrase-file <path>` のいずれかで渡す(UI 版バックアップには無い
//! 制約)。

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context};
use peercove_core::ipc::{IpcRequest, IpcResponse};
use peercove_ops::backup::{BackupPreview, BackupRole, RestoreMode};

use crate::commands::status::human_bytes;
use crate::daemon;

/// `backup create`
pub fn create(
    config: &Path,
    out: &Path,
    passphrase_file: Option<&Path>,
    include_memos: bool,
    force: bool,
) -> anyhow::Result<()> {
    let passphrase = resolve_passphrase(passphrase_file)?;
    let output_path = if out.is_dir() {
        let network = peercove_core::config::Config::load(config)
            .context("設定ファイルを読み込めません")?
            .network_name()
            .to_string();
        out.join(format!(
            "peercove-backup-{network}-{}.pcvbackup",
            utc_timestamp()
        ))
    } else {
        out.to_path_buf()
    };
    if output_path.exists() && !force {
        bail!(
            "{} は既に存在します。上書きするには --force を指定してください",
            output_path.display()
        );
    }
    let result = peercove_ops::backup::create(config, &output_path, &passphrase, include_memos)
        .context("バックアップの作成に失敗しました")?;
    let size = std::fs::metadata(&result.output_path)
        .map(|m| m.len())
        .unwrap_or(0);
    println!(
        "バックアップを作成しました: {}({})",
        result.output_path.display(),
        human_bytes(size)
    );
    print_preview(&result.preview);
    Ok(())
}

/// `backup inspect`
pub fn inspect(file: &Path, passphrase_file: Option<&Path>) -> anyhow::Result<()> {
    let passphrase = resolve_passphrase(passphrase_file)?;
    let preview = peercove_ops::backup::inspect(file, &passphrase)
        .context("バックアップを読み取れませんでした")?;
    print_preview(&preview);
    Ok(())
}

/// `backup restore`
pub fn restore(
    file: &Path,
    dir: &Path,
    name: &str,
    replace: bool,
    passphrase_file: Option<&Path>,
) -> anyhow::Result<()> {
    let passphrase = resolve_passphrase(passphrase_file)?;
    // ネットワーク名からディレクトリ名(スラッグ)を作る規約は invite/join と共通
    // (`peercove_ops::networks::network_dir` と同じ変換・同じエラー文言)
    let Some(slug) = peercove_core::names::dns_label(name) else {
        bail!(
            "ネットワーク名 \"{name}\" から有効な名前を作れませんでした。\
             半角英数字を含む名前にしてください"
        );
    };
    let target = peercove_ops::networks::networks_dir(dir).join(&slug);
    ensure_not_active(&target)?;
    let mode = if replace {
        RestoreMode::Replace
    } else {
        RestoreMode::New
    };
    let result = peercove_ops::backup::restore(file, &passphrase, dir, &slug, mode)
        .context("バックアップの復元に失敗しました")?;
    println!(
        "復元しました: {}(設定 {})",
        slug,
        result.config_path.display()
    );
    print_preview(&result.preview);
    Ok(())
}

/// 復元対象のネットワークが稼働中なら拒否する(UI 版 `restore_backup` と同じ判定)。
/// デーモンが起動していない、または応答が得られない場合は「稼働していない」
/// とみなして復元を許可する(cron からの実行はデーモン未起動が普通のため)。
fn ensure_not_active(target: &Path) -> anyhow::Result<()> {
    let Ok(IpcResponse::Status(status)) = daemon::request(IpcRequest::Status) else {
        return Ok(());
    };
    let target_canon = std::fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());
    let active = status.tunnels.iter().any(|tunnel| {
        let parent = tunnel.config.parent().unwrap_or(Path::new(""));
        let parent_canon = std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
        parent_canon == target_canon
    });
    if active {
        bail!("稼働中のネットワークは復元できません。先に切断してください(`peercove daemon stop --config <path>`)");
    }
    Ok(())
}

/// パスフレーズの解決: `--passphrase-file` があればそちらを優先し(前後の
/// 空白は trim)、無ければ環境変数 `PEERCOVE_BACKUP_PASSPHRASE`。どちらも
/// 無ければ分かりやすいエラーにする。パスフレーズ自体はログ・標準出力へ
/// 一切出さない。
fn resolve_passphrase(passphrase_file: Option<&Path>) -> anyhow::Result<String> {
    if let Some(path) = passphrase_file {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("{} を読み込めません", path.display()))?;
        let trimmed = text.trim();
        if trimmed.is_empty() {
            bail!("{} の中身が空です", path.display());
        }
        return Ok(trimmed.to_string());
    }
    std::env::var("PEERCOVE_BACKUP_PASSPHRASE").map_err(|_| {
        anyhow::anyhow!(
            "パスフレーズが指定されていません。環境変数 PEERCOVE_BACKUP_PASSPHRASE か \
             --passphrase-file <path> のいずれかで指定してください \
             (両方指定した場合は --passphrase-file を優先します)"
        )
    })
}

fn print_preview(preview: &BackupPreview) {
    let role = match preview.role {
        BackupRole::Host => "ホスト",
        BackupRole::Member => "メンバー",
    };
    println!("  ネットワーク名: {}", preview.network_name);
    println!("  役割: {role}");
    println!("  作成元 OS: {}", preview.source_os);
    println!(
        "  作成日時: {}",
        format_unix_ms_utc(preview.created_at_unix_ms)
    );
    println!("  設定ファイル: {}", preview.config_file);
    println!("  含まれる項目: {}", preview.categories.join(", "));
    if preview.member_key_rotation_recommended {
        println!("  ※ メンバーのバックアップです。復元後は鍵のローテーションを推奨します");
    }
}

/// UTC の "YYYYMMDD-HHMMSS"(既存の日付フォーマット用クレートを増やさず、
/// エポック秒から手計算する。アルゴリズムは Howard Hinnant の
/// civil_from_days、パブリックドメイン)。
fn utc_timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format_unix_secs_utc(secs)
}

fn format_unix_ms_utc(unix_ms: u64) -> String {
    format_unix_secs_utc(unix_ms / 1000)
}

fn format_unix_secs_utc(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}{mo:02}{d:02}-{h:02}{m:02}{s:02}")
}

/// エポック(1970-01-01)からの経過日数 → (年, 月, 日)。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // PEERCOVE_BACKUP_PASSPHRASE はプロセス全体で共有されるため、これを
    // 触るテスト同士は直列化する(peercove-ipc の SOCKET_ENV テストと同じ対策)。
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn base(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "peercove-cli-backup-{name}-{}-{}",
            std::process::id(),
            utc_timestamp()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn resolve_passphrase_prefers_file_over_env_and_trims() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = base("resolve");
        let file = dir.join("pass.txt");
        std::fs::write(&file, "  from-file-passphrase  \n").unwrap();

        // 環境変数だけ: それを使う
        std::env::set_var("PEERCOVE_BACKUP_PASSPHRASE", "from-env-passphrase");
        assert_eq!(
            resolve_passphrase(None).unwrap(),
            "from-env-passphrase".to_string()
        );

        // 両方あれば --passphrase-file を優先する
        assert_eq!(
            resolve_passphrase(Some(&file)).unwrap(),
            "from-file-passphrase".to_string()
        );

        // どちらも無ければ分かりやすいエラー
        std::env::remove_var("PEERCOVE_BACKUP_PASSPHRASE");
        let error = resolve_passphrase(None).unwrap_err().to_string();
        assert!(error.contains("PEERCOVE_BACKUP_PASSPHRASE"));
        assert!(error.contains("--passphrase-file"));
    }

    #[test]
    fn resolve_passphrase_rejects_empty_file() {
        let dir = base("resolve-empty");
        let file = dir.join("empty.txt");
        std::fs::write(&file, "   \n").unwrap();
        assert!(resolve_passphrase(Some(&file)).is_err());
    }

    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(format_unix_secs_utc(0), "19700101-000000");
        // 2024-03-01 00:00:00 UTC = 1709251200
        assert_eq!(format_unix_secs_utc(1_709_251_200), "20240301-000000");
    }

    #[test]
    fn create_inspect_restore_round_trip() {
        let _guard = ENV_LOCK.lock().unwrap();
        let source = base("roundtrip-source");
        let (_, network) = peercove_ops::networks::network_dir(&source, "cli-backup-test").unwrap();
        let initialized =
            peercove_ops::init::init_host(&network, "cli-backup-test", 51820, false).unwrap();

        std::env::set_var("PEERCOVE_BACKUP_PASSPHRASE", "correct horse battery staple");

        let out_dir = base("roundtrip-out");
        create(&initialized.config_path, &out_dir, None, false, false).unwrap();
        let mut entries: Vec<_> = std::fs::read_dir(&out_dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(entries.len(), 1);
        let backup_file = entries.pop().unwrap();
        assert!(backup_file
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("peercove-backup-cli-backup-test-"));

        // 既存ファイルへ --force なしでは作れない
        assert!(create(&initialized.config_path, &backup_file, None, false, false).is_err());
        // --force なら上書きできる
        create(&initialized.config_path, &backup_file, None, false, true).unwrap();

        inspect(&backup_file, None).unwrap();

        let destination = base("roundtrip-destination");
        restore(&backup_file, &destination, "restored-cli", false, None).unwrap();
        let restored_config = peercove_ops::networks::networks_dir(&destination)
            .join("restored-cli")
            .join("host.toml");
        assert!(restored_config.exists());
        let config = peercove_core::config::Config::load(&restored_config).unwrap();
        assert_eq!(config.network_name(), "restored-cli");

        std::env::remove_var("PEERCOVE_BACKUP_PASSPHRASE");
    }
}
