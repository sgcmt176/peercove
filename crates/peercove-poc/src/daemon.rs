//! デーモン(M2-G1a、ADR-0007)。
//!
//! `peercove-poc daemon run` で常駐し、ローカル IPC(Windows: 名前付きパイプ /
//! Linux: Unix ドメインソケット)でトンネルの開始・停止・状態取得を受け付ける。
//! 招待・削除などの設定ファイル操作は IPC に乗せない(UI/CLI が直接行い、
//! 実行中トンネルは 5 秒再読込で追随する)。
//!
//! トランスポート非依存の部分(`handle_connection` / `request_over`)は
//! 任意の AsyncRead+AsyncWrite で動き、テストは `tokio::io::duplex` で行う。

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use anyhow::{bail, Context};
use peercove_core::dns::DnsRecord;
use peercove_core::ipc::{
    ChatMessageInfo, DaemonStatus, IpcEnvelope, IpcReply, IpcRequest, IpcResponse, IpcResult,
    PeerSummary, TunnelInfo, TunnelRole, IPC_VERSION,
};
use peercove_ipc::MAX_LINE_LEN;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::watch;

use crate::commands::tunnel::{self, ActiveTunnel, Role, SharedSnapshot, StartLimits};

/// トンネルの起動方法(テストでは差し替える)。
type BringUp =
    Box<dyn Fn(&Path, Role, bool, &StartLimits) -> anyhow::Result<ActiveTunnel> + Send + Sync>;

/// デーモンの共有状態。複数ネットワークを同時に張れる(ADR-0012)。
/// キーは設定ファイルの絶対パス。
pub struct DaemonShared {
    active: tokio::sync::Mutex<HashMap<PathBuf, Active>>,
    bring_up: BringUp,
    shutdown_tx: watch::Sender<bool>,
    /// 全ネットワーク合算の DNS ゾーン(M3-1)。トンネルごとの DNS サーバが参照し、
    /// [`Self::refresh_zones`] が台帳の更新に合わせて書き換える。
    zones: crate::dns::SharedZones,
    /// OS のスプリット DNS 設定(NRPT / resolvectl)を触るか。
    /// serve()(実デーモン)だけ true。テストでは OS を触らない
    manage_os_dns: bool,
}

struct Active {
    role: Role,
    config: PathBuf,
    network: String,
    address: Ipv4Addr,
    /// 衝突検査(StartLimits)用
    subnet: ipnet::Ipv4Net,
    if_name: String,
    stop_tx: watch::Sender<bool>,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
    /// 内蔵 DNS サーバ(トンネル IP の :53、M3-1)。停止時に abort する
    dns_task: tokio::task::JoinHandle<()>,
    snapshot: SharedSnapshot,
    /// ファイル転送の進捗一覧(ADR-0015、M3-9)。supervise 内の受信サーバーと
    /// SendFile の送信タスクが書き、status 応答に載せる
    transfers: crate::msg::TransferRegistry,
    /// チャット履歴(ADR-0016、M3-13)。supervise 内の受信サーバーと
    /// ChatSend が書き、ChatFetch / status 応答(chat_seq)が読む
    chat: crate::chat::SharedChatLog,
    /// 既知のグループ(ADR-0016、M3-13c)。supervise 内の受信サーバーと
    /// Group 系リクエストが書き、status 応答(groups)が読む
    groups: crate::groups::SharedGroups,
}

impl DaemonShared {
    fn new(bring_up: BringUp, manage_os_dns: bool) -> (Arc<Self>, watch::Receiver<bool>) {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        (
            Arc::new(Self {
                active: tokio::sync::Mutex::new(HashMap::new()),
                bring_up,
                shutdown_tx,
                zones: Default::default(),
                manage_os_dns,
            }),
            shutdown_rx,
        )
    }

    async fn dispatch(self: &Arc<Self>, request: IpcRequest) -> anyhow::Result<IpcResponse> {
        match request {
            IpcRequest::Status => Ok(IpcResponse::Status(self.status().await)),
            IpcRequest::StartHost { config, upnp } => {
                self.start(config, Role::Host, upnp).await?;
                Ok(IpcResponse::Done)
            }
            IpcRequest::StartMember { config } => {
                self.start(config, Role::Member, false).await?;
                Ok(IpcResponse::Done)
            }
            IpcRequest::Stop { config } => {
                self.stop(config).await?;
                Ok(IpcResponse::Done)
            }
            IpcRequest::Shutdown => {
                self.stop_all().await;
                let _ = self.shutdown_tx.send(true);
                Ok(IpcResponse::Done)
            }
            IpcRequest::Logs { after_seq } => {
                let (lines, dropped) = crate::logbuf::ring().since(after_seq);
                Ok(IpcResponse::Logs { lines, dropped })
            }
            IpcRequest::SendFile { config, peer, path } => {
                let id = self.send_file(config, peer, path).await?;
                Ok(IpcResponse::Transfer { id })
            }
            IpcRequest::ChatSend {
                config,
                scope,
                peer,
                group_id,
                text,
            } => self.chat_send(config, scope, peer, group_id, text).await,
            IpcRequest::GroupCreate {
                config,
                name,
                members,
            } => self.group_create(config, name, members).await,
            IpcRequest::GroupUpdate {
                config,
                id,
                name,
                add,
            } => self.group_update(config, id, name, add).await,
            IpcRequest::GroupLeave { config, id } => self.group_leave(config, id).await,
            IpcRequest::ChatFetch { config, after_seq } => {
                let active = self.active.lock().await;
                let active = active.get(&Self::key_for(&config)).with_context(|| {
                    format!("この設定のトンネルは動いていません({})", config.display())
                })?;
                let (seq, messages) = active.chat.lock().unwrap().fetch(after_seq);
                Ok(IpcResponse::Chat { seq, messages })
            }
        }
    }

