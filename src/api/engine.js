import { editProjectMidiClips } from "../utils/midi-clip-edit.js";
import { editProjectMidiNotes } from "../utils/midi-clips.js";
import { applyNoteSelection } from "../utils/midi.js";
/** @typedef {{id: string, name: string, isDefault: boolean}} AudioDevice */
/** @typedef {{sampleRate: number, channels: number, bufferSize: number}} AudioConfig */
/** @typedef {{callbackCount: number, xrunCount: number, backendErrorCount: number, averageCallbackUs: number, maximumCallbackUs: number, p99_9CallbackUs: number, deadlineUs: number, cpuLoad: number}} Metrics */
/** @typedef {{minimum: number, maximum: number, rms: number}} WaveformPeak */
/** @typedef {{id: number, timelineStart: number, durationSamples: number, waveform: WaveformPeak[]}} ProjectClip */
/** @typedef {{id: number, start: number, durationSamples: number, key: number, velocity: number, channel: number}} MidiNote */
/** @typedef {{id: number, timelineStart: number, durationSamples: number, notes: MidiNote[]}} MidiClip */
/** @typedef {{id: number, position: number, value: number, curve: "step"|"linear"}} AutomationPoint */
/** @typedef {{id: number, parameter: string, minimum: number, maximum: number, defaultValue: number, points: AutomationPoint[]}} AutomationLane */
/** @typedef {{id: number, position: number, change: {parameter: "gain"|"pan"|"mute"|"solo", value: number|boolean}}} MixerEvent */
/** @typedef {{id: number, role: "rhythm"|"bass"|"atmosphere"|"lead", gain: number, pan: number, muted: boolean, solo: boolean, clips: ProjectClip[], midiClips: MidiClip[], automationLanes: AutomationLane[], mixerEvents?: MixerEvent[]}} ProjectTrack */
/** @typedef {{name: string, sampleRate: number, tempoBpm: number, timeSignature: {numerator: number, denominator: number}, lengthSamples: number, tracks: ProjectTrack[]}} ProjectSnapshot */
/** @typedef {{currentSample: number, sampleRate: number, tempoBpm: number, timeSignature: {numerator: number, denominator: number}, playing: boolean, recording: boolean, loopRange: null|{start: number, end: number}}} TransportSnapshot */

const validTransportAcceptance = (value) => Number.isSafeInteger(value?.engineEpoch) && value.engineEpoch >= 0 && Number.isSafeInteger(value?.commandId) && value.commandId > 0;

const isDesktop = () => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function invoke(command, args) {
  if (!isDesktop()) return previewDocumentInvoke(command, args);
  const tauri = await import("@tauri-apps/api/core");
  return tauri.invoke(command, args);
}

const makeWaveform = (kind, count = 128) => Array.from({ length: count }, (_, index) => {
  const x = index / count;
  const shape = kind === "rhythm"
    ? Math.exp(-((index % 16) / 4))
    : kind === "bass"
      ? 0.45 + Math.sin(x * Math.PI * 16) * 0.18
      : 0.2 + Math.sin(x * Math.PI * 4) * 0.1;
  return { minimum: -shape, maximum: shape, rms: shape * 0.58 };
});

const preview = {
  running: false,
  project: createPreviewProject(48_000),
  transport: {
    currentSample: 0,
    sampleRate: 48_000,
    tempoBpm: 120,
    timeSignature: { numerator: 4, denominator: 4 },
    playing: false,
    recording: false,
    loopRange: null,
  },
};

function createPreviewProject(sampleRate) {
  const bar = sampleRate * 2;
  const length = bar * 4;
  return {
    name: "Night Sketch",
    sampleRate,
    tempoBpm: 120,
    timeSignature: { numerator: 4, denominator: 4 },
    lengthSamples: length,
    tracks: [
      previewTrack(1, "rhythm", 0, length, 0.8, 0),
      previewTrack(2, "bass", 0, length, 0.62, -0.12),
      previewTrack(3, "atmosphere", bar, length - bar, 0.45, 0.18),
      previewMidiTrack(sampleRate, length),
    ],
  };
}

function previewTrack(id, role, timelineStart, durationSamples, gain, pan) {
  return {
    id, role, gain, pan, muted: false, solo: false,
    clips: [{ id: id * 100, timelineStart, durationSamples, waveform: makeWaveform(role) }],
    midiClips: [], automationLanes: [],
  };
}

