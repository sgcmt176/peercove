import { useState } from "react";
import {
  Member,
  Tunnel,
  api,
  errorMessage,
  formatBytes,
  formatHandshake,
  formatRtt,
} from "../ipc";
import { ConfirmModal } from "./Modal";
import { InviteDialog } from "./InviteDialog";
import { DnsDialog } from "./DnsDialog";
import { t } from "../i18n";

/** ネットワーク詳細(トンネル稼働中)。ホストのときだけ招待・削除・名前変更ができる。 */
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
  const rttByKey = new Map(tunnel.peers.map((peer) => [peer.publicKey, peer.rttMs]));
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
        <div className="card__head">
          <h2>{t.tunnel.membersHead(tunnel.members.length)}</h2>
          {isHost && (
            <button type="button" onClick={() => setInviting(true)}>
              {t.tunnel.invite}
            </button>
          )}
        </div>
        {tunnel.members.length === 0 ? (
          <p className="muted">{t.tunnel.ledgerPending}</p>
        ) : (
          <ul className="members">
            {tunnel.members.map((member) => (
              <MemberRow
                key={member.publicKey}
                member={member}
                rttMs={rttByKey.get(member.publicKey) ?? null}
                canManage={isHost && !member.isHost}
                onRemove={() => setRemoving(member)}
                onRename={(newName) => void rename(member, newName)}
              />
            ))}
          </ul>
        )}
        {/* 直接通信の説明(M3-4)。経路バッジが出るメンバー視点でのみ表示 */}
        {!isHost && tunnel.members.some((member) => member.route) && (
          <p className="muted small">{t.tunnel.directNote}</p>
        )}
      </section>

      {tunnel.peers.length > 0 && (
        <section className="card">
          <h2>{t.tunnel.peers.head}</h2>
          <table className="peers">
            <thead>
              <tr>
                <th>{t.tunnel.peers.publicKey}</th>
                <th>{t.tunnel.peers.endpoint}</th>
                <th>{t.tunnel.peers.lastHandshake}</th>
                <th>{t.tunnel.peers.rtt}</th>
                <th>{t.tunnel.peers.rx}</th>
                <th>{t.tunnel.peers.tx}</th>
              </tr>
            </thead>
            <tbody>
              {tunnel.peers.map((peer) => (
                <tr key={peer.publicKey}>
                  <td className="mono ellipsis" title={peer.publicKey}>
                    {peer.publicKey.slice(0, 12)}…
                  </td>
                  <td className="mono">
                    {peer.endpoint ?? t.tunnel.peers.notConnected}
                  </td>
                  <td>{formatHandshake(peer.lastHandshakeAgeSecs)}</td>
                  <td title={t.tunnel.peers.rttTitle}>{formatRtt(peer.rttMs)}</td>
                  <td>{formatBytes(peer.rxBytes)}</td>
                  <td>{formatBytes(peer.txBytes)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
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

function MemberRow({
  member,
  rttMs,
  canManage,
  onRemove,
  onRename,
}: {
  member: Member;
  rttMs: number | null;
  canManage: boolean;
  onRemove: () => void;
  onRename: (newName: string) => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(member.name ?? "");

  const commit = () => {
    const trimmed = draft.trim();
    setEditing(false);
    if (trimmed && trimmed !== member.name) onRename(trimmed);
  };

  return (
    <li className="member">
      <span
        className={member.online ? "dot dot--online" : "dot dot--offline"}
        aria-label={member.online ? t.tunnel.member.online : t.tunnel.member.offline}
      />
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
        <span className="member__name">{member.name ?? t.tunnel.member.noName}</span>
      )}
      <span className="mono muted">{member.ip}</span>
      {member.dnsName && (
        <span className="mono muted small ellipsis" title={member.dnsName}>
          {member.dnsName}
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
      {rttMs !== null && (
        <span className="tag" title={t.tunnel.member.rttTitle}>
          {formatRtt(rttMs)}
        </span>
      )}
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