    /// チャットを送る(ADR-0016、M3-13)。履歴への記録は即時、相手への配送は
    /// バックグラウンド(全宛先に失敗したら履歴に失敗の印が付く)。
    async fn chat_send(
        &self,
        config: PathBuf,
        scope: peercove_core::msg::ChatScope,
        peer: Option<Ipv4Addr>,
        group_id: Option<String>,
        text: String,
    ) -> anyhow::Result<IpcResponse> {
        use peercove_core::msg::{ChatScope, MAX_CHAT_TEXT_BYTES};

        if text.trim().is_empty() {
            bail!("本文が空です");
        }
        if text.len() > MAX_CHAT_TEXT_BYTES {
            bail!("本文が長すぎます(上限 {} KB)", MAX_CHAT_TEXT_BYTES / 1024);
        }
        let active = self.active.lock().await;
        let active = active
            .get(&Self::key_for(&config))
            .with_context(|| format!("この設定のトンネルは動いていません({})", config.display()))?;
        let ledger = {
            let snapshot = active.snapshot.lock().unwrap();
            snapshot
                .as_ref()
                .and_then(|s| s.ledger.clone())
                .unwrap_or_default()
        };
        // 宛先の決定(オフライン宛は V1 非対応 — ADR-0015/0016)
        let targets: Vec<Ipv4Addr> = match scope {
            ChatScope::Direct => {
                let peer = peer.context("宛先(peer)が指定されていません")?;
                if peer == active.address {
                    bail!("自分自身へは送れません");
                }
                let entry = ledger
                    .iter()
                    .find(|e| e.ip == peer)
                    .with_context(|| format!("{peer} はこのネットワークのメンバーにいません"))?;
                if !entry.online {
                    bail!(
                        "{} はオフラインです(オフラインのメンバーへは送れません)",
                        entry.name.as_deref().unwrap_or(&peer.to_string())
                    );
                }
                vec![peer]
            }
            // 全体宛: 送信時にオンラインのメンバー全員へ個別送信。全員オフライン
            // でも履歴には残す(誰にも届かないことは README に明記)
            ChatScope::Network => ledger
                .iter()
                .filter(|e| e.ip != active.address && e.online)
                .map(|e| e.ip)
                .collect(),
            // グループ宛: オンラインのグループメンバーへ個別送信(M3-13c)
            ChatScope::Group => {
                let group_id = group_id
                    .as_deref()
                    .context("宛先グループ(group_id)が指定されていません")?;
                let group =
                    active.groups.lock().unwrap().get(group_id).context(
                        "このグループはありません(退出したか、まだ情報が届いていません)",
                    )?;
                if !group.members.contains(&active.address) {
                    bail!("このグループのメンバーではありません");
                }
                ledger
                    .iter()
                    .filter(|e| e.ip != active.address && e.online && group.members.contains(&e.ip))
                    .map(|e| e.ip)
                    .collect()
            }
        };
        let id = crate::msg::new_transfer_id();
        let sent_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let entry = active.chat.lock().unwrap().append(ChatMessageInfo {
            seq: 0, // append が振る
            id: id.clone(),
            scope,
            group_id: match scope {
                ChatScope::Group => group_id.clone(),
                _ => None,
            },
            from: active.address,
            to: match scope {
                ChatScope::Direct => peer,
                ChatScope::Network | ChatScope::Group => None,
            },
            text: text.clone(),
            sent_at,
            failed: false,
        });
        let chat = Arc::clone(&active.chat);
        let seq = entry.seq;
        tokio::spawn(async move {
            let mut delivered = targets.is_empty(); // 宛先ゼロの全体/グループ宛は失敗扱いにしない
            let sends = targets.into_iter().map(|target| {
                let id = id.clone();
                let group_id = group_id.clone();
                let text = text.clone();
                tokio::spawn(async move {
                    // 本文はログに出さない(秘匿ルール)
                    match crate::msg::send_chat(
                        target,
                        &id,
                        scope,
                        group_id.as_deref(),
                        &text,
                        sent_at,
                    )
                    .await
                    {
                        Ok(()) => true,
                        Err(e) => {
                            tracing::warn!("{target} へのチャット送信に失敗しました: {e:#}");
                            false
                        }
                    }
                })
            });
            for send in sends.collect::<Vec<_>>() {
                delivered |= send.await.unwrap_or(false);
            }
            if !delivered {
                chat.lock().unwrap().mark_failed(seq);
            }
        });
        Ok(IpcResponse::Chat {
            seq,
            messages: vec![entry],
        })
    }

    /// グループを作る(ADR-0016、M3-13c)。`members` に自分は含めなくてよい
    /// (必ず足す)。オフラインのメンバーも入れられる(オンライン復帰時の
    /// 追いつき再送で届く)。
    async fn group_create(
        &self,
        config: PathBuf,
        name: String,
        members: Vec<Ipv4Addr>,
    ) -> anyhow::Result<IpcResponse> {
        use peercove_core::msg::{GroupInfo, MAX_GROUP_MEMBERS};

        let name = Self::valid_group_name(&name)?;
        let active = self.active.lock().await;
        let active = active
            .get(&Self::key_for(&config))
            .with_context(|| format!("この設定のトンネルは動いていません({})", config.display()))?;
        let ledger = Self::ledger_of(active);
        let mut group_members = vec![active.address];
        for ip in members {
            if ip == active.address || group_members.contains(&ip) {
                continue;
            }
            if !ledger.iter().any(|e| e.ip == ip) {
                bail!("{ip} はこのネットワークのメンバーにいません");
            }
            group_members.push(ip);
        }
        if group_members.len() < 2 {
            bail!("グループに入れるメンバーを 1 人以上選んでください");
        }
        if group_members.len() > MAX_GROUP_MEMBERS {
            bail!("グループのメンバーが多すぎます(上限 {MAX_GROUP_MEMBERS} 人)");
        }
        let group = GroupInfo {
            id: crate::msg::new_transfer_id(),
            name,
            members: group_members,
            revision: 1,
            updated_by: active.address,
        };
        active.groups.lock().unwrap().apply(group.clone());
        // グループ名はログに出さない(秘匿ルール)
        tracing::info!(
            "グループを作成しました(id={} members={})",
            group.id,
            group.members.len()
        );
        Self::propagate_group(&group, &ledger, active.address);
        Ok(IpcResponse::Group { group })
    }

