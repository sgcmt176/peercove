//! 招待トークン(pcv1 形式)。ADR-0005 案 B。
//!
//! `invite` 時にホストがメンバーの鍵ペア・仮想 IP を生成し、参加に必要な情報を
//! すべて 1 つのトークンに同梱する。メンバーは `join` でトークンを取り込むだけで
//! 接続できる(追加の登録プロトコル・開放ポートは不要)。
//!
//! **トークンはメンバー秘密鍵を含む秘密情報**。ログ・標準出力への表示は
//! 呼び出し側で明示オプトインの場合のみ行うこと。
//!
//! ワイヤ形式(バイナリ、リトルエンディアン不使用・ネットワークバイトオーダー):
//! ```text
//! u8       version (=1)
//! [u8;32]  member_private_key
//! [u8;32]  host_public_key
//! u8       flags (bit0: PSK あり)
//! [u8;32]  psk                (flags bit0 のときのみ)
//! [u8;4]   member_ip
//! u8       prefix_len
//! [u8;4]   host_virtual_ip   (コントロールチャネル接続先)
//! u8       endpoint_count (1..=MAX_ENDPOINTS)
//! per ep:  [u8;4] ip, u16 port (be)
//! u8       name_len (1..=MAX_NAME_LEN) + name (UTF-8)
//! -- ここまで version 1。version 2(ADR-0012)は末尾に追加:
//! u8       network_len (1..=63) + network (正規化済み DNS ラベル)
//! -- version 3(M3-22)は network_len(0 = 既定名)の後ろに追加:
//! [u8;16]  invite_id
//! u64      issued_at (be, UNIX 秒)
//! u64      expires_at (be, 0 = 無期限)
//! ```
//! `network` が無い場合(既定名)は version 1 としてエンコードする。これにより
//! 既定名のトークンは旧バイナリでも読める。v1 のパースは `network = None`。
//!
//! 文字列表現は `pcv1.` + base64url(パディングなし)。プレフィックスの
//! `pcv1` はトークン**書式ファミリー**の識別子で、内部バージョンとは独立。

use std::fmt;
use std::net::{Ipv4Addr, SocketAddrV4};

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine as _;
use ipnet::Ipv4Net;

use crate::keys::{PresharedKey, PrivateKey, PublicKey, KEY_LEN};
use crate::{Error, Result};

pub const TOKEN_PREFIX: &str = "pcv1.";
const VERSION_1: u8 = 1;
const VERSION_2: u8 = 2;
const VERSION_3: u8 = 3;
const INVITE_ID_LEN: usize = 16;
const FLAG_PSK: u8 = 0b0000_0001;
pub const MAX_ENDPOINTS: usize = 4;
pub const MAX_NAME_LEN: usize = 64;

/// 招待トークンの中身。`Debug` は秘密部分をリダクトする。
#[derive(Clone)]
pub struct InviteToken {
    /// メンバー用に発行された秘密鍵(トークンの秘密部分)
    pub member_private_key: PrivateKey,
    pub host_public_key: PublicKey,
    /// 任意の事前共有鍵(秘密部分)
    pub preshared_key: Option<PresharedKey>,
    /// メンバーへ割り当てた仮想 IP とサブネット
    pub member_address: Ipv4Net,
    /// ホストの仮想 IP(トンネル内コントロールチャネルの接続先)
    pub host_virtual_ip: Ipv4Addr,
    /// ホストへの到達先候補。優先順(例: LAN, 外部)
    pub endpoints: Vec<SocketAddrV4>,
    /// メンバーの表示名(台帳用)
    pub name: String,
    /// ネットワーク名(ADR-0012、正規化済み DNS ラベル)。
    /// `None` は既定名(旧トークン、または既定名のまま運用しているホスト)。
    pub network: Option<String>,
    /// v3 の招待識別子。秘密ではないが、ホスト側台帳との照合にだけ使う。
    pub invite_id: Option<String>,
    /// v3 の発行・期限時刻(UNIX 秒)。expires_at = None は無期限。
    pub issued_at: Option<u64>,
    pub expires_at: Option<u64>,
}

impl fmt::Debug for InviteToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InviteToken")
            .field("member_private_key", &"<redacted>")
            .field("host_public_key", &self.host_public_key)
            .field(
                "preshared_key",
                &self.preshared_key.as_ref().map(|_| "<redacted>"),
            )
            .field("member_address", &self.member_address)
            .field("endpoints", &self.endpoints)
            .field("name", &self.name)
            .field("network", &self.network)
            .field("invite_id", &self.invite_id)
            .field("issued_at", &self.issued_at)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

