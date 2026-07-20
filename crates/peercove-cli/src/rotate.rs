//! メンバーのデバイス鍵ローテーション(ADR-0020、M3-11)。
//!
//! 招待トークン経由の鍵(ホストが生成し、チャット等の経路を通った)を、
//! メンバーが端末上で生成した鍵へ差し替える。秘密鍵は端末から出さず、
//! 公開鍵だけをコントロールチャネルでホストへ届ける。
//!
//! # 状態モデル(鍵を失って締め出されないための設計)
//!
//! - `member.key` … 確定済みの鍵(設定 `private_key_file` が指すファイル)
//! - `member.key.new` … 更新待ちの新鍵。**依頼を送る前に必ず書く**。
//!   ホストへの反映が確認できるまで消さない
//!
//! 依頼の応答が失われても(切断・旧ホスト・クラッシュ)、ホストが新旧
//! どちらの鍵を持っていても自力で収束する:
//! - 応答 `accepted` → 確定(`member.key` を新鍵で上書き)して入れ直し
//! - ホストからの受信が [`DEAD_LINK_TIMEOUT`] 止まった + 更新待ちの鍵がある
//!   → 新鍵に切り替えて試す(疎通したら確定、しなければ元に戻して交互に試行)
//! - 現行鍵で繋がったまま応答が来ない(旧ホスト等)→ 何もしない
//!   (次のセッションで同じ新鍵の依頼を再送。冪等)

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use peercove_core::config::{Config, KeySource};
use peercove_core::keys::PrivateKey;
use peercove_core::proto::ControlMessage;
// 鍵ファイル操作はモバイル(peercove-mobile)と共用のため peercove-ops へ
// 移設した(ADR-0044)。ここは再エクスポートして既存の呼び出し元を保つ
pub use peercove_ops::keyfiles::{load_pending, pending_path};

use crate::backend::PeerStats;
use crate::control::MemberLink;

/// ホストからの受信が止まったとみなすまでの時間。健全ならメンバー→ホストの
/// keepalive(25 秒)への受動 keepalive で 35 秒以内に必ず何か届く。
/// 最終ハンドシェイク経過は通常運転でも 2 分まで伸びるため判定に使わない。
const DEAD_LINK_TIMEOUT: Duration = Duration::from_secs(45);

/// supervisor へ返す要求: トンネルを入れ直す(鍵の切り替え)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestartWith {
    /// true なら更新待ちの鍵(`member.key.new`)で、false なら設定の鍵で。
    pub use_pending: bool,
}

/// supervisor が毎周期(5 秒)呼ぶローテーションの状態機械。
pub struct Rotation {
    config_path: PathBuf,
    key_path: PathBuf,
    /// UI / IPC からの手動要求(ADR-0020 の手動トリガー)。
    manual: Arc<AtomicBool>,
    /// このトンネルが「更新待ちの鍵」で動いているか(フォールバック試行中)。
    on_pending: bool,
    /// このセッション世代で依頼を送信済みか(再送はセッションごとに 1 回)。
    attempted_session: Option<u64>,
    /// 初回依頼のログを INFO、以降の再送を debug にするためのフラグ。
    announced: bool,
    /// ホストからの受信が最後に動いていた時刻と、その時点の rx_bytes。
    last_rx: (Instant, u64),
}

impl Rotation {
    /// `current_key` は起動したトンネルの秘密鍵(更新待ちの鍵で入れ直した
    /// 直後かどうかをここで判定する)。
    pub fn new(
        config_path: PathBuf,
        key_path: PathBuf,
        current_key: &PrivateKey,
        manual: Arc<AtomicBool>,
        now: Instant,
    ) -> Self {
        let on_pending =
            load_pending(&key_path).is_some_and(|k| k.public_key() == current_key.public_key());
        Self {
            config_path,
            key_path,
            manual,
            on_pending,
            attempted_session: None,
            announced: false,
            last_rx: (now, 0),
        }
    }

