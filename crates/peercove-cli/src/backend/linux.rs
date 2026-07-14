//! Linux バックエンド: カーネル WireGuard を netlink(defguard_wireguard_rs)で制御する。
//!
//! ルーティングは interface.address のプレフィックス(例 /24)の connected route に
//! 任せるため、`configure_peer_routing` は呼ばない(M0 の allowed_ips はすべて
//! そのサブネット内に収まる前提。G-3 のハブ&スポークもこれで成立する)。

use anyhow::{bail, Context};
use defguard_wireguard_rs::error::WireguardInterfaceError;
use defguard_wireguard_rs::key::Key;
use defguard_wireguard_rs::net::IpAddrMask;
use defguard_wireguard_rs::peer::Peer;
use defguard_wireguard_rs::{InterfaceConfiguration, Kernel, WGApi, WireguardInterfaceApi};

use super::{IsolatedPeer, PeerSpec, PeerStats, TunnelSpec, WgBackend};
use peercove_core::acl::{AclAction, AclPolicy, AclProtocol, ResolvedRule};
use peercove_core::proto::CONTROL_PORT;

pub struct LinuxBackend {
    if_name: String,
    api: WGApi<Kernel>,
    /// ルーター役(ADR-0014)として適用中の状態。down で対解除する。
    router: RouterState,
    /// ACL(ADR-0018/0035)で適用中のルール。down で対解除する。
    acl_rules: Vec<Vec<String>>,
    acl_policy: Option<AclPolicy>,
    acl_generation: u64,
    /// 承認待ち端末に適用中の仮想 IP。INPUT/OUTPUT/FORWARD の解除に使う。
    isolated_ips: Vec<std::net::Ipv4Addr>,
}

#[cfg(test)]
mod acl_tests {
    use super::*;

