//! 暗号化されたネットワーク設定バックアップ (ADR-0034)。
//!
//! 平文はメモリ内だけで扱い、復元先はステージング領域で検証してから切り替える。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine as _;
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    XChaCha20Poly1305, XNonce,
};
use peercove_core::config::Config;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use toml_edit::{value, DocumentMut};
use zeroize::Zeroizing;

use crate::networks::{HOST_FILE, MEMBER_FILE};

const MAGIC: &[u8; 8] = b"PCVBKUP1";
const FORMAT_VERSION: u16 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const HEADER_LEN: usize = 8 + 2 + 4 + 4 + 4 + SALT_LEN + NONCE_LEN + 8;
const MEMORY_KIB: u32 = 64 * 1024;
const ITERATIONS: u32 = 3;
const PARALLELISM: u32 = 1;
const MAX_BACKUP_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackupRole {
    Host,
    Member,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupPreview {
    pub network_name: String,
    pub role: BackupRole,
    pub source_os: String,
    pub created_at_unix_ms: u64,
    pub categories: Vec<String>,
    pub config_file: String,
    pub member_key_rotation_recommended: bool,
}

#[derive(Debug, Clone)]
pub struct CreateResult {
    pub preview: BackupPreview,
    pub output_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct RestoreResult {
    pub preview: BackupPreview,
    pub config_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreMode {
    New,
    Replace,
}

#[derive(Serialize, Deserialize)]
struct BackupPayload {
    preview: BackupPreview,
    files: Vec<BackupFile>,
}

#[derive(Serialize, Deserialize)]
struct BackupFile {
    path: String,
    content_base64: String,
    secret: bool,
}

struct Header {
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
    salt: [u8; SALT_LEN],
    nonce: [u8; NONCE_LEN],
    ciphertext_len: u64,
}

pub fn create(
    config_path: &Path,
    output_path: &Path,
    passphrase: &str,
) -> anyhow::Result<CreateResult> {
    validate_passphrase(passphrase)?;
    let config = Config::load(config_path).context("設定ファイルを読み込めません")?;
    let role = if config_path.file_name().and_then(|v| v.to_str()) == Some(HOST_FILE) {
        BackupRole::Host
    } else if config_path.file_name().and_then(|v| v.to_str()) == Some(MEMBER_FILE) {
        BackupRole::Member
    } else {
        bail!("host.toml または member.toml を選択してください");
    };
    let config_file = match role {
        BackupRole::Host => HOST_FILE,
        BackupRole::Member => MEMBER_FILE,
    };
    let mut document: DocumentMut = std::fs::read_to_string(config_path)
        .context("設定ファイルを読み込めません")?
        .parse()
        .context("設定ファイルを解析できません")?;
    let mut files = Vec::new();

    let private_name = "secrets/private.key";
    add_file(
        &mut files,
        private_name,
        &config.interface.private_key_file,
        true,
    )?;
    document["interface"]["private_key_file"] = value(private_name);

    let peers = document
        .get_mut("peer")
        .and_then(toml_edit::Item::as_array_of_tables_mut);
    for (index, peer) in config.peers.iter().enumerate() {
        if let Some(source) = &peer.preshared_key_file {
            let logical = format!("secrets/peer-{index}.psk");
            add_file(&mut files, &logical, source, true)?;
            let Some(table) = peers.as_ref().and_then(|p| p.get(index)) else {
                bail!("peer[{index}] の設定をバックアップ用に変換できません");
            };
            let _ = table;
        }
    }
    if let Some(peers) = peers {
        for (index, table) in peers.iter_mut().enumerate() {
            if config.peers[index].preshared_key_file.is_some() {
                table["preshared_key_file"] = value(format!("secrets/peer-{index}.psk"));
            }
        }
    }

    files.push(BackupFile {
        path: config_file.to_string(),
        content_base64: encode(document.to_string().as_bytes()),
        secret: true,
    });
    let groups_path = config_path.with_extension("groups.json");
    let mut categories = vec!["network_config".to_string(), "keys".to_string()];
    if groups_path.exists() {
        let groups_name = format!(
            "{}.groups.json",
            Path::new(config_file)
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("network")
        );
        add_file(&mut files, &groups_name, &groups_path, false)?;
        categories.push("groups".to_string());
    }
    if !config.dns_records.is_empty() {
        categories.push("dns".to_string());
    }
    if !config.acl.is_empty() {
        categories.push("acl".to_string());
    }
    if config.peers.iter().any(|peer| peer.invite_id.is_some()) {
        categories.push("invite_metadata".to_string());
    }
    let preview = BackupPreview {
        network_name: config.network_name().to_string(),
        role: role.clone(),
        source_os: std::env::consts::OS.to_string(),
        created_at_unix_ms: now_ms(),
        categories,
        config_file: config_file.to_string(),
        member_key_rotation_recommended: role == BackupRole::Member,
    };
    let payload = BackupPayload {
        preview: preview.clone(),
        files,
    };
    let plaintext = Zeroizing::new(serde_json::to_vec(&payload)?);
    let mut salt = [0; SALT_LEN];
    let mut nonce = [0; NONCE_LEN];
    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut nonce);
    let header = Header {
        memory_kib: MEMORY_KIB,
        iterations: ITERATIONS,
        parallelism: PARALLELISM,
        salt,
        nonce,
        ciphertext_len: (plaintext.len() + 16) as u64,
    };
    let header_bytes = encode_header(&header);
    let key = derive_key(passphrase, &header)?;
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_ref()).expect("32-byte key");
    let nonce = XNonce::from(header.nonce);
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: &plaintext,
                aad: &header_bytes,
            },
        )
        .map_err(|_| anyhow::anyhow!("バックアップを暗号化できません"))?;
    let mut output = Vec::with_capacity(header_bytes.len() + ciphertext.len());
    output.extend_from_slice(&header_bytes);
    output.extend_from_slice(&ciphertext);
    crate::secret::write_secret_bytes(output_path, &output)
        .context("バックアップを書き込めません")?;
    Ok(CreateResult {
        preview,
        output_path: output_path.to_path_buf(),
    })
}

