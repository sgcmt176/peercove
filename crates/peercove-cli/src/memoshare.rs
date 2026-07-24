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
use peercove_core::schedule::{ScheduleEvent, ScheduleEventMsg, ScheduleOp, ScheduleReply};
use peercove_core::sheet::{SheetCell, SheetEventMsg, SheetMeta, SheetOp, SheetReply};
use peercove_memo::shared::{Actor, CacheStore, SharedStore};

use crate::control::Connections;

/// 編集ロックの無操作タイムアウト(要件 §12)。更新・取得で延長される。
const LOCK_TTL: Duration = Duration::from_secs(120);

/// 定期メンテナンス(ゴミ箱の完全削除 + WAL チェックポイント)の間隔(M5 F-3)。
const MAINTENANCE_INTERVAL: Duration = Duration::from_secs(10 * 60);

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
    /// ロック確立時点のメモ revision(M5 F-3)。解放時にここから変わって
    /// いれば履歴へ "close" 版を残す(編集終了時に 1 版 = 要件)。
    revision_at_acquire: u64,
}

/// ホスト側サービス。
pub struct MemoService {
    config_path: PathBuf,
    db_path: PathBuf,
    /// 接続中メンバー(supervisor が周期ごとに差し込む)。
    connections: Mutex<Option<Connections>>,
    /// 既知グループ全量(ADR-0051)。IP → 所属グループの解決と、グループ
    /// 権限を持つメモの再配信(`watch_groups`)に使う。
    groups: Mutex<Option<crate::groups::SharedGroups>>,
    /// 直近に見た「全グループの (id, revision)」の署名(`watch_groups`)。
    /// 変化を検知したときだけ再配信を行う。
    last_group_sig: Mutex<Option<Vec<(String, u64)>>>,
    locks: Mutex<HashMap<String, LockInfo>>,
    seq: AtomicU64,
    /// 直近の定期メンテナンス実行時刻(M5 F-3。10 分に 1 回だけ動かす)。
    last_maintenance: Mutex<Option<Instant>>,
}

impl MemoService {
    pub fn new(config_path: &Path) -> Arc<Self> {
        Arc::new(Self {
            config_path: config_path.to_path_buf(),
            db_path: config_path.with_extension("memos.db"),
            connections: Mutex::new(None),
            groups: Mutex::new(None),
            last_group_sig: Mutex::new(None),
            locks: Mutex::new(HashMap::new()),
            seq: AtomicU64::new(1),
            last_maintenance: Mutex::new(None),
        })
    }

    /// supervisor 起動時に接続表を差し込む(入れ直しをまたいで使い回す)。
    pub fn attach_connections(&self, connections: Connections) {
        *self.connections.lock().unwrap() = Some(connections);
    }

    /// supervisor 起動時にグループ表を差し込む(ADR-0051。attach_connections
    /// と同じパターン。入れ直しをまたいで使い回す)。
    pub fn attach_groups(&self, groups: crate::groups::SharedGroups) {
        *self.groups.lock().unwrap() = Some(groups);
    }

    /// `ip` が現在属しているグループの id 一覧(ADR-0051)。GroupStore は
    /// 全グループ(自分が非メンバーのものも)を持つため、ここで
    /// 「members に ip を含むか」で絞り込む。未 attach なら空。
    fn group_ids_for(&self, ip: Ipv4Addr) -> Vec<String> {
        let Some(groups) = self.groups.lock().unwrap().clone() else {
            return Vec::new();
        };
        let store = groups.lock().unwrap();
        store
            .all()
            .into_iter()
            .filter(|g| g.members.contains(&ip))
            .map(|g| g.id)
            .collect()
    }

