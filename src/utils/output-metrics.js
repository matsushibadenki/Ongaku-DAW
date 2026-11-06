const count = (value) => (value ?? 0).toLocaleString();
const us = (value) => `${Math.round(value ?? 0).toLocaleString()} μs`;

export function outputFramesText(output) {
  if (!output?.periodCount || output.minimumFrames == null || output.maximumFrames == null) return "—";
  return `${count(output.minimumFrames)}–${count(output.maximumFrames)} (${output.lastFrames == null ? "—" : count(output.lastFrames)})`;
}

export function outputMetricsRows(output, t) {
  if (!output) return [];
  const timed = output.periodCount > 0;
  const faults = `${t.malformedFrames}: ${count(output.malformedFrameCount)} · ${t.outputCapacity}: ${count(output.capacityExceededCount)} · ${t.nonFiniteSamples}: ${count(output.nonFiniteSampleCount)}`;
  return [
    [t.outputFrames, outputFramesText(output), t.outputFramesHelp],
    [t.bufferMismatches, count(output.bufferMismatchCount), t.outputFramesHelp],
    [t.outputCpu, timed ? `${((output.cpuLoad ?? 0) * 100).toFixed(2)}%` : "—", `${t.outputBodyHelp} · ${t.average} / ${t.maximum}: ${timed ? `${us(output.averageCallbackUs)} / ${us(output.maximumCallbackUs)}` : "—"}`],
    [t.driverAdapterErrors, `${count(output.driverErrorCount)} / ${count(output.adapterErrorCount)}`, faults],
    [t.outputMisses, count(output.deadlineMissCount), t.outputBodyHelp],
  ];
}
