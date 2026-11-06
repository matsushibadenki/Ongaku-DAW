import test from 'node:test';
import assert from 'node:assert/strict';
import { createMusicalGrid } from '../src/utils/musical-grid.js';

test('preview musical subdivisions preserve fractional periods and exact-tempo boundaries', () => {
  const snap = createMusicalGrid(44100, 120, 16);
  assert.equal(snap(88200), 88200);
  assert.equal(snap(5512), 5513);
  assert.equal(snap(2756), 0);
  assert.equal(snap(2757), 5513);
  assert.equal(createMusicalGrid(48000, 399.8889197445154, 16)(1900), 1800);
  for (const rate of [1, 44100, 48000, 192000]) for (const tempo of [20, 123.456, 127.5, 400]) {
    const grid = createMusicalGrid(rate, tempo, 16);
    for (const sample of [0, 1, 88200, 1000000000000, Number.MAX_SAFE_INTEGER]) assert.equal(grid(grid(sample)), grid(sample));
  }
  assert.throws(() => createMusicalGrid(0, 120, 16));
  assert.throws(() => createMusicalGrid(48000, 120, 3));
  assert.throws(() => snap(-1));
  assert.throws(() => snap(Number.MAX_SAFE_INTEGER + 1));
});

test('preview fractional musical edit survives history and preview storage', async () => {
  const { engineApi } = await import('../src/api/engine.js');
  const memory = new Map();
  globalThis.localStorage = { getItem: key => memory.get(key), setItem: (key,value) => memory.set(key,value) };
  await engineApi.configureProject(44100);
  await engineApi.writeMidiNote(4, 4100, 4000, 88100, 1000, 60);
  const before = structuredClone(await engineApi.project());
  const after = await engineApi.editMidiClip(4, 4100, 'quantize', 16, 4000);
  assert.equal(after.tracks[3].midiClips[0].notes.find(n=>n.id===4000).start, 88200);
  assert.deepEqual(await engineApi.undo(), before);
  assert.deepEqual(await engineApi.redo(), after);
  await engineApi.saveProject(true);
  await engineApi.undo();
  assert.deepEqual(await engineApi.loadProject(), after);
});
