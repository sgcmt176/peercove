//! 共有メモのサービス層(M5 F-2、ADR-0049)。
//!
//! - [`MemoService`](ホスト): `<config>.memos.db` の正本操作 + 揮発の編集ロック +
//!   接続中メンバーへのイベント配信。IPC(ホスト UI)とコントロールチャネル
//!   (メンバーの `MemoReq`)の両方がここを通るため、ロック・権限・リビジョンの
//!   判定は常に 1 か所で行われる。
//! - [`MemberMemoCache`](メンバー): `<config>.memocache.db` の読み取りキャッシュ。
//!   受信イベントを反映し、オフライン時は読み取り専用でここから表示する。
//!
//! イベントは Hello で `shared_memo` capability を名乗った接続にだけ送る
//! (旧クライアントの行長上限・未知メッセージを守る)。
//! **メモのタイトル・本文はログへ出さない**(ADR-0049。memo_id・IP は可)。

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use peercove_core::config::Config;
use peercove_core::memo::{
    MemoFolder, SharedMemoDetail, SharedMemoEvent, SharedMemoOp, SharedMemoQuery, SharedMemoReply,
    SharedMemoSummary,
};
use peercove_core::proto::ControlMessage;
use peercove_memo::shared::{Actor, CacheStore, SharedStore};

use crate::control::Connections;

/// 編集ロックの無操作タイムアウト(要件 §12)。更新・取得で延長される。
const LOCK_TTL: Duration = Duration::from_secs(120);

/// 供給側のどちらか(daemon の Active が保持する)。
#[derive(Clone)]
pub enum MemoHandle {
    Host(Arc<MemoService>),
    Member(Arc<MemberMemoCache>),
}

impl MemoHandle {
    pub fn new(role: crate::commands::tunnel::Role, config_path: &Path) -> Self {
        match role {
            crate::commands::tunnel::Role::Host => MemoHandle::Host(MemoService::new(config_path)),
            crate::commands::tunnel::Role::Member => {
                MemoHandle::Member(MemberMemoCache::new(config_path))
            }
        }
    }

    /// UI のポーリング用の変更世代。
    pub fn seq(&self) -> u64 {
        match self {
            MemoHandle::Host(service) => service.seq(),
            MemoHandle::Member(cache) => cache.generation(),
        }
    }

    /// 共有メモが使える状態か(ホスト = 常に可 / メンバー = 同期に成功済み)。
    pub fn supported(&self) -> bool {
        match self {
            MemoHandle::Host(_) => true,
            MemoHandle::Member(cache) => cache.supported(),
        }
    }
}

#[derive(Clone)]
struct LockInfo {
    /// ロック保持者(member_id。ホストは空文字)。
    key: String,
    /// 表示名(一覧の「〜が編集中」用)。
    name: String,
    last_activity: Instant,
}

/// ホスト側サービス。
pub struct MemoService {
    config_path: PathBuf,
    db_path: PathBuf,
    /// 接続中メンバー(supervisor が周期ごとに差し込む)。
    connections: Mutex<Option<Connections>>,
    locks: Mutex<HashMap<String, LockInfo>>,
    seq: AtomicU64,
}

impl MemoService {
    pub fn new(config_path: &Path) -> Arc<Self> {
        Arc::new(Self {
            config_path: config_path.to_path_buf(),
            db_path: config_path.with_extension("memos.db"),
            connections: Mutex::new(None),
            locks: Mutex::new(HashMap::new()),
            seq: AtomicU64::new(1),
        })
    }

    /// supervisor 起動時に接続表を差し込む(入れ直しをまたいで使い回す)。
    pub fn attach_connections(&self, connections: Connections) {
        *self.connections.lock().unwrap() = Some(connections);
    }

    pub fn seq(&self) -> u64 {
        self.seq.load(Ordering::Relaxed)
    }

    fn bump(&self) {
        self.seq.fetch_add(1, Ordering::Relaxed);
    }

    /// ホスト管理者としての操作主体。
    fn host_actor(config: &Config) -> Actor {
        Actor::host(
            config
                .interface
                .display_name
                .clone()
                .unwrap_or_else(|| "ホスト".to_string()),
        )
    }

