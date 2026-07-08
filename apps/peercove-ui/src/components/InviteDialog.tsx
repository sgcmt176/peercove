import { useState } from "react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { InviteResult, api, errorMessage } from "../ipc";
import { Modal } from "./Modal";

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
  const [copied, setCopied] = useState(false);

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

  const copy = async () => {
    if (!result) return;
    try {
      await writeText(result.token);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  if (result) {
    return (
      <Modal title={`${result.name} さんの招待`} onClose={onClose} wide>
        <div className="modal__body">
          <p className="warn">
            このトークンは<strong>この画面でしか表示されません</strong>。
            本人だけに渡し、受け渡し後は削除してください。
          </p>
          <div className="invite">
            <div
              className="invite__qr"
              // fast_qr が生成した SVG。入力はこちらで作ったトークンのみ
              dangerouslySetInnerHTML={{ __html: result.qrSvg }}
            />
            <div className="invite__detail">
              <dl className="facts">
                <dt>割当 IP</dt>
                <dd className="mono">{result.ip}</dd>
                <dt>接続先候補</dt>
                <dd className="mono">{result.endpoints.join(", ")}</dd>
                <dt>事前共有鍵</dt>
                <dd>{result.psk ? "あり" : "なし"}</dd>
              </dl>
              <textarea className="token" readOnly value={result.token} rows={3} />
              <button type="button" onClick={() => void copy()}>
                {copied ? "コピーしました" : "トークンをコピー"}
              </button>
              {error && <p className="error-text">{error}</p>}
            </div>
          </div>
          <p className="muted">
            同じ LAN のメンバーは LAN 側の候補、別ネットワークのメンバーは外部の
            候補で接続します。取り消すときはメンバー一覧から削除してください。
          </p>
        </div>
        <div className="modal__actions">
          <button type="button" onClick={onClose}>
            閉じる
          </button>
        </div>
      </Modal>
    );
  }

  return (
    <Modal title="メンバーを招待" onClose={onClose}>
      <div className="modal__body">
        <label className="field">
          <span>名前（省略すると自動で付きます）</span>
          <input
            value={name}
            onChange={(event) => setName(event.target.value)}
            placeholder="alice"
            autoFocus
          />
        </label>
        <label className="field">
          <span>外部の接続先（別ネットワークの人を招く場合）</span>
          <input
            value={externalEndpoint}
            onChange={(event) => setExternalEndpoint(event.target.value)}
            placeholder="203.0.113.5:51820"
            className="mono"
          />
          <small className="muted">
            LAN 内の接続先は自動で入ります。省略しても構いません。
          </small>
        </label>
        <label className="field field--check">
          <input
            type="checkbox"
            checked={psk}
            onChange={(event) => setPsk(event.target.checked)}
          />
          <span>事前共有鍵（PSK）も発行する</span>
        </label>
        {error && <p className="error-text">{error}</p>}
      </div>
      <div className="modal__actions">
        <button type="button" className="button--ghost" onClick={onClose}>
          キャンセル
        </button>
        <button type="button" onClick={() => void submit()} disabled={busy}>
          {busy ? "発行中…" : "招待を発行"}
        </button>
      </div>
    </Modal>
  );
}
