import { useState } from "react";
import { BackupPreview, NetworkInfo, api, errorMessage } from "../ipc";
import { t } from "../i18n";

export function BackupView({
  networks,
  onChanged,
}: {
  networks: NetworkInfo[];
  onChanged: () => void;
}) {
  const [networkPath, setNetworkPath] = useState(networks[0]?.configPath ?? "");
  const [createPass, setCreatePass] = useState("");
  const [confirmPass, setConfirmPass] = useState("");
  const [backupPath, setBackupPath] = useState("");
  const [restorePass, setRestorePass] = useState("");
  const [preview, setPreview] = useState<BackupPreview | null>(null);
  const [slug, setSlug] = useState("");
  const [replace, setReplace] = useState(false);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const run = async (action: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      await action();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  const create = () => run(async () => {
    if (createPass.length < 12) throw new Error(t.backup.passphraseLength);
    if (createPass !== confirmPass) throw new Error(t.backup.passphraseMismatch);
    const output = await api.createBackup(networkPath, createPass);
    if (output) {
      setNotice(t.backup.created(output));
      setCreatePass("");
      setConfirmPass("");
    }
  });

  const choose = () => run(async () => {
    const path = await api.pickBackup();
    if (path) {
      setBackupPath(path);
      setPreview(null);
    }
  });

  const inspect = () => run(async () => {
    const result = await api.inspectBackup(backupPath, restorePass);
    setPreview(result);
    setSlug(result.networkName);
  });

  const restore = () => run(async () => {
    await api.restoreBackup(backupPath, restorePass, slug, replace);
    setNotice(t.backup.restored);
    onChanged();
  });

  return (
    <section className="prefs__backup" aria-labelledby="backup-title">
      <h3 id="backup-title">{t.backup.title}</h3>
      <p className="muted small">{t.backup.description}</p>
      <div className="backup__columns">
        <form onSubmit={(event) => { event.preventDefault(); void create(); }}>
          <h4>{t.backup.createTitle}</h4>
          <label>{t.backup.network}
            <select value={networkPath} onChange={(event) => setNetworkPath(event.target.value)}>
              {networks.map((network) => <option key={network.configPath} value={network.configPath}>{network.name}</option>)}
            </select>
          </label>
          <label>{t.backup.passphrase}<input type="password" autoComplete="new-password" value={createPass} onChange={(event) => setCreatePass(event.target.value)} /></label>
          <label>{t.backup.confirm}<input type="password" autoComplete="new-password" value={confirmPass} onChange={(event) => setConfirmPass(event.target.value)} /></label>
          <p className="muted small">{t.backup.passphraseHint}</p>
          <button type="submit" disabled={busy || !networkPath}>{t.backup.create}</button>
        </form>
        <div>
          <h4>{t.backup.restoreTitle}</h4>
          <button type="button" className="button--ghost" disabled={busy} onClick={() => void choose()}>{t.backup.choose}</button>
          {backupPath && <p className="mono small backup__path">{backupPath}</p>}
          <label>{t.backup.passphrase}<input type="password" autoComplete="current-password" value={restorePass} onChange={(event) => { setRestorePass(event.target.value); setPreview(null); }} /></label>
          <button type="button" className="button--ghost" disabled={busy || !backupPath || restorePass.length < 12} onClick={() => void inspect()}>{t.backup.preview}</button>
          {preview && <div className="backup__preview">
            <dl className="facts"><dt>{t.backup.network}</dt><dd>{preview.networkName}</dd><dt>{t.backup.role}</dt><dd>{preview.role === "host" ? t.backup.host : t.backup.member}</dd><dt>{t.backup.sourceOs}</dt><dd>{preview.sourceOs}</dd><dt>{t.backup.categories}</dt><dd>{preview.categories.join(", ")}</dd></dl>
            {preview.memberKeyRotationRecommended && <p className="notice">{t.backup.rotateRecommendation}</p>}
            <label>{t.backup.restoreName}<input value={slug} onChange={(event) => setSlug(event.target.value)} /></label>
            <label className="chat__pick-row"><input type="checkbox" checked={replace} onChange={(event) => setReplace(event.target.checked)} /><span>{t.backup.replace}</span></label>
            <p className="muted small">{t.backup.replaceHint}</p>
            <button type="button" disabled={busy || !slug} onClick={() => void restore()}>{t.backup.restore}</button>
          </div>}
        </div>
      </div>
      {error && <p role="alert" className="error-text small">{error}</p>}
      {notice && <p role="status" className="notice">{notice}</p>}
    </section>
  );
}
