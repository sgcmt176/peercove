import { useEffect, useState } from "react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import {
  InboxItem,
  Member,
  Peer,
  Transfer,
  Tunnel,
  api,
  baseName,
  errorMessage,
  formatBytes,
  formatRate,
  formatRtt,
} from "../ipc";
import { rateSeries } from "../history";
import { SharedRefKind } from "../sharedRefs";
import { ConfirmModal, Modal } from "./Modal";
import { InviteDialog, MemberInviteDialog } from "./InviteDialog";
import { DnsView } from "./DnsDialog";
import { SubnetView } from "./SubnetDialog";
import { Avatar } from "./Avatar";
import { ChatPanel } from "./ChatPanel";
import { Sparkline } from "./Sparkline";
import { t } from "../i18n";
import { QualityView } from "./QualityView";

/**
 * ネットワーク詳細の中身(トンネル稼働中)。表示するビューはサイドバーで
 * 選ばれ、`view` プロパティで渡ってくる(M3-16 でタブを廃止)。ヘッダー
 * (戻る・状態・切断)とネットワーク設定ページは App 側が持つ。
 */
export function TunnelView({
  tunnel,
  view,
  chatTarget,
  onOpenChat,
  onOpenRef,
  onView,
  onChanged,
}: {
  tunnel: Tunnel;
  view: "members" | "chat" | "stats" | "inbox" | "dns" | "subnets";
  /** チャットで開く相手(メンバー行の 💬 → 1:1 会話)。 */
  chatTarget: { peer: string } | null;
  /** 相手を指定してチャットを開く(1:1 会話を選ぶ)。 */
  onOpenChat: (peer: string) => void;
  /** チャットの `@memo:id` / `@schedule:id` カード(ADR-0052 決定 1、
   * ADR-0053)をクリックしたときの遷移先。 */
  onOpenRef: (kind: SharedRefKind, id: string) => void;
  /** 別のビューへ切り替える(送信後に受信へ 等)。 */
  onView: (view: "members" | "chat" | "stats" | "inbox" | "dns") => void;
  onChanged: () => void;
}) {
  const isHost = tunnel.role === "hosting";
  // RTT はコントロールチャネルで測っているので、台帳と公開鍵で突き合わせる
  const peerByKey = new Map(tunnel.peers.map((peer) => [peer.publicKey, peer]));
  /** 受信ボックスの中身(M3-9b)。status のポーリングに合わせて読み直す。 */
  const [inbox, setInbox] = useState<InboxItem[]>([]);
  const [inviting, setInviting] = useState(false);
  // メンバーによる招待発行(ADR-0048)。自分の行の canInvite が true のときだけ
  const [memberInviting, setMemberInviting] = useState(false);
  // メンバー詳細ページ(ADR-0048)。一覧の ℹ から開く
  const [detail, setDetail] = useState<Member | null>(null);
  const [removing, setRemoving] = useState<Member | null>(null);
  /** ファイル送信ダイアログ(M3-13e: 宛先をチェックボックスで選ぶ)。 */
  const [sendingFile, setSendingFile] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  // 受信ボックス(ディレクトリ)は tunnel が 2 秒ごとに更新されるのに合わせて読む
  useEffect(() => {
    let alive = true;
    api
      .listInbox(tunnel.config)
      .then((items) => {
        if (alive) setInbox(items);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [tunnel]);

  const onlineCount = tunnel.members.filter((member) => member.online).length;
  // 全ピアの直近速度の合計(「転送速度」カード — M3-15)
  const totalRate = tunnel.peers.reduce(
    (sum, peer) => sum + (rateSeries(tunnel.config, peer.publicKey).at(-1) ?? 0),
    0,
  );

  const remove = async (member: Member) => {
    setBusy(true);
    setError(null);
    try {
      const name = await api.removeMember(tunnel.config, member.publicKey);
      setRemoving(null);
      setNotice(t.tunnel.removeNotice(name));
      setTimeout(() => setNotice(null), 8000);
      onChanged();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const approve = async (member: Member) => {
    setBusy(true);
    setError(null);
    try {
      await api.approveMember(tunnel.config, member.publicKey);
      setNotice(t.tunnel.member.approved);
      setTimeout(() => setNotice(null), 8000);
      onChanged();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  // 表示名の変更(ADR-0021 / ADR-0027、M3-14a / M3-19)。DNS 名と同じく、
  // 自分の行はホスト = 直接 host.toml、メンバー = デーモン経由(ホストが検証)。
  // ホストから見た他メンバーの行は renameMember(host.toml を直接)。
  const rename = async (member: Member, newName: string) => {
    setError(null);
    try {
      if (member.isSelf) {
        if (isHost) await api.setHostDisplayName(tunnel.config, newName);
        else await api.setMyDisplayName(tunnel.config, newName);
        setNotice(t.tunnel.member.displayRenamed);
        setTimeout(() => setNotice(null), 8000);
      } else {
        await api.renameMember(tunnel.config, member.publicKey, newName);
      }
      onChanged();
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  // DNS 名の変更(ADR-0021、M3-14a)。ホストは全員分を直接、
  // メンバーは自分の分だけデーモン経由(ホストが検証)で変更する
  const renameDns = async (member: Member, label: string) => {
    setError(null);
    try {
      if (isHost) {
        if (member.isHost) await api.setHostDnsName(tunnel.config, label);
        else await api.setMemberDnsName(tunnel.config, member.publicKey, label);
      } else {
        await api.setMyDnsName(tunnel.config, label);
      }
      setNotice(t.tunnel.member.dnsRenamed);
      setTimeout(() => setNotice(null), 8000);
      onChanged();
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  return (
    <>
      {error && <p className="error-text">{error}</p>}
      {notice && <p className="notice">{notice}</p>}

      {view === "members" ? (
        <>
          <div className="stat-cards">
            <div className="stat-card">
              <span className="stat-card__icon">IP</span>
              <div className="stat-card__body">
                <span className="stat-card__label">
                  {t.tunnel.overview.virtualIp}
                </span>
                <span className="stat-card__value mono">
                  {tunnel.address}
                  <CopyIcon value={tunnel.address} title={t.tunnel.table.copyIp} />
                </span>
              </div>
            </div>
            <div className="stat-card">
              <span className="stat-card__icon">👥</span>
              <div className="stat-card__body">
                <span className="stat-card__label">
                  {t.tunnel.overview.online}
                </span>
                <span className="stat-card__value">
                  {t.tunnel.overview.onlineCount(onlineCount)}
                </span>
              </div>
            </div>
            <div className="stat-card">
              <span className="stat-card__icon">📈</span>
              <div className="stat-card__body">
                <span className="stat-card__label">{t.tunnel.overview.rate}</span>
                <span className="stat-card__value">{formatRate(totalRate)}</span>
              </div>
            </div>
          </div>

          <section className="card">
            <div className="detail__toolbar">
              <h2 className="card-title">
                {t.tunnel.membersHead(tunnel.members.length)}
              </h2>
              <div className="detail__toolbar-actions">
                {tunnel.members.some((m) => !m.isSelf) && (
                  <button
                    type="button"
                    className="button--ghost small"
                    onClick={() => setSendingFile(true)}
                  >
                    {t.transfer.sendButton}
                  </button>
                )}
                {isHost && (
                  <button type="button" onClick={() => setInviting(true)}>
                    {t.tunnel.invite}
                  </button>
                )}
                {!isHost &&
                  tunnel.members.some((m) => m.isSelf && m.canInvite) && (
                    <button type="button" onClick={() => setMemberInviting(true)}>
                      {t.tunnel.invite}
                    </button>
                  )}
              </div>
            </div>

            {tunnel.members.length === 0 ? (
              <p className="muted">{t.tunnel.ledgerPending}</p>
            ) : (
              <>
                <div className="table-scroll">
                  <table className="member-table">
                    <thead>
                      <tr>
                        <th>{t.sidebar.members}</th>
                        <th>{t.tunnel.table.role}</th>
                        <th>{t.tunnel.table.virtualIp}</th>
                        <th>{t.tunnel.table.rate}</th>
                        <th>{t.tunnel.table.rtt}</th>
                        <th className="member-table__actions-head">
                          {t.tunnel.table.actions}
                        </th>
                      </tr>
                    </thead>
                    <tbody>
                      {tunnel.members.map((member) => (
                        <MemberRow
                          key={member.publicKey}
                          config={tunnel.config}
                          member={member}
                          peer={peerByKey.get(member.publicKey) ?? null}
                          canManage={isHost && !member.isHost}
                          canEditDns={isHost || member.isSelf}
                          onChat={() => onOpenChat(member.ip)}
                          onDetail={() => setDetail(member)}
                          onRemove={() => setRemoving(member)}
                          onApprove={() => void approve(member)}
                          onRename={(newName) => void rename(member, newName)}
                          onRenameDns={(label) => void renameDns(member, label)}
                        />
                      ))}
                    </tbody>
                  </table>
                </div>
                {/* 直接通信の説明(M3-4)。経路バッジが出るメンバー視点でのみ表示 */}
                {!isHost && tunnel.members.some((member) => member.route) && (
                  <p className="muted small">{t.tunnel.directNote}</p>
                )}
              </>
            )}
          </section>
        </>
      ) : view === "chat" ? (
        <ChatPanel
          tunnel={tunnel}
          initialConversation={chatTarget}
          onOpenRef={onOpenRef}
        />
      ) : view === "stats" ? (
        <QualityView tunnel={tunnel} />
      ) : view === "inbox" ? (
        <section className="card">
          <InboxPanel
            tunnel={tunnel}
            inbox={inbox}
            onInboxChanged={(items) => setInbox(items)}
            onNotice={(text) => {
              setNotice(text);
              setTimeout(() => setNotice(null), 8000);
            }}
            onError={(text) => setError(text)}
          />
        </section>
      ) : view === "dns" ? (
        <DnsView
          configPath={tunnel.config}
          members={tunnel.members}
          distributed={tunnel.dnsRecords}
          isHost={isHost}
        />
      ) : (
        <SubnetView configPath={tunnel.config} members={tunnel.members} />
      )}

      {inviting && (
        <InviteDialog
          configPath={tunnel.config}
          onClose={() => {
            setInviting(false);
            onChanged();
          }}
        />
      )}

      {memberInviting && (
        <MemberInviteDialog
          configPath={tunnel.config}
          onClose={() => {
            setMemberInviting(false);
            onChanged();
          }}
        />
      )}

      {detail && (
        <MemberDetailDialog
          config={tunnel.config}
          // 台帳の更新(2 秒ポーリング)を追いかけて最新の行を出す
          member={
            tunnel.members.find((m) => m.publicKey === detail.publicKey) ??
            detail
          }
          isHost={isHost}
          onClose={() => setDetail(null)}
          onNotice={(text) => {
            setNotice(text);
            setTimeout(() => setNotice(null), 8000);
          }}
          onError={(text) => setError(text)}
        />
      )}

      {removing && (
        <ConfirmModal
          title={t.tunnel.remove.title}
          confirmLabel={t.tunnel.remove.confirm}
          busy={busy}
          onClose={() => setRemoving(null)}
          onConfirm={() => void remove(removing)}
          message={t.tunnel.remove.message(removing.name ?? removing.ip)}
        />
      )}

      {sendingFile && (
        <SendFileDialog
          tunnel={tunnel}
          onClose={() => setSendingFile(false)}
          onSent={(count) => {
            setSendingFile(false);
            onView("inbox");
            setNotice(t.transfer.startedMany(count));
            setTimeout(() => setNotice(null), 8000);
          }}
        />
      )}
    </>
  );
}

/**
 * ファイル送信ダイアログ(M3-13e)。ファイルを選び、宛先メンバーを
 * チェックボックスで選んで送る(オフラインのメンバーは選べない — ADR-0015)。
 * 進捗は受信タブの転送一覧に宛先ごとに出る。
 */
function SendFileDialog({
  tunnel,
  onClose,
  onSent,
}: {
  tunnel: Tunnel;
  onClose: () => void;
  onSent: (count: number) => void;
}) {
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [path, setPath] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const candidates = tunnel.members.filter(
    (member) => !member.isSelf && member.inviteStatus !== "awaiting_approval",
  );

  const toggle = (ip: string) => {
    setChecked((prev) => {
      const next = new Set(prev);
      if (next.has(ip)) {
        next.delete(ip);
      } else {
        next.add(ip);
      }
      return next;
    });
  };

  const pick = async () => {
    setError(null);
    try {
      const picked = await api.pickFile();
      if (picked) setPath(picked);
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const send = async () => {
    if (!path || checked.size === 0) return;
    setBusy(true);
    setError(null);
    try {
      for (const ip of checked) {
        await api.sendFile(tunnel.config, ip, path);
      }
      onSent(checked.size);
    } catch (e) {
      setError(errorMessage(e));
      setBusy(false);
    }
  };

  return (
    <Modal title={t.transfer.dialogTitle} onClose={onClose}>
      <div className="modal__body">
        <span className="field__label">{t.transfer.fileLabel}</span>
        <div className="row">
          <button type="button" className="button--ghost" onClick={() => void pick()}>
            {t.transfer.pick}
          </button>
          {path ? (
            <span className="mono ellipsis" title={path}>
              {baseName(path)}
            </span>
          ) : (
            <span className="muted small">{t.transfer.noFile}</span>
          )}
        </div>
        <span className="field__label">{t.transfer.recipientsLabel}</span>
        {candidates.length === 0 ? (
          <p className="muted small">{t.transfer.noCandidates}</p>
        ) : (
          <ul className="chat__pick">
            {candidates.map((member) => (
              <li key={member.ip}>
                <label className="chat__pick-row">
                  <input
                    type="checkbox"
                    disabled={!member.online || member.blocked}
                    checked={checked.has(member.ip)}
                    onChange={() => toggle(member.ip)}
                  />
                  <Avatar
                    publicKey={member.publicKey}
                    name={member.name}
                    online={member.online}
                    onlineLabel={
                      member.online
                        ? t.tunnel.member.online
                        : t.tunnel.member.offline
                    }
                  />
                  <span className="ellipsis">{member.name ?? member.ip}</span>
                  {member.blocked ? (
                    <span
                      className="muted small"
                      title={t.tunnel.member.blockedTitle}
                    >
                      🚫 {t.tunnel.member.blocked}
                    </span>
                  ) : (
                    !member.online && (
                      <span className="muted small">
                        {t.tunnel.member.offline}
                      </span>
                    )
                  )}
                </label>
              </li>
            ))}
          </ul>
        )}
        <p className="muted small">{t.transfer.dialogNote}</p>
        {error && <p className="error-text small">{error}</p>}
      </div>
      <div className="modal__actions">
        <button type="button" className="button--ghost" onClick={onClose}>
          {t.common.cancel}
        </button>
        <button
          type="button"
          onClick={() => void send()}
          disabled={busy || !path || checked.size === 0}
        >
          {busy ? t.common.running : t.transfer.sendTo(checked.size)}
        </button>
      </div>
    </Modal>
  );
}

/**
 * 受信タブ(M3-9b): 転送の進捗と受信ボックス。
 * 受信は自動(ADR-0015)なので、ここでは「保存」か「削除」だけを選ぶ。
 */
function InboxPanel({
  tunnel,
  inbox,
  onInboxChanged,
  onNotice,
  onError,
}: {
  tunnel: Tunnel;
  inbox: InboxItem[];
  onInboxChanged: (items: InboxItem[]) => void;
  onNotice: (text: string) => void;
  onError: (text: string) => void;
}) {
  const nameByIp = new Map(
    tunnel.members.map((member) => [member.ip, member.name ?? member.ip]),
  );
  // 新しいものを上に(レジストリは開始順で並んでいる)
  const transfers = [...tunnel.transfers].reverse();

  const refresh = () => {
    api
      .listInbox(tunnel.config)
      .then(onInboxChanged)
      .catch(() => {});
  };

  const save = async (item: InboxItem) => {
    onError("");
    try {
      const saved = await api.saveInboxFile(tunnel.config, item.name);
      if (saved) onNotice(t.inbox.savedTo(saved));
      refresh();
    } catch (e) {
      onError(errorMessage(e));
    }
  };

  const remove = async (item: InboxItem) => {
    onError("");
    try {
      await api.deleteInboxFile(tunnel.config, item.name);
      onNotice(t.inbox.deleted(item.name));
      refresh();
    } catch (e) {
      onError(errorMessage(e));
    }
  };

  return (
    <>
      {transfers.length > 0 && (
        <>
          <h3 className="section-head">{t.transfer.head}</h3>
          <ul className="transfers">
            {transfers.map((transfer) => (
              <TransferRow
                key={transfer.id}
                transfer={transfer}
                peerName={nameByIp.get(transfer.peer) ?? transfer.peer}
              />
            ))}
          </ul>
        </>
      )}
      <h3 className="section-head">{t.inbox.head}</h3>
      {inbox.length === 0 ? (
        <p className="muted">{t.inbox.empty}</p>
      ) : (
        <>
          <ul className="inbox">
            {inbox.map((item) => (
              <li key={item.name} className="inbox__item">
                <span className="inbox__id">
                  <span className="mono ellipsis" title={item.name}>
                    {item.name}
                  </span>
                  <span className="muted small">
                    {item.fromName && t.inbox.from(item.fromName)}
                    {" · "}
                    {formatBytes(item.size)}
                    {item.receivedUnixMs !== null &&
                      ` · ${new Date(item.receivedUnixMs).toLocaleString()}`}
                  </span>
                </span>
                <span className="row">
                  <button type="button" onClick={() => void save(item)}>
                    {t.inbox.save}
                  </button>
                  <button
                    type="button"
                    className="button--ghost"
                    onClick={() => void remove(item)}
                  >
                    {t.inbox.delete}
                  </button>
                </span>
              </li>
            ))}
          </ul>
          <p className="muted small">{t.inbox.note}</p>
        </>
      )}
    </>
  );
}

/** 転送 1 件の行: 向き・相手・ファイル名・進捗バー(または結果)。 */
function TransferRow({
  transfer,
  peerName,
}: {
  transfer: Transfer;
  peerName: string;
}) {
  const percent =
    transfer.size === 0
      ? 100
      : Math.min(100, Math.floor((transfer.transferred * 100) / transfer.size));
  return (
    <li className="transfer">
      <span className="transfer__id">
        <span>
          <span className="tag">{t.transfer.direction(transfer.direction)}</span>{" "}
          <span className="mono ellipsis" title={transfer.name}>
            {transfer.name}
          </span>
        </span>
        <span className="muted small">{peerName}</span>
      </span>
      <span className="transfer__state">
        {transfer.error ? (
          <span className="error-text small">{t.transfer.failed(transfer.error)}</span>
        ) : transfer.done ? (
          <span className="muted small">{t.transfer.done}</span>
        ) : (
          <>
            <span className="progress" title={`${percent}%`}>
              <span className="progress__bar" style={{ width: `${percent}%` }} />
            </span>
            <span className="muted small stat">
              {t.transfer.progress(
                formatBytes(transfer.transferred),
                formatBytes(transfer.size),
              )}
            </span>
          </>
        )}
      </span>
    </li>
  );
}

/** OS 種別の表示名（E-E 11 の端末バッジ）。未知の値はそのまま出す。 */
function platformLabel(platform: string): string {
  switch (platform) {
    case "windows":
      return "Windows";
    case "linux":
      return "Linux";
    case "android":
      return "Android";
    case "macos":
      return "macOS";
    default:
      return platform;
  }
}

function MemberRow({
  config,
  member,
  peer,
  canManage,
  canEditDns,
  onChat,
  onDetail,
  onRemove,
  onApprove,
  onRename,
  onRenameDns,
}: {
  config: string;
  member: Member;
  /** この行のメンバーと張っている WG ピア(統計)。無ければ null。 */
  peer: Peer | null;
  canManage: boolean;
  /** DNS 名を変更できるか(ADR-0021。ホスト = 全員 / メンバー = 自分のみ)。 */
  canEditDns: boolean;
  /** 1:1 チャットを開く(自分以外の行の 💬)。 */
  onChat: () => void;
  /** メンバー詳細ページを開く(ADR-0048)。 */
  onDetail: () => void;
  onRemove: () => void;
  onApprove: () => void;
  onRename: (newName: string) => void;
  /** DNS 名(先頭ラベルのみ)の変更(ADR-0021、M3-14a)。 */
  onRenameDns: (label: string) => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(member.name ?? "");
  const [editingDns, setEditingDns] = useState(false);
  const [dnsDraft, setDnsDraft] = useState("");
  const rates = peer ? rateSeries(config, peer.publicKey) : [];
  /** DNS 名の先頭ラベル(fqdn の 1 つ目。編集対象はここだけ)。 */
  const dnsLabel = member.dnsName?.split(".")[0] ?? "";

  const commit = () => {
    const trimmed = draft.trim();
    setEditing(false);
    if (trimmed && trimmed !== member.name) onRename(trimmed);
  };

  const commitDns = () => {
    const trimmed = dnsDraft.trim();
    setEditingDns(false);
    if (trimmed && trimmed !== dnsLabel) onRenameDns(trimmed);
  };

  return (
    <tr className="member-table__row">
      <td className="member-cell">
        <Avatar
          publicKey={member.publicKey}
          name={member.name}
          online={member.online}
          onlineLabel={
            member.online ? t.tunnel.member.online : t.tunnel.member.offline
          }
        />
        <span className="member-cell__text">
          <span className="member__title">
            {editing ? (
              <input
                className="member__edit"
                value={draft}
                autoFocus
                onChange={(event) => setDraft(event.target.value)}
                onBlur={commit}
                onKeyDown={(event) => {
                  if (event.key === "Enter") commit();
                  if (event.key === "Escape") setEditing(false);
                }}
              />
            ) : (
              <span className="member__name">
                {member.name ?? t.tunnel.member.noName}
              </span>
            )}
            {member.isSelf && (
              <span className="tag tag--self" title={t.tunnel.member.selfTitle}>
                {t.tunnel.member.self}
              </span>
            )}
            <span
              className="tag member__version"
              title={
                member.appVersion
                  ? t.update.memberVersion(member.appVersion)
                  : t.update.memberVersionUnknown
              }
            >
              {member.appVersion ? `v${member.appVersion}` : "v?"}
            </span>
            {member.platform && (
              <span
                className="tag member__platform"
                title={t.update.memberPlatformTitle}
              >
                {platformLabel(member.platform)}
              </span>
            )}
            {member.route && (
              <span
                className={`tag tag--route-${member.route}`}
                title={t.tunnel.member.route.title}
              >
                {member.forceRelay ? t.tunnel.member.route.aclRelay : t.tunnel.member.route[member.route]}
              </span>
            )}
            {member.forceRelay && member.aclRuleId && (
              <span className="muted small" title={t.tunnel.member.route.aclTitle}>
                {member.aclRuleId}
              </span>
            )}
            {member.blocked && (
              <span
                className="tag tag--blocked"
                title={t.tunnel.member.blockedTitle}
              >
                🚫 {t.tunnel.member.blocked}
              </span>
            )}
            {!member.isHost && member.inviteStatus && (
              <span
                className={`tag tag--invite-${member.inviteStatus}`}
                title={
                  member.inviteExpiresAt
                    ? t.tunnel.member.inviteExpires(
                        new Date(member.inviteExpiresAt * 1000).toLocaleString(),
                      )
                    : t.invite.never
                }
              >
                {t.tunnel.member.inviteStatus[member.inviteStatus]}
              </span>
            )}
            {member.subnets.map((subnet) => (
              <span key={subnet} className="tag mono" title={t.subnet.badgeTitle}>
                {subnet}
              </span>
            ))}
          </span>
          <span className="member__meta">
            {editingDns ? (
              <input
                className="member__edit mono"
                value={dnsDraft}
                autoFocus
                onChange={(event) => setDnsDraft(event.target.value)}
                onBlur={commitDns}
                onKeyDown={(event) => {
                  if (event.key === "Enter") commitDns();
                  if (event.key === "Escape") setEditingDns(false);
                }}
              />
            ) : (
              member.dnsName && (
                <span className="mono ellipsis" title={member.dnsName}>
                  {member.dnsName}
                </span>
              )
            )}
            {member.dnsName && <CopyIcon value={member.dnsName} title={t.dns.copy} />}
            {canEditDns && !editingDns && (
              <button
                type="button"
                className="button--icon button--icon-inline"
                title={t.tunnel.member.editDns}
                onClick={() => {
                  setDnsDraft(dnsLabel);
                  setEditingDns(true);
                }}
              >
                ✎
              </button>
            )}
          </span>
        </span>
      </td>
      <td>
        <span className={member.isHost ? "tag tag--self" : "tag"}>
          {member.isHost ? t.networks.roleHost : t.networks.roleMember}
        </span>
      </td>
      <td className="member-table__ip">
        <span className="mono">{member.ip}</span>
        <CopyIcon value={member.ip} title={t.tunnel.table.copyIp} />
      </td>
      <td>
        {peer ? (
          <span className="cell-trend">
            <Sparkline values={rates} title={t.tunnel.member.rateTitle} />
            <span className="stat" title={t.tunnel.member.rateTitle}>
              {formatRate(rates.at(-1) ?? null)}
            </span>
          </span>
        ) : (
          <span className="muted">—</span>
        )}
      </td>
      <td>
        {peer?.rttMs != null ? (
          <span className="tag" title={t.tunnel.member.rttTitle}>
            {formatRtt(peer.rttMs)}
          </span>
        ) : (
          <span className="muted">—</span>
        )}
      </td>
      <td className="member-table__actions">
        {canManage && member.inviteStatus === "awaiting_approval" && (
          <button type="button" className="small" onClick={onApprove}>
            {t.tunnel.member.approve}
          </button>
        )}
        {!member.isSelf && member.inviteStatus !== "awaiting_approval" && (
          <button
            type="button"
            className="button--icon"
            title={t.tunnel.table.chat}
            onClick={onChat}
          >
            💬
          </button>
        )}
        <button
          type="button"
          className="button--icon"
          title={t.tunnel.member.detail}
          onClick={onDetail}
        >
          ℹ
        </button>
        {(canManage || member.isSelf) && !editing && (
          <button
            type="button"
            className="button--icon"
            title={t.tunnel.member.rename}
            onClick={() => {
              setDraft(member.name ?? "");
              setEditing(true);
            }}
          >
            ✎
          </button>
        )}
        {canManage && !editing && (
          <button
            type="button"
            className="button--icon button--icon-danger"
            title={t.tunnel.member.remove}
            onClick={onRemove}
          >
            ×
          </button>
        )}
      </td>
    </tr>
  );
}

/**
 * メンバー詳細ページ(ADR-0048)。一覧では出し切れない情報(公開鍵・招待者・
 * 招待状態など)をまとめ、ホストには「招待発行の許可」(端末指名)も出す。
 * 今後メンバー単位の情報・操作を足す受け皿。
 */
function MemberDetailDialog({
  config,
  member,
  isHost,
  onClose,
  onNotice,
  onError,
}: {
  config: string;
  member: Member;
  isHost: boolean;
  onClose: () => void;
  onNotice: (text: string) => void;
  onError: (text: string) => void;
}) {
  // 端末指名のチェックは楽観更新(台帳への反映は約 5 秒後)
  const [canInvite, setCanInvite] = useState(member.canInvite);
  const [busy, setBusy] = useState(false);

  const toggleCanInvite = async (allowed: boolean) => {
    setBusy(true);
    onError("");
    try {
      await api.setMemberCanInvite(config, member.publicKey, allowed);
      setCanInvite(allowed);
      onNotice(t.tunnel.member.canInviteUpdated);
    } catch (e) {
      onError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      title={t.tunnel.member.detailTitle(member.name ?? member.ip)}
      onClose={onClose}
    >
      <div className="modal__body">
        <dl className="facts">
          <dt>{t.tunnel.member.detailName}</dt>
          <dd>{member.name ?? t.tunnel.member.noName}</dd>
          <dt>{t.tunnel.member.detailRole}</dt>
          <dd>{member.isHost ? t.networks.roleHost : t.networks.roleMember}</dd>
          <dt>{t.tunnel.table.virtualIp}</dt>
          <dd className="mono">
            {member.ip}
            <CopyIcon value={member.ip} title={t.tunnel.table.copyIp} />
          </dd>
          {member.dnsName && (
            <>
              <dt>{t.tunnel.member.detailDns}</dt>
              <dd className="mono wrap-anywhere">
                {member.dnsName}
                <CopyIcon value={member.dnsName} title={t.dns.copy} />
              </dd>
            </>
          )}
          <dt>{t.tunnel.member.detailOs}</dt>
          <dd>{member.platform ? platformLabel(member.platform) : "—"}</dd>
          <dt>{t.tunnel.member.detailVersion}</dt>
          <dd>{member.appVersion ? `v${member.appVersion}` : "—"}</dd>
          <dt>{t.tunnel.member.detailKey}</dt>
          <dd className="mono wrap-anywhere small">{member.publicKey}</dd>
          <dt>{t.tunnel.member.detailState}</dt>
          <dd>
            {member.online
              ? t.tunnel.member.online
              : t.tunnel.member.offline}
            {member.blocked && ` · 🚫 ${t.tunnel.member.blocked}`}
          </dd>
          {member.route && (
            <>
              <dt>{t.tunnel.member.detailRoute}</dt>
              <dd>
                {member.forceRelay
                  ? t.tunnel.member.route.aclRelay
                  : t.tunnel.member.route[member.route]}
              </dd>
            </>
          )}
          {!member.isHost && member.inviteStatus && (
            <>
              <dt>{t.tunnel.member.detailInvite}</dt>
              <dd>
                {t.tunnel.member.inviteStatus[member.inviteStatus]}
                {member.inviteExpiresAt &&
                  ` · ${t.tunnel.member.inviteExpires(
                    new Date(member.inviteExpiresAt * 1000).toLocaleString(),
                  )}`}
              </dd>
            </>
          )}
          {!member.isHost && (
            <>
              <dt>{t.tunnel.member.invitedBy}</dt>
              <dd>{member.invitedBy ?? t.tunnel.member.invitedByHost}</dd>
            </>
          )}
          {member.subnets.length > 0 && (
            <>
              <dt>{t.tunnel.member.detailSubnets}</dt>
              <dd className="mono">{member.subnets.join(", ")}</dd>
            </>
          )}
        </dl>

        {isHost && !member.isHost && !member.isSelf && (
          <label className="field--check">
            <input
              type="checkbox"
              checked={canInvite}
              disabled={busy}
              onChange={(event) => void toggleCanInvite(event.target.checked)}
            />
            <span>
              {t.tunnel.member.canInviteLabel}
              <small className="muted"> {t.tunnel.member.canInviteHint}</small>
            </span>
          </label>
        )}
      </div>
      <div className="modal__actions">
        <button type="button" onClick={onClose}>
          {t.common.close}
        </button>
      </div>
    </Modal>
  );
}

/** クリップボードへコピーする小さなアイコンボタン(コピー後 1.5 秒だけ ✓)。 */
function CopyIcon({ value, title }: { value: string; title: string }) {
  const [copied, setCopied] = useState(false);

  const copy = async () => {
    try {
      await writeText(value);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // コピー失敗は致命的でないので握りつぶす
    }
  };

  return (
    <button
      type="button"
      className="button--icon button--icon-inline"
      title={title}
      onClick={() => void copy()}
    >
      {copied ? "✓" : "📋"}
    </button>
  );
}
