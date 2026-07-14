//! メンバー間直接通信の直接ピア管理(ADR-0013、M3-3)。
//!
//! ホストが台帳で配布した他メンバーの外部エンドポイントへ、実行中の WG
//! トンネルにピアを**ランタイム追加**して NAT に穴を開ける(WG 標準の
//! ハンドシェイク再送 + keepalive がパンチ動作を兼ねる)。双方が同じ台帳から
//! 同じ結論に達するため、明示的な調停メッセージは使わない。
//!
//! - 直接ピアは設定ファイルに書かない(台帳から毎回導出できるエフェメラルな
//!   最適化)。追加/削除はこのモジュールだけが行い、ホストピアには触れない
//! - **二段階 AllowedIPs(ADR-0019)**: 試行中は AllowedIPs 空のプローブとして
//!   追加し(経路を奪わない = 中継が生き続ける)、ハンドシェイクを観測して
//!   から `/32` を付与して直接経路へ切り替える
//! - 失敗・陳腐化したらピアを削除するだけで、cryptokey routing の最長一致に
//!   より自動的にホスト経由(/24)へ戻る。OS ルートは一切変更しない

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant, SystemTime};

use ipnet::Ipv4Net;
use peercove_core::ipc::DirectStatus;
use peercove_core::keys::PublicKey;

use crate::backend::{PeerSpec, PeerStats, WgBackend};
use crate::control::ReceivedDistribution;

/// 台帳のエンドポイント観測がこれより古いものは試行しない
/// (ADR-0013 追加条件 1。「配布時の観測経過 + 受信からの経過」で判定)。
const MAX_ENDPOINT_AGE: Duration = Duration::from_secs(300);
/// 追加からこれ以内にハンドシェイクが完了しなければ試行を打ち切る。
/// 試行は AllowedIPs 空のプローブなので通信への影響はない(ADR-0019)。
const TRYING_TIMEOUT: Duration = Duration::from_secs(45);
/// 最終ハンドシェイクがこれを超えたら直接経路は死んだとみなす
/// (WG のセッション有効期限 180 秒。tunnel.rs の ONLINE_THRESHOLD と同値)。
const HANDSHAKE_STALE: Duration = Duration::from_secs(180);
/// 同じエンドポイントへの再試行間隔(固定、ADR-0019。指数バックオフは廃止)。
/// パンチング(および相手側ステートフルファイアウォールのピンホール開け)には
/// **両側の試行窓が重なる**ことが必要。試行窓 45 秒 × 周期 60 秒なら、両側の
/// 位相がどうずれても重なりが 2×45−60 = 30 秒保証される。台帳のエンドポイントが
/// 変わったら待たずに再試行する。
const RETRY_INTERVAL: Duration = Duration::from_secs(60);
/// 直接ピアの keepalive 秒(NAT マッピング維持 + パンチの継続)。
const DIRECT_KEEPALIVE: u16 = 25;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    /// プローブ追加済みでハンドシェイク待ち(AllowedIPs は空 = 経路は中継のまま)。
    /// `quiet` は同じエンドポイントへの再試行(初回失敗後)で、ログは debug、
    /// UI の経路表示にも出さない(ADR-0019)。
    Trying { since: Instant, quiet: bool },
    /// ハンドシェイク確認済みで `/32` を付与済み(直接通信中)。
    Direct,
}

struct DirectState {
    ip: Ipv4Addr,
    endpoint: SocketAddr,
    phase: Phase,
}

/// 失敗の記録。同じエンドポイントへの再試行を [`RETRY_INTERVAL`] だけ抑える。
struct Cooldown {
    endpoint: SocketAddr,
    at: Instant,
}

