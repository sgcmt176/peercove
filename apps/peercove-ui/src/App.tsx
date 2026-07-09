import { useCallback, useEffect, useRef, useState } from "react";
import { Connection, Member, api, errorMessage, stateLabel } from "./ipc";
import { t } from "./i18n";
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
              settingsConfig ? t.header.settings : t.header.settingsUnavailable
            }
            disabled={settingsConfig === null}
            onClick={() => setDialog("settings")}
          >
            ⚙
          </button>
          <button
            type="button"
            className="button--icon"
            title={t.header.logs}
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
        <p className="muted">{t.state.connectingDaemon}</p>
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

      <footer className="app__footer muted small">{t.footer}</footer>
    </main>
  );
}

function ConnectionBadge({ connection }: { connection: Connection }) {
  if (connection.kind !== "ok") {
    return <span className="badge badge--off">{t.state.daemonDisconnected}</span>;
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
      <h2>{t.daemonUnreachable.title}</h2>
      <p>{t.daemonUnreachable.body}</p>
      <pre>
        <code>{t.daemonUnreachable.command}</code>
      </pre>
      <p className="muted">{t.daemonUnreachable.platforms}</p>
      <details>
        <summary>{t.daemonUnreachable.details}</summary>
        <pre className="error-detail">{message}</pre>
      </details>
      <button type="button" onClick={onRetry}>
        {t.common.retry}
      </button>
    </section>
  );
}
