import { useEffect, useRef, useState } from "react";
import { clipDragEdit, editProjectMidiClips, selectedMidiClips } from "../utils/midi-clip-edit.js";

const RULER_HEIGHT = 24;
const VISIBLE_LENGTH = 1.25;

export function TimelineCanvas({ t, project, transport, selectedTrackId, onSelectTrack, onSeek, selectedClipKeys, onSelectClip, onEditClips, busy }) {
  const canvasRef = useRef(null);
  const gesture = useRef(null);
  const pending = useRef(false);
  const [draft, setDraft] = useState(null);
  const [editing, setEditing] = useState(false);
  const cancel = () => { gesture.current = null; setDraft(null); };

  useEffect(() => { gesture.current = null; setDraft(null); }, [project, busy]);
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || !project) return undefined;
    const container = canvas.parentElement;
    const draw = () => drawTimeline(canvas, container, draft ?? project, transport, selectedTrackId, t, selectedClipKeys);
    const observer = new ResizeObserver(draw);
    observer.observe(container); draw();
    return () => observer.disconnect();
  }, [project, draft, selectedTrackId, transport, t, selectedClipKeys]);

  if (!project) return <div className="timeline-skeleton" aria-label={t.loadingProject} />;
  const commit = async (targets, edit) => {
    if (busy || pending.current) { setDraft(null); return; }
    pending.current = true; setEditing(true);
    try { await onEditClips(targets, edit); }
    finally { pending.current = false; setEditing(false); setDraft(null); }
  };
  const pointerDown = (event) => {
    if (event.button !== 0 || busy || pending.current) return;
    const canvas = event.currentTarget;
    const rect = canvas.getBoundingClientRect();
    const x = Math.max(0, Math.min(rect.width, event.clientX - rect.left));
    const y = event.clientY - rect.top;
    const sample = Math.min(project.lengthSamples, x / rect.width * project.lengthSamples * VISIBLE_LENGTH);
    const laneHeight = parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--daw-lane-height")) || 64;
    const track = project.tracks[Math.floor((y - RULER_HEIGHT) / laneHeight)];
    canvas.focus(); onSeek(sample);
    if (!track || y < RULER_HEIGHT) return;
    const clip = [...track.midiClips].reverse().find(clip => sample >= clip.timelineStart && sample < clip.timelineStart + clip.durationSamples);
    if (!clip) { onSelectTrack(track.id); return; }
    event.preventDefault();
    if (event.shiftKey || event.metaKey || event.ctrlKey) {
      onSelectClip(track.id, clip.id, true); return;
    }
    const selected = selectedClipKeys.includes(`${track.id}:${clip.id}`);
    const targets = selected ? selectedMidiClips(project,selectedClipKeys).map(source=>({trackId:source.trackId,clipId:source.clip.id})) : [{trackId:track.id,clipId:clip.id}];
    if (!selected) onSelectClip(track.id,clip.id,false);
    const left=clip.timelineStart/(project.lengthSamples*VISIBLE_LENGTH)*rect.width;
    const width=clip.durationSamples/(project.lengthSamples*VISIBLE_LENGTH)*rect.width;
    const edge=Math.min(6,width/3);
    const type=event.altKey ? "duplicate" : x<=left+edge ? "trimLeft" : x>=left+width-edge ? "trimRight" : "move";
    gesture.current={clip,targets,type,originX:x,pointerId:event.pointerId,width:rect.width};
    canvas.setPointerCapture(event.pointerId);
  };
  const pointerMove = (event) => {
    const active=gesture.current;
    if (!active || active.pointerId!==event.pointerId) return;
    const x=event.clientX-event.currentTarget.getBoundingClientRect().left;
    if (Math.abs(x-active.originX)<3 && !active.moved) return;
    active.moved=true;
    const grid=Math.max(1,Math.round(project.sampleRate*60/project.tempoBpm/4));
    active.edit=clipDragEdit(active,x,project.lengthSamples,active.width,grid);
    setDraft(editProjectMidiClips(project,active.targets,active.edit));
  };
  const pointerUp = (event) => {
    const active=gesture.current;
    if (!active || active.pointerId!==event.pointerId) return;
    pointerMove(event); gesture.current=null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    if (active.edit) commit(active.targets,active.edit); else setDraft(null);
  };
  const handleKeyDown = (event) => {
    if (event.key==="Escape") { event.preventDefault(); cancel(); return; }
    if (gesture.current || busy || pending.current) return;
    const current=transport?.currentSample ?? 0;
    const selected=selectedMidiClips(project,selectedClipKeys);
    const targets=selected.map(source=>({trackId:source.trackId,clipId:source.clip.id}));
    if ((event.metaKey||event.ctrlKey)&&event.key.toLowerCase()==="d") {
      event.preventDefault();
      if (selected.length) commit(targets,{type:"duplicate",deltaSamples:Math.max(...selected.map(({clip})=>clip.timelineStart+clip.durationSamples))-Math.min(...selected.map(({clip})=>clip.timelineStart))});
    } else if (event.key.toLowerCase()==="s"&&!event.metaKey&&!event.ctrlKey) {
      event.preventDefault();
      if (selected.some(({clip})=>current>clip.timelineStart&&current<clip.timelineStart+clip.durationSamples)) commit(targets,{type:"split",sample:current});
    } else if (event.key==="ArrowLeft"||event.key==="ArrowRight") {
      event.preventDefault(); onSeek(current+(event.key==="ArrowLeft"?-1:1)*project.sampleRate*60/project.tempoBpm);
    } else if (event.key==="Home") { event.preventDefault(); onSeek(0); }
  };
  return <div className="timeline-canvas-wrap">
    <canvas ref={canvasRef} className="timeline-canvas" role="application" tabIndex="0" aria-busy={editing}
      aria-label={t.timelineDescription} onPointerDown={pointerDown} onPointerMove={pointerMove} onPointerUp={pointerUp}
      onPointerCancel={cancel} onLostPointerCapture={() => { if (gesture.current) cancel(); }} onKeyDown={handleKeyDown} />
  </div>;
}

