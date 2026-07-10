import { useEffect, useState } from "react";
import { NetworkInfo, Tunnel, api, errorMessage } from "../ipc";
import { ConfirmModal } from "./Modal";
import { t } from "../i18n";

/**
 * ネットワーク一覧(M3-0c、ADR-0012 §5)。
 *
 * 設定済みネットワーク(listNetworks)と稼働中トンネル(Status.tunnels)を
 * configPath で突き合わせ、カードごとに接続/切断/削除できる。
 * 「+ 追加」からホスト新規作成(名前入力)と招待トークンでの参加ができる。
 * 招待ディープリンク(M3-5)は pendingJoin 経由で参加フォームを事前入力で開く。
 */
export function NetworksView({
  networks,
  tunnels,
  onChanged,
  onOpen,
  onSettings,
  pendingJoin,
  onPendingJoinHandled,
}: {
  networks: NetworkInfo[];
  tunnels: Tunnel[];
  onChanged: () => void;
  /** 稼働中ネットワークの詳細を開く(configPath)。 */
  onOpen: (configPath: string) => void;
  onSettings: (configPath: string) => void;
  /** ディープリンクで受けた招待トークン(M3-5)。 */
  pendingJoin: { token: string } | null;
  onPendingJoinHandled: () => void;
}) {
  const [adding, setAdding] = useState<"host" | "join" | null>(null);
  const [removing, setRemoving] = useState<NetworkInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** 参加フォームへ事前入力するトークン(ディープリンク経由)。 */
  const [prefillToken, setPrefillToken] = useState("");

  useEffect(() => {
    if (pendingJoin) {
      setPrefillToken(pendingJoin.token);
      setAdding("join");
      onPendingJoinHandled();
    }
  }, [pendingJoin, onPendingJoinHandled]);

  const byConfig = new Map(tunnels.map((tun) => [tun.config, tun]));
  // 一覧に無い設定で稼働しているトンネル(CLI で任意パス起動など)も見せる
  const knownPaths = new Set(networks.map((n) => n.configPath));
  const orphans = tunnels.filter((tun) => !knownPaths.has(tun.config));

  const remove = async (network: NetworkInfo) => {
    setBusy(true);
    setError(null);
    try {
      await api.deleteNetwork(network.slug);
      setRemoving(null);
      onChanged();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <section className="card">
        <div className="card__head">
          <h2>{t.networks.listHead}</h2>
          <div className="row">
            <button
              type="button"
              className={adding === "host" ? "button--ghost" : undefined}
              onClick={() => setAdding(adding === "host" ? null : "host")}
            >
              {adding === "host" ? t.networks.addClose : t.networks.addHost}
            </button>
            <button
              type="button"
              className={adding === "join" ? "button--ghost" : undefined}
              onClick={() => setAdding(adding === "join" ? null : "join")}
            >
              {adding === "join" ? t.networks.addClose : t.networks.addJoin}
            </button>
          </div>
        </div>

        {adding === "host" && (
          <CreateHostForm
            onCreated={() => {
              setAdding(null);
              onChanged();
            }}
          />
        )}
        {adding === "join" && (
          <JoinForm
            initialToken={prefillToken}
            onJoined={() => {
              setAdding(null);
              setPrefillToken("");
              onChanged();
            }}
          />
        )}

        {networks.length === 0 && orphans.length === 0 && adding === null && (
          <p className="muted">{t.networks.empty}</p>
        )}
        {error && <p className="error-text">{error}</p>}
      </section>

      {networks.map((network) => (
        <NetworkCard
          key={network.slug}
          network={network}
          tunnel={byConfig.get(network.configPath) ?? null}
          onChanged={onChanged}
          onOpen={onOpen}
          onSettings={onSettings}
          onRemove={() => setRemoving(network)}
        />
      ))}

      {orphans.map((tunnel) => (
        <NetworkCard
          key={tunnel.config}
          network={{
            slug: tunnel.config,
            name: `${tunnel.network} ${t.networks.external}`,
            role: tunnel.role,
            configPath: tunnel.config,
            address: tunnel.address,
          }}
          tunnel={tunnel}
          onChanged={onChanged}
          onOpen={onOpen}
          onSettings={onSettings}
          onRemove={null}
        />
      ))}

      {removing && (
        <ConfirmModal
          title={t.networks.deleteTitle}
          confirmLabel={t.networks.deleteConfirm}
          busy={busy}
          onClose={() => setRemoving(null)}
          onConfirm={() => void remove(removing)}
          message={t.networks.deleteMessage(removing.name)}
        />
      )}
    </>
  );
}

function NetworkCard({
  network,
  tunnel,
  onChanged,
  onOpen,
  onSettings,
  onRemove,
}: {
  network: NetworkInfo;
  tunnel: Tunnel | null;
  onChanged: () => void;
  onOpen: (configPath: string) => void;
  onSettings: (configPath: string) => void;
  /** null なら削除ボタンを出さない(一覧外の設定)。 */
  onRemove: (() => void) | null;
}) {
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [upnp, setUpnp] = useState(true);
  const running = tunnel !== null;
  const isHost = network.role === "hosting";
  const onlineCount =
    tunnel?.members.filter((member) => member.online).length ?? 0;

  const start = async () => {
    setBusy(t.networks.connecting);
    setError(null);
    try {
      if (isHost) {
        await api.startHost(network.configPath, upnp);
      } else {
        await api.startMember(network.configPath);
      }
      onChanged();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  const stop = async () => {
    setBusy(t.networks.disconnecting);
    setError(null);
    try {
      await api.stopTunnel(tunnel?.config ?? network.configPath);
      onChanged();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <section className={running ? "card network network--on" : "card network"}>
      <div className="card__head">
        <h2 className="network__title">
          <span
            className={running ? "dot dot--online" : "dot dot--offline"}
            aria-label={running ? t.networks.running : t.networks.stopped}
          />
          {network.name}
          <span className="tag">
            {isHost ? t.networks.roleHost : t.networks.roleMember}
          </span>
          {tunnel?.removed && (
            <span className="tag tag--danger">{t.networks.removedBadge}</span>
          )}
        </h2>
        <div className="row">
          {running ? (
            <>
              <button type="button" onClick={() => onOpen(tunnel.config)}>
                {t.networks.open}
              </button>
              <button
                type="button"
                className="button--ghost"
                onClick={() => void stop()}
                disabled={busy !== null}
              >
                {busy ?? t.tunnel.disconnect}
              </button>
            </>
          ) : (
            <button
              type="button"
              onClick={() => void start()}
              disabled={busy !== null}
            >
              {busy ?? t.networks.connect}
            </button>
          )}
        </div>
      </div>

      <dl className="facts">
        <dt>{t.common.virtualIp}</dt>
        <dd className="mono">{running ? tunnel.address : network.address}</dd>
        <dt>{running ? t.networks.running : t.networks.stopped}</dt>
        <dd>
          {running
            ? t.networks.membersOnline(onlineCount)
            : ""}
        </dd>
      </dl>

      {!running && isHost && (
        <label className="field field--check">
          <input
            type="checkbox"
            checked={upnp}
            onChange={(event) => setUpnp(event.target.checked)}
          />
          <span>{t.start.host.upnp}</span>
        </label>
      )}

      {error && <p className="error-text">{error}</p>}

      <div className="row network__foot">
        <button
          type="button"
          className="button--link"
          onClick={() => onSettings(network.configPath)}
        >
          {t.networks.settings}
        </button>
        {onRemove && (
          <button
            type="button"
            className="button--link button--link-danger"
            onClick={onRemove}
            disabled={running}
            title={running ? t.networks.running : undefined}
          >
            {t.networks.delete}
          </button>
        )}
      </div>
    </section>
  );
}

/** ホスト新規作成フォーム(名前を付けて init → そのまま接続はしない)。 */
function CreateHostForm({ onCreated }: { onCreated: () => void }) {
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const create = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.initHost(name.trim() || t.networks.namePlaceholder, false);
      onCreated();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="start__section">
      <label className="field">
        <span>{t.networks.nameLabel}</span>
        <input
          value={name}
          autoFocus
          placeholder={t.networks.namePlaceholder}
          onChange={(event) => setName(event.target.value)}
        />
        <small className="muted">{t.networks.nameHint}</small>
      </label>
      {error && <p className="error-text">{error}</p>}
      <button type="button" onClick={() => void create()} disabled={busy}>
        {busy ? t.networks.creating : t.networks.create}
      </button>
      <p className="muted small">{t.start.host.note}</p>
    </div>
  );
}

/** 招待トークンでの参加フォーム(StartView から移設)。 */
function JoinForm({
  initialToken,
  onJoined,
}: {
  /** ディープリンク(M3-5)からの事前入力。空文字なら手入力。 */
  initialToken: string;
  onJoined: () => void;
}) {
  const [token, setToken] = useState(initialToken);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [overwrite, setOverwrite] = useState(false);

  // フォームを開いたまま別のディープリンクが届いたら差し替える
  useEffect(() => {
    if (initialToken) setToken(initialToken);
  }, [initialToken]);

  const join = async () => {
    setError(null);
    try {
      setBusy(t.start.join.creating);
      const result = await api.joinNetwork(token.trim(), overwrite);
      setBusy(t.start.join.starting);
      await api.startMember(result.configPath);
      onJoined();
    } catch (e) {
      const message = errorMessage(e);
      setError(message);
      // 同じネットワークの参加設定が既にあるときは、上書きの意思を確認する。
      // これは i18n の表示文言ではなくバックエンドのエラー文へのマッチング
      // (多言語化はバックエンドのエラー構造化が前提 — 将来対応)
      if (message.includes("既に存在します")) setOverwrite(true);
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="start__section">
      <p className="muted small">{t.start.join.tokenHint}</p>
      <textarea
        className="token"
        rows={3}
        autoFocus
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
      {error && <p className="error-text">{error}</p>}
      <button
        type="button"
        onClick={() => void join()}
        disabled={busy !== null || token.trim() === ""}
      >
        {busy ?? t.start.join.submit}
      </button>
    </div>
  );
}
