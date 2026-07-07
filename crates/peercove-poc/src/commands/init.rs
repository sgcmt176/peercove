//! ホストの初期化(M1-6、ADR-0006)。
//!
//! host.key と host.toml を生成する。サブネットは Tailscale の CGNAT レンジ等と
//! 衝突しないランダムな `10.x.y.0/24` を選ぶ(ホスト = .1)。

use std::path::Path;

use anyhow::{bail, Context};
use peercove_core::config::Config;
use peercove_core::ipalloc::random_private_subnet;
use peercove_core::keys::{write_secret_file, PrivateKey};

pub fn run(dir: &Path, listen_port: u16, force: bool) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("{} の作成に失敗しました", dir.display()))?;
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

    let private_key = PrivateKey::generate();
    write_secret_file(&key_path, &format!("{}\n", private_key.to_base64()))
        .context("秘密鍵の保存に失敗しました")?;
    super::restrict_secret_file_acl(&key_path);

    let subnet = random_private_subnet();
    let host_ip = subnet.hosts().next().expect("/24 にはホストがある");
    let config_text = format!(
        "# peercove-poc init により生成\n\
         [interface]\n\
         private_key_file = \"host.key\"\n\
         address = \"{host_ip}/{}\"\n\
         listen_port = {listen_port}\n",
        subnet.prefix_len()
    );
    std::fs::write(&config_path, &config_text)
        .with_context(|| format!("{} の書き込みに失敗しました", config_path.display()))?;
    Config::load(&config_path).context("生成した設定の検証に失敗しました(バグの可能性)")?;

    println!("ホスト設定を初期化しました");
    println!("  設定: {}", config_path.display());
    println!("  トンネルサブネット: {subnet}(ホスト = {host_ip})");
    println!("  待受ポート: UDP {listen_port}");
    println!("  公開鍵: {}", private_key.public_key());
    println!();
    println!("次の手順:");
    #[cfg(windows)]
    {
        println!(
            "  1. (管理者ターミナルで) .\\peercove-poc.exe host --config {}",
            config_path.display()
        );
        println!(
            "  2. .\\peercove-poc.exe invite --config {} --name <メンバー名>",
            config_path.display()
        );
    }
    #[cfg(not(windows))]
    {
        println!(
            "  1. sudo ./peercove-poc host --config {}",
            config_path.display()
        );
        println!(
            "  2. ./peercove-poc invite --config {} --name <メンバー名>",
            config_path.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn out_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("peercove-init-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn init_generates_working_host_config() {
        let dir = out_dir("basic");
        run(&dir, 51820, false).unwrap();

        let config = Config::load(&dir.join("host.toml")).unwrap();
        let octets = config.interface.address.addr().octets();
        assert_eq!(octets[0], 10);
        assert!((64..=127).contains(&octets[1]));
        assert_eq!(octets[3], 1, "ホストは .1");
        assert_eq!(config.interface.listen_port, Some(51820));
        assert!(peercove_core::keys::read_private_key_file(&dir.join("host.key")).is_ok());

        // 上書きガードと --force
        assert!(run(&dir, 51820, false).is_err());
        run(&dir, 51821, true).unwrap();
        let config = Config::load(&dir.join("host.toml")).unwrap();
        assert_eq!(config.interface.listen_port, Some(51821));
    }
}
