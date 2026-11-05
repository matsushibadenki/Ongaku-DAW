import { useEffect, useMemo, useState } from "react";
import { AppHeader } from "./components/AppHeader.jsx";
import { AppMenu } from "./components/AppMenu.jsx";
import { EditorMap } from "./components/EditorMap.jsx";
import { MidiEditor } from "./components/MidiEditor.jsx";
import { StatusBar } from "./components/StatusBar.jsx";
import { TrackInspector } from "./components/TrackInspector.jsx";
import { WorkspaceToolbar } from "./components/WorkspaceToolbar.jsx";
import { StudioBrowser } from "./components/StudioBrowser.jsx";
import { useDawSession } from "./hooks/useDawSession.js";
import { midiSources, midiClipKey } from "./utils/midi-clips.js";
import { messages } from "./i18n.js";

export function App() {
  const [locale, setLocale] = useState(() => localStorage.getItem("ongaku.locale") || "en");
  const [fallbackClipKeys, setFallbackClipKeys] = useState([]);
  const [selectedClipKeys, setSelectedClipKeys] = useState(null);
  const [selectedTrackId, setSelectedTrackId] = useState(4);
  const [inspectorOpen, setInspectorOpen] = useState(false);
  const [browserOpen, setBrowserOpen] = useState(() => window.innerWidth > 620);
  const [editorOpen, setEditorOpen] = useState(true);
  const t = messages[locale] ?? messages.en;
  const session = useDawSession(t.unavailable);
  const selectedTrack = session.project?.tracks.find((track) => track.id === selectedTrackId);

  const allSources = useMemo(() => midiSources(session.project), [session.project]);
  const selectedSources = useMemo(() => {
    const keys = selectedClipKeys ?? (selectedTrack?.midiClips[0] ? [midiClipKey(selectedTrack.id, selectedTrack.midiClips[0].id)] : []);
    const resolve = list => list.map(key => allSources.find(source => midiClipKey(source.trackId, source.clip.id) === key)).filter(Boolean);
    const resolved = resolve(keys);
    if (resolved.length || !keys.length) return resolved;
    const fallback = resolve(fallbackClipKeys);
    return fallback.length ? fallback : allSources.filter(source => source.trackId === selectedTrackId).slice(0,1);
  }, [allSources, selectedClipKeys, fallbackClipKeys, selectedTrack, selectedTrackId]);
  const activeSource = selectedSources.at(-1);
  const activeTrack = session.project?.tracks.find(track => track.id === activeSource?.trackId);
  const effectiveClipKeys = selectedSources.map(source => midiClipKey(source.trackId, source.clip.id));
  const selectTrack = (id) => {
    setSelectedTrackId(id);
    const track = session.project?.tracks.find(track => track.id === id);
    setSelectedClipKeys(track?.midiClips[0] ? [midiClipKey(id, track.midiClips[0].id)] : []);
  };
  const selectClip = (trackId, clipId, additive = false) => {
    const key = midiClipKey(trackId, clipId);
    const keys = additive ? (effectiveClipKeys.includes(key) ? effectiveClipKeys.filter(item => item !== key) : [...effectiveClipKeys, key]) : [key];
    const active = allSources.find(source => midiClipKey(source.trackId, source.clip.id) === keys.at(-1));
    setSelectedTrackId(active?.trackId ?? trackId);
    setSelectedClipKeys(keys);
  };
  const selectAllClips = () => {
    setSelectedClipKeys(allSources.map(source => midiClipKey(source.trackId, source.clip.id)));
    if (allSources.length) setSelectedTrackId(allSources.at(-1).trackId);
  };
  const addClip = async () => {
    if (!activeSource || session.busy) return;
    const start = Math.min(session.project.lengthSamples - 1, session.transport?.currentSample ?? 0);
    const bar = Math.round(session.project.sampleRate * 60 / session.project.tempoBpm * session.project.timeSignature.numerator * 4 / session.project.timeSignature.denominator);
    const snapshot = await session.addMidiClip(activeSource.trackId, start, Math.min(bar, session.project.lengthSamples - start));
    const existing = new Set(allSources.map(source => midiClipKey(source.trackId, source.clip.id)));
    const added = midiSources(snapshot).find(source => !existing.has(midiClipKey(source.trackId, source.clip.id)));
    if (added) setSelectedClipKeys([...effectiveClipKeys, midiClipKey(added.trackId, added.clip.id)]);
  };

  const editClips = async (targets, edit) => {
    const existing = new Set(allSources.map(source => midiClipKey(source.trackId, source.clip.id)));
    const snapshot = await session.editMidiClips(targets, edit);
    if (!snapshot?.tracks) return false;
    const added = midiSources(snapshot).filter(source => !existing.has(midiClipKey(source.trackId, source.clip.id)));
    if (added.length) {
      if (edit.type === "duplicate") setFallbackClipKeys(effectiveClipKeys);
      const keys = added.map(source => midiClipKey(source.trackId, source.clip.id));
      setSelectedClipKeys(edit.type === "duplicate" ? keys : [...effectiveClipKeys, ...keys]);
      setSelectedTrackId(added.at(-1).trackId);
    }
    return snapshot;
  };

  useEffect(() => { setSelectedClipKeys(null); setFallbackClipKeys([]); }, [session.projectEpoch]);

  useEffect(() => {
    localStorage.setItem("ongaku.locale", locale);
    document.documentElement.lang = locale === "zh" ? "zh-CN" : locale;
  }, [locale]);

  useEffect(() => {
    if (session.project && !session.project.tracks.some((track) => track.id === selectedTrackId)) {
      setSelectedTrackId(session.project.tracks[0]?.id ?? 1);
    }
  }, [selectedTrackId, session.project]);

  useEffect(() => {
    const handleShortcut = (event) => {
      const target = event.target;
      const editing = target instanceof HTMLInputElement || target instanceof HTMLSelectElement
        || target instanceof HTMLTextAreaElement || target?.isContentEditable;
      if ((event.metaKey || event.ctrlKey) && !event.repeat) {
        const key = event.key.toLowerCase();
        if (["s", "o", "z", "y"].includes(key) && (!editing || key === "s" || key === "o")) {
          event.preventDefault();
          if (session.busy) return;
          if (key === "s") session.saveProject(event.shiftKey);
          else if (key === "o") {
            if (!session.documentStatus.dirty || window.confirm(t.discardChanges)) session.loadProject();
          } else if (key === "y" || event.shiftKey) session.redo();
          else session.undo();
          return;
        }
      }
      if (event.code === "Space" && !editing && !event.repeat) {
        event.preventDefault();
        session.togglePlayback();
      }
    };
    window.addEventListener("keydown", handleShortcut);
    return () => window.removeEventListener("keydown", handleShortcut);
  }, [session, t.discardChanges]);

  useEffect(() => {
    const warn = (event) => {
      if (session.documentStatus.dirty) { event.preventDefault(); event.returnValue = ""; }
    };
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, [session.documentStatus.dirty]);

  return (
    <div className="app-shell studio-shell">
      <AppMenu t={t} locale={locale} onLocaleChange={setLocale} session={session} />
      <WorkspaceToolbar t={t} selectedTrack={selectedTrack} inspectorOpen={inspectorOpen}
        editorOpen={editorOpen} browserOpen={browserOpen} onToggleBrowser={() => setBrowserOpen((value) => !value)}
        onToggleInspector={() => setInspectorOpen((value) => !value)}
        onToggleEditor={() => setEditorOpen((value) => !value)} />
      <div className={`studio-body${browserOpen ? " browser-open" : ""}`}>
      <main className={`workspace${inspectorOpen ? " has-inspector" : ""}${editorOpen ? " has-editor" : ""}`}>
        {(session.error || session.timingWarning) && <div className={`error-banner${session.error ? "" : " timing-warning"}`} role={session.error ? "alert" : "status"}>
          <strong>{session.error ? t.commandFailed : t.editTiming}</strong>
          <span>{session.error || (session.timingWarning === "late" ? t.lateEditNotice : t.adoptionHistoryNotice)}</span>
          <button type="button" onClick={() => session.error ? session.setError("") : session.dismissTimingWarning()}>{t.dismiss}</button></div>}
        {inspectorOpen && <TrackInspector t={t} track={selectedTrack} session={session} />}
        <EditorMap t={t} session={session} onEditClips={editClips} selectedClipKeys={effectiveClipKeys} onSelectClip={selectClip} selectedTrackId={selectedTrackId} onSelectTrack={selectTrack} />
        {editorOpen && <div className="detail-region">
          <div className="detail-region-bar"><span>{t.detailEditor}</span><strong>{selectedTrack ? t.trackRoles[selectedTrack.role] : "—"}</strong></div>
          {activeSource ? <MidiEditor key={session.projectEpoch} t={t} track={activeTrack} sources={selectedSources} allSources={allSources}
            playhead={session.transport?.currentSample ?? 0} onEditClips={editClips} onSelectClip={selectClip} onSelectAllClips={selectAllClips} onAddClip={addClip} onCrossEdit={session.editMidiNotes} busy={session.busy}
            sampleRate={session.project.sampleRate} tempoBpm={session.project.tempoBpm} onWriteNote={session.writeMidiNote}
            projectLength={session.project?.lengthSamples ?? 1} onVelocity={session.setMidiNoteVelocity}
            onAutomation={session.setAutomationPointValue} /> : <div className="audio-detail">
            <span>{selectedTrack?.midiClips?.length ? t.selectMidiClips : t.audioTrack}</span>
            <strong>{selectedTrack ? t.trackRoles[selectedTrack.role] : "—"}</strong>
            <span>{selectedTrack?.clips?.length ?? 0} {t.clips}</span>
          </div>}
        </div>}
      </main>
      {browserOpen && <StudioBrowser t={t} project={session.project} selectedClipKeys={effectiveClipKeys} onSelectClip={selectClip} selectedTrackId={selectedTrackId} onSelectTrack={selectTrack} />}
      </div>
      <AppHeader t={t} session={session}
        transportProps={{ project: session.project, transport: session.transport, running: session.running,
          busy: session.busy, onTogglePlayback: session.togglePlayback, onStop: session.stopPlayback,
          onSeek: session.seek, onToggleLoop: session.toggleLoop }} />
      <StatusBar t={t} config={session.config} metrics={session.metrics} running={session.running}
        previewMode={session.previewMode} publicationStatus={session.publicationStatus} />
    </div>
  );
}
