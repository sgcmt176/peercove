import { useCallback, useEffect, useState } from "react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { DnsRecord, Member, api, errorMessage } from "../ipc";
import { t } from "../i18n";

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
  // 転送先: "ip" = IP 直指定 / それ以外 = メンバー参照("host" or 公開鍵)
  const [target, setTarget] = useState("ip");
  const [ip, setIp] = useState("");
  const [scheme, setScheme] = useState("");
  const [port, setPort] = useState("");
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState<string | null>(null);

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

  const shown = isHost ? records : distributed;

  /** メンバー参照("host" or 公開鍵)の表示名。 */
  const memberName = (ref: string): string => {
    const member = members.find((m) =>
      ref === "host" ? m.isHost : m.publicKey === ref,
    );
    if (!member) return t.dns.brokenRef;
    return member.name ?? member.dnsName?.split(".")[0] ?? member.ip;
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

  const canAdd =
    (machineBase ? prefix.trim() !== "" : baseFree.trim() !== "") &&
    (target !== "ip" || ip.trim() !== "");

  const add = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.addDnsRecord(
        configPath,
        relative,
        target === "ip" ? { ip } : { member: target },
        machineRef,
        scheme.trim() === "" ? undefined : scheme.trim(),
        port.trim() === "" ? undefined : Number(port),
      );
      setPrefix("");
      setBaseFree("");
      setIp("");
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

        <h3 className="subhead">{t.dns.customHead}</h3>
        {shown.length === 0 ? (
          <p className="muted small">{t.dns.customEmpty}</p>
        ) : (
          <ul className="dns-list">
            {shown.map((record) => (
              <li key={`${record.name}@${record.under ?? ""}`} className="dns-list__row">
                <span className="dns-list__names">
                  <span className="mono ellipsis" title={record.fqdn}>
                    {record.fqdn}
                  </span>
                  {record.url !== null ? (
                    <span className="mono small ellipsis" title={record.url}>
                      {record.url}
                    </span>
                  ) : record.port !== null ? (
                    <span className="mono small ellipsis">
                      {record.fqdn}:{record.port}
                    </span>
                  ) : null}
                </span>
                {record.member !== null && (
                  <span className="muted small ellipsis">
                    {t.dns.targetOf(memberName(record.member))}
                  </span>
                )}
                <span className="mono muted">{record.ip ?? t.dns.brokenRef}</span>
                {copyButton(record.fqdn)}
                {record.url !== null && copyButton(record.url, t.dns.copyUrl)}
                {isHost && (
                  <button
                    type="button"
                    className="button--link button--link-danger small"
                    onClick={() => void remove(record)}
                  >
                    {t.dns.remove}
                  </button>
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
                onChange={(event) => setBaseKind(event.target.value)}
              >
                <option value="free">{t.dns.baseFree}</option>
                {members.map((member) => (
                  <option
                    key={member.publicKey}
                    value={member.isHost ? "host" : member.publicKey}
                  >
                    {labelOf(member)}
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
                  {members.map((member) => (
                    <option
                      key={member.publicKey}
                      value={member.isHost ? "host" : member.publicKey}
                    >
                      {t.dns.forwardMember(
                        member.name ??
                          member.dnsName?.split(".")[0] ??
                          member.ip,
                      )}
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
            </div>

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
    </section>
  );
}
