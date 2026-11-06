import { outputFramesText } from "../utils/output-metrics.js";
import { Icon } from "./Icons.jsx";
import { transportStatusHelp } from "../utils/transport-status.js";

export function StatusBar({ t, config, metrics, running, previewMode, publicationStatus }) {
  const receipt = running && !previewMode ? publicationStatus?.streamReadiness : null;
  const current = receipt?.graphRevision === publicationStatus?.engine?.adoptedRevision;
  const state = current ? receipt?.readiness?.state : null;
  const labels = { ready: t.streamReady, waiting: t.streamWaiting, failed: t.streamFailed, capacityExceeded: t.streamCapacity };
  const label = labels[state];
  const detail = label ? `${label} · ${receipt.readiness.readyPages}/${receipt.readiness.requiredPages} ${t.streamPages}` : "";
  const commandHelp = running && !previewMode ? transportStatusHelp(publicationStatus?.transportCommands, t) : "";
  const revisionHelp = [publicationStatus?.preparation?.pending ? `${t.graphRevisionHelp} · ${t.graphPreparation}` : t.graphRevisionHelp, commandHelp].filter(Boolean).join(" · ");
  const cpu = `${((metrics?.cpuLoad ?? 0) * 100).toFixed(2)}%`;
  return (
    <footer className="status-bar">
      <p>
        <span>{previewMode ? t.previewMode : t.nativeMode}</span>
        <span>{config.sampleRate.toLocaleString()} Hz</span>
        <span>{config.channels} ch</span>
        <span title={`${t.outputFramesHelp} · ${outputFramesText(metrics?.output)}`}>{config.bufferSize} spl</span>
      </p>
      <p aria-live="polite">
        {label ? <span className="stream-readiness" data-state={state} title={detail} aria-label={detail} role="status">
          <Icon name={state === "ready" ? "wave" : state === "waiting" ? "pause" : "alert"} size={12} />
        </span> : null}
        <span className="transport-command-status" title={revisionHelp}>{t.audioRevision} {publicationStatus?.engine ? `${publicationStatus.engine.adoptedRevision}/${publicationStatus.engine.acceptedRevision}` : "—"}{publicationStatus?.preparation?.pending ? " …" : ""}</span>
        <span>{t.cpu} {running ? cpu : "—"}</span>
        <span title={t.processTimingHelp}>{t.processMisses} {metrics?.processDeadlineMissCount ?? metrics?.xrunCount ?? 0}</span>
        <span>{t.callbacks} {metrics?.callbackCount?.toLocaleString() ?? "—"}</span>
      </p>
    </footer>
  );
}
