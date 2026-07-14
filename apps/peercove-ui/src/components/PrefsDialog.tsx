import { useState } from "react";
import { Prefs, loadPrefs, savePrefs } from "../prefs";
import { t } from "../i18n";

/**
 * アプリ全体の設定ページ(M3-13e → M3-16 でページ化)。localStorage 保存で
 * このマシンだけに効き、トグルは即時反映(保存ボタンなし)。ネットワークごとの
 * 設定(受信上限など)は各ネットワーク詳細の「設定」から。
 *
 * サイドバーの「設定」(ネットワーク一覧を表示中)から開く 1 ページ。
 */
export function AppSettingsView() {
  const [prefs, setPrefs] = useState<Prefs>(loadPrefs);

  const toggle = (key: keyof Prefs) => {
    const next = { ...prefs, [key]: !prefs[key] };
    setPrefs(next);
    savePrefs(next);
  };

  return (
    <section className="card">
      <h2 className="card-title">{t.prefs.title}</h2>
      <div className="page-body">
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
        <p className="muted small prefs__license">{t.footer}</p>
      </div>
    </section>
  );
}
