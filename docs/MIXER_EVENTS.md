# Sample-position mixer events

The Rust project supports persistent Gain, Pan, Mute and Solo target changes at zero-based integer **project sample positions**. This is a native engine/editing API; a dedicated graphical event-lane editor is not yet implemented. The browser preview edits/stores the same metadata, but does not implement a realtime audio clock.

## Editing API

`engineApi.setMixerEvent(trackId, event)` inserts or replaces a track-local stable event ID. `engineApi.removeMixerEvent(trackId, eventId)` removes it. Both return a project snapshot and use ordinary atomic document edits and Undo/Redo. Desktop graph preparation is asynchronous; inspect [preparation/publication status](GRAPH_PREPARATION.md) to distinguish a committed edit from accepted/adopted audio. Native Tauri commands are `set_mixer_event` and `remove_mixer_event`; Rust applications use `DawApplication::set_mixer_event` / `remove_mixer_event`.

```js
await engineApi.setMixerEvent(4, {
  id: 10,
  position: 48000,
  change: { parameter: "gain", value: 0.4 },
});
await engineApi.setMixerEvent(4, {
  id: 11,
  position: 48127,
  change: { parameter: "mute", value: true },
});
await engineApi.undo();
await engineApi.redo();
await engineApi.removeMixerEvent(4, 11);
```

`change` uses `parameter: "gain" | "pan" | "mute" | "solo"`; Gain ranges from 0 to 2, Pan from -1 to 1, and Mute/Solo use booleans. Values must be finite. A track accepts at most 4,096 events with unique IDs; event positions must be inside `[0, project.lengthSamples)`. JavaScript callers should use safe integer IDs and positions; the native representation is `u64`.

## Playback contract

- A prepared graph must be adopted **at or before rendering** the event sample for the event to affect that exact sample. Graph publication is callback-boundary work with latest-wins coalescing for application snapshots. The [publication protocol](GRAPH_PUBLICATION.md) now reports accepted/adopted revisions and actual late adoption; acceptance alone still cannot guarantee a future deadline. Late/past edits chase the current timeline value at the first sample rendered with the new graph; they cannot alter already rendered audio.
- Before rendering each frame, evaluate the latest event at or before the current project sample for each parameter. For events at the same position and parameter, the **largest event ID wins**, independent of insertion order. All tracks' Solo values are evaluated before any exclusion decisions.
- Events override their parameter's base track setting from their position onward. Before the first event, that parameter uses the base track setting. Base fader/button edits still modify the base document value; they do not override an active event. To change scheduled playback, edit/remove the event or insert a later event.
- A changed target starts the existing 5 ms sample-counted coefficient ramp **on the event sample**. The first affected coefficient is one ramp increment from the current level. Repeated identical targets do not restart the ramp. No MIDI, clip or transport position is shifted.
- Every loop pass evaluates project positions again. Backward seeks and loop wraps chase destination values instead of retaining values from later in the previous pass. A target change ramps from the current audible coefficient; source seek/loop phase policy remains unchanged. Stopped transport consumes no ramp samples.
- Graph publication inherits live ramp state and compatible reference-MIDI voices by stable track ID; the new event schedule remains authoritative. Undo/Redo changes the document/schedule, never rewinds the clock or restores historical phase/ramp cursors. Removing a past event ramps toward the newly evaluated value; undoing a future event removes its future effect once the revised graph is adopted.
- Preparation validates and sorts four parameter lanes outside the callback. Rendering uses four bounded binary searches per track/frame, fixed-size evaluated controls, and the existing ramp state. No event insertion, allocation, deallocation, mutex, I/O or command sorting runs in the callback. Dense identical-time events collapse to the winning target without a per-frame event-drain loop. Total work still depends on the track count; broader project/resource budgets remain P0 work.

## Persistence and compatibility

Track snapshots include `mixerEvents`. The new reader defaults a missing field to an empty list and loads version 1 files. Projects with any mixer events save as **`.ongaku` version 2**, ensuring older version-1-only readers reject unsupported scheduled behavior instead of silently dropping it. Projects without mixer events continue to save as version 1. Version 1 documents containing nonempty event data are rejected. Event IDs/positions/values are validated before replacing the active document; invalid edits and rejected graph publication leave project/history state intact.

Sample-rate conversion of event-bearing demo projects is explicitly rejected; automatic event rescaling/resampling is not implemented. Project loading remains a full stopped-engine replacement, without state inheritance from a different document.

## Evidence and remaining scope

The [P0 validation record](P0_VALIDATION.md) and [roadmap](../ROADMAP.md) track exact-boundary, partition, loop, seek, history, queue-saturation, allocation/free and storage tests. Native hardware loopback, listening and endurance qualification remain pending. Graph adoption acknowledgements and late-edit notifications are implemented. Pending graph cancellation/coalescing is also implemented; proactive deadline admission estimates, cancellation of startup/import/analysis work, graphical event lanes, external timestamped controller input, plugin bypass and general effects/stream state transfer remain unfinished. No Pro-Wave source or assets were used.
