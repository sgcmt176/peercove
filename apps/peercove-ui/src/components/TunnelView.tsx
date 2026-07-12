import { useEffect, useState } from "react";
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
  formatHandshake,
  formatRate,
  formatRtt,
} from "../ipc";
import { rateSeries, rttSeries } from "../history";
import { totalUnread } from "../chat";
import { ConfirmModal, Modal } from "./Modal";
import { InviteDialog } from "./InviteDialog";
import { DnsDialog } from "./DnsDialog";
import { SubnetDialog } from "./SubnetDialog";
import { AclDialog } from "./AclDialog";
import { Avatar } from "./Avatar";
import { ChatPanel } from "./ChatPanel";
import { Sparkline } from "./Sparkline";
import { t } from "../i18n";

/**
 * ネットワーク詳細(トンネル稼働中)。ホストのときだけ招待・削除・名前変更ができる。
 *
 * 中身はタブ構成(M3-6)。「メンバー」「統計」に加えて、チャット(M3-13)や
 * ファイル送信(M3-9)は将来ここへタブを足して収める。
 */
export function TunnelView({
  tunnel,
  onChanged,
  onSettings,
}: {
  tunnel: Tunnel;
  onChanged: () => void;
  onSettings: () => void;
}) {
  const isHost = tunnel.role === "hosting";
  // RTT はコントロールチャネルで測っているので、台帳と公開鍵で突き合わせる
  const peerByKey = new Map(tunnel.peers.map((peer) => [peer.publicKey, peer]));
  const [tab, setTab] = useState<"members" | "chat" | "stats" | "inbox">(
    "members",
  );
  /** 受信ボックスの中身(M3-9b)。status のポーリングに合わせて読み直す。 */
  const [inbox, setInbox] = useState<InboxItem[]>([]);
  const [inviting, setInviting] = useState(false);
  const [showDns, setShowDns] = useState(false);
  const [removing, setRemoving] = useState<Member | null>(null);
  /** 広告サブネット編集の対象(M3-7b、ホストのみ)。 */
  const [editingSubnets, setEditingSubnets] = useState<Member | null>(null);
  /** ファイル送信ダイアログ(M3-13e: 宛先をチェックボックスで選ぶ)。 */
  const [sendingFile, setSendingFile] = useState(false);
  /** 通信制御ダイアログ(M3-10、ADR-0018。ホストのみ)。 */
  const [showAcl, setShowAcl] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  // 受信ボックス(ディレクトリ)は tunnel が 2 秒ごとに更新されるのに合わせて
  // 読み直す。一覧はタブのバッジにも使うので、タブが閉じていても読む
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

  const activeTransfers = tunnel.transfers.filter(
    (transfer) => !transfer.done,
  ).length;
  /** チャットの未読合計(M3-13b)。ストアは App のポーリングが同期済み。 */
  const chatUnread = totalUnread(tunnel);

  const stop = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.stopTunnel(tunnel.config);
      onChanged();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

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

  const rename = async (member: Member, newName: string) => {
    setError(null);
    try {
      await api.renameMember(tunnel.config, member.publicKey, newName);
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
      {tunnel.removed && (
        <section className="card card--error">
          <h2>{t.tunnel.removedTitle}</h2>
          <p>{t.tunnel.removedBody}</p>
          <button
            type="button"
            className="button--danger"
            onClick={() => void stop()}
            disabled={busy}
          >
            {t.tunnel.disconnectConfirm}
          </button>
        </section>
      )}

      <section className="card">
        <div className="card__head">
          <h2>
            {tunnel.network}
            <span className="tag">
              {isHost ? t.networks.roleHost : t.networks.roleMember}
            </span>
          </h2>
          <div className="row">
            <button
              type="button"
              className="button--ghost"
              onClick={() => setShowDns(true)}
            >
              {t.dns.button}
            </button>
            <button type="button" className="button--ghost" onClick={onSettings}>
              {t.networks.settings}
            </button>
            <button
              type="button"
              className="button--ghost"
              onClick={() => void stop()}
              disabled={busy}
            >
              {t.tunnel.disconnect}
            </button>
          </div>
        </div>
        <dl className="facts">
          <dt>{t.common.virtualIp}</dt>
          <dd className="mono">{tunnel.address}</dd>
          <dt>{t.tunnel.configFileLabel}</dt>
          <dd className="mono ellipsis" title={tunnel.config}>
            {tunnel.config}
          </dd>
        </dl>
        {error && <p className="error-text">{error}</p>}
        {notice && <p className="notice">{notice}</p>}
      </section>

      <section className="card">
        <div className="tabs">
          <button
            type="button"
            className={tab === "members" ? "tabs__tab tabs__tab--active" : "tabs__tab"}
            onClick={() => setTab("members")}
          >
            {t.tunnel.membersHead(tunnel.members.length)}
          </button>
          <button
            type="button"
            className={tab === "chat" ? "tabs__tab tabs__tab--active" : "tabs__tab"}
            onClick={() => setTab("chat")}
          >
            {t.tunnel.tabs.chat}
            {chatUnread > 0 && <span className="tabs__badge">{chatUnread}</span>}
          </button>
          <button
            type="button"
            className={tab === "stats" ? "tabs__tab tabs__tab--active" : "tabs__tab"}
            onClick={() => setTab("stats")}
          >
            {t.tunnel.tabs.stats}
          </button>
          <button
            type="button"
            className={tab === "inbox" ? "tabs__tab tabs__tab--active" : "tabs__tab"}
            onClick={() => setTab("inbox")}
          >
            {t.tunnel.tabs.inbox}
            {inbox.length + activeTransfers > 0 && (
              <span className="tabs__badge">{inbox.length + activeTransfers}</span>
            )}
          </button>
          {tab === "members" && tunnel.members.some((m) => !m.isSelf) && (
            <button
              type="button"
              className="tabs__action"
              onClick={() => setSendingFile(true)}
            >
              {t.transfer.sendButton}
            </button>
          )}
          {isHost &&
            tab === "members" &&
            tunnel.members.filter((m) => !m.isHost).length >= 2 && (
              <button
                type="button"
                className="tabs__action"
                onClick={() => setShowAcl(true)}
              >
                {t.acl.button}
              </button>
            )}
          {isHost && tab === "members" && (
            <button
              type="button"
              className="tabs__action"
              onClick={() => setInviting(true)}
            >
              {t.tunnel.invite}
            </button>
          )}
        </div>

        {tab === "members" ? (
          tunnel.members.length === 0 ? (
            <p className="muted">{t.tunnel.ledgerPending}</p>
          ) : (
            <>
              <ul className="members">
                {tunnel.members.map((member) => (
                  <MemberRow
                    key={member.publicKey}
                    config={tunnel.config}
                    member={member}
                    peer={peerByKey.get(member.publicKey) ?? null}
                    canManage={isHost && !member.isHost}
                    canEditDns={isHost || member.isSelf}
                    onRemove={() => setRemoving(member)}
                    onRename={(newName) => void rename(member, newName)}
                    onRenameDns={(label) => void renameDns(member, label)}
                    onSubnets={() => setEditingSubnets(member)}
                  />
                ))}
              </ul>
              {/* 直接通信の説明(M3-4)。経路バッジが出るメンバー視点でのみ表示 */}
              {!isHost && tunnel.members.some((member) => member.route) && (
                <p className="muted small">{t.tunnel.directNote}</p>
              )}
            </>
          )
        ) : tab === "chat" ? (
          <ChatPanel tunnel={tunnel} />
        ) : tab === "inbox" ? (
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
        ) : tunnel.peers.length === 0 ? (
          <p className="muted">{t.tunnel.peers.empty}</p>
        ) : (
          <PeersTable config={tunnel.config} peers={tunnel.peers} />
        )}
      </section>

      {inviting && (
        <InviteDialog
          configPath={tunnel.config}
          onClose={() => {
            setInviting(false);
            onChanged();
          }}
        />
      )}

      {showDns && (
        <DnsDialog
          configPath={tunnel.config}
          members={tunnel.members}
          isHost={isHost}
          onClose={() => {
            setShowDns(false);
            onChanged();
          }}
        />
      )}

      {showAcl && (
        <AclDialog
          configPath={tunnel.config}
          members={tunnel.members}
          onClose={() => {
            setShowAcl(false);
            onChanged();
          }}
        />
      )}

      {editingSubnets && (
        <SubnetDialog
          configPath={tunnel.config}
          member={editingSubnets}
          onClose={() => {
            setEditingSubnets(null);
            onChanged();
          }}
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
            setTab("inbox");
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
  const candidates = tunnel.members.filter((member) => !member.isSelf);

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

/** WG のピア統計(暗号セッション単位)。転送速度は履歴バッファから出す。 */
function PeersTable({ config, peers }: { config: string; peers: Peer[] }) {
  return (
    <table className="peers">
      <thead>
        <tr>
          <th>{t.tunnel.peers.publicKey}</th>
          <th>{t.tunnel.peers.endpoint}</th>
          <th>{t.tunnel.peers.lastHandshake}</th>
          <th>{t.tunnel.peers.rtt}</th>
          <th>{t.tunnel.peers.rate}</th>
          <th>{t.tunnel.peers.rx}</th>
          <th>{t.tunnel.peers.tx}</th>
        </tr>
      </thead>
      <tbody>
        {peers.map((peer) => {
          const rates = rateSeries(config, peer.publicKey);
          const rtts = rttSeries(config, peer.publicKey);
          return (
            <tr key={peer.publicKey}>
              <td className="mono ellipsis" title={peer.publicKey}>
                {peer.publicKey.slice(0, 12)}…
              </td>
              <td className="mono">
                {peer.endpoint ?? t.tunnel.peers.notConnected}
              </td>
              <td>{formatHandshake(peer.lastHandshakeAgeSecs)}</td>
              <td title={t.tunnel.peers.rttTitle}>
                <span className="cell-trend">
                  <Sparkline values={rtts} title={t.tunnel.peers.rttTitle} />
                  <span className="stat stat--rtt">{formatRtt(peer.rttMs)}</span>
                </span>
              </td>
              <td title={t.tunnel.peers.rateTitle}>
                <span className="cell-trend">
                  <Sparkline values={rates} title={t.tunnel.peers.rateTitle} />
                  <span className="stat">{formatRate(rates.at(-1) ?? null)}</span>
                </span>
              </td>
              <td>{formatBytes(peer.rxBytes)}</td>
              <td>{formatBytes(peer.txBytes)}</td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}

function MemberRow({
  config,
  member,
  peer,
  canManage,
  canEditDns,
  onRemove,
  onRename,
  onRenameDns,
  onSubnets,
}: {
  config: string;
  member: Member;
  /** この行のメンバーと張っている WG ピア(統計)。無ければ null。 */
  peer: Peer | null;
  canManage: boolean;
  /** DNS 名を変更できるか(ADR-0021。ホスト = 全員 / メンバー = 自分のみ)。 */
  canEditDns: boolean;
  onRemove: () => void;
  onRename: (newName: string) => void;
  /** DNS 名(先頭ラベルのみ)の変更(ADR-0021、M3-14a)。 */
  onRenameDns: (label: string) => void;
  /** 広告サブネットの編集を開く(M3-7b、ホストのみ)。 */
  onSubnets: () => void;
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
    <li className="member">
      <Avatar
        publicKey={member.publicKey}
        name={member.name}
        online={member.online}
        onlineLabel={member.online ? t.tunnel.member.online : t.tunnel.member.offline}
      />
      <span className="member__id">
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
          {member.isHost && <span className="tag">host</span>}
          {member.route && (
            <span
              className={`tag tag--route-${member.route}`}
              title={t.tunnel.member.route.title}
            >
              {t.tunnel.member.route[member.route]}
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
          {member.subnets.map((subnet) => (
            <span key={subnet} className="tag mono" title={t.subnet.badgeTitle}>
              {subnet}
            </span>
          ))}
        </span>
        <span className="member__meta">
          <span className="mono">{member.ip}</span>
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
                {canEditDns && (
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
            )
          )}
        </span>
      </span>
      <span className="member__stats">
        {peer && (
          <>
            <Sparkline values={rates} title={t.tunnel.member.rateTitle} />
            <span className="stat" title={t.tunnel.member.rateTitle}>
              {formatRate(rates.at(-1) ?? null)}
            </span>
          </>
        )}
        {peer?.rttMs != null && (
          <span className="tag" title={t.tunnel.member.rttTitle}>
            {formatRtt(peer.rttMs)}
          </span>
        )}
      </span>
      {canManage && !editing && (
        <span className="member__actions">
          <button
            type="button"
            className="button--icon"
            title={t.subnet.edit}
            onClick={onSubnets}
          >
            🖧
          </button>
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
          <button
            type="button"
            className="button--icon button--icon-danger"
            title={t.tunnel.member.remove}
            onClick={onRemove}
          >
            ×
          </button>
        </span>
      )}
    </li>
  );
}
