import { Icon } from "./Icons.jsx";
import { TransportBar } from "./TransportBar.jsx";

export function AppHeader({ t, session, transportProps }) {
  const { config, running, busy, toggleEngine, previewMode } = session;
  const outputFaulted = session.publicationStatus?.outputFaulted ?? false;
  const powerLabel = busy ? t.wait : outputFaulted ? t.stopOutput : running ? t.audioOn : t.audioOff;
  return (
    <header className="app-header">
      <TransportBar t={t} {...transportProps} busy={busy || outputFaulted} />
      <div className="session-format" aria-label={t.audioFormat}>
        <span className={running && !outputFaulted ? "status-light is-live" : "status-light"} aria-hidden="true" />
        <strong>{outputFaulted ? t.outputFaultTitle : previewMode ? t.preview : running ? t.online : t.offline}</strong>
        <span>{config.sampleRate / 1_000} kHz · {config.bufferSize} spl</span>
      </div>
      <button className="engine-button" type="button" data-state={busy ? "loading" : outputFaulted ? "error" : running ? "success" : "default"}
        aria-label={powerLabel} title={powerLabel}
        disabled={busy} onClick={toggleEngine} aria-pressed={running}>
        <Icon name="power" />
        <span>{powerLabel}</span>
      </button>
    </header>
  );
}
