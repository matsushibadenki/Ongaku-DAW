# Phase 4 Validation

Validated on 2026-09-08 against the Phase 4 scope in the architecture specification.

## Implemented boundaries

- [Done] `daw-midi`: MIDI notes, clips, events, prepared scheduling, and a reusable reference instrument
- [Done] `daw-automation`: immutable prepared lanes with linear and step evaluation at integer sample positions
- [Done] `daw-mixer::RealtimeTimelineSource`: Transport-aware prepared source contract for instruments and metronomes
- [Done] `daw-project`: Rust-owned MIDI clips and Automation lanes with graph rebuilds after edits
- [Done] `daw-app` and Tauri: shell-independent MIDI velocity and Automation point commands
- [Done] React: Piano Roll, Automation Canvas, selection inspector, and multilingual editing controls
- [Done] Browser preview: the same Project snapshot and command contract without native audio hardware

MIDI timing is never driven by a JavaScript timer. Notes are compiled to integer sample positions and
processed from the Rust audio callback. Automation is sorted and validated on the control side; the
callback evaluates only the prepared lane. Project edits publish a new immutable graph using the
bounded block-boundary swap established in Phase 2.

## Realtime validation

- [Done] Note-on and note-off events preserve exact sample positions.
- [Done] A note beginning inside an audio block remains silent before its scheduled sample.
- [Done] Seek and loop discontinuities reconstruct active note state without allocation.
- [Done] MIDI instrument rendering performs no allocation or deallocation in the callback.
- [Done] Automation interpolation uses sample positions and performs no Project mutation.

## UI validation

- [Done] The four-track Arrangement displays audio waveform and MIDI note clips together.
- [Done] Piano Roll note selection exposes editable Velocity.
- [Done] Automation point selection exposes editable instrument level.
- [Done] English, Japanese, and Simplified Chinese copy is available.
- [Done] Desktop layout follows a reusable editor workbench pattern: full-width transport, left inspector rail, central arrangement, and lower MIDI/automation editor.
- [Done] Mixer controls use compact reusable MiniSlider and MiniVolume primitives with a consistent horizontal presentation.
- [Done] Arrangement Tracks can be reordered by drag and drop, with the new order persisted in the Rust Project model.
- [Done] Playwright checks passed at 1280 px and 320 px without horizontal overflow or wrapped controls.
- [Done] No framework overlay, application console error, or warning remains.

## Verification

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --release
npm run build
```

The workspace currently runs 35 Rust unit and integration tests, including callback allocation
tracking for the MIDI instrument with sample-position Automation enabled.

## Roadmap

- [Done] Phase 1 realtime baseline through Phase 4 MIDI and Automation
- [Next] Phase 5 recording, offline rendering, project save, undo/redo, and autosave
- [Later] Phase 6 plugin hosting, PDC, VST3, CLAP, and Audio Unit support
