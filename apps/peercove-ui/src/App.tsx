import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Connection,
  Status,
  formatBytes,
  formatHandshake,
  stateLabel,
} from "./ipc";

/** デーモンの状態を取りに行く間隔。CLI の status と同じ 5 秒より短くしておく。 */
const POLL_INTERVAL_MS = 2000;

export default function App() {
  const [connection, setConnection] = useState<Connection>({
    kind: "connecting",
  });

  const refresh = useCallback(async () => {
    try {
      const status = await invoke<Status>("daemon_status");
      setConnection({ kind: "ok", status });
    } catch (error) {
      setConnection({ kind: "unreachable", message: String(error) });
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
      ) : (
        <StatusView status={connection.status} />
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

function StatusView({ status }: { status: Status }) {
  if (status.state === "idle" || status.tunnel === null) {
    return (
      <section className="card">
        <h2>待機中</h2>
        <p>
          トンネルは動いていません。ホストを開始するか、招待トークンで参加すると
          ここに状態が表示されます。
        </p>
        <p className="muted">
          （開始・参加の操作は M2-G3 で追加します。現在は CLI の
          <code> daemon start-host </code>/<code> daemon start-member </code>
          を使ってください）
        </p>
      </section>
    );
  }

  const { tunnel } = status;
  return (
    <>
      <section className="card">
        <h2>{stateLabel(status.state)}</h2>
        <dl className="facts">
          <dt>仮想 IP</dt>
          <dd>{tunnel.address}</dd>
          <dt>設定ファイル</dt>
          <dd className="mono ellipsis" title={tunnel.config}>
            {tunnel.config}
          </dd>
        </dl>
      </section>

      <section className="card">
        <h2>メンバー（{tunnel.members.length}）</h2>
        {tunnel.members.length === 0 ? (
          <p className="muted">
            台帳をまだ受信していません（接続直後は数秒かかります）。
          </p>
        ) : (
          <ul className="members">
            {tunnel.members.map((member) => (
              <li key={member.publicKey} className="member">
                <span
                  className={
                    member.online ? "dot dot--online" : "dot dot--offline"
                  }
                  aria-label={member.online ? "オンライン" : "オフライン"}
                />
                <span className="member__name">
                  {member.name ?? "(名前なし)"}
                </span>
                <span className="mono muted">{member.ip}</span>
                {member.isHost && <span className="tag">host</span>}
              </li>
            ))}
          </ul>
        )}
      </section>

      {tunnel.peers.length > 0 && (
        <section className="card">
          <h2>ピア統計</h2>
          <table className="peers">
            <thead>
              <tr>
                <th>公開鍵</th>
                <th>エンドポイント</th>
                <th>最終ハンドシェイク</th>
                <th>受信</th>
                <th>送信</th>
              </tr>
            </thead>
            <tbody>
              {tunnel.peers.map((peer) => (
                <tr key={peer.publicKey}>
                  <td className="mono ellipsis" title={peer.publicKey}>
                    {peer.publicKey.slice(0, 12)}…
                  </td>
                  <td className="mono">{peer.endpoint ?? "(未接続)"}</td>
                  <td>{formatHandshake(peer.lastHandshakeAgeSecs)}</td>
                  <td>{formatBytes(peer.rxBytes)}</td>
                  <td>{formatBytes(peer.txBytes)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      )}
    </>
  );
}
