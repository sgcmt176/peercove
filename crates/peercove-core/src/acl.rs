//! ACL v2 の設定型と OS 非依存のパケット評価器 (ADR-0035)。

use std::net::Ipv4Addr;

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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<String>,
    #[serde(default = "enabled", skip_serializing_if = "std::ops::Not::not")]
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
    if packet.len() < 20 || packet[0] >> 4 != 4 {
        return None;
    }
    let header_len = usize::from(packet[0] & 0x0f) * 4;
    if header_len < 20 || packet.len() < header_len {
        return None;
    }
    let src = Ipv4Addr::new(packet[12], packet[13], packet[14], packet[15]);
    let dst = Ipv4Addr::new(packet[16], packet[17], packet[18], packet[19]);
    let protocol = packet[9];
    let port = if matches!(protocol, 6 | 17) && packet.len() >= header_len + 4 {
        Some(u16::from_be_bytes([
            packet[header_len + 2],
            packet[header_len + 3],
        ]))
    } else {
        None
    };
    Some((src, dst, protocol, port))
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
}
