import { useCallback, useEffect, useRef, useState } from "react";
import { engineApi } from "../api/engine.js";

const DEFAULT_CONFIG = { sampleRate: 48_000, channels: 2, bufferSize: 128 };

export function useDawSession(unavailableMessage) {
  const [devices, setDevices] = useState([]);
  const [deviceId, setDeviceId] = useState("");
  const [config, setConfig] = useState(DEFAULT_CONFIG);
  const [projectEpoch, setProjectEpoch] = useState(0);
  const [project, setProject] = useState(null);
  const [transport, setTransport] = useState(null);
  const [metrics, setMetrics] = useState(null);
  const [publicationStatus, setPublicationStatus] = useState(null);
  const [timingWarning, setTimingWarning] = useState("");
  const [running, setRunning] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [documentStatus, setDocumentStatus] = useState({ canUndo: false, canRedo: false, dirty: true, path: null });
  const commandQueue = useRef(Promise.resolve());
  const documentBusy = useRef(false);

  const reportError = useCallback((reason) => {
    setError(reason instanceof Error ? reason.message : String(reason));
  }, []);

  const refreshProject = useCallback(async () => {
    const snapshot = await engineApi.project();
    setProject(snapshot);
    setDocumentStatus(await engineApi.documentStatus());
    return snapshot;
  }, []);

  useEffect(() => {
    let active = true;
    Promise.all([engineApi.devices(), engineApi.defaultConfig()])
      .then(async ([nextDevices, nextConfig]) => {
        const snapshot = await engineApi.project();
        if (!active) return;
        setDevices(nextDevices);
        setConfig({ ...nextConfig, sampleRate: snapshot.sampleRate });
        setProject(snapshot);
        setDocumentStatus(await engineApi.documentStatus());
      })
      .catch(() => active && setError(unavailableMessage));
    return () => { active = false; };
  }, []);

  useEffect(() => {
    if (!running) return undefined;
    let active = true;
    let inFlight = false;
    let cursor = 0;
    let epoch = null;
    let lateCount = 0;
    let preparationFailure = null;
    const poll = window.setInterval(() => {
      if (inFlight) return;
      inFlight = true;
      Promise.all([engineApi.transport(), engineApi.metrics(), engineApi.publicationStatus(cursor)])
        .then(([nextTransport, nextMetrics, nextPublication]) => {
          if (!active) return;
          setTransport(nextTransport);
          setMetrics(nextMetrics);
          setPublicationStatus(nextPublication);
          if (epoch !== nextPublication.engineEpoch) {
            epoch = nextPublication.engineEpoch;
            lateCount = 0;
            preparationFailure = null;
          }
          const failure = nextPublication.preparation?.failure;
          if (failure && failure.preparationRevision !== preparationFailure) {
            preparationFailure = failure.preparationRevision;
            reportError(failure.message);
          }
          const engine = nextPublication.engine;
          if (engine) {
            if (engine.lateCount > lateCount) setTimingWarning("late");
            else if (engine.historyLost) setTimingWarning((previous) => previous || "history");
            lateCount = engine.lateCount;
            cursor = engine.settledRevision ?? engine.adoptedRevision;
          } else cursor = 0;
        })
        .catch((reason) => { if (active) reportError(reason); })
        .finally(() => { inFlight = false; });
    }, 100);
    return () => { active = false; window.clearInterval(poll); };
  }, [reportError, running]);

  const startEngine = useCallback(async () => {
    if (running || documentBusy.current) return;
    setBusy(true);
    setError("");
    try {
      await engineApi.start(deviceId || null, config);
      if (transport?.currentSample) await engineApi.seek(transport.currentSample);
      setRunning(true);
      setProject(await engineApi.project());
      setTransport(await engineApi.transport());
    } catch (reason) {
      reportError(reason);
      throw reason;
    } finally {
      setBusy(false);
    }
  }, [config, deviceId, reportError, running, transport?.currentSample]);

  const stopEngine = useCallback(async () => {
    setBusy(true);
    setError("");
    try {
      await engineApi.stop();
      setRunning(false);
      setTransport(null);
      setMetrics(null);
      setPublicationStatus(null);
      setTimingWarning("");
    } catch (reason) {
      reportError(reason);
    } finally {
      setBusy(false);
    }
  }, [reportError]);

  const toggleEngine = useCallback(() => running ? stopEngine() : startEngine(), [running, startEngine, stopEngine]);

  const togglePlayback = useCallback(async () => {
    if (documentBusy.current) return;
    setError("");
    try {
      if (!running) await startEngine();
      const isPlaying = transport?.playing ?? false;
      await (isPlaying ? engineApi.pause() : engineApi.play());
      setTransport((current) => ({
        ...(current ?? {
          currentSample: 0,
          sampleRate: project?.sampleRate ?? config.sampleRate,
          tempoBpm: project?.tempoBpm ?? 120,
          timeSignature: project?.timeSignature ?? { numerator: 4, denominator: 4 },
          recording: false,
          loopRange: null,
        }),
        playing: !isPlaying,
      }));
    } catch (reason) {
      reportError(reason);
    }
  }, [config.sampleRate, project, reportError, running, startEngine, transport?.playing]);

  const seek = useCallback(async (sample) => {
    const bounded = Math.max(0, Math.min(project?.lengthSamples ?? sample, Math.round(sample)));
    setTransport((current) => ({ ...(current ?? { sampleRate: project?.sampleRate ?? config.sampleRate,
      tempoBpm: project?.tempoBpm ?? 120, timeSignature: project?.timeSignature ?? {numerator:4,denominator:4},
      playing:false, recording:false, loopRange:null }), currentSample: bounded }));
    if (!running) return;
    try { await engineApi.seek(bounded); } catch (reason) { reportError(reason); }
  }, [project, config.sampleRate, reportError, running]);

  const stopPlayback = useCallback(async () => {
    setTransport((current) => current ? { ...current, playing: false, currentSample: 0 } : current);
    if (!running) return;
    try {
      await engineApi.pause();
      await engineApi.seek(0);
    } catch (reason) { reportError(reason); }
  }, [reportError, running]);

  const toggleLoop = useCallback(async () => {
    if (!project || !running) return;
    const enabled = !transport?.loopRange;
    const barSamples = Math.round(project.sampleRate * 60 / project.tempoBpm * project.timeSignature.numerator);
    const start = barSamples;
    const end = Math.min(project.lengthSamples, barSamples * 3);
    try {
      await engineApi.setLoop(enabled, start, end);
      setTransport((current) => current ? { ...current, loopRange: enabled ? { start, end } : null } : current);
    } catch (reason) { reportError(reason); }
  }, [project, reportError, running, transport?.loopRange]);

  // Serialize edits and document operations so Save/Undo include the last slider update.
  const mutate = useCallback((operation) => {
    if (documentBusy.current) return Promise.resolve(false);
    const task = commandQueue.current.then(async () => {
      const result = await operation();
      await refreshProject();
      return result ?? true;
    });
    commandQueue.current = task.catch(reportError);
    return task.catch(() => false);
  }, [refreshProject, reportError]);

  const documentAction = useCallback((action, saveAs = false) => {
    if (documentBusy.current) return Promise.resolve(false);
    documentBusy.current = true;
    setBusy(true);
    const task = commandQueue.current.then(async () => {
      if (action === "save") {
        await engineApi.saveProject(saveAs);
      } else if (action === "open") {
        const loaded = await engineApi.loadProject();
        if (loaded) {
          setProjectEpoch(value => value + 1);
          setRunning(false); setTransport(null); setMetrics(null);
          setPublicationStatus(null); setTimingWarning(""); setError("");
          setConfig((current) => ({ ...current, sampleRate: loaded.sampleRate }));
        }
      } else if (action === "undo") await engineApi.undo();
      else await engineApi.redo();
      await refreshProject();
      return true;
    }).catch((reason) => { reportError(reason); return false; })
      .finally(() => { documentBusy.current = false; setBusy(false); });
    commandQueue.current = task;
    return task;
  }, [refreshProject, reportError]);

  return {
    devices, deviceId, setDeviceId, config, project, projectEpoch, transport, metrics, documentStatus,
    running, busy, error, setError, publicationStatus, timingWarning, dismissTimingWarning: () => setTimingWarning(""), toggleEngine, togglePlayback, stopPlayback, seek, toggleLoop,
    undo: () => documentAction("undo"), redo: () => documentAction("redo"),
    saveProject: (saveAs = false) => documentAction("save", saveAs),
    loadProject: () => documentAction("open"),
    setTrackGain: (id, value) => mutate(() => engineApi.setTrackGain(id, value)),
    setTrackPan: (id, value) => mutate(() => engineApi.setTrackPan(id, value)),
    setTrackMute: (id, value) => mutate(() => engineApi.setTrackMute(id, value)),
    setTrackSolo: (id, value) => mutate(() => engineApi.setTrackSolo(id, value)),
    reorderTracks: (ids) => mutate(() => engineApi.reorderTracks(ids)),
    editMidiClip: (...args) => mutate(() => engineApi.editMidiClip(...args)),
    editMidiClips: (...args) => mutate(() => engineApi.editMidiClips(...args)),
    editMidiNotes: (...args) => mutate(() => engineApi.editMidiNotes(...args)),
    addMidiClip: (...args) => mutate(() => engineApi.addMidiClip(...args)),
    editMidiSelection: (...args) => mutate(() => engineApi.editMidiSelection(...args)),
    writeMidiNote: (...args) => mutate(() => engineApi.writeMidiNote(...args)),
    setMidiNoteVelocity: (...args) => mutate(() => engineApi.setMidiNoteVelocity(...args)),
    setAutomationPointValue: (...args) => mutate(() => engineApi.setAutomationPointValue(...args)),
    previewMode: !engineApi.isDesktop(),
  };
}
