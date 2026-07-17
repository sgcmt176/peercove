//! デーモン(M2-G1a、ADR-0007)。
//!
//! `peercove daemon run` で常駐し、ローカル IPC(Windows: 名前付きパイプ /
//! Linux: Unix ドメインソケット)でトンネルの開始・停止・状態取得を受け付ける。
//! 招待・削除などの設定ファイル操作は IPC に乗せない(UI/CLI が直接行い、
//! 実行中トンネルは 5 秒再読込で追随する)。
//!
//! トランスポート非依存の部分(`handle_connection` / `request_over`)は
//! 任意の AsyncRead+AsyncWrite で動き、テストは `tokio::io::duplex` で行う。

use std::collections::{BTreeMap, HashMap};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use anyhow::{bail, Context};
use peercove_core::diagnostics::{
    redact_log_line, DiagnosticCategory, DiagnosticCheck, DiagnosticReport, DiagnosticScope,
    DiagnosticStatus,
};
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
    /// (member)鍵ローテーションの手動要求(ADR-0020、M3-11)。IPC が立て、
    /// supervise 内の状態機械が次の周期で拾う
    rotate_request: Arc<std::sync::atomic::AtomicBool>,
    /// (member)制御接続への差し込み口(ADR-0021)。IPC の SetDnsName が
    /// ここからホストへ依頼を送り、応答を待つ
    member_link: Arc<crate::control::MemberLink>,
    /// 1 分粒度の端末ローカル通信品質履歴(M3-23)。
    quality: crate::quality::SharedQuality,
    /// DNS サービスの直近ヘルス状態と手動再確認要求(M3-14e)。
    health: crate::health::SharedHealth,
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
            IpcRequest::Diagnose { config } => Ok(IpcResponse::Diagnostic {
                report: self.diagnose(config).await,
            }),
            IpcRequest::Quality {
                config,
                since_unix_ms,
            } => {
                let key = Self::key_for(&config);
                let active = self.active.lock().await;
                let tunnel = active
                    .get(&key)
                    .ok_or_else(|| anyhow::anyhow!("このネットワークは稼働していません"))?;
                let report = tunnel.quality.lock().unwrap().report(since_unix_ms);
                Ok(IpcResponse::Quality { report })
            }
            IpcRequest::CheckDnsHealth { config } => {
                let key = Self::key_for(&config);
                let active = self.active.lock().await;
                let tunnel = active
                    .get(&key)
                    .ok_or_else(|| anyhow::anyhow!("このネットワークは稼働していません"))?;
                if tunnel.role != Role::Host {
                    bail!("サービスの確認はホストでだけ実行できます");
                }
                tunnel.health.lock().unwrap().request_now();
                Ok(IpcResponse::Done)
            }
            IpcRequest::SendFile {
                config,
                peer,
                path,
                chat,
            } => {
                let id = self.send_file(config, peer, path, chat).await?;
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
            IpcRequest::RotateKey { config } => {
                let active = self.active.lock().await;
                let active = active.get(&Self::key_for(&config)).with_context(|| {
                    format!("この設定のトンネルは動いていません({})", config.display())
                })?;
                if active.role != Role::Member {
                    bail!(
                        "鍵の更新はメンバーとして参加しているネットワークでのみ実行できます\
                        (ホスト鍵の更新は未対応 — ADR-0020)"
                    );
                }
                active
                    .rotate_request
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                tracing::info!("鍵の更新要求を受け付けました(network={})", active.network);
                Ok(IpcResponse::Done)
            }
            IpcRequest::SetDnsName { config, name } => {
                // (member)DNS 名の変更依頼(ADR-0021)。ホストへ送って
                // 検証・適用の結果を待つ。ロックは送信前に手放す
                let (link, network) = {
                    let active = self.active.lock().await;
                    let active = active.get(&Self::key_for(&config)).with_context(|| {
                        format!("この設定のトンネルは動いていません({})", config.display())
                    })?;
                    if active.role != Role::Member {
                        bail!(
                            "この操作はメンバーとして参加しているネットワーク用です\
                            (ホストの DNS 名は設定画面から変更できます)"
                        );
                    }
                    (Arc::clone(&active.member_link), active.network.clone())
                };
                let reply = link
                    .request_dns_name(name)
                    .context("ホストに接続していません(接続が確立してからやり直してください)")?;
                match tokio::time::timeout(std::time::Duration::from_secs(10), reply).await {
                    Ok(Ok((true, message))) => {
                        tracing::info!("DNS 名を変更しました(network={network}): {message}");
                        Ok(IpcResponse::Done)
                    }
                    Ok(Ok((false, message))) => bail!("{message}"),
                    Ok(Err(_)) => bail!("ホストとの接続が切れました。やり直してください"),
                    Err(_) => bail!(
                        "ホストから応答がありません(ホストのバージョンが古い可能性があります)"
                    ),
                }
            }
            IpcRequest::SetDisplayName { config, name } => {
                // (member)表示名の変更依頼(ADR-0027)。DNS 名変更と同じ経路で
                // ホストへ送り、検証・適用の結果を待つ。ロックは送信前に手放す
                let (link, network) = {
                    let active = self.active.lock().await;
                    let active = active.get(&Self::key_for(&config)).with_context(|| {
                        format!("この設定のトンネルは動いていません({})", config.display())
                    })?;
                    if active.role != Role::Member {
                        bail!(
                            "この操作はメンバーとして参加しているネットワーク用です\
                            (ホスト自身の表示名はメンバー一覧の自分の行から変更できます)"
                        );
                    }
                    (Arc::clone(&active.member_link), active.network.clone())
                };
                let reply = link
                    .request_display_name(name)
                    .context("ホストに接続していません(接続が確立してからやり直してください)")?;
                match tokio::time::timeout(std::time::Duration::from_secs(10), reply).await {
                    Ok(Ok((true, message))) => {
                        tracing::info!("表示名を変更しました(network={network}): {message}");
                        Ok(IpcResponse::Done)
                    }
                    Ok(Ok((false, message))) => bail!("{message}"),
                    Ok(Err(_)) => bail!("ホストとの接続が切れました。やり直してください"),
                    Err(_) => bail!(
                        "ホストから応答がありません(ホストのバージョンが古い可能性があります)"
                    ),
                }
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
            file: None,
            system: false,
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
        let applied = active.groups.lock().unwrap().apply(group.clone());
        if let Some(update) = applied {
            Self::append_group_system(active, &ledger, update.previous.as_ref(), &group);
        }
        // グループ名はログに出さない(秘匿ルール)
        tracing::info!(
            "グループを作成しました(id={} members={})",
            group.id,
            group.members.len()
        );
        Self::propagate_group(&group, &ledger, active.address, &active.groups);
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
        let applied = active.groups.lock().unwrap().apply(group.clone());
        if let Some(update) = applied {
            Self::append_group_system(active, &ledger, update.previous.as_ref(), &group);
        }
        tracing::info!(
            "グループを更新しました(id={} rev={})",
            group.id,
            group.revision
        );
        Self::propagate_group(&group, &ledger, active.address, &active.groups);
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
        let ledger = Self::ledger_of(active);
        let applied = active.groups.lock().unwrap().apply(group.clone());
        if let Some(update) = applied {
            Self::append_group_system(active, &ledger, update.previous.as_ref(), &group);
        }
        tracing::info!("グループから退出しました(id={})", group.id);
        Self::propagate_group(&group, &ledger, active.address, &active.groups);
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
    /// 成功は ack として記録し、失敗は supervise の送達同期
    /// (30 秒間隔の再送)に任せる。
    fn propagate_group(
        group: &peercove_core::msg::GroupInfo,
        ledger: &[peercove_core::proto::LedgerEntry],
        self_ip: Ipv4Addr,
        groups: &crate::groups::SharedGroups,
    ) {
        for entry in ledger {
            if entry.ip == self_ip || !entry.online || !group.members.contains(&entry.ip) {
                continue;
            }
            let group = group.clone();
            let ip = entry.ip;
            let groups = Arc::clone(groups);
            tokio::spawn(async move {
                match crate::msg::send_group_update(ip, &group).await {
                    Ok(()) => groups
                        .lock()
                        .unwrap()
                        .mark_acked(&group.id, ip, group.revision),
                    Err(e) => tracing::warn!(
                        "{ip} へのグループ更新の送信に失敗しました(30 秒間隔で自動再送): {e:#}"
                    ),
                }
            });
        }
    }

    /// グループ操作(作成・追加・退出・改名)のお知らせを会話に出す
    /// (LINE 風 — 2026-07-11 検証フィードバック)。
    fn append_group_system(
        active: &Active,
        ledger: &[peercove_core::proto::LedgerEntry],
        previous: Option<&peercove_core::msg::GroupInfo>,
        group: &peercove_core::msg::GroupInfo,
    ) {
        let self_ip = active.address;
        let name_of = |ip: Ipv4Addr| -> String {
            if ip == self_ip {
                return "自分".to_string();
            }
            ledger
                .iter()
                .find(|e| e.ip == ip)
                .and_then(|e| e.name.clone())
                .unwrap_or_else(|| ip.to_string())
        };
        for text in crate::groups::system_messages(previous, group, self_ip, &name_of) {
            active.chat.lock().unwrap().append(ChatMessageInfo {
                seq: 0, // append が振る
                id: crate::msg::new_transfer_id(),
                scope: peercove_core::msg::ChatScope::Group,
                group_id: Some(group.id.clone()),
                from: group.updated_by,
                to: None,
                text,
                sent_at: crate::msg::now_unix_ms(),
                failed: false,
                file: None,
                system: true,
            });
        }
    }

    /// メンバーへのファイル送信を開始する(ADR-0015、M3-9)。送信自体は
    /// バックグラウンドタスクで走り、進捗は status 応答の transfers で追う。
    /// チャット文脈付き(M3-13d)なら履歴にファイルのエントリを記録し、
    /// network / group 宛は対象メンバーへの個別転送になる(履歴は 1 エントリ)。
    async fn send_file(
        &self,
        config: PathBuf,
        peer: Option<Ipv4Addr>,
        path: PathBuf,
        chat_ctx: Option<peercove_core::msg::ChatContext>,
    ) -> anyhow::Result<String> {
        use peercove_core::ipc::ChatFileInfo;
        use peercove_core::msg::ChatScope;

        let active = self.active.lock().await;
        let active = active
            .get(&Self::key_for(&config))
            .with_context(|| format!("この設定のトンネルは動いていません({})", config.display()))?;
        let ledger = Self::ledger_of(active);
        let scope = chat_ctx
            .as_ref()
            .map(|c| c.scope)
            .unwrap_or(ChatScope::Direct);
        // 宛先の決定(チャットと同じ規則。オフライン宛は V1 非対応)
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
            ChatScope::Network => ledger
                .iter()
                .filter(|e| e.ip != active.address && e.online)
                .map(|e| e.ip)
                .collect(),
            ChatScope::Group => {
                let group_id = chat_ctx
                    .as_ref()
                    .and_then(|c| c.group_id.as_deref())
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
        if !path.is_file() {
            bail!(
                "{} が見つからないか、ファイルではありません",
                path.display()
            );
        }
        // 宛先ごとに 1 転送(TransferInfo の id は一意)。履歴には 1 エントリ
        let ids: Vec<String> = targets
            .iter()
            .map(|_| crate::msg::new_transfer_id())
            .collect();
        let chat_entry_seq = match &chat_ctx {
            Some(ctx) => {
                let name = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .context("ファイル名を取得できません")?
                    .to_string();
                let size = std::fs::metadata(&path)
                    .with_context(|| format!("{} を読めません", path.display()))?
                    .len();
                let entry = active.chat.lock().unwrap().append(ChatMessageInfo {
                    seq: 0, // append が振る
                    id: crate::msg::new_transfer_id(),
                    scope,
                    group_id: ctx.group_id.clone(),
                    from: active.address,
                    to: match scope {
                        ChatScope::Direct => peer,
                        ChatScope::Network | ChatScope::Group => None,
                    },
                    text: String::new(),
                    sent_at: crate::msg::now_unix_ms(),
                    failed: false,
                    file: Some(ChatFileInfo {
                        name,
                        size,
                        transfers: ids.clone(),
                        // UI のインラインプレビュー用(送信側は元ファイル)
                        path: Some(path.clone()),
                    }),
                    system: false,
                });
                Some(entry.seq)
            }
            None => None,
        };
        let first_id = ids
            .first()
            .cloned()
            .unwrap_or_else(crate::msg::new_transfer_id);
        let transfers = Arc::clone(&active.transfers);
        let chat_log = Arc::clone(&active.chat);
        tokio::spawn(async move {
            let mut delivered = targets.is_empty(); // 宛先ゼロの全体/グループ宛は失敗扱いにしない
            let sends: Vec<_> = targets
                .into_iter()
                .zip(ids)
                .map(|(target, id)| {
                    let path = path.clone();
                    let transfers = Arc::clone(&transfers);
                    let ctx = chat_ctx.clone();
                    tokio::spawn(async move {
                        // ファイル名はログに出さない(秘匿ルール)。進捗一覧にはエラーが載る
                        match crate::msg::send_file(target, &path, transfers, id, ctx).await {
                            Ok(()) => true,
                            Err(e) => {
                                tracing::warn!("{target} へのファイル送信に失敗しました: {e:#}");
                                false
                            }
                        }
                    })
                })
                .collect();
            for send in sends {
                delivered |= send.await.unwrap_or(false);
            }
            if !delivered {
                if let Some(seq) = chat_entry_seq {
                    chat_log.lock().unwrap().mark_failed(seq);
                }
            }
        });
        Ok(first_id)
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
        let rotate_request: Arc<std::sync::atomic::AtomicBool> = Default::default();
        // (member)制御接続への差し込み口(ADR-0020/0021)。supervisor の
        // 入れ直しをまたいで共有し、IPC の SetDnsName がここから送る
        let member_link: Arc<crate::control::MemberLink> = Default::default();
        // チャット履歴・グループ情報の読み込み(数 MB 程度の同期 I/O。起動時のみ)
        let chat = crate::chat::ChatLog::load(&config);
        let groups = crate::groups::GroupStore::load(&config);
        let quality = crate::quality::QualityStore::load(&config);
        let health: crate::health::SharedHealth = Default::default();
        let task_snapshot = Arc::clone(&snapshot);
        let task_transfers = Arc::clone(&transfers);
        let task_chat = Arc::clone(&chat);
        let task_groups = Arc::clone(&groups);
        let task_rotate = Arc::clone(&rotate_request);
        let task_link = Arc::clone(&member_link);
        let task_quality = Arc::clone(&quality);
        let task_health = Arc::clone(&health);
        let task_config = config.clone();
        // Linux のスプリット DNS は per-link 設定のため、鍵ローテーションの
        // 入れ直し(インターフェース再作成)後に付け直す(ADR-0020)
        let task_manage_dns = self.manage_os_dns;
        let task_if_name = if_name.clone();
        let task = tokio::spawn(async move {
            let mut tunnel = tunnel;
            // 鍵ローテーション(ADR-0020)の入れ直しで supervisor を回し直す
            let supervise_result = loop {
                let result = tunnel
                    .supervise_run(
                        &task_config,
                        stop_rx.clone(),
                        Some(Arc::clone(&task_snapshot)),
                        Arc::clone(&task_transfers),
                        Arc::clone(&task_chat),
                        Arc::clone(&task_groups),
                        Arc::clone(&task_rotate),
                        Arc::clone(&task_link),
                        Arc::clone(&task_quality),
                        Arc::clone(&task_health),
                    )
                    .await;
                match result {
                    Ok(tunnel::SuperviseExit::Restart { use_pending }) => {
                        let restart_config = task_config.clone();
                        let (returned, restarted) = tokio::task::spawn_blocking(move || {
                            let mut tunnel = tunnel;
                            let result = tunnel.restart_in_place(&restart_config, use_pending);
                            (tunnel, result)
                        })
                        .await
                        .context("入れ直しタスクの実行に失敗しました")?;
                        tunnel = returned;
                        match restarted {
                            Ok(()) => {
                                // 入れ直し中に停止要求が来ていたら、次の supervisor へ
                                // 入らない(clone した watch は既送の値を「見た」扱いに
                                // するため、changed() では拾えない)
                                if *stop_rx.borrow() {
                                    break Ok(());
                                }
                                if task_manage_dns {
                                    let link = task_if_name.clone();
                                    tokio::task::spawn_blocking(move || {
                                        crate::dnscfg::register_link(&link, address)
                                    })
                                    .await
                                    .ok();
                                }
                            }
                            Err(e) => {
                                break Err(e).context("鍵ローテーション後の入れ直しに失敗しました")
                            }
                        }
                    }
                    other => break other.map(|_| ()),
                }
            };
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
                rotate_request,
                member_link,
                quality,
                health,
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
        let data: Vec<crate::dns::NetworkZoneData> = {
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
                            s.cname_records.clone(),
                        ),
                        None => (a.network.clone(), Vec::new(), Vec::new(), Vec::new()),
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
        active.health.lock().unwrap().stop();
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
            active.health.lock().unwrap().stop();
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
            app_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            tunnels,
        }
    }

    /// 現在状態と設定ファイルを読むだけの診断。OS やトンネルは変更しない。
    async fn diagnose(&self, config_path: PathBuf) -> DiagnosticReport {
        let status = self.status().await;
        let key = Self::key_for(&config_path);
        let tunnel = status.tunnels.iter().find(|tunnel| tunnel.config == key);
        let loaded = peercove_core::config::Config::load(&config_path);
        let mut checks = Vec::new();

        checks.push(diagnostic_check(
            "app.ipc_compatible",
            DiagnosticCategory::App,
            if status.version == IPC_VERSION {
                DiagnosticStatus::Pass
            } else {
                DiagnosticStatus::Fail
            },
            [("ipc_version", status.version.to_string())],
        ));
        checks.push(diagnostic_check(
            "app.version_known",
            DiagnosticCategory::App,
            if status.app_version.is_some() {
                DiagnosticStatus::Pass
            } else {
                DiagnosticStatus::Warning
            },
            status
                .app_version
                .as_ref()
                .map(|version| [("version", version.clone())].into_iter())
                .into_iter()
                .flatten(),
        ));

        match &loaded {
            Ok(config) => {
                checks.push(diagnostic_check(
                    "config.valid",
                    DiagnosticCategory::App,
                    DiagnosticStatus::Pass,
                    [("mtu", config.interface.mtu.to_string())],
                ));
                let mut missing = Vec::new();
                let mut insecure = Vec::new();
                let mut permissions_unknown = false;
                let secret_paths = std::iter::once(&config.interface.private_key_file).chain(
                    config
                        .peers
                        .iter()
                        .filter_map(|peer| peer.preshared_key_file.as_ref()),
                );
                for path in secret_paths {
                    match std::fs::metadata(path) {
                        Ok(metadata) => match secret_permissions_are_private(&metadata) {
                            Some(false) => insecure.push(mask_path(path)),
                            None => permissions_unknown = true,
                            Some(true) => {}
                        },
                        Err(_) => missing.push(mask_path(path)),
                    }
                }
                let secret_status = if !missing.is_empty() {
                    DiagnosticStatus::Fail
                } else if !insecure.is_empty() {
                    DiagnosticStatus::Warning
                } else if permissions_unknown {
                    DiagnosticStatus::Unknown
                } else {
                    DiagnosticStatus::Pass
                };
                checks.push(diagnostic_check(
                    "permissions.secret_files",
                    DiagnosticCategory::Permissions,
                    secret_status,
                    [
                        ("missing", missing.join(", ")),
                        ("insecure", insecure.join(", ")),
                        (
                            "permission_check",
                            if permissions_unknown {
                                "not_available"
                            } else {
                                secret_permission_check_label()
                            }
                            .to_string(),
                        ),
                    ]
                    .into_iter()
                    .filter(|(_, value)| !value.is_empty()),
                ));
            }
            Err(error) => checks.push(diagnostic_check(
                "config.valid",
                DiagnosticCategory::App,
                DiagnosticStatus::Fail,
                [("error", error.to_string())],
            )),
        }

        checks.push(diagnostic_check(
            "tunnel.running",
            DiagnosticCategory::Tunnel,
            if tunnel.is_some() {
                DiagnosticStatus::Pass
            } else {
                DiagnosticStatus::Fail
            },
            tunnel
                .map(|value| [("interface", value.interface_name.clone())].into_iter())
                .into_iter()
                .flatten(),
        ));

        if let Some(tunnel) = tunnel {
            checks.push(diagnostic_check(
                "tunnel.interface_ready",
                DiagnosticCategory::Tunnel,
                if tunnel.interface_name.is_empty() {
                    DiagnosticStatus::Warning
                } else {
                    DiagnosticStatus::Pass
                },
                [
                    ("interface", tunnel.interface_name.clone()),
                    ("address", tunnel.address.to_string()),
                ],
            ));
            let handshake_count = tunnel
                .peers
                .iter()
                .filter(|peer| peer.last_handshake_age_secs.is_some())
                .count();
            checks.push(diagnostic_check(
                "tunnel.handshake",
                DiagnosticCategory::Tunnel,
                if tunnel.peers.is_empty() {
                    DiagnosticStatus::Unknown
                } else if handshake_count > 0 {
                    DiagnosticStatus::Pass
                } else {
                    DiagnosticStatus::Warning
                },
                [
                    ("peers", tunnel.peers.len().to_string()),
                    ("with_handshake", handshake_count.to_string()),
                ],
            ));
            checks.push(diagnostic_check(
                "internet.reachability_evidence",
                DiagnosticCategory::Internet,
                if handshake_count > 0 {
                    DiagnosticStatus::Pass
                } else {
                    DiagnosticStatus::Unknown
                },
                [("handshakes", handshake_count.to_string())],
            ));
            checks.push(diagnostic_check(
                "dns.zone_available",
                DiagnosticCategory::Dns,
                if tunnel.ledger.is_empty() {
                    DiagnosticStatus::Warning
                } else {
                    DiagnosticStatus::Pass
                },
                [
                    ("members", tunnel.ledger.len().to_string()),
                    (
                        "custom_records",
                        (tunnel.dns_records.len() + tunnel.cname_records.len()).to_string(),
                    ),
                ],
            ));
            let unknown_versions = tunnel
                .ledger
                .iter()
                .filter(|member| member.online && member.app_version.is_none())
                .count();
            checks.push(diagnostic_check(
                "app.peer_compatibility",
                DiagnosticCategory::App,
                if unknown_versions == 0 {
                    DiagnosticStatus::Pass
                } else {
                    DiagnosticStatus::Warning
                },
                [("unknown_versions", unknown_versions.to_string())],
            ));
            if tunnel.removed {
                checks.push(diagnostic_check(
                    "tunnel.host_removed_member",
                    DiagnosticCategory::Tunnel,
                    DiagnosticStatus::Fail,
                    std::iter::empty::<(&str, String)>(),
                ));
            }
            let blocked = tunnel.ledger.iter().filter(|member| member.blocked).count();
            let forced = tunnel
                .ledger
                .iter()
                .filter(|member| member.force_relay)
                .count();
            let mut rule_ids: Vec<_> = tunnel
                .ledger
                .iter()
                .filter_map(|member| member.acl_rule_id.clone())
                .collect();
            rule_ids.sort();
            rule_ids.dedup();
            checks.push(diagnostic_check(
                "tunnel.acl",
                DiagnosticCategory::Tunnel,
                if blocked == 0 && forced == 0 {
                    DiagnosticStatus::Pass
                } else {
                    DiagnosticStatus::Warning
                },
                [
                    ("blocked_members", blocked.to_string()),
                    ("force_relay_members", forced.to_string()),
                    ("acl_rule_ids", rule_ids.join(",")),
                ],
            ));
        }

        let (logs, _) = crate::logbuf::ring().since(0);
        let logs = logs.iter().map(redact_log_line).collect();
        let overall = DiagnosticReport::calculate_overall(&checks);
        let config = loaded.as_ref().ok();
        DiagnosticReport {
            generated_at_unix_ms: unix_ms(),
            scope: DiagnosticScope {
                config: config_path.display().to_string(),
                network: tunnel
                    .map(|value| value.network.clone())
                    .or_else(|| config.map(|value| value.network_name().to_string())),
                role: tunnel.map(|value| match value.role {
                    TunnelRole::Host => "host".to_string(),
                    TunnelRole::Member => "member".to_string(),
                }),
            },
            overall,
            checks,
            logs,
        }
    }
}

fn diagnostic_check<K, V>(
    id: &str,
    category: DiagnosticCategory,
    status: DiagnosticStatus,
    evidence: impl IntoIterator<Item = (K, V)>,
) -> DiagnosticCheck
where
    K: Into<String>,
    V: Into<String>,
{
    DiagnosticCheck {
        id: id.to_string(),
        category,
        status,
        evidence: evidence
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect::<BTreeMap<_, _>>(),
    }
}

fn mask_path(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("<secret-file>")
        .to_string()
}

#[cfg(unix)]
fn secret_permissions_are_private(metadata: &std::fs::Metadata) -> Option<bool> {
    use std::os::unix::fs::PermissionsExt;
    Some(metadata.permissions().mode() & 0o077 == 0)
}

#[cfg(windows)]
fn secret_permissions_are_private(_metadata: &std::fs::Metadata) -> Option<bool> {
    // Windows の秘密ファイルは peercove-ops::secret::write_secret が作成時に
    // 継承を外し「現在のユーザー + SYSTEM」のみに ACL を制限する。ここでは
    // ファイルの存在を確認できれば、その管理契約を満たす正常状態として扱う。
    Some(true)
}

#[cfg(unix)]
fn secret_permission_check_label() -> &'static str {
    "mode_bits_verified"
}

#[cfg(windows)]
fn secret_permission_check_label() -> &'static str {
    "peercove_acl_managed"
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// 1 トンネル分の status 応答を組み立てる。
fn tunnel_info(active: &Active) -> TunnelInfo {
    let (peers, ledger, dns_records, cname_records, rtt_ms, removed, connection_error, direct) = {
        let snapshot = active.snapshot.lock().unwrap();
        match snapshot.as_ref() {
            Some(snapshot) => (
                snapshot.peers.clone(),
                snapshot.ledger.clone(),
                snapshot.dns_records.clone(),
                snapshot.cname_records.clone(),
                snapshot.rtt_ms.clone(),
                snapshot.removed,
                snapshot.connection_error.clone(),
                snapshot.direct.clone(),
            ),
            None => (
                Vec::new(),
                None,
                Vec::new(),
                Vec::new(),
                HashMap::new(),
                false,
                None,
                HashMap::new(),
            ),
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
        interface_name: active.if_name.clone(),
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
        dns_records,
        cname_records,
        removed,
        connection_error,
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

    /// 操作を許可するユーザーの SID(`PEERCOVE_OWNER_SID`、インストーラが付与)を
    /// 返す。未設定や不正な形式なら `None`(呼び出し側で従来動作へフォールバック)。
    /// 文字列を SDDL に埋めるため、SID 形式(`S-1-` + 数字とハイフンのみ)を厳格に
    /// 検証してインジェクションを防ぐ(ADR-0038)。
    pub fn owner_sid_from_env() -> Option<String> {
        let raw = std::env::var("PEERCOVE_OWNER_SID").ok()?;
        let sid = raw.trim();
        if is_valid_sid(sid) {
            Some(sid.to_string())
        } else {
            None
        }
    }

    /// SID 文字列の厳格な検証。`S-1-` で始まり、以降は数字とハイフンのみ。
    pub fn is_valid_sid(sid: &str) -> bool {
        sid.len() >= 5
            && sid.len() <= 187 // SID 文字列の理論上限に十分な余裕
            && sid.starts_with("S-1-")
            && sid[4..].bytes().all(|b| b.is_ascii_digit() || b == b'-')
    }

    /// パイプの DACL を作る(ADR-0038):
    /// - SYSTEM(SY)と Administrators(BA)にフルアクセス
    /// - 所有者に FILE_GENERIC_READ | FILE_GENERIC_WRITE
    ///   (FW は FILE_APPEND_DATA を含み、= FILE_CREATE_PIPE_INSTANCE)
    ///
    /// `owner` に検証済み SID を渡すとそのユーザーのみに絞る。`None` なら従来どおり
    /// 認証済みユーザー(AU)へ開く(後方互換)。ACE に総称権(GA/GR/GW)を書くと
    /// オブジェクト固有権へマップされずアクセス拒否になるため、必ず FR/FW/FA を使う。
    pub fn pipe_sddl(owner: Option<&str>) -> String {
        let principal = match owner {
            Some(sid) if is_valid_sid(sid) => sid,
            _ => "AU",
        };
        format!("D:(A;;FA;;;SY)(A;;FA;;;BA)(A;;FRFW;;;{principal})")
    }

    /// 上記 DACL を持つセキュリティ記述子。
    pub struct PipeSecurity {
        descriptor: PSECURITY_DESCRIPTOR,
    }

    // 記述子は不変のポインタを保持するだけで、スレッド間で共有しても安全。
    unsafe impl Send for PipeSecurity {}
    unsafe impl Sync for PipeSecurity {}

    impl PipeSecurity {
        /// 所有者 SID が分かればそのユーザーのみ、分からなければ認証済みユーザーへ
        /// 開くセキュリティ記述子を作る。
        pub fn for_owner(owner: Option<&str>) -> anyhow::Result<Self> {
            let sddl: Vec<u16> = format!("{}\0", pipe_sddl(owner)).encode_utf16().collect();
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

    // 操作を許可するユーザーの SID(インストーラが PEERCOVE_OWNER_SID で渡す)が
    // 分かればそのユーザーのみに、分からなければ従来どおり全認証済みユーザーに
    // パイプを開く(ADR-0038)。
    let owner = winsec::owner_sid_from_env();
    if owner.is_none() {
        tracing::warn!(
            "IPC パイプの所有者(PEERCOVE_OWNER_SID)が未設定のため、全認証済みユーザーに \
             開放します。共用 PC ではインストーラ経由(所有者 SID の設定)を推奨します(ADR-0038)"
        );
    }
    let security = winsec::PipeSecurity::for_owner(owner.as_deref())?;
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
                 起動していないか確認してください(タスクマネージャーで peercove を確認。\
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

/// Unix: このデーモンを操作してよい「所有者 uid」を決める(ADR-0038)。
///
/// - `PEERCOVE_OWNER_UID`(systemd ユニット / インストーラが設定)を最優先。
/// - なければ `SUDO_UID`(`sudo peercove daemon run` で自動的に入る)。
///
/// どちらも無い場合は `None`。所有者不明 = 後方互換で全 uid を許可しつつ警告する
/// (ソース版の素の root 実行など。共用機では非推奨)。
#[cfg(unix)]
fn authorized_owner_uid() -> Option<u32> {
    for key in ["PEERCOVE_OWNER_UID", "SUDO_UID"] {
        if let Ok(value) = std::env::var(key) {
            if let Ok(uid) = value.trim().parse::<u32>() {
                if uid != 0 {
                    return Some(uid);
                }
            }
        }
    }
    None
}

/// Unix: 接続元 uid を認可してよいか(ADR-0038)。root(uid 0)は常に許可。
/// 所有者が判明していればその uid のみ、判明していなければ後方互換で全て許可する
/// (呼び出し側が警告を出す)。
#[cfg(unix)]
fn authorize_peer(owner: Option<u32>, peer_uid: u32) -> bool {
    peer_uid == 0 || owner.is_none_or(|o| peer_uid == o)
}

#[cfg(unix)]
async fn accept_loop(shared: Arc<DaemonShared>) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let path = peercove_ipc::socket_path();
    let _ = std::fs::remove_file(&path); // 前回異常終了の残骸
    let listener = tokio::net::UnixListener::bind(&path)
        .with_context(|| format!("{} の bind に失敗しました", path.display()))?;
    // root で起動したデーモンのソケットへ、非特権の UI/CLI が接続できるようにする。
    // 所有者(PEERCOVE_OWNER_UID / SUDO_UID)が分かればそのユーザー専用に絞り、
    // 分からなければ後方互換で全ユーザーへ開く(ADR-0038)。いずれの場合も
    // accept 時に接続元 uid を検証する。
    let owner = authorized_owner_uid();
    if let Some(uid) = owner {
        // 所有者所有・0600 にして、OS レベルでも所有者(と root)のみ接続可にする。
        if let Err(e) = std::os::unix::fs::chown(&path, Some(uid), None) {
            tracing::warn!("ソケットの所有者設定に失敗しました(uid={uid}): {e}");
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("{} のパーミッション設定に失敗しました", path.display()))?;
    } else {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666))
            .with_context(|| format!("{} のパーミッション設定に失敗しました", path.display()))?;
        tracing::warn!(
            "IPC ソケットの所有者を特定できないため全ユーザーに開放します。共用 PC では \
             PEERCOVE_OWNER_UID(操作を許可するユーザーの uid)を設定してください(ADR-0038)"
        );
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
        // 接続元 uid を検証(SO_PEERCRED)。所有者以外(root を除く)は拒否する。
        match stream.peer_cred() {
            Ok(cred) if authorize_peer(owner, cred.uid()) => {}
            Ok(cred) => {
                tracing::warn!(
                    "認可されていないユーザー(uid={})からの IPC 接続を拒否しました",
                    cred.uid()
                );
                continue;
            }
            Err(e) => {
                tracing::warn!("接続元の資格情報を取得できないため拒否しました: {e}");
                continue;
            }
        }
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

/// `12:34:56.789 INFO  peercove::daemon: メッセージ`
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

#[cfg(all(test, unix))]
mod auth_tests {
    use super::{authorize_peer, authorized_owner_uid};

    #[test]
    fn authorize_peer_respects_owner() {
        // 所有者が判明: root と所有者のみ許可、他ユーザーは拒否。
        assert!(authorize_peer(Some(1000), 0), "root は常に許可");
        assert!(authorize_peer(Some(1000), 1000), "所有者は許可");
        assert!(!authorize_peer(Some(1000), 1001), "別ユーザーは拒否");
        // 所有者不明: 後方互換で全て許可(呼び出し側が警告)。
        assert!(authorize_peer(None, 0));
        assert!(authorize_peer(None, 1234));
    }

    #[test]
    fn owner_uid_prefers_env_and_ignores_root() {
        // 環境変数はプロセス共有なので 1 テストにまとめ、必ず後始末する。
        for key in ["PEERCOVE_OWNER_UID", "SUDO_UID"] {
            std::env::remove_var(key);
        }
        assert_eq!(authorized_owner_uid(), None, "どちらも無ければ None");

        std::env::set_var("SUDO_UID", "1000");
        assert_eq!(authorized_owner_uid(), Some(1000), "SUDO_UID を採用");

        std::env::set_var("PEERCOVE_OWNER_UID", "1005");
        assert_eq!(
            authorized_owner_uid(),
            Some(1005),
            "PEERCOVE_OWNER_UID を優先"
        );

        std::env::set_var("PEERCOVE_OWNER_UID", "0");
        std::env::set_var("SUDO_UID", "0");
        assert_eq!(authorized_owner_uid(), None, "uid 0 は所有者にしない");

        for key in ["PEERCOVE_OWNER_UID", "SUDO_UID"] {
            std::env::remove_var(key);
        }
    }
}

#[cfg(all(test, windows))]
mod winsec_tests {
    use super::winsec::{is_valid_sid, pipe_sddl};

    #[test]
    fn valid_sid_restricts_and_invalid_falls_back() {
        // 所有者不明: 従来どおり認証済みユーザー(AU)。
        assert_eq!(pipe_sddl(None), "D:(A;;FA;;;SY)(A;;FA;;;BA)(A;;FRFW;;;AU)");
        // 正当な SID: そのユーザーのみに絞る。
        let sid = "S-1-5-21-1111111111-2222222222-3333333333-1001";
        let sddl = pipe_sddl(Some(sid));
        assert!(sddl.contains(sid));
        assert!(!sddl.contains(";AU)"));
        // 不正な値(SDDL インジェクション狙い)は SID として弾き AU へフォールバック。
        assert_eq!(pipe_sddl(Some("AU)(A;;FA;;;WD")), pipe_sddl(None));
        assert_eq!(pipe_sddl(Some("../etc")), pipe_sddl(None));
    }

    #[test]
    fn sid_validation() {
        assert!(is_valid_sid("S-1-5-18"));
        assert!(is_valid_sid("S-1-5-21-1-2-3-1001"));
        assert!(!is_valid_sid("S-1-5-x")); // 数字以外
        assert!(!is_valid_sid("D:(A;;FA;;;WD)")); // SDDL 断片
        assert!(!is_valid_sid("")); // 空
        assert!(!is_valid_sid("S-1-")); // 短すぎ
    }
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

    #[tokio::test]
    async fn diagnostics_keep_independent_failures_in_one_report() {
        let (shared, _rx) = test_shared();
        let missing = std::env::temp_dir().join(format!(
            "peercove-diagnostic-missing-{}-{}.toml",
            std::process::id(),
            unix_ms()
        ));
        let report = shared.diagnose(missing).await;
        assert_eq!(
            report.overall,
            peercove_core::diagnostics::DiagnosticOverall::Problem
        );
        assert!(report
            .checks
            .iter()
            .any(|check| { check.id == "config.valid" && check.status == DiagnosticStatus::Fail }));
        assert!(report.checks.iter().any(|check| {
            check.id == "tunnel.running" && check.status == DiagnosticStatus::Fail
        }));
        assert!(report
            .checks
            .iter()
            .any(|check| check.id == "app.ipc_compatible"));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_existing_secret_is_reported_as_acl_managed_and_passes() {
        let (shared, _rx) = test_shared();
        let dir = std::env::temp_dir().join(format!(
            "peercove-diagnostic-windows-secret-{}-{}",
            std::process::id(),
            unix_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("host.key"), PrivateKey::generate().to_base64()).unwrap();
        let config = dir.join("host.toml");
        std::fs::write(
            &config,
            "[interface]\nprivate_key_file = \"host.key\"\naddress = \"10.99.200.1/24\"\nlisten_port = 51820\n",
        )
        .unwrap();

        let report = shared.diagnose(config).await;
        let check = report
            .checks
            .iter()
            .find(|check| check.id == "permissions.secret_files")
            .unwrap();
        assert_eq!(check.status, DiagnosticStatus::Pass);
        assert_eq!(
            check.evidence.get("permission_check").map(String::as_str),
            Some("peercove_acl_managed")
        );

        let _ = std::fs::remove_dir_all(dir);
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
                app_version: Some(env!("CARGO_PKG_VERSION").to_string()),
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
                app_version: Some(env!("CARGO_PKG_VERSION").to_string()),
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
        let security = winsec::PipeSecurity::for_owner(None).expect("記述子の作成");
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

        ring.push("INFO", "peercove::test", "テスト行".to_string());
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
