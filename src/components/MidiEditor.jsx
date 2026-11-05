import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { buildMidiView, midiClipKey, midiNoteKey } from "../utils/midi-clips.js";
import { MidiClipTools } from "./MidiClipTools.jsx";
import { MiniSlider } from "./MiniSlider.jsx";

import { KEY_WIDTH, VELOCITY_HEIGHT, clamp, snapSample, pitchRange, noteBox, noteAtPoint, dragSelection, notesInRectangle, selectionLimits } from "../utils/midi.js";

export const MidiEditor = memo(function MidiEditor({ t, track, projectLength, onVelocity, onAutomation, onWriteNote, sources, allSources, onSelectClip, onSelectAllClips, onAddClip, onCrossEdit, onEditClips, playhead, sampleRate, tempoBpm, busy }) {
  const clip = useMemo(() => buildMidiView(sources, projectLength), [sources, projectLength]);
  const activeSource = sources.at(-1);
  const lane = track?.automationLanes?.[0] ?? null;
  const gesture = useRef(null);
  const pending = useRef(false);
  const [draft, setDraft] = useState(null);
  const [marquee, setMarquee] = useState(null);
  const [selectedIds, setSelectedIds] = useState([]);
  const [grid, setGrid] = useState(16);
  const [editing, setEditing] = useState(false);
  const [noteId, setNoteId] = useState(null);
  const [pointId, setPointId] = useState(null);
  const [view, setView] = useState("piano");
  const pianoView = view === "piano" || !lane;

  useEffect(() => {
    setNoteId(clip?.notes[0]?.id ?? null);
    setSelectedIds(clip?.notes[0] ? [clip.notes[0].id] : []);
    setPointId(lane?.points[0]?.id ?? null);
  }, [clip?.id]);

  useEffect(() => {
    gesture.current = null;
    setDraft(null); setMarquee(null);
  }, [clip, busy]);

  const selectedSet = useMemo(() => new Set(selectedIds), [selectedIds]);
  const selectedNotes = useMemo(() => clip?.notes.filter(item => selectedSet.has(item.id)) ?? [], [clip, selectedSet]);
  const note = selectedNotes.find(item => item.id === noteId) ?? selectedNotes[0] ?? null;
  const limits = selectionLimits(selectedNotes, clip?.durationSamples ?? 1);
  const interacting = !!draft || !!marquee;
  const point = useMemo(() => lane?.points.find((item) => item.id === pointId) ?? lane?.points[0] ?? null, [lane, pointId]);
  const range = useMemo(() => pitchRange(clip?.notes ?? []), [clip]);
  const beatSamples = sampleRate * 60 / tempoBpm;
  const gridSamples = Math.max(1, Math.round(beatSamples * 4 / grid));
  const drawNotes = useCallback((canvas) => drawPianoRoll(canvas, clip, selectedSet, draft, range, gridSamples, marquee, sources), [clip, selectedSet, draft, range, gridSamples, marquee, sources]);
  const drawLane = useCallback((canvas) => drawAutomation(canvas, lane, projectLength, point?.id), [lane, point?.id, projectLength]);

  const editSelection = async (edit, ids = selectedNotes.map(item => item.id)) => {
    if (pending.current || busy || !ids.length) return;
    pending.current = true; setEditing(true);
    try { await onCrossEdit(clip.notes.filter(item => ids.includes(item.id)).map(item => item.target), edit); }
    finally { pending.current = false; setEditing(false); setDraft(null); }
  };

  const write = async (source, geometry) => {
    if (pending.current || busy || !source) return;
    pending.current = true; setEditing(true);
    try {
      const snapshot = await onWriteNote(source.trackId, source.clip.id, null,
        geometry.start - source.clip.timelineStart, geometry.durationSamples, geometry.key);
      const previousIds = new Set(source.clip.notes.map(item => item.id));
      const added = snapshot?.tracks?.find(item => item.id === source.trackId)?.midiClips
        .find(item => item.id === source.clip.id)?.notes.find(item => !previousIds.has(item.id));
      if (added) {
        const id = midiNoteKey({trackId:source.trackId,clipId:source.clip.id,noteId:added.id});
        setNoteId(id); setSelectedIds([id]);
      }
    } finally { pending.current = false; setEditing(false); setDraft(null); }
  };
  const add = (position, key = note?.key ?? 60) => {
    const source = position == null ? activeSource : [...sources].reverse().find(source => position >= source.clip.timelineStart
      && position < source.clip.timelineStart + source.clip.durationSamples);
    if (!source) return;
    const beginning = source.clip.timelineStart;
    const end = beginning + source.clip.durationSamples;
    const suggested = position ?? (note?.clipKey === midiClipKey(source.trackId, source.clip.id) ? note.start + note.durationSamples : beginning);
    const start = clamp(snapSample(suggested, gridSamples), beginning, end - 1);
    return write(source, { start, key, durationSamples: Math.min(gridSamples, end - start) });
  };
  const cancelGesture = () => {
    if (gesture.current?.mode === "select") setSelectedIds(gesture.current.previousSelection);
    gesture.current = null; setDraft(null); setMarquee(null);
  };
  const pointerDown = (event) => {
    if (event.button !== 0 || busy || pending.current) return;
    const canvas = event.currentTarget;
    const rect = canvas.getBoundingClientRect();
    const x = event.clientX - rect.left; const y = event.clientY - rect.top;
    const height = rect.height - VELOCITY_HEIGHT;
    if (x < KEY_WIDTH || y < 0 || y >= height) return;
    event.preventDefault(); canvas.focus();
    const hit = noteAtPoint(x, y, clip, rect.width, height, range);
    const additive = event.shiftKey || event.metaKey || event.ctrlKey;
    let ids = selectedNotes.map(item => item.id);
    if (!hit) {
      const baseIds = additive ? ids : [];
      setSelectedIds(baseIds);
      gesture.current = { mode: "select", baseIds, previousSelection: selectedIds,
        originX: x, originY: y, width: rect.width, height, range, pointerId: event.pointerId };
      canvas.setPointerCapture(event.pointerId);
      return;
    }
    if (additive) {
      ids = ids.includes(hit.id) ? ids.filter(id => id !== hit.id) : [...ids, hit.id];
      setSelectedIds(ids); setNoteId(hit.id);
      return;
    }
    if (!ids.includes(hit.id)) ids = [hit.id];
    setSelectedIds(ids); setNoteId(hit.id);
    const box = noteBox(hit, clip, rect.width, height, range);
    gesture.current = { original: hit, ids, originX: x, originY: y, width: rect.width, height, range,
      grid: gridSamples, pointerId: event.pointerId, mode: x >= box.x + box.width - Math.min(6, box.width / 3) ? "resize" : "move" };
    canvas.setPointerCapture(event.pointerId);
  };
  const pointerMove = (event) => {
    const active = gesture.current;
    if (!active || active.pointerId !== event.pointerId) return;
    const rect = event.currentTarget.getBoundingClientRect();
    const x = event.clientX - rect.left; const y = event.clientY - rect.top;
    if (Math.hypot(x - active.originX, y - active.originY) < 3 && !active.moved) return;
    active.moved = true;
    if (active.mode === "select") {
      const rectangle = { x1: active.originX, y1: active.originY,
        x2: clamp(x, KEY_WIDTH, active.width), y2: clamp(y, 0, active.height) };
      setMarquee(rectangle);
      setSelectedIds([...new Set([...active.baseIds, ...notesInRectangle(clip, rectangle, active.width, active.height, active.range)])]);
    } else {
      active.candidate = dragSelection(active, x, y, clip);
      setDraft(active.candidate.notes);
    }
  };
  const pointerUp = (event) => {
    const active = gesture.current;
    if (!active || active.pointerId !== event.pointerId) return;
    pointerMove(event);
    gesture.current = null; setMarquee(null);
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    if (active.candidate) editSelection(active.candidate.edit, active.ids);
    else setDraft(null);
  };
  const selectAll = () => { if (!busy && !pending.current) setSelectedIds(clip.notes.map(item => item.id)); };
  const keyDown = (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      if (gesture.current) cancelGesture(); else setSelectedIds([]);
      return;
    }
    if (gesture.current) return;
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "a") {
      event.preventDefault(); event.stopPropagation(); selectAll(); return;
    }
    if (event.repeat || gesture.current) return;
    if (event.key === "Delete" || event.key === "Backspace") {
      event.preventDefault(); editSelection({ type: "delete" });
    } else if (event.key.toLowerCase() === "q" && !event.ctrlKey && !event.metaKey) {
      event.preventDefault(); editSelection({ type: "quantize", division: grid });
    }
  };
  const doubleClick = (event) => {
    if (event.shiftKey || event.metaKey || event.ctrlKey) return;
    const rect = event.currentTarget.getBoundingClientRect();
    const x = event.clientX - rect.left; const y = event.clientY - rect.top;
    const height = rect.height - VELOCITY_HEIGHT;
    if (x < KEY_WIDTH || y < 0 || y >= height || noteAtPoint(x, y, clip, rect.width, height, range)) return;
    add((x - KEY_WIDTH) / Math.max(1, rect.width - KEY_WIDTH) * clip.durationSamples,
      clamp(range.max - Math.floor(y / height * (range.max - range.min + 1)), range.min, range.max));
  };

  return (
    <section className="midi-editor" aria-label={t.midiEditor}>
      <div className="midi-clip-picker" aria-label={t.midiClips}>
        <button type="button" title={t.selectAllClips} aria-label={t.selectAllClips} onClick={onSelectAllClips}>▣</button>
        <button type="button" title={t.addMidiClip} aria-label={t.addMidiClip} disabled={busy || editing || interacting} onClick={onAddClip}>＋▭</button>
        <div className="midi-clip-options">
          {allSources.map((source) => {
            const selected = sources.some(item => midiClipKey(item.trackId, item.clip.id) === midiClipKey(source.trackId, source.clip.id));
            const active = source.trackId === activeSource?.trackId && source.clip.id === activeSource?.clip.id;
            return <button type="button" key={midiClipKey(source.trackId, source.clip.id)} aria-pressed={selected}
              className={active ? "is-active" : ""} title={`${t.trackRoles[source.role]} · ${source.clip.id}`}
              onClick={event => onSelectClip(source.trackId, source.clip.id, event.shiftKey || event.metaKey || event.ctrlKey)}>
              {source.number} · {t.trackRoles[source.role]}
            </button>;
          })}
        </div>
        <span className="midi-gesture-help" title={t.clipEditHelp} aria-label={t.clipEditHelp}>ⓘ</span>
      </div>
      <MidiClipTools t={t} sources={sources} sampleRate={sampleRate} tempoBpm={tempoBpm} projectLength={projectLength}
        playhead={playhead} busy={busy||editing||interacting} onEditClips={onEditClips} />
      {!clip ? null : (
        <div className="midi-editor-grid">
          <div className="editor-canvases">
            <div className="editor-view-header">
            <div className="editor-view-tabs" role="tablist" aria-label={t.midiEditor}>
              <button type="button" role="tab" aria-selected={pianoView} onClick={() => setView("piano")}>{t.pianoRoll}</button>
              <button type="button" role="tab" aria-selected={!pianoView} disabled={!lane} onClick={() => setView("automation")}>{t.automation}</button>
            </div>
            {pianoView && <div className="midi-header-actions">
              <span className="midi-note-count" aria-live="polite">{selectedNotes.length}/{clip.notes.length} {t.notes}</span>
              <button type="button" disabled={editing || busy || interacting || !clip.notes.length} title={t.selectAllNotes} aria-label={t.selectAllNotes} onClick={selectAll}>▣</button>
              <button type="button" disabled={editing || busy || interacting || !selectedNotes.length} title={t.quantizeSelection} aria-label={t.quantizeSelection} onClick={() => editSelection({ type: "quantize", division: grid })}>Q</button>
              <button type="button" disabled={editing || busy || interacting || !selectedNotes.length} title={t.deleteSelection} aria-label={t.deleteSelection} onClick={() => editSelection({ type: "delete" })}>⌫</button>
              <button type="button" disabled={editing || busy || interacting} title={t.addNote} aria-label={t.addNote} onClick={() => add()}>＋</button>
            </div>}
            </div>
            <div className="editor-canvas-stage" role="tabpanel" aria-label={pianoView ? t.pianoRoll : t.automation}>
              {pianoView ? <CanvasPanel title={t.pianoRoll} description={t.pianoRollDescription} className="piano-roll-canvas"
                draw={drawNotes} onPointerDown={pointerDown} onPointerMove={pointerMove} onPointerUp={pointerUp}
                onPointerCancel={cancelGesture} onLostPointerCapture={() => { if (gesture.current) cancelGesture(); }}
                onDoubleClick={doubleClick} onKeyDown={keyDown} />
                : <CanvasPanel title={t.automation} description={t.automationDescription} className="automation-canvas"
                  draw={drawLane} onPoint={(event, canvas) => setPointId(hitTestPoint(event, canvas, lane, projectLength) ?? point?.id)} />}
            </div>
          </div>
          <aside className="midi-inspector" aria-label={t.inspector}>
            {pianoView ? <>
              <fieldset className="midi-edit-tools" disabled={editing || busy || interacting}>
                <label>{t.quantize}<select aria-label={t.quantizeGrid} value={grid} onChange={(event) => setGrid(Number(event.target.value))}>
                  {[4, 8, 16, 32].map((value) => <option key={value} value={value}>1/{value}</option>)}
                </select></label>
                <button type="button" disabled={!note} title={t.transposeDown} aria-label={t.transposeDown} onClick={() => editSelection({ type: "move", deltaSamples: 0, semitones: -1 })}>−♩</button>
                <button type="button" disabled={!note} title={t.transposeUp} aria-label={t.transposeUp} onClick={() => editSelection({ type: "move", deltaSamples: 0, semitones: 1 })}>+♩</button>
              </fieldset>
              <span className="midi-gesture-help" title={t.noteEditHelp} aria-label={t.noteEditHelp}>ⓘ</span>
              <p className="inspector-label">{t.selectedNote}</p>
              <strong>{selectedNotes.length > 1 ? `${selectedNotes.length} ${t.selectedNotes}` : note ? `${noteName(note.key)} · ${t.channel} ${note.channel + 1}` : "—"}</strong>
              <fieldset className="midi-note-fields" disabled={!note || editing || busy || interacting}>
                <NoteField label={t.notePitch} value={note?.key ?? 60} min={note ? note.key + limits.lower : 0} max={note ? note.key + limits.higher : 127} step={1}
                  onCommit={(value) => editSelection({ type: "move", deltaSamples: 0, semitones: Math.round(value) - note.key })} />
                <NoteField label={t.noteStart} value={(note?.start ?? 0) / beatSamples} min={note ? (note.start + limits.left) / beatSamples : 0}
                  max={note ? (note.start + limits.right) / beatSamples : 0} step={4 / grid}
                  onCommit={(value) => editSelection({ type: "move", deltaSamples: Math.round(value * beatSamples) - note.start, semitones: 0 })} />
                <NoteField label={t.noteLength} value={(note?.durationSamples ?? gridSamples) / beatSamples} min={note ? (note.durationSamples + limits.shorter) / beatSamples : 1 / beatSamples}
                  max={note ? (note.durationSamples + limits.right) / beatSamples : 1} step={4 / grid}
                  onCommit={(value) => editSelection({ type: "resize", deltaSamples: Math.round(value * beatSamples) - note.durationSamples })} />
              </fieldset>
              <MiniSlider label={t.velocity} min={0} max={1} step={0.01} value={note?.velocity ?? 0}
                disabled={selectedNotes.length !== 1 || editing || busy || interacting} displayValue={note ? Math.round(note.velocity * 127) : 0}
                onChange={(value) => onVelocity(note.target.trackId, note.target.clipId, note.target.noteId, value)} />
            </> : <>
              <p className="inspector-label">{t.selectedAutomation}</p>
              <strong>{t.instrumentLevel}</strong>
              <MiniSlider label={t.value} min={lane.minimum} max={lane.maximum} step={0.01} value={point?.value ?? 0}
                disabled={!point} displayValue={point ? `${Math.round(point.value * 100)}%` : "—"}
                onChange={(value) => onAutomation(track.id, lane.id, point.id, value)} />
            </>}
          </aside>
        </div>
      )}
    </section>
  );
});

