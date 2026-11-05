import { useState } from "react";
import { Icon } from "./Icons.jsx";

export function StudioBrowser({ t, project, selectedTrackId, onSelectTrack, selectedClipKeys, onSelectClip }) {
  const [view, setView] = useState("tracks");
  const [query, setQuery] = useState("");
  const tracks = (project?.tracks ?? []).filter((track) =>
    (t.trackRoles[track.role] ?? track.role).toLowerCase().includes(query.toLowerCase()));
  const selected = project?.tracks.find((track) => track.id === selectedTrackId);
  return (
    <aside className="studio-browser" aria-label={t.browser}>
      <div className="browser-tabs" role="tablist" aria-label={t.browser}>
        <button type="button" role="tab" aria-selected={view === "tracks"} onClick={() => setView("tracks")}>{t.tracks}</button>
        <button type="button" role="tab" aria-selected={view === "clips"} onClick={() => setView("clips")}>{t.clips}</button>
      </div>
      <label className="browser-search"><Icon name="search" size={13} />
        <input type="search" aria-label={t.searchTracks} placeholder={t.searchTracks} value={query} onChange={(event) => setQuery(event.target.value)} />
      </label>
      <div className="browser-tree" role="tabpanel" aria-label={view === "tracks" ? t.tracks : t.clips}>
        <div className="browser-tree-root"><Icon name="folder" size={14} /><span>{t.projectBrowser}</span></div>
        {tracks.map((track) => <div className="browser-tree-group" key={track.id}>
          <button type="button" className={selectedTrackId === track.id ? "is-selected" : ""}
            onClick={() => onSelectTrack(track.id)} aria-pressed={selectedTrackId === track.id}>
            <Icon name={track.midiClips?.length ? "note" : "wave"} size={13} />
            <span>{t.trackRoles[track.role]}</span>
          </button>
          {view === "clips" && [...track.clips, ...(track.midiClips ?? [])].map((clip, index) =>
            <button type="button" className={`browser-clip${selectedClipKeys.includes(`${track.id}:${clip.id}`) ? " is-selected" : ""}`} key={`${track.id}-${clip.id}`}
              aria-pressed={selectedClipKeys.includes(`${track.id}:${clip.id}`)} onClick={(event) => track.midiClips.some(item => item.id === clip.id)
                ? onSelectClip(track.id, clip.id, event.shiftKey || event.metaKey || event.ctrlKey) : onSelectTrack(track.id)}>
              <span>{t.trackRoles[track.role]} · {String(index + 1).padStart(2, "0")}</span>
              <small>{(clip.durationSamples / (project.sampleRate || 48000)).toFixed(2)} s</small>
            </button>)}
        </div>)}
        {tracks.length === 0 && <span className="browser-no-results">{t.noResults}</span>}
      </div>
      {selected && <div className="browser-selection">
        <Icon name={selected.midiClips?.length ? "note" : "wave"} size={26} />
        <strong>{t.trackRoles[selected.role]}</strong>
        <span>{selected.midiClips?.length ? t.midiTrack : t.audioTrack}</span>
        <span>{(selected.clips?.length ?? 0) + (selected.midiClips?.length ?? 0)} {t.clips}</span>
      </div>}
    </aside>
  );
}
