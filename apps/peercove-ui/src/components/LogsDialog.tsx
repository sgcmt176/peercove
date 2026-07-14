import { useCallback, useEffect, useRef, useState } from "react";
import { LogEntry, api, errorMessage, formatLogTime } from "../ipc";
import { t } from "../i18n";

/** ログの取りに行く間隔。開いている間だけポーリングする。 */
const POLL_INTERVAL_MS = 1000;
/** 画面に保持する行数。デーモン側のリングバッファ(500 行)と合わせる。 */
const MAX_LINES = 500;

const LEVELS = ["ERROR", "WARN", "INFO", "DEBUG", "TRACE"] as const;
type Level = (typeof LEVELS)[number];

/**
 * デーモンのログページ(M2-G5 → M3-18 でページ化)。
 *
 * デーモンは別プロセス・別権限なので標準エラー出力は読めない。IPC で
 * リングバッファの差分を取りに行く(`after_seq` 方式)。
 *
 * 表示できるのはデーモンが**記録している**レベルまで。`--log-level warn` で
 * 起動していれば debug 行はそもそも溜まっていない。
 */
export function LogsView() {
  const [lines, setLines] = useState<LogEntry[]>([]);
  const [dropped, setDropped] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [minLevel, setMinLevel] = useState<Level>("INFO");
  const [follow, setFollow] = useState(true);

  // seq は再レンダーを跨いで持ち回るだけなので state にしない(ポーリングの再起動を防ぐ)
  const afterSeq = useRef(0);
  const bottom = useRef<HTMLDivElement>(null);

  const poll = useCallback(async () => {
    try {
      const logs = await api.daemonLogs(afterSeq.current);
      setError(null);
      if (logs.dropped > 0) setDropped((total) => total + logs.dropped);
      if (logs.lines.length === 0) return;
      afterSeq.current = logs.lines[logs.lines.length - 1].seq;
      setLines((current) => [...current, ...logs.lines].slice(-MAX_LINES));
    } catch (e) {
      setError(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    void poll();
    const timer = setInterval(() => void poll(), POLL_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [poll]);

  const threshold = LEVELS.indexOf(minLevel);
  const visible = lines.filter((line) => {
    const index = LEVELS.indexOf(line.level as Level);
    return index === -1 || index <= threshold;
  });

  useEffect(() => {
    if (follow) bottom.current?.scrollIntoView({ block: "end" });
  }, [visible.length, follow]);

  return (
    <section className="card">
      <h2 className="card-title">{t.logs.title}</h2>
      <div className="page-body">
        <div className="row logs__controls">
          <label className="logs__level">
            <span className="muted">{t.logs.level}</span>
            <select
              value={minLevel}
              onChange={(event) => setMinLevel(event.target.value as Level)}
            >
              {LEVELS.map((level) => (
                <option key={level} value={level}>
                  {t.logs.levelOption(level)}
                </option>
              ))}
            </select>
          </label>
          <label className="field--check">
            <input
              type="checkbox"
              checked={follow}
              onChange={(event) => setFollow(event.target.checked)}
            />
            <span>{t.logs.follow}</span>
          </label>
          <button
            type="button"
            className="button--ghost small"
            onClick={() => setLines([])}
          >
            {t.logs.clear}
          </button>
        </div>

        {error && <p className="error-text">{error}</p>}
        {dropped > 0 && <p className="muted small">{t.logs.dropped(dropped)}</p>}

        <div className="logs">
          {visible.length === 0 ? (
            <p className="muted">
              {lines.length === 0 ? t.logs.empty : t.logs.emptyForLevel}
            </p>
          ) : (
            visible.map((line) => (
              <div key={line.seq} className="logs__line">
                <span className="muted logs__time">
                  {formatLogTime(line.unixMs)}
                </span>
                <span className={`logs__level-${line.level.toLowerCase()}`}>
                  {line.level}
                </span>
                <span className="logs__message">{line.message}</span>
              </div>
            ))
          )}
          <div ref={bottom} />
        </div>

        <p className="muted small">{t.logs.footer}</p>
      </div>
    </section>
  );
}
