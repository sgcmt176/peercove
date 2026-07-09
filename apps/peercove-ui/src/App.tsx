import { useCallback, useEffect, useRef, useState } from "react";
import { Connection, Member, NetworkInfo, api, errorMessage } from "./ipc";
import { t } from "./i18n";
import { diffMembers, notifyMemberEvents } from "./notify";
import { NetworksView } from "./components/NetworksView";
import { TunnelView } from "./components/TunnelView";
import { LogsDialog } from "./components/LogsDialog";
import { SettingsDialog } from "./components/SettingsDialog";

/** デーモンの状態を取りに行く間隔。CLI の status(5 秒)より短くしておく。 */
const POLL_INTERVAL_MS = 2000;

export default function App() {
  const [connection, setConnection] = useState<Connection>({
    kind: "connecting",
  });
  /** 設定済みネットワークの一覧(M3-0c)。 */
  const [networks, setNetworks] = useState<NetworkInfo[]>([]);
  const [dialog, setDialog] = useState<"logs" | null>(null);
  /** 設定ダイアログの対象(ネットワークごと — カード/詳細の「設定」から)。 */
  const [settingsFor, setSettingsFor] = useState<string | null>(null);
  /** 詳細表示中のネットワーク(configPath)。null なら一覧。 */
  const [openConfig, setOpenConfig] = useState<string | null>(null);

  // 通知の差分計算に使う前回の台帳(ネットワークごと)。レンダー非依存なので ref
  const previousMembers = useRef<Map<string, Member[]>>(new Map());

  const refresh = useCallback(async () => {
    try {
      const status = await api.daemonStatus();
      setConnection({ kind: "ok", status });
      // ネットワークごとに前回の台帳と比べて参加・切断を通知する
      const seen = new Set<string>();
      for (const tunnel of status.tunnels) {
        seen.add(tunnel.config);
        void notifyMemberEvents(
          diffMembers(
            previousMembers.current.get(tunnel.config) ?? null,
            tunnel.members,
            tunnel.address,
          ),
          tunnel.network,
        );
        previousMembers.current.set(tunnel.config, tunnel.members);
      }
      // 止まったネットワークは次回接続を「初回」に戻す(全員分の通知を防ぐ)
      for (const key of [...previousMembers.current.keys()]) {
        if (!seen.has(key)) previousMembers.current.delete(key);
      }
    } catch (error) {
      setConnection({ kind: "unreachable", message: errorMessage(error) });
      previousMembers.current.clear();
    }
  }, []);

  const refreshNetworks = useCallback(() => {
    api
      .listNetworks()
      .then(setNetworks)
      .catch(() => setNetworks([]));
  }, []);

  useEffect(() => {
    void refresh();
    const timer = setInterval(() => void refresh(), POLL_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [refresh]);

  useEffect(() => {
    refreshNetworks();
  }, [refreshNetworks, connection.kind]);

  const changed = () => {
    void refresh();
    refreshNetworks();
  };

  const tunnels = connection.kind === "ok" ? connection.status.tunnels : [];
  const openTunnel =
    openConfig === null
      ? null
      : (tunnels.find((tun) => tun.config === openConfig) ?? null);

  return (
    <main className="app">
      <header className="app__header">
        <h1>PeerCove</h1>
        <div className="app__actions">
          <ConnectionBadge connection={connection} />
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

      {connection.kind === "ok" && connection.status.daemonOutdated && (
        <section className="card card--error">
          <h2>{t.daemonOutdated.title}</h2>
          <p>{t.daemonOutdated.body}</p>
          <p className="muted small mono">{t.daemonOutdated.windows}</p>
          <p className="muted small mono">{t.daemonOutdated.linux}</p>
        </section>
      )}

      {connection.kind === "unreachable" ? (
        <DaemonUnreachable
          message={connection.message}
          onRetry={() => void refresh()}
        />
      ) : connection.kind === "connecting" ? (
        <p className="muted">{t.state.connectingDaemon}</p>
      ) : openTunnel !== null ? (
        <>
          <button
            type="button"
            className="button--link"
            onClick={() => setOpenConfig(null)}
          >
            {t.networks.back}
          </button>
          <TunnelView
            tunnel={openTunnel}
            onChanged={changed}
            onSettings={() => setSettingsFor(openTunnel.config)}
          />
        </>
      ) : (
        <NetworksView
          networks={networks}
          tunnels={tunnels}
          onChanged={changed}
          onOpen={(configPath) => setOpenConfig(configPath)}
          onSettings={(configPath) => setSettingsFor(configPath)}
        />
      )}

      {dialog === "logs" && <LogsDialog onClose={() => setDialog(null)} />}
      {settingsFor && (
        <SettingsDialog
          configPath={settingsFor}
          onClose={() => {
            setSettingsFor(null);
            changed();
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
  const count = connection.status.tunnels.length;
  return (
    <span className={count === 0 ? "badge" : "badge badge--on"}>
      {count === 0 ? t.state.idle : t.state.runningCount(count)}
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
