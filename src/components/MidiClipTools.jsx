import { useRef, useState } from 'react';

export function MidiClipTools({t,sources,sampleRate,tempoBpm,projectLength,playhead,busy,onEditClips}) {
  const [editing,setEditing]=useState(false);
  const pending=useRef(false);
  const active=sources.at(-1)?.clip;
  if (!active) return null;
  const beat=sampleRate*60/tempoBpm;
  const targets=sources.map(source=>({trackId:source.trackId,clipId:source.clip.id}));
  const edit=async(action)=>{
    if (busy||pending.current) return;
    pending.current=true;setEditing(true);
    try { await onEditClips(targets,action); } finally {pending.current=false;setEditing(false);}
  };
  const span=Math.max(...sources.map(({clip})=>clip.timelineStart+clip.durationSamples))-Math.min(...sources.map(({clip})=>clip.timelineStart));
  const canSplit=sources.some(({clip})=>playhead>clip.timelineStart&&playhead<clip.timelineStart+clip.durationSamples);
  return <fieldset className="midi-clip-tools" disabled={busy||editing} aria-label={t.clipOperations}>
    <button type="button" title={t.duplicateClips} aria-label={t.duplicateClips} onClick={()=>edit({type:'duplicate',deltaSamples:span})}>⧉</button>
    <button type="button" title={t.splitClips} aria-label={t.splitClips} disabled={!canSplit} onClick={()=>edit({type:'split',sample:playhead})}>✂</button>
    <ClipField label={t.clipPosition} value={active.timelineStart/beat} max={projectLength/beat}
      onCommit={value=>edit({type:'move',deltaSamples:Math.round(value*beat)-active.timelineStart})} />
    <ClipField label={t.clipLeft} value={active.timelineStart/beat} max={projectLength/beat}
      onCommit={value=>edit({type:'trimLeft',deltaSamples:Math.round(value*beat)-active.timelineStart})} />
    <ClipField label={t.clipRight} value={(active.timelineStart+active.durationSamples)/beat} max={projectLength/beat}
      onCommit={value=>edit({type:'trimRight',deltaSamples:Math.round(value*beat)-active.timelineStart-active.durationSamples})} />
    <span className="midi-gesture-help" title={t.clipOperationHelp} aria-label={t.clipOperationHelp}>ⓘ</span>
  </fieldset>;
}
function ClipField({label,value,max,onCommit}) {
  const display=Number(value.toFixed(4));
  return <label><span>{label}</span><input key={display} type="number" min={0} max={max} step={0.25}
    aria-label={label} defaultValue={display} onKeyDown={event=>{
      if (event.key==='Escape') {event.currentTarget.value=String(display);event.currentTarget.blur();}
      if (event.key==='Enter') event.currentTarget.blur();
    }} onBlur={event=>{
      const next=event.currentTarget.valueAsNumber;
      event.currentTarget.value=String(display);
      if (Number.isFinite(next)&&next!==display) onCommit(Math.max(0,Math.min(max,next)));
      else event.currentTarget.value=String(display);
    }}/></label>;
}