    /// グループの改名・メンバー追加(M3-13c)。全量 + リビジョンの置換として
    /// 全メンバーへ配る。
    async fn group_update(
        &self,
        config: PathBuf,
        id: String,
        name: Option<String>,
        add: Vec<Ipv4Addr>,
    ) -> anyhow::Result<IpcResponse> {
        use peercove_core::msg::MAX_GROUP_MEMBERS;

        let active = self.active.lock().await;
        let active = active
            .get(&Self::key_for(&config))
            .with_context(|| format!("この設定のトンネルは動いていません({})", config.display()))?;
        let ledger = Self::ledger_of(active);
        let mut group = active
            .groups
            .lock()
            .unwrap()
            .get(&id)
            .context("このグループはありません")?;
        if !group.members.contains(&active.address) {
            bail!("このグループのメンバーではありません");
        }
        if let Some(name) = name {
            group.name = Self::valid_group_name(&name)?;
        }
        for ip in add {
            if group.members.contains(&ip) {
                continue;
            }
            if !ledger.iter().any(|e| e.ip == ip) {
                bail!("{ip} はこのネットワークのメンバーにいません");
            }
            group.members.push(ip);
        }
        if group.members.len() > MAX_GROUP_MEMBERS {
            bail!("グループのメンバーが多すぎます(上限 {MAX_GROUP_MEMBERS} 人)");
        }
        group.revision += 1;
        group.updated_by = active.address;
        active.groups.lock().unwrap().apply(group.clone());
        tracing::info!(
            "グループを更新しました(id={} rev={})",
            group.id,
            group.revision
        );
        Self::propagate_group(&group, &ledger, active.address);
        Ok(IpcResponse::Group { group })
    }

    /// 自分がグループから抜ける(M3-13c)。ローカルには「自分抜きの全量」が
    /// 残る(履歴の表示名に使う。UI は会話リストから隠す)。
    async fn group_leave(&self, config: PathBuf, id: String) -> anyhow::Result<IpcResponse> {
        let active = self.active.lock().await;
        let active = active
            .get(&Self::key_for(&config))
            .with_context(|| format!("この設定のトンネルは動いていません({})", config.display()))?;
        let mut group = active
            .groups
            .lock()
            .unwrap()
            .get(&id)
            .context("このグループはありません")?;
        if !group.members.contains(&active.address) {
            bail!("このグループのメンバーではありません");
        }
        group.members.retain(|ip| *ip != active.address);
        group.revision += 1;
        group.updated_by = active.address;
        active.groups.lock().unwrap().apply(group.clone());
        tracing::info!("グループから退出しました(id={})", group.id);
        Self::propagate_group(&group, &Self::ledger_of(active), active.address);
        Ok(IpcResponse::Done)
    }

    /// グループ名の検証(空・上限)。
    fn valid_group_name(name: &str) -> anyhow::Result<String> {
        use peercove_core::msg::MAX_GROUP_NAME_BYTES;
        let name = name.trim();
        if name.is_empty() {
            bail!("グループ名が空です");
        }
        if name.len() > MAX_GROUP_NAME_BYTES {
            bail!("グループ名が長すぎます");
        }
        Ok(name.to_string())
    }

    /// 台帳スナップショットの複製(宛先検証用)。
    fn ledger_of(active: &Active) -> Vec<peercove_core::proto::LedgerEntry> {
        let snapshot = active.snapshot.lock().unwrap();
        snapshot
            .as_ref()
            .and_then(|s| s.ledger.clone())
            .unwrap_or_default()
    }

    /// グループ全量をオンラインの対象メンバーへ配る(バックグラウンド)。
    /// 失敗はオンライン復帰時の追いつき再送に任せる。
    fn propagate_group(
        group: &peercove_core::msg::GroupInfo,
        ledger: &[peercove_core::proto::LedgerEntry],
        self_ip: Ipv4Addr,
    ) {
        for entry in ledger {
            if entry.ip == self_ip || !entry.online || !group.members.contains(&entry.ip) {
                continue;
            }
            let group = group.clone();
            let ip = entry.ip;
            tokio::spawn(async move {
                if let Err(e) = crate::msg::send_group_update(ip, &group).await {
                    tracing::warn!(
                        "{ip} へのグループ更新の送信に失敗しました(オンライン復帰時に再送): {e:#}"
                    );
                }
            });
        }
    }

    /// メンバーへのファイル送信を開始する(ADR-0015、M3-9)。送信自体は
    /// バックグラウンドタスクで走り、進捗は status 応答の transfers で追う。
    async fn send_file(
        &self,
        config: PathBuf,
        peer: Ipv4Addr,
        path: PathBuf,
    ) -> anyhow::Result<String> {
        let active = self.active.lock().await;
        let active = active
            .get(&Self::key_for(&config))
            .with_context(|| format!("この設定のトンネルは動いていません({})", config.display()))?;
        if peer == active.address {
            bail!("自分自身へは送れません");
        }
        // 台帳照合: 宛先はネットワークのメンバーで、オンラインであること
        // (オフライン宛は V1 非対応 — ADR-0015)
        let entry = {
            let snapshot = active.snapshot.lock().unwrap();
            snapshot
                .as_ref()
                .and_then(|s| s.ledger.as_ref())
                .and_then(|ledger| ledger.iter().find(|e| e.ip == peer).cloned())
        };
        let entry =
            entry.with_context(|| format!("{peer} はこのネットワークのメンバーにいません"))?;
        if !entry.online {
            bail!(
                "{} はオフラインです(オフラインのメンバーへは送れません)",
                entry.name.as_deref().unwrap_or(&peer.to_string())
            );
        }
        if !path.is_file() {
            bail!(
                "{} が見つからないか、ファイルではありません",
                path.display()
            );
        }
        let id = crate::msg::new_transfer_id();
        let transfers = Arc::clone(&active.transfers);
        let task_id = id.clone();
        tokio::spawn(async move {
            // ファイル名はログに出さない(秘匿ルール)。進捗一覧にはエラーが載る
            if let Err(e) = crate::msg::send_file(peer, &path, transfers, task_id).await {
                tracing::warn!("{peer} へのファイル送信に失敗しました: {e:#}");
            }
        });
        Ok(id)
    }

    /// IPC で渡されたパスをキー用に正規化する。ファイルが消えている等で
    /// 正規化できない場合は渡されたまま使う(停止要求を弾かないため)。
    fn key_for(config: &Path) -> PathBuf {
        std::fs::canonicalize(config).unwrap_or_else(|_| config.to_path_buf())
    }

