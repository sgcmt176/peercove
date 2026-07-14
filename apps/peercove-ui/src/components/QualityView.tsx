import { useEffect, useMemo, useState } from "react";
import { QualityPoint, Tunnel, api, errorMessage, formatBytes } from "../ipc";
import { t } from "../i18n";

type Period = 15 | 60 | 1440 | 10080;
const PERIODS: Array<{ value: Period; label: string }> = [
  { value: 15, label: "15分" },
  { value: 60, label: "1時間" },
  { value: 1440, label: "24時間" },
  { value: 10080, label: "7日" },
];

export function QualityView({ tunnel }: { tunnel: Tunnel }) {
  const [period, setPeriod] = useState<Period>(60);
  const [samples, setSamples] = useState<QualityPoint[]>([]);
  const [peerKey, setPeerKey] = useState("");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [corruptLines, setCorruptLines] = useState(0);

  useEffect(() => {
    let alive = true;
    const load = () => {
      const since = Date.now() - period * 60_000;
      api
        .qualityHistory(tunnel.config, since)
        .then((report) => {
          if (!alive) return;
          setSamples(report.samples);
          setCorruptLines(report.skippedCorruptLines);
          setError(null);
          setLoading(false);
        })
        .catch((reason) => {
          if (!alive) return;
          setError(errorMessage(reason));
          setLoading(false);
        });
    };
    setLoading(true);
    load();
    const timer = window.setInterval(load, 30_000);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [period, tunnel.config]);

  const peers = useMemo(() => {
    const byKey = new Map<string, { key: string; label: string }>();
    for (const sample of samples) {
      byKey.set(sample.publicKey, {
        key: sample.publicKey,
        label: sample.name || sample.ip,
      });
    }
    return [...byKey.values()].sort((a, b) => a.label.localeCompare(b.label));
  }, [samples]);

  useEffect(() => {
    if (!peers.some((peer) => peer.key === peerKey)) {
      setPeerKey(peers[0]?.key ?? "");
    }
  }, [peerKey, peers]);

  const selected = samples.filter((sample) => sample.publicKey === peerKey);
  const latest = selected.at(-1) ?? null;
  const measured = selected.filter((sample) => sample.rttAvgMs !== null);
  const avg = average(measured.map((sample) => sample.rttAvgMs as number));
  const p95 = percentile(measured.map((sample) => sample.rttP95Ms as number), 0.95);
  const loss = weightedLoss(selected);
  const lastConnected = [...selected]
    .reverse()
    .find((sample) => sample.availability === "connected");

  return (
    <div className="quality">
      <section className="card quality__head">
        <div>
          <h2 className="card-title">{t.quality.title}</h2>
          <p className="muted small">{t.quality.description}</p>
        </div>
        <div className="quality__controls">
          <label>
            <span className="sr-only">{t.quality.peer}</span>
            <select value={peerKey} onChange={(event) => setPeerKey(event.target.value)}>
              {peers.map((peer) => (
                <option key={peer.key} value={peer.key}>{peer.label}</option>
              ))}
            </select>
          </label>
          <div className="quality__periods" role="group" aria-label={t.quality.period}>
            {PERIODS.map((item) => (
              <button
                key={item.value}
                type="button"
                className={period === item.value ? "active" : ""}
                aria-pressed={period === item.value}
                onClick={() => setPeriod(item.value)}
              >
                {item.label}
              </button>
            ))}
          </div>
        </div>
      </section>

      {error && <p className="error">{error}</p>}
      {corruptLines > 0 && <p className="notice">{t.quality.corrupt(corruptLines)}</p>}
      {loading && samples.length === 0 ? (
        <section className="card"><p className="muted">{t.quality.loading}</p></section>
      ) : peers.length === 0 ? (
        <section className="card"><p className="muted">{t.quality.empty}</p></section>
      ) : (
        <>
          <section className="quality__summary" aria-label={t.quality.summary}>
            <Metric label={t.quality.latestRtt} value={formatMs(latest?.rttLatestMs ?? null)} />
            <Metric label={t.quality.averageRtt} value={formatMs(avg)} />
            <Metric label={t.quality.p95Rtt} value={formatMs(p95)} />
            <Metric label={t.quality.loss} value={loss === null ? "—" : `${loss.toFixed(1)}%`} />
            <Metric
              label={t.quality.lastConnected}
              value={lastConnected ? formatTime(lastConnected.windowStartUnixMs) : "—"}
              detail={availabilityLabel(latest?.availability)}
            />
          </section>

          <section className="card quality__chart-card">
            <div className="quality__chart-title">
              <div><h3>{t.quality.rttChart}</h3><p className="muted small">{t.quality.gaps}</p></div>
              <span className="quality__legend"><i className="quality__legend-line" />RTT</span>
            </div>
            <RttChart samples={selected} />
          </section>

          <section className="card quality__chart-card">
            <div className="quality__chart-title">
              <div><h3>{t.quality.lossChart}</h3><p className="muted small">{t.quality.lossNote}</p></div>
            </div>
            <LossChart samples={selected} />
          </section>

          <section className="card quality__route">
            <h3>{t.quality.route}</h3>
            <div className="quality__route-strip" aria-label={t.quality.route}>
              {thinSamples(selected, 480).map((sample) => (
                <span
                  key={sample.windowStartUnixMs}
                  className={`quality__route-segment quality__route-segment--${sample.route}`}
                  title={`${formatTime(sample.windowStartUnixMs)}: ${routeLabel(sample.route)}`}
                />
              ))}
            </div>
            <div className="quality__route-legend small">
              <span><i className="direct" />{t.quality.direct}</span>
              <span><i className="relay" />{t.quality.relay}</span>
              <span><i className="trying" />{t.quality.trying}</span>
              <span>{t.quality.switches(selected.reduce((sum, sample) => sum + sample.routeSwitches, 0))}</span>
            </div>
          </section>

          <section className="card">
            <h3>{t.quality.table}</h3>
            <div className="table-scroll">
              <table className="peers quality__table">
                <thead><tr><th>{t.quality.time}</th><th>{t.quality.state}</th><th>RTT</th><th>P95</th><th>{t.quality.jitter}</th><th>{t.quality.loss}</th><th>{t.quality.route}</th><th>{t.quality.transfer}</th></tr></thead>
                <tbody>
                  {[...selected].reverse().slice(0, 30).map((sample) => (
                    <tr key={sample.windowStartUnixMs}>
                      <td>{formatTime(sample.windowStartUnixMs)}</td>
                      <td>{availabilityLabel(sample.availability)}</td>
                      <td>{formatMs(sample.rttAvgMs)}</td>
                      <td>{formatMs(sample.rttP95Ms)}</td>
                      <td>{formatMs(sample.jitterMs)}</td>
                      <td>{sample.lossPercent === null ? "—" : `${sample.lossPercent.toFixed(1)}%`}</td>
                      <td>{routeLabel(sample.route)}</td>
                      <td>{formatBytes(sample.rxBytes + sample.txBytes)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </section>
        </>
      )}
    </div>
  );
}

function Metric({ label, value, detail }: { label: string; value: string; detail?: string }) {
  return <div className="card quality__metric"><span className="muted small">{label}</span><strong>{value}</strong>{detail && <span className="small">{detail}</span>}</div>;
}

function RttChart({ samples }: { samples: QualityPoint[] }) {
  const shown = thinSamples(samples, 480);
  const values = shown.map((sample) => sample.rttAvgMs).filter((value): value is number => value !== null);
  if (values.length === 0) return <p className="muted quality__no-data">{t.quality.noRtt}</p>;
  const max = Math.max(1, ...values) * 1.1;
  const width = 720, height = 170, pad = 24;
  const points = shown.map((sample, index) => sample.rttAvgMs === null ? null : `${xAt(index, shown.length, width, pad)},${height - pad - sample.rttAvgMs / max * (height - pad * 2)}`);
  const segments: string[] = [];
  let current: string[] = [];
  for (const point of points) {
    if (point) current.push(point);
    else if (current.length) { segments.push(current.join(" ")); current = []; }
  }
  if (current.length) segments.push(current.join(" "));
  return <svg className="quality__chart" viewBox={`0 0 ${width} ${height}`} role="img" aria-label={t.quality.rttAria}>
    {[0, .5, 1].map((ratio) => <line key={ratio} x1={pad} x2={width-pad} y1={pad + ratio*(height-pad*2)} y2={pad + ratio*(height-pad*2)} className="quality__grid" />)}
    {segments.map((points, index) => <polyline key={index} points={points} className="quality__rtt-line" />)}
    <text x={pad} y={15} className="quality__axis">{max.toFixed(0)} ms</text><text x={pad} y={height-5} className="quality__axis">0 ms</text>
  </svg>;
}

function LossChart({ samples }: { samples: QualityPoint[] }) {
  const width = 720, height = 120, pad = 24;
  const shown = thinSamples(samples, 480);
  if (shown.every((sample) => sample.lossPercent === null)) return <p className="muted quality__no-data">{t.quality.noLoss}</p>;
  const bar = (width - pad * 2) / Math.max(shown.length, 1);
  return <svg className="quality__chart quality__chart--loss" viewBox={`0 0 ${width} ${height}`} role="img" aria-label={t.quality.lossAria}>
    <line x1={pad} x2={width-pad} y1={height-pad} y2={height-pad} className="quality__grid" />
    {shown.map((sample, index) => sample.lossPercent === null ? null : <rect key={sample.windowStartUnixMs} x={pad+index*bar} y={height-pad-sample.lossPercent/100*(height-pad*2)} width={Math.max(1,bar-1)} height={sample.lossPercent/100*(height-pad*2)} className="quality__loss-bar" />)}
    <text x={pad} y={15} className="quality__axis">100%</text><text x={pad} y={height-5} className="quality__axis">0%</text>
  </svg>;
}

function xAt(index: number, count: number, width: number, pad: number) { return count <= 1 ? width/2 : pad + index/(count-1)*(width-pad*2); }
function average(values: number[]) { return values.length ? values.reduce((a,b)=>a+b,0)/values.length : null; }
function percentile(values: number[], ratio: number) { if (!values.length) return null; const sorted=[...values].sort((a,b)=>a-b); return sorted[Math.min(sorted.length-1,Math.ceil(sorted.length*ratio)-1)]; }
function weightedLoss(samples: QualityPoint[]) { const measured=samples.filter((sample)=>sample.lossPercent !== null); const sent=measured.reduce((sum,s)=>sum+s.probesSent,0); const recv=measured.reduce((sum,s)=>sum+s.probesReceived,0); return sent ? Math.max(0,sent-recv)*100/sent : null; }
function thinSamples(samples: QualityPoint[], limit: number) { const step=Math.max(1,Math.ceil(samples.length/limit)); return samples.filter((_,index)=>index%step===0 || index===samples.length-1); }
function formatMs(value: number | null) { return value === null ? "—" : `${value < 10 ? value.toFixed(1) : value.toFixed(0)} ms`; }
function formatTime(unixMs: number) { return new Intl.DateTimeFormat("ja-JP", { month:"numeric", day:"numeric", hour:"2-digit", minute:"2-digit" }).format(new Date(unixMs)); }
function availabilityLabel(value?: QualityPoint["availability"]) { return value === "connected" ? t.quality.connected : value === "disconnected" ? t.quality.disconnected : t.quality.unmeasured; }
function routeLabel(value: QualityPoint["route"]) { return value === "direct" ? t.quality.direct : value === "trying" ? t.quality.trying : t.quality.relay; }
