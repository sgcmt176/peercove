//! PeerCove モバイル用コア(M4、ADR-0039)。
//!
//! Android アプリ(apps/peercove-android)から UniFFI 経由で呼ばれる。
//! 役割分担は「頭脳は Rust、OS との付き合いは Kotlin」:
//! ここに接続管理・WG・台帳同期・チャット/ファイルのプロトコル処理を実装し、
//! OS 連携(VpnService・通知・保存先)は Kotlin 側が担う。
//!
//! E-A(土台)の現時点では、Rust↔Kotlin ↔ peercove-core の疎通を確認する
//! 最小 API のみを公開している。

uniffi::setup_scaffolding!();

use peercove_core::keys::PrivateKey;

/// モバイルコアのバージョン文字列(アプリの情報表示用)
#[uniffi::export]
pub fn core_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// E-A の疎通確認: peercove-core の暗号(X25519 鍵生成)が Android 上で
/// 動くことを証明する。新規生成した鍵の公開鍵 base64 を返す。
/// 秘密鍵は返さない・ログにも出さない(CLAUDE.md の秘匿規約)。
#[uniffi::export]
pub fn probe_core() -> String {
    let key = PrivateKey::generate();
    format!("core ok / pubkey {}", key.public_key().to_base64())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_version_is_semver_like() {
        let v = core_version();
        assert!(v.split('.').count() >= 3, "version = {v}");
    }

    #[test]
    fn probe_core_returns_public_key() {
        let s = probe_core();
        assert!(s.starts_with("core ok / pubkey "));
        // X25519 公開鍵の base64 は 44 文字("=" パディング込み)
        let b64 = s.rsplit(' ').next().unwrap();
        assert_eq!(b64.len(), 44);
    }
}
