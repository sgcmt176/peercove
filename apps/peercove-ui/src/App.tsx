import { useCallback, useEffect, useRef, useState } from "react";
import { onOpenUrl } from "@tauri-apps/plugin-deep-link";
import { Connection, Member, NetworkInfo, Transfer, api, errorMessage } from "./ipc";
import { t } from "./i18n";
import {
  diffMembers,
  diffTransfers,
  notifyChatEvents,
  notifyFileEvents,
  notifyMemberEvents,
} from "./notify";
import { clearChat, syncChat } from "./chat";
import { clearHistory, recordStatus } from "./history";
import { Theme, applyTheme, loadTheme, nextTheme } from "./theme";
import { NetworksView } from "./components/NetworksView";
import { TunnelView } from "./components/TunnelView";
import { LogsDialog } from "./components/LogsDialog";
import { SettingsDialog } from "./components/SettingsDialog";

/** デーモンの状態を取りに行く間隔。CLI の status(5 秒)より短くしておく。 */
const POLL_INTERVAL_MS = 2000;

/**
 * 招待ディープリンク `peercove://join?token=…` からトークンを取り出す(M3-5)。
 * 該当しない URL は null(黙って無視する)。
 */
export function parseJoinUrl(url: string): string | null {
  try {
    const parsed = new URL(url);
    if (parsed.protocol !== "peercove:") return null;
    // `peercove://join` はパーサによって hostname になったり pathname に
    // なったりするため両方を見る
    const action = parsed.hostname || parsed.pathname.replace(/^\/+/, "");
    if (action !== "join") return null;
    const token = parsed.searchParams.get("token")?.trim();
    return token ? token : null;
  } catch {
    return null;
  }
}

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
  /**
   * ディープリンクで受けた招待トークン(M3-5)。オブジェクトで包むのは、
   * 同じリンクを 2 回クリックしても再度フォームを開くため(参照が変わる)。
   */
  const [pendingJoin, setPendingJoin] = useState<{ token: string } | null>(null);
  /** 外観テーマ(M3-6)。localStorage に保存され、このマシンだけに効く。 */
  const [theme, setTheme] = useState<Theme>(loadTheme);

  // 通知の差分計算に使う前回の台帳(ネットワークごと)。レンダー非依存なので ref
  const previousMembers = useRef<Map<string, Member[]>>(new Map());
  // ファイル受信通知(M3-9b)の差分計算に使う前回の転送一覧
  const previousTransfers = useRef<Map<string, Transfer[]>>(new Map());

  const refresh = useCallback(async () => {
    try {
      const status = await api.daemonStatus();
      // チャットの差分フェッチ(M3-13b)。描画前に済ませて新着を即表示する。
      // 新しく取れた受信分は OS 通知(いま見ている会話は鳴らさない)
      await Promise.all(
        status.tunnels.map(async (tunnel) => {
          try {
            const fresh = await syncChat(tunnel.config, tunnel.chatSeq);
            void notifyChatEvents(fresh, tunnel, tunnel.members);
          } catch {
            // フェッチ失敗で状態表示を止めない(次のポーリングで再試行)
          }
        }),
      );
      setConnection({ kind: "ok", status });
      recordStatus(status); // スパークライン用の時系列(M3-6)
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
        // ファイルの受信完了を通知する(M3-9b)
        void notifyFileEvents(
          diffTransfers(
            previousTransfers.current.get(tunnel.config) ?? null,
            tunnel.transfers,
          ),
          tunnel.members,
          tunnel.network,
        );
        previousTransfers.current.set(tunnel.config, tunnel.transfers);
      }
      // 止まったネットワークは次回接続を「初回」に戻す(全員分の通知を防ぐ)
      for (const key of [...previousMembers.current.keys()]) {
        if (!seen.has(key)) {
          previousMembers.current.delete(key);
          previousTransfers.current.delete(key);
          clearChat(key);
        }
      }
    } catch (error) {
      setConnection({ kind: "unreachable", message: errorMessage(error) });
      previousMembers.current.clear();
      previousTransfers.current.clear();
      clearHistory();
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

  // 招待ディープリンク(M3-5)。起動時の URL と、稼働中に届いた URL の両方が
  // ここへ来る(single-instance が二重起動分を転送する)
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let closed = false;
    void onOpenUrl((urls) => {
      for (const url of urls) {
        const token = parseJoinUrl(url);
        if (token) {
          setPendingJoin({ token });
          setOpenConfig(null); // 参加フォームのある一覧画面へ
        }
      }
    }).then((fn) => {
      if (closed) fn();
      else unlisten = fn;
    });
    return () => {
      closed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    refreshNetworks();
  }, [refreshNetworks, connection.kind]);

  // テーマの適用(M3-6)。初回マウント時と切替時の両方でここを通る
  useEffect(() => {
    applyTheme(theme);
  }, [theme]);

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
        <div className="app__brand">
          <span className="app__logo" aria-hidden>
            P
          </span>
          <h1>PeerCove</h1>
        </div>
        <div className="app__actions">
          <ConnectionBadge connection={connection} />
          <button
            type="button"
            className="button--icon"
            title={t.header.theme(theme)}
            onClick={() => setTheme(nextTheme(theme))}
          >
            {theme === "light" ? "☀" : "☾"}
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
          pendingJoin={pendingJoin}
          onPendingJoinHandled={() => setPendingJoin(null)}
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
