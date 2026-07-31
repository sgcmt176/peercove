import { useEffect, useState } from "react";
import {
  AdminNetworkSettings,
  Settings,
  StorageUsage,
  api,
  errorMessage,
  formatBytes,
} from "../ipc";
import { ConfirmModal } from "./Modal";
import { t } from "../i18n";

/**
 * ネットワークごとの設定ページ(M2-G5 → M3-16 でページ化)。編集できるのは
 * 「自分側の設定」だけ。サイドバーの「設定」(ネットワーク詳細を表示中)、
 * または一覧カードの「設定」から開く 1 ページ。
 *
 * メンバーの追加・削除・改名はメンバー一覧側の操作([[peer]] の編集)なので
 * ここには出さない。**表示名・DNS 名の変更もメンバー一覧に一元化**した
 * (自分の行の ✎ から。ADR-0027、M3-19)。仮想 IP / サブネットは init・join の
 * やり直しになるため表示のみ。
 *
 * MTU・待受ポート・ホストのエンドポイントは、インターフェース生成時に決まる
 * ので保存すると supervisor が変更を検知して**自動的に再接続して反映する**
 * (約 5 秒以内)。保存後にその旨を出す。
 */
export function NetworkSettingsView({
  configPath,
  liveInterfaceName,
  isConnected,
  isSelfAdmin,
}: {
  configPath: string;
  /** 接続中の実アダプタ名(自動採番後）。未接続なら null で設定値を出す。 */
  liveInterfaceName: string | null;
  /** このネットワークに接続中か(ストレージ管理: チャット履歴のクリアは
   *  接続中のみ実行できる — M6 H-10a)。省略時は未接続扱い。 */
  isConnected?: boolean;
  /** 自分が管理を委任されている(ADR-0059 J-2 拡大)。メンバーの設定ページに
   *  「ネットワーク管理(委任)」節を追加で出す。省略時は false(ホスト自身の
   *  設定ページには不要)。 */
  isSelfAdmin?: boolean;
}) {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .readSettings(configPath)
      .then(setSettings)
      .catch((e) => setError(errorMessage(e)));
  }, [configPath]);

  return (
    <section className="card">
      <h2 className="card-title">{t.settings.title}</h2>
      {settings ? (
        <SettingsForm
          configPath={configPath}
          settings={settings}
          liveInterfaceName={liveInterfaceName}
          isConnected={isConnected ?? false}
          isSelfAdmin={isSelfAdmin ?? false}
        />
      ) : (
        <div className="page-body">
          {error ? (
            <p className="error-text">{error}</p>
          ) : (
            <p className="muted">{t.common.loading}</p>
          )}
        </div>
      )}
    </section>
  );
}