impl InviteToken {
    /// 内容を検証する(encode/parse の双方から呼ぶ)。
    fn validate(&self) -> Result<()> {
        let invalid = |m: String| Err(Error::InvalidToken(m));
        if self.endpoints.is_empty() || self.endpoints.len() > MAX_ENDPOINTS {
            return invalid(format!(
                "エンドポイント数が不正です(1〜{MAX_ENDPOINTS}、実際 {})",
                self.endpoints.len()
            ));
        }
        let name_len = self.name.len();
        if name_len == 0 || name_len > MAX_NAME_LEN {
            return invalid(format!(
                "名前の長さが不正です(1〜{MAX_NAME_LEN} バイト、実際 {name_len})"
            ));
        }
        if self.member_address.prefix_len() > 30 {
            return invalid(format!(
                "プレフィックス長 /{} が不正です",
                self.member_address.prefix_len()
            ));
        }
        if let Some(network) = &self.network {
            if !crate::names::is_dns_label(network) {
                return invalid(format!(
                    "ネットワーク名 \"{network}\" が不正です(正規化済みの DNS ラベルのみ)"
                ));
            }
        }
        match (&self.invite_id, self.issued_at) {
            (Some(id), Some(issued_at)) => {
                decode_invite_id(id)?;
                if let Some(expires_at) = self.expires_at {
                    if expires_at <= issued_at {
                        return invalid("招待期限は発行時刻より後にしてください".to_string());
                    }
                }
            }
            (None, None) if self.expires_at.is_none() => {}
            _ => return invalid("招待 v3 のメタデータが不完全です".to_string()),
        }
        Ok(())
    }

    /// `pcv1.` 付きの文字列にエンコードする。**戻り値は秘密情報**。
    pub fn encode(&self) -> Result<String> {
        self.validate()?;
        let mut buf = Vec::with_capacity(160);
        // 既定名(network なし)は v1 のまま = 旧バイナリでも読める
        buf.push(if self.invite_id.is_some() {
            VERSION_3
        } else if self.network.is_some() {
            VERSION_2
        } else {
            VERSION_1
        });
        buf.extend_from_slice(self.member_private_key.as_bytes());
        buf.extend_from_slice(self.host_public_key.as_bytes());
        match &self.preshared_key {
            Some(psk) => {
                buf.push(FLAG_PSK);
                buf.extend_from_slice(psk.as_bytes());
            }
            None => buf.push(0),
        }
        buf.extend_from_slice(&self.member_address.addr().octets());
        buf.push(self.member_address.prefix_len());
        buf.extend_from_slice(&self.host_virtual_ip.octets());
        buf.push(self.endpoints.len() as u8);
        for ep in &self.endpoints {
            buf.extend_from_slice(&ep.ip().octets());
            buf.extend_from_slice(&ep.port().to_be_bytes());
        }
        buf.push(self.name.len() as u8);
        buf.extend_from_slice(self.name.as_bytes());
        if let Some(network) = &self.network {
            buf.push(network.len() as u8);
            buf.extend_from_slice(network.as_bytes());
        }
        if let (Some(invite_id), Some(issued_at)) = (&self.invite_id, self.issued_at) {
            // v3 は network を常に 1 バイト長付きで持つ(None は長さ 0)。
            if self.network.is_none() {
                buf.push(0);
            }
            buf.extend_from_slice(&decode_invite_id(invite_id)?);
            buf.extend_from_slice(&issued_at.to_be_bytes());
            buf.extend_from_slice(&self.expires_at.unwrap_or(0).to_be_bytes());
        }
        Ok(format!("{TOKEN_PREFIX}{}", B64URL.encode(&buf)))
    }

