import { useState } from "react";
import { Member, api, errorMessage } from "../ipc";
import { Avatar } from "./Avatar";
import { t } from "../i18n";

/**
 * サブネット共有ページ(M3-7b、ADR-0014 → M3-16 でページ化)。ホストと、
 * 管理を委任された(admin)メンバーが各メンバーの公開する背後 LAN(CIDR)を
 * 編集する(ADR-0059 J-2 拡大の残り)。検証(CIDR 形式・重複)はバックエンドが
 * 行い、エラーはそのまま表示する。ホスト自身は転送役にならないので一覧は
 * ホスト以外のメンバー。ホストは設定ファイルへ直接、admin メンバーは
 * `api.adminSetSubnets` でホストへ中継する(最終判定は常にホスト側)。
 */
export function SubnetView({
  configPath,
  members,
  isHost,
}: {
  configPath: string;
  members: Member[];
  isHost: boolean;
}) {
  const targets = members.filter((member) => !member.isHost);

  return (
    <section className="card">
      <h2 className="card-title">{t.subnet.pageTitle}</h2>
      <p className="muted small">{t.subnet.intro}</p>
      {targets.length === 0 ? (
        <p className="muted">{t.subnet.empty}</p>
      ) : (
        <ul className="subnet-list">
          {targets.map((member) => (
            <SubnetRow
              key={member.publicKey}
              configPath={configPath}
              member={member}
              isHost={isHost}
            />
          ))}
        </ul>
      )}
      <p className="muted small">{t.subnet.note}</p>
    </section>
  );
}

/** メンバー 1 人分の公開 LAN 編集行。 */
function SubnetRow({
  configPath,
  member,
  isHost,
}: {
  configPath: string;
  member: Member;
  isHost: boolean;
}) {
  const [text, setText] = useState(member.subnets.join(" "));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const save = async () => {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      const subnets = text.split(/[\s,]+/).filter((s) => s.length > 0);
      if (isHost) {
        await api.setMemberSubnets(configPath, member.publicKey, subnets);
        setNotice(t.subnet.saved);
      } else {
        // admin メンバー(ADR-0059 J-2 拡大の残り): ホストへ中継。反映は次回の台帳配布
        await api.adminSetSubnets(configPath, member.ip, subnets);
        setNotice(t.dns.adminApplyNotice);
      }
      setTimeout(() => setNotice(null), 6000);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <li className="subnet-row">
      <div className="subnet-row__head">
        <Avatar
          publicKey={member.publicKey}
          name={member.name}
          online={member.online}
          onlineLabel={
            member.online ? t.tunnel.member.online : t.tunnel.member.offline
          }
        />
        <span className="subnet-row__name">
          {t.subnet.memberLabel(member.name ?? member.ip)}
        </span>
      </div>
      <div className="subnet-row__edit">
        <input
          value={text}
          placeholder={t.subnet.placeholder}
          className="mono"
          onChange={(event) => setText(event.target.value)}
        />
        <button type="button" onClick={() => void save()} disabled={busy}>
          {busy ? t.common.saving : t.subnet.save}
        </button>
      </div>
      <small className="muted">{t.subnet.hint}</small>
      {error && <p className="error-text small">{error}</p>}
      {notice && <p className="notice small">{notice}</p>}
    </li>
  );
}
