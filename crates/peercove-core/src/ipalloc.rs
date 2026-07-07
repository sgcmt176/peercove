//! 仮想 IP 割当ヘルパ。
//!
//! M0 では add-peer 時の推奨 IP 提示に使う。M1 の台帳ベース割当でも流用する想定。

use std::net::Ipv4Addr;

use ipnet::Ipv4Net;
use rand_core::{OsRng, RngCore};

/// サブネット内で未使用の最小ホストアドレスを返す。
/// ネットワーク・ブロードキャストアドレスは対象外。空きが無ければ `None`。
pub fn next_free_ip(net: Ipv4Net, used: &[Ipv4Addr]) -> Option<Ipv4Addr> {
    net.hosts().find(|ip| !used.contains(ip))
}

/// トンネル用のサブネットをランダムに生成する(ADR-0006)。
///
/// `10.x.y.0/24`(x = 64〜127、y = 0〜255)から選ぶ。Tailscale の CGNAT レンジ
/// (100.64.0.0/10)、家庭 LAN に多い 192.168.0.0/16 や 10.0.x/10.1.x、
/// Docker 既定の 172.17.0.0/16 をすべて避けた帯。
pub fn random_private_subnet() -> Ipv4Net {
    let mut bytes = [0u8; 2];
    OsRng.fill_bytes(&mut bytes);
    let x = 64 + (bytes[0] & 0x3F); // 64..=127
    let y = bytes[1];
    Ipv4Net::new(Ipv4Addr::new(10, x, y, 0), 24).expect("/24 は常に有効")
}

/// Tailscale 等が使う CGNAT レンジ(100.64.0.0/10)と重なるか。
/// 重なる場合、Tailscale 導入機ではパケットが DROP される(decisions.md 参照)。
pub fn overlaps_cgnat(net: Ipv4Net) -> bool {
    let cgnat: Ipv4Net = "100.64.0.0/10".parse().expect("固定値");
    cgnat.contains(&net.addr()) || net.contains(&cgnat.addr())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net() -> Ipv4Net {
        "100.100.42.0/24".parse().unwrap()
    }

    fn ip(s: &str) -> Ipv4Addr {
        s.parse().unwrap()
    }

    #[test]
    fn first_host_when_unused() {
        assert_eq!(next_free_ip(net(), &[]), Some(ip("100.100.42.1")));
    }

    #[test]
    fn skips_used_addresses() {
        let used = [ip("100.100.42.1"), ip("100.100.42.2"), ip("100.100.42.4")];
        assert_eq!(next_free_ip(net(), &used), Some(ip("100.100.42.3")));
    }

    #[test]
    fn none_when_exhausted() {
        let small: Ipv4Net = "100.100.42.0/30".parse().unwrap();
        let used = [ip("100.100.42.1"), ip("100.100.42.2")];
        assert_eq!(next_free_ip(small, &used), None);
    }

    #[test]
    fn random_subnet_stays_in_safe_range() {
        for _ in 0..100 {
            let net = random_private_subnet();
            let octets = net.addr().octets();
            assert_eq!(octets[0], 10);
            assert!((64..=127).contains(&octets[1]), "x={} が範囲外", octets[1]);
            assert_eq!(octets[3], 0);
            assert_eq!(net.prefix_len(), 24);
            assert!(!overlaps_cgnat(net));
        }
    }

    #[test]
    fn cgnat_overlap_detection() {
        assert!(overlaps_cgnat("100.100.42.0/24".parse().unwrap()));
        assert!(overlaps_cgnat("100.64.0.0/10".parse().unwrap()));
        assert!(overlaps_cgnat("100.0.0.0/8".parse().unwrap())); // CGNAT を包含
        assert!(!overlaps_cgnat("10.100.42.0/24".parse().unwrap()));
        assert!(!overlaps_cgnat("192.168.0.0/24".parse().unwrap()));
    }
}
