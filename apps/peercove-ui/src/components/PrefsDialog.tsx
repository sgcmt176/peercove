import { useState } from "react";
import { Modal } from "./Modal";
import { Prefs, loadPrefs, savePrefs } from "../prefs";
import { t } from "../i18n";

/**
 * アプリ全体の設定(M3-13e)。localStorage 保存でこのマシンだけに効き、
 * トグルは即時反映(保存ボタンなし)。ネットワークごとの設定(受信上限など)は
 * 各ネットワークの「設定」から。
 */
export function PrefsDialog({ onClose }: { onClose: () => void }) {
  const [prefs, setPrefs] = useState<Prefs>(loadPrefs);

  const toggle = (key: keyof Prefs) => {
    const next = { ...prefs, [key]: !prefs[key] };
    setPrefs(next);
    savePrefs(next);
  };

  return (
    <Modal title={t.prefs.title} onClose={onClose}>
      <div className="modal__body">
        <label className="chat__pick-row">
          <input
            type="checkbox"
            checked={prefs.notifications}
            onChange={() => toggle("notifications")}
          />
          <span>{t.prefs.notifications}</span>
        </label>
        <p className="muted small prefs__hint">{t.prefs.notificationsHint}</p>
        <label className="chat__pick-row">
          <input
            type="checkbox"
            checked={prefs.linkPreview}
            onChange={() => toggle("linkPreview")}
          />
          <span>{t.prefs.linkPreview}</span>
        </label>
        <p className="muted small prefs__hint">{t.prefs.linkPreviewHint}</p>
        <p className="muted small">{t.prefs.note}</p>
      </div>
    </Modal>
  );
}
