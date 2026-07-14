import { useEffect, useState } from "react";
import { Settings, api, errorMessage } from "../ipc";
import { ConfirmModal } from "./Modal";
import { t } from "../i18n";

/**
 * ネットワークごとの設定ページ(M2-G5 → M3-16 でページ化)。編集できるのは
 * 「自分側の設定」だけ。サイドバーの「設定」(ネットワーク詳細を表示中)、
 * または一覧カードの「設定」から開く 1 ページ。
 *
 * メンバーの追加・削除・改名はメンバー一覧側の操作([[peer]] の編集)なので
 * ここには出さない。**表示名・DNS 名の変更もメンバー一覧に一元化**した
 * (自分の行の ✎ から。ADR-0027、M3-19)。仮想 IP / サブネットは init・join の
 * やり直しになるため表示のみ。
 *
 * MTU・待受ポート・ホストのエンドポイントは、インターフェース生成時に決まる
 * ので**再接続するまで反映されない**。保存後にその旨を出す。
 */
export function NetworkSettingsView({
  configPath,
  liveInterfaceName,
}: {
  configPath: string;
  /** 接続中の実アダプタ名(自動採番後）。未接続なら null で設定値を出す。 */
  liveInterfaceName: string | null;
}) {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .readSettings(configPath)
      .then(setSettings)
      .catch((e) => setError(errorMessage(e)));
  }, [configPath]);

  return (
    <section className="card">
      <h2 className="card-title">{t.settings.title}</h2>
      {settings ? (
        <SettingsForm
          configPath={configPath}
          settings={settings}
          liveInterfaceName={liveInterfaceName}
        />
      ) : (
        <div className="page-body">
          {error ? (
            <p className="error-text">{error}</p>
          ) : (
            <p className="muted">{t.common.loading}</p>
          )}
        </div>
      )}
    </section>
  );
}