function drawTimeline(canvas, container, project, transport, selectedTrackId, t, selectedClipKeys) {
  const rect = container.getBoundingClientRect();
  const width = Math.max(1, Math.floor(rect.width));
  const height = Math.max(1, Math.floor(rect.height));
  const scale = Math.max(1, window.devicePixelRatio || 1);
  canvas.width = Math.floor(width * scale);
  canvas.height = Math.floor(height * scale);
  canvas.style.width = `${width}px`;
  canvas.style.height = `${height}px`;
  const context = canvas.getContext("2d");
  context.setTransform(scale, 0, 0, scale, 0, 0);
  const styles = getComputedStyle(document.documentElement);
  const color = (name) => styles.getPropertyValue(name).trim();
  const paper = color("--color-paper");
  const surface = color("--color-paper-3");
  const rule = color("--color-rule");
  const ruleStrong = color("--color-rule-strong");
  const ink = color("--color-ink");
  const muted = color("--color-muted");
  const accent = color("--color-accent");
  const roleColors = {
    rhythm: color("--color-track-rhythm"),
    bass: color("--color-track-bass"),
    atmosphere: color("--color-track-atmosphere"),
    lead: color("--color-track-lead"),
  };

  context.fillStyle = paper;
  context.fillRect(0, 0, width, height);
  context.fillStyle = surface;
  context.fillRect(0, 0, width, RULER_HEIGHT);
  const visibleLength = project.lengthSamples * VISIBLE_LENGTH;
  const beats = visibleLength / project.sampleRate * project.tempoBpm / 60;
  context.font = `11px ${color("--font-mono")}`;
  context.textBaseline = "top";
  for (let beat = 0; beat <= beats; beat += 1) {
    const x = beat / beats * width;
    const isBar = beat % project.timeSignature.numerator === 0;
    context.strokeStyle = isBar ? ruleStrong : rule;
    context.lineWidth = 1;
    context.beginPath();
    context.moveTo(Math.round(x) + 0.5, isBar ? 0 : RULER_HEIGHT - 8);
    context.lineTo(Math.round(x) + 0.5, height);
    context.stroke();
    if (isBar && beat < beats) {
      context.fillStyle = muted;
      context.fillText(String(beat / project.timeSignature.numerator + 1), x + 7, 9);
    }
  }

  const laneHeight = parseFloat(color("--daw-lane-height")) || 64;
  project.tracks.forEach((track, trackIndex) => {
    const laneTop = RULER_HEIGHT + laneHeight * trackIndex;
    if (track.id === selectedTrackId) {
      context.save();
      context.globalAlpha = 0.45;
      context.fillStyle = surface;
      context.fillRect(0, laneTop, width, laneHeight);
      context.restore();
    }
    context.strokeStyle = rule;
    context.beginPath();
    context.moveTo(0, Math.round(laneTop + laneHeight) + 0.5);
    context.lineTo(width, Math.round(laneTop + laneHeight) + 0.5);
    context.stroke();
    track.clips.forEach((clip) => drawClip(context, clip, track, laneTop, laneHeight, width, visibleLength, roleColors[track.role], color("--color-clip-ink")));
    (track.midiClips ?? []).forEach((clip) => drawMidiClip(context, clip, track, laneTop, laneHeight, width, visibleLength, roleColors[track.role], color("--color-clip-ink")));
    for (const clip of track.midiClips) {
      if (!(selectedClipKeys ?? []).includes(`${track.id}:${clip.id}`)) continue;
      context.strokeStyle = color("--color-focus"); context.lineWidth = 2;
      context.strokeRect(clip.timelineStart / visibleLength * width + 1, laneTop + 2,
        Math.max(1, clip.durationSamples / visibleLength * width - 2), laneHeight - 4);
      context.lineWidth = 1;
    }
    [...track.clips, ...(track.midiClips ?? [])].forEach((clip, index) => {
      const x = clip.timelineStart / visibleLength * width;
      const clipWidth = clip.durationSamples / visibleLength * width;
      context.save();
      context.beginPath(); context.rect(x, laneTop + 1, clipWidth, 12); context.clip();
      context.globalAlpha = 0.15; context.fillStyle = color("--color-clip-ink");
      context.fillRect(x, laneTop + 1, clipWidth, 12);
      context.globalAlpha = 1; context.font = `8px ${color("--font-body")}`;
      context.fillText(`${t.trackRoles[track.role]} · ${index + 1}`, x + 4, laneTop + 3);
      context.restore();
    });
  });

  const loop = transport?.loopRange;
  if (loop) {
    const start = loop.start / visibleLength * width;
    const end = loop.end / visibleLength * width;
    context.save();
    context.globalAlpha = 0.1;
    context.fillStyle = accent;
    context.fillRect(start, RULER_HEIGHT, end - start, height - RULER_HEIGHT);
    context.restore();
    context.fillStyle = accent;
    context.fillRect(start, 0, Math.max(2, end - start), 2);
  }

  const playhead = (transport?.currentSample ?? 0) / visibleLength * width;
  context.strokeStyle = accent;
  context.lineWidth = 1.5;
  context.beginPath();
  context.moveTo(playhead + 0.5, 0);
  context.lineTo(playhead + 0.5, height);
  context.stroke();
  context.fillStyle = accent;
  context.beginPath();
  context.moveTo(playhead - 4, 0);
  context.lineTo(playhead + 5, 0);
  context.lineTo(playhead + 0.5, 7);
  context.closePath();
  context.fill();

  context.fillStyle = ink;
}

