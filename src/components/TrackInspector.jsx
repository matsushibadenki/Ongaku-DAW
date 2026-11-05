import { MiniSlider } from "./MiniSlider.jsx";
import { MiniVolume } from "./MiniVolume.jsx";

const gainToDb = (gain) => gain <= 0 ? "−∞" : `${(20 * Math.log10(gain)).toFixed(1)} dB`;

export function TrackInspector({ t, track, session }) {
  if (!track) return <aside className="track-inspector" aria-label={t.inspector} />;
  const name = t.trackRoles[track.role];
  return (
    <aside className="track-inspector" aria-label={t.inspector}>
      <div className="track-inspector-heading">
        <span>{t.inspector}</span>
        <strong>{name}</strong>
        <small>{track.midiClips?.length ? t.midiTrack : t.audioTrack}</small>
      </div>
      <div className="track-inspector-controls">
        <div className="inspector-value"><span>{t.level}</span><output>{gainToDb(track.gain)}</output></div>
        <MiniVolume label={t.level} min={0} max={1.25} step={0.01} value={track.gain}
          displayValue="" onChange={(value) => session.setTrackGain(track.id, value)} />
        <MiniSlider label={t.pan} min={-1} max={1} step={0.01} value={track.pan}
          displayValue={track.pan === 0 ? "C" : track.pan < 0 ? `L${Math.round(-track.pan * 100)}` : `R${Math.round(track.pan * 100)}`}
          onChange={(value) => session.setTrackPan(track.id, value)} />
        <div className="inspector-switches">
          <button type="button" aria-pressed={track.muted} data-state={track.muted ? "success" : "default"}
            onClick={() => session.setTrackMute(track.id, !track.muted)}>{t.mute}</button>
          <button type="button" aria-pressed={track.solo} data-state={track.solo ? "success" : "default"}
            onClick={() => session.setTrackSolo(track.id, !track.solo)}>{t.solo}</button>
        </div>
      </div>
    </aside>
  );
}
