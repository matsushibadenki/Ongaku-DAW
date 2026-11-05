import { TimelineCanvas } from "./TimelineCanvas.jsx";
import { TrackLaneList } from "./TrackLaneList.jsx";

export function EditorMap({ t, session, selectedTrackId, onSelectTrack, selectedClipKeys, onSelectClip, onEditClips }) {
  const { project, transport, seek, setTrackMute, setTrackSolo, setTrackGain, setTrackPan, reorderTracks } = session;
  return (
    <section className="editor-map" aria-label={t.timeline} style={{ "--track-count": project?.tracks.length ?? 0 }}>
      <div className="timeline-layout">
        <TrackLaneList t={t} tracks={project?.tracks ?? []} selectedTrackId={selectedTrackId}
          onSelectTrack={onSelectTrack} onMute={setTrackMute} onSolo={setTrackSolo}
          onGain={setTrackGain} onPan={setTrackPan} onReorder={reorderTracks} />
        <TimelineCanvas busy={session.busy} onEditClips={onEditClips} selectedClipKeys={selectedClipKeys} onSelectClip={onSelectClip} t={t} project={project} transport={transport} selectedTrackId={selectedTrackId}
          onSelectTrack={onSelectTrack} onSeek={seek} />
      </div>
    </section>
  );
}