    async fn start(
        self: &Arc<Self>,
        config: PathBuf,
        role: Role,
        upnp: bool,
    ) -> anyhow::Result<()> {
        let key = Self::key_for(&config);
        let mut active = self.active.lock().await;
        if active.contains_key(&key) {
            bail!("この設定のトンネルは既に動いています({})", config.display());
        }
        // 稼働中トンネルとの衝突検査(サブネット重複・インターフェース名)は
        // bring_up 内の plan_interface が行う。材料だけここで集める
        let limits = StartLimits {
            used_subnets: active
                .values()
                .map(|a| (a.subnet, a.network.clone()))
                .collect(),
            used_if_names: active.values().map(|a| a.if_name.clone()).collect(),
        };
        // bring_up はブロッキング処理(netlink / netsh / UPnP)なので専用スレッドで
        let shared = Arc::clone(self);
        let config_for_up = config.clone();
        let tunnel = tokio::task::spawn_blocking(move || {
            (shared.bring_up)(&config_for_up, role, upnp, &limits)
        })
        .await
        .context("起動タスクの実行に失敗しました")??;

        let address = tunnel.spec.address.addr();
        let subnet = tunnel.spec.address.trunc();
        let network = tunnel.network.clone();
        let if_name = tunnel.if_name.clone();
        // 内蔵 DNS(M3-1): トンネル IP の :53 で待受け(準備でき次第 bind)
        let dns_task = tokio::spawn(crate::dns::run_for_tunnel(address, Arc::clone(&self.zones)));
        // Linux のスプリット DNS は per-link 設定(リンク消滅で自動解除)
        if self.manage_os_dns {
            let link = if_name.clone();
            tokio::task::spawn_blocking(move || crate::dnscfg::register_link(&link, address))
                .await
                .ok();
        }
        let (stop_tx, stop_rx) = watch::channel(false);
        let snapshot: SharedSnapshot = Arc::new(Mutex::new(None));
        let transfers: crate::msg::TransferRegistry = Default::default();
        // チャット履歴・グループ情報の読み込み(数 MB 程度の同期 I/O。起動時のみ)
        let chat = crate::chat::ChatLog::load(&config);
        let groups = crate::groups::GroupStore::load(&config);
        let task_snapshot = Arc::clone(&snapshot);
        let task_transfers = Arc::clone(&transfers);
        let task_chat = Arc::clone(&chat);
        let task_groups = Arc::clone(&groups);
        let task_config = config.clone();
        let task = tokio::spawn(async move {
            let mut tunnel = tunnel;
            let supervise_result = tunnel
                .supervise_run(
                    &task_config,
                    stop_rx,
                    Some(task_snapshot),
                    task_transfers,
                    task_chat,
                    task_groups,
                )
                .await;
            // クリーンアップ(ブロッキング)は必ず実行する
            let down_result =
                tokio::task::spawn_blocking(move || tunnel::tear_down(tunnel, &task_config))
                    .await
                    .context("停止タスクの実行に失敗しました")?;
            supervise_result.and(down_result)
        });
        tracing::info!("トンネルを開始しました(network={network})");
        active.insert(
            key,
            Active {
                role,
                config,
                network,
                address,
                subnet,
                if_name,
                stop_tx,
                task,
                dns_task,
                snapshot,
                transfers,
                chat,
                groups,
            },
        );
        drop(active);
        self.sync_os_dns().await;
        Ok(())
    }

    /// OS のスプリット DNS 設定を現在の稼働トンネル一覧に同期する
    /// (Windows の NRPT。Linux は per-link 設定なのでここでは何もしない)。
    async fn sync_os_dns(&self) {
        if !self.manage_os_dns {
            return;
        }
        let mut servers: Vec<Ipv4Addr> = self
            .active
            .lock()
            .await
            .values()
            .map(|a| a.address)
            .collect();
        servers.sort_unstable();
        tokio::task::spawn_blocking(move || crate::dnscfg::apply_servers(&servers))
            .await
            .ok();
    }

    /// 全ネットワーク合算の DNS ゾーンを最新の台帳から作り直す(5 秒周期)。
    async fn refresh_zones(&self) {
        let data: Vec<(
            String,
            Vec<peercove_core::proto::LedgerEntry>,
            Vec<DnsRecord>,
        )> = {
            let active = self.active.lock().await;
            active
                .values()
                .map(|a| {
                    let snapshot = a.snapshot.lock().unwrap();
                    match snapshot.as_ref() {
                        Some(s) => (
                            a.network.clone(),
                            s.ledger.clone().unwrap_or_default(),
                            s.dns_records.clone(),
                        ),
                        None => (a.network.clone(), Vec::new(), Vec::new()),
                    }
                })
                .collect()
        };
        let merged = crate::dns::merge_zones(&data);
        *self.zones.write().unwrap() = merged;
    }

    /// `config` 指定でそのトンネルを、省略時は「1 本だけ稼働中」の場合に
    /// そのトンネルを止める(複数稼働中の省略はエラー — 誤爆防止)。
    async fn stop(self: &Arc<Self>, config: Option<PathBuf>) -> anyhow::Result<()> {
        let active = {
            let mut map = self.active.lock().await;
            match config {
                Some(config) => {
                    let key = Self::key_for(&config);
                    match map.remove(&key) {
                        Some(active) => active,
                        None => bail!("この設定のトンネルは動いていません({})", config.display()),
                    }
                }
                None => {
                    if map.is_empty() {
                        bail!("トンネルは動いていません");
                    }
                    if map.len() > 1 {
                        let running: Vec<String> = map
                            .values()
                            .map(|a| format!("{}({})", a.network, a.config.display()))
                            .collect();
                        bail!(
                            "複数のネットワークが稼働中です。--config で指定してください: {}",
                            running.join(", ")
                        );
                    }
                    let key = map.keys().next().expect("len==1").clone();
                    map.remove(&key).expect("直前に確認済み")
                }
            }
        };
        let network = active.network.clone();
        active.dns_task.abort();
        let _ = active.stop_tx.send(true);
        let stopped = active
            .task
            .await
            .context("トンネルタスクの終了待ちに失敗しました")
            .and_then(|r| r.context("トンネルの停止処理でエラーが発生しました"));
        // DNS 側の後片付け(NRPT の同期)は停止の成否に関わらず行う
        self.sync_os_dns().await;
        stopped?;
        tracing::info!("トンネルを停止しました(network={network})");
        Ok(())
    }

