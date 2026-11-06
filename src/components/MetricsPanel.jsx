import { outputMetricsRows } from "../utils/output-metrics.js";

const formatUs = (value) => `${Math.round(value ?? 0).toLocaleString()} μs`;

export function MetricsPanel({ t, metrics, running }) {
  const values = [
    [t.callbacks, metrics?.callbackCount?.toLocaleString() ?? "—"],
    [t.average, formatUs(metrics?.averageCallbackUs)],
    [t.maximum, formatUs(metrics?.maximumCallbackUs)],
    [t.percentile, formatUs(metrics?.p99_9CallbackUs)],
    [t.deadline, formatUs(metrics?.deadlineUs)],
    [t.cpu, `${((metrics?.cpuLoad ?? 0) * 100).toFixed(2)}%`],
    [t.processMisses, (metrics?.processDeadlineMissCount ?? metrics?.xrunCount ?? 0).toLocaleString(), t.processTimingHelp],
    [t.backendErrors, metrics?.backendErrorCount?.toLocaleString() ?? "0"],
    ...outputMetricsRows(metrics?.output, t),
  ];
  return (
    <section className="panel metrics-panel" aria-labelledby="metrics-title">
      <header className="panel-header"><h2 id="metrics-title">{t.realtime}</h2><span className={running ? "live-dot is-live" : "live-dot"}>{running ? "LIVE" : "IDLE"}</span></header>
      <dl className="metrics-grid">
        {values.map(([label, value, help]) => <div className="metric" key={label} title={help}><dt>{label}</dt><dd>{value}</dd></div>)}
      </dl>
      {!running && <p className="empty-note">{t.idle}</p>}
    </section>
  );
}