/// 次の周期でやること(状態の参照と変更を分けるための中間表現)。
enum Act {
    Add,
    Rebind,
    Establish,
    Fail(&'static str),
    Keep,
}

/// メンバー側の直接ピア管理。supervise の周期(5 秒)ごとに [`Self::tick`] を呼ぶ。
pub struct DirectManager {
    /// 自分の公開鍵(台帳の自分自身のエントリを除外する)。
    self_key: [u8; 32],
    /// トンネルのサブネット。台帳が壊れていても外の経路を奪わないためのガード。
    subnet: Ipv4Net,
    states: HashMap<[u8; 32], DirectState>,
    /// 失敗した相手ごとのバックオフ状態。
    cooldown: HashMap<[u8; 32], Cooldown>,
}

impl DirectManager {
    pub fn new(self_key: [u8; 32], subnet: Ipv4Net) -> Self {
        Self {
            self_key,
            subnet,
            states: HashMap::new(),
            cooldown: HashMap::new(),
        }
    }

    /// 台帳と WG 統計から直接ピアを追加・確認・除去する。
    ///
    /// `enabled` は設定の `direct` フラグ(false なら全直接ピアを解除して中継のみ)。
    pub fn tick(
        &mut self,
        now: Instant,
        enabled: bool,
        received: Option<&ReceivedDistribution>,
        stats: &[PeerStats],
        backend: &mut dyn WgBackend,
    ) {
        // 台帳から消えた相手などの古い失敗記録を掃除する(メモリ衛生)
        self.cooldown
            .retain(|_, cd| now.duration_since(cd.at) < RETRY_INTERVAL.saturating_mul(2));

        let desired = if enabled {
            self.desired(now, received)
        } else {
            HashMap::new()
        };
        let handshake_fresh: HashMap<[u8; 32], bool> = stats
            .iter()
            .map(|s| {
                let fresh = s
                    .last_handshake
                    .and_then(|t| SystemTime::now().duration_since(t).ok())
                    .is_some_and(|age| age <= HANDSHAKE_STALE);
                (*s.public_key.as_bytes(), fresh)
            })
            .collect();

        // 望まれなくなった相手(オフライン・台帳から消えた・direct=false)を除去。
        // 相手がいなくなっただけなのでクールダウンは付けない
        let gone: Vec<[u8; 32]> = self
            .states
            .keys()
            .filter(|key| !desired.contains_key(*key))
            .copied()
            .collect();
        for key in gone {
            let quiet = matches!(
                self.states.get(&key).map(|s| s.phase),
                Some(Phase::Trying { quiet: true, .. })
            );
            self.drop_peer(
                &key,
                backend,
                "台帳から外れたため直接ピアを解除します",
                quiet,
            );
        }

        for (key, (ip, endpoint)) in desired {
            let act = match self.states.get(&key) {
                Some(state) if state.endpoint != endpoint => Act::Rebind,
                Some(state) => {
                    let fresh = handshake_fresh.get(&key).copied().unwrap_or(false);
                    match state.phase {
                        Phase::Trying { .. } if fresh => Act::Establish,
                        Phase::Trying { since, .. }
                            if now.duration_since(since) > TRYING_TIMEOUT =>
                        {
                            Act::Fail(
                                "直接接続がタイムアウトしました(中継のまま、裏で再試行を続けます)",
                            )
                        }
                        Phase::Trying { .. } => Act::Keep,
                        Phase::Direct if fresh => Act::Keep,
                        Phase::Direct => Act::Fail("直接経路が途絶えました(中継へ戻します)"),
                    }
                }
                None => match self.cooldown.get(&key) {
                    // 同じエンドポイントへの再試行は固定間隔(ADR-0019)。
                    // エンドポイントが変わったら即再試行(ADR-0013)
                    Some(cd)
                        if cd.endpoint == endpoint
                            && now.duration_since(cd.at) < RETRY_INTERVAL =>
                    {
                        Act::Keep
                    }
                    _ => Act::Add,
                },
            };
            match act {
                Act::Add => {
                    // 同じエンドポイントへの再試行はログ・UI 表示を静かにする
                    let quiet = self
                        .cooldown
                        .remove(&key)
                        .is_some_and(|cd| cd.endpoint == endpoint);
                    self.try_add(key, ip, endpoint, now, quiet, backend);
                }
                Act::Rebind => {
                    self.cooldown.remove(&key);
                    self.drop_peer(
                        &key,
                        backend,
                        "エンドポイントが変わったため直接ピアを張り直します",
                        false,
                    );
                    self.try_add(key, ip, endpoint, now, false, backend);
                }
                Act::Establish => {
                    // ハンドシェイク確認 → `/32` を付与して経路を直接側へ
                    // 切り替える(二段階 AllowedIPs、ADR-0019)。endpoint は
                    // 渡さない(roaming 学習済みの実エンドポイントを維持)
                    let spec = PeerSpec {
                        public_key: PublicKey::from_bytes(key),
                        endpoint: None,
                        allowed_ips: vec![Ipv4Net::new(ip, 32).expect("/32 は常に有効")],
                        persistent_keepalive: Some(DIRECT_KEEPALIVE),
                        preshared_key: None,
                    };
                    match backend.add_peer(&spec) {
                        Ok(()) => {
                            if let Some(state) = self.states.get_mut(&key) {
                                state.phase = Phase::Direct;
                                tracing::info!("直接通信を確立しました({ip} = {endpoint})");
                            }
                            self.cooldown.remove(&key);
                        }
                        // 次の周期で再判定される(ハンドシェイクが新鮮なうちは
                        // 再び Establish に来る)
                        Err(e) => {
                            tracing::warn!("直接経路への切り替えに失敗しました({ip}): {e:#}")
                        }
                    }
                }
                Act::Fail(why) => {
                    let quiet = matches!(
                        self.states.get(&key).map(|s| s.phase),
                        Some(Phase::Trying { quiet: true, .. })
                    );
                    self.cooldown.insert(key, Cooldown { endpoint, at: now });
                    self.drop_peer(&key, backend, why, quiet);
                }
                Act::Keep => {}
            }
        }
    }

