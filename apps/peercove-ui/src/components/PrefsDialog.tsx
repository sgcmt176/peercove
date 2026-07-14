import { useState } from "react";
import { Prefs, loadPrefs, savePrefs } from "../prefs";
import { UpdateInfo, api, errorMessage } from "../ipc";
import { checkForUpdate, clearUpdateCache } from "../update";
import { t } from "../i18n";
import { NetworkInfo } from "../ipc";
import { BackupView } from "./BackupView";

/**
 * アプリ全体の設定ページ(M3-13e → M3-16 でページ化)。localStorage 保存で
 * このマシンだけに効き、トグルは即時反映(保存ボタンなし)。ネットワークごとの
 * 設定(受信上限など)は各ネットワーク詳細の「設定」から。
 *
 * サイドバーの「設定」(ネットワーク一覧を表示中)から開く 1 ページ。
 */
export function AppSettingsView({
  version,
  daemonVersion,
  updateInfo,
  onUpdateInfo,
  networks,
  onChanged,
}: {
  version: string;
  daemonVersion: string | null;
  updateInfo: UpdateInfo | null;
  onUpdateInfo: (info: UpdateInfo | null) => void;
  networks: NetworkInfo[];
  onChanged: () => void;
}) {
  const [prefs, setPrefs] = useState<Prefs>(loadPrefs);
  const [checking, setChecking] = useState(false);
  const [updateError, setUpdateError] = useState<string | null>(null);

  const toggle = (key: "notifications" | "linkPreview" | "updateChecks" | "qualityAlerts") => {
    const next = { ...prefs, [key]: !prefs[key] };
    setPrefs(next);
    savePrefs(next);
    if (key === "updateChecks") {
      clearUpdateCache();
      if (!next.updateChecks) onUpdateInfo(null);
    }
  };

  const checkUpdate = async () => {
    setChecking(true);
    setUpdateError(null);
    try {
      const info = await checkForUpdate(version, true);
      onUpdateInfo(info);
    } catch (error) {
      setUpdateError(errorMessage(error));
    } finally {
      setChecking(false);
    }
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
        <label className="chat__pick-row">
          <input
            type="checkbox"
            checked={prefs.updateChecks}
            onChange={() => toggle("updateChecks")}
          />
          <span>{t.update.enabled}</span>
        </label>
        <p className="muted small prefs__hint">{t.update.enabledHint}</p>
        <label className="chat__pick-row">
          <input
            type="checkbox"
            checked={prefs.qualityAlerts}
            onChange={() => toggle("qualityAlerts")}
          />
          <span>{t.prefs.qualityAlerts}</span>
        </label>
        <p className="muted small prefs__hint">{t.prefs.qualityAlertsHint}</p>
        {prefs.qualityAlerts && (
          <label className="prefs__threshold">
            <span>{t.prefs.qualityLossThreshold}</span>
            <input
              type="number"
              min={1}
              max={100}
              step={1}
              value={prefs.qualityLossThreshold}
              onChange={(event) => {
                const next = {
                  ...prefs,
                  qualityLossThreshold: Math.min(100, Math.max(1, Number(event.target.value) || 5)),
                };
                setPrefs(next);
                savePrefs(next);
              }}
            />
            <span>%</span>
          </label>
        )}

        <section className="prefs__update" aria-labelledby="update-title">
          <h3 id="update-title">{t.update.title}</h3>
          <dl className="facts">
            <dt>{t.update.uiVersion}</dt>
            <dd className="mono">{version || t.update.unknown}</dd>
            <dt>{t.update.daemonVersion}</dt>
            <dd className="mono">{daemonVersion ?? t.update.unknown}</dd>
            {updateInfo && (
              <>
                <dt>{t.update.latestVersion}</dt>
                <dd className="mono">v{updateInfo.latestVersion}</dd>
              </>
            )}
          </dl>
          {updateInfo && (
            <p className={updateInfo.available ? "notice" : "muted small"}>
              {updateInfo.available
                ? t.update.available(updateInfo.latestVersion)
                : t.update.current}
            </p>
          )}
          {updateError && <p className="error-text small">{updateError}</p>}
          <div className="row">
            <button
              type="button"
              className="button--ghost"
              disabled={checking}
              onClick={() => void checkUpdate()}
            >
              {checking ? t.update.checking : t.update.checkNow}
            </button>
            {updateInfo?.available && (
              <button
                type="button"
                onClick={() => void api.openLink(updateInfo.releaseUrl)}
              >
                {t.update.openRelease}
              </button>
            )}
          </div>
        </section>
        <BackupView networks={networks} onChanged={onChanged} />
        <p className="muted small">{t.prefs.note}</p>
        <p className="muted small prefs__license">{t.footer}</p>
      </div>
    </section>
  );
}
