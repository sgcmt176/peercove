//! PeerCove の OS 非依存コア。
//!
//! - [`keys`]: X25519 鍵ペア・事前共有鍵の生成と base64 表現
//! - [`config`]: TOML 設定ファイルの型と読み込み
//! - [`ipalloc`]: 仮想 IP 割当ヘルパ

pub mod acl;
pub mod config;
pub mod diagnostics;
pub mod dns;
pub mod ipalloc;
pub mod ipc;
pub mod keys;
pub mod memo;
pub mod msg;
pub mod names;
pub mod proto;
pub mod quality;
pub mod schedule;
pub mod sheet;
pub mod token;

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("base64 のデコードに失敗しました: {0}")]
    InvalidBase64(#[from] base64::DecodeError),
    #[error("鍵の長さが不正です(期待 32 バイト、実際 {0} バイト)")]
    InvalidKeyLength(usize),
    #[error("ファイル {path} の入出力に失敗しました: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("TOML の解析に失敗しました: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("設定が不正です: {0}")]
    InvalidConfig(String),
    #[error("招待トークンが不正です: {0}")]
    InvalidToken(String),
}

pub type Result<T> = std::result::Result<T, Error>;