    /// detail に載るグループ権限の表示名を、GroupStore に現存するグループ
    /// なら現在名で上書きする(改名追従)。ストアの保存値はスナップショット
    /// のままなので、返す直前にサービス層で行う。
    fn refresh_group_names(&self, memo: &mut SharedMemoDetail) {
        if memo.groups.is_empty() {
            return;
        }
        let Some(groups) = self.groups.lock().unwrap().clone() else {
            return;
        };
        let store = groups.lock().unwrap();
        for perm in &mut memo.groups {
            if let Some(group) = store.get(&perm.group_id) {
                perm.name = group.name;
            }
        }
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
            Ok(actor) => actor.with_groups(self.group_ids_for(member_ip)),
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
                self.refresh_group_names(&mut memo);
                Ok(SharedMemoReply::Memo { memo })
            }
            SharedMemoOp::ResolveTitles { titles } => {
                let map = self
                    .blocking({
                        let actor = actor.clone();
                        move |store| store.resolve_titles(&actor, &titles)
                    })
                    .await?;
                Ok(SharedMemoReply::Titles { map })
            }
            SharedMemoOp::Backlinks { id } => {
                let memos = self
                    .blocking({
                        let actor = actor.clone();
                        move |store| store.backlinks(&actor, &id)
                    })
                    .await?;
                Ok(SharedMemoReply::Memos {
                    memos,
                    folders: Vec::new(),
                    offline: false,
                })
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
                self.refresh_group_names(&mut memo);
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
                            revision_at_acquire: memo.revision,
                        },
                    );
                }
                memo.locked_by = Some(actor.name.clone());
                self.refresh_group_names(&mut memo);
                self.broadcast_lock(&id, Some(actor.name.clone()));
                self.bump();
                Ok(SharedMemoReply::Memo { memo })
            }
            SharedMemoOp::ReleaseLock { id } => {
                let revision_at_acquire = {
                    let mut locks = self.locks.lock().unwrap();
                    match locks.get(&id) {
                        Some(lock) if lock.key == key => {
                            let revision = lock.revision_at_acquire;
                            locks.remove(&id);
                            Some(revision)
                        }
                        _ => None,
                    }
                };
                if let Some(revision) = revision_at_acquire {
                    self.snapshot_after_unlock(&id, revision).await;
                    self.broadcast_lock(&id, None);
                    self.bump();
                }
                Ok(SharedMemoReply::Done)
            }
            SharedMemoOp::ForceUnlock { id } => {
                if actor.member_id.is_some() {
                    bail!("編集ロックの強制解除はホスト管理者だけができます");
                }
                let revision_at_acquire = self
                    .locks
                    .lock()
                    .unwrap()
                    .remove(&id)
                    .map(|lock| lock.revision_at_acquire);
                if let Some(revision) = revision_at_acquire {
                    tracing::info!("共有メモの編集ロックを強制解除しました(memo={id})");
                    self.snapshot_after_unlock(&id, revision).await;
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
                groups,
            } => {
                let mut memo = self
                    .blocking({
                        let actor = actor.clone();
                        let id = id.clone();
                        move |store| {
                            store.set_perms(&actor, &id, everyone, &members, groups.as_deref())
                        }
                    })
                    .await?;
                memo.locked_by = self.lock_holder(&memo.id);
                self.refresh_group_names(&mut memo);
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
            SharedMemoOp::HistoryList { id } => {
                let entries = self
                    .blocking({
                        let actor = actor.clone();
                        move |store| store.history_list(&actor, &id)
                    })
                    .await?;
                Ok(SharedMemoReply::History { entries })
            }
            SharedMemoOp::HistoryGet { id, hid } => {
                let detail = self
                    .blocking({
                        let actor = actor.clone();
                        move |store| store.history_get(&actor, &id, hid)
                    })
                    .await?;
                Ok(SharedMemoReply::HistoryDetail { detail })
            }
            SharedMemoOp::HistoryDiff {
                id,
                from_hid,
                to_hid,
            } => {
                let lines = self
                    .blocking({
                        let actor = actor.clone();
                        move |store| store.history_diff(&actor, &id, from_hid, to_hid)
                    })
                    .await?;
                Ok(SharedMemoReply::Diff { lines })
            }
            SharedMemoOp::SaveVersion { id } => {
                self.blocking({
                    let actor = actor.clone();
                    move |store| store.save_version(&actor, &id)
                })
                .await?;
                Ok(SharedMemoReply::Done)
            }
            SharedMemoOp::HistoryRestore { id, hid } => {
                // 単一編集者ロック: 他人が編集中なら拒否。自分が保持中(または
                // 誰も保持していない)なら許可する(要件はロック取得を強制しない)
                {
                    let mut locks = self.locks.lock().unwrap();
                    prune_expired(&mut locks);
                    if let Some(lock) = locks.get(&id) {
                        if lock.key != key {
                            bail!("「{}」が編集中です", lock.name);
                        }
                    }
                }
                let mut memo = self
                    .blocking({
                        let actor = actor.clone();
                        let id = id.clone();
                        move |store| store.history_restore(&actor, &id, hid)
                    })
                    .await?;
                memo.locked_by = self.lock_holder(&memo.id);
                self.refresh_group_names(&mut memo);
                self.broadcast_changed(id);
                self.bump();
                Ok(SharedMemoReply::Memo { memo })
            }
            SharedMemoOp::GetLimits => {
                // 誰でも可(秘匿情報ではない)
                let limits = self.blocking(move |store| store.limits()).await?;
                Ok(SharedMemoReply::Limits { limits })
            }
            SharedMemoOp::SetLimits { limits } => {
                // ホスト検査は store 側(set_limits)で行う
                self.blocking(move |store| store.set_limits(&actor, &limits))
                    .await?;
                Ok(SharedMemoReply::Done)
            }
            SharedMemoOp::CommentList { id } => {
                let comments = self
                    .blocking({
                        let actor = actor.clone();
                        move |store| store.comment_list(&actor, &id)
                    })
                    .await?;
                Ok(SharedMemoReply::Comments { comments })
            }
            SharedMemoOp::CommentAdd { id, body } => {
                let comment = self
                    .blocking({
                        let actor = actor.clone();
                        let id = id.clone();
                        move |store| store.comment_add(&actor, &id, &body)
                    })
                    .await?;
                // comment_count が変わるので既存の Changed 配信に相乗りする
                // (ADR-0052 決定 4: 新しいイベント種別は追加しない)。受信側は
                // detail の comment_count の増分を見てコメントを取り直し、
                // メンション・自メモ宛の通知を判定する
                self.broadcast_changed(id);
                self.bump();
                Ok(SharedMemoReply::Comment { comment })
            }
            SharedMemoOp::CommentDelete { id, comment_id } => {
                self.blocking({
                    let actor = actor.clone();
                    let id = id.clone();
                    move |store| store.comment_delete(&actor, &id, &comment_id)
                })
                .await?;
                self.broadcast_changed(id);
                self.bump();
                Ok(SharedMemoReply::Done)
            }
            SharedMemoOp::Schedule { schedule } => {
                let reply = self.handle_schedule(actor, schedule).await?;
                Ok(SharedMemoReply::Schedule { reply })
            }
            SharedMemoOp::Sheet { sheet } => {
                let reply = self.handle_sheet(actor, sheet).await?;
                Ok(SharedMemoReply::Sheet { reply })
            }
        }
    }

    /// 共有スケジュール表(M6 G-1、ADR-0053)。全員閲覧・追加可、編集・削除は
    /// 作成者 + ホストのみ(権限判定は store 側)。編集ロックは持たない
    /// (revision CAS のみ)。
    async fn handle_schedule(
        self: &Arc<Self>,
        actor: Actor,
        op: ScheduleOp,
    ) -> anyhow::Result<ScheduleReply> {
        match op {
            ScheduleOp::List => {
                let events = self
                    .blocking({
                        let actor = actor.clone();
                        move |store| store.schedule_list(&actor)
                    })
                    .await?;
                Ok(ScheduleReply::Events {
                    events,
                    offline: false,
                })
            }
            ScheduleOp::Create {
                title,
                note,
                start_unix_ms,
                end_unix_ms,
                all_day,
            } => {
                let event = self
                    .blocking({
                        let actor = actor.clone();
                        move |store| {
                            store.schedule_create(
                                &actor,
                                &title,
                                &note,
                                start_unix_ms,
                                end_unix_ms,
                                all_day,
                            )
                        }
                    })
                    .await?;
                self.broadcast_schedule_changed(event.clone());
                self.bump();
                Ok(ScheduleReply::Event { event })
            }
            ScheduleOp::Update {
                id,
                base_revision,
                title,
                note,
                start_unix_ms,
                end_unix_ms,
                all_day,
            } => {
                let event = self
                    .blocking({
                        let actor = actor.clone();
                        let id = id.clone();
                        move |store| {
                            store.schedule_update(
                                &actor,
                                &id,
                                base_revision,
                                &title,
                                &note,
                                start_unix_ms,
                                end_unix_ms,
                                all_day,
                            )
                        }
                    })
                    .await?;
                self.broadcast_schedule_changed(event.clone());
                self.bump();
                Ok(ScheduleReply::Event { event })
            }
            ScheduleOp::Delete { id } => {
                self.blocking({
                    let actor = actor.clone();
                    let id = id.clone();
                    move |store| store.schedule_delete(&actor, &id)
                })
                .await?;
                self.broadcast_schedule_removed(&id);
                self.bump();
                Ok(ScheduleReply::Done)
            }
        }
    }

    /// 共有シート(M6 G-2、ADR-0054)。全員閲覧・セル編集可、シートの作成・
    /// 改名・削除は作成者 + ホストのみ(権限判定は store 側)。編集ロックは
    /// 持たない(セル単位の revision CAS のみ)。
    async fn handle_sheet(
        self: &Arc<Self>,
        actor: Actor,
        op: SheetOp,
    ) -> anyhow::Result<SheetReply> {
        match op {
            SheetOp::List => {
                let sheets = self
                    .blocking({
                        let actor = actor.clone();
                        move |store| store.sheet_list(&actor)
                    })
                    .await?;
                Ok(SheetReply::Sheets {
                    sheets,
                    offline: false,
                })
            }
            SheetOp::Cells { sheet_id } => {
                let cells = self
                    .blocking({
                        let sheet_id = sheet_id.clone();
                        move |store| store.sheet_cells(&sheet_id)
                    })
                    .await?;
                Ok(SheetReply::CellsData {
                    sheet_id,
                    cells,
                    offline: false,
                })
            }
            SheetOp::Create { name } => {
                let sheet = self
                    .blocking({
                        let actor = actor.clone();
                        move |store| store.sheet_create(&actor, &name)
                    })
                    .await?;
                self.broadcast_sheet_changed(sheet.clone());
                self.bump();
                Ok(SheetReply::Sheet { sheet })
            }
            SheetOp::Rename { sheet_id, name } => {
                let sheet = self
                    .blocking({
                        let actor = actor.clone();
                        let sheet_id = sheet_id.clone();
                        move |store| store.sheet_rename(&actor, &sheet_id, &name)
                    })
                    .await?;
                self.broadcast_sheet_changed(sheet.clone());
                self.bump();
                Ok(SheetReply::Sheet { sheet })
            }
            SheetOp::Delete { sheet_id } => {
                self.blocking({
                    let actor = actor.clone();
                    let sheet_id = sheet_id.clone();
                    move |store| store.sheet_delete(&actor, &sheet_id)
                })
                .await?;
                self.broadcast_sheet_removed(&sheet_id);
                self.bump();
                Ok(SheetReply::Done)
            }
            SheetOp::Write { sheet_id, cells } => {
                let (applied, conflicts, changed) = self
                    .blocking({
                        let actor = actor.clone();
                        let sheet_id = sheet_id.clone();
                        move |store| store.sheet_write(&actor, &sheet_id, &cells)
                    })
                    .await?;
                if applied > 0 {
                    self.broadcast_sheet_cells_changed(sheet_id, changed);
                    self.bump();
                }
                Ok(SheetReply::WriteResult { applied, conflicts })
            }
        }
    }

    /// ロック解放時、確立時点から revision が変わっていれば履歴へ "close" 版を
    /// 残す(要件: 編集終了時に 1 版)。失敗してもロック解放自体は妨げない
    /// (memo_id のみ warn。内容は出さない — ADR-0049)。
    async fn snapshot_after_unlock(self: &Arc<Self>, id: &str, revision_at_acquire: u64) {
        let target = id.to_string();
        let result = self
            .blocking(move |store| store.snapshot_if_revision_changed(&target, revision_at_acquire))
            .await;
        if let Err(e) = result {
            tracing::warn!("共有メモの履歴スナップショットに失敗しました(memo={id}): {e:#}");
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
            let released: Vec<(String, u64)> = {
                let mut locks = service.locks.lock().unwrap();
                let ids: Vec<(String, u64)> = locks
                    .iter()
                    .filter(|(_, lock)| lock.key == key)
                    .map(|(id, lock)| (id.clone(), lock.revision_at_acquire))
                    .collect();
                for (id, _) in &ids {
                    locks.remove(id);
                }
                ids
            };
            if released.is_empty() {
                return;
            }
            snapshot_closed(&service.db_path, &released);
            for (id, _) in &released {
                service.broadcast_lock(id, None);
                service.bump();
            }
        });
    }

    /// 無操作タイムアウトのロックを解放する(supervisor の周期から呼ぶ)。
    pub fn sweep_expired_locks(self: &Arc<Self>) {
        let expired: Vec<(String, u64)> = {
            let mut locks = self.locks.lock().unwrap();
            let ids: Vec<(String, u64)> = locks
                .iter()
                .filter(|(_, lock)| lock.last_activity.elapsed() > LOCK_TTL)
                .map(|(id, lock)| (id.clone(), lock.revision_at_acquire))
                .collect();
            for (id, _) in &ids {
                locks.remove(id);
            }
            ids
        };
        if expired.is_empty() {
            return;
        }
        {
            // DB IO なのでバックグラウンドで行う(ロック解放・配信は待たない)
            let db_path = self.db_path.clone();
            let released = expired.clone();
            tokio::task::spawn_blocking(move || snapshot_closed(&db_path, &released));
        }
        for (id, _) in expired {
            tracing::debug!("共有メモの編集ロックを無操作で解放しました(memo={id})");
            self.broadcast_lock(&id, None);
            self.bump();
        }
    }

    /// 定期メンテナンス(M5 F-3)。10 分に 1 回だけ、ゴミ箱の完全削除と WAL
    /// チェックポイントを行う。supervisor の tick(sweep_expired_locks と
    /// 同じ場所)から毎回呼んでよい(呼び出し側でのゲートは不要)。
    pub fn maintain(self: &Arc<Self>) {
        // 共有メモが一度も使われていなければ DB を作ってまで掃除しない
        if !self.db_path.exists() {
            return;
        }
        {
            let mut last = self.last_maintenance.lock().unwrap();
            let due = match *last {
                Some(instant) => instant.elapsed() >= MAINTENANCE_INTERVAL,
                None => true,
            };
            if !due {
                return;
            }
            *last = Some(Instant::now());
        }
        let db_path = self.db_path.clone();
        tokio::task::spawn_blocking(move || {
            let store = match SharedStore::open(&db_path) {
                Ok(store) => store,
                Err(e) => {
                    tracing::warn!("共有メモの定期メンテナンスに失敗しました: {e:#}");
                    return;
                }
            };
            match store.purge_expired() {
                Ok(0) => {}
                Ok(n) => tracing::info!("共有メモのゴミ箱から {n} 件を完全削除しました"),
                Err(e) => tracing::warn!("共有メモのゴミ箱整理に失敗しました: {e:#}"),
            }
            if let Err(e) = store.checkpoint() {
                tracing::warn!("共有メモの WAL チェックポイントに失敗しました: {e:#}");
            }
        });
    }

    /// グループの改名・メンバー増減・削除への追従(ADR-0051)。全グループの
    /// (id, revision) から作った署名を前回と比較し、**変化した時だけ**
    /// グループ権限を持つメモを受信者ごとに可視判定し直して再配信する
    /// (見える人には Changed、見えなくなった人には Removed —
    /// `broadcast_changed` がそのまま面倒を見る)。supervisor の tick から
    /// (sweep_expired_locks / maintain と同じ場所で)毎周期呼んでよい。
    pub fn watch_groups(self: &Arc<Self>) {
        let Some(groups) = self.groups.lock().unwrap().clone() else {
            return;
        };
        let sig: Vec<(String, u64)> = {
            let store = groups.lock().unwrap();
            let mut sig: Vec<(String, u64)> = store
                .all()
                .into_iter()
                .map(|g| (g.id, g.revision))
                .collect();
            sig.sort();
            sig
        };
        let changed = {
            let mut last = self.last_group_sig.lock().unwrap();
            let changed = last.as_ref() != Some(&sig);
            *last = Some(sig);
            changed
        };
        if !changed || !self.db_path.exists() {
            return;
        }
        let service = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let store = match SharedStore::open(&service.db_path) {
                Ok(store) => store,
                Err(e) => {
                    tracing::warn!("共有メモのグループ権限再配信に失敗しました: {e:#}");
                    return;
                }
            };
            match store.memo_ids_with_group_perms() {
                Ok(ids) => {
                    for id in ids {
                        service.broadcast_changed(id);
                        service.bump();
                    }
                }
                Err(e) => tracing::warn!("共有メモのグループ権限再配信に失敗しました: {e:#}"),
            }
        });
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
                // グループ権限のメモをリアルタイム配信するには、受信者ごとの
                // Actor に所属グループを付けておく必要がある(ADR-0051)
                let actor = actor.with_groups(service.group_ids_for(ip));
                let event = match store.detail_if_visible(&actor, &id) {
                    Ok(Some(mut memo)) => {
                        memo.locked_by = holder.clone();
                        service.refresh_group_names(&mut memo);
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

    /// 予定の作成・更新を全員へ配信する(**閲覧権限フィルタなし** —
    /// ADR-0053 決定 3。可視性で振り分ける共有メモの broadcast_changed とは
    /// 異なる)。受信者ごとに can_edit だけ計算し直して詰める。
    fn broadcast_schedule_changed(self: &Arc<Self>, event: ScheduleEvent) {
        let service = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let targets = service.memo_connections();
            if targets.is_empty() {
                return;
            }
            let Ok(config) = Config::load(&service.config_path) else {
                return;
            };
            for (ip, connection) in targets {
                let Ok(actor) = Self::actor_for_ip(&config, ip) else {
                    continue;
                };
                let mut for_actor = event.clone();
                for_actor.can_edit = actor.member_id.is_none()
                    || actor.member_id.as_deref() == Some(event.owner_id.as_str());
                connection.send(ControlMessage::MemoEvent {
                    event: SharedMemoEvent::Schedule {
                        schedule: ScheduleEventMsg::Changed { event: for_actor },
                    },
                });
            }
        });
    }

    /// 予定の削除を全員へ配信する(フィルタなし)。
    fn broadcast_schedule_removed(self: &Arc<Self>, id: &str) {
        let event = SharedMemoEvent::Schedule {
            schedule: ScheduleEventMsg::Removed { id: id.to_string() },
        };
        for (_, connection) in self.memo_connections() {
            connection.send(ControlMessage::MemoEvent {
                event: event.clone(),
            });
        }
    }

    /// シートの作成・改名を全員へ配信する(**閲覧権限フィルタなし** —
    /// ADR-0054 決定 5)。受信者ごとに can_manage だけ計算し直して詰める。
    fn broadcast_sheet_changed(self: &Arc<Self>, sheet: SheetMeta) {
        let service = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let targets = service.memo_connections();
            if targets.is_empty() {
                return;
            }
            let Ok(config) = Config::load(&service.config_path) else {
                return;
            };
            for (ip, connection) in targets {
                let Ok(actor) = Self::actor_for_ip(&config, ip) else {
                    continue;
                };
                let mut for_actor = sheet.clone();
                for_actor.can_manage = actor.member_id.is_none()
                    || actor.member_id.as_deref() == Some(sheet.owner_id.as_str());
                connection.send(ControlMessage::MemoEvent {
                    event: SharedMemoEvent::Sheet {
                        sheet: SheetEventMsg::SheetChanged { sheet: for_actor },
                    },
                });
            }
        });
    }

    /// シートの削除を全員へ配信する(フィルタなし)。
    fn broadcast_sheet_removed(self: &Arc<Self>, sheet_id: &str) {
        let event = SharedMemoEvent::Sheet {
            sheet: SheetEventMsg::SheetRemoved {
                sheet_id: sheet_id.to_string(),
            },
        };
        for (_, connection) in self.memo_connections() {
            connection.send(ControlMessage::MemoEvent {
                event: event.clone(),
            });
        }
    }

    /// セルのバッチ変更を全員へ配信する(**閲覧権限フィルタなし・受信者ごとの
    /// 再計算も不要** — 全員が同じ内容を編集できる、ADR-0054 決定 5)。
    fn broadcast_sheet_cells_changed(self: &Arc<Self>, sheet_id: String, cells: Vec<SheetCell>) {
        if cells.is_empty() {
            return;
        }
        let event = SharedMemoEvent::Sheet {
            sheet: SheetEventMsg::CellsChanged { sheet_id, cells },
        };
        for (_, connection) in self.memo_connections() {
            connection.send(ControlMessage::MemoEvent {
                event: event.clone(),
            });
        }
    }
}

