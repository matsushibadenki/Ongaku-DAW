import { clamp, snapSample } from './midi.js';
import { midiClipKey } from './midi-clips.js';

export function selectedMidiClips(project, keys) {
  const wanted = new Set(keys);
  return project.tracks.flatMap(track => track.midiClips.filter(clip => wanted.has(midiClipKey(track.id,clip.id)))
    .map(clip => ({trackId:track.id,clip})));
}
export function clipEditDelta(clips, length, edit) {
  if (edit.type === 'split') return 0;
  if (!Number.isSafeInteger(edit.deltaSamples)) throw new Error('Invalid MIDI clip delta');
  const left = Math.max(...clips.map(clip => edit.type === 'trimRight' ? 1-clip.durationSamples : -clip.timelineStart));
  const right = Math.min(...clips.map(clip => edit.type === 'trimLeft' ? clip.durationSamples-1 : length-clip.timelineStart-clip.durationSamples));
  return clamp(edit.deltaSamples,left,right);
}
function crop(clip, start, end, id) {
  return { ...clip, id, timelineStart:start, durationSamples:end-start, notes:clip.notes.flatMap(note => {
    const originalStart=clip.timelineStart+note.start;
    const left=Math.max(originalStart,start); const right=Math.min(originalStart+note.durationSamples,end);
    return left<right ? [{...note,start:left-start,durationSamples:right-left}] : [];
  }).sort((a,b)=>a.start-b.start||a.id-b.id) };
}
export function editProjectMidiClips(project, targets, edit) {
  const keys=targets.map(target=>midiClipKey(target.trackId,target.clipId));
  const selected=selectedMidiClips(project,keys);
  if (!keys.length || new Set(keys).size!==keys.length || selected.length!==keys.length
    || !['move','duplicate','trimLeft','trimRight','split'].includes(edit.type)) throw new Error('Invalid MIDI clip selection');
  if (edit.type==='split' && (!Number.isSafeInteger(edit.sample) || !selected.some(({clip})=>edit.sample>clip.timelineStart&&edit.sample<clip.timelineStart+clip.durationSamples))) throw new Error('Split point must be inside a selected clip');
  const delta=clipEditDelta(selected.map(source=>source.clip),project.lengthSamples,edit);
  const candidate=structuredClone(project);
  for (const source of selected) {
    const track=candidate.tracks.find(track=>track.id===source.trackId);
    const index=track.midiClips.findIndex(clip=>clip.id===source.clip.id);
    const original=source.clip; const start=original.timelineStart; const end=start+original.durationSamples;
    const nextId=()=> {
      const id=[...track.clips,...track.midiClips].reduce((max,clip)=>Math.max(max,clip.id),0)+1;
      if (!Number.isSafeInteger(id)) throw new Error('Clip identifiers exhausted');
      return id;
    };
    if (edit.type==='move') track.midiClips[index].timelineStart=start+delta;
    else if (edit.type==='duplicate') track.midiClips.push({...structuredClone(original),id:nextId(),timelineStart:start+delta});
    else if (edit.type==='trimLeft') track.midiClips[index]=crop(original,start+delta,end,original.id);
    else if (edit.type==='trimRight') track.midiClips[index]=crop(original,start,end+delta,original.id);
    else if (edit.sample>start && edit.sample<end) {
      const id=nextId(); track.midiClips[index]=crop(original,start,edit.sample,original.id);
      track.midiClips.push(crop(original,edit.sample,end,id));
    }
  }
  return candidate;
}
export function clipDragEdit(gesture, x, projectLength, width, grid) {
  const delta=(x-gesture.originX)/width*projectLength*1.25;
  const anchor=gesture.type==='trimRight' ? gesture.clip.timelineStart+gesture.clip.durationSamples : gesture.clip.timelineStart;
  return {type:gesture.type,deltaSamples:snapSample(anchor+delta,grid)-anchor};
}