function CanvasPanel({ title, description, className, draw, onPoint, ...events }) {
  const ref = useRef(null);
  useEffect(() => {
    const canvas = ref.current;
    if (!canvas) return undefined;
    const render = () => draw(canvas);
    const observer = new ResizeObserver(render);
    observer.observe(canvas.parentElement);
    render();
    return () => observer.disconnect();
  }, [draw]);
  return (
    <div className="editor-canvas-panel">
      <canvas ref={ref} className={className} tabIndex="0" role="application" aria-label={description}
        onPointerDown={onPoint ? (event) => onPoint(event, ref.current) : undefined} {...events} />
    </div>
  );
}

function prepareCanvas(canvas, height) {
  const width = Math.max(1, Math.floor(canvas.parentElement.getBoundingClientRect().width));
  height = Math.max(1, Math.floor(canvas.parentElement.getBoundingClientRect().height) || height);
  const scale = Math.max(1, window.devicePixelRatio || 1);
  canvas.width = Math.floor(width * scale);
  canvas.height = Math.floor(height * scale);
  canvas.style.width = `${width}px`;
  canvas.style.height = `${height}px`;
  const context = canvas.getContext("2d");
  context.setTransform(scale, 0, 0, scale, 0, 0);
  return { context, width, height, styles: getComputedStyle(document.documentElement) };
}

