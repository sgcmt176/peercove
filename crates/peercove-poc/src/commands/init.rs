//! `init`: ホスト初期化のコマンド。ロジックは `peercove-ops`(ADR-0008)。

use std::path::Path;

use anyhow::{bail, Context};

pub fn run(dir: &Path, name: &str, listen_port: u16, force: bool) -> anyhow::Result<()> {
    let key_path = dir.join("host.key");
    let config_path = dir.join("host.toml");
    for path in [&key_path, &config_path] {
        if path.exists() && !force {
            bail!(
                "{} は既に存在します。上書きするには --force を指定してください",
                path.display()
            );
        }
    }

    let result = peercove_ops::init::init_host(dir, name, listen_port, force)
        .context("ホストの初期化に失敗しました")?;

    println!("ホスト設定を初期化しました");
    println!("  ネットワーク名: {}", result.network);
    println!("  設定: {}", result.config_path.display());
    println!(
        "  トンネルサブネット: {}(ホスト = {})",
        result.subnet, result.host_ip
    );
    println!("  待受ポート: UDP {}", result.listen_port);
    println!("  公開鍵: {}", result.public_key);
    println!();
    println!("次の手順:");
    let config = result.config_path.display();
    #[cfg(windows)]
    {
        println!("  1. (管理者ターミナルで) .\\peercove-poc.exe host --config {config}");
        println!("  2. .\\peercove-poc.exe invite --config {config} --name <メンバー名>");
    }
    #[cfg(not(windows))]
    {
        println!("  1. sudo ./peercove-poc host --config {config}");
        println!("  2. ./peercove-poc invite --config {config} --name <メンバー名>");
    }
    Ok(())
}