    /// 全トンネルを停止する(shutdown・常駐終了時)。エラーはログに落として続行。
    async fn stop_all(self: &Arc<Self>) {
        let all: Vec<Active> = self.active.lock().await.drain().map(|(_, a)| a).collect();
        for active in all {
            let network = active.network.clone();
            active.dns_task.abort();
            let _ = active.stop_tx.send(true);
            match active.task.await {
                Ok(Ok(())) => tracing::info!("トンネルを停止しました(network={network})"),
                Ok(Err(e)) => {
                    tracing::warn!("トンネル(network={network})の停止でエラー: {e:#}")
                }
                Err(e) => tracing::warn!("トンネルタスクの終了待ちに失敗: {e:#}"),
            }
        }
        self.sync_os_dns().await;
    }

    async fn status(&self) -> DaemonStatus {
        let active = self.active.lock().await;
        let mut tunnels: Vec<TunnelInfo> = active.values().map(tunnel_info).collect();
        // HashMap の順序は不定なので、表示が揺れないよう設定パスで安定させる
        tunnels.sort_by(|a, b| a.config.cmp(&b.config));
        DaemonStatus {
            version: IPC_VERSION,
            tunnels,
        }
    }
}

/// 1 トンネル分の status 応答を組み立てる。
fn tunnel_info(active: &Active) -> TunnelInfo {
    let (peers, ledger, rtt_ms, removed, direct) = {
        let snapshot = active.snapshot.lock().unwrap();
        match snapshot.as_ref() {
            Some(snapshot) => (
                snapshot.peers.clone(),
                snapshot.ledger.clone(),
                snapshot.rtt_ms.clone(),
                snapshot.removed,
                snapshot.direct.clone(),
            ),
            None => (Vec::new(), None, HashMap::new(), false, HashMap::new()),
        }
    };
    let ledger = ledger.unwrap_or_default();
    // RTT は仮想 IP をキーに測っている。台帳が公開鍵 ↔ 仮想 IP を対応づける
    let ip_by_key: HashMap<&[u8; 32], Ipv4Addr> = ledger
        .iter()
        .map(|entry| (entry.public_key.as_bytes(), entry.ip))
        .collect();
    let now = SystemTime::now();
    TunnelInfo {
        config: active.config.clone(),
        network: active.network.clone(),
        role: match active.role {
            Role::Host => TunnelRole::Host,
            Role::Member => TunnelRole::Member,
        },
        address: active.address,
        peers: peers
            .iter()
            .map(|p| PeerSummary {
                public_key: p.public_key,
                endpoint: p.endpoint,
                last_handshake_age_secs: p
                    .last_handshake
                    .and_then(|t| now.duration_since(t).ok())
                    .map(|d| d.as_secs()),
                rx_bytes: p.rx_bytes,
                tx_bytes: p.tx_bytes,
                rtt_ms: ip_by_key
                    .get(p.public_key.as_bytes())
                    .and_then(|ip| rtt_ms.get(ip))
                    .copied(),
            })
            .collect(),
        ledger,
        removed,
        direct,
        // 進捗はレジストリから直接読む(スナップショットの 5 秒周期より新しい)
        transfers: active.transfers.lock().unwrap().clone(),
        chat_seq: active.chat.lock().unwrap().latest_seq(),
        groups: active.groups.lock().unwrap().list(),
    }
}

/// 1 本の IPC 接続を処理する(トランスポート非依存)。
async fn handle_connection<S>(stream: S, shared: Arc<DaemonShared>) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (read_half, mut write_half) = tokio::io::split(stream);
    let mut reader = BufReader::new(read_half).take(MAX_LINE_LEN);
    let mut line = String::new();
    loop {
        // take の上限は reader の累計なので、1 行ごとに戻す。今のクライアントは
        // 1 接続 1 リクエストなので効いていないが、接続を使い回すと上限に達した
        // 時点で EOF と区別できなくなる(control.rs で同じ罠を踏んだ)
        reader.set_limit(MAX_LINE_LEN);
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            return Ok(()); // クライアント切断
        }
        let reply = match serde_json::from_str::<IpcEnvelope>(&line) {
            Ok(envelope) => {
                let result = match shared.dispatch(envelope.req).await {
                    Ok(response) => IpcResult::Ok(response),
                    Err(e) => IpcResult::Err(format!("{e:#}")),
                };
                IpcReply {
                    id: envelope.id,
                    result,
                }
            }
            Err(e) => IpcReply {
                id: 0,
                result: IpcResult::Err(format!("リクエストを解析できません: {e}")),
            },
        };
        let mut json = serde_json::to_string(&reply).expect("IpcReply は常に直列化可能");
        json.push('\n');
        write_half.write_all(json.as_bytes()).await?;
    }
}

// クライアント側(接続・リクエスト送信)は UI と共用するため
// `peercove-ipc` crate にある(ADR-0007)。
pub use peercove_ipc::request;

// ---- サーバー(OS 別トランスポート) ----

/// `daemon run`: IPC サーバーを起動して常駐する(コンソールモード。Ctrl+C で終了)。
pub fn run_server() -> anyhow::Result<()> {
    serve(None)
}