function drawPianoRoll(canvas, clip, selectedSet, draft, range, grid, marquee, sources) {
  const { context, width, height, styles } = prepareCanvas(canvas, 208);
  const color = (name) => styles.getPropertyValue(name).trim();
  context.fillStyle = color("--color-paper-3");
  context.fillRect(0, 0, width, height);
  const rows = range.max - range.min + 1;
  const rollHeight = height - VELOCITY_HEIGHT;
  for (let key = range.min; key <= range.max; key += 1) {
    const y = (range.max - key) / rows * rollHeight;
    const black = [1, 3, 6, 8, 10].includes(key % 12);
    context.fillStyle = black ? color("--color-piano-grid") : color("--color-piano-grid-alt");
    context.fillRect(KEY_WIDTH, y, width - KEY_WIDTH, rollHeight / rows + 1);
    context.fillStyle = color("--color-key-white");
    context.fillRect(0, y, KEY_WIDTH - 1, rollHeight / rows);
    if (black) { context.fillStyle = color("--color-key-black"); context.fillRect(0, y, KEY_WIDTH * 0.65, rollHeight / rows); }
  }
  context.strokeStyle = color("--color-piano-rule");
  for (let sample = 0; sample <= clip.durationSamples; sample += Math.max(grid, Math.ceil(clip.durationSamples / 512))) {
    const x = KEY_WIDTH + sample / clip.durationSamples * (width - KEY_WIDTH);
    context.beginPath(); context.moveTo(x + 0.5, 0); context.lineTo(x + 0.5, height); context.stroke();
  }
  sources.forEach((source, index) => {
    const x = KEY_WIDTH + source.clip.timelineStart / clip.durationSamples * (width - KEY_WIDTH);
    const w = source.clip.durationSamples / clip.durationSamples * (width - KEY_WIDTH);
    context.strokeStyle = color("--color-rule-strong"); context.lineWidth = 1;
    context.strokeRect(x + 0.5, 0.5, w - 1, rollHeight - 1);
    context.fillStyle = color("--color-ink"); context.font = `9px ${color("--font-body")}`;
    context.fillText(String(source.number), x + 3, 11 + index % 3 * 10);
  });
  const sourceOrder = new Map(sources.map((source, index) => [midiClipKey(source.trackId, source.clip.id), index]));
  const orderedNotes = draft ? [...draft].sort((a, b) => sourceOrder.get(a.clipKey) - sourceOrder.get(b.clipKey)) : clip.notes;
  for (const note of orderedNotes) {
    const box = noteBox(note, clip, width, rollHeight, range);
    context.fillStyle = selectedSet.has(note.id) ? color("--color-focus") : color(`--color-track-${note.role ?? "atmosphere"}`);
    context.globalAlpha = 0.45 + note.velocity * 0.55;
    context.fillRect(box.x, box.y, box.width, box.height);
    context.fillRect(box.x, height - note.velocity * (VELOCITY_HEIGHT - 5), 2, note.velocity * (VELOCITY_HEIGHT - 5));
  }
  context.globalAlpha = 1;
  if (marquee) {
    context.fillStyle = color("--color-focus"); context.globalAlpha = 0.15;
    const x = Math.min(marquee.x1, marquee.x2); const y = Math.min(marquee.y1, marquee.y2);
    const w = Math.abs(marquee.x2 - marquee.x1); const h = Math.abs(marquee.y2 - marquee.y1);
    context.fillRect(x, y, w, h); context.globalAlpha = 1;
    context.strokeStyle = color("--color-focus"); context.lineWidth = 1; context.strokeRect(x, y, w, h);
  }
}

