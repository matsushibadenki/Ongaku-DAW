import { Icon } from "./Icons.jsx";
import { MiniSlider } from "./MiniSlider.jsx";
import { MiniVolume } from "./MiniVolume.jsx";

const gainToDb = (gain) => gain <= 0 ? "−∞" : `${(20 * Math.log10(gain)).toFixed(1)}`;

export function MixerPanel({ t, tracks, selectedTrackId, onSelectTrack, onGain, onPan, onMute, onSolo }) {
  return (
    <section className="mixer-panel" aria-labelledby="mixer-title">
      <header className="section-heading">
        <div><h2 id="mixer-title">{t.mixer}</h2><p>{t.mixerDescription}</p></div>
        <Icon name="sliders" size={20} />
      </header>
      <div className="channel-grid">
        {tracks.map((track, index) => (
          <article className={track.id === selectedTrackId ? "channel-strip is-selected" : "channel-strip"} key={track.id}
            onPointerDown={() => onSelectTrack(track.id)}>
            <header className="channel-header">
              <span>{String(index + 1).padStart(2, "0")}</span>
              <h3>{t.trackRoles[track.role]}</h3>
              <output>{gainToDb(track.gain)} dB</output>
            </header>
            <MiniVolume label={t.level} min={0} max={1.25} step={0.01} value={track.gain}
              displayValue={gainToDb(track.gain)} onChange={(value) => onGain(track.id, value)} />
            <MiniSlider label={t.pan} min={-1} max={1} step={0.01} value={track.pan}
              displayValue={track.pan === 0 ? "C" : track.pan < 0 ? `L${Math.round(Math.abs(track.pan) * 100)}` : `R${Math.round(track.pan * 100)}`}
              onChange={(value) => onPan(track.id, value)} />
            <div className="channel-actions">
              <button type="button" data-state={track.muted ? "success" : "default"} aria-pressed={track.muted}
                onClick={() => onMute(track.id, !track.muted)}>{t.mute}</button>
              <button type="button" data-state={track.solo ? "success" : "default"} aria-pressed={track.solo}
                onClick={() => onSolo(track.id, !track.solo)}>{t.solo}</button>
            </div>
          </article>
        ))}
      </div>
    </section>
  );
}
