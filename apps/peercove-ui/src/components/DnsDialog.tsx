import { useCallback, useEffect, useState } from "react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { DnsRecord, Member, api, errorMessage } from "../ipc";
import { Modal } from "./Modal";
import { t } from "../i18n";

/**
 * DNS 管理画面(M3-1c、ADR-0011 §1b)。
 *
 * - 自動レコード: 台帳から導出されたメンバーの DNS 名(閲覧のみ。改名は
 *   メンバー一覧の ✎ から)
 * - カスタムレコード: ホストのみ追加・削除できる。設定に書かれ、5 秒の
 *   再読込 → 台帳と一緒に全メンバーへ配布される
 */
export function DnsDialog({
  configPath,
  members,
  isHost,
  onClose,
}: {
  configPath: string;
  members: Member[];
  isHost: boolean;
  onClose: () => void;
}) {
  const [records, setRecords] = useState<DnsRecord[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [ip, setIp] = useState("");
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState<string | null>(null);

  const reload = useCallback(() => {
    api
      .listDnsRecords(configPath)
      .then(setRecords)
      .catch((e) => setError(errorMessage(e)));
  }, [configPath]);

  useEffect(() => {
    reload();
  }, [reload]);

  const add = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.addDnsRecord(configPath, name, ip);
      setName("");
      setIp("");
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
      await api.removeDnsRecord(configPath, record.name);
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

  const copyButton = (fqdn: string) => (
    <button
      type="button"
      className="button--link small"
      onClick={() => void copy(fqdn)}
    >
      {copied === fqdn ? t.dns.copied : t.dns.copy}
    </button>
  );

  return (
    <Modal title={t.dns.title} onClose={onClose} wide>
      <div className="modal__body">
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
        {records.length === 0 ? (
          <p className="muted small">{t.dns.customEmpty}</p>
        ) : (
          <ul className="dns-list">
            {records.map((record) => (
              <li key={record.name} className="dns-list__row">
                <span className="mono ellipsis" title={record.fqdn}>
                  {record.fqdn}
                </span>
                <span className="mono muted">{record.ip}</span>
                {copyButton(record.fqdn)}
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
            <div className="row">
              <label className="field">
                <span>{t.dns.nameLabel}</span>
                <input
                  value={name}
                  placeholder={t.dns.namePlaceholder}
                  onChange={(event) => setName(event.target.value)}
                />
              </label>
              <label className="field">
                <span>{t.dns.ipLabel}</span>
                <input
                  value={ip}
                  placeholder={t.dns.ipPlaceholder}
                  className="mono"
                  onChange={(event) => setIp(event.target.value)}
                />
              </label>
              <button
                type="button"
                onClick={() => void add()}
                disabled={busy || name.trim() === "" || ip.trim() === ""}
              >
                {busy ? t.dns.adding : t.dns.add}
              </button>
            </div>
          </>
        ) : (
          <p className="muted small">{t.dns.customNoteMember}</p>
        )}

        {error && <p className="error-text">{error}</p>}
      </div>
      <div className="modal__actions">
        <button type="button" className="button--ghost" onClick={onClose}>
          {t.common.close}
        </button>
      </div>
    </Modal>
  );
}