function drawAutomation(canvas, lane, projectLength, selectedId) {
  const { context, width, height, styles } = prepareCanvas(canvas, 112);
  const color = (name) => styles.getPropertyValue(name).trim();
  context.fillStyle = color("--color-paper-3"); context.fillRect(0, 0, width, height);
  context.strokeStyle = color("--color-rule");
  for (let beat = 0; beat <= 16; beat += 1) {
    const x = beat / 16 * width;
    context.beginPath(); context.moveTo(x + 0.5, 0); context.lineTo(x + 0.5, height); context.stroke();
  }
  context.strokeStyle = color("--color-track-lead"); context.lineWidth = 2; context.beginPath();
  lane.points.forEach((point, index) => {
    const { x, y } = automationPoint(point, lane, projectLength, width, height);
    if (index === 0) context.moveTo(x, y); else context.lineTo(x, y);
  });
  context.stroke();
  lane.points.forEach((point) => {
    const { x, y } = automationPoint(point, lane, projectLength, width, height);
    context.fillStyle = point.id === selectedId ? color("--color-focus") : color("--color-track-lead");
    context.beginPath(); context.arc(x, y, point.id === selectedId ? 6 : 4, 0, Math.PI * 2); context.fill();
  });
}

function automationPoint(point, lane, projectLength, width, height) {
  const phase = (point.value - lane.minimum) / (lane.maximum - lane.minimum);
  return { x: point.position / projectLength * width, y: height - phase * (height - 16) - 8 };
}