pub fn inspect(path: &Path, passphrase: &str) -> anyhow::Result<BackupPreview> {
    Ok(decrypt(path, passphrase)?.preview)
}

pub fn restore(
    path: &Path,
    passphrase: &str,
    base: &Path,
    slug: &str,
    mode: RestoreMode,
) -> anyhow::Result<RestoreResult> {
    if !peercove_core::names::is_dns_label(slug) {
        bail!("復元先の名前が不正です");
    }
    let payload = decrypt(path, passphrase)?;
    let target = crate::networks::networks_dir(base).join(slug);
    if target.exists() && mode == RestoreMode::New {
        bail!("同じ名前のネットワークが既にあります。別名か置換を選択してください");
    }
    if mode == RestoreMode::Replace
        && !target.join(HOST_FILE).exists()
        && !target.join(MEMBER_FILE).exists()
    {
        bail!("置換先は PeerCove のネットワークではありません");
    }
    std::fs::create_dir_all(crate::networks::networks_dir(base))?;
    let stage = crate::networks::networks_dir(base).join(format!(
        ".restore-{}-{}",
        std::process::id(),
        now_ms()
    ));
    if stage.exists() {
        std::fs::remove_dir_all(&stage)?;
    }
    std::fs::create_dir_all(&stage)?;
    let result = restore_to_stage(&payload, &stage, slug);
    if let Err(error) = result {
        let _ = std::fs::remove_dir_all(&stage);
        return Err(error);
    }
    let config_name = payload.preview.config_file.clone();
    let old = crate::networks::networks_dir(base).join(format!(
        ".replace-old-{}-{}",
        std::process::id(),
        now_ms()
    ));
    // staged dir には復号済みの秘密鍵・PSK が置かれているため、どの失敗経路でも
    // 必ず削除する。target の退避に失敗した時点で stage を残さない。
    if target.exists() {
        if let Err(error) = std::fs::rename(&target, &old) {
            let _ = std::fs::remove_dir_all(&stage);
            return Err(error).context("既存ネットワークを退避できません");
        }
    }
    if let Err(error) = std::fs::rename(&stage, &target) {
        let _ = std::fs::remove_dir_all(&stage);
        if old.exists() {
            let _ = std::fs::rename(&old, &target);
        }
        return Err(error).context("復元したネットワークを配置できません");
    }
    if old.exists() {
        let _ = std::fs::remove_dir_all(old);
    }
    Ok(RestoreResult {
        preview: payload.preview,
        config_path: target.join(config_name),
    })
}