    /// 仮想 IP → 操作主体(member_id + 表示名)。
    fn actor_for_ip(config: &Config, ip: Ipv4Addr) -> anyhow::Result<Actor> {
        let peer = config
            .peers
            .iter()
            .find(|peer| peer.allowed_ips.first().map(|net| net.addr()) == Some(ip))
            .context("送信元のメンバーが見つかりません")?;
        let name = peer.name.clone().unwrap_or_else(|| format!("member-{ip}"));
        // invite_id(= member_id、ADR-0047)。旧形式の登録は IP で代替する
        // (権限の個別指定には使えないが、全体権限での閲覧・編集はできる)
        let id = peer
            .invite_id
            .clone()
            .unwrap_or_else(|| format!("legacy-{ip}"));
        Ok(Actor::member(id, name))
    }

    /// メンバーからの依頼(コントロールチャネル)。エラーは Err 応答に変換する。
    pub async fn handle_for_member(
        self: &Arc<Self>,
        member_ip: Ipv4Addr,
        op: SharedMemoOp,
    ) -> SharedMemoReply {
        let path = self.config_path.clone();
        let actor = tokio::task::spawn_blocking(move || {
            let config = Config::load(&path)?;
            Self::actor_for_ip(&config, member_ip)
        })
        .await
        .unwrap_or_else(|e| Err(anyhow::anyhow!("内部エラー: {e}")));
        let actor = match actor {
            Ok(actor) => actor,
            Err(e) => {
                return SharedMemoReply::Err {
                    message: format!("{e:#}"),
                }
            }
        };
        match self.handle(actor, op).await {
            Ok(reply) => reply,
            Err(e) => SharedMemoReply::Err {
                message: format!("{e:#}"),
            },
        }
    }

    /// ホスト UI(IPC)からの操作。
    pub async fn handle_for_host(
        self: &Arc<Self>,
        op: SharedMemoOp,
    ) -> anyhow::Result<SharedMemoReply> {
        let path = self.config_path.clone();
        let actor = tokio::task::spawn_blocking(move || {
            let config = Config::load(&path)?;
            Ok::<_, anyhow::Error>(Self::host_actor(&config))
        })
        .await
        .context("設定読み込みタスクが失敗しました")??;
        self.handle(actor, op).await
    }

