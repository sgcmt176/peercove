import { useEffect, useState } from "react";
import { Settings, api, errorMessage } from "../ipc";
import { ConfirmModal, Modal } from "./Modal";
import { t } from "../i18n";

/**
 * 設定編集(M2-G5)。編集できるのは「自分側の設定」だけ。
 *
 * メンバーの追加・削除・改名はメンバー一覧側の操作([[peer]] の編集)なので
 * ここには出さない。仮想 IP / サブネットは init・join のやり直しになるため
 * 表示のみ。
 *
 * MTU・待受ポート・ホストのエンドポイントは、インターフェース生成時に決まる
 * ので**再接続するまで反映されない**。保存後にその旨を出す。
 */
export function SettingsDialog({
  configPath,
  onClose,
}: {
  configPath: string;
  onClose: () => void;
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
    <Modal title={t.settings.title} onClose={onClose}>
      {settings ? (
        <SettingsForm
          configPath={configPath}
          settings={settings}
          onClose={onClose}
        />
      ) : (
        <div className="modal__body">
          {error ? (
            <p className="error-text">{error}</p>
          ) : (
            <p className="muted">{t.common.loading}</p>
          )}
        </div>
      )}
    </Modal>
  );
}

function SettingsForm({
  configPath,
  settings,
  onClose,
}: {
  configPath: string;
  settings: Settings;
  onClose: () => void;
}) {
  const [displayName, setDisplayName] = useState(settings.displayName ?? "");
  // (host のみ)自分の DNS 名(ADR-0021、M3-14a)。空なら既定(host)
  const [dnsName, setDnsName] = useState(settings.dnsName ?? "");
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
        displayName: displayName.trim() === "" ? null : displayName.trim(),
        dnsName:
          settings.isMember || dnsName.trim() === "" ? null : dnsName.trim(),
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
      <div className="modal__body">
        <dl className="facts">
          <dt>{t.settings.interface}</dt>
          <dd className="mono">{settings.interfaceName}</dd>
          <dt>{t.common.virtualIp}</dt>
          <dd className="mono">{settings.address}</dd>
          <dt>{t.common.configFile}</dt>
          <dd className="mono wrap-anywhere" title={configPath}>
            {configPath}
          </dd>
        </dl>

        <label className="field">
          <span>{t.settings.displayNameLabel}</span>
          <input
            type="text"
            value={displayName}
            placeholder={
              settings.isMember
                ? t.settings.displayNamePlaceholderMember
                : t.settings.displayNamePlaceholderHost
            }
            onChange={(event) => setDisplayName(event.target.value)}
          />
        </label>

        {!settings.isMember && (
          <label className="field">
            <span>
              {t.settings.dnsNameLabel}
              <small className="muted">{t.settings.dnsNameHint}</small>
            </span>
            <input
              type="text"
              value={dnsName}
              placeholder="host"
              onChange={(event) => setDnsName(event.target.value)}
            />
          </label>
        )}

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
      <div className="modal__actions">
        <button type="button" className="button--ghost" onClick={onClose}>
          {t.common.close}
        </button>
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
