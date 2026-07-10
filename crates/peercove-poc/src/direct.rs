//! メンバー間直接通信の直接ピア管理(ADR-0013、M3-3)。
//!
//! ホストが台帳で配布した他メンバーの外部エンドポイントへ、実行中の WG
//! トンネルにピアを**ランタイム追加**して NAT に穴を開ける(WG 標準の
//! ハンドシェイク再送 + keepalive がパンチ動作を兼ねる)。双方が同じ台帳から
//! 同じ結論に達するため、明示的な調停メッセージは使わない。
//!
//! - 直接ピアは設定ファイルに書かない(台帳から毎回導出できるエフェメラルな
//!   最適化)。追加/削除はこのモジュールだけが行い、ホストピアには触れない
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
/// 追加からこれ以内にハンドシェイクが完了しなければ失敗として除去する
/// (→ 中継のまま)。WG のハンドシェイク再送は 5 秒間隔なので約 6 回分。
const TRYING_TIMEOUT: Duration = Duration::from_secs(30);
/// 最終ハンドシェイクがこれを超えたら直接経路は死んだとみなす
/// (WG のセッション有効期限 180 秒。tunnel.rs の ONLINE_THRESHOLD と同値)。
const HANDSHAKE_STALE: Duration = Duration::from_secs(180);
/// 失敗した相手への再試行までの基本待ち時間。失敗を重ねるごとに 2 倍
/// (上限 [`RETRY_MAX`]、M3-4)。台帳のエンドポイントが変わったら待たずに
/// 再試行し、バックオフもリセットする。
const RETRY_COOLDOWN: Duration = Duration::from_secs(300);
/// 再試行間隔の上限(指数バックオフの頭打ち)。
const RETRY_MAX: Duration = Duration::from_secs(3600);

/// `failures` 回連続で失敗した後の待ち時間(5 分 → 10 分 → … → 上限 1 時間)。
fn backoff(failures: u32) -> Duration {
    RETRY_COOLDOWN
        .saturating_mul(1u32 << failures.saturating_sub(1).min(4))
        .min(RETRY_MAX)
}
/// 直接ピアの keepalive 秒(NAT マッピング維持 + パンチの継続)。
const DIRECT_KEEPALIVE: u16 = 25;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    /// 追加済みでハンドシェイク待ち。
    Trying { since: Instant },
    /// ハンドシェイク確認済み(直接通信中)。
    Direct,
}

struct DirectState {
    ip: Ipv4Addr,
    endpoint: SocketAddr,
    phase: Phase,
}

