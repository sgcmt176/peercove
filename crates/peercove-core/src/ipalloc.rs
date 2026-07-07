//! 仮想 IP 割当ヘルパ。
//!
//! M0 では add-peer 時の推奨 IP 提示に使う。M1 の台帳ベース割当でも流用する想定。

use std::net::Ipv4Addr;

use ipnet::Ipv4Net;

/// サブネット内で未使用の最小ホストアドレスを返す。
/// ネットワーク・ブロードキャストアドレスは対象外。空きが無ければ `None`。
pub fn next_free_ip(net: Ipv4Net, used: &[Ipv4Addr]) -> Option<Ipv4Addr> {
    net.hosts().find(|ip| !used.contains(ip))
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
}
