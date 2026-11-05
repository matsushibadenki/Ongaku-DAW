import { Icon } from "./Icons.jsx";
import { TransportBar } from "./TransportBar.jsx";

export function AppHeader({ t, session, transportProps }) {
  const { config, running, busy, toggleEngine, previewMode } = session;
  return (
    <header className="app-header">
      <TransportBar t={t} {...transportProps} />
      <div className="session-format" aria-label={t.audioFormat}>
        <span className={running ? "status-light is-live" : "status-light"} aria-hidden="true" />
        <strong>{previewMode ? t.preview : running ? t.online : t.offline}</strong>
        <span>{config.sampleRate / 1_000} kHz · {config.bufferSize} spl</span>
      </div>
      <button className="engine-button" type="button" data-state={busy ? "loading" : running ? "success" : "default"}
        aria-label={busy ? t.wait : running ? t.audioOn : t.audioOff} title={busy ? t.wait : running ? t.audioOn : t.audioOff}
        disabled={busy} onClick={toggleEngine} aria-pressed={running}>
        <Icon name="power" />
        <span>{busy ? t.wait : running ? t.audioOn : t.audioOff}</span>
      </button>
    </header>
  );
}