    /// 毎周期の判定。トンネルの入れ直しが必要なら Some を返す。
    pub fn tick(
        &mut self,
        now: Instant,
        config: &Config,
        stats: &[PeerStats],
        link: &MemberLink,
    ) -> Option<RestartWith> {
        let host_key = config.peers.first()?.public_key;
        let rx = stats
            .iter()
            .find(|s| s.public_key == host_key)
            .map(|s| s.rx_bytes)
            .unwrap_or(0);
        if rx != self.last_rx.1 {
            self.last_rx = (now, rx);
        }
        let link_dead = now.duration_since(self.last_rx.0) >= DEAD_LINK_TIMEOUT;

        // 1. ホストからの応答を回収
        if let Some((accepted, message)) = link.take_rotate_result() {
            if accepted {
                match self.commit() {
                    Ok(()) => {
                        tracing::info!("デバイス鍵を更新しました(新しい鍵で接続し直します)");
                        self.manual.store(false, Ordering::Relaxed);
                        return Some(RestartWith { use_pending: false });
                    }
                    Err(e) => tracing::warn!(
                        "更新した鍵の保存に失敗しました(次の接続で再試行します): {e:#}"
                    ),
                }
            } else {
                tracing::warn!("デバイス鍵の更新がホストに拒否されました: {message}");
                let _ = std::fs::remove_file(pending_path(&self.key_path));
                self.manual.store(false, Ordering::Relaxed);
            }
            return None;
        }

        // 2. フォールバック試行中(更新待ちの鍵で動いている)
        if self.on_pending {
            if rx > 0 {
                // この鍵でホストから受信できた = ホストは新鍵を登録済み → 確定
                match self.commit() {
                    Ok(()) => {
                        tracing::info!("デバイス鍵の更新が完了しました");
                        self.on_pending = false;
                    }
                    Err(e) => tracing::warn!("更新した鍵の保存に失敗しました: {e:#}"),
                }
            } else if link_dead {
                tracing::info!("更新待ちの鍵では疎通しないため、元の鍵に戻します");
                return Some(RestartWith { use_pending: false });
            }
            return None;
        }

        // 3. 確定済みの鍵で疎通しない + 更新待ちの鍵がある → 切り替えて試す
        //    (ホストが先に新鍵へ切り替えた後にこちらが落ちたケースの自己回復)
        if link_dead && load_pending(&self.key_path).is_some() {
            tracing::info!("ホストと疎通できないため、更新待ちの鍵に切り替えて試します");
            return Some(RestartWith { use_pending: true });
        }

        // 4. ローテーションの開始(自動 = 鍵の出どころがトークン / 手動要求)
        let auto = config.interface.key_source != Some(KeySource::SelfGenerated);
        if !(auto || self.manual.load(Ordering::Relaxed)) {
            return None;
        }
        let session = link.session()?;
        if self.attempted_session == Some(session) {
            return None;
        }
        let new_key = match self.ensure_pending() {
            Ok(key) => key,
            Err(e) => {
                tracing::warn!("新しい鍵の生成・保存に失敗しました: {e:#}");
                return None;
            }
        };
        let public = new_key.public_key();
        if link.send(ControlMessage::RotateKey {
            new_public_key: public,
        }) {
            self.attempted_session = Some(session);
            if self.announced {
                tracing::debug!("デバイス鍵の更新をホストへ再依頼しました({public})");
            } else {
                tracing::info!("デバイス鍵の更新をホストへ依頼しました(新しい公開鍵 {public})");
                self.announced = true;
            }
        }
        None
    }

    /// 更新待ちの新鍵を用意する(あれば再利用 — 依頼の再送を冪等にする)。
    fn ensure_pending(&self) -> anyhow::Result<PrivateKey> {
        peercove_ops::keyfiles::ensure_pending(&self.key_path)
    }