    /// 現在の直接経路(相手の仮想 IP → 状態)。status / UI 表示用(M3-4)。
    /// 載っていない相手はホスト経由(中継)。静かな再試行(初回失敗後の
    /// プローブ)は経路を奪っていないので「中継」として見せる(ADR-0019)。
    pub fn routes(&self) -> HashMap<Ipv4Addr, DirectStatus> {
        self.states
            .values()
            .filter_map(|state| {
                let status = match state.phase {
                    Phase::Trying { quiet: true, .. } => return None,
                    Phase::Trying { .. } => DirectStatus::Trying,
                    Phase::Direct => DirectStatus::Direct,
                };
                Some((state.ip, status))
            })
            .collect()
    }

    /// 台帳から「直接接続を試すべき相手」を導出する(自分・ホスト・オフライン・
    /// エンドポイントなし・観測が古い・サブネット外は除外)。
    fn desired(
        &self,
        now: Instant,
        received: Option<&ReceivedDistribution>,
    ) -> HashMap<[u8; 32], (Ipv4Addr, SocketAddr)> {
        let Some(received) = received else {
            return HashMap::new();
        };
        let since_receipt = now.saturating_duration_since(received.received_at);
        received
            .distribution
            .members
            .iter()
            .filter_map(|entry| {
                // blocked は ACL の遮断相手(ADR-0018)。ホストは endpoint も
                // 落として配るが、こちらでも張らない(二重の守り)
                if entry.is_host || !entry.online || entry.blocked {
                    return None;
                }
                let key = *entry.public_key.as_bytes();
                if key == self.self_key {
                    return None;
                }
                let endpoint = entry.endpoint?;
                let age = Duration::from_secs(entry.endpoint_age_secs.unwrap_or(0)) + since_receipt;
                if age > MAX_ENDPOINT_AGE {
                    return None;
                }
                if !self.subnet.contains(&entry.ip) {
                    return None; // 台帳が壊れていてもトンネル外の経路は奪わない
                }
                Some((key, (entry.ip, endpoint)))
            })
            .collect()
    }

