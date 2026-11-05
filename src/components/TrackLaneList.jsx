import { useRef, useState } from "react";
import { MiniSlider } from "./MiniSlider.jsx";
import { MiniVolume } from "./MiniVolume.jsx";

const gainToDb = (gain) => gain <= 0 ? "−∞" : `${(20 * Math.log10(gain)).toFixed(1)} dB`;

export function TrackLaneList({ t, tracks, selectedTrackId, onSelectTrack, onMute, onSolo, onGain, onPan, onReorder }) {
  const [draggedTrackId, setDraggedTrackId] = useState(null);
  const draggedIdRef = useRef(null);
  const reorderedRef = useRef(false);
  const tracksRef = useRef(tracks);
  tracksRef.current = tracks;

  const finishDrag = () => setDraggedTrackId(null);
  const moveTrack = (sourceId, targetId, complete = true) => {
    if (sourceId === null || sourceId === targetId) return complete && finishDrag();
    const next = [...tracksRef.current];
    const from = next.findIndex((track) => track.id === sourceId);
    const to = next.findIndex((track) => track.id === targetId);
    if (from < 0 || to < 0) return complete && finishDrag();
    const [moved] = next.splice(from, 1);
    next.splice(to, 0, moved);
    tracksRef.current = next;
    reorderedRef.current = true;
    onReorder(next.map((track) => track.id));
    if (complete) finishDrag();
  };

  return (
    <div className="track-lane-list" aria-label={t.tracks} style={{ "--track-count": tracks.length }}>
      <div className="track-ruler-spacer"><span>{t.tracks}</span></div>
      {tracks.map((track, index) => (
        <div className={track.id === selectedTrackId ? "track-lane-head is-selected" : "track-lane-head"} key={track.id}
          data-track-id={track.id}
          data-role={track.role}
          onPointerDown={(event) => {
            if (event.button !== 0 || event.target.closest(".track-toggles, .mini-slider, .mini-volume")) return;
            draggedIdRef.current = track.id;
            reorderedRef.current = false;
            setDraggedTrackId(track.id);
            event.currentTarget.setPointerCapture(event.pointerId);
          }}
          onPointerMove={(event) => {
            const sourceId = draggedIdRef.current;
            if (sourceId === null) return;
            const target = document.elementFromPoint(event.clientX, event.clientY)?.closest("[data-track-id]");
            const targetId = target ? Number(target.dataset.trackId) : null;
            if (targetId !== null) moveTrack(sourceId, targetId, false);
          }}
          onPointerUp={(event) => {
            if (draggedIdRef.current !== null) {
              if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
              if (!reorderedRef.current) onSelectTrack(track.id);
              draggedIdRef.current = null;
              reorderedRef.current = false;
              finishDrag();
            }
          }}
          onPointerCancel={() => { draggedIdRef.current = null; reorderedRef.current = false; finishDrag(); }}
          aria-grabbed={draggedTrackId === track.id}>
          <button className="track-name" type="button" onClick={() => onSelectTrack(track.id)}>
            <span>{String(index + 1).padStart(2, "0")}</span>
            <strong>{t.trackRoles[track.role]}</strong>
          </button>
          <div className="track-toggles">
            <button type="button" data-state={track.muted ? "success" : "default"} aria-pressed={track.muted}
              aria-label={`${t.mute} ${t.trackRoles[track.role]}`} onClick={() => onMute(track.id, !track.muted)}>M</button>
            <button type="button" data-state={track.solo ? "success" : "default"} aria-pressed={track.solo}
              aria-label={`${t.solo} ${t.trackRoles[track.role]}`} onClick={() => onSolo(track.id, !track.solo)}>S</button>
          </div>
          <MiniVolume label={t.level} min={0} max={1.25} step={0.01} value={track.gain}
            displayValue="" onChange={(value) => onGain(track.id, value)} />
          <MiniSlider label={t.pan} min={-1} max={1} step={0.01} value={track.pan}
            displayValue={track.pan === 0 ? "C" : track.pan < 0 ? `L${Math.round(Math.abs(track.pan) * 100)}` : `R${Math.round(track.pan * 100)}`}
            onChange={(value) => onPan(track.id, value)} />
          <output className="track-db">{gainToDb(track.gain)}</output>
        </div>
      ))}
    </div>
  );
}
