import { useCallback, useEffect, useState } from "react";
import { Connection, api, errorMessage, stateLabel } from "./ipc";
import { StartView } from "./components/StartView";
import { TunnelView } from "./components/TunnelView";

/** デーモンの状態を取りに行く間隔。CLI の status(5 秒)より短くしておく。 */
const POLL_INTERVAL_MS = 2000;

export default function App() {
  const [connection, setConnection] = useState<Connection>({
    kind: "connecting",
  });

  const refresh = useCallback(async () => {
    try {
      setConnection({ kind: "ok", status: await api.daemonStatus() });
    } catch (error) {
      setConnection({ kind: "unreachable", message: errorMessage(error) });
    }
  }, []);

  useEffect(() => {
    void refresh();
    const timer = setInterval(() => void refresh(), POLL_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [refresh]);

  return (
    <main className="app">
      <header className="app__header">
        <h1>PeerCove</h1>
        <ConnectionBadge connection={connection} />
      </header>

      {connection.kind === "unreachable" ? (
        <DaemonUnreachable
          message={connection.message}
          onRetry={() => void refresh()}
        />
      ) : connection.kind === "connecting" ? (
        <p className="muted">デーモンに接続しています…</p>
      ) : connection.status.state === "idle" || connection.status.tunnel === null ? (
        <StartView onStarted={() => void refresh()} />
      ) : (
        <TunnelView status={connection.status} onChanged={() => void refresh()} />
      )}
    </main>
  );
}

function ConnectionBadge({ connection }: { connection: Connection }) {
  if (connection.kind !== "ok") {
    return <span className="badge badge--off">デーモン未接続</span>;
  }
  const { state } = connection.status;
  return (
    <span className={state === "idle" ? "badge" : "badge badge--on"}>
      {stateLabel(state)}
    </span>
  );
}

function DaemonUnreachable({
  message,
  onRetry,
}: {
  message: string;
  onRetry: () => void;
}) {
  return (
    <section className="card card--error">
      <h2>デーモンに接続できません</h2>
      <p>
        トンネルの操作には管理者権限のデーモンが必要です。ターミナルで次を実行して
        ください:
      </p>
      <pre>
        <code>peercove-poc daemon run</code>
      </pre>
      <p className="muted">Windows は管理者ターミナル、Linux は sudo で実行します。</p>
      <details>
        <summary>詳細</summary>
        <pre className="error-detail">{message}</pre>
      </details>
      <button type="button" onClick={onRetry}>
        再試行
      </button>
    </section>
  );
}
