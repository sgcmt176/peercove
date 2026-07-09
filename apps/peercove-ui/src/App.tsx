import { useCallback, useEffect, useRef, useState } from "react";
import { Connection, Member, api, errorMessage, stateLabel } from "./ipc";
import { diffMembers, notifyMemberEvents } from "./notify";
import { StartView } from "./components/StartView";
import { TunnelView } from "./components/TunnelView";
import { LogsDialog } from "./components/LogsDialog";
import { SettingsDialog } from "./components/SettingsDialog";

/** デーモンの状態を取りに行く間隔。CLI の status(5 秒)より短くしておく。 */
const POLL_INTERVAL_MS = 2000;

export default function App() {
  const [connection, setConnection] = useState<Connection>({
    kind: "connecting",
  });
  const [dialog, setDialog] = useState<"logs" | "settings" | null>(null);
  /** 設定編集の対象。トンネル稼働中はその設定、待機中は既定のホスト/参加設定。 */
  const [idleConfig, setIdleConfig] = useState<string | null>(null);

  // 通知の差分計算に使う前回の台帳。レンダーに関係しないので ref に置く
  const previousMembers = useRef<Member[] | null>(null);

  const refresh = useCallback(async () => {
    try {
      const status = await api.daemonStatus();
      setConnection({ kind: "ok", status });
      const members = status.tunnel?.members ?? null;
      if (members === null) {
        // 切断したら次回の接続を「初回」に戻す(全員分の通知が鳴るのを防ぐ)
        previousMembers.current = null;
      } else {
        const selfAddress = status.tunnel?.address ?? null;
        void notifyMemberEvents(
          diffMembers(previousMembers.current, members, selfAddress),
        );
        previousMembers.current = members;
      }
    } catch (error) {
      setConnection({ kind: "unreachable", message: errorMessage(error) });
      previousMembers.current = null;
    }
  }, []);

  useEffect(() => {
    void refresh();
    const timer = setInterval(() => void refresh(), POLL_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [refresh]);

  // 待機中に設定を編集できるよう、既定の設定ファイルの所在を調べておく
  useEffect(() => {
    api
      .configPaths()
      .then((paths) => {
        const slot = paths.host.exists
          ? paths.host
          : paths.member.exists
            ? paths.member
            : null;
        setIdleConfig(slot?.path ?? null);
      })
      .catch(() => setIdleConfig(null));
  }, [connection.kind]);

  const tunnelConfig =
    connection.kind === "ok" ? (connection.status.tunnel?.config ?? null) : null;
  const settingsConfig = tunnelConfig ?? idleConfig;

  return (
    <main className="app">
      <header className="app__header">
        <h1>PeerCove</h1>
        <div className="app__actions">
          <ConnectionBadge connection={connection} />
          <button
            type="button"
            className="button--icon"
            title={
              settingsConfig
                ? "設定"
                : "設定ファイルがまだありません（ホストを始めるか参加してください）"
            }
            disabled={settingsConfig === null}
            onClick={() => setDialog("settings")}
          >
            ⚙
          </button>
          <button
            type="button"
            className="button--icon"
            title="デーモンのログ"
            disabled={connection.kind === "unreachable"}
            onClick={() => setDialog("logs")}
          >
            ☰
          </button>
        </div>
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

      {dialog === "logs" && <LogsDialog onClose={() => setDialog(null)} />}
      {dialog === "settings" && settingsConfig && (
        <SettingsDialog
          configPath={settingsConfig}
          onClose={() => {
            setDialog(null);
            void refresh();
          }}
        />
      )}

      <footer className="app__footer muted small">
        wintun.dll © WireGuard LLC — Prebuilt Binaries License の下で無改変同梱
        （インストール先の <span className="mono">wintun-LICENSE.txt</span> を参照）。
      </footer>
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