function drawMidiClip(context, clip, track, laneTop, laneHeight, width, projectLength, color, border) {
  const x = clip.timelineStart / projectLength * width;
  const clipWidth = Math.max(2, clip.durationSamples / projectLength * width);
  const top = laneTop + 1;
  const clipHeight = Math.max(12, laneHeight - 2);
  context.save();
  context.globalAlpha = track.muted ? 0.25 : 1;
  context.fillStyle = color;
  context.fillRect(x, top, clipWidth, clipHeight);
  context.restore();
  context.strokeStyle = border;
  context.strokeRect(Math.round(x) + 0.5, Math.round(top) + 0.5, Math.max(1, Math.round(clipWidth) - 1), Math.max(1, Math.round(clipHeight) - 1));
  context.fillStyle = border;
  for (const note of clip.notes) {
    const noteX = x + note.start / projectLength * width;
    const noteWidth = Math.max(2, note.durationSamples / projectLength * width);
    const pitch = Math.max(0, Math.min(1, (note.key - 48) / 36));
    const noteY = top + (1 - pitch) * Math.max(1, clipHeight - 5);
    context.globalAlpha = track.muted ? 0.25 : 0.5 + note.velocity * 0.5;
    context.fillRect(noteX, noteY, noteWidth, 3);
  }
  context.globalAlpha = 1;
}

function drawClip(context, clip, track, laneTop, laneHeight, width, projectLength, color, border) {
  const x = clip.timelineStart / projectLength * width;
  const clipWidth = Math.max(2, clip.durationSamples / projectLength * width);
  const top = laneTop + 1;
  const clipHeight = Math.max(12, laneHeight - 2);
  context.save();
  context.globalAlpha = track.muted ? 0.25 : 1;
  context.fillStyle = color;
  context.fillRect(x, top, clipWidth, clipHeight);
  context.restore();
  context.strokeStyle = border;
  context.strokeRect(Math.round(x) + 0.5, Math.round(top) + 0.5, Math.max(1, Math.round(clipWidth) - 1), Math.max(1, Math.round(clipHeight) - 1));
  context.strokeStyle = border;
  context.lineWidth = 1;
  const middle = top + clipHeight / 2;
  const amplitude = clipHeight * 0.42;
  const step = clipWidth / clip.waveform.length;
  context.beginPath();
  clip.waveform.forEach((peak, index) => {
    const peakX = x + (index + 0.5) * step;
    context.moveTo(peakX, middle - peak.maximum * amplitude);
    context.lineTo(peakX, middle - peak.minimum * amplitude);
  });
  context.stroke();
}
