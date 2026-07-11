import { useEffect, useState } from "react";
import { Member, api, errorMessage } from "../ipc";
import { Modal } from "./Modal";
import { Avatar } from "./Avatar";
import { t } from "../i18n";

/** 順不同の組を一意なキーにする（表示・保存の突き合わせ用）。 */
function pairKey(a: string, b: string): string {
  return a <= b ? `${a}|${b}` : `${b}|${a}`;
}

/**
 * メンバー間の通信制御（ACL — M3-10、ADR-0018）。ホストのみ。
 * メンバーの全組み合わせを一覧し、チェックした組を遮断する。
 * 変更は即保存され、実行中のデーモンが約 5 秒で反映する
 * （リレー遮断 + 台帳の再配布 → 直接通信も解除）。
 */
export function AclDialog({
  configPath,
  members,
  onClose,
}: {
  configPath: string;
  members: Member[];
  onClose: () => void;
}) {
  /** 遮断中の組（pairKey の集合）。 */
  const [deny, setDeny] = useState<Set<string>>(new Set());
  const [loaded, setLoaded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const candidates = members.filter((member) => !member.isHost);
  const pairs: [Member, Member][] = [];
  for (let i = 0; i < candidates.length; i += 1) {
    for (let j = i + 1; j < candidates.length; j += 1) {
      pairs.push([candidates[i], candidates[j]]);
    }
  }

  const load = async () => {
    try {
      const current = await api.listAcl(configPath);
      setDeny(new Set(current.map(([a, b]) => pairKey(a, b))));
      setLoaded(true);
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  useEffect(() => {
    void load();
    // 開いたときに 1 度だけ読む
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [configPath]);

  const toggle = async (a: Member, b: Member) => {
    const key = pairKey(a.ip, b.ip);
    const next = new Set(deny);
    if (next.has(key)) {
      next.delete(key);
    } else {
      next.add(key);
    }
    setBusy(true);
    setError(null);
    try {
      await api.setAcl(
        configPath,
        [...next].map((entry) => entry.split("|") as [string, string]),
      );
      setDeny(next);
    } catch (e) {
      setError(errorMessage(e));
      void load(); // 失敗したら実際の設定に合わせ直す
    } finally {
      setBusy(false);
    }
  };

  const name = (member: Member) => member.name ?? member.ip;

  return (
    <Modal title={t.acl.title} onClose={onClose}>
      <div className="modal__body">
        <p className="muted small">{t.acl.intro}</p>
        {pairs.length === 0 ? (
          <p className="muted small">{t.acl.needTwo}</p>
        ) : (
          <ul className="chat__pick">
            {pairs.map(([a, b]) => {
              const blocked = deny.has(pairKey(a.ip, b.ip));
              return (
                <li key={pairKey(a.ip, b.ip)}>
                  <label className="chat__pick-row">
                    <input
                      type="checkbox"
                      disabled={!loaded || busy}
                      checked={blocked}
                      onChange={() => void toggle(a, b)}
                    />
                    <Avatar
                      publicKey={a.publicKey}
                      name={a.name}
                      online={a.online}
                      onlineLabel=""
                    />
                    <span className="ellipsis">{name(a)}</span>
                    <span className="muted">⇔</span>
                    <Avatar
                      publicKey={b.publicKey}
                      name={b.name}
                      online={b.online}
                      onlineLabel=""
                    />
                    <span className="ellipsis">{name(b)}</span>
                    {blocked && (
                      <span className="tag tag--blocked">
                        {t.acl.blockedTag}
                      </span>
                    )}
                  </label>
                </li>
              );
            })}
          </ul>
        )}
        {error && <p className="error-text small">{error}</p>}
      </div>
      <div className="modal__actions">
        <button type="button" onClick={onClose}>
          {t.common.close}
        </button>
      </div>
    </Modal>
  );
}
