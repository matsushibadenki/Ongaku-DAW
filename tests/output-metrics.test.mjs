import test from "node:test";
import assert from "node:assert/strict";
import { messages } from "../src/i18n.js";
import { outputFramesText, outputMetricsRows } from "../src/utils/output-metrics.js";

test("unobserved output never fabricates callback dimensions or measured load", () => {
  assert.equal(outputFramesText(null), "—");
  for (const t of Object.values(messages)) {
    assert.deepEqual(outputMetricsRows(null, t), []);
    const rows = outputMetricsRows({ periodCount: 0, cpuLoad: 0 }, t);
    assert.equal(rows[0][1], "—"); assert.equal(rows[2][1], "—");
  }
});
test("delivered frames, typed faults and output load are labeled independently in every language", () => {
  const data = { periodCount: 2, minimumFrames: 64, maximumFrames: 256, lastFrames: 128, bufferMismatchCount: 2, cpuLoad: 0.125, driverErrorCount: 1, adapterErrorCount: 3, malformedFrameCount: 1, capacityExceededCount: 1, nonFiniteSampleCount: 1, deadlineMissCount: 4 };
  assert.equal(outputFramesText(data), "64–256 (128)");
  for (const t of Object.values(messages)) {
    const rows = outputMetricsRows(data, t);
    assert.equal(rows[2][1], "12.50%"); assert.equal(rows[3][1], "1 / 3"); assert.equal(rows[4][1], "4");
    assert.ok(rows[3][2].includes(t.malformedFrames)); assert.ok(rows[3][2].includes(t.outputCapacity)); assert.ok(rows[3][2].includes(t.nonFiniteSamples));
    assert.ok(rows[0][2].includes(t.outputFramesHelp));
  }
});
