import { useCallback, useEffect, useMemo, useState } from "react";
import {
  DiagnosticCheck,
  DiagnosticReport,
  DiagnosticStatus,
  api,
  errorMessage,
} from "../ipc";
import { t } from "../i18n";

export function DiagnosticsView({ configPath }: { configPath: string }) {
  const [report, setReport] = useState<DiagnosticReport | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saveConfirm, setSaveConfirm] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);

  const run = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      setReport(await api.diagnoseNetwork(configPath));
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }, [configPath]);

  useEffect(() => {
    void run();
  }, [run]);

  const issues = useMemo(
    () => report?.checks.filter((check) => check.status !== "pass") ?? [],
    [report],
  );
  const passed = useMemo(
    () => report?.checks.filter((check) => check.status === "pass") ?? [],
    [report],
  );

  const save = async () => {
    if (!report) return;
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      const path = await api.saveDiagnosticReport(report);
      if (path) setNotice(t.diagnostics.saved(path));
      setSaveConfirm(false);
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="diagnostics">
      <header className="page-head diagnostics__head">
        <div>
          <h2>{t.diagnostics.title}</h2>
          <p className="muted">{t.diagnostics.lead}</p>
        </div>
        <div className="diagnostics__actions">
          <button type="button" onClick={() => void run()} disabled={busy}>
            {busy ? t.common.running : t.diagnostics.rerun}
          </button>
          <button
            type="button"
            className="button--ghost"
            onClick={() => setSaveConfirm(true)}
            disabled={!report || busy}
          >
            {t.diagnostics.export}
          </button>
        </div>
      </header>

      {error && <p className="error" role="alert">{error}</p>}
      {notice && <p className="notice" role="status">{notice}</p>}
      {!report && !error && <p className="muted">{t.diagnostics.running}</p>}

      {report && (
        <>
          <div className={`diagnostics__overall diagnostics__overall--${report.overall}`}>
            <strong>{t.diagnostics.overall[report.overall]}</strong>
            <span className="muted small">
              {new Date(report.generated_at_unix_ms).toLocaleString()}
            </span>
          </div>

          {issues.length === 0 ? (
            <p className="notice">{t.diagnostics.noIssues}</p>
          ) : (
            <div className="diagnostics__list">
              {issues.map((check) => <CheckCard key={check.id} check={check} />)}
            </div>
          )}

          {passed.length > 0 && (
            <details className="diagnostics__passed">
              <summary>{t.diagnostics.passed(passed.length)}</summary>
              <div className="diagnostics__list">
                {passed.map((check) => <CheckCard key={check.id} check={check} />)}
              </div>
            </details>
          )}
        </>
      )}

      {saveConfirm && report && (
        <div className="card diagnostics__confirm" role="alertdialog" aria-modal="true">
          <h3>{t.diagnostics.exportConfirmTitle}</h3>
          <p>{t.diagnostics.exportConfirmBody}</p>
          <p className="muted small">{t.diagnostics.exportConfirmDetail}</p>
          <div className="row-actions">
            <button type="button" onClick={() => setSaveConfirm(false)} disabled={busy}>
              {t.common.cancel}
            </button>
            <button type="button" onClick={() => void save()} disabled={busy}>
              {busy ? t.common.saving : t.diagnostics.exportConfirm}
            </button>
          </div>
        </div>
      )}
    </section>
  );
}

function CheckCard({ check }: { check: DiagnosticCheck }) {
  const copy = (t.diagnostics.check as Record<string, { summary: string; action: string }>)[check.id] ?? {
    summary: check.id,
    action: t.diagnostics.unknownAction,
  };
  return (
    <article className={`diagnostics__check diagnostics__check--${check.status}`}>
      <span className="diagnostics__status" aria-label={statusLabel(check.status)}>
        {statusIcon(check.status)}
      </span>
      <div>
        <strong>{copy.summary}</strong>
        <p className="muted small">{copy.action}</p>
        {Object.keys(check.evidence).length > 0 && (
          <dl className="diagnostics__evidence">
            {Object.entries(check.evidence).map(([key, value]) => (
              <div key={key}>
                <dt>{key}</dt>
                <dd className="mono">{value}</dd>
              </div>
            ))}
          </dl>
        )}
      </div>
    </article>
  );
}

function statusIcon(status: DiagnosticStatus) {
  return status === "pass" ? "✓" : status === "fail" ? "×" : status === "warning" ? "!" : "?";
}

function statusLabel(status: DiagnosticStatus) {
  return t.diagnostics.status[status];
}
