import { useCallback, useEffect, useRef, useState } from "react";
import { onOpenUrl } from "@tauri-apps/plugin-deep-link";
import { getVersion } from "@tauri-apps/api/app";
import { Connection, Member, NetworkInfo, Transfer, api, errorMessage } from "./ipc";
import { t } from "./i18n";
import {
  diffMembers,
  diffTransfers,
  notifyChatEvents,
  notifyFileEvents,
  notifyMemberEvents,
} from "./notify";
import { clearChat, syncChat, totalUnread } from "./chat";
import { clearHistory, recordStatus } from "./history";
import { Theme, applyTheme, loadTheme, nextTheme } from "./theme";
import { NetworksView } from "./components/NetworksView";
import { TunnelView } from "./components/TunnelView";
import { LogsView } from "./components/LogsDialog";
import { NetworkSettingsView } from "./components/SettingsDialog";
import { AppSettingsView } from "./components/PrefsDialog";

/** サイドバーで選ぶ表示。openConfig の有無で有効な値が決まる(M3-16)。 */
type View =
  | "networks"
  | "app-settings"
  | "logs"
  | "members"
  | "chat"
  | "stats"
  | "inbox"
  | "dns"
  | "subnets"
  | "net-settings";

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
  /** 詳細/設定を開いているネットワーク(configPath)。null なら一覧側。 */
  const [openConfig, setOpenConfig] = useState<string | null>(null);
  /** サイドバーで選んでいる表示(M3-16)。 */
  const [view, setView] = useState<View>("networks");
  /** アプリのバージョン(サイドバー下部に出す)。取得できなければ空。 */
  const [version, setVersion] = useState("");
  /** 詳細ヘッダーの「切断」実行中。 */
  const [disconnecting, setDisconnecting] = useState(false);
  /**
   * チャットで開く相手(メンバー行の 💬 から。1:1 会話を選ぶ)。オブジェクトで
   * 包むのは同じ相手を続けてクリックしても再選択されるようにするため。
   */
  const [chatTarget, setChatTarget] = useState<{ peer: string } | null>(null);
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
          // 参加フォームのある一覧画面へ
          setOpenConfig(null);
          setView("networks");
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

  // アプリのバージョン(サイドバー下部の表示用)。1 回だけ取れば十分
  useEffect(() => {
    void getVersion()
      .then(setVersion)
      .catch(() => {});
  }, []);

  const changed = () => {
    void refresh();
    refreshNetworks();
  };

  const tunnels = connection.kind === "ok" ? connection.status.tunnels : [];
  const openTunnel =
    openConfig === null
      ? null
      : (tunnels.find((tun) => tun.config === openConfig) ?? null);
  const openInfo =
    openConfig === null
      ? null
      : (networks.find((n) => n.configPath === openConfig) ?? null);

  // ネットワークを開いていて、かつデーモンに接続できている(詳細/設定を出す)
  const detail = connection.kind === "ok" && openConfig !== null;
  const detailName = openTunnel?.network ?? openInfo?.name ?? "";
  const detailHost = (openTunnel?.role ?? openInfo?.role) === "hosting";
  const chatUnread = openTunnel ? totalUnread(openTunnel) : 0;
  const inboxBadge = openTunnel
    ? openTunnel.transfers.filter((tr) => !tr.done).length
    : 0;

  // 稼働していないネットワークの「稼働専用ビュー」に留まらないよう調整する
  // (切断・削除後)。設定ページは停止中でも見られるのでそのまま
  useEffect(() => {
    if (connection.kind !== "ok" || openConfig === null) return;
    const running = tunnels.some((tun) => tun.config === openConfig);
    if (running || view === "net-settings" || view === "logs") return;
    if (networks.some((n) => n.configPath === openConfig)) {
      setView("net-settings");
    } else {
      setOpenConfig(null);
      setView("networks");
    }
  }, [connection.kind, openConfig, tunnels, networks, view]);

  // ネットワークを開く(一覧カードの「開く」)= メンバービューから始める
  const openNetwork = (configPath: string) => {
    setOpenConfig(configPath);
    setView("members");
  };
  // ネットワーク設定ページを開く(一覧カードの「設定」= 停止中でも可)
  const openNetworkSettings = (configPath: string) => {
    setOpenConfig(configPath);
    setView("net-settings");
  };
  const backToList = () => {
    setOpenConfig(null);
    setView("networks");
  };
  const disconnect = async () => {
    if (openConfig === null) return;
    setDisconnecting(true);
    try {
      await api.stopTunnel(openConfig);
    } catch {
      // 表示は次のポーリングで更新される
    }
    setDisconnecting(false);
    backToList();
    changed();
  };

  return (
    <div className="app">
      <nav className="sidebar">
        <div className="sidebar__brand">
          <span className="app__logo" aria-hidden>
            P
          </span>
          <span className="sidebar__brand-name">PeerCove</span>
        </div>
        <ul className="sidebar__nav">
          {detail ? (
            <>
              <SidebarItem
                icon="👥"
                label={t.sidebar.members}
                active={view === "members"}
                disabled={openTunnel === null}
                onClick={() => setView("members")}
              />
              <SidebarItem
                icon="💬"
                label={t.sidebar.chat}
                active={view === "chat"}
                disabled={openTunnel === null}
                badge={chatUnread}
                onClick={() => setView("chat")}
              />
              <SidebarItem
                icon="📊"
                label={t.sidebar.stats}
                active={view === "stats"}
                disabled={openTunnel === null}
                onClick={() => setView("stats")}
              />
              <SidebarItem
                icon="📥"
                label={t.sidebar.inbox}
                active={view === "inbox"}
                disabled={openTunnel === null}
                badge={inboxBadge}
                onClick={() => setView("inbox")}
              />
              <SidebarItem
                icon="🌐"
                label={t.sidebar.dns}
                active={view === "dns"}
                disabled={openTunnel === null}
                onClick={() => setView("dns")}
              />
              {detailHost && (
                <SidebarItem
                  icon="🖧"
                  label={t.sidebar.subnets}
                  active={view === "subnets"}
                  disabled={openTunnel === null}
                  onClick={() => setView("subnets")}
                />
              )}
              <SidebarItem
                icon="🧾"
                label={t.sidebar.logs}
                active={view === "logs"}
                onClick={() => setView("logs")}
              />
              <SidebarItem
                icon="⚙"
                label={t.sidebar.settings}
                active={view === "net-settings"}
                onClick={() => setView("net-settings")}
              />
            </>
          ) : (
            <>
              <SidebarItem
                icon="🖧"
                label={t.sidebar.networks}
                active={view === "networks"}
                onClick={backToList}
              />
              <SidebarItem
                icon="🧾"
                label={t.sidebar.logs}
                active={view === "logs"}
                onClick={() => {
                  setOpenConfig(null);
                  setView("logs");
                }}
              />
              <SidebarItem
                icon="⚙"
                label={t.sidebar.settings}
                active={view === "app-settings"}
                onClick={() => {
                  setOpenConfig(null);
                  setView("app-settings");
                }}
              />
            </>
          )}
        </ul>
        <div className="sidebar__foot">
          <ConnectionStatus connection={connection} />
          <div className="sidebar__foot-row">
            {version && (
              <span className="sidebar__version muted small">
                {t.sidebar.version(version)}
              </span>
            )}
            <span className="sidebar__foot-actions">
              <button
                type="button"
                className="button--icon"
                title={t.sidebar.theme(theme)}
                onClick={() => setTheme(nextTheme(theme))}
              >
                {theme === "light" ? "☀" : "☾"}
              </button>
            </span>
          </div>
        </div>
      </nav>

      <main className="app__main">
        {connection.kind === "ok" && connection.status.daemonOutdated && (
          <section className="card card--error">
            <h2>{t.daemonOutdated.title}</h2>
            <p>{t.daemonOutdated.body}</p>
            <p className="muted small mono">{t.daemonOutdated.windows}</p>
            <p className="muted small mono">{t.daemonOutdated.linux}</p>
          </section>
        )}

        {detail && (
          <DetailHeader
            name={detailName}
            isHost={detailHost}
            running={openTunnel !== null}
            disconnecting={disconnecting}
            onBack={backToList}
            onDisconnect={() => void disconnect()}
          />
        )}
        {detail && openTunnel?.removed && (
          <section className="card card--error">
            <h2>{t.tunnel.removedTitle}</h2>
            <p>{t.tunnel.removedBody}</p>
          </section>
        )}

        <div
          className={
            view === "logs" || (detail && view === "chat")
              ? "app__content app__content--flush"
              : "app__content"
          }
        >
          {connection.kind === "unreachable" ? (
            <DaemonUnreachable
              message={connection.message}
              onRetry={() => void refresh()}
            />
          ) : connection.kind === "connecting" ? (
            <p className="muted">{t.state.connectingDaemon}</p>
          ) : view === "logs" ? (
            <LogsView />
          ) : !detail ? (
            view === "app-settings" ? (
              <AppSettingsView />
            ) : (
              <NetworksView
                networks={networks}
                tunnels={tunnels}
                onChanged={changed}
                onOpen={openNetwork}
                onSettings={openNetworkSettings}
                pendingJoin={pendingJoin}
                onPendingJoinHandled={() => setPendingJoin(null)}
              />
            )
          ) : view === "net-settings" ? (
            <NetworkSettingsView configPath={openConfig!} />
          ) : openTunnel !== null ? (
            <TunnelView
              tunnel={openTunnel}
              view={
                view === "chat" ||
                view === "stats" ||
                view === "inbox" ||
                view === "dns" ||
                view === "subnets"
                  ? view
                  : "members"
              }
              chatTarget={chatTarget}
              onOpenChat={(peer) => {
                setChatTarget({ peer });
                setView("chat");
              }}
              onView={setView}
              onChanged={changed}
            />
          ) : (
            <p className="muted">{t.tunnel.ledgerPending}</p>
          )}
        </div>
      </main>
    </div>
  );
}

