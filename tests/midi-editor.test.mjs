import assert from 'node:assert/strict';
import { pathToFileURL } from 'node:url';
const { dragNote, noteBox, noteAtPoint, pitchRange } = await import(pathToFileURL(process.cwd() + '/src/utils/midi.js'));
const original = {id: 1, start: 100, durationSamples: 200, key: 60, velocity: 0.5};
const clip = { durationSamples: 1000, notes: [original] };
const range = pitchRange(clip.notes);
const gesture = { original, width: 1028, height: 370, range, grid: 100, mode: 'move', originX: 0, originY: 0 };
assert.equal(dragNote(gesture, 200, -20, 1000).start, 300);
assert.equal(dragNote(gesture, 200, -20, 1000).key, 62);
assert.equal(dragNote(gesture, 2000, 0, 1000).start, 800);
assert.equal(dragNote(gesture, -2000, 0, 1000).start, 0);
assert.equal(dragNote({...gesture, mode: 'resize'}, 2000, 0, 1000).durationSamples, 900);
assert.equal(dragNote({...gesture, mode: 'resize'}, -2000, 0, 1000).durationSamples, 1);
const box = noteBox(original, clip, 1028, 370, range);
assert.equal(noteAtPoint(box.x+10, box.y+1, clip, 1028, 370, range)?.id, 1);
assert.equal(noteAtPoint(10, box.y, clip, 1028, 370, range), null);
const { engineApi } = await import(pathToFileURL(process.cwd() + '/src/api/engine.js'));
const memory = new Map(); globalThis.localStorage = { getItem: k=>memory.get(k), setItem:(k,v)=>memory.set(k,v) };
let project = await engineApi.writeMidiNote(4, 4100, null, 12000, 6000, 72);
assert.equal(project.tracks[3].midiClips[0].notes.length, 17);
await engineApi.writeMidiNote(4, 4100, 4016, 24000, 12000, 74);
await engineApi.undo(); project = await engineApi.project();
assert.equal(project.tracks[3].midiClips[0].notes.find(n=>n.id===4016).start, 12000);
await engineApi.redo(); await engineApi.saveProject();
await engineApi.writeMidiNote(4, 4100, 4016, 0, 1, 0);
await engineApi.loadProject(); project = await engineApi.project();
assert.equal(project.tracks[3].midiClips[0].notes.find(n=>n.id===4016).key, 74);
await assert.rejects(engineApi.writeMidiNote(4,4100,4016,0,0,60));
console.log('MIDI geometry, boundaries, preview history and storage passed');

