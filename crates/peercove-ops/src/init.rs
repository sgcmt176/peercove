//! ホストの初期化(ADR-0006)。host.key と host.toml を生成する。

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use ipnet::Ipv4Net;
use peercove_core::config::Config;
use peercove_core::ipalloc::random_private_subnet;
use peercove_core::keys::{PrivateKey, PublicKey};
use peercove_core::names;

pub struct InitResult {
    pub config_path: PathBuf,
    pub key_path: PathBuf,
    pub network: String,
    pub subnet: Ipv4Net,
    pub host_ip: Ipv4Addr,
    pub listen_port: u16,
    pub public_key: PublicKey,
}

/// `dir` に host.key と host.toml を作る。サブネットは Tailscale の CGNAT レンジ等と
/// 衝突しないランダムな `10.x.y.0/24` を選ぶ(ホスト = .1)。
///
/// `network_name` はネットワーク名(ADR-0012)。表示名のまま渡してよく、
/// ここで DNS ラベルへ正規化する(正規化で空になる名前はエラー)。
pub fn init_host(
    dir: &Path,
    network_name: &str,
    listen_port: u16,
    force: bool,
) -> anyhow::Result<InitResult> {
    let Some(network) = names::dns_label(network_name) else {
        bail!(
            "ネットワーク名 \"{network_name}\" から有効な名前を作れませんでした。\
             半角英数字を含む名前にしてください"
        );
    };
    std::fs::create_dir_all(dir)
        .with_context(|| format!("{} の作成に失敗しました", dir.display()))?;
    let key_path = dir.join("host.key");
    let config_path = dir.join("host.toml");
    for path in [&key_path, &config_path] {
        if path.exists() && !force {
            bail!("{} は既に存在します", path.display());
        }
    }

    let private_key = PrivateKey::generate();
    crate::secret::write_secret(&key_path, &format!("{}\n", private_key.to_base64()))
        .context("秘密鍵の保存に失敗しました")?;

    let subnet = random_private_subnet();
    let host_ip = subnet.hosts().next().expect("/24 にはホストがある");
    let config_text = format!(
        "# peercove により生成\n\
         [interface]\n\
         network_name = \"{network}\"\n\
         private_key_file = \"host.key\"\n\
         address = \"{host_ip}/{}\"\n\
         listen_port = {listen_port}\n",
        subnet.prefix_len()
    );
    std::fs::write(&config_path, &config_text)
        .with_context(|| format!("{} の書き込みに失敗しました", config_path.display()))?;
    Config::load(&config_path).context("生成した設定の検証に失敗しました(バグの可能性)")?;

    Ok(InitResult {
        config_path,
        key_path,
        network,
        subnet,
        host_ip,
        listen_port,
        public_key: private_key.public_key(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_generates_working_host_config() {
        let dir = std::env::temp_dir().join("peercove-ops-init");
        let _ = std::fs::remove_dir_all(&dir);

        let result = init_host(&dir, "home", 51820, false).unwrap();
        let config = Config::load(&result.config_path).unwrap();
        let octets = config.interface.address.addr().octets();
        assert_eq!(octets[0], 10);
        assert!((64..=127).contains(&octets[1]));
        assert_eq!(octets[3], 1, "ホストは .1");
        assert_eq!(config.interface.listen_port, Some(51820));
        assert_eq!(config.network_name(), "home");
        assert_eq!(result.host_ip, config.interface.address.addr());
        assert!(peercove_core::keys::read_private_key_file(&result.key_path).is_ok());

        // 上書きガードと force
        assert!(init_host(&dir, "home", 51820, false).is_err());
        let result = init_host(&dir, "home", 51821, true).unwrap();
        assert_eq!(result.listen_port, 51821);
    }

    #[test]
    fn init_normalizes_network_name_and_rejects_unusable() {
        let dir = std::env::temp_dir().join("peercove-ops-init-name");
        let _ = std::fs::remove_dir_all(&dir);

        let result = init_host(&dir, "My Game LAN", 51820, false).unwrap();
        assert_eq!(result.network, "my-game-lan");
        let config = Config::load(&result.config_path).unwrap();
        assert_eq!(config.network_name(), "my-game-lan");

        // 正規化で空になる名前は明確なエラー
        assert!(init_host(&dir, "たろう", 51820, true).is_err());
    }
}