    fn try_add(
        &mut self,
        key: [u8; 32],
        ip: Ipv4Addr,
        endpoint: SocketAddr,
        now: Instant,
        quiet: bool,
        backend: &mut dyn WgBackend,
    ) {
        // プローブ: AllowedIPs は空で追加する(経路を奪わない = 試行中も
        // 中継で通信が続く)。ハンドシェイクと keepalive だけが走り、
        // 確立を観測してから `/32` を付与する(二段階、ADR-0019)
        let spec = PeerSpec {
            public_key: PublicKey::from_bytes(key),
            endpoint: Some(endpoint),
            allowed_ips: vec![],
            persistent_keepalive: Some(DIRECT_KEEPALIVE),
            // 直接ピアに PSK は付けない(ADR-0013。WG の Noise で機密性は担保)
            preshared_key: None,
        };
        match backend.add_peer(&spec) {
            Ok(()) => {
                if quiet {
                    tracing::debug!("直接接続を再試行します({ip} → {endpoint})");
                } else {
                    tracing::info!("直接接続を試行します({ip} → {endpoint})");
                }
                self.states.insert(
                    key,
                    DirectState {
                        ip,
                        endpoint,
                        phase: Phase::Trying { since: now, quiet },
                    },
                );
            }
            Err(e) => tracing::warn!("直接ピアの追加に失敗しました({ip}): {e:#}"),
        }
    }

