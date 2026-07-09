import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { ConfigPaths, api, errorMessage } from "../ipc";

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
        <h2>設定の場所を特定できません</h2>
        <p className="error-text">{error}</p>
      </section>
    );
  }
  if (!paths) return <p className="muted">読み込み中…</p>;

  return (
    <>
      <p className="muted start__intro">
        このネットワークの<strong>中心（ホスト）になる</strong>か、
        既存のネットワークに<strong>参加する</strong>かを選んでください。
        どちらも「新しく始める」と「保存済みの設定で再開する」を選べます。
      </p>
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
      filters: [{ name: "PeerCove の設定", extensions: ["toml"] }],
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
        setBusy("ネットワークを作成中…");
        const created = await api.initHost(false);
        path = created.configPath;
        setConfigPath(path);
        setExists(true);
      }
      setBusy("トンネルを開始中…");
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
      <h2>ホストを始める</h2>
      <p className="muted">
        あなたの PC がネットワークの中心になります。参加者を招待できます。
      </p>
      <dl className="facts">
        <dt>設定</dt>
        <dd className="mono ellipsis" title={configPath}>
          {configPath}
        </dd>
        <dt>状態</dt>
        <dd>
          {exists ? "既存のネットワークを使います" : "新しく作成します（鍵とアドレスを自動生成）"}
        </dd>
      </dl>
      <label className="field field--check">
        <input
          type="checkbox"
          checked={upnp}
          onChange={(event) => setUpnp(event.target.checked)}
        />
        <span>
          ルーターのポートを自動で開ける（UPnP）
          <small className="muted"> — 別ネットワークの人を招くとき必要</small>
        </span>
      </label>
      {localError && <p className="error-text">{localError}</p>}
      <div className="row">
        <button type="button" onClick={() => void start()} disabled={busy !== null}>
          {busy ?? (exists ? "開始" : "作成して開始")}
        </button>
        <button
          type="button"
          className="button--ghost"
          onClick={() => void pickConfig().catch((e) => setError(errorMessage(e)))}
        >
          別の設定ファイルを使う
        </button>
      </div>
      <p className="muted small">
        トンネルの操作には管理者権限のデーモンが必要です（このアプリ自体は通常権限で動きます）。
      </p>
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
    setBusy("トンネルを開始中…");
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
      filters: [{ name: "PeerCove の設定", extensions: ["toml"] }],
    });
    if (typeof picked === "string") await startFrom(picked);
  };

  // 招待トークンから新しく参加設定を作って接続する
  const join = async () => {
    setLocalError(null);
    try {
      setBusy("参加設定を作成中…");
      const result = await api.joinNetwork(token.trim(), overwrite);
      setBusy("トンネルを開始中…");
      await api.startMember(result.configPath);
      onStarted();
    } catch (e) {
      const message = errorMessage(e);
      setLocalError(message);
      // 既存の member.toml があるときは、上書きの意思を確認してから再実行させる
      if (message.includes("既に存在します")) setOverwrite(true);
    } finally {
      setBusy(null);
    }
  };

  return (
    <section className="card">
      <h2>参加する</h2>
      <p className="muted">
        ほかの人がホストするネットワークにメンバーとして加わります。
      </p>

      {paths.member.exists && (
        <div className="start__section">
          <h3 className="subhead">保存済みの設定で再接続</h3>
          <dl className="facts">
            <dt>設定</dt>
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
              {busy ?? "前回のネットワークに再接続"}
            </button>
            <button
              type="button"
              className="button--ghost"
              onClick={() =>
                void pickConfig().catch((e) => setError(errorMessage(e)))
              }
            >
              別の設定ファイルを使う
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
            {showNewJoin
              ? "新しい招待での参加を閉じる"
              : "別のネットワークに新しく参加する（招待トークン）"}
          </button>
        ) : (
          <h3 className="subhead">招待トークンで新しく参加</h3>
        )}

        {showNewJoin && (
          <>
            <p className="muted small">
              ホストから受け取った招待トークン（<code>pcv1.</code> で始まる
              文字列）を貼り付けてください。
            </p>
            <textarea
              className="token"
              rows={3}
              placeholder="pcv1.…"
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
                <span>既存の参加設定を上書きする</span>
              </label>
            )}
            <button
              type="button"
              onClick={() => void join()}
              disabled={busy !== null || token.trim() === ""}
            >
              {busy ?? "参加する"}
            </button>
          </>
        )}
      </div>

      {localError && <p className="error-text">{localError}</p>}
    </section>
  );
}