fn restore_to_stage(payload: &BackupPayload, stage: &Path, slug: &str) -> anyhow::Result<()> {
    let allowed_config = payload.preview.config_file.as_str();
    for file in &payload.files {
        let valid = file.path == allowed_config
            || file.path
                == Path::new(allowed_config)
                    .with_extension("groups.json")
                    .to_string_lossy()
            || file.path.starts_with("secrets/") && !file.path["secrets/".len()..].contains('/');
        if !valid || file.path.contains("..") || Path::new(&file.path).is_absolute() {
            bail!("バックアップに不正なファイル名が含まれています");
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&file.content_base64)
            .context("バックアップ内容が壊れています")?;
        let output = stage.join(&file.path);
        if file.secret {
            crate::secret::write_secret_bytes(&output, &bytes)?;
        } else {
            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(output, bytes)?;
        }
    }
    let config_path = stage.join(allowed_config);
    let mut document: DocumentMut = std::fs::read_to_string(&config_path)?.parse()?;
    document["interface"]["network_name"] = value(slug);
    crate::secret::write_secret(&config_path, &document.to_string())?;
    Config::load(&config_path).context("復元内容の検証に失敗しました")?;
    Ok(())
}

fn decrypt(path: &Path, passphrase: &str) -> anyhow::Result<BackupPayload> {
    validate_passphrase(passphrase)?;
    let metadata = std::fs::metadata(path).context("バックアップを開けません")?;
    if metadata.len() > MAX_BACKUP_BYTES {
        bail!("バックアップが大きすぎます");
    }
    let bytes = std::fs::read(path).context("バックアップを読めません")?;
    let (header, header_bytes) = decode_header(&bytes)?;
    let ciphertext = bytes
        .get(HEADER_LEN..)
        .context("バックアップが途中で切れています")?;
    if ciphertext.len() as u64 != header.ciphertext_len {
        bail!("バックアップが途中で切れているか、形式が不正です");
    }
    let key = derive_key(passphrase, &header)?;
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_ref()).expect("32-byte key");
    let nonce = XNonce::from(header.nonce);
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: ciphertext,
                    aad: header_bytes,
                },
            )
            .map_err(|_| {
                anyhow::anyhow!("パスフレーズが違うか、バックアップが改ざんされています")
            })?,
    );
    serde_json::from_slice(&plaintext).context("バックアップ内容を解析できません")
}

fn add_file(
    files: &mut Vec<BackupFile>,
    logical: &str,
    source: &Path,
    secret: bool,
) -> anyhow::Result<()> {
    let bytes = std::fs::read(source)
        .with_context(|| format!("必要なファイル {} を読めません", source.display()))?;
    files.push(BackupFile {
        path: logical.to_string(),
        content_base64: encode(&bytes),
        secret,
    });
    Ok(())
}

fn derive_key(passphrase: &str, header: &Header) -> anyhow::Result<Zeroizing<[u8; 32]>> {
    if header.memory_kib > 256 * 1024 || header.iterations > 10 || header.parallelism > 8 {
        bail!("バックアップの鍵導出パラメーターが許容範囲外です");
    }
    let params = Params::new(
        header.memory_kib,
        header.iterations,
        header.parallelism,
        Some(32),
    )
    .map_err(|_| anyhow::anyhow!("鍵導出パラメーターが不正です"))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = Zeroizing::new([0; 32]);
    argon2
        .hash_password_into(passphrase.as_bytes(), &header.salt, key.as_mut())
        .map_err(|_| anyhow::anyhow!("パスフレーズから鍵を作成できません"))?;
    Ok(key)
}