fn prune_expired(locks: &mut HashMap<String, LockInfo>) {
    locks.retain(|_, lock| lock.last_activity.elapsed() <= LOCK_TTL);
}

/// (同期文脈から)複数メモの編集終了時スナップショットをまとめて行う。
/// DB を開けない・書けない場合も呼び出し元のロック解放は既に完了している
/// ため warn に留める(memo_id のみ。内容は出さない — ADR-0049)。
fn snapshot_closed(db_path: &Path, released: &[(String, u64)]) {
    match SharedStore::open(db_path) {
        Ok(store) => {
            for (id, revision) in released {
                if let Err(e) = store.snapshot_if_revision_changed(id, *revision) {
                    tracing::warn!(
                        "共有メモの履歴スナップショットに失敗しました(memo={id}): {e:#}"
                    );
                }
            }
        }
        Err(e) => tracing::warn!("共有メモの履歴スナップショットに失敗しました: {e:#}"),
    }
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
            SharedMemoEvent::Schedule { schedule } => match schedule {
                ScheduleEventMsg::Changed { event } => {
                    self.blocking(move |store| store.schedule_upsert(&event))
                        .await
                }
                ScheduleEventMsg::Removed { id } => {
                    self.blocking(move |store| store.schedule_remove(&id)).await
                }
            },
            SharedMemoEvent::Sheet { sheet } => match sheet {
                SheetEventMsg::SheetChanged { sheet } => {
                    self.blocking(move |store| store.sheet_upsert(&sheet)).await
                }
                SheetEventMsg::SheetRemoved { sheet_id } => {
                    self.blocking(move |store| store.sheet_remove(&sheet_id))
                        .await
                }
                SheetEventMsg::CellsChanged { sheet_id, cells } => {
                    self.blocking(move |store| store.sheet_cells_apply(&sheet_id, &cells))
                        .await
                }
            },
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

    /// メモ間リンクの解決(キャッシュから。オフラインでも使える、ADR-0052)。
    pub async fn resolve_titles(
        self: &Arc<Self>,
        titles: Vec<String>,
    ) -> anyhow::Result<HashMap<String, String>> {
        self.blocking(move |store| store.resolve_titles(&titles))
            .await
    }

    /// バックリンク(キャッシュから。オフラインでも使える、ADR-0052)。
    pub async fn backlinks(self: &Arc<Self>, id: String) -> anyhow::Result<Vec<SharedMemoSummary>> {
        self.blocking(move |store| store.backlinks(&id)).await
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

    /// 一覧(オフライン時はここが唯一のソース、M6 G-1)。
    pub async fn schedule_list(self: &Arc<Self>) -> anyhow::Result<Vec<ScheduleEvent>> {
        self.blocking(|store| store.schedule_list()).await
    }

    /// 接続時の同期: List 応答で全量を置き換える(メモと違い差分取得は
    /// 行わない — 件数上限が小さく全量置き換えで十分軽い、ADR-0053)。
    pub async fn schedule_sync_from_list(
        self: &Arc<Self>,
        events: Vec<ScheduleEvent>,
    ) -> anyhow::Result<()> {
        self.blocking(move |store| store.schedule_replace_all(&events))
            .await?;
        self.bump();
        Ok(())
    }

    /// シート一覧(オフライン時はここが唯一のソース、M6 G-2)。
    pub async fn sheet_list(self: &Arc<Self>) -> anyhow::Result<Vec<SheetMeta>> {
        self.blocking(|store| store.sheet_list()).await
    }

    /// 1 シートの全非空セル(オフライン時はここが唯一のソース、M6 G-2)。
    pub async fn sheet_cells(self: &Arc<Self>, sheet_id: String) -> anyhow::Result<Vec<SheetCell>> {
        self.blocking(move |store| store.sheet_cells(&sheet_id))
            .await
    }

    /// 接続時の同期: List 応答でシートのメタ全量を置き換える(孤児セルも
    /// 掃除される、ADR-0054)。
    pub async fn sheet_sync_from_list(
        self: &Arc<Self>,
        sheets: Vec<SheetMeta>,
    ) -> anyhow::Result<()> {
        self.blocking(move |store| store.sheet_replace_all(&sheets))
            .await?;
        self.bump();
        Ok(())
    }

    /// 接続時の同期: 1 シートの Cells 応答でそのシートのセル全量を置き換える。
    pub async fn sheet_sync_cells(
        self: &Arc<Self>,
        sheet_id: String,
        cells: Vec<SheetCell>,
    ) -> anyhow::Result<()> {
        self.blocking(move |store| store.sheet_cells_replace_all(&sheet_id, &cells))
            .await?;
        self.bump();
        Ok(())
    }
}