    async fn handle(
        self: &Arc<Self>,
        actor: Actor,
        op: SharedMemoOp,
    ) -> anyhow::Result<SharedMemoReply> {
        let key = actor.member_id.clone().unwrap_or_default();
        match op {
            SharedMemoOp::List { query } => {
                let (mut memos, folders) = self
                    .blocking({
                        let actor = actor.clone();
                        move |store| store.list(&actor, &query)
                    })
                    .await?;
                self.inject_lock_summaries(&mut memos);
                Ok(SharedMemoReply::Memos {
                    memos,
                    folders,
                    offline: false,
                })
            }
            SharedMemoOp::Get { id } => {
                let mut memo = self
                    .blocking({
                        let actor = actor.clone();
                        move |store| store.get(&actor, &id)
                    })
                    .await?;
                memo.locked_by = self.lock_holder(&memo.id);
                Ok(SharedMemoReply::Memo { memo })
            }
            SharedMemoOp::Create {
                title,
                body,
                folder_id,
            } => {
                let memo = self
                    .blocking({
                        let actor = actor.clone();
                        move |store| store.create(&actor, &title, &body, folder_id.as_deref())
                    })
                    .await?;
                self.broadcast_changed(memo.id.clone());
                self.bump();
                Ok(SharedMemoReply::Memo { memo })
            }
            SharedMemoOp::Update {
                id,
                base_revision,
                title,
                body,
            } => {
                // 単一編集者ロック(要件 §12): ロックを握っている人だけ保存できる
                {
                    let mut locks = self.locks.lock().unwrap();
                    prune_expired(&mut locks);
                    match locks.get_mut(&id) {
                        Some(lock) if lock.key == key => lock.last_activity = Instant::now(),
                        Some(lock) => bail!("「{}」が編集中です", lock.name),
                        None => bail!("編集を始める前に編集ロックを取得してください"),
                    }
                }
                let mut memo = self
                    .blocking({
                        let actor = actor.clone();
                        let id = id.clone();
                        move |store| store.update(&actor, &id, base_revision, &title, &body)
                    })
                    .await?;
                memo.locked_by = self.lock_holder(&memo.id);
                self.broadcast_changed(id);
                self.bump();
                Ok(SharedMemoReply::Memo { memo })
            }
            SharedMemoOp::AcquireLock { id } => {
                let mut memo = self
                    .blocking({
                        let actor = actor.clone();
                        let id = id.clone();
                        move |store| store.get(&actor, &id)
                    })
                    .await?;
                if !memo.can_edit {
                    bail!("このメモを編集する権限がありません(閲覧のみ)");
                }
                {
                    let mut locks = self.locks.lock().unwrap();
                    prune_expired(&mut locks);
                    match locks.get(&id) {
                        Some(lock) if lock.key != key => {
                            bail!("「{}」が編集中です(編集は 1 人ずつ)", lock.name)
                        }
                        _ => {}
                    }
                    locks.insert(
                        id.clone(),
                        LockInfo {
                            key,
                            name: actor.name.clone(),
                            last_activity: Instant::now(),
                        },
                    );
                }
                memo.locked_by = Some(actor.name.clone());
                self.broadcast_lock(&id, Some(actor.name.clone()));
                self.bump();
                Ok(SharedMemoReply::Memo { memo })
            }
            SharedMemoOp::ReleaseLock { id } => {
                let released = {
                    let mut locks = self.locks.lock().unwrap();
                    match locks.get(&id) {
                        Some(lock) if lock.key == key => {
                            locks.remove(&id);
                            true
                        }
                        _ => false,
                    }
                };
                if released {
                    self.broadcast_lock(&id, None);
                    self.bump();
                }
                Ok(SharedMemoReply::Done)
            }
            SharedMemoOp::ForceUnlock { id } => {
                if actor.member_id.is_some() {
                    bail!("編集ロックの強制解除はホスト管理者だけができます");
                }
                if self.locks.lock().unwrap().remove(&id).is_some() {
                    tracing::info!("共有メモの編集ロックを強制解除しました(memo={id})");
                    self.broadcast_lock(&id, None);
                    self.bump();
                }
                Ok(SharedMemoReply::Done)
            }
            SharedMemoOp::Trash { id } => {
                self.blocking({
                    let actor = actor.clone();
                    let id = id.clone();
                    move |store| store.trash(&actor, &id)
                })
                .await?;
                self.locks.lock().unwrap().remove(&id);
                self.broadcast_removed(&id);
                self.bump();
                Ok(SharedMemoReply::Done)
            }
            SharedMemoOp::Restore { id } => {
                self.blocking({
                    let actor = actor.clone();
                    let id = id.clone();
                    move |store| store.restore(&actor, &id)
                })
                .await?;
                self.broadcast_changed(id);
                self.bump();
                Ok(SharedMemoReply::Done)
            }
            SharedMemoOp::DeleteForever { id } => {
                self.blocking({
                    let actor = actor.clone();
                    let id = id.clone();
                    move |store| store.delete_forever(&actor, &id)
                })
                .await?;
                self.broadcast_removed(&id);
                self.bump();
                Ok(SharedMemoReply::Done)
            }
            SharedMemoOp::SetPerms {
                id,
                everyone,
                members,
            } => {
                let mut memo = self
                    .blocking({
                        let actor = actor.clone();
                        let id = id.clone();
                        move |store| store.set_perms(&actor, &id, everyone, &members)
                    })
                    .await?;
                memo.locked_by = self.lock_holder(&memo.id);
                // 権限を失ったメンバーには Removed が届く(broadcast_changed が
                // 受信者ごとに可視判定して振り分ける)
                self.broadcast_changed(id);
                self.bump();
                Ok(SharedMemoReply::Memo { memo })
            }
            SharedMemoOp::FolderCreate { name } => {
                self.blocking({
                    let actor = actor.clone();
                    move |store| store.folder_create(&actor, &name)
                })
                .await?;
                self.broadcast_folders();
                self.bump();
                Ok(SharedMemoReply::Done)
            }
            SharedMemoOp::FolderRename { id, name } => {
                self.blocking({
                    let actor = actor.clone();
                    move |store| store.folder_rename(&actor, &id, &name)
                })
                .await?;
                self.broadcast_folders();
                self.bump();
                Ok(SharedMemoReply::Done)
            }
            SharedMemoOp::FolderDelete { id } => {
                self.blocking({
                    let actor = actor.clone();
                    move |store| store.folder_delete(&actor, &id)
                })
                .await?;
                self.broadcast_folders();
                self.bump();
                Ok(SharedMemoReply::Done)
            }
        }
    }