const { applyNoteSelection, dragSelection, notesInRectangle } = await import(pathToFileURL(process.cwd() + '/src/utils/midi.js'));
const groupClip = { durationSamples: 1000, notes: [
  { id: 1, start: 100, durationSamples: 100, key: 125, velocity: .5, channel: 1 },
  { id: 2, start: 300, durationSamples: 200, key: 127, velocity: .7, channel: 2 },
  { id: 3, start: 200, durationSamples: 50, key: 60, velocity: .9, channel: 0 },
] };
let group = applyNoteSelection(groupClip, [1,2], { type: 'move', deltaSamples: -1000, semitones: 10 });
assert.deepEqual(group.filter(n=>n.id!==3).map(n=>[n.start,n.key]), [[0,125],[200,127]]);
assert.deepEqual(group.find(n=>n.id===3), groupClip.notes[2]);
group = applyNoteSelection(groupClip, [1,2], { type: 'resize', deltaSamples: -1000 });
assert.equal(group.find(n=>n.id===1).durationSamples,1);
assert.equal(group.find(n=>n.id===2).durationSamples,101);
group = applyNoteSelection(groupClip, [1,2], { type: 'move', deltaSamples: 1000, semitones: -200 });
assert.equal(group.find(n=>n.id===2).start+group.find(n=>n.id===2).durationSamples,1000);
assert.equal(group.find(n=>n.id===2).key-group.find(n=>n.id===1).key,2);
for (const ids of [[],[1,1],[1,999]]) assert.throws(()=>applyNoteSelection(groupClip,ids,{type:'delete'}));
group = applyNoteSelection(groupClip,[1],{type:'quantize',division:16},200);
assert.equal(group.find(n=>n.id===1).start,200);
assert.equal(group.find(n=>n.id===1).durationSamples,100);
assert.deepEqual(group.find(n=>n.id===2),groupClip.notes[1]);
const groupRange = pitchRange(groupClip.notes);
const target = noteBox(groupClip.notes[2],groupClip,1028,370,groupRange);
assert.deepEqual(notesInRectangle(groupClip,{x1:target.x-1,y1:target.y-1,x2:target.x+target.width+1,y2:target.y+target.height+1},1028,370,groupRange),[3]);
const dragged = dragSelection({...gesture, ids:[1,2], original:groupClip.notes[0], range:groupRange},100,0,groupClip);
assert.equal(dragged.notes.find(n=>n.id===2).start-dragged.notes.find(n=>n.id===1).start,200);
const stages = [await engineApi.project()];
for (const edit of [{type:'move',deltaSamples:1000,semitones:1},{type:'resize',deltaSamples:250},
  {type:'quantize',division:16},{type:'delete'}]) {
  await engineApi.editMidiSelection(4,4100,[4000,4001],edit);
  stages.push(await engineApi.project());
}
for (let index=3; index>=0; index--) { await engineApi.undo(); assert.deepEqual(await engineApi.project(),stages[index]); }
for (let index=1; index<=4; index++) { await engineApi.redo(); assert.deepEqual(await engineApi.project(),stages[index]); }
await engineApi.undo();
await engineApi.saveProject();
const savedSelection = await engineApi.project();
await engineApi.editMidiSelection(4,4100,[4000,4001],{type:'delete'});
await engineApi.loadProject();
assert.deepEqual(await engineApi.project(),savedSelection);
await assert.rejects(engineApi.editMidiSelection(4,4100,[4000,999],{type:'delete'}));
assert.deepEqual(await engineApi.project(),savedSelection);
console.log('Selection bounds, marquee, atomic history, and storage passed');

const { buildMidiView, midiSources, editProjectMidiNotes } = await import(pathToFileURL(process.cwd() + '/src/utils/midi-clips.js'));
let multiProject = { sampleRate:8000,tempoBpm:120,lengthSamples:64000, tracks:[{id:1,role:'lead',clips:[],midiClips:[
  {id:1,timelineStart:2500,durationSamples:10000,notes:[{id:1,start:100,durationSamples:2000,key:60,velocity:.5,channel:0}]},
  {id:2,timelineStart:7000,durationSamples:10000,notes:[{id:1,start:600,durationSamples:1000,key:64,velocity:.7,channel:1}]}
]}] };
const multiTargets=[{trackId:1,clipId:1,noteId:1},{trackId:1,clipId:2,noteId:1}];
const multiView = buildMidiView(midiSources(multiProject),64000);
assert.equal(new Set(multiView.notes.map(n=>n.id)).size,2);
assert.deepEqual(multiView.notes.map(n=>n.start),[2600,7600]);
let changed = editProjectMidiNotes(multiProject,multiTargets,{type:'quantize',division:16});
assert.deepEqual(changed.tracks[0].midiClips.map(c=>c.notes[0].start),[500,1000]);
changed = editProjectMidiNotes(multiProject,multiTargets,{type:'move',deltaSamples:-1000,semitones:1});
assert.deepEqual(changed.tracks[0].midiClips.map(c=>c.notes[0].start),[0,500]);
changed = editProjectMidiNotes(multiProject,[multiTargets[0]],{type:'delete'});
assert.equal(changed.tracks[0].midiClips[0].notes.length,0);
assert.equal(changed.tracks[0].midiClips[1].notes.length,1);
assert.throws(()=>editProjectMidiNotes(multiProject,[multiTargets[0],{trackId:99,clipId:1,noteId:1}],{type:'delete'}));
assert.deepEqual(multiProject.tracks[0].midiClips.map(c=>c.notes[0].start),[100,600]);
let created = await engineApi.addMidiClip(4,1000,12000);
const createdClip = created.tracks[3].midiClips.at(-1);
await engineApi.writeMidiNote(4,createdClip.id,null,100,1000,60);
const crossTargets = [{trackId:4,clipId:4100,noteId:4000},{trackId:4,clipId:createdClip.id,noteId:1}];
const crossBefore = await engineApi.project();
await engineApi.editMidiNotes(crossTargets,{type:'move',deltaSamples:2000,semitones:1});
const crossAfter = await engineApi.project();
await engineApi.undo();assert.deepEqual(await engineApi.project(),crossBefore);
await engineApi.redo();assert.deepEqual(await engineApi.project(),crossAfter);
await engineApi.saveProject();await engineApi.editMidiNotes(crossTargets,{type:'delete'});
await engineApi.loadProject();assert.deepEqual(await engineApi.project(),crossAfter);
console.log('Cross-clip identities, project time, bounds, history and persistence passed');

