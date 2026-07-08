import { useEffect, useState } from "react";
import { Settings, api, errorMessage } from "../ipc";
import { Modal } from "./Modal";

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
    <Modal title="設定" onClose={onClose}>
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
            <p className="muted">読み込み中…</p>
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
  const [listenPort, setListenPort] = useState(
    settings.listenPort === null ? "" : String(settings.listenPort),
  );
  const [mtu, setMtu] = useState(String(settings.mtu));
  const [hostEndpoint, setHostEndpoint] = useState(settings.hostEndpoint ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const save = async () => {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      const mtuValue = Number(mtu);
      if (!Number.isInteger(mtuValue)) throw "MTU には整数を入力してください";
      const portValue = listenPort.trim() === "" ? null : Number(listenPort);
      if (portValue !== null && !Number.isInteger(portValue)) {
        throw "待受ポートには整数を入力してください";
      }
      const result = await api.saveSettings(configPath, {
        displayName: displayName.trim() === "" ? null : displayName.trim(),
        listenPort: portValue,
        mtu: mtuValue,
        hostEndpoint:
          settings.isMember && hostEndpoint.trim() !== ""
            ? hostEndpoint.trim()
            : null,
      });
      setNotice(
        result.restartRequired
          ? "保存しました。切断して接続し直すと反映されます。"
          : "保存しました。数秒でトンネルに反映されます。",
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
          <dt>インターフェース</dt>
          <dd className="mono">{settings.interfaceName}</dd>
          <dt>仮想 IP</dt>
          <dd className="mono">{settings.address}</dd>
          <dt>設定ファイル</dt>
          <dd className="mono ellipsis" title={configPath}>
            {configPath}
          </dd>
        </dl>

        <label className="field">
          <span>表示名（メンバー一覧に出る名前）</span>
          <input
            type="text"
            value={displayName}
            placeholder={settings.isMember ? "（未設定）" : "host"}
            onChange={(event) => setDisplayName(event.target.value)}
          />
        </label>

        <label className="field">
          <span>
            待受ポート（UDP）
            <small className="muted"> — 空欄なら{settings.isMember ? "OS 任せ" : `既定の ${settings.defaultListenPort}`}</small>
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
            MTU
            <small className="muted"> — 既定 {settings.defaultMtu}。回線によっては下げると安定します</small>
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
              ホストのエンドポイント
              <small className="muted"> — ホストの IP:ポート。引っ越し後の付け替えに使います</small>
            </span>
            <input
              type="text"
              value={hostEndpoint}
              placeholder="203.0.113.5:51820"
              onChange={(event) => setHostEndpoint(event.target.value)}
            />
          </label>
        )}

        <p className="muted small">
          待受ポート・MTU
          {settings.isMember && "・ホストのエンドポイント"}
          は、トンネルを作り直すまで反映されません（切断 →
          接続で反映されます）。
        </p>

        {error && <p className="error-text">{error}</p>}
        {notice && <p className="notice">{notice}</p>}
      </div>
      <div className="modal__actions">
        <button type="button" className="button--ghost" onClick={onClose}>
          閉じる
        </button>
        <button type="button" onClick={() => void save()} disabled={busy}>
          {busy ? "保存中…" : "保存"}
        </button>
      </div>
    </>
  );
}
