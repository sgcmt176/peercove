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
//! ```
//! 文字列表現は `pcv1.` + base64url(パディングなし)。

use std::fmt;
use std::net::{Ipv4Addr, SocketAddrV4};

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine as _;
use ipnet::Ipv4Net;

use crate::keys::{PresharedKey, PrivateKey, PublicKey, KEY_LEN};
use crate::{Error, Result};

pub const TOKEN_PREFIX: &str = "pcv1.";
const VERSION: u8 = 1;
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
        Ok(())
    }

    /// `pcv1.` 付きの文字列にエンコードする。**戻り値は秘密情報**。
    pub fn encode(&self) -> Result<String> {
        self.validate()?;
        let mut buf = Vec::with_capacity(160);
        buf.push(VERSION);
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
        if version != VERSION {
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
        r.finish()?;

        let token = Self {
            member_private_key,
            host_public_key,
            preshared_key,
            member_address,
            host_virtual_ip,
            endpoints,
            name,
        };
        token.validate()?;
        Ok(token)
    }
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
        }
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