const {editProjectMidiClips,clipDragEdit}=await import(pathToFileURL(process.cwd()+'/src/utils/midi-clip-edit.js'));
const regionProject={sampleRate:8000,tempoBpm:120,lengthSamples:64000,tracks:[{id:4,clips:[],midiClips:[
 {id:1,timelineStart:1000,durationSamples:10000,notes:[{id:1,start:1000,durationSamples:6000,key:64,velocity:.4,channel:2}]},
 {id:2,timelineStart:7000,durationSamples:10000,notes:[]}
]}]};
const regionTargets=[{trackId:4,clipId:1},{trackId:4,clipId:2}];
let regionEdit=editProjectMidiClips(regionProject,regionTargets,{type:'move',deltaSamples:-100000});
assert.deepEqual(regionEdit.tracks[0].midiClips.map(c=>c.timelineStart),[0,6000]);
regionEdit=editProjectMidiClips(regionProject,regionTargets,{type:'duplicate',deltaSamples:16000});
assert.deepEqual(regionEdit.tracks[0].midiClips.slice(2).map(c=>[c.id,c.timelineStart]),[[3,17000],[4,23000]]);
regionEdit.tracks[0].midiClips[2].notes[0].key=10;
assert.equal(regionProject.tracks[0].midiClips[0].notes[0].key,64);
regionEdit=editProjectMidiClips(regionProject,[regionTargets[0]],{type:'trimLeft',deltaSamples:2000});
assert.deepEqual(regionEdit.tracks[0].midiClips[0].notes.map(n=>[n.start,n.durationSamples]),[[0,5000]]);
regionEdit=editProjectMidiClips(regionProject,[regionTargets[0]],{type:'split',sample:5000});
assert.deepEqual(regionEdit.tracks[0].midiClips[0].notes.map(n=>[n.start,n.durationSamples]),[[1000,3000]]);
assert.deepEqual(regionEdit.tracks[0].midiClips[2].notes.map(n=>[n.start,n.durationSamples]),[[0,3000]]);
assert.throws(()=>editProjectMidiClips(regionProject,[regionTargets[0]],{type:'split',sample:1000}));
assert.throws(()=>editProjectMidiClips(regionProject,[regionTargets[0],{trackId:4,clipId:999}],{type:'move',deltaSamples:100}));
assert.deepEqual(clipDragEdit({originX:0,type:'move',clip:{timelineStart:1000}},100,64000,800,1000),{type:'move',deltaSamples:10000});
created=await engineApi.addMidiClip(4,1000,12000);
const regionId=created.tracks[3].midiClips.at(-1).id;
await engineApi.writeMidiNote(4,regionId,null,0,10000,60);
const arrangementTargets=[{trackId:4,clipId:regionId}];
const arrangementStages=[await engineApi.project()];
for (const edit of [{type:'move',deltaSamples:1000},{type:'duplicate',deltaSamples:16000},
 {type:'trimLeft',deltaSamples:1000},{type:'trimRight',deltaSamples:-1000},{type:'split',sample:6000}]) {
 await engineApi.editMidiClips(arrangementTargets,edit);arrangementStages.push(await engineApi.project());
}
for(let i=4;i>=0;i--){await engineApi.undo();assert.deepEqual(await engineApi.project(),arrangementStages[i]);}
for(let i=1;i<=5;i++){await engineApi.redo();assert.deepEqual(await engineApi.project(),arrangementStages[i]);}
await engineApi.saveProject();await engineApi.editMidiClips(arrangementTargets,{type:'move',deltaSamples:100});
await engineApi.loadProject();assert.deepEqual(await engineApi.project(),arrangementStages[5]);
console.log('Clip move, duplicate, trim, split, history and persistence passed');
