export function formatBarsBeats(sample, sampleRate, tempoBpm, numerator = 4) {
  const samplesPerBeat = sampleRate * 60 / tempoBpm;
  const totalBeats = Math.max(0, sample) / samplesPerBeat;
  const bar = Math.floor(totalBeats / numerator) + 1;
  const beat = Math.floor(totalBeats % numerator) + 1;
  const tick = Math.floor((totalBeats % 1) * 960);
  return `${pad(bar, 3)}.${pad(beat, 2)}.${pad(tick, 3)}`;
}

export function formatClock(sample, sampleRate) {
  const totalSeconds = Math.max(0, sample) / sampleRate;
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = Math.floor(totalSeconds % 60);
  const milliseconds = Math.floor((totalSeconds % 1) * 1_000);
  return `${pad(minutes, 2)}:${pad(seconds, 2)}.${pad(milliseconds, 3)}`;
}

const pad = (value, length) => String(value).padStart(length, "0");