function SettingsForm({
  configPath,
  settings,
  liveInterfaceName,
  isConnected,
  isSelfAdmin,
}: {
  configPath: string;
  settings: Settings;
  liveInterfaceName: string | null;
  isConnected: boolean;
  isSelfAdmin: boolean;
}) {
  // 接続中は実際に使われているアダプタ名を出す(既定名が衝突すると自動採番で
  // peercove1 等になるため、設定ファイルの値と食い違いうる — ADR-0028、M3-20)
  const interfaceName = liveInterfaceName ?? settings.interfaceName;
  const [listenPort, setListenPort] = useState(
    settings.listenPort === null ? "" : String(settings.listenPort),
  );
  const [mtu, setMtu] = useState(String(settings.mtu));
  const [hostEndpoint, setHostEndpoint] = useState(settings.hostEndpoint ?? "");
  const [direct, setDirect] = useState(settings.direct);
  const [maxRecvFileMb, setMaxRecvFileMb] = useState(
    String(settings.maxRecvFileMb),
  );
  const [requireInviteApproval, setRequireInviteApproval] = useState(
    settings.requireInviteApproval,
  );
  // メンバーによる招待発行(ADR-0048)。端末指名はメンバー詳細から行う
  const [memberInvites, setMemberInvites] = useState(settings.memberInvites);
  // 起動時の自動接続復旧(ADR-0057、M6 H-8e)。host / member 共通
  const [autoConnect, setAutoConnect] = useState(settings.autoConnect);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  // デバイス鍵ローテーション(ADR-0020、M3-11)
  const [confirmRotate, setConfirmRotate] = useState(false);
  const [rotateBusy, setRotateBusy] = useState(false);

  const rotate = async () => {
    setRotateBusy(true);
    setError(null);
    setNotice(null);
    try {
      await api.rotateKey(configPath);
      setNotice(t.settings.rotateKeyRequested);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setRotateBusy(false);
      setConfirmRotate(false);
    }
  };

  const save = async () => {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      const mtuValue = Number(mtu);
      if (!Number.isInteger(mtuValue)) throw t.settings.mtuInteger;
      const portValue = listenPort.trim() === "" ? null : Number(listenPort);
      if (portValue !== null && !Number.isInteger(portValue)) {
        throw t.settings.portInteger;
      }
      const maxFileValue = Number(maxRecvFileMb);
      if (!Number.isInteger(maxFileValue) || maxFileValue < 0) {
        throw t.settings.maxFileInteger;
      }
      const result = await api.saveSettings(configPath, {
        // 表示名・DNS 名はメンバー一覧から変更する(ADR-0027、M3-19)。
        // ネットワーク表示名(ADR-0058)はネットワーク詳細ヘッダーの ✏️ から
        // 変更する(ホストのみ)。ここでは現在値をそのまま書き戻して消さない
        displayName: settings.displayName,
        dnsName: settings.dnsName,
        networkDisplayName: settings.networkDisplayName,
        listenPort: portValue,
        mtu: mtuValue,
        hostEndpoint:
          settings.isMember && hostEndpoint.trim() !== ""
            ? hostEndpoint.trim()
            : null,
        direct,
        maxRecvFileMb: maxFileValue,
        requireInviteApproval,
        memberInvites,
        autoConnect,
      });
      // auto_connect を ON にしたときだけ、デーモンに設定パスを明示登録する
      // (一度も接続していないネットワークでも次回起動時に見つけられるように
      // する — ADR-0057 決定 2)。OFF なら次回のデーモン起動時にフラグを
      // 見ないので登録不要
      if (autoConnect) {
        await api.registerAutostart(configPath, settings.isMember);
      }
      setNotice(
        result.restartRequired
          ? t.settings.savedRestart
          : t.settings.savedLive,
      );
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <div className="page-body">
        <dl className="facts">
          <dt>{t.settings.interface}</dt>
          <dd className="mono">{interfaceName}</dd>
          <dt>{t.common.virtualIp}</dt>
          <dd className="mono">{settings.address}</dd>
          <dt>{t.common.configFile}</dt>
          <dd className="mono wrap-anywhere" title={configPath}>
            {configPath}
          </dd>
        </dl>

        <p className="muted small">{t.settings.nameMovedHint}</p>

        <label className="field">
          <span>
            {t.settings.portLabel}
            <small className="muted">
              {t.settings.portHint(settings.isMember, settings.defaultListenPort)}
            </small>
          </span>
          <input
            type="text"
            inputMode="numeric"
            value={listenPort}
            placeholder={String(settings.defaultListenPort)}
            onChange={(event) => setListenPort(event.target.value)}
          />
        </label>

        <label className="field">
          <span>
            {t.settings.mtuLabel}
            <small className="muted">{t.settings.mtuHint(settings.defaultMtu)}</small>
          </span>
          <input
            type="text"
            inputMode="numeric"
            value={mtu}
            onChange={(event) => setMtu(event.target.value)}
          />
        </label>

        {settings.isMember && (
          <label className="field">
            <span>
              {t.settings.hostEndpointLabel}
              <small className="muted">{t.settings.hostEndpointHint}</small>
            </span>
            <input
              type="text"
              value={hostEndpoint}
              placeholder={t.settings.hostEndpointPlaceholder}
              onChange={(event) => setHostEndpoint(event.target.value)}
            />
          </label>
        )}

        <label className="field">
          <span>
            {t.settings.maxFileLabel}
            <small className="muted">
              {t.settings.maxFileHint(settings.defaultMaxRecvFileMb)}
            </small>
          </span>
          <input
            type="text"
            inputMode="numeric"
            value={maxRecvFileMb}
            placeholder={String(settings.defaultMaxRecvFileMb)}
            onChange={(event) => setMaxRecvFileMb(event.target.value)}
          />
        </label>

        {settings.isMember && (
          <label className="field field--check">
            <input
              type="checkbox"
              checked={direct}
              onChange={(event) => setDirect(event.target.checked)}
            />
            <span>
              {t.settings.directLabel}
              <small className="muted"> {t.settings.directHint}</small>
            </span>
          </label>
        )}

        {!settings.isMember && (
          <label className="field field--check">
            <input
              type="checkbox"
              checked={requireInviteApproval}
              onChange={(event) => setRequireInviteApproval(event.target.checked)}
            />
            <span>
              {t.settings.inviteApprovalLabel}
              <small className="muted"> {t.settings.inviteApprovalHint}</small>
            </span>
          </label>
        )}

        {!settings.isMember && (
          <label className="field field--check">
            <input
              type="checkbox"
              checked={memberInvites}
              onChange={(event) => setMemberInvites(event.target.checked)}
            />
            <span>
              {t.settings.memberInvitesLabel}
              <small className="muted"> {t.settings.memberInvitesHint}</small>
            </span>
          </label>
        )}

        <label className="field field--check">
          <input
            type="checkbox"
            checked={autoConnect}
            onChange={(event) => setAutoConnect(event.target.checked)}
          />
          <span>
            {t.settings.autoConnectLabel}
            <small className="muted"> {t.settings.autoConnectHint}</small>
          </span>
        </label>

        {settings.isMember && (
          <div className="field">
            <span>
              {t.settings.rotateKeyLabel}
              <small className="muted"> — {t.settings.rotateKeyHint}</small>
            </span>
            <div>
              <button
                type="button"
                className="button--ghost"
                onClick={() => setConfirmRotate(true)}
              >
                {t.settings.rotateKeyButton}
              </button>
            </div>
          </div>
        )}

        <p className="muted small">{t.settings.restartHint(settings.isMember)}</p>

        {error && <p className="error-text">{error}</p>}
        {notice && <p className="notice">{notice}</p>}

        {settings.isMember && isSelfAdmin && (
          <AdminNetworkSection configPath={configPath} />
        )}

        <StorageManagement configPath={configPath} isConnected={isConnected} />
      </div>
      <div className="page-actions">
        <button type="button" onClick={() => void save()} disabled={busy}>
          {busy ? t.common.saving : t.common.save}
        </button>
      </div>
      {confirmRotate && (
        <ConfirmModal
          title={t.settings.rotateKeyButton}
          message={<p>{t.settings.rotateKeyConfirm}</p>}
          confirmLabel={t.settings.rotateKeyButton}
          busy={rotateBusy}
          onConfirm={() => void rotate()}
          onClose={() => setConfirmRotate(false)}
        />
      )}
    </>
  );
}

