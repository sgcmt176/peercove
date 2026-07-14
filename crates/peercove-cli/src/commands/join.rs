//! `join`: 招待トークンから参加設定を生成するコマンド。ロジックは `peercove-ops`。

use std::path::Path;

use anyhow::{bail, Context};

pub struct CliOptions<'a> {
    /// トークン文字列(--token)。token_file とどちらか一方
    pub token: Option<&'a str>,
    /// トークンファイル(--token-file)
    pub token_file: Option<&'a Path>,
    pub out_dir: &'a Path,
    pub force: bool,
}

pub fn run(options: &CliOptions) -> anyhow::Result<()> {
    let text = match (options.token, options.token_file) {
        (Some(text), None) => text.to_string(),
        (None, Some(path)) => std::fs::read_to_string(path)
            .with_context(|| format!("{} の読み込みに失敗しました", path.display()))?,
        _ => bail!("--token か --token-file のどちらか一方を指定してください"),
    };
    for path in [
        options.out_dir.join("member.key"),
        options.out_dir.join("member.toml"),
    ] {
        if path.exists() && !options.force {
            bail!(
                "{} は既に存在します。上書きするには --force を指定してください",
                path.display()
            );
        }
    }

    let result = peercove_ops::join::join(&text, options.out_dir, options.force)?;

    println!("参加設定を生成しました({} さん)", result.name);
    println!("  ネットワーク名: {}", result.network);
    println!("  設定: {}", result.config_path.display());
    println!("  割当 IP: {}", result.address);
    if result.other_endpoints.is_empty() {
        println!("  エンドポイント: {}", result.endpoint);
    } else {
        println!(
            "  エンドポイント: {}(他の候補は member.toml のコメント参照)",
            result.endpoint
        );
        println!("  ※ ホストと同じ LAN にいる場合は LAN 側の候補を使ってください");
    }
    println!();
    println!("次の手順で接続します:");
    let config = result.config_path.display();
    #[cfg(windows)]
    println!("  (管理者ターミナルで) .\\peercove.exe member --config {config}");
    #[cfg(not(windows))]
    println!("  sudo ./peercove member --config {config}");
    println!("使い終わったトークン(文字列・ファイル)は削除してください");
    Ok(())
}
