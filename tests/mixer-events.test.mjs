import test from "node:test";
import assert from "node:assert/strict";
import { engineApi } from "../src/api/engine.js";

const memory = new Map();
globalThis.localStorage = { getItem: key => memory.get(key), setItem: (key, value) => memory.set(key, value) };

test("mixer event API validates atomically and preserves scheduling through history and preview storage", async () => {
  const before = await engineApi.project();
  let snapshot = before;
  for (const [id, [parameter, value]] of [["gain", 0.4], ["pan", -0.6], ["mute", true], ["solo", true]].entries()) {
    snapshot = await engineApi.setMixerEvent(4, { id, position: 129, change: { parameter, value } });
  }
  assert.equal(snapshot.tracks[3].mixerEvents.length, 4);
  const status = await engineApi.documentStatus();
  for (const event of [
    { id: Number.MAX_SAFE_INTEGER + 1, position: 17, change: { parameter: "gain", value: 1 } },
    { id: 9, position: -1, change: { parameter: "gain", value: 1 } },
    { id: 9, position: 17, change: { parameter: "pan", value: NaN } },
    { id: 9, position: 17, change: { parameter: "mute", value: 1 } },
  ]) await assert.rejects(engineApi.setMixerEvent(4, event));
  assert.deepEqual(await engineApi.project(), snapshot);
  assert.deepEqual(await engineApi.documentStatus(), status);
  const undone = await engineApi.undo();
  assert.equal(undone.tracks[3].mixerEvents.length, 3);
  assert.deepEqual(await engineApi.redo(), snapshot);
  await engineApi.saveProject();
  await engineApi.removeMixerEvent(4, 2);
  assert.equal((await engineApi.project()).tracks[3].mixerEvents.length, 3);
  await engineApi.loadProject();
  assert.deepEqual(await engineApi.project(), snapshot);
});

test("preview reports no native adoption and rejects imprecise revision cursors", async () => {
  const status = await engineApi.publicationStatus();
  assert.equal(status.engine, null);
  assert.equal(status.preparation, null);
  assert.equal(status.streamReadiness, null);
  assert.equal(status.requestedRevision, null);
  await assert.rejects(engineApi.publicationStatus(Number.MAX_SAFE_INTEGER + 1));
  await assert.rejects(engineApi.publicationStatus(-1));
});