    /// メンバー切断時に、その人が握っていた編集ロックを解放する(要件 §12)。
    pub fn release_locks_for_ip(self: &Arc<Self>, member_ip: Ipv4Addr) {
        let service = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let Ok(config) = Config::load(&service.config_path) else {
                return;
            };
            let Ok(actor) = Self::actor_for_ip(&config, member_ip) else {
                return;
            };
            let key = actor.member_id.unwrap_or_default();
            let released: Vec<String> = {
                let mut locks = service.locks.lock().unwrap();
                let ids: Vec<String> = locks
                    .iter()
                    .filter(|(_, lock)| lock.key == key)
                    .map(|(id, _)| id.clone())
                    .collect();
                for id in &ids {
                    locks.remove(id);
                }
                ids
            };
            for id in released {
                service.broadcast_lock(&id, None);
                service.bump();
            }
        });
    }

    /// 無操作タイムアウトのロックを解放する(supervisor の周期から呼ぶ)。
    pub fn sweep_expired_locks(self: &Arc<Self>) {
        let expired: Vec<String> = {
            let mut locks = self.locks.lock().unwrap();
            let ids: Vec<String> = locks
                .iter()
                .filter(|(_, lock)| lock.last_activity.elapsed() > LOCK_TTL)
                .map(|(id, _)| id.clone())
                .collect();
            for id in &ids {
                locks.remove(id);
            }
            ids
        };
        for id in expired {
            tracing::debug!("共有メモの編集ロックを無操作で解放しました(memo={id})");
            self.broadcast_lock(&id, None);
            self.bump();
        }
    }

    fn lock_holder(&self, id: &str) -> Option<String> {
        let mut locks = self.locks.lock().unwrap();
        prune_expired(&mut locks);
        locks.get(id).map(|lock| lock.name.clone())
    }

    fn inject_lock_summaries(&self, memos: &mut [SharedMemoSummary]) {
        let mut locks = self.locks.lock().unwrap();
        prune_expired(&mut locks);
        for memo in memos {
            memo.locked_by = locks.get(&memo.id).map(|lock| lock.name.clone());
        }
    }

    async fn blocking<T, F>(&self, f: F) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut SharedStore) -> anyhow::Result<T> + Send + 'static,
    {
        let path = self.db_path.clone();
        tokio::task::spawn_blocking(move || {
            let mut store = SharedStore::open(&path)?;
            f(&mut store)
        })
        .await
        .context("メモ処理タスクが失敗しました")?
    }

    /// `shared_memo` capability を名乗った接続の (IP, 送信口) 一覧。
    fn memo_connections(&self) -> Vec<(Ipv4Addr, crate::control::ConnectionInfo)> {
        let Some(connections) = self.connections.lock().unwrap().clone() else {
            return Vec::new();
        };
        let map = connections.lock().unwrap();
        map.iter()
            .filter(|(_, info)| info.capabilities.iter().any(|c| c == "shared_memo"))
            .map(|(ip, info)| (*ip, info.clone()))
            .collect()
    }

    /// 変更(作成・更新・復元・権限変更)を受信者ごとに配信する。
    /// 見える人には Changed、見えなくなった人には Removed。
    fn broadcast_changed(self: &Arc<Self>, id: String) {
        let service = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let targets = service.memo_connections();
            if targets.is_empty() {
                return;
            }
            let Ok(config) = Config::load(&service.config_path) else {
                return;
            };
            let Ok(store) = SharedStore::open(&service.db_path) else {
                return;
            };
            let holder = service.lock_holder(&id);
            for (ip, connection) in targets {
                let Ok(actor) = Self::actor_for_ip(&config, ip) else {
                    continue;
                };
                let event = match store.detail_if_visible(&actor, &id) {
                    Ok(Some(mut memo)) => {
                        memo.locked_by = holder.clone();
                        SharedMemoEvent::Changed { memo }
                    }
                    Ok(None) => SharedMemoEvent::Removed { id: id.clone() },
                    Err(_) => continue,
                };
                connection.send(ControlMessage::MemoEvent { event });
            }
        });
    }

    fn broadcast_removed(self: &Arc<Self>, id: &str) {
        let event = SharedMemoEvent::Removed { id: id.to_string() };
        for (_, connection) in self.memo_connections() {
            connection.send(ControlMessage::MemoEvent {
                event: event.clone(),
            });
        }
    }

    fn broadcast_lock(self: &Arc<Self>, id: &str, holder: Option<String>) {
        let event = SharedMemoEvent::Lock {
            id: id.to_string(),
            holder,
        };
        for (_, connection) in self.memo_connections() {
            connection.send(ControlMessage::MemoEvent {
                event: event.clone(),
            });
        }
    }

    fn broadcast_folders(self: &Arc<Self>) {
        let service = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let targets = service.memo_connections();
            if targets.is_empty() {
                return;
            }
            let Ok(store) = SharedStore::open(&service.db_path) else {
                return;
            };
            let Ok(folders) = store.folders() else {
                return;
            };
            let event = SharedMemoEvent::Folders { folders };
            for (_, connection) in targets {
                connection.send(ControlMessage::MemoEvent {
                    event: event.clone(),
                });
            }
        });
    }
}

