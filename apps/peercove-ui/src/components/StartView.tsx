import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { ConfigPaths, api, errorMessage } from "../ipc";
import { t } from "../i18n";

/**
 * 待機中の画面。「ホストを始める」と「参加する」の 2 択(1 アプリで両モード)。
 *
 * 設定が無ければ init 相当(鍵 + ランダムサブネットの host.toml)を UI が作る。
 * 別の設定ファイルを選ぶこともできる(ADR-0008)。
 */
export function StartView({ onStarted }: { onStarted: () => void }) {
  const [paths, setPaths] = useState<ConfigPaths | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .configPaths()
      .then(setPaths)
      .catch((e) => setError(errorMessage(e)));
  }, []);

  if (error) {
    return (
      <section className="card card--error">
        <h2>{t.start.locateError}</h2>
        <p className="error-text">{error}</p>
      </section>
    );
  }
  if (!paths) return <p className="muted">{t.common.loading}</p>;

  return (
    <>
      <p className="muted start__intro">{t.start.intro}</p>
      <HostCard paths={paths} onStarted={onStarted} setError={setError} />
      <JoinCard paths={paths} onStarted={onStarted} setError={setError} />
    </>
  );
}

function HostCard({
  paths,
  onStarted,
  setError,
}: {
  paths: ConfigPaths;
  onStarted: () => void;
  setError: (message: string) => void;
}) {
  const [configPath, setConfigPath] = useState(paths.host.path);
  const [exists, setExists] = useState(paths.host.exists);
  const [upnp, setUpnp] = useState(true);
  const [busy, setBusy] = useState<string | null>(null);
  const [localError, setLocalError] = useState<string | null>(null);

  const pickConfig = async () => {
    const picked = await open({
      multiple: false,
      directory: false,
      filters: [{ name: t.common.configFilter, extensions: ["toml"] }],
    });
    if (typeof picked === "string") {
      setConfigPath(picked);
      setExists(true);
    }
  };

  const start = async () => {
    setLocalError(null);
    try {
      let path = configPath;
      if (!exists) {
        setBusy(t.start.host.creating);
        const created = await api.initHost(false);
        path = created.configPath;
        setConfigPath(path);
        setExists(true);
      }
      setBusy(t.start.host.starting);
      await api.startHost(path, upnp);
      onStarted();
    } catch (e) {
      setLocalError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <section className="card">
      <h2>{t.start.host.title}</h2>
      <p className="muted">{t.start.host.lead}</p>
      <dl className="facts">
        <dt>{t.common.configLabel}</dt>
        <dd className="mono ellipsis" title={configPath}>
          {configPath}
        </dd>
        <dt>{t.start.host.stateLabel}</dt>
        <dd>{exists ? t.start.host.stateExisting : t.start.host.stateNew}</dd>
      </dl>
      <label className="field field--check">
        <input
          type="checkbox"
          checked={upnp}
          onChange={(event) => setUpnp(event.target.checked)}
        />
        <span>{t.start.host.upnp}</span>
      </label>
      {localError && <p className="error-text">{localError}</p>}
      <div className="row">
        <button type="button" onClick={() => void start()} disabled={busy !== null}>
          {busy ?? (exists ? t.start.host.start : t.start.host.createAndStart)}
        </button>
        <button
          type="button"
          className="button--ghost"
          onClick={() => void pickConfig().catch((e) => setError(errorMessage(e)))}
        >
          {t.common.useAnotherConfig}
        </button>
      </div>
      <p className="muted small">{t.start.host.note}</p>
    </section>
  );
}

function JoinCard({
  paths,
  onStarted,
  setError,
}: {
  paths: ConfigPaths;
  onStarted: () => void;
  setError: (message: string) => void;
}) {
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [localError, setLocalError] = useState<string | null>(null);
  const [overwrite, setOverwrite] = useState(false);
  // 保存済みの設定が無いときは、最初からトークン入力を開いておく
  const [showNewJoin, setShowNewJoin] = useState(!paths.member.exists);

  // 既存の member.toml(または選んだファイル)でそのまま接続する
  const startFrom = async (configPath: string) => {
    setBusy(t.start.join.starting);
    setLocalError(null);
    try {
      await api.startMember(configPath);
      onStarted();
    } catch (e) {
      setLocalError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  const pickConfig = async () => {
    const picked = await open({
      multiple: false,
      directory: false,
      filters: [{ name: t.common.configFilter, extensions: ["toml"] }],
    });
    if (typeof picked === "string") await startFrom(picked);
  };

  // 招待トークンから新しく参加設定を作って接続する
  const join = async () => {
    setLocalError(null);
    try {
      setBusy(t.start.join.creating);
      const result = await api.joinNetwork(token.trim(), overwrite);
      setBusy(t.start.join.starting);
      await api.startMember(result.configPath);
      onStarted();
    } catch (e) {
      const message = errorMessage(e);
      setLocalError(message);
      // 既存の member.toml があるときは、上書きの意思を確認してから再実行させる。
      // これは i18n の表示文言ではなく、デーモン(Rust 側)が返すエラー文への
      // マッチング。多言語化する場合はバックエンド側のエラーを構造化してから
      // 判定する必要がある(将来対応)
      if (message.includes("既に存在します")) setOverwrite(true);
    } finally {
      setBusy(null);
    }
  };

  return (
    <section className="card">
      <h2>{t.start.join.title}</h2>
      <p className="muted">{t.start.join.lead}</p>

      {paths.member.exists && (
        <div className="start__section">
          <h3 className="subhead">{t.start.join.savedHead}</h3>
          <dl className="facts">
            <dt>{t.common.configLabel}</dt>
            <dd className="mono ellipsis" title={paths.member.path}>
              {paths.member.path}
            </dd>
          </dl>
          <div className="row">
            <button
              type="button"
              onClick={() => void startFrom(paths.member.path)}
              disabled={busy !== null}
            >
              {busy ?? t.start.join.reconnect}
            </button>
            <button
              type="button"
              className="button--ghost"
              onClick={() =>
                void pickConfig().catch((e) => setError(errorMessage(e)))
              }
            >
              {t.common.useAnotherConfig}
            </button>
          </div>
        </div>
      )}

      <div className="start__section">
        {paths.member.exists ? (
          <button
            type="button"
            className="button--link"
            onClick={() => setShowNewJoin((v) => !v)}
          >
            {showNewJoin ? t.start.join.toggleClose : t.start.join.toggleOpen}
          </button>
        ) : (
          <h3 className="subhead">{t.start.join.newHead}</h3>
        )}

        {showNewJoin && (
          <>
            <p className="muted small">{t.start.join.tokenHint}</p>
            <textarea
              className="token"
              rows={3}
              placeholder={t.start.join.tokenPlaceholder}
              value={token}
              onChange={(event) => setToken(event.target.value)}
            />
            {overwrite && (
              <label className="field field--check">
                <input
                  type="checkbox"
                  checked={overwrite}
                  onChange={(event) => setOverwrite(event.target.checked)}
                />
                <span>{t.start.join.overwrite}</span>
              </label>
            )}
            <button
              type="button"
              onClick={() => void join()}
              disabled={busy !== null || token.trim() === ""}
            >
              {busy ?? t.start.join.submit}
            </button>
          </>
        )}
      </div>

      {localError && <p className="error-text">{localError}</p>}
    </section>
  );
}
