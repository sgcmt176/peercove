//! ACL v2 の設定型と OS 非依存のパケット評価器 (ADR-0035)。

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use ipnet::Ipv4Net;
use serde::{Deserialize, Serialize};

use crate::config::{Config, MemberRef};
use crate::keys::PublicKey;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AclAction {
    #[default]
    Allow,
    Deny,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AclProtocol {
    #[default]
    Any,
    Tcp,
    Udp,
    Icmp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AclTarget {
    Any(String),
    Member { member: PublicKey },
    Group { group: String },
    Subnet { subnet: Ipv4Net },
    Service { service: String },
}

impl Default for AclTarget {
    fn default() -> Self {
        Self::Any("any".to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AclGroup {
    pub id: String,
    #[serde(default)]
    pub members: Vec<PublicKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AclRule {
    pub id: String,
    pub action: AclAction,
    #[serde(default)]
    pub source: AclTarget,
    #[serde(default)]
    pub destination: AclTarget,
    #[serde(default)]
    pub protocol: AclProtocol,
    /// `80` または `8000-8100`。空は全ポート。
    // UI は再読込時に常に配列として扱うため、空でも JSON から省略しない。
    #[serde(default)]
    pub ports: Vec<String>,
    // UI のチェックボックスへ明示的に返すため、既定の true も省略しない。
    #[serde(default = "enabled")]
    pub enabled: bool,
}

fn enabled() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AclPolicy {
    pub default: AclAction,
    pub rules: Vec<ResolvedRule>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRule {
    pub id: String,
    pub action: AclAction,
    pub source: Vec<Ipv4Net>,
    pub destination: Vec<Ipv4Net>,
    pub protocol: AclProtocol,
    pub ports: Vec<(u16, u16)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AclDecision<'a> {
    pub action: AclAction,
    pub rule_id: Option<&'a str>,
}

/// Windows ユーザー空間リレーで、許可された新規通信の応答方向だけを通すための
/// 有効期限付きフロー表。Linux の conntrack `ESTABLISHED --ctdir REPLY` と同じ境界を持つ。
#[derive(Debug, Default)]
pub struct AclSessionTracker {
    replies: HashMap<ReplyKey, Session>,
    /// 許可された先頭フラグメントの (src,dst,proto,ip-id)。後続フラグメントは
    /// L4 ポートを持たないため、先頭の判定に追随させる(Linux の conntrack が
    /// 再構成してから評価するのと同じ結果にする、ADR-0035)。
    fragments: HashMap<FragmentKey, Session>,
    next_prune: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct FragmentKey {
    src: Ipv4Addr,
    dst: Ipv4Addr,
    protocol: u8,
    identification: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ReplyKey {
    Transport {
        src: Ipv4Addr,
        dst: Ipv4Addr,
        protocol: u8,
        src_port: u16,
        dst_port: u16,
    },
    IcmpEcho {
        src: Ipv4Addr,
        dst: Ipv4Addr,
        kind: u8,
        identifier: u16,
    },
}

#[derive(Debug, Clone, Copy)]
struct Session {
    expires_at: Instant,
    lifetime: Duration,
}

const TCP_SESSION_LIFETIME: Duration = Duration::from_secs(5 * 60);
const UDP_SESSION_LIFETIME: Duration = Duration::from_secs(30);
const ICMP_SESSION_LIFETIME: Duration = Duration::from_secs(10);
/// 1 データグラムのフラグメント群が届く猶予。再構成待ちに合わせて短くする。
const FRAGMENT_LIFETIME: Duration = Duration::from_secs(15);
const MAX_ACL_SESSIONS: usize = 16_384;

impl AclSessionTracker {
    /// ポリシーで許可されたパケットを観測し、応答方向のピンホールを作成・延長する。
    pub fn observe_allowed(&mut self, packet: &[u8], now: Instant) {
        self.remove_expired(now);
        let Some(candidate) = session_candidate(packet) else {
            return;
        };
        match candidate {
            SessionCandidate::Transport {
                reply,
                protocol,
                opens_session,
            } => {
                let lifetime = if protocol == 6 {
                    TCP_SESSION_LIFETIME
                } else {
                    UDP_SESSION_LIFETIME
                };
                if opens_session || self.replies.contains_key(&reply) {
                    self.insert(reply, lifetime, now);
                }
            }
            SessionCandidate::IcmpEchoRequest { reply } => {
                self.insert(reply, ICMP_SESSION_LIFETIME, now);
            }
            SessionCandidate::Other => {}
        }
    }

    /// ACL で deny となったパケットが、許可済みセッションの応答かを判定する。
    pub fn allows_reply(&mut self, packet: &[u8], now: Instant) -> bool {
        self.remove_expired(now);
        let Some(key) = packet_reply_key(packet) else {
            return false;
        };
        let Some(session) = self.replies.get_mut(&key) else {
            return false;
        };
        session.expires_at = now + session.lifetime;
        true
    }

    /// リレーを許可した先頭フラグメント(後続あり)を記録し、後続フラグメントを通せるようにする。
    /// allow・許可応答のどちらの経路でも呼ぶ。
    pub fn note_forwarded_fragment(&mut self, packet: &[u8], now: Instant) {
        if is_initial_fragment(packet) && has_more_fragments(packet) {
            if let Some(key) = fragment_key(packet) {
                self.insert_fragment(key, now);
            }
        }
    }

    /// 非先頭フラグメント(offset != 0)が、許可済みの先頭フラグメントに属するか判定する。
    /// 属していれば通す。ポリシー評価には回さない(ポートが読めないため)。
    pub fn allows_fragment(&mut self, packet: &[u8], now: Instant) -> bool {
        self.remove_expired(now);
        let Some(key) = fragment_key(packet) else {
            return false;
        };
        self.fragments.contains_key(&key)
    }

    pub fn clear(&mut self) {
        self.replies.clear();
        self.fragments.clear();
        self.next_prune = None;
    }

    fn insert(&mut self, key: ReplyKey, lifetime: Duration, now: Instant) {
        if self.replies.len() >= MAX_ACL_SESSIONS && !self.replies.contains_key(&key) {
            // 上限後のユニークUDPフローで毎回全表を走査するDoSを避ける。
            return;
        }
        self.replies.insert(
            key,
            Session {
                expires_at: now + lifetime,
                lifetime,
            },
        );
    }

    fn insert_fragment(&mut self, key: FragmentKey, now: Instant) {
        if self.fragments.len() >= MAX_ACL_SESSIONS && !self.fragments.contains_key(&key) {
            return;
        }
        self.fragments.insert(
            key,
            Session {
                expires_at: now + FRAGMENT_LIFETIME,
                lifetime: FRAGMENT_LIFETIME,
            },
        );
    }

    fn remove_expired(&mut self, now: Instant) {
        if self.next_prune.is_some_and(|next| now < next) {
            return;
        }
        self.replies.retain(|_, session| session.expires_at > now);
        self.fragments.retain(|_, session| session.expires_at > now);
        self.next_prune = Some(now + Duration::from_secs(1));
    }
}

enum SessionCandidate {
    Transport {
        reply: ReplyKey,
        protocol: u8,
        opens_session: bool,
    },
    IcmpEchoRequest {
        reply: ReplyKey,
    },
    Other,
}

impl AclPolicy {
    pub fn compile(config: &Config) -> Result<Self, String> {
        let mut rules = Vec::new();
        // 旧 deny pair は最優先の双方向 deny として扱い、従来の結果を維持する。
        for (index, (a, b)) in config.acl.normalized_deny().into_iter().enumerate() {
            for (source, destination) in [(a, b), (b, a)] {
                rules.push(ResolvedRule {
                    id: format!("legacy-deny-{index}"),
                    action: AclAction::Deny,
                    source: side_networks(config, source),
                    destination: side_networks(config, destination),
                    protocol: AclProtocol::Any,
                    ports: vec![],
                });
            }
        }
        for rule in config.acl.rules.iter().filter(|rule| rule.enabled) {
            if rule.id.trim().is_empty() || rule.id.len() > 64 {
                return Err("ACL rule id は1〜64文字にしてください".to_string());
            }
            let ports = parse_ports(&rule.ports)?;
            if !ports.is_empty() && !matches!(rule.protocol, AclProtocol::Tcp | AclProtocol::Udp) {
                return Err(format!(
                    "ACL rule {}: ports は TCP/UDP だけで指定できます",
                    rule.id
                ));
            }
            rules.push(ResolvedRule {
                id: rule.id.clone(),
                action: rule.action,
                source: resolve_target(config, &rule.source, false)?,
                destination: resolve_target(config, &rule.destination, true)?,
                protocol: rule.protocol,
                ports,
            });
        }
        Ok(Self {
            default: config.acl.default,
            rules,
        })
    }

    pub fn evaluate(
        &self,
        src: Ipv4Addr,
        dst: Ipv4Addr,
        protocol: u8,
        dst_port: Option<u16>,
    ) -> AclDecision<'_> {
        for rule in &self.rules {
            if !matches_ip(&rule.source, src) || !matches_ip(&rule.destination, dst) {
                continue;
            }
            if !protocol_matches(rule.protocol, protocol) {
                continue;
            }
            if !rule.ports.is_empty()
                && !dst_port
                    .is_some_and(|port| rule.ports.iter().any(|&(a, b)| a <= port && port <= b))
            {
                continue;
            }
            return AclDecision {
                action: rule.action,
                rule_id: Some(&rule.id),
            };
        }
        AclDecision {
            action: self.default,
            rule_id: None,
        }
    }

    pub fn evaluate_ipv4_packet(&self, packet: &[u8]) -> AclDecision<'_> {
        let Some((src, dst, protocol, port)) = packet_fields(packet) else {
            return AclDecision {
                action: self.default,
                rule_id: None,
            };
        };
        self.evaluate(src, dst, protocol, port)
    }

    /// 細粒度ルールに関係するメンバー組は直接通信させない。
    pub fn force_relay_pairs(&self, members: &[Ipv4Addr]) -> Vec<(Ipv4Addr, Ipv4Addr)> {
        let mut pairs = Vec::new();
        for (i, &a) in members.iter().enumerate() {
            for &b in &members[i + 1..] {
                let relevant = self.default == AclAction::Deny
                    || self.rules.iter().any(|rule| {
                        let directional = (matches_ip(&rule.source, a)
                            && matches_ip(&rule.destination, b))
                            || (matches_ip(&rule.source, b) && matches_ip(&rule.destination, a));
                        directional && !rule.id.starts_with("legacy-deny-")
                    });
                if relevant {
                    pairs.push((a, b));
                }
            }
        }
        pairs
    }
}

fn resolve_target(
    config: &Config,
    target: &AclTarget,
    destination: bool,
) -> Result<Vec<Ipv4Net>, String> {
    match target {
        AclTarget::Any(value) if value == "any" => Ok(vec![]),
        AclTarget::Any(_) => Err("ACL target の文字列は \"any\" だけ使用できます".to_string()),
        AclTarget::Member { member } => peer_networks(config, member)
            .ok_or_else(|| "ACLが未登録メンバーを参照しています".to_string()),
        AclTarget::Group { group } => {
            let group = config
                .acl
                .groups
                .iter()
                .find(|candidate| &candidate.id == group)
                .ok_or_else(|| format!("ACL group {group} がありません"))?;
            let mut result = Vec::new();
            for member in &group.members {
                result.extend(peer_networks(config, member).ok_or_else(|| {
                    format!("ACL group {} が未登録メンバーを参照しています", group.id)
                })?);
            }
            Ok(result)
        }
        AclTarget::Subnet { subnet } => Ok(vec![*subnet]),
        AclTarget::Service { service } if destination => {
            let record = config
                .dns_records
                .iter()
                .find(|record| record.id.as_deref() == Some(service))
                .ok_or_else(|| format!("ACL service {service} がありません"))?;
            if record.cname.is_some() {
                return Err("外部CNAMEはACL対象にできません".to_string());
            }
            if record.port.is_none() {
                return Err(format!("ACL service {service} にportがありません"));
            }
            if let Some(ip) = record.ip {
                Ok(vec![Ipv4Net::new(ip, 32).unwrap()])
            } else if let Some(MemberRef::Key(key)) = record.member {
                peer_networks(config, &key)
                    .map(|mut nets| {
                        nets.truncate(1);
                        nets
                    })
                    .ok_or_else(|| "ACL service のメンバーがありません".to_string())
            } else {
                Err("ACL service の宛先を解決できません".to_string())
            }
        }
        AclTarget::Service { .. } => Err("service は宛先だけに指定できます".to_string()),
    }
}

fn peer_networks(config: &Config, key: &PublicKey) -> Option<Vec<Ipv4Net>> {
    let peer = config.peers.iter().find(|peer| &peer.public_key == key)?;
    let mut nets = peer.allowed_ips.clone();
    nets.extend(peer.subnets.iter().copied());
    Some(nets)
}

fn side_networks(config: &Config, ip: Ipv4Addr) -> Vec<Ipv4Net> {
    let mut nets = vec![Ipv4Net::new(ip, 32).unwrap()];
    if let Some(peer) = config
        .peers
        .iter()
        .find(|peer| peer.allowed_ips.iter().any(|net| net.contains(&ip)))
    {
        nets.extend(peer.subnets.iter().copied());
    }
    nets
}

fn parse_ports(values: &[String]) -> Result<Vec<(u16, u16)>, String> {
    values
        .iter()
        .map(|value| {
            let (a, b) = value.split_once('-').unwrap_or((value, value));
            let start: u16 = a
                .parse()
                .map_err(|_| format!("ACL port {value} が不正です"))?;
            let end: u16 = b
                .parse()
                .map_err(|_| format!("ACL port {value} が不正です"))?;
            if start == 0 || start > end {
                return Err(format!("ACL port {value} が不正です"));
            }
            Ok((start, end))
        })
        .collect()
}

fn matches_ip(nets: &[Ipv4Net], ip: Ipv4Addr) -> bool {
    nets.is_empty() || nets.iter().any(|net| net.contains(&ip))
}
fn protocol_matches(expected: AclProtocol, actual: u8) -> bool {
    matches!(expected, AclProtocol::Any)
        || matches!(
            (expected, actual),
            (AclProtocol::Tcp, 6) | (AclProtocol::Udp, 17) | (AclProtocol::Icmp, 1)
        )
}

fn packet_fields(packet: &[u8]) -> Option<(Ipv4Addr, Ipv4Addr, u8, Option<u16>)> {
    let (src, dst, protocol, header_len) = ipv4_fields(packet)?;
    // 非先頭フラグメント(offset != 0)では header_len 以降はポートではなく
    // ペイロードなので、ポートを読まず None にする。ここを読むと後続フラグメントが
    // ゴミポートで評価され、Linux(conntrack が再構成する)と食い違う(ADR-0035)。
    let port = if matches!(protocol, 6 | 17)
        && is_initial_fragment(packet)
        && packet.len() >= header_len + 4
    {
        Some(u16::from_be_bytes([
            packet[header_len + 2],
            packet[header_len + 3],
        ]))
    } else {
        None
    };
    Some((src, dst, protocol, port))
}

fn ipv4_fields(packet: &[u8]) -> Option<(Ipv4Addr, Ipv4Addr, u8, usize)> {
    if packet.len() < 20 || packet[0] >> 4 != 4 {
        return None;
    }
    let header_len = usize::from(packet[0] & 0x0f) * 4;
    if header_len < 20 || packet.len() < header_len {
        return None;
    }
    Some((
        Ipv4Addr::new(packet[12], packet[13], packet[14], packet[15]),
        Ipv4Addr::new(packet[16], packet[17], packet[18], packet[19]),
        packet[9],
        header_len,
    ))
}

fn session_candidate(packet: &[u8]) -> Option<SessionCandidate> {
    if !is_initial_fragment(packet) {
        return None;
    }
    let (src, dst, protocol, header_len) = ipv4_fields(packet)?;
    match protocol {
        6 | 17 if packet.len() >= header_len + 4 => {
            let src_port = u16::from_be_bytes([packet[header_len], packet[header_len + 1]]);
            let dst_port = u16::from_be_bytes([packet[header_len + 2], packet[header_len + 3]]);
            let opens_session = if protocol == 17 {
                true
            } else if packet.len() >= header_len + 14 {
                let flags = packet[header_len + 13];
                flags & 0x02 != 0 && flags & 0x10 == 0 // SYN && !ACK
            } else {
                false
            };
            Some(SessionCandidate::Transport {
                reply: ReplyKey::Transport {
                    src: dst,
                    dst: src,
                    protocol,
                    src_port: dst_port,
                    dst_port: src_port,
                },
                protocol,
                opens_session,
            })
        }
        1 if packet.len() >= header_len + 8 && packet[header_len] == 8 => {
            let identifier = u16::from_be_bytes([packet[header_len + 4], packet[header_len + 5]]);
            Some(SessionCandidate::IcmpEchoRequest {
                reply: ReplyKey::IcmpEcho {
                    src: dst,
                    dst: src,
                    kind: 0,
                    identifier,
                },
            })
        }
        _ => Some(SessionCandidate::Other),
    }
}

fn packet_reply_key(packet: &[u8]) -> Option<ReplyKey> {
    if !is_initial_fragment(packet) {
        return None;
    }
    let (src, dst, protocol, header_len) = ipv4_fields(packet)?;
    match protocol {
        6 | 17 if packet.len() >= header_len + 4 => Some(ReplyKey::Transport {
            src,
            dst,
            protocol,
            src_port: u16::from_be_bytes([packet[header_len], packet[header_len + 1]]),
            dst_port: u16::from_be_bytes([packet[header_len + 2], packet[header_len + 3]]),
        }),
        1 if packet.len() >= header_len + 8 && matches!(packet[header_len], 0 | 8) => {
            Some(ReplyKey::IcmpEcho {
                src,
                dst,
                kind: packet[header_len],
                identifier: u16::from_be_bytes([packet[header_len + 4], packet[header_len + 5]]),
            })
        }
        _ => None,
    }
}

fn is_initial_fragment(packet: &[u8]) -> bool {
    packet.len() >= 8 && u16::from_be_bytes([packet[6], packet[7]]) & 0x1fff == 0
}

/// 非先頭フラグメント(フラグメントオフセット != 0)か。IPv4 以外・短すぎるパケットは false。
pub fn is_non_first_fragment(packet: &[u8]) -> bool {
    packet.len() >= 20
        && packet[0] >> 4 == 4
        && u16::from_be_bytes([packet[6], packet[7]]) & 0x1fff != 0
}

/// More Fragments フラグ。先頭フラグメント(offset==0)でこれが立つと後続がある。
fn has_more_fragments(packet: &[u8]) -> bool {
    packet.len() >= 8 && packet[6] & 0x20 != 0
}

fn fragment_key(packet: &[u8]) -> Option<FragmentKey> {
    let (src, dst, protocol, _) = ipv4_fields(packet)?;
    Some(FragmentKey {
        src,
        dst,
        protocol,
        identification: u16::from_be_bytes([packet[4], packet[5]]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> AclPolicy {
        AclPolicy {
            default: AclAction::Allow,
            rules: vec![
                ResolvedRule {
                    id: "web".into(),
                    action: AclAction::Allow,
                    source: vec!["10.0.0.0/24".parse().unwrap()],
                    destination: vec!["10.0.1.2/32".parse().unwrap()],
                    protocol: AclProtocol::Tcp,
                    ports: vec![(443, 443)],
                },
                ResolvedRule {
                    id: "deny-server".into(),
                    action: AclAction::Deny,
                    source: vec![],
                    destination: vec!["10.0.1.0/24".parse().unwrap()],
                    protocol: AclProtocol::Any,
                    ports: vec![],
                },
            ],
        }
    }

    #[test]
    fn first_match_direction_protocol_port_range_icmp_and_default() {
        let p = policy();
        assert_eq!(
            p.evaluate(
                "10.0.0.5".parse().unwrap(),
                "10.0.1.2".parse().unwrap(),
                6,
                Some(443)
            )
            .action,
            AclAction::Allow
        );
        assert_eq!(
            p.evaluate(
                "10.0.0.5".parse().unwrap(),
                "10.0.1.2".parse().unwrap(),
                17,
                Some(443)
            )
            .rule_id,
            Some("deny-server")
        );
        assert_eq!(
            p.evaluate(
                "10.0.1.2".parse().unwrap(),
                "10.0.0.5".parse().unwrap(),
                6,
                Some(443)
            )
            .action,
            AclAction::Allow,
            "directional"
        );
        assert_eq!(
            p.evaluate(
                "192.0.2.1".parse().unwrap(),
                "10.0.1.9".parse().unwrap(),
                1,
                None
            )
            .action,
            AclAction::Deny
        );
        assert_eq!(
            p.evaluate(
                "192.0.2.1".parse().unwrap(),
                "192.0.2.2".parse().unwrap(),
                1,
                None
            )
            .action,
            AclAction::Allow
        );
    }

    #[test]
    fn parses_single_and_range_ports() {
        assert_eq!(
            parse_ports(&["53".into(), "8000-8100".into()]).unwrap(),
            vec![(53, 53), (8000, 8100)]
        );
        assert!(parse_ports(&["0".into()]).is_err());
        assert!(parse_ports(&["10-2".into()]).is_err());
    }

    #[test]
    fn rule_json_keeps_ui_defaults() {
        let rule = AclRule {
            id: "default-fields".into(),
            action: AclAction::Deny,
            source: AclTarget::Any("any".into()),
            destination: AclTarget::Any("any".into()),
            protocol: AclProtocol::Any,
            ports: vec![],
            enabled: true,
        };
        let json = serde_json::to_value(rule).unwrap();
        assert_eq!(json["ports"], serde_json::json!([]));
        assert_eq!(json["enabled"], serde_json::json!(true));
    }

    fn transport_packet(
        src: Ipv4Addr,
        dst: Ipv4Addr,
        protocol: u8,
        src_port: u16,
        dst_port: u16,
        tcp_flags: u8,
    ) -> Vec<u8> {
        let transport_len = if protocol == 6 { 20 } else { 8 };
        let mut packet = vec![0u8; 20 + transport_len];
        packet[0] = 0x45;
        packet[9] = protocol;
        packet[12..16].copy_from_slice(&src.octets());
        packet[16..20].copy_from_slice(&dst.octets());
        packet[20..22].copy_from_slice(&src_port.to_be_bytes());
        packet[22..24].copy_from_slice(&dst_port.to_be_bytes());
        if protocol == 6 {
            packet[33] = tcp_flags;
        }
        packet
    }

    fn icmp_echo_packet(src: Ipv4Addr, dst: Ipv4Addr, kind: u8, identifier: u16) -> Vec<u8> {
        let mut packet = vec![0u8; 28];
        packet[0] = 0x45;
        packet[9] = 1;
        packet[12..16].copy_from_slice(&src.octets());
        packet[16..20].copy_from_slice(&dst.octets());
        packet[20] = kind;
        packet[24..26].copy_from_slice(&identifier.to_be_bytes());
        packet
    }

    /// フラグメント断片を作る。`offset_units` は 8 バイト単位、`more` は MF フラグ。
    /// 先頭(offset==0)だけ dst_port を持つ(src_port は 40000 固定)。
    fn fragment_packet(
        src: Ipv4Addr,
        dst: Ipv4Addr,
        identification: u16,
        offset_units: u16,
        more: bool,
        dst_port: u16,
    ) -> Vec<u8> {
        let mut packet = vec![0u8; 28];
        packet[0] = 0x45;
        packet[4..6].copy_from_slice(&identification.to_be_bytes());
        let flags_frag = (offset_units & 0x1fff) | if more { 0x2000 } else { 0 };
        packet[6..8].copy_from_slice(&flags_frag.to_be_bytes());
        packet[9] = 17; // UDP
        packet[12..16].copy_from_slice(&src.octets());
        packet[16..20].copy_from_slice(&dst.octets());
        if offset_units == 0 {
            packet[20..22].copy_from_slice(&40_000u16.to_be_bytes());
            packet[22..24].copy_from_slice(&dst_port.to_be_bytes());
        }
        packet
    }

    #[test]
    fn non_first_fragment_follows_allowed_head_not_default() {
        // default=deny + allow A→B udp 51820。フラグメント化した UDP を通す。
        let a: Ipv4Addr = "10.0.0.5".parse().unwrap();
        let b: Ipv4Addr = "10.0.1.2".parse().unwrap();
        let p = AclPolicy {
            default: AclAction::Deny,
            rules: vec![ResolvedRule {
                id: "game".into(),
                action: AclAction::Allow,
                source: vec!["10.0.0.0/24".parse().unwrap()],
                destination: vec!["10.0.1.2/32".parse().unwrap()],
                protocol: AclProtocol::Udp,
                ports: vec![(51820, 51820)],
            }],
        };
        let now = Instant::now();
        let mut tracker = AclSessionTracker::default();

        // 先頭フラグメント(ポート 51820、後続あり)は許可され、印が残る。
        let head = fragment_packet(a, b, 0x1234, 0, true, 51820);
        assert_eq!(p.evaluate_ipv4_packet(&head).action, AclAction::Allow);
        tracker.note_forwarded_fragment(&head, now);

        // 非先頭フラグメント(ポートを持たない)は、印に追随して通る。
        let tail = fragment_packet(a, b, 0x1234, 185, false, 0);
        assert!(is_non_first_fragment(&tail));
        assert!(tracker.allows_fragment(&tail, now));

        // 別データグラム(ip-id 違い)の非先頭フラグメントは印が無く通らない。
        let stray = fragment_packet(a, b, 0x9999, 185, false, 0);
        assert!(!tracker.allows_fragment(&stray, now));
    }

    #[test]
    fn stateful_tracker_allows_only_tcp_reply_direction() {
        let a: Ipv4Addr = "10.0.0.2".parse().unwrap();
        let b: Ipv4Addr = "10.0.0.3".parse().unwrap();
        let now = Instant::now();
        let mut tracker = AclSessionTracker::default();

        let b_syn = transport_packet(b, a, 6, 50_000, 443, 0x02);
        let a_syn_ack = transport_packet(a, b, 6, 443, 50_000, 0x12);
        let unrelated_a_syn = transport_packet(a, b, 6, 443, 50_001, 0x02);
        tracker.observe_allowed(&b_syn, now);
        assert!(tracker.allows_reply(&a_syn_ack, now));
        assert!(!tracker.allows_reply(&unrelated_a_syn, now));

        assert!(!tracker.allows_reply(
            &a_syn_ack,
            now + TCP_SESSION_LIFETIME + Duration::from_secs(1)
        ));
    }

    #[test]
    fn stateful_tracker_matches_udp_ports_and_icmp_identifier() {
        let a: Ipv4Addr = "10.0.0.2".parse().unwrap();
        let b: Ipv4Addr = "10.0.0.3".parse().unwrap();
        let now = Instant::now();
        let mut tracker = AclSessionTracker::default();

        tracker.observe_allowed(&transport_packet(b, a, 17, 50_000, 53, 0), now);
        assert!(tracker.allows_reply(&transport_packet(a, b, 17, 53, 50_000, 0), now));
        assert!(!tracker.allows_reply(&transport_packet(a, b, 17, 53, 50_001, 0), now));

        tracker.observe_allowed(&icmp_echo_packet(b, a, 8, 42), now);
        assert!(tracker.allows_reply(&icmp_echo_packet(a, b, 0, 42), now));
        assert!(!tracker.allows_reply(&icmp_echo_packet(a, b, 0, 43), now));
    }

    #[test]
    fn stateful_tracker_does_not_open_tcp_session_without_initial_syn() {
        let a: Ipv4Addr = "10.0.0.2".parse().unwrap();
        let b: Ipv4Addr = "10.0.0.3".parse().unwrap();
        let now = Instant::now();
        let mut tracker = AclSessionTracker::default();
        tracker.observe_allowed(&transport_packet(b, a, 6, 50_000, 443, 0x10), now);
        assert!(!tracker.allows_reply(&transport_packet(a, b, 6, 443, 50_000, 0x10), now));
    }
}
