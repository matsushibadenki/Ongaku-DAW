# Ongaku DAW

Ongaku is an independently implemented digital audio workstation built with **Rust, Tauri 2, and React**. The Rust engine owns the audio clock and project data. The WebView provides the interface and sends editing commands.

**Playback accuracy and stability take precedence over feature count, interface polish, and release dates.** The development goal is a full production workflow comparable in scope to Pro-Wave, with stricter evidence requirements for timing, continuity, overload behavior, and session recovery. Comparative performance claims require reproducible measurements on matched hardware and projects.

> Development status: an editable DAW foundation with audio playback, MIDI sequencing, project files, and undoable arrangement editing. Recording, production instruments/effects, plugin hosting, and broad hardware qualification remain unfinished.

See the [detailed roadmap](ROADMAP.md) for implementation status, priorities, feature coverage, and release gates.

## Implemented functionality

- **Audio foundation:** independent Rust engine; backend abstraction; CPAL output with capability-matched format/buffer validation, explicit selected-output metadata and a mock backend; prepared graphs; bounded graph/transport queues; off-callback graph reclamation; epoch-scoped graph resolution receipts, latest-wins pending snapshot coalescing, cancelable single-worker graph preparation, scheduled-edit deadline warnings, and epoch-scoped Transport acceptance IDs with readiness/cancellation results.
- **Transport:** integer sample positions, play/pause, seek, loop ranges, tempo and time-signature state; complete-window stream readiness holds musical time safely during cold reads. Accepted command IDs distinguish queued, waiting, applied and canceled results, with explicit post-mutation cancellation semantics. Recording state exists in the model; audio capture is not implemented.
- **Audio sources:** immutable PCM assets, non-destructive audio clip ranges, background WAV decoding, bounded read-ahead streaming, project-integrated file-backed timeline caches shared across edits/Undo/Redo, and min/max/RMS waveform analysis. These building blocks are not yet a complete audio-import/recording editor.
- **Mixing:** track gain, pan, mute/solo, master gain, timeline-aligned audio rendering, and persistent sample-position Gain/Pan/Mute/Solo events through the native editing API. Reusable mixer controls are integrated into the arrangement and inspector.
- **MIDI:** sample-position scheduling and a small reference instrument; note creation, movement, resizing, transposition, velocity edits, selection/marquee selection, deletion, and grid quantization.
- **Multiple clips:** MIDI clip creation and selection; project-time note editing across clips and tracks; clip movement, independent duplication, edge trimming, and playhead splitting.
- **History:** Rust-owned Undo/Redo with up to 100 history entries, coalesced continuous parameter edits, and one history step per committed gesture or batch operation.
- **Persistence:** self-contained `.ongaku` projects (versions 1/2) with PCM, MIDI, automation, and mixer settings; Save/Save As/Open; validation before document replacement; temporary-file writing and atomic replacement after flushing/synchronizing the file.
- **Automation:** prepared linear/step lanes and instrument-level automation editing. Full channel/plugin automation and recording modes are planned.
- **Interface:** dense arrangement workspace, inspector, piano roll, project browser, transport, telemetry, and English, Japanese, and Simplified Chinese UI text.
- **Development preview:** the UI can run without Tauri through a preview adapter. Preview projects use browser local storage, separate from native `.ongaku` files.

## Playback principles

1. The hardware audio callback advances playback. JavaScript timers and animation frames never schedule audio or MIDI.
2. Schedule events against integer sample positions; specify rounding when converting musical time and test boundary behavior.
3. Prepare graphs, decode files, allocate buffers, and reclaim retired graphs outside the realtime callback.
4. Keep callback work bounded. Queue saturation, missing media, device failures, and plugin timeouts must have explicit, observable outcomes.
5. Preserve playback state across edits. Voice continuity, phase, effect tails, automation state, and read positions need regression evidence before production qualification.
6. Protect project integrity: failed edits and failed loads must leave the active document intact; recovery must be testable.
7. Every new instrument, effect, editor, or backend inherits the same correctness and stability gates.

