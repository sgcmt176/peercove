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
      <HostCard paths={paths} onStarted={onStarted} setError={setError} />
      <JoinCard onStarted={onStarted} />
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

function JoinCard({ onStarted }: { onStarted: () => void }) {
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [overwrite, setOverwrite] = useState(false);

  const join = async () => {
    setError(null);
    try {
      setBusy("参加設定を作成中…");
      const result = await api.joinNetwork(token.trim(), overwrite);
      setBusy("トンネルを開始中…");
      await api.startMember(result.configPath);
      onStarted();
    } catch (e) {
      const message = errorMessage(e);
      setError(message);
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
        ホストから受け取った招待トークン（<code>pcv1.</code> で始まる文字列）を
        貼り付けてください。
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
      {error && <p className="error-text">{error}</p>}
      <button
        type="button"
        onClick={() => void join()}
        disabled={busy !== null || token.trim() === ""}
      >
        {busy ?? "参加する"}
      </button>
    </section>
  );
}
