import { useState } from "react";
import { Member, api, errorMessage } from "../ipc";
import { Avatar } from "./Avatar";
import { t } from "../i18n";

/**
 * サブネット共有ページ(M3-7b、ADR-0014 → M3-16 でページ化)。ホストが
 * 各メンバーの公開する背後 LAN(CIDR)を編集する。検証(CIDR 形式・重複)は
 * バックエンドが行い、エラーはそのまま表示する。ホスト自身は転送役にならない
 * ので一覧はホスト以外のメンバー。
 */
export function SubnetView({
  configPath,
  members,
}: {
  configPath: string;
  members: Member[];
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
}: {
  configPath: string;
  member: Member;
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
      await api.setMemberSubnets(configPath, member.publicKey, subnets);
      setNotice(t.subnet.saved);
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