    /// 文字列からデコードする。前後の空白は無視する。
    pub fn parse(text: &str) -> Result<Self> {
        let invalid = |m: &str| Error::InvalidToken(m.to_string());
        let text = text.trim();
        let body = text.strip_prefix(TOKEN_PREFIX).ok_or_else(|| {
            invalid("pcv1 形式ではありません(`pcv1.` で始まる文字列を貼り付けてください)")
        })?;
        let bytes = B64URL.decode(body).map_err(|_| {
            invalid("base64 として解釈できません(コピー漏れ・改行混入がないか確認してください)")
        })?;
        let mut r = Reader::new(&bytes);

        let version = r.u8()?;
        if version != VERSION_1 && version != VERSION_2 && version != VERSION_3 {
            return Err(Error::InvalidToken(format!(
                "未対応のトークンバージョンです({version})。新しい peercove に更新してください"
            )));
        }
        let member_private_key = PrivateKey::from_bytes(r.key()?);
        let host_public_key = PublicKey::from_bytes(r.key()?);
        let flags = r.u8()?;
        let preshared_key = if flags & FLAG_PSK != 0 {
            Some(PresharedKey::from_bytes(r.key()?))
        } else {
            None
        };
        let ip = Ipv4Addr::from(r.array::<4>()?);
        let prefix = r.u8()?;
        let member_address =
            Ipv4Net::new(ip, prefix).map_err(|_| invalid("仮想 IP のプレフィックスが不正です"))?;
        let host_virtual_ip = Ipv4Addr::from(r.array::<4>()?);
        let count = r.u8()? as usize;
        if count == 0 || count > MAX_ENDPOINTS {
            return Err(invalid("エンドポイント数が不正です"));
        }
        let mut endpoints = Vec::with_capacity(count);
        for _ in 0..count {
            let ip = Ipv4Addr::from(r.array::<4>()?);
            let port = u16::from_be_bytes(r.array::<2>()?);
            endpoints.push(SocketAddrV4::new(ip, port));
        }
        let name_len = r.u8()? as usize;
        let name = String::from_utf8(r.bytes(name_len)?.to_vec())
            .map_err(|_| invalid("名前が UTF-8 ではありません"))?;
        let network = if version >= VERSION_2 {
            let len = r.u8()? as usize;
            let text = String::from_utf8(r.bytes(len)?.to_vec())
                .map_err(|_| invalid("ネットワーク名が UTF-8 ではありません"))?;
            (!text.is_empty()).then_some(text)
        } else {
            None
        };
        let (invite_id, issued_at, expires_at) = if version >= VERSION_3 {
            let id = encode_invite_id(&r.array::<INVITE_ID_LEN>()?);
            let issued = u64::from_be_bytes(r.array::<8>()?);
            let expires = u64::from_be_bytes(r.array::<8>()?);
            (Some(id), Some(issued), (expires != 0).then_some(expires))
        } else {
            (None, None, None)
        };
        r.finish()?;

        let token = Self {
            member_private_key,
            host_public_key,
            preshared_key,
            member_address,
            host_virtual_ip,
            endpoints,
            name,
            network,
            invite_id,
            issued_at,
            expires_at,
        };
        token.validate()?;
        Ok(token)
    }
}

/// OS CSPRNG で v3 招待 ID を作る。表示・設定保存用に小文字 hex を返す。
pub fn generate_invite_id() -> String {
    use rand_core::{OsRng, RngCore};
    let mut bytes = [0u8; INVITE_ID_LEN];
    OsRng.fill_bytes(&mut bytes);
    encode_invite_id(&bytes)
}

fn encode_invite_id(bytes: &[u8; INVITE_ID_LEN]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_invite_id(value: &str) -> Result<[u8; INVITE_ID_LEN]> {
    if value.len() != INVITE_ID_LEN * 2 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::InvalidToken("招待 ID が不正です".to_string()));
    }
    let mut bytes = [0u8; INVITE_ID_LEN];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(chunk).expect("ASCII は UTF-8");
        bytes[index] = u8::from_str_radix(text, 16)
            .map_err(|_| Error::InvalidToken("招待 ID が不正です".to_string()))?;
    }
    Ok(bytes)
}

