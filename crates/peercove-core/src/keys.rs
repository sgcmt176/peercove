//! X25519 鍵ペアと事前共有鍵。
//!
//! 秘密鍵・PSK は `Debug` でも中身を出さない。base64 文字列化はファイル保存
//! 専用の [`PrivateKey::to_base64`] / [`PresharedKey::to_base64`] のみで行い、
//! 呼び出し側はログ・標準出力へ渡さないこと。

use std::fmt;
use std::path::Path;
use std::str::FromStr;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use rand_core::{OsRng, RngCore};

use crate::{Error, Result};

pub const KEY_LEN: usize = 32;

fn decode_key(s: &str) -> Result<[u8; KEY_LEN]> {
    let bytes = B64.decode(s.trim())?;
    let len = bytes.len();
    <[u8; KEY_LEN]>::try_from(bytes).map_err(|_| Error::InvalidKeyLength(len))
}

/// X25519 秘密鍵。
#[derive(Clone)]
pub struct PrivateKey([u8; KEY_LEN]);

impl PrivateKey {
    pub fn generate() -> Self {
        Self(x25519_dalek::StaticSecret::random_from_rng(OsRng).to_bytes())
    }

    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(bytes)
    }

    pub fn from_base64(s: &str) -> Result<Self> {
        decode_key(s).map(Self)
    }

    /// ファイル保存専用。ログ・標準出力へ出さないこと。
    pub fn to_base64(&self) -> String {
        B64.encode(self.0)
    }

    pub fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }

    pub fn public_key(&self) -> PublicKey {
        let secret = x25519_dalek::StaticSecret::from(self.0);
        PublicKey(x25519_dalek::PublicKey::from(&secret).to_bytes())
    }
}

impl fmt::Debug for PrivateKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PrivateKey(<redacted>)")
    }
}

/// X25519 公開鍵。表示・ログ出力可。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PublicKey([u8; KEY_LEN]);

impl PublicKey {
    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(bytes)
    }

    pub fn from_base64(s: &str) -> Result<Self> {
        decode_key(s).map(Self)
    }

    pub fn to_base64(&self) -> String {
        B64.encode(self.0)
    }

    pub fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_base64())
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({})", self.to_base64())
    }
}

impl FromStr for PublicKey {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Self::from_base64(s)
    }
}

impl serde::Serialize for PublicKey {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_base64())
    }
}

impl<'de> serde::Deserialize<'de> for PublicKey {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_base64(&s).map_err(serde::de::Error::custom)
    }
}

/// WireGuard プロトコルの事前共有鍵(32 バイト乱数)。
#[derive(Clone)]
pub struct PresharedKey([u8; KEY_LEN]);

impl PresharedKey {
    pub fn generate() -> Self {
        let mut bytes = [0u8; KEY_LEN];
        OsRng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(bytes)
    }

    pub fn from_base64(s: &str) -> Result<Self> {
        decode_key(s).map(Self)
    }

    /// ファイル保存専用。ログ・標準出力へ出さないこと。
    pub fn to_base64(&self) -> String {
        B64.encode(self.0)
    }

    pub fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }
}

impl fmt::Debug for PresharedKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PresharedKey(<redacted>)")
    }
}

/// 秘密情報をファイルへ書き込む。Unix ではパーミッション 600 を付ける。
/// Windows の ACL 制限は呼び出し側(CLI 層)で行う。
pub fn write_secret_file(path: &Path, contents: &str) -> Result<()> {
    let io_err = |source| Error::Io {
        path: path.to_path_buf(),
        source,
    };
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
        // 保存先が既にシンボリックリンクなら拒否する。root デーモンが攻撃者の
        // 仕込んだリンク先へ秘密を書くのを防ぐ(best-effort の事前チェック)。
        if let Ok(meta) = std::fs::symlink_metadata(path) {
            if meta.file_type().is_symlink() {
                return Err(io_err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "秘密ファイルの保存先がシンボリックリンクです",
                )));
            }
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(io_err)?;
        // `.mode(0o600)` は新規作成時のみ効く。既存ファイルを上書きした場合に
        // 緩い権限が残らないよう、開いた fd に 0600 を明示的に再適用する。
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(io_err)?;
        file.write_all(contents.as_bytes()).map_err(io_err)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, contents).map_err(io_err)
    }
}

fn read_key_file(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

pub fn read_private_key_file(path: &Path) -> Result<PrivateKey> {
    PrivateKey::from_base64(&read_key_file(path)?)
}

pub fn read_preshared_key_file(path: &Path) -> Result<PresharedKey> {
    PresharedKey::from_base64(&read_key_file(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from_hex(hex: &str) -> [u8; KEY_LEN] {
        let mut out = [0u8; KEY_LEN];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
        }
        out
    }

    /// RFC 7748 6.1 のテストベクター(Alice の鍵ペア)。
    #[test]
    fn public_key_derivation_matches_rfc7748() {
        let private = PrivateKey::from_bytes(from_hex(
            "77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a",
        ));
        let expected = PublicKey::from_bytes(from_hex(
            "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a",
        ));
        assert_eq!(private.public_key(), expected);
    }

    #[test]
    fn base64_roundtrip() {
        let private = PrivateKey::generate();
        let restored = PrivateKey::from_base64(&private.to_base64()).unwrap();
        assert_eq!(private.as_bytes(), restored.as_bytes());
        assert_eq!(private.public_key(), restored.public_key());

        let psk = PresharedKey::generate();
        let restored = PresharedKey::from_base64(&psk.to_base64()).unwrap();
        assert_eq!(psk.as_bytes(), restored.as_bytes());
    }

    #[test]
    fn decode_rejects_wrong_length() {
        assert!(matches!(
            PublicKey::from_base64("dG9vLXNob3J0"),
            Err(Error::InvalidKeyLength(_))
        ));
        assert!(matches!(
            PrivateKey::from_base64("not!!base64"),
            Err(Error::InvalidBase64(_))
        ));
    }

    #[test]
    fn secrets_are_redacted_in_debug() {
        let private = PrivateKey::generate();
        let debug = format!("{private:?}");
        assert!(!debug.contains(&private.to_base64()));
        assert!(debug.contains("redacted"));

        let psk = PresharedKey::generate();
        let debug = format!("{psk:?}");
        assert!(!debug.contains(&psk.to_base64()));
    }

    #[test]
    fn trailing_newline_is_accepted() {
        let private = PrivateKey::generate();
        let with_newline = format!("{}\n", private.to_base64());
        assert!(PrivateKey::from_base64(&with_newline).is_ok());
    }
}