/// IPC サーバー本体。
///
/// `external_stop` は外部からの停止シグナル(Windows サービスの SCM からの
/// Stop 等)。`None` ならコンソールモードとして Ctrl+C を待つ。
/// どちらの場合も IPC の shutdown 要求では終了する。
pub fn serve(external_stop: Option<watch::Receiver<bool>>) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .context("非同期ランタイムの初期化に失敗しました")?;
    let (shared, shutdown_rx) = DaemonShared::new(Box::new(tunnel::bring_up), true);
    // 「開始しました」は待受け開始後(accept_loop 内)に表示する。
    // 先に出すと、パイプ/ソケットの作成に失敗したときに紛らわしいため
    runtime.block_on(async {
        // 前回異常終了の NRPT 残骸を掃除する(Linux は per-link なので残骸なし)
        shared.sync_os_dns().await;
        // DNS ゾーンを台帳の更新(5 秒周期)に合わせて作り直すループ(M3-1)
        let zone_refresher = tokio::spawn({
            let shared = Arc::clone(&shared);
            async move {
                let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
                loop {
                    tick.tick().await;
                    shared.refresh_zones().await;
                }
            }
        });
        let stop_request = async {
            match external_stop {
                Some(rx) => {
                    wait_shutdown(rx).await;
                    Ok(())
                }
                None => tokio::signal::ctrl_c()
                    .await
                    .context("シグナル待機に失敗しました"),
            }
        };
        let result = tokio::select! {
            result = accept_loop(Arc::clone(&shared)) => result,
            result = stop_request => result,
            _ = wait_shutdown(shutdown_rx) => Ok(()),
        };
        zone_refresher.abort();
        result
    })?;
    // 常駐終了時にトンネルが残っていれば必ず片付ける(NRPT の掃除も stop_all 内)
    runtime.block_on(async {
        shared.stop_all().await;
    });
    // UDS のファイルは自動で消えないため、残骸を残さない(Windows のパイプは不要)
    #[cfg(unix)]
    let _ = std::fs::remove_file(peercove_ipc::socket_path());
    println!("peercove デーモンを終了しました");
    Ok(())
}

async fn wait_shutdown(mut rx: watch::Receiver<bool>) {
    while !*rx.borrow() {
        if rx.changed().await.is_err() {
            return;
        }
    }
}

/// 昇格したデーモンが作るパイプへ、非特権の UI/CLI が接続できるようにする
/// セキュリティ記述子(認証済みユーザーへ読み書き許可)。M2 の権限モデル
/// (デーモン = サービス / UI = 非特権)の前提。
#[cfg(windows)]
mod winsec {
    use anyhow::Context;
    use windows_sys::Win32::Foundation::{LocalFree, HLOCAL};
    use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};

    const SDDL_REVISION_1: u32 = 1;

    /// パイプの DACL:
    /// - SYSTEM(SY)と Administrators(BA)にフルアクセス
    /// - 認証済みユーザー(AU)に FILE_GENERIC_READ | FILE_GENERIC_WRITE
    ///   (FW は FILE_APPEND_DATA を含み、= FILE_CREATE_PIPE_INSTANCE)
    ///
    /// ACE に総称権(GA/GR/GW)を書くとオブジェクト固有権へマップされず
    /// アクセス拒否になるため、必ず FR/FW/FA を使うこと。
    const PIPE_SDDL: &str = "D:(A;;FA;;;SY)(A;;FA;;;BA)(A;;FRFW;;;AU)\0";

    /// 上記 DACL を持つセキュリティ記述子。
    pub struct PipeSecurity {
        descriptor: PSECURITY_DESCRIPTOR,
    }

    // 記述子は不変のポインタを保持するだけで、スレッド間で共有しても安全。
    unsafe impl Send for PipeSecurity {}
    unsafe impl Sync for PipeSecurity {}

    impl PipeSecurity {
        pub fn authenticated_users() -> anyhow::Result<Self> {
            let sddl: Vec<u16> = PIPE_SDDL.encode_utf16().collect();
            let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
            // SAFETY: FFI 境界。sddl は null 終端の UTF-16。descriptor は関数側が
            // LocalAlloc で確保し、Drop で LocalFree する
            let ok = unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    SDDL_REVISION_1,
                    &mut descriptor,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(std::io::Error::last_os_error())
                    .context("パイプのセキュリティ記述子の作成に失敗しました");
            }
            Ok(Self { descriptor })
        }

        /// SECURITY_ATTRIBUTES を組み立てる。戻り値は self より長生きさせないこと。
        pub fn attributes(&self) -> SECURITY_ATTRIBUTES {
            SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: self.descriptor,
                bInheritHandle: 0,
            }
        }
    }

    impl Drop for PipeSecurity {
        fn drop(&mut self) {
            // SAFETY: descriptor は Convert... が確保したもののみ
            unsafe {
                LocalFree(self.descriptor as HLOCAL);
            }
        }
    }
}

#[cfg(windows)]
async fn accept_loop(shared: Arc<DaemonShared>) -> anyhow::Result<()> {
    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};

    let security = winsec::PipeSecurity::authenticated_users()?;
    let make = |first: bool| -> anyhow::Result<NamedPipeServer> {
        let mut attrs = security.attributes();
        // SAFETY: attrs は本呼び出し中のみ参照される。指す記述子は security が保持
        unsafe {
            ServerOptions::new()
                .first_pipe_instance(first)
                .create_with_security_attributes_raw(
                    peercove_core::ipc::PIPE_NAME,
                    &mut attrs as *mut _ as *mut std::ffi::c_void,
                )
        }
        .with_context(|| {
            format!(
                "名前付きパイプ {} を作成できません。既に peercove デーモンが\
                 起動していないか確認してください(タスクマネージャーで peercove-poc を確認。\
                 管理者で起動したデーモンは管理者ターミナルからしか終了できません)",
                peercove_core::ipc::PIPE_NAME
            )
        })
    };

    let mut server = make(true)?;
    println!(
        "peercove デーモンを開始しました({} で待受け中。Ctrl+C か shutdown 要求で終了)",
        peercove_core::ipc::PIPE_NAME
    );
    loop {
        server
            .connect()
            .await
            .context("パイプ接続の待受に失敗しました")?;
        let stream = server;
        server = make(false)?;
        let shared = Arc::clone(&shared);
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, shared).await {
                tracing::debug!("IPC 接続が終了: {e:#}");
            }
        });
    }
}

#[cfg(unix)]
async fn accept_loop(shared: Arc<DaemonShared>) -> anyhow::Result<()> {
    let path = peercove_ipc::socket_path();
    let _ = std::fs::remove_file(&path); // 前回異常終了の残骸
    let listener = tokio::net::UnixListener::bind(&path)
        .with_context(|| format!("{} の bind に失敗しました", path.display()))?;
    // root で起動したデーモンのソケットへ、非特権の UI/CLI が接続できるようにする
    // (Windows 側で認証済みユーザーに許可するのと同じ方針。単一ユーザー PC 前提で、
    //  複数ユーザーの権限分離は将来課題 — ADR-0007)
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666))
            .with_context(|| format!("{} のパーミッション設定に失敗しました", path.display()))?;
    }
    println!(
        "peercove デーモンを開始しました({} で待受け中。Ctrl+C か shutdown 要求で終了)",
        path.display()
    );
    loop {
        let (stream, _) = listener
            .accept()
            .await
            .context("UDS の accept に失敗しました")?;
        let shared = Arc::clone(&shared);
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, shared).await {
                tracing::debug!("IPC 接続が終了: {e:#}");
            }
        });
    }
}

