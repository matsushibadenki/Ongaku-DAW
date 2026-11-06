import test from "node:test";
import assert from "node:assert/strict";
import { engineApi } from "../src/api/engine.js";
import { transportStatusHelp } from "../src/utils/transport-status.js";
import { messages } from "../src/i18n.js";
const memory = new Map();
globalThis.localStorage = { getItem: key => memory.get(key), setItem: (key, value) => memory.set(key, value) };

test("Transport IDs validate epochs and preview never fabricates native receipts", async () => {
  const status = await engineApi.publicationStatus();
  assert.equal(status.transportCommands, null);
  for (const acceptance of [null, {}, { engineEpoch: -1, commandId: 1 }, { engineEpoch: 1, commandId: 0 }, { engineEpoch: 1, commandId: 1.5 }, { engineEpoch: 1, commandId: Number.MAX_SAFE_INTEGER + 1 }]) {
    await assert.rejects(engineApi.transportCommandStatus(acceptance));
    await assert.rejects(engineApi.cancelTransportCommand(acceptance));
  }
  const acceptance = { engineEpoch: 1, commandId: 3 };
  assert.equal(await engineApi.transportCommandStatus(acceptance), null);
  assert.equal(await engineApi.cancelTransportCommand(acceptance), false);
  assert.equal(await engineApi.play(), null);
  assert.equal(await engineApi.seek(40), null);
  assert.equal(await engineApi.pause(), null);
});


test("Transport hover text correlates IDs and distinguishes cancellation after a state change", () => {
  for (const t of Object.values(messages)) {
    const protocol = { latest: { commandId: 8, result: { commandId: 8, state: "waiting", effectApplied: true } } };
    assert.ok(transportStatusHelp(protocol, t).includes(t.transportWaiting));
    protocol.latest.result.state = "canceled"; protocol.latest.result.cancelReason = "superseded";
    const canceled = transportStatusHelp(protocol, t);
    assert.ok(canceled.includes(t.transportCanceled)); assert.ok(canceled.includes(t.transportSuperseded)); assert.ok(canceled.includes(t.transportNoRollback));
    protocol.latest.result.effectApplied = false;
    assert.ok(!transportStatusHelp(protocol, t).includes(t.transportNoRollback));
    protocol.latest.result.commandId = 7;
    assert.ok(transportStatusHelp(protocol, t).includes(t.transportUpdating));
    assert.ok(!transportStatusHelp(protocol, t).includes(t.transportCanceled));
    protocol.latest.historyLost = true;
    assert.ok(transportStatusHelp(protocol, t).includes(t.transportHistoryLost));
    assert.equal(transportStatusHelp(null, t), "");
  }
});
