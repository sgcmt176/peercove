import { useCallback, useEffect, useState } from "react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import {
  DnsRecord,
  HealthSettings,
  Member,
  ServiceHealth,
  api,
  errorMessage,
} from "../ipc";
import { t } from "../i18n";
import { Modal } from "./Modal";

/**
 * DNS 管理画面(M3-1c、ADR-0011 §1b、ADR-0022)。
 *
 * - 自動レコード: 台帳から導出されたメンバーの DNS 名(閲覧のみ。改名は
 *   メンバー一覧の ✎ から)
 * - カスタムレコード: ホストのみ追加・削除できる。ターゲットは IP 直指定か
 *   メンバー参照(IP 自動追随 = 別名・サービス名)、配置は最上位かメンバー
 *   配下(端末配下サブドメイン・LAN 機器)。設定に書かれ、5 秒の再読込 →
 *   解決 → 台帳と一緒に全メンバーへ配布される
 */
export function DnsView({
  configPath,
  members,
  distributed,
  isHost,
}: {
  configPath: string;
  members: Member[];
  /** status で配信された解決済みレコード(member の一覧表示用)。 */
  distributed: DnsRecord[];
  isHost: boolean;
}) {
  const [records, setRecords] = useState<DnsRecord[]>([]);
  const [error, setError] = useState<string | null>(null);
  // ドメイン名(ADR-0024):
  //   <prefix>.<base>.<ネットワーク名>.peercove.internal
  //   prefix   = 先頭の自由入力(空可・先頭 * でワイルドカード)
  //   baseKind = "free"(自由入力)/ それ以外はマシン参照("host" or 公開鍵)。
  //              リストで選び、"free" のときだけ baseFree を入力できる
  const [prefix, setPrefix] = useState("");
  const [baseKind, setBaseKind] = useState("free");
  const [baseFree, setBaseFree] = useState("");
  // 転送先: "ip" = IP 直指定 / "cname" = ドメイン別名 / それ以外 = メンバー参照
  const [target, setTarget] = useState("ip");
  const [ip, setIp] = useState("");
  const [cnameInput, setCnameInput] = useState("");
  const [scheme, setScheme] = useState("");
  const [port, setPort] = useState("");
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState<string | null>(null);
  const [healthEditing, setHealthEditing] = useState<DnsRecord | null>(null);
  const [checking, setChecking] = useState(false);

  // 固定サフィックス <ネットワーク名>.peercove.internal(メンバーの DNS 名から拝借)
  const suffix =
    members.find((m) => m.dnsName)?.dnsName?.split(".").slice(1).join(".") ??
    "peercove.internal";

  /** メンバーの現在の DNS ラベル(先頭ラベル)。候補・表示に使う。 */
  const labelOf = (member: Member): string =>
    member.dnsName?.split(".")[0] ?? member.name ?? member.ip;

  /** マシン参照("host" or 公開鍵)の現在の DNS ラベル。 */
  const labelOfRef = (ref: string): string => {
    const member = members.find((m) =>
      ref === "host" ? m.isHost : m.publicKey === ref,
    );
    return member ? labelOf(member) : "";
  };

  /** リスト表示用のマシンの表示名(表示名 → DNS ラベル → IP)。 */
  const displayName = (member: Member): string =>
    member.name ?? member.dnsName?.split(".")[0] ?? member.ip;

  // ホスト = 設定ファイルから(編集用の参照情報つき)。
  // メンバー = 配信された status から(設定ファイルには載っていない)
  const reload = useCallback(() => {
    if (!isHost) return;
    api
      .listDnsRecords(configPath)
      .then(setRecords)
      .catch((e) => setError(errorMessage(e)));
  }, [configPath, isHost]);

  useEffect(() => {
    reload();
  }, [reload]);

  const shown = isHost
    ? records.map((record) => ({
        ...record,
        health:
          distributed.find((current) => current.fqdn === record.fqdn)?.health ??
          record.health,
      }))
    : distributed;

  const checkNow = async () => {
    setChecking(true);
    setError(null);
    try {
      await api.checkDnsHealth(configPath);
      window.setTimeout(() => setChecking(false), 4000);
    } catch (reason) {
      setChecking(false);
      setError(errorMessage(reason));
    }
  };

  const machineBase = baseKind !== "free";
  const machineRef = machineBase ? baseKind : undefined;
  // 表示・登録に使う base ラベル(マシンなら現在ラベル、自由入力なら入力値)
  const baseTrim = machineBase ? labelOfRef(baseKind) : baseFree.trim();
  // 登録する相対名(net の左側)。base がマシンなら prefix だけ(under で親を指す)、
  // 自由入力なら prefix + base を結合する
  const relative = machineBase
    ? prefix.trim()
    : prefix.trim()
      ? `${prefix.trim()}.${baseFree.trim()}`
      : baseFree.trim();
  const leftShown = prefix.trim()
    ? baseTrim
      ? `${prefix.trim()}.${baseTrim}`
      : prefix.trim()
    : baseTrim;
  const previewFqdn = leftShown ? `${leftShown}.${suffix}` : suffix;

  // 転送先は常に選択値(target/ip/cname)に従う。base がマシンのときは初期値として
  // そのマシンを入れるが(baseKind の onChange で設定)、プルダウンから変更できる。
  const effectiveTarget: { ip?: string; member?: string; cname?: string } =
    target === "ip"
      ? { ip }
      : target === "cname"
        ? { cname: cnameInput.trim() }
        : { member: target };

  const targetOk =
    target === "ip"
      ? ip.trim() !== ""
      : target === "cname"
        ? cnameInput.trim() !== ""
        : true;
  const canAdd =
    (machineBase ? prefix.trim() !== "" : baseFree.trim() !== "") && targetOk;

  const add = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.addDnsRecord(
        configPath,
        relative,
        effectiveTarget,
        machineRef,
        scheme.trim() === "" ? undefined : scheme.trim(),
        port.trim() === "" ? undefined : Number(port),
      );
      setPrefix("");
      setBaseFree("");
      setIp("");
      setCnameInput("");
      setScheme("");
      setPort("");
      reload();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const remove = async (record: DnsRecord) => {
    setError(null);
    try {
      await api.removeDnsRecord(configPath, record.name, record.under);
      reload();
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const copy = async (fqdn: string) => {
    try {
      await writeText(fqdn);
      setCopied(fqdn);
      setTimeout(() => setCopied(null), 2000);
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const copyButton = (value: string, label = t.dns.copy) => (
    <button
      type="button"
      className="button--link small"
      onClick={() => void copy(value)}
    >
      {copied === value ? t.dns.copied : label}
    </button>
  );

  return (
    <section className="card">
      <h2 className="card-title">{t.dns.title}</h2>
      <div className="page-body">
        <p className="muted small">{t.dns.intro}</p>

        <h3 className="subhead">{t.dns.autoHead}</h3>
        {members.length === 0 ? (
          <p className="muted small">{t.dns.autoEmpty}</p>
        ) : (
          <ul className="dns-list">
            {members.map((member) => (
              <li key={member.publicKey} className="dns-list__row">
                <span className="mono ellipsis" title={member.dnsName ?? ""}>
                  {member.dnsName ?? "—"}
                </span>
                <span className="mono muted">{member.ip}</span>
                {member.dnsName && copyButton(member.dnsName)}
              </li>
            ))}
          </ul>
        )}

        <div className="dns-services__head">
          <h3 className="subhead">{t.dns.customHead}</h3>
          {isHost && shown.some((record) => record.port !== null) && (
            <button
              type="button"
              className="button--ghost"
              disabled={checking}
              onClick={() => void checkNow()}
            >
              {checking ? t.dns.health.checking : t.dns.health.checkNow}
            </button>
          )}
        </div>
        {shown.length === 0 ? (
          <p className="muted small">{t.dns.customEmpty}</p>
        ) : (
          <ul className="service-list">
            {shown.map((record) => (
              <li key={`${record.name}@${record.under ?? ""}`} className="service">
                <ServiceIcon />
                <div className="service__text">
                  <span className="service__name mono">{record.fqdn}</span>
                  {record.url !== null ? (
                    <button
                      type="button"
                      className="service__url mono"
                      title={t.dns.openTitle}
                      onClick={() => void api.openLink(record.url as string)}
                    >
                      {record.url}
                    </button>
                  ) : record.port !== null ? (
                    <span className="service__url mono muted">
                      {record.fqdn}:{record.port}
                    </span>
                  ) : null}
                  <span className="service__target mono muted small">
                    {record.cname ?? record.ip ?? t.dns.brokenRef}
                  </span>
                  {record.port !== null && <HealthLine health={record.health} />}
                  {record.health?.status === "unhealthy" && (
                    <span className="service__warning small">
                      {t.dns.health.openWarning}
                    </span>
                  )}
                </div>
                {copyButton(record.fqdn)}
                {record.url !== null && copyButton(record.url, t.dns.copyUrl)}
                {isHost && (
                  <>
                    {record.port !== null && record.healthSettings !== null && (
                      <button
                        type="button"
                        className="button--link small"
                        onClick={() => setHealthEditing(record)}
                      >
                        {t.dns.health.settings}
                      </button>
                    )}
                    <button
                      type="button"
                      className="button--link button--link-danger small"
                      onClick={() => void remove(record)}
                    >
                      {t.dns.remove}
                    </button>
                  </>
                )}
              </li>
            ))}
          </ul>
        )}

        {isHost ? (
          <>
            <p className="muted small">{t.dns.customNote}</p>

            {/* ドメイン名: <prefix>.<base>.<net>.peercove.internal(ADR-0024) */}
            <span className="field__label">{t.dns.domainLabel}</span>
            <div className="dns-domain">
              <input
                className="dns-domain__prefix mono"
                value={prefix}
                placeholder={t.dns.prefixPlaceholder}
                onChange={(event) => setPrefix(event.target.value)}
              />
              <span className="dns-domain__dot">.</span>
              <select
                className="dns-domain__basekind"
                value={baseKind}
                onChange={(event) => {
                  const value = event.target.value;
                  setBaseKind(value);
                  // マシンを選んだら転送先の初期値を同マシンにする(変更可)。
                  if (value !== "free") setTarget(value);
                }}
              >
                <option value="free">{t.dns.baseFree}</option>
                {members.map((member) => (
                  <option
                    key={member.publicKey}
                    value={member.isHost ? "host" : member.publicKey}
                  >
                    {displayName(member)}
                  </option>
                ))}
              </select>
              {!machineBase && (
                <input
                  className="dns-domain__base mono"
                  value={baseFree}
                  placeholder={t.dns.baseFreePlaceholder}
                  onChange={(event) => setBaseFree(event.target.value)}
                />
              )}
              <span className="dns-domain__suffix mono muted">.{suffix}</span>
            </div>
            <p className="muted small">
              {machineBase ? t.dns.baseIsMachine : t.dns.wildcardHint}
            </p>
            <p className="muted small">
              {t.dns.previewLabel}:{" "}
              <span className="mono">{previewFqdn}</span>
            </p>

            {/* 転送先: マシン(IP 自動追随)or IP アドレス直指定 */}
            <span className="field__label">{t.dns.forwardLabel}</span>
            <div className="row">
              <label className="field">
                <select
                  value={target}
                  onChange={(event) => setTarget(event.target.value)}
                >
                  <option value="ip">{t.dns.forwardIp}</option>
                  <option value="cname">{t.dns.forwardCname}</option>
                  {members.map((member) => (
                    <option
                      key={member.publicKey}
                      value={member.isHost ? "host" : member.publicKey}
                    >
                      {t.dns.forwardMember(displayName(member))}
                    </option>
                  ))}
                </select>
              </label>
              {target === "ip" && (
                <label className="field">
                  <input
                    value={ip}
                    placeholder={t.dns.ipPlaceholder}
                    className="mono"
                    onChange={(event) => setIp(event.target.value)}
                  />
                </label>
              )}
              {target === "cname" && (
                <label className="field">
                  <input
                    value={cnameInput}
                    placeholder={t.dns.cnamePlaceholder}
                    className="mono"
                    onChange={(event) => setCnameInput(event.target.value)}
                  />
                </label>
              )}
            </div>
            {target === "cname" && (
              <p className="muted small">{t.dns.cnameHint}</p>
            )}

            {/* サービス情報(任意): スキーム・ポート → URL 表示 */}
            <div className="row">
              <label className="field">
                <span>{t.dns.schemeLabel}</span>
                <input
                  list="peercove-dns-schemes"
                  value={scheme}
                  placeholder={t.dns.schemePlaceholder}
                  onChange={(event) => setScheme(event.target.value)}
                />
                <datalist id="peercove-dns-schemes">
                  <option value="http" />
                  <option value="https" />
                </datalist>
              </label>
              <label className="field">
                <span>{t.dns.portLabel}</span>
                <input
                  type="number"
                  min={1}
                  max={65535}
                  step={1}
                  value={port}
                  placeholder={t.dns.portPlaceholder}
                  onChange={(event) => setPort(event.target.value)}
                />
              </label>
              <button
                type="button"
                onClick={() => void add()}
                disabled={busy || !canAdd}
              >
                {busy ? t.dns.adding : t.dns.add}
              </button>
            </div>
            <p className="muted small">{t.dns.serviceHint}</p>
          </>
        ) : (
          <p className="muted small">{t.dns.customNoteMember}</p>
        )}

        {error && <p className="error-text">{error}</p>}
      </div>
      {healthEditing?.healthSettings && (
        <HealthDialog
          configPath={configPath}
          record={healthEditing}
          onClose={() => setHealthEditing(null)}
          onSaved={() => {
            setHealthEditing(null);
            reload();
            void checkNow();
          }}
        />
      )}
    </section>
  );
}

function ServiceIcon() {
  return (
    <span className="service__icon" aria-hidden>
      <svg viewBox="0 0 24 24" width="20" height="20">
        <path
          d="M10.5 13.5a4 4 0 0 0 5.7.1l2.4-2.4a4 4 0 0 0-5.7-5.7l-1.4 1.4m2 3.6a4 4 0 0 0-5.7-.1l-2.4 2.4a4 4 0 0 0 5.7 5.7l1.4-1.4"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.8"
          strokeLinecap="round"
        />
      </svg>
    </span>
  );
}

function HealthLine({ health }: { health: ServiceHealth | null }) {
  const status = health?.status ?? "unknown";
  const checked = health?.checkedAtUnixMs
    ? new Intl.DateTimeFormat("ja-JP", {
        hour: "2-digit",
        minute: "2-digit",
        second: "2-digit",
      }).format(new Date(health.checkedAtUnixMs))
    : t.dns.health.notChecked;
  const detail =
    health?.responseMs !== null && health?.responseMs !== undefined
      ? `${health.responseMs} ms`
      : reasonLabel(health?.reason);
  return (
    <span className={`service-health service-health--${status}`}>
      <i aria-hidden />
      <strong>{t.dns.health.status[status]}</strong>
      <span>{detail}</span>
      <span>{t.dns.health.checked(checked)}</span>
    </span>
  );
}

function reasonLabel(reason?: ServiceHealth["reason"]) {
  if (!reason) return t.dns.health.reason.not_checked;
  return t.dns.health.reason[reason];
}

function HealthDialog({
  configPath,
  record,
  onClose,
  onSaved,
}: {
  configPath: string;
  record: DnsRecord;
  onClose: () => void;
  onSaved: () => void;
}) {
  const original = record.healthSettings as HealthSettings;
  const [settings, setSettings] = useState<HealthSettings>(original);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const isExternal = record.cname !== null;
  const canHead = record.scheme === "http";

  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.setDnsHealth(configPath, record, settings);
      onSaved();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal title={t.dns.health.dialogTitle} onClose={onClose}>
      <div className="modal__body health-form">
        <p className="mono small health-form__fqdn">{record.fqdn}</p>
        <label className="chat__pick-row">
          <input
            type="checkbox"
            checked={settings.enabled}
            onChange={(event) =>
              setSettings({ ...settings, enabled: event.target.checked })
            }
          />
          <span>{t.dns.health.enabled}</span>
        </label>
        <p className="muted small">{t.dns.health.enabledHint}</p>
        {isExternal && (
          <>
            <label className="chat__pick-row">
              <input
                type="checkbox"
                checked={settings.external}
                onChange={(event) =>
                  setSettings({ ...settings, external: event.target.checked })
                }
              />
              <span>{t.dns.health.external}</span>
            </label>
            <p className="muted small">{t.dns.health.externalHint}</p>
          </>
        )}
        <label className="field">
          <span>{t.dns.health.kind}</span>
          <select
            value={settings.kind}
            onChange={(event) =>
              setSettings({
                ...settings,
                kind: event.target.value as HealthSettings["kind"],
              })
            }
          >
            <option value="tcp">{t.dns.health.tcp}</option>
            <option value="http_head" disabled={!canHead}>
              {t.dns.health.httpHead}
            </option>
          </select>
        </label>
        {settings.kind === "http_head" && (
          <div className="row">
            <label className="field">
              <span>{t.dns.health.path}</span>
              <input
                value={settings.path}
                className="mono"
                onChange={(event) =>
                  setSettings({ ...settings, path: event.target.value })
                }
              />
            </label>
            <label className="field">
              <span>{t.dns.health.expected}</span>
              <input
                type="number"
                min={100}
                max={599}
                placeholder="200–399"
                value={settings.expectedStatus ?? ""}
                onChange={(event) =>
                  setSettings({
                    ...settings,
                    expectedStatus:
                      event.target.value === "" ? null : Number(event.target.value),
                  })
                }
              />
            </label>
          </div>
        )}
        {error && <p className="error-text">{error}</p>}
      </div>
      <div className="modal__actions">
        <button type="button" className="button--ghost" onClick={onClose}>
          {t.common.cancel}
        </button>
        <button type="button" disabled={busy} onClick={() => void save()}>
          {busy ? t.common.saving : t.common.save}
        </button>
      </div>
    </Modal>
  );
}