/**
 * ネットワーク管理(委任、ADR-0059 J-2 拡大)。管理を委任されたメンバーが、
 * ホストのネットワーク設定(承認制・メンバー招待許可・表示名)を
 * `api.adminOp` 経由(中継)で読み書きする。開いたときに `ReadNetworkSettings`
 * で現在値を取得し、変更は各 Set 系を個別に送る(保存ボタンでまとめない —
 * チェックボックスは即時反映、表示名だけ入力欄 + ボタン)。
 */
function AdminNetworkSection({ configPath }: { configPath: string }) {
  const [settings, setSettings] = useState<AdminNetworkSettings | null>(null);
  const [requireApproval, setRequireApproval] = useState(false);
  const [memberInvites, setMemberInvites] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    api
      .adminOp(configPath, { op: "read_network_settings" })
      .then((data) => {
        if (!data) return;
        setSettings(data);
        setRequireApproval(data.requireInviteApproval);
        setMemberInvites(data.memberInvites);
      })
      .catch((e) => setError(errorMessage(e)));
  }, [configPath]);

  const notifyApplied = () => {
    setNotice(t.dns.adminApplyNotice);
    setTimeout(() => setNotice(null), 8000);
  };

  const applyRequireApproval = async (enabled: boolean) => {
    setBusy(true);
    setError(null);
    try {
      await api.adminOp(configPath, { op: "set_require_approval", enabled });
      setRequireApproval(enabled);
      notifyApplied();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const applyMemberInvites = async (enabled: boolean) => {
    setBusy(true);
    setError(null);
    try {
      await api.adminOp(configPath, { op: "set_member_invites", enabled });
      setMemberInvites(enabled);
      notifyApplied();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  // ネットワーク表示名の変更はこの画面から撤去した(M7 J-3 追補)。
  // 上部ヘッダーの ✏️(App.tsx の DetailHeader)へ一本化しており、admin も
  // そこから adminOp 経由で変更できる。

  return (
    <div className="field">
      <h3 className="section-head">{t.settings.adminNetworkTitle}</h3>
      <p className="muted small">{t.settings.adminNetworkHint}</p>
      {!settings ? (
        error ? (
          <p className="error-text">{error}</p>
        ) : (
          <p className="muted small">{t.common.loading}</p>
        )
      ) : (
        <>
          <label className="field field--check">
            <input
              type="checkbox"
              checked={requireApproval}
              disabled={busy}
              onChange={(event) => void applyRequireApproval(event.target.checked)}
            />
            <span>
              {t.settings.inviteApprovalLabel}
              <small className="muted"> {t.settings.inviteApprovalHint}</small>
            </span>
          </label>
          <label className="field field--check">
            <input
              type="checkbox"
              checked={memberInvites}
              disabled={busy}
              onChange={(event) => void applyMemberInvites(event.target.checked)}
            />
            <span>
              {t.settings.memberInvitesLabel}
              <small className="muted"> {t.settings.memberInvitesHint}</small>
            </span>
          </label>
          {/* ネットワーク表示名の変更はここには置かない(M7 J-3 追補、
              依頼者指示)。画面上部のネットワーク名の横にある ✏️ から
              直接編集する経路に一本化する(admin もその経路を使える)。 */}
          {error && <p className="error-text small">{error}</p>}
          {notice && <p className="notice">{notice}</p>}
        </>
      )}
    </div>
  );
}

/**
 * ストレージ管理(M6 H-10a)。Android の設定タブ末尾の節をデスクトップへ
 * 移植したもの。受信ボックス・チャット履歴・品質履歴はクリアでき、共有メモは
 * サイズ表示のみ(容量管理は共有メモ画面の上限設定で行う)。
 */
function StorageManagement({
  configPath,
  isConnected,
}: {
  configPath: string;
  /** チャット履歴のクリアは接続中のみ実行できる(履歴はデーモンのメモリが
   *  正本のため — M6 H-10a)。 */
  isConnected: boolean;
}) {
  const [usage, setUsage] = useState<StorageUsage | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [confirmKind, setConfirmKind] = useState<"inbox" | "chat" | null>(null);
  const [busy, setBusy] = useState(false);

  const load = () => {
    api
      .storageUsage(configPath)
      .then(setUsage)
      .catch((e) => setError(errorMessage(e)));
  };
  useEffect(load, [configPath]);

  const runClear = async (action: () => Promise<string>) => {
    setBusy(true);
    setError(null);
    try {
      setNotice(await action());
      load();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
      setConfirmKind(null);
    }
  };

  const clearQuality = () => {
    setError(null);
    setNotice(null);
    api
      .qualityClear(configPath)
      .then(() => {
        setNotice(t.settings.storageCleared);
        load();
      })
      .catch((e) => setError(errorMessage(e)));
  };

  return (
    <div className="field">
      <h3 className="section-head">{t.settings.storageTitle}</h3>
      {!usage ? (
        error ? (
          <p className="error-text">{error}</p>
        ) : (
          <p className="muted small">{t.common.loading}</p>
        )
      ) : (
        <>
          <StorageRow
            label={t.settings.storageInbox}
            bytes={usage.inboxBytes}
            actionLabel={t.settings.storageClear}
            onClick={() => setConfirmKind("inbox")}
          />
          <StorageRow
            label={t.settings.storageChat}
            bytes={usage.chatBytes}
            actionLabel={t.settings.storageClear}
            onClick={() => setConfirmKind("chat")}
            disabled={!isConnected}
            disabledHint={isConnected ? undefined : t.settings.storageChatDisabledHint}
          />
          <StorageRow
            label={t.settings.storageQuality}
            bytes={usage.qualityBytes}
            actionLabel={t.settings.storageClear}
            onClick={clearQuality}
          />
          <StorageRow
            label={t.settings.storageMemo}
            bytes={usage.memoBytes}
            hint={t.settings.storageMemoHint}
          />
          {error && <p className="error-text">{error}</p>}
          {notice && <p className="notice">{notice}</p>}
        </>
      )}
      {confirmKind && (
        <ConfirmModal
          title={
            confirmKind === "inbox"
              ? t.settings.storageConfirmInboxTitle
              : t.settings.storageConfirmChatTitle
          }
          message={
            <p>
              {confirmKind === "inbox"
                ? t.settings.storageConfirmInbox
                : t.settings.storageConfirmChat}
            </p>
          }
          confirmLabel={t.settings.storageClear}
          busy={busy}
          onConfirm={() =>
            void runClear(
              confirmKind === "inbox"
                ? async () => t.settings.storageClearedInbox(await api.clearInbox(configPath))
                : async () => {
                    await api.chatClearAll(configPath);
                    return t.settings.storageCleared;
                  },
            )
          }
          onClose={() => setConfirmKind(null)}
        />
      )}
    </div>
  );
}

/** ストレージ管理の 1 行(ラベル + サイズ + 任意のクリアボタン)。 */
function StorageRow({
  label,
  bytes,
  hint,
  actionLabel,
  onClick,
  disabled,
  disabledHint,
}: {
  label: string;
  bytes: number;
  /** クリア操作を持たない行(共有メモ)向けの注記。 */
  hint?: string;
  actionLabel?: string;
  onClick?: () => void;
  disabled?: boolean;
  disabledHint?: string;
}) {
  return (
    <div className="field">
      <span>
        {label}
        <small className="muted">
          {" "}
          — {formatBytes(bytes)}
          {hint ? ` ${hint}` : ""}
        </small>
      </span>
      {onClick && actionLabel && (
        <div>
          <button
            type="button"
            className="button--ghost"
            disabled={disabled}
            title={disabled ? disabledHint : undefined}
            onClick={onClick}
          >
            {actionLabel}
          </button>
        </div>
      )}
    </div>
  );
}
