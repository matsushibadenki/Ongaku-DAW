import { applyNoteSelection } from './midi.js';

export const midiClipKey = (trackId, clipId) => `${trackId}:${clipId}`;
export const midiNoteKey = ({trackId, clipId, noteId}) => `${trackId}:${clipId}:${noteId}`;
export function midiSources(project) {
  return (project?.tracks ?? []).flatMap(track => track.midiClips.map(clip => ({ trackId: track.id, role: track.role, clip }))).map((source, index) => ({ ...source, number: index + 1 }));
}
export function buildMidiView(sources, projectLength) {
  return {
    id: sources.map(source => midiClipKey(source.trackId, source.clip.id)).join('|'),
    durationSamples: projectLength,
    notes: sources.flatMap(({trackId, role, clip}) => clip.notes.map(note => {
      const target = { trackId, clipId: clip.id, noteId: note.id };
      return { ...note, id: midiNoteKey(target), target, role, clipKey: midiClipKey(trackId, clip.id),
        start: clip.timelineStart + note.start, minStart: clip.timelineStart, maxEnd: clip.timelineStart + clip.durationSamples };
    })),
  };
}
export function editProjectMidiNotes(project, targets, edit) {
  const sources = midiSources(project);
  const view = buildMidiView(sources, project.lengthSamples);
  const ids = targets.map(midiNoteKey);
  const grid = Math.max(1, Math.round(project.sampleRate * 60 / project.tempoBpm * 4 / (edit.division ?? 16)));
  const updated = applyNoteSelection(view, ids, edit, grid);
  const byId = new Map(updated.map(note => [note.id, note]));
  const result = structuredClone(project);
  for (const track of result.tracks) for (const clip of track.midiClips) {
    clip.notes = clip.notes.flatMap(note => {
      const value = byId.get(midiNoteKey({trackId:track.id,clipId:clip.id,noteId:note.id}));
      return value ? [{ ...note, start: value.start - clip.timelineStart, key: value.key, durationSamples: value.durationSamples }] : [];
    }).sort((a,b) => a.start - b.start || a.id - b.id);
  }
  return result;
}
