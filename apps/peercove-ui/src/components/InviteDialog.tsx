import { useState } from "react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { InviteResult, api, errorMessage } from "../ipc";
import { Modal } from "./Modal";
import { t } from "../i18n";

/**
 * 招待の発行と、発行直後のトークン表示。
 *
 * トークンはメンバーの秘密鍵を含む(ADR-0005)ため、**このダイアログを閉じたら
 * 二度と表示しない**(ADR-0008)。再発行は invite をもう一度行う。
 */
export function InviteDialog({
  configPath,
  onClose,
}: {
  configPath: string;
  onClose: () => void;
}) {
  const [name, setName] = useState("");
  const [psk, setPsk] = useState(false);
  const [externalEndpoint, setExternalEndpoint] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<InviteResult | null>(null);
  const [copied, setCopied] = useState<"token" | "link" | null>(null);

  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      const endpoints = externalEndpoint.trim() ? [externalEndpoint.trim()] : [];
      setResult(
        await api.createInvite(configPath, name.trim() || null, psk, endpoints),
      );
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const copy = async (kind: "token" | "link") => {
    if (!result) return;
    try {
      // 参加リンク(M3-5): クリックすると PeerCove が開いて参加フォームに
      // トークンが入る。アプリ未導入の相手にはトークンをそのまま渡す
      const text =
        kind === "token"
          ? result.token
          : `peercove://join?token=${encodeURIComponent(result.token)}`;
      await writeText(text);
      setCopied(kind);
      setTimeout(() => setCopied(null), 2000);
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  if (result) {
    return (
      <Modal title={t.invite.resultTitle(result.name)} onClose={onClose} wide>
        <div className="modal__body">
          <p className="warn">{t.invite.warn}</p>
          <div className="invite">
            <div
              className="invite__qr"
              // fast_qr が生成した SVG。入力はこちらで作ったトークンのみ
              dangerouslySetInnerHTML={{ __html: result.qrSvg }}
            />
            <div className="invite__detail">
              <dl className="facts">
                <dt>{t.invite.allocatedIp}</dt>
                <dd className="mono">{result.ip}</dd>
                <dt>{t.invite.endpoints}</dt>
                <dd className="mono">{result.endpoints.join(", ")}</dd>
                <dt>{t.invite.psk}</dt>
                <dd>{result.psk ? t.invite.yes : t.invite.no}</dd>
              </dl>
              <textarea className="token" readOnly value={result.token} rows={3} />
              <div className="row">
                <button type="button" onClick={() => void copy("token")}>
                  {copied === "token" ? t.invite.copied : t.invite.copy}
                </button>
                <button
                  type="button"
                  className="button--ghost"
                  title={t.invite.copyLinkHint}
                  onClick={() => void copy("link")}
                >
                  {copied === "link" ? t.invite.copied : t.invite.copyLink}
                </button>
              </div>
              <p className="muted small">{t.invite.copyLinkHint}</p>
              {error && <p className="error-text">{error}</p>}
            </div>
          </div>
          <p className="muted">{t.invite.resultNote}</p>
        </div>
        <div className="modal__actions">
          <button type="button" onClick={onClose}>
            {t.common.close}
          </button>
        </div>
      </Modal>
    );
  }

  return (
    <Modal title={t.invite.formTitle} onClose={onClose}>
      <div className="modal__body">
        <label className="field">
          <span>{t.invite.nameLabel}</span>
          <input
            value={name}
            onChange={(event) => setName(event.target.value)}
            placeholder={t.invite.namePlaceholder}
            autoFocus
          />
        </label>
        <label className="field">
          <span>{t.invite.externalLabel}</span>
          <input
            value={externalEndpoint}
            onChange={(event) => setExternalEndpoint(event.target.value)}
            placeholder={t.invite.externalPlaceholder}
            className="mono"
          />
          <small className="muted">{t.invite.externalHint}</small>
        </label>
        <label className="field field--check">
          <input
            type="checkbox"
            checked={psk}
            onChange={(event) => setPsk(event.target.checked)}
          />
          <span>{t.invite.pskLabel}</span>
        </label>
        {error && <p className="error-text">{error}</p>}
      </div>
      <div className="modal__actions">
        <button type="button" className="button--ghost" onClick={onClose}>
          {t.common.cancel}
        </button>
        <button type="button" onClick={() => void submit()} disabled={busy}>
          {busy ? t.invite.issuing : t.invite.issue}
        </button>
      </div>
    </Modal>
  );
}