    /// 直接ピアを WG から外し、状態を忘れる。削除に失敗しても状態は消す
    /// (次の周期の add が失敗として観測される。残骸で固まるより良い)。
    /// `quiet` は静かな再試行の後片付け(ログは debug に落とす)。
    fn drop_peer(&mut self, key: &[u8; 32], backend: &mut dyn WgBackend, why: &str, quiet: bool) {
        let public_key = PublicKey::from_bytes(*key);
        if let Some(state) = self.states.remove(key) {
            match backend.remove_peer(&public_key) {
                Ok(()) if quiet => tracing::debug!("{why}({})", state.ip),
                Ok(()) => tracing::info!("{why}({})", state.ip),
                Err(e) => {
                    tracing::warn!("直接ピア {public_key} の削除に失敗しました: {e:#}")
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::mock::MockBackend;
    use crate::control::Distribution;
    use peercove_core::keys::PrivateKey;
    use peercove_core::proto::LedgerEntry;

    const SUBNET: &str = "10.100.42.0/24";

    fn manager(self_key: &PublicKey) -> DirectManager {
        DirectManager::new(*self_key.as_bytes(), SUBNET.parse().unwrap())
    }

    fn entry(key: &PublicKey, ip: &str, endpoint: Option<&str>, age: u64) -> LedgerEntry {
        LedgerEntry {
            name: None,
            dns_name: None,
            ip: ip.parse().unwrap(),
            public_key: *key,
            app_version: None,
            capabilities: vec![],
            invite_status: None,
            invite_expires_at: None,
            online: true,
            is_host: false,
            endpoint: endpoint.map(|e| e.parse().unwrap()),
            endpoint_age_secs: endpoint.map(|_| age),
            subnets: vec![],
            blocked: false,
        }
    }

    fn received(members: Vec<LedgerEntry>, at: Instant) -> ReceivedDistribution {
        ReceivedDistribution {
            distribution: Distribution {
                members,
                dns_records: vec![],
                cname_records: vec![],
                deny: vec![],
            },
            received_at: at,
        }
    }

    fn fresh_stats(key: &PublicKey) -> Vec<PeerStats> {
        vec![PeerStats {
            public_key: *key,
            endpoint: None,
            last_handshake: Some(SystemTime::now()),
            tx_bytes: 0,
            rx_bytes: 0,
            allowed_ips: vec![],
        }]
    }

    fn stale_stats(key: &PublicKey) -> Vec<PeerStats> {
        vec![PeerStats {
            public_key: *key,
            endpoint: None,
            last_handshake: Some(SystemTime::now() - HANDSHAKE_STALE - Duration::from_secs(30)),
            tx_bytes: 0,
            rx_bytes: 0,
            allowed_ips: vec![],
        }]
    }

    /// オンラインでエンドポイント付きの他メンバーには直接ピアを張り、
    /// ホスト・自分・オフライン・エンドポイントなしは対象外。
    #[test]
    fn adds_direct_peers_only_for_eligible_members() {
        let me = PrivateKey::generate().public_key();
        let peer = PrivateKey::generate().public_key();
        let offline = PrivateKey::generate().public_key();
        let host = PrivateKey::generate().public_key();
        let mut m = manager(&me);
        let mut backend = MockBackend::default();
        let now = Instant::now();

        let mut host_entry = entry(&host, "10.100.42.1", Some("198.51.100.1:1"), 0);
        host_entry.is_host = true;
        let mut offline_entry = entry(&offline, "10.100.42.4", Some("198.51.100.4:4"), 0);
        offline_entry.online = false;
        let dist = received(
            vec![
                host_entry,
                entry(&me, "10.100.42.2", Some("198.51.100.2:2"), 0),
                entry(&peer, "10.100.42.3", Some("198.51.100.3:3"), 0),
                offline_entry,
                entry(&PrivateKey::generate().public_key(), "10.100.42.5", None, 0),
            ],
            now,
        );
        m.tick(now, true, Some(&dist), &[], &mut backend);
        assert_eq!(backend.ops, vec![format!("add:{peer}")]);
    }

    /// direct = false なら追加しない。true → false で既存の直接ピアも解除する。
    #[test]
    fn disabled_flag_prevents_and_removes_direct_peers() {
        let me = PrivateKey::generate().public_key();
        let peer = PrivateKey::generate().public_key();
        let mut m = manager(&me);
        let mut backend = MockBackend::default();
        let now = Instant::now();
        let dist = received(
            vec![entry(&peer, "10.100.42.3", Some("198.51.100.3:3"), 0)],
            now,
        );

        m.tick(now, false, Some(&dist), &[], &mut backend);
        assert!(backend.ops.is_empty(), "無効なら試行しない");

        m.tick(now, true, Some(&dist), &[], &mut backend);
        assert_eq!(backend.ops, vec![format!("add:{peer}")]);
        m.tick(now, false, Some(&dist), &[], &mut backend);
        assert_eq!(
            backend.ops,
            vec![format!("add:{peer}"), format!("remove:{peer}")],
            "無効化で解除して中継へ戻る"
        );
    }

    /// ACL で遮断された相手(blocked、ADR-0018)には直接ピアを張らず、
    /// 既存の直接ピアも解除する。
    #[test]
    fn blocked_entries_are_not_tried_and_get_removed() {
        let me = PrivateKey::generate().public_key();
        let peer = PrivateKey::generate().public_key();
        let mut m = manager(&me);
        let mut backend = MockBackend::default();
        let now = Instant::now();

        let mut blocked = entry(&peer, "10.100.42.3", Some("198.51.100.3:3"), 0);
        blocked.blocked = true;
        let dist = received(vec![blocked.clone()], now);
        m.tick(now, true, Some(&dist), &[], &mut backend);
        assert!(backend.ops.is_empty(), "blocked には張らない");

        // 確立済みの相手が blocked になったら解除される
        let open = received(
            vec![entry(&peer, "10.100.42.3", Some("198.51.100.3:3"), 0)],
            now,
        );
        m.tick(now, true, Some(&open), &[], &mut backend);
        assert_eq!(backend.ops, vec![format!("add:{peer}")]);
        let dist = received(vec![blocked], now + Duration::from_secs(5));
        m.tick(
            now + Duration::from_secs(5),
            true,
            Some(&dist),
            &[],
            &mut backend,
        );
        assert_eq!(
            backend.ops,
            vec![format!("add:{peer}"), format!("remove:{peer}")],
            "遮断されたら直接ピアを解除して中継(→ホストで遮断)へ戻す"
        );
    }

    /// 観測が古いエンドポイントは試行しない(配布時の経過 + 受信からの経過)。
    #[test]
    fn stale_endpoints_are_not_tried() {
        let me = PrivateKey::generate().public_key();
        let peer = PrivateKey::generate().public_key();
        let mut m = manager(&me);
        let mut backend = MockBackend::default();
        let now = Instant::now();

        // 配布時点で既に古い
        let dist = received(
            vec![entry(
                &peer,
                "10.100.42.3",
                Some("198.51.100.3:3"),
                MAX_ENDPOINT_AGE.as_secs() + 60,
            )],
            now,
        );
        m.tick(now, true, Some(&dist), &[], &mut backend);
        assert!(backend.ops.is_empty(), "配布時点で古い観測は使わない");

        // 配布時は新鮮でも、受信からの経過で古くなる
        let dist = received(
            vec![entry(&peer, "10.100.42.3", Some("198.51.100.3:3"), 0)],
            now,
        );
        let later = now + MAX_ENDPOINT_AGE + Duration::from_secs(60);
        m.tick(later, true, Some(&dist), &[], &mut backend);
        assert!(backend.ops.is_empty(), "受信後に古くなった観測は使わない");
    }

    /// サブネット外の仮想 IP は台帳が壊れていても /32 を張らない。
    #[test]
    fn out_of_subnet_entries_are_ignored() {
        let me = PrivateKey::generate().public_key();
        let peer = PrivateKey::generate().public_key();
        let mut m = manager(&me);
        let mut backend = MockBackend::default();
        let now = Instant::now();
        let dist = received(
            vec![entry(&peer, "8.8.8.8", Some("198.51.100.3:3"), 0)],
            now,
        );
        m.tick(now, true, Some(&dist), &[], &mut backend);
        assert!(backend.ops.is_empty());
    }

    /// タイムアウトで除去 → 同じエンドポイントへは再試行間隔内は控える →
    /// エンドポイントが変われば即再試行 → 間隔が明ければ再試行(固定間隔、
    /// ADR-0019)。
    #[test]
    fn trying_timeout_retries_on_fixed_interval() {
        let me = PrivateKey::generate().public_key();
        let peer = PrivateKey::generate().public_key();
        let mut m = manager(&me);
        let mut backend = MockBackend::default();
        let t0 = Instant::now();
        let dist = received(
            vec![entry(&peer, "10.100.42.3", Some("198.51.100.3:3"), 0)],
            t0,
        );

        m.tick(t0, true, Some(&dist), &[], &mut backend);
        // ハンドシェイクが来ないままタイムアウト
        let t1 = t0 + TRYING_TIMEOUT + Duration::from_secs(1);
        m.tick(t1, true, Some(&dist), &[], &mut backend);
        assert_eq!(
            backend.ops,
            vec![format!("add:{peer}"), format!("remove:{peer}")]
        );

        // 再試行間隔内は同じエンドポイントを再試行しない
        backend.ops.clear();
        m.tick(
            t1 + Duration::from_secs(5),
            true,
            Some(&dist),
            &[],
            &mut backend,
        );
        assert!(backend.ops.is_empty(), "再試行間隔内");

        // エンドポイントが変わったら即再試行
        let rebound = received(
            vec![entry(&peer, "10.100.42.3", Some("198.51.100.9:9"), 0)],
            t1,
        );
        m.tick(
            t1 + Duration::from_secs(10),
            true,
            Some(&rebound),
            &[],
            &mut backend,
        );
        assert_eq!(backend.ops, vec![format!("add:{peer}")]);

        // 間隔が明ければ同じエンドポイントでも再試行する
        let mut m = manager(&me);
        let mut backend = MockBackend::default();
        m.tick(t0, true, Some(&dist), &[], &mut backend);
        m.tick(t1, true, Some(&dist), &[], &mut backend); // タイムアウト
        backend.ops.clear();
        let after = t1 + RETRY_INTERVAL + Duration::from_secs(1);
        // 受信も新しくないと鮮度ガードに引っかかるため台帳を再受信した体にする
        let refreshed = received(
            vec![entry(&peer, "10.100.42.3", Some("198.51.100.3:3"), 0)],
            after,
        );
        m.tick(after, true, Some(&refreshed), &[], &mut backend);
        assert_eq!(backend.ops, vec![format!("add:{peer}")]);
    }

    /// 何度失敗しても再試行間隔は一定のまま伸びない(指数バックオフの廃止、
    /// ADR-0019。両側の試行窓の重なりを保証するため)。
    #[test]
    fn retry_interval_stays_fixed_after_repeated_failures() {
        let me = PrivateKey::generate().public_key();
        let peer = PrivateKey::generate().public_key();
        let mut m = manager(&me);
        let mut backend = MockBackend::default();
        let dist_at = |at: Instant| {
            received(
                vec![entry(&peer, "10.100.42.3", Some("198.51.100.3:3"), 0)],
                at,
            )
        };

        // 3 回連続で失敗させる
        let mut t = Instant::now();
        for round in 1..=3 {
            m.tick(t, true, Some(&dist_at(t)), &[], &mut backend);
            assert_eq!(
                backend.ops.last(),
                Some(&format!("add:{peer}")),
                "{round} 回目も待ちが伸びずに再試行される"
            );
            t += TRYING_TIMEOUT + Duration::from_secs(1);
            m.tick(t, true, Some(&dist_at(t)), &[], &mut backend); // タイムアウト
            t += RETRY_INTERVAL + Duration::from_secs(1);
        }
    }

    /// ハンドシェイクが観測できたら `/32` を付与して確立(二段階 AllowedIPs、
    /// ADR-0019。タイムアウトを過ぎても除去されない)。その後途絶えたら
    /// 除去して中継へ戻る。
    #[test]
    fn establishes_then_falls_back_when_handshake_goes_stale() {
        let me = PrivateKey::generate().public_key();
        let peer = PrivateKey::generate().public_key();
        let mut m = manager(&me);
        let mut backend = MockBackend::default();
        let t0 = Instant::now();
        let dist = received(
            vec![entry(&peer, "10.100.42.3", Some("198.51.100.3:3"), 0)],
            t0,
        );

        m.tick(t0, true, Some(&dist), &[], &mut backend);
        // ハンドシェイク成功 → /32 を付与(2 回目の add = upsert)して確立。
        // タイムアウトを過ぎても除去されない
        let t1 = t0 + Duration::from_secs(10);
        m.tick(t1, true, Some(&dist), &fresh_stats(&peer), &mut backend);
        let t2 = t0 + TRYING_TIMEOUT + Duration::from_secs(10);
        m.tick(t2, true, Some(&dist), &fresh_stats(&peer), &mut backend);
        assert_eq!(
            backend.ops,
            vec![format!("add:{peer}"), format!("add:{peer}")],
            "プローブ追加 → 確立時の /32 付与、以後は維持"
        );

        // ハンドシェイクが陳腐化 → 除去(中継へ)
        // 鮮度ガードを避けるため台帳は再受信した体にする
        let refreshed = received(
            vec![entry(&peer, "10.100.42.3", Some("198.51.100.3:3"), 0)],
            t2,
        );
        m.tick(
            t2 + Duration::from_secs(5),
            true,
            Some(&refreshed),
            &stale_stats(&peer),
            &mut backend,
        );
        assert_eq!(
            backend.ops,
            vec![
                format!("add:{peer}"),
                format!("add:{peer}"),
                format!("remove:{peer}")
            ]
        );
    }

    /// routes(): 確立中は Trying、確立後は Direct、解除後は消える。
    /// 初回失敗後の静かな再試行は経路表示に出ない(中継扱い、ADR-0019)。
    #[test]
    fn routes_reflect_phase() {
        let me = PrivateKey::generate().public_key();
        let peer = PrivateKey::generate().public_key();
        let ip: Ipv4Addr = "10.100.42.3".parse().unwrap();
        let mut m = manager(&me);
        let mut backend = MockBackend::default();
        let t0 = Instant::now();
        let dist = received(
            vec![entry(&peer, "10.100.42.3", Some("198.51.100.3:3"), 0)],
            t0,
        );

        assert!(m.routes().is_empty());
        m.tick(t0, true, Some(&dist), &[], &mut backend);
        assert_eq!(m.routes().get(&ip), Some(&DirectStatus::Trying));
        m.tick(
            t0 + Duration::from_secs(5),
            true,
            Some(&dist),
            &fresh_stats(&peer),
            &mut backend,
        );
        assert_eq!(m.routes().get(&ip), Some(&DirectStatus::Direct));
        m.tick(t0 + Duration::from_secs(10), true, None, &[], &mut backend);
        assert!(m.routes().is_empty(), "台帳が無ければ解除される");

        // 失敗 → 静かな再試行は「中継」として見せる(Trying を出さない)
        let mut m = manager(&me);
        let mut backend = MockBackend::default();
        m.tick(t0, true, Some(&dist), &[], &mut backend);
        let t1 = t0 + TRYING_TIMEOUT + Duration::from_secs(1);
        m.tick(t1, true, Some(&dist), &[], &mut backend); // タイムアウト
        let t2 = t1 + RETRY_INTERVAL + Duration::from_secs(1);
        let refreshed = received(
            vec![entry(&peer, "10.100.42.3", Some("198.51.100.3:3"), 0)],
            t2,
        );
        m.tick(t2, true, Some(&refreshed), &[], &mut backend); // 静かな再試行
        assert_eq!(
            backend.ops.last(),
            Some(&format!("add:{peer}")),
            "裏では再試行している"
        );
        assert!(m.routes().is_empty(), "静かな再試行は経路表示に出ない");

        // 静かな再試行中でも確立すれば Direct になる
        m.tick(
            t2 + Duration::from_secs(5),
            true,
            Some(&refreshed),
            &fresh_stats(&peer),
            &mut backend,
        );
        assert_eq!(m.routes().get(&ip), Some(&DirectStatus::Direct));
    }

    /// 相手がオフラインになったら(クールダウンなしで)解除する。
    #[test]
    fn removes_peer_that_goes_offline_and_readds_when_back() {
        let me = PrivateKey::generate().public_key();
        let peer = PrivateKey::generate().public_key();
        let mut m = manager(&me);
        let mut backend = MockBackend::default();
        let t0 = Instant::now();
        let dist = received(
            vec![entry(&peer, "10.100.42.3", Some("198.51.100.3:3"), 0)],
            t0,
        );
        m.tick(t0, true, Some(&dist), &[], &mut backend);

        let mut off = entry(&peer, "10.100.42.3", Some("198.51.100.3:3"), 0);
        off.online = false;
        let offline = received(vec![off], t0);
        m.tick(
            t0 + Duration::from_secs(5),
            true,
            Some(&offline),
            &[],
            &mut backend,
        );
        assert_eq!(
            backend.ops,
            vec![format!("add:{peer}"), format!("remove:{peer}")]
        );

        // 戻ってきたら(クールダウンなしで)すぐ張り直す
        backend.ops.clear();
        let back = received(
            vec![entry(&peer, "10.100.42.3", Some("198.51.100.3:3"), 0)],
            t0 + Duration::from_secs(10),
        );
        m.tick(
            t0 + Duration::from_secs(10),
            true,
            Some(&back),
            &[],
            &mut backend,
        );
        assert_eq!(backend.ops, vec![format!("add:{peer}")]);
    }
}
