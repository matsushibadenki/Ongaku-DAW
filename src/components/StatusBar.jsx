export function StatusBar({ t, config, metrics, running, previewMode, publicationStatus }) {
  const cpu = `${((metrics?.cpuLoad ?? 0) * 100).toFixed(2)}%`;
  return (
    <footer className="status-bar">
      <p>
        <span>{previewMode ? t.previewMode : t.nativeMode}</span>
        <span>{config.sampleRate.toLocaleString()} Hz</span>
        <span>{config.channels} ch</span>
        <span>{config.bufferSize} spl</span>
      </p>
      <p aria-live="polite">
        <span title={publicationStatus?.preparation?.pending ? `${t.graphRevisionHelp} · ${t.graphPreparation}` : t.graphRevisionHelp}>{t.audioRevision} {publicationStatus?.engine ? `${publicationStatus.engine.adoptedRevision}/${publicationStatus.engine.acceptedRevision}` : "—"}{publicationStatus?.preparation?.pending ? " …" : ""}</span>
        <span>{t.cpu} {running ? cpu : "—"}</span>
        <span>{t.xruns} {metrics?.xrunCount ?? 0}</span>
        <span>{t.callbacks} {metrics?.callbackCount?.toLocaleString() ?? "—"}</span>
      </p>
    </footer>
  );
}