function previewMidiTrack(sampleRate, length) {
  const beat = sampleRate / 2;
  const keys = [60, 63, 67, 70, 67, 63, 65, 68, 72, 68, 65, 63, 60, 67, 65, 63];
  return {
    id: 4, role: "lead", gain: 0.5, pan: 0.08, muted: false, solo: false, clips: [],
    midiClips: [{ id: 4_100, timelineStart: 0, durationSamples: length,
      notes: keys.map((key, index) => ({ id: 4_000 + index, start: index * beat,
        durationSamples: beat * 0.75, key, velocity: index % 4 === 0 ? 0.86 : 0.64, channel: 0 })) }],
    automationLanes: [{ id: 4_200, parameter: "instrumentLevel", minimum: 0, maximum: 1,
      defaultValue: 0.7, points: [
        { id: 4_201, position: 0, value: 0.45, curve: "linear" },
        { id: 4_202, position: length / 2, value: 0.9, curve: "linear" },
        { id: 4_203, position: length, value: 0.55, curve: "linear" },
      ] }],
  };
}

function editPreviewTrack(id, update) {
  preview.project = {
    ...preview.project,
    tracks: preview.project.tracks.map((track) => track.id === id ? { ...track, ...update } : track),
  };
}

function reorderPreviewTracks(trackIds) {
  const tracks = trackIds.map((id) => preview.project.tracks.find((track) => track.id === id)).filter(Boolean);
  preview.project = { ...preview.project, tracks };
  return preview.project;
}

function editPreviewMidiNote(trackId, clipId, noteId, velocity) {
  preview.project = { ...preview.project, tracks: preview.project.tracks.map((track) => track.id !== trackId ? track : {
    ...track, midiClips: track.midiClips.map((clip) => clip.id !== clipId ? clip : {
      ...clip, notes: clip.notes.map((note) => note.id === noteId ? { ...note, velocity: Math.max(0, Math.min(1, velocity)) } : note),
    }),
  }) };
}

function editPreviewAutomation(trackId, laneId, pointId, value) {
  preview.project = { ...preview.project, tracks: preview.project.tracks.map((track) => track.id !== trackId ? track : {
    ...track, automationLanes: track.automationLanes.map((lane) => lane.id !== laneId ? lane : {
      ...lane, points: lane.points.map((point) => point.id === pointId
        ? { ...point, value: Math.max(lane.minimum, Math.min(lane.maximum, value)) } : point),
    }),
  }) };
}

const previewHistory = { undo: [], redo: [], saved: null, path: null };
const previewEdits = new Set(["set_mixer_event", "remove_mixer_event", "set_track_gain", "set_track_pan", "set_track_mute", "set_track_solo",
  "reorder_tracks", "edit_midi_clips", "edit_midi_notes", "add_midi_clip", "edit_midi_selection", "write_midi_note", "edit_midi_clip", "set_midi_note_velocity", "set_automation_point_value"]);
const previewStatus = () => ({ canUndo: previewHistory.undo.length > 0, canRedo: previewHistory.redo.length > 0,
  dirty: JSON.stringify(preview.project) !== previewHistory.saved, path: previewHistory.path });
async function previewDocumentInvoke(command, args) {
  if (command === "get_document_status") return previewStatus();
  if (command === "undo_project" || command === "redo_project") {
    const source = command === "undo_project" ? previewHistory.undo : previewHistory.redo;
    const target = command === "undo_project" ? previewHistory.redo : previewHistory.undo;
    if (source.length) { target.push(structuredClone(preview.project)); preview.project = source.pop(); }
    return structuredClone(preview.project);
  }
  if (command === "save_project") {
    const text = JSON.stringify(preview.project);
    localStorage.setItem("ongaku.previewProject.v1", text);
    previewHistory.saved = text; previewHistory.path = "Browser preview";
    return previewStatus();
  }
  if (command === "load_project") {
    const text = localStorage.getItem("ongaku.previewProject.v1");
    if (!text) throw new Error("No saved browser preview project");
    const project = JSON.parse(text);
    if (!Array.isArray(project.tracks) || !(project.sampleRate > 0)) throw new Error("Invalid browser preview project");
    preview.project = project; preview.running = false;
    preview.transport = { ...preview.transport, playing: false, currentSample: 0, sampleRate: project.sampleRate };
    previewHistory.undo = []; previewHistory.redo = []; previewHistory.saved = text; previewHistory.path = "Browser preview";
    return structuredClone(project);
  }
  const before = previewEdits.has(command) ? structuredClone(preview.project) : null;
  const result = await previewInvoke(command, args);
  if (before && JSON.stringify(before) !== JSON.stringify(preview.project)) {
    previewHistory.undo.push(before);
    if (previewHistory.undo.length > 100) previewHistory.undo.shift();
    previewHistory.redo = [];
  }
  return result == null ? result : structuredClone(result);
}