fn prune_expired(locks: &mut HashMap<String, LockInfo>) {
    locks.retain(|_, lock| lock.last_activity.elapsed() <= LOCK_TTL);
}

/// メンバー側の読み取りキャッシュ + 同期状態。
pub struct MemberMemoCache {
    path: PathBuf,
    generation: AtomicU64,
    /// ホストが共有メモに応答した(= 対応バージョン)。
    supported: AtomicBool,
}

impl MemberMemoCache {
    pub fn new(config_path: &Path) -> Arc<Self> {
        Arc::new(Self {
            path: config_path.with_extension("memocache.db"),
            generation: AtomicU64::new(1),
            supported: AtomicBool::new(false),
        })
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    pub fn supported(&self) -> bool {
        self.supported.load(Ordering::Relaxed)
    }

    pub fn set_supported(&self, value: bool) {
        self.supported.store(value, Ordering::Relaxed);
    }

    fn bump(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    async fn blocking<T, F>(&self, f: F) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut CacheStore) -> anyhow::Result<T> + Send + 'static,
    {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut store = CacheStore::open(&path)?;
            f(&mut store)
        })
        .await
        .context("メモキャッシュタスクが失敗しました")?
    }

    /// ホストからのイベントをキャッシュへ反映する。
    pub async fn apply_event(self: &Arc<Self>, event: SharedMemoEvent) {
        let result = match event {
            SharedMemoEvent::Changed { memo } => {
                self.blocking(move |store| store.upsert(&memo)).await
            }
            SharedMemoEvent::Removed { id } => self.blocking(move |store| store.remove(&id)).await,
            SharedMemoEvent::Lock { id, holder } => {
                self.blocking(move |store| store.set_lock(&id, holder.as_deref()))
                    .await
            }
            SharedMemoEvent::Folders { folders } => {
                self.blocking(move |store| store.replace_folders(&folders))
                    .await
            }
        };
        match result {
            Ok(()) => self.bump(),
            Err(e) => tracing::warn!("共有メモのキャッシュ更新に失敗しました: {e:#}"),
        }
    }

    /// 一覧(オフライン時はここが唯一のソース)。
    pub async fn list(
        self: &Arc<Self>,
        query: SharedMemoQuery,
    ) -> anyhow::Result<(Vec<SharedMemoSummary>, Vec<MemoFolder>)> {
        self.blocking(move |store| store.list(&query)).await
    }

    pub async fn get(self: &Arc<Self>, id: String) -> anyhow::Result<SharedMemoDetail> {
        self.blocking(move |store| store.get(&id)).await
    }

    /// 接続時の同期: List 応答を突き合わせ、取り直しが必要な ID を返す。
    pub async fn sync_from_list(
        self: &Arc<Self>,
        memos: Vec<SharedMemoSummary>,
        folders: Vec<MemoFolder>,
    ) -> anyhow::Result<Vec<String>> {
        let stale = self
            .blocking(move |store| {
                let ids: Vec<String> = memos.iter().map(|memo| memo.id.clone()).collect();
                store.retain(&ids)?;
                let mut stale = Vec::new();
                for memo in &memos {
                    match store.revision(&memo.id)? {
                        Some(revision) if revision == memo.revision => {
                            // 内容は最新。ロック表示だけ合わせる
                            store.set_lock(&memo.id, memo.locked_by.as_deref())?;
                        }
                        _ => stale.push(memo.id.clone()),
                    }
                }
                store.replace_folders(&folders)?;
                Ok(stale)
            })
            .await?;
        self.bump();
        Ok(stale)
    }

    pub async fn upsert(self: &Arc<Self>, memo: SharedMemoDetail) {
        if let Err(e) = self.blocking(move |store| store.upsert(&memo)).await {
            tracing::warn!("共有メモのキャッシュ書き込みに失敗しました: {e:#}");
        }
        self.bump();
    }
}