/// 長さ検査付きの単純なバイナリリーダ。
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn bytes(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(len).filter(|&e| e <= self.bytes.len());
        match end {
            Some(end) => {
                let slice = &self.bytes[self.pos..end];
                self.pos = end;
                Ok(slice)
            }
            None => Err(Error::InvalidToken(
                "トークンが途中で切れています(コピー漏れの可能性)".to_string(),
            )),
        }
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self
            .bytes(N)?
            .try_into()
            .expect("長さは bytes() で保証済み"))
    }

    fn key(&mut self) -> Result<[u8; KEY_LEN]> {
        self.array::<KEY_LEN>()
    }

    fn finish(&self) -> Result<()> {
        if self.pos == self.bytes.len() {
            Ok(())
        } else {
            Err(Error::InvalidToken(
                "トークン末尾に余分なデータがあります".to_string(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> InviteToken {
        InviteToken {
            member_private_key: PrivateKey::generate(),
            host_public_key: PrivateKey::generate().public_key(),
            preshared_key: Some(PresharedKey::generate()),
            member_address: "100.100.42.2/24".parse().unwrap(),
            host_virtual_ip: "100.100.42.1".parse().unwrap(),
            endpoints: vec![
                "192.168.0.12:51820".parse().unwrap(),
                "203.0.113.5:51820".parse().unwrap(),
            ],
            name: "member-a".to_string(),
            network: Some("my-game-lan".to_string()),
            invite_id: None,
            issued_at: None,
            expires_at: None,
        }
    }

    #[test]
    fn roundtrip_with_and_without_psk() {
        for psk in [true, false] {
            let mut token = sample();
            if !psk {
                token.preshared_key = None;
            }
            let encoded = token.encode().unwrap();
            assert!(encoded.starts_with("pcv1."));
            let parsed = InviteToken::parse(&encoded).unwrap();
            assert_eq!(
                parsed.member_private_key.as_bytes(),
                token.member_private_key.as_bytes()
            );
            assert_eq!(parsed.host_public_key, token.host_public_key);
            assert_eq!(parsed.preshared_key.is_some(), psk);
            assert_eq!(parsed.member_address, token.member_address);
            assert_eq!(parsed.host_virtual_ip, token.host_virtual_ip);
            assert_eq!(parsed.endpoints, token.endpoints);
            assert_eq!(parsed.name, token.name);
            assert_eq!(parsed.network, token.network);
        }
    }

    #[test]
    fn network_none_encodes_as_v1_for_backward_compat() {
        let mut token = sample();
        token.network = None;
        let encoded = token.encode().unwrap();
        let bytes = B64URL.decode(&encoded[TOKEN_PREFIX.len()..]).unwrap();
        assert_eq!(bytes[0], VERSION_1, "既定名のトークンは v1 のまま");
        let parsed = InviteToken::parse(&encoded).unwrap();
        assert_eq!(parsed.network, None);
    }

    #[test]
    fn network_some_encodes_as_v2_and_roundtrips() {
        let token = sample();
        let encoded = token.encode().unwrap();
        let bytes = B64URL.decode(&encoded[TOKEN_PREFIX.len()..]).unwrap();
        assert_eq!(bytes[0], VERSION_2);
        assert_eq!(
            InviteToken::parse(&encoded).unwrap().network.as_deref(),
            Some("my-game-lan")
        );
    }

    #[test]
    fn invite_metadata_encodes_as_v3_and_roundtrips() {
        let mut token = sample();
        token.invite_id = Some("0123456789abcdef0123456789abcdef".to_string());
        token.issued_at = Some(1_700_000_000);
        token.expires_at = Some(1_700_086_400);
        let encoded = token.encode().unwrap();
        let bytes = B64URL.decode(&encoded[TOKEN_PREFIX.len()..]).unwrap();
        assert_eq!(bytes[0], VERSION_3);
        let parsed = InviteToken::parse(&encoded).unwrap();
        assert_eq!(parsed.invite_id, token.invite_id);
        assert_eq!(parsed.issued_at, token.issued_at);
        assert_eq!(parsed.expires_at, token.expires_at);
    }

    #[test]
    fn rejects_non_normalized_network_name() {
        let mut token = sample();
        token.network = Some("My LAN".to_string());
        assert!(token.encode().is_err());
        token.network = Some(String::new());
        assert!(token.encode().is_err());
    }

    #[test]
    fn parse_accepts_surrounding_whitespace() {
        let encoded = format!("  {}\n", sample().encode().unwrap());
        assert!(InviteToken::parse(&encoded).is_ok());
    }

    #[test]
    fn rejects_bad_prefix_truncation_and_trailing_garbage() {
        let encoded = sample().encode().unwrap();
        assert!(InviteToken::parse("mlk1.abc").is_err());
        assert!(InviteToken::parse(&encoded[..encoded.len() - 8]).is_err());
        // 末尾に余分なバイトを足す(base64 として有効な形で)
        let body = &encoded[TOKEN_PREFIX.len()..];
        let mut bytes = B64URL.decode(body).unwrap();
        bytes.push(0);
        let extended = format!("{TOKEN_PREFIX}{}", B64URL.encode(&bytes));
        assert!(InviteToken::parse(&extended).is_err());
    }

    #[test]
    fn rejects_unknown_version() {
        let encoded = sample().encode().unwrap();
        let mut bytes = B64URL.decode(&encoded[TOKEN_PREFIX.len()..]).unwrap();
        bytes[0] = 99;
        let bad = format!("{TOKEN_PREFIX}{}", B64URL.encode(&bytes));
        let err = InviteToken::parse(&bad).unwrap_err();
        assert!(err.to_string().contains("バージョン"));
    }

    #[test]
    fn validates_name_and_endpoints() {
        let mut token = sample();
        token.name = String::new();
        assert!(token.encode().is_err());

        let mut token = sample();
        token.name = "あ".repeat(33); // 99 バイト > 64
        assert!(token.encode().is_err());

        let mut token = sample();
        token.endpoints.clear();
        assert!(token.encode().is_err());
    }

    #[test]
    fn debug_redacts_secrets() {
        let token = sample();
        let debug = format!("{token:?}");
        assert!(!debug.contains(&token.member_private_key.to_base64()));
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn token_is_reasonably_compact_for_qr() {
        let encoded = sample().encode().unwrap();
        // QR バージョン 10(避けたい上限)よりだいぶ小さいこと
        assert!(
            encoded.len() < 300,
            "トークンが長すぎます: {}",
            encoded.len()
        );
    }
}