function previewInvoke(command, args = {}) {
  switch (command) {
    case "get_transport_command_status": return Promise.resolve(null);
    case "cancel_transport_command": return Promise.resolve(false);
    case "get_graph_publication_status": return Promise.resolve({ documentRevision: 0, requestedRevision: null, engineEpoch: 0, engine: null, preparation: null, streamReadiness: null, transportCommands: null });
    case "list_output_devices": return Promise.resolve([{ id: "preview", name: "Preview Output", isDefault: true }]);
    case "default_output_config": return Promise.resolve({ sampleRate: 48_000, channels: 2, bufferSize: 128 });
    case "configure_project":
      preview.project = createPreviewProject(args.sampleRate);
      preview.transport = { ...preview.transport, sampleRate: args.sampleRate, currentSample: 0 };
      return Promise.resolve(preview.project);
    case "get_project_snapshot": return Promise.resolve(preview.project);
    case "start_engine": preview.running = true; return Promise.resolve();
    case "stop_engine": preview.running = false; preview.transport.playing = false; return Promise.resolve();
    case "play_transport": preview.transport.playing = true; return Promise.resolve(null);
    case "pause_transport": preview.transport.playing = false; return Promise.resolve(null);
    case "seek_transport": preview.transport.currentSample = args.sample; return Promise.resolve(null);
    case "set_transport_loop":
      preview.transport.loopRange = args.enabled ? { start: args.startSample, end: args.endSample } : null;
      return Promise.resolve(null);
    case "set_mixer_event":
    case "remove_mixer_event": {
      const track = preview.project.tracks.find((track) => track.id === args.trackId);
      if (!track) throw new Error("Track not found");
      let events = [...(track.mixerEvents ?? [])];
      if (command === "set_mixer_event") {
        const event = args.event;
        const change = event?.change;
        const validValue = change?.parameter === "gain" ? Number.isFinite(change.value) && change.value >= 0 && change.value <= 2
          : change?.parameter === "pan" ? Number.isFinite(change.value) && change.value >= -1 && change.value <= 1
            : ["mute", "solo"].includes(change?.parameter) && typeof change.value === "boolean";
        if (!Number.isSafeInteger(event?.id) || event.id < 0 || !Number.isSafeInteger(event.position)
          || event.position < 0 || event.position >= preview.project.lengthSamples || !validValue) throw new Error("Invalid mixer event");
        const index = events.findIndex((item) => item.id === event.id);
        if (index >= 0) events[index] = structuredClone(event); else events.push(structuredClone(event));
        if (events.length > 4096) throw new Error("Mixer event limit exceeded");
        events.sort((a, b) => a.position - b.position || a.id - b.id);
      } else {
        if (!events.some((event) => event.id === args.eventId)) throw new Error("Mixer event not found");
        events = events.filter((event) => event.id !== args.eventId);
      }
      preview.project = { ...preview.project, tracks: preview.project.tracks.map((item) => item.id === args.trackId ? { ...item, mixerEvents: events } : item) };
      return structuredClone(preview.project);
    }
    case "set_track_gain": editPreviewTrack(args.trackId, { gain: args.gain }); return Promise.resolve();
    case "set_track_pan": editPreviewTrack(args.trackId, { pan: args.pan }); return Promise.resolve();
    case "set_track_mute": editPreviewTrack(args.trackId, { muted: args.muted }); return Promise.resolve();
    case "set_track_solo": editPreviewTrack(args.trackId, { solo: args.solo }); return Promise.resolve();
    case "reorder_tracks": return Promise.resolve(reorderPreviewTracks(args.trackIds));
    case "edit_midi_clip": {
      const clip = preview.project.tracks.find((track) => track.id === args.trackId)?.midiClips.find((item) => item.id === args.clipId);
      if (!clip) return Promise.reject(new Error("MIDI clip was not found"));
      const grid = Math.max(1, Math.round(preview.project.sampleRate * 60 / preview.project.tempoBpm * 4 / args.amount));
      if (args.action === "delete") clip.notes = clip.notes.filter((note) => note.id !== args.noteId);
      else clip.notes = clip.notes.map((note) => {
        if (args.noteId != null && note.id !== args.noteId) return note;
        if (args.action === "transpose") return { ...note, key: Math.max(0, Math.min(127, note.key + args.amount)) };
        const start = Math.min(clip.durationSamples - 1, Math.floor((note.start + Math.floor(grid / 2)) / grid) * grid);
        return { ...note, start, durationSamples: Math.min(note.durationSamples, clip.durationSamples - start) };
      }).sort((a, b) => a.start - b.start || a.id - b.id);
      return Promise.resolve(structuredClone(preview.project));
    }
    case "edit_midi_clips":
      preview.project = editProjectMidiClips(preview.project, args.targets, args.edit);
      return Promise.resolve(structuredClone(preview.project));
    case "edit_midi_notes":
      preview.project = editProjectMidiNotes(preview.project, args.targets, args.edit);
      return Promise.resolve(structuredClone(preview.project));
    case "add_midi_clip": {
      const track = preview.project.tracks.find(track => track.id === args.trackId);
      if (!track?.midiClips.length || !Number.isSafeInteger(args.start) || !Number.isSafeInteger(args.durationSamples)
        || args.start < 0 || args.durationSamples < 1 || args.start + args.durationSamples > preview.project.lengthSamples) throw new Error("Invalid MIDI clip");
      const id = [...track.clips, ...track.midiClips].reduce((max,clip)=>Math.max(max,clip.id),0)+1;
      track.midiClips.push({ id, timelineStart: args.start, durationSamples: args.durationSamples, notes: [] });
      return Promise.resolve(structuredClone(preview.project));
    }
    case "edit_midi_selection": {
      const clip = preview.project.tracks.find(track => track.id === args.trackId)?.midiClips.find(clip => clip.id === args.clipId);
      if (!clip) throw new Error("MIDI clip was not found");
      const grid = Math.max(1, Math.round(preview.project.sampleRate * 60 / preview.project.tempoBpm * 4 / (args.edit.division ?? 16)));
      clip.notes = applyNoteSelection(clip, args.noteIds, args.edit, grid);
      return Promise.resolve(structuredClone(preview.project));
    }
    case "write_midi_note": {
      const clip = preview.project.tracks.find((track) => track.id === args.trackId)?.midiClips.find((item) => item.id === args.clipId);
      if (!clip || !Number.isSafeInteger(args.start) || !Number.isSafeInteger(args.durationSamples)
        || !Number.isInteger(args.key) || args.key < 0 || args.key > 127 || args.start < 0
        || args.durationSamples < 1 || args.start + args.durationSamples > clip.durationSamples) throw new Error("Invalid MIDI note");
      if (args.noteId != null) {
        const note = clip.notes.find((note) => note.id === args.noteId);
        if (!note) throw new Error("MIDI note was not found");
        Object.assign(note, { start: args.start, durationSamples: args.durationSamples, key: args.key });
      } else {
        const id = clip.notes.reduce((max, note) => Math.max(max, note.id), 0) + 1;
        clip.notes.push({ id, start: args.start, durationSamples: args.durationSamples, key: args.key, velocity: 0.75, channel: 0 });
      }
      clip.notes.sort((a, b) => a.start - b.start || a.id - b.id);
      return Promise.resolve(structuredClone(preview.project));
    }
    case "set_midi_note_velocity":
      editPreviewMidiNote(args.trackId, args.clipId, args.noteId, args.velocity);
      return Promise.resolve(preview.project);
    case "set_automation_point_value":
      editPreviewAutomation(args.trackId, args.laneId, args.pointId, args.value);
      return Promise.resolve(preview.project);
    case "set_gain": return Promise.resolve();
    case "get_transport_snapshot": return Promise.resolve(preview.running ? preview.transport : null);
    case "get_metrics": return Promise.resolve(preview.running ? {
      callbackCount: 15_841, xrunCount: 0, backendErrorCount: 0,
      averageCallbackUs: 1, maximumCallbackUs: 8, p99_9CallbackUs: 4,
      deadlineUs: 2_667, cpuLoad: 0.001,
    } : null);
    default: return Promise.reject(new Error(`Preview command is unavailable: ${command}`));
  }
}