/** ネットワーク詳細・設定ページのヘッダー(戻る・名前・状態・切断)。 */
function DetailHeader({
  name,
  isHost,
  running,
  disconnecting,
  onBack,
  onDisconnect,
}: {
  name: string;
  isHost: boolean;
  running: boolean;
  disconnecting: boolean;
  onBack: () => void;
  onDisconnect: () => void;
}) {
  return (
    <div className="detail__head">
      <button
        type="button"
        className="button--icon detail__back"
        title={t.tunnel.back}
        onClick={onBack}
      >
        ←
      </button>
      <h2 className="detail__title">{name}</h2>
      <span className={running ? "badge badge--on" : "badge"}>
        {running ? t.tunnel.connected : t.networks.stopped}
      </span>
      <span className="tag">
        {isHost ? t.networks.roleHost : t.networks.roleMember}
      </span>
      {running && (
        <div className="detail__actions">
          <button
            type="button"
            className="button--ghost button--ghost-danger"
            onClick={onDisconnect}
            disabled={disconnecting}
          >
            ⏻ {t.tunnel.disconnect}
          </button>
        </div>
      )}
    </div>
  );
}

/** サイドバーのナビ項目(アイコン + ラベル。狭い幅ではアイコンだけになる)。 */
function SidebarItem({
  icon,
  label,
  active,
  disabled,
  badge,
  onClick,
}: {
  icon: string;
  label: string;
  active: boolean;
  disabled?: boolean;
  /** 0 なら出さない未読・件数バッジ。 */
  badge?: number;
  onClick: () => void;
}) {
  return (
    <li>
      <button
        type="button"
        className={
          active ? "sidebar__item sidebar__item--active" : "sidebar__item"
        }
        disabled={disabled}
        onClick={onClick}
        title={label}
      >
        <span className="sidebar__icon" aria-hidden>
          {icon}
        </span>
        <span className="sidebar__label">{label}</span>
        {badge !== undefined && badge > 0 && (
          <span className="sidebar__badge">{badge}</span>
        )}
      </button>
    </li>
  );
}

/** サイドバー下部のデーモン接続状態(ドット + 文字)。 */
function ConnectionStatus({ connection }: { connection: Connection }) {
  const ok = connection.kind === "ok";
  return (
    <span className="sidebar__status">
      <span className={ok ? "dot dot--online" : "dot"} aria-hidden />
      <span className="muted small">
        {ok ? t.sidebar.connected : t.sidebar.disconnected}
      </span>
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