/// status 応答を人間向けに表示する。
pub fn print_status(status: &DaemonStatus) {
    if status.tunnels.is_empty() {
        println!("状態: 待機中(トンネルなし)");
        return;
    }
    if status.tunnels.len() > 1 {
        println!("{} 個のネットワークが稼働中:", status.tunnels.len());
    }
    for info in &status.tunnels {
        let role = match info.role {
            TunnelRole::Host => "ホスト",
            TunnelRole::Member => "メンバー",
        };
        println!("ネットワーク {}: {role}として稼働中", info.network);
        println!("  設定: {}", info.config.display());
        println!("  仮想 IP: {}", info.address);
        if !info.ledger.is_empty() {
            println!("  members:");
            for entry in &info.ledger {
                println!(
                    "    {} {}({}){}",
                    if entry.online { "●" } else { "○" },
                    entry.name.as_deref().unwrap_or("(名前なし)"),
                    entry.ip,
                    if entry.is_host { " [host]" } else { "" }
                );
            }
        }
        for peer in &info.peers {
            let handshake = match peer.last_handshake_age_secs {
                Some(age) => format!("{age} 秒前"),
                None => "なし".to_string(),
            };
            let rtt = match peer.rtt_ms {
                Some(ms) => format!(", rtt {ms:.1} ms"),
                None => String::new(),
            };
            println!(
                "  peer {}: handshake {handshake}, rx {} B, tx {} B{rtt}",
                peer.public_key, peer.rx_bytes, peer.tx_bytes
            );
        }
    }
}