    #[test]
    fn acl_rule_arguments_match_common_protocol_and_port_semantics() {
        let rule = ResolvedRule {
            id: "allow-game".into(),
            action: AclAction::Allow,
            source: vec!["10.0.0.0/24".parse().unwrap()],
            destination: vec!["10.0.1.2/32".parse().unwrap()],
            protocol: AclProtocol::Udp,
            ports: vec![(25565, 25570)],
        };
        let args = acl_rule_args(
            "-I",
            "pcv0",
            &rule,
            "10.0.0.0/24",
            "10.0.1.2/32",
            Some((25565, 25570)),
            "test",
        );
        assert!(args.windows(2).any(|pair| pair == ["-p", "udp"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--dport", "25565:25570"]));
        assert!(args.windows(2).any(|pair| pair == ["-j", "ACCEPT"]));
    }

    #[test]
    fn acl_reply_rule_accepts_only_conntrack_reply_direction() {
        let args = acl_reply_rule_args("-I", "pcv0", "test");
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--ctstate", "ESTABLISHED,RELATED"]));
        assert!(args.windows(2).any(|pair| pair == ["--ctdir", "REPLY"]));
        assert!(args.windows(2).any(|pair| pair == ["-j", "ACCEPT"]));
    }
}

/// ルーター役の適用済み状態(サブネットごとの NAT ルールと、
/// 転送を有効化した LAN 側 IF)。
#[derive(Default)]
struct RouterState {
    /// 適用済みサブネット(NAT ルールの -d と一致)。
    subnets: Vec<(ipnet::Ipv4Net, bool)>, // (subnet, snat 適用済みか)
    /// 仮想サブネット(NAT ルールの -s。解除時に同じ引数が要る)。
    virtual_subnet: Option<ipnet::Ipv4Net>,
    /// このプロセスが 0→1 にした LAN 側 IF(down で 0 に戻す)。
    enabled_forwarding: Vec<String>,
}

/// 外部コマンドを実行し、失敗なら stderr 込みでエラーにする。
fn run(program: &str, args: &[&str]) -> anyhow::Result<String> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("{program} を実行できません(インストールされていますか?)"))?;
    if !output.status.success() {
        bail!(
            "{program} {} が失敗しました: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// MASQUERADE ルールの引数(追加 -A / 確認 -C / 削除 -D で共通)。
fn nat_rule_args(op: &str, virt: &str, subnet: &str) -> Vec<String> {
    [
        "-t",
        "nat",
        op,
        "POSTROUTING",
        "-s",
        virt,
        "-d",
        subnet,
        "-j",
        "MASQUERADE",
        "-m",
        "comment",
        "--comment",
        "peercove-subnet-router",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// FORWARD 許可ルールの引数。Docker 等が FORWARD の既定ポリシーを DROP に
/// している環境では、これが無いとトンネル ↔ LAN の転送が落ちる。
/// `-I`(先頭挿入)で Docker の隔離ルールより先に評価させる。
fn forward_rule_args(op: &str, src: &str, dst: &str) -> Vec<String> {
    [
        op,
        "FORWARD",
        "-s",
        src,
        "-d",
        dst,
        "-j",
        "ACCEPT",
        "-m",
        "comment",
        "--comment",
        "peercove-subnet-router",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// ACL の DROP ルールの引数(ADR-0018)。トンネル IF に入りトンネル IF から
/// 出る(= ホストがリレーする)トラフィックだけを対象にする。`-I`(先頭挿入)で
/// 既存の ACCEPT 系ルールより先に評価させる。
fn acl_rule_args(
    op: &str,
    wg_if: &str,
    rule: &ResolvedRule,
    src: &str,
    dst: &str,
    port: Option<(u16, u16)>,
    comment: &str,
) -> Vec<String> {
    let mut args: Vec<String> = [
        op, "FORWARD", "-i", wg_if, "-o", wg_if, "-s", src, "-d", dst,
    ]
    .iter()
    .map(|value| value.to_string())
    .collect();
    match rule.protocol {
        AclProtocol::Any => {}
        AclProtocol::Tcp => args.extend(["-p".into(), "tcp".into()]),
        AclProtocol::Udp => args.extend(["-p".into(), "udp".into()]),
        AclProtocol::Icmp => args.extend(["-p".into(), "icmp".into()]),
    }
    if let Some((start, end)) = port {
        args.extend([
            "--dport".into(),
            if start == end {
                start.to_string()
            } else {
                format!("{start}:{end}")
            },
        ]);
    }
    args.extend([
        "-j".into(),
        match rule.action {
            AclAction::Allow => "ACCEPT",
            AclAction::Deny => "DROP",
        }
        .into(),
        "-m".into(),
        "comment".into(),
        "--comment".into(),
        comment.into(),
    ]);
    args
}

/// 許可された逆方向セッションの応答だけを、通常の方向 ACL より先に通す。
/// `--ctdir REPLY` を付けるため、元の開始方向は引き続き通常ルールで評価される。
fn acl_reply_rule_args(op: &str, wg_if: &str, comment: &str) -> Vec<String> {
    [
        op,
        "FORWARD",
        "-i",
        wg_if,
        "-o",
        wg_if,
        "-m",
        "conntrack",
        "--ctstate",
        "ESTABLISHED,RELATED",
        "--ctdir",
        "REPLY",
        "-j",
        "ACCEPT",
        "-m",
        "comment",
        "--comment",
        comment,
    ]
    .iter()
    .map(|value| value.to_string())
    .collect()
}

fn isolation_rule_args(op: &str, wg_if: &str, ip: std::net::Ipv4Addr) -> Vec<Vec<String>> {
    let ip = format!("{ip}/32");
    let port = CONTROL_PORT.to_string();
    let rule = |parts: &[&str]| {
        parts
            .iter()
            .copied()
            .chain(["-m", "comment", "--comment", "peercove-invite-isolation"])
            .map(str::to_string)
            .collect()
    };
    // DROP を先に挿入し、ACCEPT を最後に -I することで許可規則が最上位になる。
    vec![
        rule(&[op, "FORWARD", "-i", wg_if, "-s", &ip, "-j", "DROP"]),
        rule(&[op, "FORWARD", "-o", wg_if, "-d", &ip, "-j", "DROP"]),
        rule(&[op, "INPUT", "-i", wg_if, "-s", &ip, "-j", "DROP"]),
        rule(&[op, "OUTPUT", "-o", wg_if, "-d", &ip, "-j", "DROP"]),
        rule(&[
            op, "INPUT", "-i", wg_if, "-s", &ip, "-p", "tcp", "--dport", &port, "-j", "ACCEPT",
        ]),
        rule(&[
            op, "OUTPUT", "-o", wg_if, "-d", &ip, "-p", "tcp", "--sport", &port, "-j", "ACCEPT",
        ]),
    ]
}

fn iptables(args: &[String]) -> anyhow::Result<String> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    run("iptables", &args)
}

impl LinuxBackend {
    pub fn new(if_name: &str) -> anyhow::Result<Self> {
        let api = WGApi::<Kernel>::new(if_name.to_string())
            .with_context(|| format!("WG API の初期化に失敗しました({if_name})"))?;
        Ok(Self {
            if_name: if_name.to_string(),
            api,
            router: RouterState::default(),
            acl_rules: Vec::new(),
            acl_policy: None,
            acl_generation: 0,
            isolated_ips: Vec::new(),
        })
    }

    /// 適用中の ACL ルールをすべて解除する(down・全解除の共通処理)。
    fn clear_acl(&mut self) {
        for mut args in std::mem::take(&mut self.acl_rules) {
            args[0] = "-D".to_string();
            if let Err(e) = iptables(&args) {
                tracing::warn!("ACL ルールの削除に失敗しました: {e:#}");
            }
        }
        self.acl_policy = None;
    }

    fn clear_isolation(&mut self) {
        for ip in std::mem::take(&mut self.isolated_ips) {
            // 削除は挿入と逆順でなくても、各規則が一意なので安全。
            for args in isolation_rule_args("-D", &self.if_name, ip) {
                if let Err(e) = iptables(&args) {
                    tracing::warn!("招待隔離ルールの削除に失敗しました({ip}): {e:#}");
                }
            }
        }
    }

    /// サブネットへの経路が向く LAN 側 IF を特定する(`ip route get`)。
    fn lan_interface_for(&self, subnet: &ipnet::Ipv4Net) -> anyhow::Result<String> {
        let probe = subnet
            .hosts()
            .next()
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| subnet.addr().to_string());
        let out = run("ip", &["-4", "route", "get", &probe])?;
        let mut tokens = out.split_whitespace();
        while let Some(token) = tokens.next() {
            if token == "dev" {
                if let Some(dev) = tokens.next() {
                    if dev == self.if_name {
                        bail!(
                            "サブネット {subnet} への経路がトンネル {} を向いています。\
                             ルーター役のマシンから直接届く LAN を指定してください",
                            self.if_name
                        );
                    }
                    return Ok(dev.to_string());
                }
            }
        }
        bail!("サブネット {subnet} への経路(LAN 側 IF)を特定できません: {out}")
    }

    /// LAN 側 IF の per-IF forwarding を有効化する(0→1 にしたときだけ記録し、
    /// down で戻す)。
    fn ensure_forwarding(&mut self, if_name: &str) -> anyhow::Result<()> {
        let path = format!("/proc/sys/net/ipv4/conf/{if_name}/forwarding");
        let current = std::fs::read_to_string(&path).unwrap_or_default();
        if current.trim() != "1" {
            std::fs::write(&path, "1")
                .with_context(|| format!("IP フォワーディングの有効化に失敗しました({path})"))?;
            if !self
                .router
                .enabled_forwarding
                .contains(&if_name.to_string())
            {
                self.router.enabled_forwarding.push(if_name.to_string());
            }
            tracing::info!("IP フォワーディングを有効化しました({path} = 1)");
        }
        Ok(())
    }

    /// 1 サブネット分の iptables ルール(NAT + FORWARD 両方向)を削除する。
    fn remove_subnet_rules(virt: &ipnet::Ipv4Net, subnet: &ipnet::Ipv4Net, snat: bool) {
        let (virt, subnet) = (virt.to_string(), subnet.to_string());
        if snat {
            if let Err(e) = iptables(&nat_rule_args("-D", &virt, &subnet)) {
                tracing::warn!("NAT ルールの削除に失敗しました({subnet}): {e:#}");
            }
        }
        for (src, dst) in [(&virt, &subnet), (&subnet, &virt)] {
            if let Err(e) = iptables(&forward_rule_args("-D", src, dst)) {
                tracing::warn!("FORWARD ルールの削除に失敗しました({src}→{dst}): {e:#}");
            }
        }
    }

    /// ルーター役の適用済み状態をすべて解除する(down・全撤回の共通処理)。
    fn clear_router(&mut self) {
        if let Some(virt) = self.router.virtual_subnet {
            for (subnet, snat) in std::mem::take(&mut self.router.subnets) {
                Self::remove_subnet_rules(&virt, &subnet, snat);
            }
        }
        for if_name in std::mem::take(&mut self.router.enabled_forwarding) {
            let path = format!("/proc/sys/net/ipv4/conf/{if_name}/forwarding");
            if let Err(e) = std::fs::write(&path, "0") {
                tracing::warn!("IP フォワーディングの復元に失敗しました({path}): {e:#}");
            }
        }
        self.router.virtual_subnet = None;
    }

    fn ensure_root() -> anyhow::Result<()> {
        // SAFETY: geteuid は引数なし・常に成功する POSIX API。OS 境界のため unsafe。
        let euid = unsafe { libc::geteuid() };
        if euid != 0 {
            bail!("root 権限が必要です。sudo を付けて再実行してください");
        }
        Ok(())
    }

    /// ピア間転送のためインターフェース単位の IP フォワーディングを有効化する
    /// (ADR-0003)。この設定はインターフェースの消滅とともに消えるため、
    /// `down` での原状回復は不要。グローバルの ip_forward は変更しない。
    fn enable_forwarding(&self) -> anyhow::Result<()> {
        let path = format!("/proc/sys/net/ipv4/conf/{}/forwarding", self.if_name);
        std::fs::write(&path, "1")
            .with_context(|| format!("IP フォワーディングの有効化に失敗しました({path})"))?;
        tracing::info!("IP フォワーディングを有効化しました({path} = 1)");
        Ok(())
    }
}

fn to_peer(spec: &PeerSpec) -> Peer {
    let mut peer = Peer::new(Key::new(*spec.public_key.as_bytes()));
    peer.endpoint = spec.endpoint;
    peer.persistent_keepalive_interval = spec.persistent_keepalive;
    peer.preshared_key = spec
        .preshared_key
        .as_ref()
        .map(|psk| Key::new(*psk.as_bytes()));
    peer.allowed_ips = spec
        .allowed_ips
        .iter()
        .map(|net| IpAddrMask::new(net.addr().into(), net.prefix_len()))
        .collect();
    peer
}

impl WgBackend for LinuxBackend {
    fn up(&mut self, spec: &TunnelSpec) -> anyhow::Result<()> {
        Self::ensure_root()?;
        self.api.create_interface().map_err(|e| {
            anyhow::anyhow!(e).context(format!(
                "インターフェース {} の作成に失敗しました。既に存在する場合は \
                 `peercove down` で残骸を掃除してください。カーネル WireGuard \
                 モジュールが無い場合は `sudo modprobe wireguard` を試してください",
                self.if_name
            ))
        })?;

        let config = InterfaceConfiguration {
            name: self.if_name.clone(),
            prvkey: spec.private_key.to_base64(),
            addresses: vec![IpAddrMask::new(
                spec.address.addr().into(),
                spec.address.prefix_len(),
            )],
            port: spec.listen_port.unwrap_or(0),
            peers: spec.peers.iter().map(to_peer).collect(),
            mtu: Some(u32::from(spec.mtu)),
            fwmark: None,
        };
        if let Err(e) = self.api.configure_interface(&config) {
            // 設定に失敗したら作りかけの TUN を残さない
            let _ = self.api.remove_interface();
            return Err(anyhow::anyhow!(e).context("インターフェースの設定に失敗しました"));
        }
        if spec.forwarding {
            if let Err(e) = self.enable_forwarding() {
                let _ = self.api.remove_interface();
                return Err(e);
            }
        }
        Ok(())
    }

    fn add_peer(&mut self, peer: &PeerSpec) -> anyhow::Result<()> {
        Self::ensure_root()?;
        // カーネル WG のピア設定は元々 upsert(既存ピアは属性更新、
        // セッション維持)。roaming 学習済みエンドポイントを台帳の値で
        // 上書きしないよう、更新の呼び出し(direct.rs の確立)は endpoint を
        // 渡さない(ADR-0019)
        self.api
            .configure_peer(&to_peer(peer))
            .with_context(|| format!("ピア {} の追加に失敗しました", peer.public_key))
    }

    fn remove_peer(&mut self, public_key: &peercove_core::keys::PublicKey) -> anyhow::Result<()> {
        Self::ensure_root()?;
        self.api
            .remove_peer(&Key::new(*public_key.as_bytes()))
            .with_context(|| format!("ピア {public_key} の削除に失敗しました"))
    }

    fn stats(&mut self) -> anyhow::Result<Vec<PeerStats>> {
        let host = self
            .api
            .read_interface_data()
            .with_context(|| format!("{} の情報取得に失敗しました", self.if_name))?;
        let mut stats: Vec<PeerStats> = host
            .peers
            .values()
            .map(|peer| PeerStats {
                public_key: peercove_core::keys::PublicKey::from_bytes(peer.public_key.as_array()),
                endpoint: peer.endpoint,
                // カーネルは「未成立」を UNIX エポックで返すため None に正規化する
                last_handshake: peer.last_handshake.filter(|t| *t != std::time::UNIX_EPOCH),
                tx_bytes: peer.tx_bytes,
                rx_bytes: peer.rx_bytes,
                allowed_ips: peer
                    .allowed_ips
                    .iter()
                    .filter_map(|mask| format!("{mask}").parse().ok())
                    .collect(),
            })
            .collect();
        stats.sort_by_key(|s| *s.public_key.as_bytes());
        Ok(stats)
    }

    fn add_route(&mut self, subnet: ipnet::Ipv4Net) -> anyhow::Result<()> {
        run(
            "ip",
            &[
                "route",
                "replace",
                &subnet.to_string(),
                "dev",
                &self.if_name,
            ],
        )
        .map(|_| ())
        .with_context(|| format!("経路 {subnet} の追加に失敗しました"))
    }

    fn remove_route(&mut self, subnet: ipnet::Ipv4Net) -> anyhow::Result<()> {
        match run(
            "ip",
            &["route", "del", &subnet.to_string(), "dev", &self.if_name],
        ) {
            Ok(_) => Ok(()),
            // 既に無い経路の削除は成功扱い(冪等)
            Err(e) if format!("{e:#}").contains("No such") => Ok(()),
            Err(e) => Err(e.context(format!("経路 {subnet} の削除に失敗しました"))),
        }
    }

    fn sync_acl(&mut self, policy: &AclPolicy) -> anyhow::Result<()> {
        if self.acl_policy.as_ref() == Some(policy) {
            return Ok(());
        }
        self.acl_generation = self.acl_generation.wrapping_add(1);
        let generation = self.acl_generation;
        let mut desired = Vec::new();
        if policy.default == AclAction::Deny
            || policy
                .rules
                .iter()
                .any(|rule| rule.action == AclAction::Deny)
        {
            desired.push(acl_reply_rule_args(
                "-I",
                &self.if_name,
                &format!("peercove-acl-v2-{generation}-established-reply"),
            ));
        }
        for rule in &policy.rules {
            let sources: Vec<String> = if rule.source.is_empty() {
                vec!["0.0.0.0/0".into()]
            } else {
                rule.source.iter().map(ToString::to_string).collect()
            };
            let destinations: Vec<String> = if rule.destination.is_empty() {
                vec!["0.0.0.0/0".into()]
            } else {
                rule.destination.iter().map(ToString::to_string).collect()
            };
            let ports: Vec<Option<(u16, u16)>> = if rule.ports.is_empty() {
                vec![None]
            } else {
                rule.ports.iter().copied().map(Some).collect()
            };
            for src in &sources {
                for dst in &destinations {
                    for &port in &ports {
                        desired.push(acl_rule_args(
                            "-I",
                            &self.if_name,
                            rule,
                            src,
                            dst,
                            port,
                            &format!("peercove-acl-v2-{generation}-{}", rule.id),
                        ));
                    }
                }
            }
        }
        if policy.default == AclAction::Deny {
            let rule = ResolvedRule {
                id: "default".into(),
                action: AclAction::Deny,
                source: vec![],
                destination: vec![],
                protocol: AclProtocol::Any,
                ports: vec![],
            };
            desired.push(acl_rule_args(
                "-I",
                &self.if_name,
                &rule,
                "0.0.0.0/0",
                "0.0.0.0/0",
                None,
                &format!("peercove-acl-v2-{generation}-default"),
            ));
        }
        let mut added: Vec<Vec<String>> = Vec::new();
        for args in desired.iter().rev() {
            if let Err(error) = iptables(args) {
                for mut rollback in added {
                    rollback[0] = "-D".into();
                    let _ = iptables(&rollback);
                }
                return Err(error).context("ACL v2 ルールを設定できません。iptables が必要です (sudo apt install iptables)");
            }
            added.push(args.clone());
        }
        // 新規一式が先に入った後で旧世代を外すため、再読込中に全許可の窓を作らない。
        for mut old in std::mem::take(&mut self.acl_rules) {
            old[0] = "-D".into();
            let _ = iptables(&old);
        }
        self.acl_rules = desired;
        self.acl_policy = Some(policy.clone());
        Ok(())
    }

    fn sync_isolation(&mut self, isolated: &[IsolatedPeer]) -> anyhow::Result<()> {
        let mut desired: Vec<_> = isolated.iter().map(|peer| peer.ip).collect();
        desired.sort_unstable();
        desired.dedup();
        if desired == self.isolated_ips {
            return Ok(());
        }
        self.clear_isolation();
        for ip in &desired {
            self.isolated_ips.push(*ip);
            for args in isolation_rule_args("-I", &self.if_name, *ip) {
                if let Err(error) = iptables(&args) {
                    self.clear_isolation();
                    return Err(
                        error.context("承認待ち端末の隔離に失敗しました。iptables が必要です")
                    );
                }
            }
        }
        Ok(())
    }

    fn sync_subnet_router(
        &mut self,
        virtual_subnet: ipnet::Ipv4Net,
        subnets: &[ipnet::Ipv4Net],
        snat: bool,
    ) -> anyhow::Result<()> {
        // 撤回されたサブネットの解除
        let desired: std::collections::HashSet<_> = subnets.iter().copied().collect();
        if let Some(virt) = self.router.virtual_subnet {
            let (keep, gone): (Vec<_>, Vec<_>) = std::mem::take(&mut self.router.subnets)
                .into_iter()
                .partition(|(subnet, _)| desired.contains(subnet));
            self.router.subnets = keep;
            for (subnet, applied_snat) in gone {
                Self::remove_subnet_rules(&virt, &subnet, applied_snat);
                tracing::info!("サブネット {subnet} の広告を解除しました");
            }
        }
        if subnets.is_empty() {
            self.clear_router();
            return Ok(());
        }
        self.router.virtual_subnet = Some(virtual_subnet);

        // 新しく広告されたサブネットの適用
        let wg_if = self.if_name.clone();
        for subnet in subnets {
            if self.router.subnets.iter().any(|(s, _)| s == subnet) {
                continue;
            }
            let lan_if = self.lan_interface_for(subnet)?;
            self.ensure_forwarding(&wg_if)?;
            self.ensure_forwarding(&lan_if)?;
            // FORWARD の明示許可(両方向)。Docker が入っている環境は FORWARD の
            // 既定ポリシーが DROP のため、これが無いと転送が黙って落ちる
            let (virt_str, subnet_str) = (virtual_subnet.to_string(), subnet.to_string());
            for (src, dst) in [(&virt_str, &subnet_str), (&subnet_str, &virt_str)] {
                if iptables(&forward_rule_args("-C", src, dst)).is_err() {
                    iptables(&forward_rule_args("-I", src, dst)).context(
                        "FORWARD 許可ルールの設定に失敗しました。iptables が必要です \
                         (sudo apt install iptables)",
                    )?;
                }
            }
            if snat {
                let args_check =
                    nat_rule_args("-C", &virtual_subnet.to_string(), &subnet.to_string());
                if iptables(&args_check).is_err() {
                    iptables(&nat_rule_args(
                        "-A",
                        &virtual_subnet.to_string(),
                        &subnet.to_string(),
                    ))
                    .context(
                        "SNAT(MASQUERADE)の設定に失敗しました。iptables が必要です \
                         (sudo apt install iptables)",
                    )?;
                }
            }
            self.router.subnets.push((*subnet, snat));
            tracing::info!("サブネットルーターを有効化しました({subnet} → {lan_if}、SNAT={snat})");
        }
        Ok(())
    }

    fn down(&mut self) -> anyhow::Result<()> {
        Self::ensure_root()?;
        self.clear_router();
        self.clear_isolation();
        self.clear_acl();
        match self.api.remove_interface() {
            Ok(()) => Ok(()),
            // 存在しない場合は成功扱い(クリーンアップの冪等性)
            Err(WireguardInterfaceError::NetlinkError(msg)) if msg.contains("No such") => {
                tracing::info!("インターフェース {} は存在しません", self.if_name);
                Ok(())
            }
            Err(e) => Err(anyhow::anyhow!(e).context(format!(
                "インターフェース {} の削除に失敗しました",
                self.if_name
            ))),
        }
    }
}