/// 失敗の記録。同じエンドポイントへの再試行を [`backoff`] だけ抑える。
struct Cooldown {
    endpoint: SocketAddr,
    at: Instant,
    /// 連続失敗回数(バックオフの指数)。成功またはエンドポイント変化でリセット。
    failures: u32,
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
        // 長く放置されたバックオフ記録を掃除する(メモリ衛生。上限バックオフの
        // 2 倍以上経っていれば、忘れて 1 からやり直して問題ない)
        self.cooldown
            .retain(|_, cd| now.duration_since(cd.at) < RETRY_MAX.saturating_mul(2));

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
            self.drop_peer(&key, backend, "台帳から外れたため直接ピアを解除します");
        }

        for (key, (ip, endpoint)) in desired {
            let act = match self.states.get(&key) {
                Some(state) if state.endpoint != endpoint => Act::Rebind,
                Some(state) => {
                    let fresh = handshake_fresh.get(&key).copied().unwrap_or(false);
                    match state.phase {
                        Phase::Trying { .. } if fresh => Act::Establish,
                        Phase::Trying { since } if now.duration_since(since) > TRYING_TIMEOUT => {
                            Act::Fail(
                                "直接接続がタイムアウトしました(中継で継続、後で再試行します)",
                            )
                        }
                        Phase::Trying { .. } => Act::Keep,
                        Phase::Direct if fresh => Act::Keep,
                        Phase::Direct => Act::Fail("直接経路が途絶えました(中継へ戻します)"),
                    }
                }
                None => match self.cooldown.get(&key) {
                    // 同じエンドポイントへの再試行はバックオフ中は控える。
                    // エンドポイントが変わったら即再試行(ADR-0013)
                    Some(cd)
                        if cd.endpoint == endpoint
                            && now.duration_since(cd.at) < backoff(cd.failures) =>
                    {
                        Act::Keep
                    }
                    _ => Act::Add,
                },
            };
            match act {
                Act::Add => {
                    // エンドポイントが変わっていたらバックオフをリセット
                    // (同じままなら失敗回数を持ち越して待ちを伸ばす)
                    if self
                        .cooldown
                        .get(&key)
                        .is_some_and(|cd| cd.endpoint != endpoint)
                    {
                        self.cooldown.remove(&key);
                    }
                    self.try_add(key, ip, endpoint, now, backend);
                }
                Act::Rebind => {
                    self.cooldown.remove(&key);
                    self.drop_peer(
                        &key,
                        backend,
                        "エンドポイントが変わったため直接ピアを張り直します",
                    );
                    self.try_add(key, ip, endpoint, now, backend);
                }
                Act::Establish => {
                    if let Some(state) = self.states.get_mut(&key) {
                        state.phase = Phase::Direct;
                        tracing::info!("直接通信を確立しました({ip} = {endpoint})");
                    }
                    self.cooldown.remove(&key); // 成功したらバックオフをリセット
                }
                Act::Fail(why) => {
                    let failures = match self.cooldown.get(&key) {
                        Some(cd) if cd.endpoint == endpoint => cd.failures + 1,
                        _ => 1,
                    };
                    self.cooldown.insert(
                        key,
                        Cooldown {
                            endpoint,
                            at: now,
                            failures,
                        },
                    );
                    self.drop_peer(&key, backend, why);
                }
                Act::Keep => {}
            }
        }
    }

    /// 現在の直接経路(相手の仮想 IP → 状態)。status / UI 表示用(M3-4)。
    /// 載っていない相手はホスト経由(中継)。
    pub fn routes(&self) -> HashMap<Ipv4Addr, DirectStatus> {
        self.states
            .values()
            .map(|state| {
                let status = match state.phase {
                    Phase::Trying { .. } => DirectStatus::Trying,
                    Phase::Direct => DirectStatus::Direct,
                };
                (state.ip, status)
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
                if entry.is_host || !entry.online {
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
        backend: &mut dyn WgBackend,
    ) {
        let spec = PeerSpec {
            public_key: PublicKey::from_bytes(key),
            endpoint: Some(endpoint),
            allowed_ips: vec![Ipv4Net::new(ip, 32).expect("/32 は常に有効")],
            persistent_keepalive: Some(DIRECT_KEEPALIVE),
            // 直接ピアに PSK は付けない(ADR-0013。WG の Noise で機密性は担保)
            preshared_key: None,
        };
        match backend.add_peer(&spec) {
            Ok(()) => {
                tracing::info!("直接接続を試行します({ip} → {endpoint})");
                self.states.insert(
                    key,
                    DirectState {
                        ip,
                        endpoint,
                        phase: Phase::Trying { since: now },
                    },
                );
            }
            Err(e) => tracing::warn!("直接ピアの追加に失敗しました({ip}): {e:#}"),
        }
    }

    /// 直接ピアを WG から外し、状態を忘れる。削除に失敗しても状態は消す
    /// (次の周期の add が失敗として観測される。残骸で固まるより良い)。
    fn drop_peer(&mut self, key: &[u8; 32], backend: &mut dyn WgBackend, why: &str) {
        let public_key = PublicKey::from_bytes(*key);
        if let Some(state) = self.states.remove(key) {
            match backend.remove_peer(&public_key) {
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
            ip: ip.parse().unwrap(),
            public_key: *key,
            online: true,
            is_host: false,
            endpoint: endpoint.map(|e| e.parse().unwrap()),
            endpoint_age_secs: endpoint.map(|_| age),
            subnets: vec![],
        }
    }

    fn received(members: Vec<LedgerEntry>, at: Instant) -> ReceivedDistribution {
        ReceivedDistribution {
            distribution: Distribution {
                members,
                dns_records: vec![],
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

    /// タイムアウトで除去 → 同じエンドポイントへはクールダウン中再試行しない →
    /// エンドポイントが変われば即再試行。
    #[test]
    fn trying_timeout_backs_off_until_endpoint_changes() {
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

        // クールダウン中は同じエンドポイントを再試行しない
        backend.ops.clear();
        m.tick(
            t1 + Duration::from_secs(5),
            true,
            Some(&dist),
            &[],
            &mut backend,
        );
        assert!(backend.ops.is_empty(), "クールダウン中");

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

        // クールダウンが明ければ同じエンドポイントでも再試行する
        let mut m = manager(&me);
        let mut backend = MockBackend::default();
        m.tick(t0, true, Some(&dist), &[], &mut backend);
        m.tick(t1, true, Some(&dist), &[], &mut backend); // タイムアウト → cooldown
        backend.ops.clear();
        let after = t1 + RETRY_COOLDOWN + Duration::from_secs(1);
        // 受信も新しくないと鮮度ガードに引っかかるため台帳を再受信した体にする
        let refreshed = received(
            vec![entry(&peer, "10.100.42.3", Some("198.51.100.3:3"), 0)],
            after,
        );
        m.tick(after, true, Some(&refreshed), &[], &mut backend);
        assert_eq!(backend.ops, vec![format!("add:{peer}")]);
    }

    /// ハンドシェイクが観測できたら確立(除去しない)。その後途絶えたら
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
        // ハンドシェイク成功 → 確立(タイムアウトを過ぎても除去されない)
        let t1 = t0 + Duration::from_secs(10);
        m.tick(t1, true, Some(&dist), &fresh_stats(&peer), &mut backend);
        let t2 = t0 + TRYING_TIMEOUT + Duration::from_secs(10);
        m.tick(t2, true, Some(&dist), &fresh_stats(&peer), &mut backend);
        assert_eq!(backend.ops, vec![format!("add:{peer}")], "確立後は維持");

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
            vec![format!("add:{peer}"), format!("remove:{peer}")]
        );
    }

    /// 失敗を重ねるとバックオフが 2 倍ずつ伸びる(5 分 → 10 分 → … → 上限 1 時間)。
    /// 確立に成功するとリセットされる。
    #[test]
    fn backoff_doubles_after_repeated_failures_and_resets_on_success() {
        assert_eq!(backoff(1), Duration::from_secs(300));
        assert_eq!(backoff(2), Duration::from_secs(600));
        assert_eq!(backoff(3), Duration::from_secs(1200));
        assert_eq!(backoff(10), RETRY_MAX, "上限で頭打ち");

        let me = PrivateKey::generate().public_key();
        let peer = PrivateKey::generate().public_key();
        let mut m = manager(&me);
        let mut backend = MockBackend::default();
        let t0 = Instant::now();
        let dist_at = |at: Instant| {
            received(
                vec![entry(&peer, "10.100.42.3", Some("198.51.100.3:3"), 0)],
                at,
            )
        };

        // 1 回目の失敗
        m.tick(t0, true, Some(&dist_at(t0)), &[], &mut backend);
        let t1 = t0 + TRYING_TIMEOUT + Duration::from_secs(1);
        m.tick(t1, true, Some(&dist_at(t1)), &[], &mut backend);
        // 5 分後に再試行 → 2 回目の失敗
        let t2 = t1 + backoff(1) + Duration::from_secs(1);
        m.tick(t2, true, Some(&dist_at(t2)), &[], &mut backend);
        let t3 = t2 + TRYING_TIMEOUT + Duration::from_secs(1);
        m.tick(t3, true, Some(&dist_at(t3)), &[], &mut backend);
        backend.ops.clear();

        // 2 回目の失敗後は 5 分では再試行せず、10 分待つ
        let after_5min = t3 + backoff(1) + Duration::from_secs(1);
        m.tick(
            after_5min,
            true,
            Some(&dist_at(after_5min)),
            &[],
            &mut backend,
        );
        assert!(backend.ops.is_empty(), "バックオフが 10 分に伸びている");
        let after_10min = t3 + backoff(2) + Duration::from_secs(1);
        m.tick(
            after_10min,
            true,
            Some(&dist_at(after_10min)),
            &[],
            &mut backend,
        );
        assert_eq!(backend.ops, vec![format!("add:{peer}")]);

        // 今回は確立に成功 → バックオフはリセットされ、次の失敗はまた 5 分から
        m.tick(
            after_10min + Duration::from_secs(5),
            true,
            Some(&dist_at(after_10min)),
            &fresh_stats(&peer),
            &mut backend,
        );
        assert_eq!(
            m.routes().get(&"10.100.42.3".parse().unwrap()),
            Some(&DirectStatus::Direct)
        );
        backend.ops.clear();
        let t4 = after_10min + Duration::from_secs(10);
        // 経路が途絶える(失敗 1 回目扱い)
        let refreshed = dist_at(t4);
        m.tick(
            t4,
            true,
            Some(&refreshed),
            &stale_stats(&peer),
            &mut backend,
        );
        assert_eq!(backend.ops, vec![format!("remove:{peer}")]);
        let t5 = t4 + backoff(1) + Duration::from_secs(1);
        m.tick(t5, true, Some(&dist_at(t5)), &[], &mut backend);
        assert_eq!(
            backend.ops,
            vec![format!("remove:{peer}"), format!("add:{peer}")],
            "リセット後は 5 分で再試行する"
        );
    }

    /// routes(): 確立中は Trying、確立後は Direct、解除後は消える。
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