/// `daemon logs`: デーモンが保持する直近のログを表示する(M2-G5)。
///
/// `--follow` のときは 1 秒ごとに続きを取りに行く(Ctrl+C で終了)。
pub fn print_logs(follow: bool) -> anyhow::Result<()> {
    let mut after_seq = 0u64;
    loop {
        if let IpcResponse::Logs { lines, dropped } = request(IpcRequest::Logs { after_seq })? {
            if dropped > 0 {
                eprintln!("(バッファから溢れた {dropped} 行は失われました)");
            }
            for line in &lines {
                println!("{}", format_log_line(line));
            }
            if let Some(last) = lines.last() {
                after_seq = last.seq;
            }
        }
        if !follow {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

/// `12:34:56.789 INFO  peercove_poc::daemon: メッセージ`
///
/// 時刻は UTC(デーモンの標準エラー出力に出る `tracing` の既定表記に合わせる)。
fn format_log_line(line: &peercove_core::ipc::LogLine) -> String {
    let secs_of_day = (line.unix_ms / 1000) % 86_400;
    format!(
        "{:02}:{:02}:{:02}.{:03} {:<5} {}: {}",
        secs_of_day / 3600,
        (secs_of_day / 60) % 60,
        secs_of_day % 60,
        line.unix_ms % 1000,
        line.level,
        line.target,
        line.message
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::mock::MockBackend;
    use crate::backend::TunnelSpec;
    use peercove_core::keys::PrivateKey;
    use peercove_ipc::request_over;

    fn test_shared() -> (Arc<DaemonShared>, watch::Receiver<bool>) {
        // manage_os_dns = false: テストで NRPT / resolvectl を触らない
        DaemonShared::new(
            Box::new(|config, role, _upnp, _limits| {
                // 実トンネルの代わりにモックを起動する。複数トンネルのテストが
                // できるよう、アドレスは設定パスから機械的に変える
                let octet = (config
                    .to_string_lossy()
                    .bytes()
                    .fold(0u32, |acc, b| acc.wrapping_add(b as u32))
                    % 200
                    + 1) as u8;
                let spec = TunnelSpec {
                    private_key: PrivateKey::generate(),
                    address: format!("10.99.{octet}.1/24").parse().unwrap(),
                    listen_port: Some(51820),
                    mtu: 1420,
                    forwarding: role == Role::Host,
                    peers: Vec::new(),
                };
                Ok(ActiveTunnel::new_for_test(
                    spec,
                    role,
                    Box::new(MockBackend::default()),
                ))
            }),
            false,
        )
    }

    /// duplex ストリーム越しに start → status → stop → shutdown の一連を流す。
    #[tokio::test]
    async fn ipc_lifecycle_over_duplex() {
        let (shared, mut shutdown_rx) = test_shared();
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let server = tokio::spawn(handle_connection(server_io, Arc::clone(&shared)));
        let mut client = client_io;

        // Idle(トンネルなし)
        let response = request_over(&mut client, 1, &IpcRequest::Status)
            .await
            .unwrap();
        assert_eq!(
            response,
            IpcResponse::Status(DaemonStatus {
                version: IPC_VERSION,
                tunnels: vec![]
            })
        );

        // Start host → tunnels に 1 本(role = host)
        let response = request_over(
            &mut client,
            2,
            &IpcRequest::StartHost {
                config: PathBuf::from("host.toml"),
                upnp: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(response, IpcResponse::Done);
        let response = request_over(&mut client, 3, &IpcRequest::Status)
            .await
            .unwrap();
        match response {
            IpcResponse::Status(status) => {
                assert_eq!(status.tunnels.len(), 1);
                assert_eq!(status.tunnels[0].role, TunnelRole::Host);
                assert_eq!(status.tunnels[0].network, "test");
            }
            other => panic!("Status を期待: {other:?}"),
        }

        // 同じ設定の二重起動は拒否
        let err = request_over(
            &mut client,
            4,
            &IpcRequest::StartHost {
                config: PathBuf::from("host.toml"),
                upnp: false,
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("既に動いています"));

        // Stop(1 本なら config 省略可)→ Idle
        let response = request_over(&mut client, 5, &IpcRequest::Stop { config: None })
            .await
            .unwrap();
        assert_eq!(response, IpcResponse::Done);
        let response = request_over(&mut client, 6, &IpcRequest::Status)
            .await
            .unwrap();
        assert_eq!(
            response,
            IpcResponse::Status(DaemonStatus {
                version: IPC_VERSION,
                tunnels: vec![]
            })
        );

        // Shutdown シグナル
        let response = request_over(&mut client, 7, &IpcRequest::Shutdown)
            .await
            .unwrap();
        assert_eq!(response, IpcResponse::Done);
        shutdown_rx.changed().await.unwrap();
        assert!(*shutdown_rx.borrow());

        drop(client);
        server.await.unwrap().unwrap();
    }

    /// 複数ネットワークの同時稼働(ADR-0012): 2 本張って個別に止める。
    #[tokio::test]
    async fn runs_multiple_tunnels_and_stops_individually() {
        let (shared, _rx) = test_shared();
        // どちらも Host ロール(Member は supervise 開始時に実ファイルが要る)
        shared
            .start(PathBuf::from("a.toml"), Role::Host, false)
            .await
            .unwrap();
        shared
            .start(PathBuf::from("b.toml"), Role::Host, false)
            .await
            .unwrap();

        let status = shared.status().await;
        assert_eq!(status.tunnels.len(), 2);
        assert_eq!(status.tunnels[0].config, PathBuf::from("a.toml"));
        assert_eq!(status.tunnels[0].role, TunnelRole::Host);
        assert_ne!(
            status.tunnels[0].address, status.tunnels[1].address,
            "モックはパスごとに別サブネット"
        );

        // 複数稼働中の config 省略 stop は誤爆防止で拒否
        let err = shared.stop(None).await.unwrap_err();
        assert!(err.to_string().contains("複数のネットワーク"));

        // 個別 stop
        shared.stop(Some(PathBuf::from("a.toml"))).await.unwrap();
        let status = shared.status().await;
        assert_eq!(status.tunnels.len(), 1);
        assert_eq!(status.tunnels[0].config, PathBuf::from("b.toml"));

        // 残り 1 本なら省略で止められる
        shared.stop(None).await.unwrap();
        assert!(shared.status().await.tunnels.is_empty());

        // 何も無い状態の stop はエラー
        assert!(shared.stop(None).await.is_err());
    }

    /// stop_all は全トンネルを止める(shutdown・常駐終了時の後片付け)。
    #[tokio::test]
    async fn stop_all_clears_every_tunnel() {
        let (shared, _rx) = test_shared();
        shared
            .start(PathBuf::from("a.toml"), Role::Host, false)
            .await
            .unwrap();
        shared
            .start(PathBuf::from("b.toml"), Role::Host, false)
            .await
            .unwrap();
        shared.stop_all().await;
        assert!(shared.status().await.tunnels.is_empty());
    }

    /// パイプに付けたセキュリティ記述子で、クライアントが接続できること。
    /// (総称権 GA を ACE に書くとここで access denied になる。FR/FW が必要)
    #[cfg(windows)]
    #[tokio::test]
    async fn pipe_security_descriptor_allows_client_connect() {
        use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};

        let name = format!(r"\\.\pipe\peercove-sdtest-{}", std::process::id());
        let security = winsec::PipeSecurity::authenticated_users().expect("記述子の作成");
        let mut attrs = security.attributes();
        // SAFETY: attrs は本呼び出し中のみ参照される
        let server = unsafe {
            ServerOptions::new()
                .first_pipe_instance(true)
                .create_with_security_attributes_raw(
                    &name,
                    &mut attrs as *mut _ as *mut std::ffi::c_void,
                )
        }
        .expect("パイプの作成");

        let accept = tokio::spawn(async move {
            server.connect().await.expect("接続の受理");
            server
        });
        let client = ClientOptions::new()
            .open(&name)
            .expect("クライアントからの接続");
        let server = accept.await.unwrap();
        drop(client);
        drop(server);
    }

    /// Logs 要求は `after_seq` より後の行だけを返す(UI のポーリング用)。
    #[tokio::test]
    async fn logs_return_only_new_lines() {
        let (shared, _rx) = test_shared();
        let ring = crate::logbuf::ring();
        let logs = |after_seq| {
            let shared = Arc::clone(&shared);
            async move {
                match shared
                    .dispatch(IpcRequest::Logs { after_seq })
                    .await
                    .unwrap()
                {
                    IpcResponse::Logs { lines, dropped } => (lines, dropped),
                    other => panic!("Logs を期待: {other:?}"),
                }
            }
        };

        // 他のテストが積んだ行と混ざらないよう、いまの末尾から見る
        let after_seq = ring.since(0).0.last().map(|line| line.seq).unwrap_or(0);
        assert!(logs(after_seq).await.0.is_empty(), "新しい行はまだ無い");

        ring.push("INFO", "peercove_poc::test", "テスト行".to_string());
        let (lines, dropped) = logs(after_seq).await;
        assert_eq!(dropped, 0);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].message, "テスト行");
        assert!(lines[0].seq > after_seq);

        // 取り込んだ続きからは、また空になる
        assert!(logs(lines[0].seq).await.0.is_empty());
    }

    /// stop 時に MockBackend の down が呼ばれる(クリーンアップの対称性)。
    #[tokio::test]
    async fn stop_tears_down_backend() {
        let ops: Arc<Mutex<Vec<String>>> = Default::default();
        let ops_for_factory = Arc::clone(&ops);
        let (shared, _rx) = DaemonShared::new(
            Box::new(move |_config, role, _upnp, _limits| {
                let spec = TunnelSpec {
                    private_key: PrivateKey::generate(),
                    address: "10.99.0.2/24".parse().unwrap(),
                    listen_port: None,
                    mtu: 1420,
                    forwarding: false,
                    peers: Vec::new(),
                };
                Ok(ActiveTunnel::new_for_test(
                    spec,
                    role,
                    Box::new(MockBackend::with_shared_ops(Arc::clone(&ops_for_factory))),
                ))
            }),
            false,
        );
        // Host ロールにする(Member は supervise 開始時に設定を読むため実ファイルが要る)
        shared
            .start(PathBuf::from("h.toml"), Role::Host, false)
            .await
            .unwrap();
        shared.stop(None).await.unwrap();
        let ops = ops.lock().unwrap();
        assert!(
            ops.contains(&"down".to_string()),
            "down が呼ばれる: {ops:?}"
        );
    }
}
