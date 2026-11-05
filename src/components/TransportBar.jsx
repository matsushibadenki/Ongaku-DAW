import { Icon } from "./Icons.jsx";
import { formatBarsBeats, formatClock } from "../utils/time.js";

export function TransportBar({ t, project, transport, running, busy, onTogglePlayback, onStop, onSeek, onToggleLoop }) {
  const sampleRate = transport?.sampleRate ?? project?.sampleRate ?? 48_000;
  const tempo = transport?.tempoBpm ?? project?.tempoBpm ?? 120;
  const signature = transport?.timeSignature ?? project?.timeSignature ?? { numerator: 4, denominator: 4 };
  const sample = transport?.currentSample ?? 0;
  const playing = transport?.playing ?? false;
  const looped = Boolean(transport?.loopRange);
  return (
    <section className="transport-bar" aria-label={t.transport}>
      <div className="transport-actions">
        <button className="icon-button" type="button" onClick={() => onSeek(0)} disabled={busy}
          aria-label={t.toStart} title={t.toStart}><Icon name="rewind" /></button>
        <button className="play-button" type="button" data-state={playing ? "success" : running ? "default" : "idle"}
          onClick={onTogglePlayback} disabled={busy} aria-label={playing ? t.pause : t.play}>
          <Icon name={playing ? "pause" : "play"} />
          <span>{playing ? t.pause : t.play}</span>
        </button>
        <button className="icon-button" type="button" onClick={onStop} disabled={!running || busy}
          aria-label={t.stopTransport} title={t.stopTransport}><Icon name="stop" /></button>
        <button className="icon-button" type="button" data-state={looped ? "success" : "default"}
          onClick={onToggleLoop} disabled={!running || busy} aria-pressed={looped}
          aria-label={t.loop} title={t.loop}><Icon name="loop" /></button>
      </div>
      <div className="transport-readout" aria-live="off">
        <output className="position-primary">{formatBarsBeats(sample, sampleRate, tempo, signature.numerator)}</output>
        <output className="position-secondary">{formatClock(sample, sampleRate)}</output>
      </div>
      <dl className="tempo-readout">
        <div><dt>{t.tempo}</dt><dd>{tempo.toFixed(1)}</dd></div>
        <div><dt>{t.meter}</dt><dd>{signature.numerator}/{signature.denominator}</dd></div>
      </dl>
    </section>
  );
}