Current allocation-regression tests cover the existing callback, graph swap, streaming, MIDI instrument, and automation paths. A complete production qualification matrix is still required; see [roadmap acceptance gates](ROADMAP.md#3-playback-and-stability-acceptance-gates).

## Architecture

```text
React / WebView
    | commands and bounded view snapshots
Tauri adapter
    |
Rust application / Project / history / file services
    | prepared graphs and bounded control messages
Rust realtime engine
    | Transport / MIDI / automation / mixer / DSP
AudioBackend abstraction
    | CPAL output or mock backend
Audio device
```

The engine and project crates do not depend on Tauri. Desktop edits commit the Rust document/history and request a bounded background preparation worker; stale work is cooperatively canceled before publication. Preparation, queue acceptance and actual adoption have distinct status/IDs. Control-side mutexes are separate from the realtime callback, which consumes prepared messages through bounded queues. The synchronous Rust control API remains available.

Heavy rendering and GPU work are planned for a Rust-owned renderer using **wgpu**, with resources kept on the Rust/GPU side. Current timeline and piano-roll rendering uses WebView Canvas. Rust-owned workspace state, native docking, floating panels, and multi-window synchronization are planned, not implemented by the present panel toggles.

| Area | Location |
| --- | --- |
| Formats, configuration, errors, and source contracts | `crates/audio-types` |
| Transport and sample-time primitives | `crates/daw-core` |
| PCM assets and audio clip ranges | `crates/audio-clip` |
| Realtime processing and telemetry | `crates/audio-engine` |
| Backend contract and mock output | `crates/audio-backend` |
| CPAL output adapter | `crates/audio-backend-cpal` |
| Mixer, DSP, MIDI, and automation | `crates/mixer`, `crates/dsp`, `crates/midi`, `crates/automation` |
| WAV decoding, streaming, and waveform analysis | `crates/audio-file`, `crates/streaming`, `crates/waveform` |
| Project model, file storage, and editing operations | `crates/daw-project` |
| Application control and history | `crates/daw-app` |
| Desktop shell and commands | `src-tauri` |
| React interface and browser preview adapter | `src` |

## Development

Use the Rust toolchain pinned in `rust-toolchain.toml` (currently Rust 1.93.0), Node.js compatible with the locked Vite version, npm, and the native build dependencies needed by Tauri and CPAL on your operating system.

```sh
npm ci
npm run tauri dev
```

Browser-only interface development:

```sh
npm run dev
```

The initial project, **Night Sketch**, has four bars and four tracks: three generated audio tracks and one MIDI track. Its sounds are generated by this project, without external sample-library dependencies.

### Verification

Run from the repository root:

```sh
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets
npm run test:midi
npm run test:mixer
npm run test:transport
npm run build
```

Native packaging:

```sh
npm run tauri build
```

The current Clippy run reports existing warnings. A warning-clean release baseline and stricter CI enforcement are roadmap work; passing a browser build does not qualify native audio behavior.

### Basic editing

- Click a MIDI clip to select it; Shift/Cmd/Ctrl-click adds or removes clips. The clip picker also supports selection.
- Double-click empty piano-roll space or use `+` to add notes. Drag note bodies to move and right edges to resize.
- Use modifier-click or marquee selection for multiple notes; `Q` quantizes selected notes and Delete/Backspace removes them when the piano roll has focus.
- Drag MIDI clip bodies in Arrangement to move, Alt-drag to duplicate, and drag edges to trim. The clip tools also provide numeric position/edge fields, duplication, and splitting at the playhead.
- With Arrangement focused, Cmd/Ctrl+D duplicates and `S` splits. Escape cancels an unfinished drag.
- Cmd/Ctrl+Z undoes; Shift+Cmd/Ctrl+Z or Ctrl+Y redoes. Cmd/Ctrl+S saves, Shift+Cmd/Ctrl+S saves as, and Cmd/Ctrl+O opens.

## Current limits

- The reference instrument is a small 16-voice implementation. Per-note ownership handles overlapping unison notes and deterministic voice stealing. Sustain/controller reconstruction, live-edit continuity, and production-grade instruments remain priorities.
- Live project edits build prepared graphs and inherit compatible reference-instrument voices at publication. Unchanged notes keep phase through edits and Undo/Redo; changed notes are reconciled by stable identity. Track Gain/Pan/Mute/Solo use sample-counted 5 ms ramps that survive publication and history edits; prepared-graph master Gain also ramps. Project streams retain shared immutable media caches and address exact source frames across seeks, loops and history publications. Complete callback windows are admitted with retained cache pages, preserving MIDI/ramp state while loading. General effect-tail continuity and scheduled readiness/deadline guarantees remain unfinished.
- Native project streaming spools embedded PCM into private temporary backing files and warms a bounded cache off audio. Cold/evicted output windows hold the musical timeline and emit silence until all required pages are retained; failure/capacity states are explicit. Last-callback readiness receipts and localized status icons expose the result. Cold seeks/large loops are not guaranteed gap-free in wall-clock time. The document still retains its full PCM, and aggregate stream budgets/automatic recovery remain planned.
- Constant-tempo musical conversion uses exact integer quotient/remainder arithmetic and a separately tested rational oracle; see the [clock contract and limits](docs/MUSICAL_CLOCK.md). Musical single-clip quantization computes each subdivision directly to avoid fractional-step drift; stored sample positions retain their existing timing.
- Engine telemetry includes process duration, an approximate P99.9 histogram and process deadline misses (`xrunCount` remains a compatibility alias). Separate output observations report adapter-body duration, delivered frame ranges, requested-buffer mismatches and typed adapter versus driver notifications. These are not physical xrun, wake-up or latency measurements; see [output telemetry](docs/OUTPUT_TELEMETRY.md).
- `.ongaku` stores JSON with embedded PCM; projects containing scheduled mixer events use version 2, while event-free projects use version 1. The new reader accepts both versions and currently limits input files to 512 MiB. Media deduplication, compression, autosave/recovery, and background file-operation progress are planned. Undo history is not saved.
- MIDI trimming removes or shortens notes outside the new range. Undo restores the previous state; extending a trimmed edge adds silence and does not recover discarded notes. A persistent source-window model is planned for non-destructive MIDI trimming.
- MIDI splits divide crossing notes into left/right notes; the right side retriggers. Sustain/tie-preserving split behavior is planned.
- Clip duplication stays within the current project length; copies may overlap when insufficient space remains. Track-to-track clip dragging and automatic project extension are planned.
- Device configuration must match the project sample rate and advertised output capabilities. Unsupported requests fail without replacing the document; native status reports requested/validated settings and encoding. Unknown buffer bounds stay explicitly unknown. There is no automatic project resampling, measured driver-period negotiation or production device-recovery policy yet. Malformed/oversized integer callbacks silence and report errors; variable-period recovery remains unfinished.
- Modern hardware/OS qualification remains pending. Historical macOS observations in the validation documents apply to their recorded versions and workloads.

## Documentation

- [Detailed product and reliability roadmap](ROADMAP.md): authoritative forward plan and release gates.
- [Cancelable background graph preparation](docs/GRAPH_PREPARATION.md): request IDs, history/persistence semantics, cancellation checkpoints, bounded retry and failure handling.
- [Graph adoption and deadline protocol](docs/GRAPH_PUBLICATION.md): accepted/adopted revisions, history mapping, bounded receipts, and timing notices.
- [Sample-position mixer event API](docs/MIXER_EVENTS.md): scheduling, history, loop/seek behavior, and file compatibility.
- [Output configuration and PCM boundaries](docs/OUTPUT_CONFIGURATION.md): advertised capability matching, failure-safe startup, selection metadata and tested conversion/fault policy.
- [Transport command IDs and results](docs/TRANSPORT_COMMANDS.md): accepted IDs, epoch checks, readiness/cancellation, Stop priority and bounded history.
- [Stream readiness and safe transport admission](docs/STREAMING_READINESS.md): retained windows, waiting policy, graph/history consistency and observable receipts.
- [Streaming handoff contract](docs/STREAMING_VALIDATION.md): seek generations, complete-frame delivery, project timeline integration, shared history caches, frame integrity and remaining readiness/resource work.
- [P0 validation record](docs/P0_VALIDATION.md): playback fixes, regression evidence, and remaining qualification gaps.
- [Implemented editing workflows](docs/REFERENCE_ROADMAP.md): chronological implementation notes, behavior, and validation details, currently in Japanese.
- [Architecture specification](docs/Cross-Platform%20Rust%20DAW%20Architecture%20Specification.md): original architecture direction.
- [Screen design](docs/SCREEN_DESIGN.md): interface structure and rendering limitations.
- [Phase 1](docs/PHASE1_VALIDATION.md), [Phase 2](docs/PHASE2_VALIDATION.md), [Phase 3](docs/PHASE3_VALIDATION.md), and [Phase 4 validation](docs/PHASE4_VALIDATION.md): historical evidence, not statements that later release gates have passed.

## Independent implementation

Pro-Wave is a temporary local reference for design and functional scope. Ongaku does not reuse its source code, artwork, sounds, presets, or binary seeds. It is not a build dependency and can be removed without affecting the application.

The reference directory is excluded from Git. Future file-format or plugin compatibility requires its own implementation, licensing review, and interoperability fixtures; no proprietary format compatibility is implied by the current editing features.
