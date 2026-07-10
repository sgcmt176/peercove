import { useState } from "react";
import {
  Member,
  Peer,
  Tunnel,
  api,
  errorMessage,
  formatBytes,
  formatHandshake,
  formatRate,
  formatRtt,
} from "../ipc";
import { rateSeries, rttSeries } from "../history";
import { ConfirmModal } from "./Modal";
import { InviteDialog } from "./InviteDialog";
import { DnsDialog } from "./DnsDialog";
import { Avatar } from "./Avatar";
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
  const [tab, setTab] = useState<"members" | "stats">("members");
  const [inviting, setInviting] = useState(false);
  const [showDns, setShowDns] = useState(false);
  const [removing, setRemoving] = useState<Member | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

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
            className={tab === "stats" ? "tabs__tab tabs__tab--active" : "tabs__tab"}
            onClick={() => setTab("stats")}
          >
            {t.tunnel.tabs.stats}
          </button>
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
                    onRemove={() => setRemoving(member)}
                    onRename={(newName) => void rename(member, newName)}
                  />
                ))}
              </ul>
              {/* 直接通信の説明(M3-4)。経路バッジが出るメンバー視点でのみ表示 */}
              {!isHost && tunnel.members.some((member) => member.route) && (
                <p className="muted small">{t.tunnel.directNote}</p>
              )}
            </>
          )
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
    </>
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
  onRemove,
  onRename,
}: {
  config: string;
  member: Member;
  /** この行のメンバーと張っている WG ピア(統計)。無ければ null。 */
  peer: Peer | null;
  canManage: boolean;
  onRemove: () => void;
  onRename: (newName: string) => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(member.name ?? "");
  const rates = peer ? rateSeries(config, peer.publicKey) : [];

  const commit = () => {
    const trimmed = draft.trim();
    setEditing(false);
    if (trimmed && trimmed !== member.name) onRename(trimmed);
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
        </span>
        <span className="member__meta">
          <span className="mono">{member.ip}</span>
          {member.dnsName && (
            <span className="mono ellipsis" title={member.dnsName}>
              {member.dnsName}
            </span>
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