export const engineApi = {
  /** @returns {Promise<AudioDevice[]>} */
  devices: () => invoke("list_output_devices"),
  /** @returns {Promise<AudioConfig>} */
  defaultConfig: () => invoke("default_output_config"),
  /** @param {number} sampleRate @returns {Promise<ProjectSnapshot>} */
  configureProject: (sampleRate) => invoke("configure_project", { sampleRate }),
  /** @returns {Promise<ProjectSnapshot>} */
  project: () => invoke("get_project_snapshot"),
  /** @param {string|null} deviceId @param {AudioConfig} config */
  start: (deviceId, config) => invoke("start_engine", { deviceId, config }),
  stop: () => invoke("stop_engine"),
  /** @param {number} gain */
  setGain: (gain) => invoke("set_gain", { gain }),
  /** @returns {Promise<Metrics|null>} */
  metrics: () => invoke("get_metrics"),
  /** @returns {Promise<TransportSnapshot|null>} */
  transport: () => invoke("get_transport_snapshot"),
  play: () => invoke("play_transport"),
  pause: () => invoke("pause_transport"),
  /** @param {number} sample */
  seek: (sample) => invoke("seek_transport", { sample: Math.max(0, Math.round(sample)) }),
  /** @param {boolean} enabled @param {number} startSample @param {number} endSample */
  setLoop: (enabled, startSample, endSample) => invoke("set_transport_loop", { enabled, startSample, endSample }),
  /** @param {number} trackId @param {MixerEvent} event @returns {Promise<ProjectSnapshot>} */
  setMixerEvent: (trackId, event) => {
    if (![trackId, event?.id, event?.position].every((value) => Number.isSafeInteger(value) && value >= 0)) {
      return Promise.reject(new Error("Mixer event IDs and positions must be safe nonnegative integers"));
    }
    return invoke("set_mixer_event", { trackId, event });
  },
  removeMixerEvent: (trackId, eventId) => {
    if (![trackId, eventId].every((value) => Number.isSafeInteger(value) && value >= 0)) {
      return Promise.reject(new Error("Mixer event IDs must be safe nonnegative integers"));
    }
    return invoke("remove_mixer_event", { trackId, eventId });
  },
  /** @param {number} trackId @param {number} gain */
  setTrackGain: (trackId, gain) => invoke("set_track_gain", { trackId, gain }),
  /** @param {number} trackId @param {number} pan */
  setTrackPan: (trackId, pan) => invoke("set_track_pan", { trackId, pan }),
  /** @param {number} trackId @param {boolean} muted */
  setTrackMute: (trackId, muted) => invoke("set_track_mute", { trackId, muted }),
  /** @param {number} trackId @param {boolean} solo */
  setTrackSolo: (trackId, solo) => invoke("set_track_solo", { trackId, solo }),
  reorderTracks: (trackIds) => invoke("reorder_tracks", { trackIds }),
  /** @returns {Promise<ProjectSnapshot>} */
  setMidiNoteVelocity: (trackId, clipId, noteId, velocity) => invoke("set_midi_note_velocity", { trackId, clipId, noteId, velocity }),
  /** @returns {Promise<ProjectSnapshot>} */
  setAutomationPointValue: (trackId, laneId, pointId, value) => invoke("set_automation_point_value", { trackId, laneId, pointId, value }),
  editMidiClip: (trackId, clipId, action, amount, noteId = null) => invoke("edit_midi_clip", { trackId, clipId, action, amount, noteId }),
  editMidiClips: (targets, edit) => invoke("edit_midi_clips", { targets, edit }),
  editMidiNotes: (targets, edit) => invoke("edit_midi_notes", { targets, edit }),
  addMidiClip: (trackId, start, durationSamples) => invoke("add_midi_clip", { trackId, start, durationSamples }),
  editMidiSelection: (trackId, clipId, noteIds, edit) => invoke("edit_midi_selection", { trackId, clipId, noteIds, edit }),
  writeMidiNote: (trackId, clipId, noteId, start, durationSamples, key) => invoke("write_midi_note", { trackId, clipId, noteId, start, durationSamples, key }),
  transportCommandStatus: (acceptance) => validTransportAcceptance(acceptance)
    ? invoke("get_transport_command_status", { engineEpoch: acceptance.engineEpoch, commandId: acceptance.commandId })
    : Promise.reject(new Error("Transport acceptance requires a safe epoch and positive command ID")),
  cancelTransportCommand: (acceptance) => validTransportAcceptance(acceptance)
    ? invoke("cancel_transport_command", { engineEpoch: acceptance.engineEpoch, commandId: acceptance.commandId })
    : Promise.reject(new Error("Transport acceptance requires a safe epoch and positive command ID")),
  publicationStatus: (afterRevision = 0) => Number.isSafeInteger(afterRevision) && afterRevision >= 0
    ? invoke("get_graph_publication_status", { afterRevision })
    : Promise.reject(new Error("Publication revision must be a safe nonnegative integer")),
  documentStatus: () => invoke("get_document_status"),
  undo: () => invoke("undo_project"),
  redo: () => invoke("redo_project"),
  saveProject: (saveAs = false) => invoke("save_project", { saveAs }),
  loadProject: () => invoke("load_project"),
  isDesktop,
};