function SettingsForm({
  configPath,
  settings,
  liveInterfaceName,
}: {
  configPath: string;
  settings: Settings;
  liveInterfaceName: string | null;
}) {
  // 接続中は実際に使われているアダプタ名を出す(既定名が衝突すると自動採番で
  // peercove1 等になるため、設定ファイルの値と食い違いうる — ADR-0028、M3-20)
  const interfaceName = liveInterfaceName ?? settings.interfaceName;
  const [listenPort, setListenPort] = useState(
    settings.listenPort === null ? "" : String(settings.listenPort),
  );
  const [mtu, setMtu] = useState(String(settings.mtu));
  const [hostEndpoint, setHostEndpoint] = useState(settings.hostEndpoint ?? "");
  const [direct, setDirect] = useState(settings.direct);
  const [maxRecvFileMb, setMaxRecvFileMb] = useState(
    String(settings.maxRecvFileMb),
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  // デバイス鍵ローテーション(ADR-0020、M3-11)
  const [confirmRotate, setConfirmRotate] = useState(false);
  const [rotateBusy, setRotateBusy] = useState(false);

  const rotate = async () => {
    setRotateBusy(true);
    setError(null);
    setNotice(null);
    try {
      await api.rotateKey(configPath);
      setNotice(t.settings.rotateKeyRequested);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setRotateBusy(false);
      setConfirmRotate(false);
    }
  };

  const save = async () => {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      const mtuValue = Number(mtu);
      if (!Number.isInteger(mtuValue)) throw t.settings.mtuInteger;
      const portValue = listenPort.trim() === "" ? null : Number(listenPort);
      if (portValue !== null && !Number.isInteger(portValue)) {
        throw t.settings.portInteger;
      }
      const maxFileValue = Number(maxRecvFileMb);
      if (!Number.isInteger(maxFileValue) || maxFileValue < 0) {
        throw t.settings.maxFileInteger;
      }
      const result = await api.saveSettings(configPath, {
        // 表示名・DNS 名はメンバー一覧から変更する(ADR-0027、M3-19)。
        // ここでは現在値をそのまま書き戻して消さない
        displayName: settings.displayName,
        dnsName: settings.dnsName,
        listenPort: portValue,
        mtu: mtuValue,
        hostEndpoint:
          settings.isMember && hostEndpoint.trim() !== ""
            ? hostEndpoint.trim()
            : null,
        direct,
        maxRecvFileMb: maxFileValue,
      });
      setNotice(
        result.restartRequired
          ? t.settings.savedRestart
          : t.settings.savedLive,
      );
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <div className="page-body">
        <dl className="facts">
          <dt>{t.settings.interface}</dt>
          <dd className="mono">{interfaceName}</dd>
          <dt>{t.common.virtualIp}</dt>
          <dd className="mono">{settings.address}</dd>
          <dt>{t.common.configFile}</dt>
          <dd className="mono wrap-anywhere" title={configPath}>
            {configPath}
          </dd>
        </dl>

        <p className="muted small">{t.settings.nameMovedHint}</p>

        <label className="field">
          <span>
            {t.settings.portLabel}
            <small className="muted">
              {t.settings.portHint(settings.isMember, settings.defaultListenPort)}
            </small>
          </span>
          <input
            type="text"
            inputMode="numeric"
            value={listenPort}
            placeholder={String(settings.defaultListenPort)}
            onChange={(event) => setListenPort(event.target.value)}
          />
        </label>

        <label className="field">
          <span>
            {t.settings.mtuLabel}
            <small className="muted">{t.settings.mtuHint(settings.defaultMtu)}</small>
          </span>
          <input
            type="text"
            inputMode="numeric"
            value={mtu}
            onChange={(event) => setMtu(event.target.value)}
          />
        </label>

        {settings.isMember && (
          <label className="field">
            <span>
              {t.settings.hostEndpointLabel}
              <small className="muted">{t.settings.hostEndpointHint}</small>
            </span>
            <input
              type="text"
              value={hostEndpoint}
              placeholder={t.settings.hostEndpointPlaceholder}
              onChange={(event) => setHostEndpoint(event.target.value)}
            />
          </label>
        )}

        <label className="field">
          <span>
            {t.settings.maxFileLabel}
            <small className="muted">
              {t.settings.maxFileHint(settings.defaultMaxRecvFileMb)}
            </small>
          </span>
          <input
            type="text"
            inputMode="numeric"
            value={maxRecvFileMb}
            placeholder={String(settings.defaultMaxRecvFileMb)}
            onChange={(event) => setMaxRecvFileMb(event.target.value)}
          />
        </label>

        {settings.isMember && (
          <label className="field--check">
            <input
              type="checkbox"
              checked={direct}
              onChange={(event) => setDirect(event.target.checked)}
            />
            <span>
              {t.settings.directLabel}
              <small className="muted"> {t.settings.directHint}</small>
            </span>
          </label>
        )}

        {settings.isMember && (
          <div className="field">
            <span>
              {t.settings.rotateKeyLabel}
              <small className="muted"> — {t.settings.rotateKeyHint}</small>
            </span>
            <div>
              <button
                type="button"
                className="button--ghost"
                onClick={() => setConfirmRotate(true)}
              >
                {t.settings.rotateKeyButton}
              </button>
            </div>
          </div>
        )}

        <p className="muted small">{t.settings.restartHint(settings.isMember)}</p>

        {error && <p className="error-text">{error}</p>}
        {notice && <p className="notice">{notice}</p>}
      </div>
      <div className="page-actions">
        <button type="button" onClick={() => void save()} disabled={busy}>
          {busy ? t.common.saving : t.common.save}
        </button>
      </div>
      {confirmRotate && (
        <ConfirmModal
          title={t.settings.rotateKeyButton}
          message={<p>{t.settings.rotateKeyConfirm}</p>}
          confirmLabel={t.settings.rotateKeyButton}
          busy={rotateBusy}
          onConfirm={() => void rotate()}
          onClose={() => setConfirmRotate(false)}
        />
      )}
    </>
  );
}