    /// 更新を確定する(peercove-ops::keyfiles、モバイルと共用)。
    fn commit(&mut self) -> anyhow::Result<()> {
        peercove_ops::keyfiles::commit_pending(&self.config_path, &self.key_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::MemberLink;
    use peercove_core::keys::{read_private_key_file, write_secret_file};

    struct Env {
        config_path: PathBuf,
        key_path: PathBuf,
        host_key: peercove_core::keys::PublicKey,
        current: PrivateKey,
    }

    fn setup(name: &str, key_source: &str) -> Env {
        let dir = std::env::temp_dir().join(format!("peercove-rotate-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let key_path = dir.join("member.key");
        let current = PrivateKey::generate();
        write_secret_file(&key_path, &format!("{}\n", current.to_base64())).unwrap();
        let host = PrivateKey::generate().public_key();
        let config_path = dir.join("member.toml");
        std::fs::write(
            &config_path,
            format!(
                "# メンバー設定のコメント\n[interface]\nprivate_key_file = \"member.key\"\naddress = \"10.100.42.2/24\"\n{key_source}\n[[peer]]\npublic_key = \"{host}\"\nendpoint = \"203.0.113.5:51820\"\nallowed_ips = [\"10.100.42.0/24\"]\n"
            ),
        )
        .unwrap();
        Env {
            config_path,
            key_path,
            host_key: host,
            current,
        }
    }

    fn config_of(env: &Env) -> Config {
        Config::load(&env.config_path).unwrap()
    }

    fn host_stats(env: &Env, rx_bytes: u64) -> Vec<PeerStats> {
        vec![PeerStats {
            public_key: env.host_key,
            endpoint: Some("203.0.113.5:51820".parse().unwrap()),
            last_handshake: Some(std::time::SystemTime::now()),
            tx_bytes: 0,
            rx_bytes,
            allowed_ips: vec![],
        }]
    }

    fn rotation(env: &Env, manual: &Arc<AtomicBool>, now: Instant) -> Rotation {
        Rotation::new(
            env.config_path.clone(),
            env.key_path.clone(),
            &env.current,
            Arc::clone(manual),
            now,
        )
    }

    /// 接続済みの MemberLink(送信キューの受け口も返す)。
    fn connected_link() -> (
        Arc<MemberLink>,
        tokio::sync::mpsc::UnboundedReceiver<ControlMessage>,
    ) {
        let link = Arc::new(MemberLink::default());
        let rx = link.attach_for_test();
        (link, rx)
    }

    /// 自動ローテーション: 接続すると依頼が 1 回だけ送られ、`.new` が
    /// 依頼の**前に**書かれている。同一セッションでは再送しない。
    #[test]
    fn auto_rotation_sends_request_once_per_session() {
        let env = setup("auto", "");
        let manual = Arc::new(AtomicBool::new(false));
        let now = Instant::now();
        let mut rotation = rotation(&env, &manual, now);
        let (link, mut out) = connected_link();

        assert_eq!(
            rotation.tick(now, &config_of(&env), &host_stats(&env, 10), &link),
            None
        );
        let pending = load_pending(&env.key_path).expect("依頼前に .new が書かれる");
        match out.try_recv().unwrap() {
            ControlMessage::RotateKey { new_public_key } => {
                assert_eq!(new_public_key, pending.public_key());
            }
            other => panic!("RotateKey を期待: {other:?}"),
        }

        // 同一セッションでは送らない
        rotation.tick(now, &config_of(&env), &host_stats(&env, 10), &link);
        assert!(out.try_recv().is_err());

        // 再接続(セッション世代が進む)で同じ鍵を再送(冪等)
        let rx2 = link.attach_for_test();
        rotation.tick(now, &config_of(&env), &host_stats(&env, 10), &link);
        let mut rx2 = rx2;
        match rx2.try_recv().unwrap() {
            ControlMessage::RotateKey { new_public_key } => {
                assert_eq!(new_public_key, pending.public_key(), "再送は同じ鍵");
            }
            other => panic!("RotateKey を期待: {other:?}"),
        }
    }

    /// key_source = "self"(ローテーション済み)なら自動では何も送らない。
    /// 手動要求(UI の「鍵を更新」)があれば送る。
    #[test]
    fn rotated_config_only_rotates_on_manual_request() {
        let env = setup("manual", "key_source = \"self\"\n");
        let manual = Arc::new(AtomicBool::new(false));
        let now = Instant::now();
        let mut rotation = rotation(&env, &manual, now);
        let (link, mut out) = connected_link();

        rotation.tick(now, &config_of(&env), &host_stats(&env, 10), &link);
        assert!(out.try_recv().is_err(), "自動では送らない");
        assert!(load_pending(&env.key_path).is_none());

        manual.store(true, Ordering::Relaxed);
        rotation.tick(now, &config_of(&env), &host_stats(&env, 10), &link);
        assert!(matches!(
            out.try_recv().unwrap(),
            ControlMessage::RotateKey { .. }
        ));
    }

    /// accepted 応答 → member.key が新鍵になり、.new は消え、key_source は
    /// "self" になり(コメント保持)、設定の鍵での入れ直しを要求する。
    #[test]
    fn accepted_result_commits_and_requests_restart() {
        let env = setup("commit", "");
        let manual = Arc::new(AtomicBool::new(false));
        let now = Instant::now();
        let mut rotation = rotation(&env, &manual, now);
        let (link, _out) = connected_link();

        rotation.tick(now, &config_of(&env), &host_stats(&env, 10), &link);
        let pending = load_pending(&env.key_path).unwrap();

        link.put_rotate_result_for_test(true, "更新を受け付けました".to_string());
        let action = rotation.tick(now, &config_of(&env), &host_stats(&env, 10), &link);
        assert_eq!(action, Some(RestartWith { use_pending: false }));

        let committed = read_private_key_file(&env.key_path).unwrap();
        assert_eq!(committed.public_key(), pending.public_key());
        assert!(load_pending(&env.key_path).is_none(), ".new は消える");
        let text = std::fs::read_to_string(&env.config_path).unwrap();
        assert!(text.starts_with("# メンバー設定のコメント"), "コメント保持");
        assert_eq!(
            config_of(&env).interface.key_source,
            Some(KeySource::SelfGenerated)
        );
    }

    /// 拒否応答 → .new を破棄して現行鍵のまま(次のセッションで新しい鍵を
    /// 生成してやり直す)。
    #[test]
    fn rejected_result_discards_pending() {
        let env = setup("reject", "");
        let manual = Arc::new(AtomicBool::new(false));
        let now = Instant::now();
        let mut rotation = rotation(&env, &manual, now);
        let (link, _out) = connected_link();

        rotation.tick(now, &config_of(&env), &host_stats(&env, 10), &link);
        assert!(load_pending(&env.key_path).is_some());
        link.put_rotate_result_for_test(false, "別のメンバーが使用中".to_string());
        let action = rotation.tick(now, &config_of(&env), &host_stats(&env, 10), &link);
        assert_eq!(action, None);
        assert!(load_pending(&env.key_path).is_none());
    }

    /// 応答が失われた後の自己回復: ホストからの受信が 45 秒止まったら
    /// 更新待ちの鍵への切り替えを要求し、それでも疎通しなければ元へ戻す。
    #[test]
    fn dead_link_alternates_between_keys() {
        let env = setup("fallback", "");
        let manual = Arc::new(AtomicBool::new(false));
        let start = Instant::now();
        let mut rotation = rotation(&env, &manual, start);
        let (link, _out) = connected_link();

        // 依頼送信(応答は来ない想定)
        rotation.tick(start, &config_of(&env), &host_stats(&env, 10), &link);
        assert!(load_pending(&env.key_path).is_some());

        // 受信が動いている間は切り替えない
        let t1 = start + Duration::from_secs(40);
        assert_eq!(
            rotation.tick(t1, &config_of(&env), &host_stats(&env, 20), &link),
            None
        );

        // そこから 45 秒受信が止まる → 更新待ちの鍵で入れ直し
        let t2 = t1 + DEAD_LINK_TIMEOUT;
        assert_eq!(
            rotation.tick(t2, &config_of(&env), &host_stats(&env, 20), &link),
            Some(RestartWith { use_pending: true })
        );
    }

    /// 更新待ちの鍵で起動したトンネル: 受信できたら確定(入れ直し不要)、
    /// できなければ元の鍵へ戻す。
    #[test]
    fn pending_key_promotes_on_traffic_or_falls_back() {
        let env = setup("promote", "");
        let manual = Arc::new(AtomicBool::new(false));
        let start = Instant::now();

        // .new を用意し、「その鍵で起動した」状態を作る
        let pending = PrivateKey::generate();
        write_secret_file(
            &pending_path(&env.key_path),
            &format!("{}\n", pending.to_base64()),
        )
        .unwrap();
        let mut rotation = Rotation::new(
            env.config_path.clone(),
            env.key_path.clone(),
            &pending,
            Arc::clone(&manual),
            start,
        );
        let (link, _out) = connected_link();

        // 受信ゼロのまま 45 秒 → 元の鍵へ戻す
        let t1 = start + DEAD_LINK_TIMEOUT;
        assert_eq!(
            rotation.tick(t1, &config_of(&env), &host_stats(&env, 0), &link),
            Some(RestartWith { use_pending: false })
        );

        // 受信があれば確定: member.key が新鍵になる
        let mut rotation = Rotation::new(
            env.config_path.clone(),
            env.key_path.clone(),
            &pending,
            Arc::clone(&manual),
            start,
        );
        assert_eq!(
            rotation.tick(start, &config_of(&env), &host_stats(&env, 100), &link),
            None
        );
        let committed = read_private_key_file(&env.key_path).unwrap();
        assert_eq!(committed.public_key(), pending.public_key());
        assert!(load_pending(&env.key_path).is_none());
        assert_eq!(
            config_of(&env).interface.key_source,
            Some(KeySource::SelfGenerated)
        );
    }
}
