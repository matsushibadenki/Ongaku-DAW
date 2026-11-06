// Browser-preview control model only. Native document/audio timing lives in Rust.
export function createMusicalGrid(sampleRate, tempoBpm, subdivision) {
  if (!Number.isInteger(sampleRate) || sampleRate <= 0 || sampleRate > 0xffffffff ||
      !Number.isInteger(subdivision) || subdivision < 1 || subdivision > 128 || (subdivision & (subdivision - 1))) {
    throw new Error("Invalid musical grid");
  }
  const tempo = Number.isFinite(tempoBpm) ? Math.max(20, Math.min(400, tempoBpm)) : 120;
  const view = new DataView(new ArrayBuffer(8));
  view.setFloat64(0, tempo);
  const bits = view.getBigUint64(0);
  const significand = (bits & ((1n << 52n) - 1n)) | (1n << 52n);
  const exponent = (bits >> 52n) & 0x7ffn;
  let numerator = (BigInt(sampleRate) * 240n) << (1075n - exponent);
  let denominator = BigInt(subdivision) * significand;
  if (numerator < denominator) { numerator = 1n; denominator = 1n; }
  return sample => {
    if (!Number.isSafeInteger(sample) || sample < 0) throw new Error("Invalid sample position");
    const index = (BigInt(sample) * denominator + numerator / 2n) / numerator;
    const snapped = (index * numerator + denominator / 2n) / denominator;
    return Number(snapped > BigInt(Number.MAX_SAFE_INTEGER) ? BigInt(Number.MAX_SAFE_INTEGER) : snapped);
  };
}
