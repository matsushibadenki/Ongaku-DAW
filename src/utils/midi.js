// Clip-local sample coordinates shared by pointer gestures and the inspector.
export const KEY_WIDTH = 28;
export const VELOCITY_HEIGHT = 34;
export const clamp = (value, min, max) => Math.max(min, Math.min(max, value));
export const snapSample = (sample, grid) => Math.round(sample / grid) * grid;
export const pitchRange = (notes) => notes.reduce((range, note) => ({
  min: Math.min(range.min, note.key), max: Math.max(range.max, note.key),
}), { min: 48, max: 84 });
export function noteBox(note, clip, width, height, range = pitchRange(clip.notes)) {
  const rows = range.max - range.min + 1;
  return {
    x: KEY_WIDTH + note.start / clip.durationSamples * (width - KEY_WIDTH),
    y: (range.max - note.key) / rows * height,
    width: Math.max(3, note.durationSamples / clip.durationSamples * (width - KEY_WIDTH)),
    height: Math.max(2, height / rows - 1),
  };
}
export function noteAtPoint(x, y, clip, width, height, range) {
  if (x < KEY_WIDTH || y < 0 || y >= height) return null;
  return [...clip.notes].reverse().find((note) => {
    const box = noteBox(note, clip, width, height, range);
    return x >= box.x && x <= box.x + box.width && y >= box.y && y < box.y + height / (range.max - range.min + 1);
  }) ?? null;
}
export function dragNote(gesture, x, y, clipDuration) {
  const { original, width, height, range, grid, mode, originX, originY } = gesture;
  const delta = (x - originX) / Math.max(1, width - KEY_WIDTH) * clipDuration;
  if (mode === "resize") {
    const end = clamp(snapSample(original.start + original.durationSamples + delta, grid), original.start + 1, clipDuration);
    return { ...original, durationSamples: end - original.start };
  }
  const keyDelta = Math.round((originY - y) / height * (range.max - range.min + 1));
  return { ...original,
    start: clamp(snapSample(original.start + delta, grid), 0, clipDuration - original.durationSamples),
    key: clamp(original.key + keyDelta, range.min, range.max),
  };
}

export function selectionLimits(notes, clipDuration) {
  return notes.reduce((limits, note) => ({
    left: Math.max(limits.left, (note.minStart ?? 0) - note.start),
    right: Math.min(limits.right, (note.maxEnd ?? clipDuration) - note.start - note.durationSamples),
    shorter: Math.max(limits.shorter, 1 - note.durationSamples),
    lower: Math.max(limits.lower, -note.key),
    higher: Math.min(limits.higher, 127 - note.key),
  }), { left: -Infinity, right: Infinity, shorter: -Infinity, lower: -Infinity, higher: Infinity });
}

// Preview adapter and transient gesture preview; Rust validates and commits the real edit.
export function applyNoteSelection(clip, ids, edit, grid = 1) {
  const selectedIds = new Set(ids);
  const selected = clip.notes.filter(note => selectedIds.has(note.id));
  if (!ids.length || selectedIds.size !== ids.length || selected.length !== ids.length) throw new Error('Invalid note selection');
  if (!['move', 'resize', 'delete', 'quantize'].includes(edit.type)) throw new Error('Invalid MIDI edit');
  if (['move', 'resize'].includes(edit.type) && !Number.isSafeInteger(edit.deltaSamples)) throw new Error('Invalid sample delta');
  if (edit.type === 'move' && !Number.isInteger(edit.semitones)) throw new Error('Invalid pitch delta');
  if (edit.type === 'quantize' && (![4, 8, 16, 32].includes(edit.division) || !(grid >= 1))) throw new Error('Invalid quantize grid');
  const limits = selectionLimits(selected, clip.durationSamples);
  const delta = edit.type === 'move' ? clamp(edit.deltaSamples, limits.left, limits.right)
    : edit.type === 'resize' ? clamp(edit.deltaSamples, limits.shorter, limits.right) : 0;
  const pitch = edit.type === 'move' ? clamp(edit.semitones, limits.lower, limits.higher) : 0;
  return clip.notes.filter(note => edit.type !== 'delete' || !selectedIds.has(note.id)).map(note => {
    if (!selectedIds.has(note.id)) return note;
    if (edit.type === 'move') return { ...note, start: note.start + delta, key: note.key + pitch };
    if (edit.type === 'resize') return { ...note, durationSamples: note.durationSamples + delta };
    if (edit.type === 'quantize') return { ...note, start: clamp(snapSample(note.start, grid), note.minStart ?? 0, (note.maxEnd ?? clip.durationSamples) - note.durationSamples) };
    return note;
  }).sort((a, b) => a.start - b.start || (typeof a.id === "number" ? a.id - b.id : a.id.localeCompare(b.id)));
}

export function dragSelection(gesture, x, y, clip) {
  const { original, width, height, range, grid, mode, originX, originY } = gesture;
  const delta = (x - originX) / Math.max(1, width - KEY_WIDTH) * clip.durationSamples;
  const anchor = mode === 'resize' ? original.start + original.durationSamples : original.start;
  const edit = { type: mode === 'resize' ? 'resize' : 'move', deltaSamples: snapSample(anchor + delta, grid) - anchor };
  if (edit.type === 'move') edit.semitones = Math.round((originY - y) / height * (range.max - range.min + 1));
  return { edit, notes: applyNoteSelection(clip, gesture.ids, edit) };
}

export function notesInRectangle(clip, rectangle, width, height, range) {
  const left = Math.min(rectangle.x1, rectangle.x2); const right = Math.max(rectangle.x1, rectangle.x2);
  const top = Math.min(rectangle.y1, rectangle.y2); const bottom = Math.max(rectangle.y1, rectangle.y2);
  return clip.notes.filter(note => {
    const box = noteBox(note, clip, width, height, range);
    return box.x <= right && box.x + box.width >= left && box.y <= bottom && box.y + box.height >= top;
  }).map(note => note.id);
}