fn encode_header(header: &Header) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&header.memory_kib.to_le_bytes());
    out.extend_from_slice(&header.iterations.to_le_bytes());
    out.extend_from_slice(&header.parallelism.to_le_bytes());
    out.extend_from_slice(&header.salt);
    out.extend_from_slice(&header.nonce);
    out.extend_from_slice(&header.ciphertext_len.to_le_bytes());
    out
}

fn decode_header(bytes: &[u8]) -> anyhow::Result<(Header, &[u8])> {
    let header_bytes = bytes
        .get(..HEADER_LEN)
        .context("バックアップが途中で切れています")?;
    if &header_bytes[..8] != MAGIC {
        bail!("PeerCove バックアップではありません");
    }
    let version = u16::from_le_bytes(header_bytes[8..10].try_into().unwrap());
    if version != FORMAT_VERSION {
        bail!("未対応のバックアップ形式です (version={version})");
    }
    let u32_at = |start| u32::from_le_bytes(header_bytes[start..start + 4].try_into().unwrap());
    let mut salt = [0; SALT_LEN];
    salt.copy_from_slice(&header_bytes[22..38]);
    let mut nonce = [0; NONCE_LEN];
    nonce.copy_from_slice(&header_bytes[38..62]);
    let ciphertext_len = u64::from_le_bytes(header_bytes[62..70].try_into().unwrap());
    Ok((
        Header {
            memory_kib: u32_at(10),
            iterations: u32_at(14),
            parallelism: u32_at(18),
            salt,
            nonce,
            ciphertext_len,
        },
        header_bytes,
    ))
}

fn validate_passphrase(passphrase: &str) -> anyhow::Result<()> {
    if passphrase.chars().count() < 12 {
        bail!("パスフレーズは12文字以上にしてください");
    }
    Ok(())
}

fn encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "peercove-backup-{name}-{}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn round_trip_and_rejects_wrong_password_tamper_and_truncation() {
        let source = base("roundtrip-source");
        let (_, network) = crate::networks::network_dir(&source, "backup-test").unwrap();
        let initialized = crate::init::init_host(&network, "backup-test", 51820, false).unwrap();
        std::fs::write(
            initialized.config_path.with_extension("groups.json"),
            "{\"revision\":1,\"groups\":[]}",
        )
        .unwrap();
        let backup = source.join("test.pcvbackup");
        let created = create(&initialized.config_path, &backup, "correct horse battery").unwrap();
        assert_eq!(created.preview.role, BackupRole::Host);
        assert!(inspect(&backup, "wrong password!!").is_err());

        let destination = base("roundtrip-destination");
        let restored = restore(
            &backup,
            "correct horse battery",
            &destination,
            "restored",
            RestoreMode::New,
        )
        .unwrap();
        let config = Config::load(&restored.config_path).unwrap();
        assert_eq!(config.network_name(), "restored");
        assert!(config.interface.private_key_file.exists());
        assert!(restored.config_path.with_extension("groups.json").exists());

        let original = std::fs::read(&backup).unwrap();
        let mut tampered = original.clone();
        *tampered.last_mut().unwrap() ^= 1;
        std::fs::write(source.join("tampered.pcvbackup"), tampered).unwrap();
        assert!(inspect(&source.join("tampered.pcvbackup"), "correct horse battery").is_err());
        std::fs::write(
            source.join("short.pcvbackup"),
            &original[..original.len() - 1],
        )
        .unwrap();
        assert!(inspect(&source.join("short.pcvbackup"), "correct horse battery").is_err());
    }

    #[test]
    fn replace_is_explicit_and_atomic() {
        let base = base("replace");
        let (_, first) = crate::networks::network_dir(&base, "same").unwrap();
        let initialized = crate::init::init_host(&first, "same", 51820, false).unwrap();
        let backup = base.join("replace.pcvbackup");
        create(
            &initialized.config_path,
            &backup,
            "a sufficiently long passphrase",
        )
        .unwrap();
        assert!(restore(
            &backup,
            "a sufficiently long passphrase",
            &base,
            "same",
            RestoreMode::New
        )
        .is_err());
        assert!(restore(
            &backup,
            "a sufficiently long passphrase",
            &base,
            "same",
            RestoreMode::Replace
        )
        .is_ok());
    }
}