function hitTestPoint(event, canvas, lane, projectLength) {
  const rect = canvas.getBoundingClientRect();
  const x = event.clientX - rect.left; const y = event.clientY - rect.top;
  let closest = null; let distance = 24;
  for (const point of lane.points) {
    const position = automationPoint(point, lane, projectLength, rect.width, rect.height);
    const next = Math.hypot(position.x - x, position.y - y);
    if (next < distance) { closest = point.id; distance = next; }
  }
  return closest;
}

function noteName(key) {
  const names = ["C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B"];
  return `${names[key % 12]}${Math.floor(key / 12) - 1}`;
}

function NoteField({ label, value, min, max, step, onCommit }) {
  const display = Number(value.toFixed(4));
  return <label><span>{label}</span><input key={display} type="number" aria-label={label}
    min={min} max={max} step={step} defaultValue={display}
    onKeyDown={(event) => {
      if (event.key === "Escape") { event.currentTarget.value = String(display); event.currentTarget.blur(); }
      if (event.key === "Enter") event.currentTarget.blur();
    }}
    onBlur={(event) => {
      const next = event.currentTarget.valueAsNumber;
      if (Number.isFinite(next) && next !== display) onCommit(clamp(next, min, max));
      else event.currentTarget.value = String(display);
    }} /></label>;
}
