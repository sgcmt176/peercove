import { useState } from "react";
import { Member, api, errorMessage } from "../ipc";
import { Modal } from "./Modal";
import { t } from "../i18n";

/**
 * 広告サブネットの編集(M3-7b、ADR-0014)。ホストのメンバー行から開く。
 * 空で保存すると解除。検証(CIDR 形式・重複)はバックエンドが行い、
 * エラーはそのまま表示する。
 */
export function SubnetDialog({
  configPath,
  member,
  onClose,
}: {
  configPath: string;
  member: Member;
  onClose: () => void;
}) {
  const [text, setText] = useState(member.subnets.join(" "));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      const subnets = text.split(/[\s,]+/).filter((s) => s.length > 0);
      await api.setMemberSubnets(configPath, member.publicKey, subnets);
      onClose();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal title={t.subnet.title(member.name ?? member.ip)} onClose={onClose}>
      <p className="muted small">{t.subnet.intro}</p>
      <label className="field">
        <span>{t.subnet.label}</span>
        <input
          value={text}
          autoFocus
          placeholder={t.subnet.placeholder}
          onChange={(event) => setText(event.target.value)}
        />
        <small className="muted">{t.subnet.hint}</small>
      </label>
      <p className="muted small">{t.subnet.note}</p>
      {error && <p className="error-text">{error}</p>}
      <div className="row">
        <button type="button" onClick={() => void save()} disabled={busy}>
          {busy ? t.common.saving : t.common.save}
        </button>
        <button type="button" className="button--ghost" onClick={onClose}>
          {t.common.cancel}
        </button>
      </div>
    </Modal>
  );
}
