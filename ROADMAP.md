# Ongaku DAW — Product and Playback Reliability Roadmap

Planning baseline: **2026-11-05**. This is the authoritative forward plan. Historical validation reports and `docs/REFERENCE_ROADMAP.md` describe earlier implementations and observations; they do not supersede the release gates here.

## 1. Product objective and priority order

Build an independent professional DAW with a functional breadth comparable to the workflows and feature families described by Pro-Wave. Implement the behavior in Ongaku's Rust/Tauri architecture without copying Pro-Wave code, artwork, sounds, presets, or binary seeds.

**Playback accuracy and stability are the highest product priorities.** Timing correctness, continuous sound, bounded execution, and recovery take precedence over adding instruments, matching another application's appearance, meeting a date, or winning a track-count benchmark. Feature parity never exempts a subsystem from these requirements.

Decisions use this order:

1. Correct and stable playback, recording timing, and audio-device behavior.
2. Session integrity, recoverability, and transparent error reporting.
3. Predictable resource use, latency, and responsiveness under load.
4. Complete production workflows and interoperable files.
5. Feature breadth, visual polish, and optional services.

The goal is to make these priorities stronger than those of competing DAWs. Any published claim of superior accuracy, stability, or latency must identify the tested competitors, versions, hardware, drivers, projects, settings, and raw results. No unmeasured universal superiority claim is an acceptance criterion.

### Status notation

- 🟢 [Done] Implemented in the current codebase, within the stated scope.
- 🟠 [Next] Highest-priority unfinished work to pursue now.
- 🔴 [Later] Planned work that is not the closest next step.
- ⭕️ [Pending] Verification requiring an unavailable environment, hardware, fixture, or external dependency. An unperformed test is not a pass.

Code completion and qualification are separate. A feature can have implemented code while its native or hardware acceptance remains pending. Planned targets below are not current performance guarantees.

### What “comparable scope” means

| Coverage level | Required outcome |
| --- | --- |
| Core workflow parity | Arrange audio/MIDI, record and edit takes, use instruments/patches, mix with effects/automation, save/recover projects, and deliver audio/MIDI reliably. |
| Extended functional parity | Advanced sequencing, sampling/synthesis, time/pitch editing, plugin formats, buses/VCA/groups, notation, loops, synchronization, control surfaces, and the instrument/effect families catalogued below. |
| Independent extensions | Native workspace/renderer architecture, fault isolation, long-running reliability qualification, optional mobile/web/remote workflows, and optional AI services. |

Compatibility with a product's proprietary presets, project binaries, or exact sound is a separate deliverable. Similar names or controls do not establish compatibility, and a reference implementation's planned features are not assumed to be working today.

## 2. Current implementation and qualification gaps

| Area | Implemented baseline | Remaining qualification or scope |
| --- | --- | --- |
| Engine | 🟢 [Done] Independent Rust callback, prepared graph exchange, bounded queues, retired-graph reclamation, allocation/free regression tests. | 🟠 [Next] Stress/failure matrix, general processor/stream continuity, physical driver/wake-up accounting. Actual callback-period accounting is implemented. |
| Transport | 🟢 [Done] Integer sample positions, play/pause, seek, loops, tempo/meter state, stopped-position editing. | 🟠 [Next] Comprehensive musical-time boundary oracle, general transport command acknowledgements, dense MIDI qualification. Graph publication receipts are implemented. |
| Audio | 🟢 [Done] PCM assets, audio clip views, WAV decoder, read-ahead streaming, waveform pyramid. | 🔴 [Later] Complete import/editor, input recording, additional codecs, offline bounce, project resampling. |
| MIDI notes | 🟢 [Done] Create/move/resize/delete/transpose/velocity, multiple selection, marquee, quantization, cross-clip/track editing. | 🟠 [Next] Sustain/controller state, explicit panic/chase semantics, and general instrument state contracts. Reference-instrument per-note ownership and compatible live-edit voice continuity are implemented. 🔴 [Later] Advanced transforms and expression editors. |
| MIDI clips | 🟢 [Done] Creation, selection, group movement, independent copies, edge trim, playhead split, atomic history. | 🔴 [Later] Persistent non-destructive source windows, ties across splits, track-to-track clip dragging, automatic project extension. |
| Mixing | 🟢 [Done] Track gain/pan/mute/solo, master gain, prepared timeline rendering, 5 ms mixer ramps with publication/history continuity, persistent sample-position Gain/Pan/Mute/Solo events. | 🔴 [Later] Buses, sends, effects, PDC, metering suite, production summing policy. |
| Instrument/automation | 🟢 [Done] Small 16-voice reference instrument, prepared linear/step lanes, instrument-level editing. | 🔴 [Later] Production instruments, controller handling, automation modes and full parameter coverage. |
| Project files | 🟢 [Done] Version 1/2 `.ongaku`, embedded PCM, validation, temporary-file replacement, Save/Save As/Open. | 🟠 [Next] Recovery, migration/resource limits, crash-durable save qualification, background I/O. |
| History | 🟢 [Done] Rust-owned 100-entry Undo/Redo, parameter coalescing, atomic batch edits. | 🟠 [Next] Long-session memory budget and saved-revision/recovery integration. Audio publication receipts now map Undo/Redo document revisions. |
| Interface | 🟢 [Done] Arrangement, track controls, inspector, MIDI editor, project browser, three languages, browser preview. | 🔴 [Later] Rust-owned workspace state, docking/floating windows, wgpu rendering, accessibility expansion. |
| Platforms | 🟢 [Done] CPAL/mock adapters and desktop shell. Historical macOS physical-output evidence exists. | ⭕️ [Pending] Current-build macOS/Windows/Linux hardware qualification and native dialog/recovery matrix. |

Important current boundaries:

- “Sample-position scheduling is implemented” does not mean every live-edit, overlapping-note, device, or plugin scenario has been qualified.
- The callback's current xrun field counts measured processing-deadline exceedances. Driver underruns, wake-up lateness, and physical latency need separate measurements.
- Current project edits prepare fresh graphs; continuous oscillator phase, ongoing voices, future effect tails, and all streaming states need explicit state handoff.
- MIDI trim currently crops note data. Undo restores it, but extending the edge after trimming does not reveal a retained source. Splits retrigger the right-hand portion of crossing notes.
- Current rendering uses WebView Canvas. wgpu, native docking, and multi-window state ownership are future work.

## 3. Playback and stability acceptance gates

Every phase inherits these gates. Tests must distinguish implemented coverage, planned tests, and hardware-pending evidence. A feature regression in timing or stability blocks release even when its UI works.

### 3.1 Timing and signal correctness

| Gate | Required behavior | Evidence required |
| --- | --- | --- |
| T1: sample timing | For explicit sample positions, internal audio/MIDI/automation events occur at the exact intended sample: **zero scheduling-sample error**. | Impulse/event oracle tests at block boundaries, note boundaries, clip edges, and loop wraps; event logs against expected integer positions. |
| T2: musical conversion | Define rounding for ticks/beats/tempo maps; avoid cumulative floating-point drift. Compare each conversion to an independent rational/high-precision reference. | Fixed-tempo, tempo-ramp, compound/irregular-meter, and long-duration property tests. Round-to-nearest conversions must meet the documented half-sample bound. |
| T3: block partition | Supported block partitioning must not move events, lose/duplicate frames, or change total rendered length. | Software blocks of 1, 17, 64, 127, 128, 257, 512, and 1024 frames; changing block lengths; loops shorter than one block and multiple wraps per block. Hardware support is qualified separately. |
| T4: discontinuity | Seek, pause/resume, stop, looping, punch boundaries, and project replacement reconstruct the intended musical state. | Tests of voices, sustain, pitch bend, CCs, automation, delay history policy, and streaming generation changes. No stuck notes or stale-buffer replay. |
| T5: live-edit continuity | Parameter edits and graph changes preserve unaffected voices, phase, envelopes, tails, and media positions. | Sustained-note and sustained-tail fixtures while moving faders, editing unrelated tracks, reordering, Undo/Redo, and repeated graph publication. |
| T6: rendering agreement | Deterministic realtime/offline paths agree under a fixed build, configuration, seed, and processing order. | Exact PCM comparison for exact paths; a documented numerical error budget for paths with unavoidable floating-point differences. Timing errors cannot be excused as numerical tolerance. |
| T7: latency alignment | Routing, sends, sidechains, look-ahead, oversampling, and plugin paths remain aligned after latency compensation. | Impulse and null tests; exact integer delay alignment; documented fractional-delay/interpolation tolerance; latency change and bypass transitions. |

A discontinuity policy must specify whether voices restart, continue, release, or crossfade. An audible fade may be an intentional transport behavior; it must not conceal a scheduling defect or silently shift the timeline.

### 3.2 Realtime execution and overload

| Gate | Required behavior | Evidence required |
| --- | --- | --- |
| R1: callback discipline | Zero heap allocation/deallocation, blocking locks, file/network I/O, synchronous logging, GPU waits, or waits for a UI/worker/plugin process in the callback. | Allocation/free instrumentation covering success, error, queue-full, graph retirement, seek, loop, and shutdown paths; code review of callback-reachable code. |
| R2: bounded work | Bound commands, graph adoption, voices/events, interpolation, and source work per callback. No unbounded retry/spin loop in the realtime path. | Explicit capacity/work budgets, saturation tests, and worst-case event-density fixtures. |
| R3: deadline margin | On a declared qualification workload, target P99.99 callback execution at or below 50% of the actual buffer period and observed maximum at or below 80%. Zero observed deadline misses is required. | Actual frame-count/sample-rate deadlines, raw distributions, maximum duration, wake-up lateness, driver errors, and hardware underrun observations. These margins are proposed release targets, not current measurements. |
| R4: command pressure | Saturation has an explicit outcome. Preserve the last accepted audio state; reject/defer an unaccepted edit without committing mismatched document state. | Full graph/transport/retirement queues, producer stalls, rapid edits, revision acknowledgements, and a reserved or equivalent reliable stop/all-notes-off path. |
| R5: overload policy | Degrade optional analysis/UI work before audio. Protect recording and transport. Never silently change sample rate, buffer size, quality, or pitch. | CPU/storage pressure, low-memory and plugin-timeout injection; explicit state/error reports; bounded silence/ramp policy where recovery is impossible. |
| R6: isolation | Disk analysis, GPU activity, rendering, export, AI jobs, and networking cannot become dependencies of the live audio clock. | Simultaneous worker/GPU/UI saturation, process crashes, hidden/minimized windows, and disconnected optional services. |

Qualification workloads must be versioned fixtures:

- **W0 — correctness:** small impulse, sustained-note, overlapping-note, automation, and streaming projects with independently calculated expected results.
- **W1 — production reference target:** 32 stereo audio tracks, 16 instrument tracks, a declared 64-voice total, 48 representative effect instances, routing/sidechains, automation, and recording once those systems exist. Publish media, presets, block size, and allowed substitutions. Until then, qualify the implemented subset without claiming W1 passes.
- **W2 — scaling:** progressively increase tracks, voices, effects, and disk demand until a reported limit. Large project/track counts must never imply a guaranteed realtime capacity on unspecified hardware.

Initially target 48 kHz/128 frames on each reference desktop system, then qualify 44.1–192 kHz and 64/128/256/512-frame hardware configurations where supported. A 32-frame mode is optional and must earn its own result. Higher buffer sizes do not waive timing correctness.

### 3.3 Duration, failure recovery, and evidence

- 🟠 [Next] Add an automated overnight **8-hour** playback/edit/loop workload and a **48-hour** release soak on declared reference systems; schedule longer endurance runs after the initial matrix is stable.
- 🟠 [Next] Log expected/produced/captured frame counts and transport generations. Zero unexplained missing/duplicated frames, stuck notes, non-finite output, corruption, crashes, or deadlocks is required in a qualifying run.
- 🟠 [Next] Run full-queue, slow-disk, missing-media, unplug/replug, default-device change, unsupported-format, sleep/wake, and interrupted-save scenarios.
- 🟠 [Next] Separate callback duration, scheduler lateness, driver-reported underruns, stream failures, and physical round-trip latency. Unavailable counters must be reported as unknown.
- 🟠 [Next] Publish build/commit, OS, CPU, power mode, device/driver, requested and actual buffer configuration, storage, project hash, seed, load, duration, maxima, quantiles, and raw failure records.
- ⭕️ [Pending] Physical loopback measurements and matched comparisons with other DAWs. Measure input/output offsets, distributions, drift, and sample-rate accuracy; do not infer physical performance from mock tests.
- 🔴 [Later] Maintain a reproducible public comparison suite with at least five repeated runs per comparable configuration and a stated measurement uncertainty. Report limitations and failed runs alongside successes.

## 4. Architecture rules for every phase

- 🟢 [Done] Keep the engine, project, and application layers independent of Tauri.
- 🟠 [Next] Prepared graph publication revisions/receipts are implemented below. Extend acknowledgement/cancellation contracts to general transport and future processor updates; qualify audible-versus-persisted state beyond callback adoption.
- 🟠 [Next] Current graph preparation, validation, state transfer, publication and retirement stages are explicit and cancelable as described below. Extend contracts to future processors/import/analysis, and qualify failure/shutdown paths broadly.
- 🟠 [Next] Preserve a single authoritative Rust document. UI snapshots are projections; windows must not become competing project owners.
- 🔴 [Later] Build a Rust-owned Document/State/Renderer and workspace service shared by all Tauri windows. Add native dock/floating-panel persistence and reconnectable view subscriptions.
- 🔴 [Later] Move heavy timeline/waveform/analysis rendering into Rust/wgpu. Keep textures, geometry, PCM, and frame data in Rust/GPU memory; IPC carries commands and bounded state, not raw frames.
- 🔴 [Later] Use explicit latency, tail, reset, parameter, and state contracts for every processor. Serialize stable IDs rather than memory addresses or UI indexes.
- 🔴 [Later] Prepare dependency-ordered DSP graphs and scratch buffers outside realtime processing. Detect routing cycles before publication; support feedback only through a defined bounded delay.
- 🔴 [Later] Introduce parallel DSP only after the serial reference path is proven. Specify deterministic reduction order, task deadlines, and safe fallback without introducing realtime mutex waits.

## 5. P0 — Playback correctness and stability baseline

This is the immediate engineering priority, ahead of adding a large feature catalogue.

### First implementation batch — 2026-11-05

Evidence and reproducible commands: [P0 validation record](docs/P0_VALIDATION.md). P0 remains open; these results qualify the named software paths only.

- 🟢 [Done] **Reference MIDI ownership:** clip/note identity follows scheduled events into voices. Note Off releases only its owner; deterministic oldest-voice stealing and silent-note handling have regression tests. Invalid identities/ranges are rejected before playback; frequency increments are prepared off the callback.
- 🟢 [Done] **DSP gain ramp:** the reusable `GainNode` reaches its target after a fixed number of samples, independent of callback partition. This does not yet smooth project track faders across graph replacement.
- 🟢 [Done] **Stop delivery:** an atomic generation barrier bypasses a saturated transport queue, prevents older queued Play/record-start requests from restarting playback, and acknowledges Stop after callback processing. New playback requests while Stop is pending are explicitly rejected for retry after acknowledgement.
- 🟢 [Done] **Integer loop arithmetic:** wrap offsets without overflowing before modulo; an independent `u128` oracle covers short loops, prefixes, multiple wraps, and positions near `u64::MAX`. Musical-time conversion still needs its separate oracle.
- 🟢 [Done] **Actual callback budgets:** deadline and CPU-load accounting use actual frame counts; histogram power-of-two boundaries and long-duration coverage are corrected. Driver underruns remain distinct, unmeasured work.
- 🟢 [Done] **Regression evidence:** reproduce five original failures; add exact PCM partition comparisons for reference MIDI and full-engine MIDI/automation/loops, 2,000 stop cycles, and allocation/free checks on overlap/seek/loop/saturated-stop callback paths.
- 🟠 [Next] **Immediate continuation:** complete control revision outcomes; prepared sample-position mixer events are implemented in the fourth batch below. Mixer ramps are implemented in the third batch below. Compatible reference-instrument voice continuity is implemented in the second batch below.
- ⭕️ [Pending] Physical loopback/device-fault qualification and 8-hour/48-hour endurance runs. Short deterministic stress tests do not replace these gates.

### Second implementation batch — graph publication and history

- 🟢 [Done] **Reference-instrument state transfer:** compatible timeline sources match by stable track ID at the callback boundary. Active unchanged notes retain oscillator phase, voice slot, and stealing history while the new schedule and automation remain authoritative.
- 🟢 [Done] **Edit reconciliation:** removed/ended notes stop, newly active or moved/repitched notes chase once, velocity and end changes retain compatible phase, and unchanged stolen notes remain stolen. Non-contiguous seek/loop playback keeps the existing chase policy.
- 🟢 [Done] **History and mixer evidence:** application-level edit/Undo/Redo and reorder tests compare uninterrupted PCM under variable callback sizes and looping. Active gain/pan/mute/solo changes preserve voice progression; a separate graph-publication test checks exact PCM and zero callback allocations/frees.
- 🟠 [Next] **Continuity completion:** arbitrary sample-position parameter events, general processor/effect-tail state contracts, and streaming source/media generations. Mixer publication ramps are implemented below. The default source transfer is deliberately a no-op for unsupported types; this is not blanket graph-state preservation.

### Third implementation batch — mixer ramps and history

- 🟢 [Done] **Mixer transitions:** fixed 5 ms linear interpolation of effective per-channel coefficients for track Gain/Pan/Mute/Solo, plus master Gain. Duration is rounded to the nearest sample (minimum one), independent of callback size; endpoints are exact.
- 🟢 [Done] **Runtime continuity:** stable track-ID publication inherits current level, target, step, and remaining sample count alongside reference MIDI voices. Unchanged targets retain the original deadline; mid-ramp edits and Undo/Redo retarget from the current level. Muted/solo-excluded sources continue advancing. Document/history files store only target settings.
- 🟢 [Done] **Regression evidence:** exact PCM across callback partitions and repeated publications within a ramp; constant-source checks bound control-induced sample jumps; mono/stereo/four-channel endpoint tests span 1/8k/44.1k/48k/96k sample rates. Application history tests compare sustained MIDI against an independent ramp oracle; instrumented active-ramp callbacks allocate/free nothing.
- 🟠 [Next] **Remaining transition scope:** engine-applied revisions for live control, plugin bypass, removed-track/source crossfades, and general effect/stream state contracts. Prepared mixer events are implemented below. Existing pan law, scheduling, and transport discontinuity policy are retained.
- ⭕️ [Pending] Physical listening/loopback and long-session qualification. Deterministic coefficient-continuity tests do not guarantee absence of audible artifacts for arbitrary signals or clipping.

### Fourth implementation batch — sample-position mixer events

- 🟢 [Done] **Prepared event timelines:** persistent Gain/Pan/Mute/Solo events start target ramps at exact integer project samples, including sample zero and callback edges. Four prepared lanes use latest-position/largest-ID resolution, and same-sample Solo evaluation is independent of track order.
- 🟢 [Done] **Chase and history:** loop/seek destinations reevaluate targets; revised schedules remain authoritative while live ramps and reference-MIDI voices continue across publication. Atomic set/remove APIs support Undo/Redo, including future cancellation and past-event edits. Rejected publication leaves history/document state intact.
- 🟢 [Done] **Storage and API:** native Rust/Tauri and WebView adapter editing APIs; preview metadata/history validation; default-empty legacy events. Event-bearing projects save as version 2; event-free projects retain version 1 compatibility. Per-track event limits and strict range/ID validation apply before preparation.
- 🟢 [Done] **Evidence:** exact event/ramp boundaries; callback partition invariance with loops and repeated graph publication; backward-seek chase; same-sample priority; active MIDI/history reference comparison; saturated queue rollback; allocation/free instrumentation; native storage and preview API tests.
- 🟠 [Next] **Delivery/interaction completion:** graph adoption receipts and late-edit reporting are implemented in the fifth batch. Graphical event-lane controls and timestamped external control input remain unfinished. Exact event timing requires the prepared graph to be adopted before its event sample; a late edit chases the current value without retroactive rendering.
- ⭕️ [Pending] Physical loopback/listening, native device matrix, and endurance qualification. See the [event API contract](docs/MIXER_EVENTS.md) and [validation record](docs/P0_VALIDATION.md).

### Fifth implementation batch — adoption revisions and late-edit notices

- 🟢 [Done] **Publication identity:** engine-epoch-scoped monotonic graph IDs, distinct from history document revisions. Accepted/adopted status and coherent receipts expose actual callback adoption; Undo/Redo receipts identify restored document revisions. Rejected updates consume no graph ID and preserve document/history/requested status.
- 🟢 [Done] **Deadline evidence:** automatic earliest-affected mixer-event deadlines include insertions, removals and both positions of a move. The callback evaluates actual post-transport adoption position; equality is on time, active playback past the deadline is late, and paused adoption creates no late warning. Late edits chase current state without stopping playback or rewinding history.
- 🟢 [Done] **Bounded observation:** fixed 64-receipt atomic history, explicit retention/concurrent-overwrite loss indication, persistent late count, JSON-safe ID exhaustion checks, and epoch reset on restart. Retirement backpressure leaves queued graphs unacknowledged until actually adopted; the callback performs no receipt allocation/free or mutex wait.
- 🟢 [Done] **Notification:** native status API, serialized off-audio polling, adopted/accepted status-bar display, and dismissible late/history-loss notices in English/Japanese/Simplified Chinese. Repeated polls do not redisplay dismissed unchanged late results. Narrow-layout warning layering is verified.
- 🟢 [Done] **Regression evidence:** document/history mapping alongside the sustained-MIDI PCM oracle; exact/late/paused/seek boundaries; queue-full and retirement backpressure; restart epochs; coherent concurrent reads and bounded-history overrun. Browser-preview receipt fixtures verify pending-to-adopted display, warning/dismiss/new-warning flow and three languages; these are UI tests, not physical audio measurements.
- 🟠 [Next] **Protocol completion:** proactive deadline admission estimates, cancellation of startup/import/analysis work, general transport/controller acknowledgements, graphical event lanes and native push delivery. Callback adoption does not measure hardware DAC latency.
- ⭕️ [Pending] Physical loopback/listening, hardware/device matrix and endurance qualification. See the [publication contract](docs/GRAPH_PUBLICATION.md).

### Sixth implementation batch — pending graph cancellation and coalescing

- 🟢 [Done] **Latest-wins publication:** application edit/Undo/Redo snapshots supersede older pending replaceable updates. The callback skips obsolete snapshots and adopts at most one live graph, examining at most four entries. Explicit ordered low-level updates remain barriers; rejected submissions supersede nothing.
- 🟢 [Done] **Cancellation ownership:** five fixed atomic cells separate pending/claimed/canceled/superseded states. Cancel and adoption claiming have one winner; claimed graphs cannot be canceled. Every skipped graph is retired outside audio processing, with existing bounded fallback/backpressure and control-thread polling reclamation.
- 🟢 [Done] **Revision outcomes:** receipts identify `adopted`, `canceled` and `superseded`; skipped graphs have no adoption sample and create no late warning. Settlement revisions, cumulative skip counts and pending state distinguish completed cancellation from actual adoption. Receipt cursors now advance by settlement; older IDs are not assumed adopted merely because a newer ID was adopted.
- 🟢 [Done] **Actual-basis deadlines:** prepared stable-ID indexes compare the final snapshot to the graph actually sounding, without callback allocation. Retained changes cannot lose deadlines through skipped parents; Undo/Redo back to the audible schedule causes no ghost deadline. Final retained events keep original project-sample positions and 5 ms ramps.
- 🟢 [Done] **Evidence:** explicit cancellation races, latest-only and ordered-barrier behavior, rejected-newest rollback, retirement stalls, queued set/Undo/Redo/Undo PCM identity, restored-event deadline detection, independent deadline oracles, loop/variable-buffer PCM and zero callback allocations/frees. UI receipt fixtures verify settlement-cursor advancement and absence of canceled-graph late notices.
- 🟠 [Next] **Remaining protocol work:** proactive deadline admission, cancellation of document validation/startup/import/analysis work, broader resource/stress budgets, general transport/controller receipts and native push delivery. The four-entry input bound and explicit queue-full rejection remain intentional.
- ⭕️ [Pending] Physical loopback/listening, device matrix and endurance qualification. See the updated [publication contract](docs/GRAPH_PUBLICATION.md).

### Seventh implementation batch — cancelable graph preparation

- 🟢 [Done] **Desktop preparation worker:** native editing/Undo/Redo commits validated document/history and requests immutable snapshots on one Rust worker, retaining one active and one latest waiting request. New valid work cancels the active token and replaces unstarted waiting work. Initial startup and the synchronous Rust control API retain their existing baseline contract.
- 🟢 [Done] **Cooperative construction:** checkpoints span project track/clip/source stages and MIDI validation, collection, event generation and stable sorting/index construction. Chunk/merge work checks every 256 elements; canceled partial graphs are never published and are destroyed off audio. Standard allocation/container operations remain non-preemptible between checkpoints.
- 🟢 [Done] **Publication fence and retained retry:** latest preparation identity is checked under the publication gate after construction. Completion after cancellation cannot publish. Queue pressure retains the same ready graph for bounded condition-variable retry without rebuilding, while newer requests can still cancel it. Existing pending-graph coalescing and actual-audible-schedule deadline comparison remain authoritative.
- 🟢 [Done] **Identity/history contract:** preparation IDs are distinct from publication IDs and history document IDs. Canceled/failed work consumes no publication ID. Status exposes pending/active/waiting/published work, bounded outcomes with explicit dropped-detail counts and failure. Background failures retain the committed document/history and old audio graph; no asynchronous history rollback occurs. Save writes the committed document even while preparing.
- 🟢 [Done] **UI/lifecycle:** desktop factory enables background mode; stop/load cancels and joins work off audio and restart creates fresh state. Localized compact preparation indication distinguishes document success from audio acceptance. Cancellations are not errors; new preparation failures notify once per ID/epoch. Preview reports no native preparation worker.
- 🟢 [Done] **Evidence:** stable-sort reference/checkpoint cancellation, active/waiting cancellation, publish-after-final-checkpoint fencing, busy queue retry without rebuild, retained deadlines/full reverts with queue coalescing, bounded outcome retention, failure/recovery and shutdown. Background application PCM/history/save/restart oracles and existing callback allocation/free gates pass.
- 🟠 [Next] **Remaining cancellation scope:** document validation/snapshot cloning, initial startup, import/decoder/analysis and future processors; measured overload cancellation budgets and broader fault/resource qualification. Cooperative cancellation is not hard realtime preemption; proactive admission remains unfinished.
- ⭕️ [Pending] Hardware timing/listening, device failures and endurance qualification. See the [preparation contract](docs/GRAPH_PREPARATION.md).

### Remaining P0 work

- 🟠 [Next] **Clock oracle:** integer sample/tick model; independent musical-time conversion reference; exhaustive edge and overflow tests; explicit half-open clip/loop intervals.
- 🟠 [Next] **MIDI lifetime completion:** sustain/controller chase, explicit all-notes-off/panic behavior, and continuity through live edits. Reference-instrument identity, overlap ownership, and deterministic stealing are complete within the scope above.
- 🟠 [Next] **Graph continuity completion:** reference-instrument voices now survive compatible mixer, unrelated-note, reorder, and history publications. Add general processor/effect-tail contracts and stream state transfer; incompatible instruments retain explicit fresh-state behavior.
- 🟠 [Next] **Parameter transitions completion:** timestamped external control, general transport/controller acknowledgements, graphical event lanes, plugin bypass ramps, and broader listening/zipper-noise qualification. Prepared Gain/Pan/Mute/Solo event timelines are implemented below. Track Gain/Pan/Mute/Solo and master Gain publication ramps are complete within the scope below; timeline positions are unchanged.
- 🟠 [Next] **Command protocol completion:** prepared graph revisions, rejection rollback, bounded adoption receipts and late-edit notices are implemented. Pending snapshot cancellation/coalescing is implemented below. Background graph preparation cancellation is implemented below. Add cancellation to future processors/import/analysis, general transport acknowledgements and a complete panic/controller protocol; retain the existing out-of-band Stop barrier.
- 🟠 [Next] **Callback telemetry completion:** finer tail resolution, distinct driver/wake-up/process counters, and off-thread telemetry export. Actual-block budgets and fixed-memory collection are implemented; histogram percentiles remain approximate.
- 🟠 [Next] **Streaming discipline:** generation-tagged seek/loop requests, stale-read rejection, bounded underrun handling, reference-count/drop audit, read-ahead limits, and disk-stall tests.
- 🟠 [Next] **Device negotiation:** validate supported sample rates/channels/buffers; report requested versus actual settings; reject unsupported configurations without replacing the project or silently rebuilding the demo.
- 🟠 [Next] **Device lifecycle:** serialized start/stop/restart, unplug/default-device changes, suspension/resume, audio-thread panic containment where possible, and explicit stopped/recovering/faulted states.
- 🟠 [Next] **Regression infrastructure:** property tests, randomized partitions/seeds, deterministic PCM/event fixtures, queue saturation, long-session soak, and known failing cases promoted to permanent tests.
- 🟠 [Next] **Release hygiene:** remove existing compiler/Clippy warnings, then enforce a warning-clean baseline for maintained targets; lock dependencies and record toolchain changes with benchmark results.
- ⭕️ [Pending] Current-build hardware qualification on macOS, Windows, and Linux with independent loopback capture and an identified audio interface.

**Exit:** W0 passes all applicable gates, queue/failure outcomes are reproducible, live mixer edits do not restart sustained notes, overlap ownership is correct, and the initial hardware reports are published. Missing hardware keeps that platform pending rather than blocking unrelated software verification.

## 6. P1 — Durable projects, recovery, and application state

- 🟢 [Done] Rust Project, atomic editing candidates, saved-revision tracking, Undo/Redo, and version 1 self-contained files.
- 🟠 [Next] **Crash-durable save:** verify temporary-file placement, replacement behavior, parent-directory durability where supported, interruption at every save stage, full disk, permissions, and removable/network volumes. Preserve the previous valid file on failure.
- 🟠 [Next] **Recovery:** debounced autosave, append/checkpoint journal, recovery browser, clear original-versus-recovered naming, retention limits, and user-approved replacement of originals.
- 🟠 [Next] **Resource limits:** byte/track/clip/note/event limits before expensive preparation; nesting/size limits; malformed/truncated file tests; versioned migrations and unknown-version rejection.
- 🟠 [Next] **Background I/O:** bounded progress/cancellation for save/load/decode/analysis, immutable save snapshots, no long UI mutex holds, and no dependency from playback to project-file serialization.
- 🟠 [Next] **History budget:** memory-based as well as entry-based limits, shared immutable resources, transaction labels, explicit coalescing boundaries, and correct dirty state across Undo/Redo/autosave.
- 🔴 [Later] **Media bundles:** content-addressed audio, deduplication, lossless compression, relative references, missing-media relink, portable consolidate/copy, unused-media preview, and safe cleanup.
- 🔴 [Later] **Project tools:** New/Close/templates, rename, sample-rate conversion with explicit quality settings, backups, project alternatives, version history, and session notes.
- 🔴 [Later] **Editable sources:** retained MIDI source windows, reversible trim/reverse/transpose/quantize transforms, source-versus-instance ownership, and copy-on-write edits. Preserve the ability to reopen existing cropped clips.
- 🔴 [Later] **Identity/migration:** persistent asset/track/clip/note/plugin IDs, collision handling, schema migrations, and forward-compatible metadata.

**Exit:** kill/interruption tests recover a valid session; a failed save/load cannot destroy the active or previous saved document; opening historical fixtures reproduces the same event/PCM timeline.

## 7. P2 — Complete audio recording and arrangement workflow

- 🔴 [Later] **Track management:** create/delete/rename/color/icon; mono/stereo audio and instrument tracks; record arm; input/output selection; groups, folder stacks, track alternatives, and master visibility.
- 🔴 [Later] **Audio import:** file picker and drag/drop, background decode, metadata, project media ownership, unsupported-rate conversion, cancel/retry, and missing-file recovery.
- 🔴 [Later] **Formats:** WAV/BWF first; AIFF, FLAC, MP3, CAF, Ogg Vorbis, AAC/ALAC/M4A and ADPCM after decoder/licensing/fixture review. Test actual decoded samples and channel order.
- 🔴 [Later] **Input recording:** input backend contract, capture ring buffers, writer workers, timestamped chunks, channel maps, long-file/RF64 strategy, overflow reporting, and recoverable partial takes.
- 🔴 [Later] **Recording alignment:** measured device input/output latency, software monitoring latency, manual calibration offsets, punch-in/out, pre-roll/count-in, replace/overdub, loop takes, and dropout markers.
- 🔴 [Later] **Monitoring:** input monitoring, direct-monitor coexistence, feedback protection, latency display, and explicit monitoring path/PDC policy.
- 🔴 [Later] **Audio regions:** move/copy/trim/split/join, slip edits, repeat/loop, snapping, marquee ranges, clip gain/envelopes, fades/crossfades, reverse, normalize, silence, transient/silence splitting, and strip silence.
- 🔴 [Later] **Comping:** take folders, swipe comping, crossfade boundaries, alternate comps, flatten-to-copy, and undoable take management.
- 🔴 [Later] **Arrangement navigation:** zoom, independent track heights, rulers in bars/beats/time/samples, markers, colored arrangement sections, selection ranges, play-from-selection/last-locate, and configurable snap modes.
- 🔴 [Later] **Transport production tools:** metronome, count-in, tempo/signature maps, tempo ramps, pickup bars, tempo/event lists, project tuning, varispeed with explicit speed/pitch modes, and tuning-reference controls.
- 🔴 [Later] **MIDI clip completion:** track-to-track drag, repeat/loop, automatic project extension, reversible trim source, tie/sustain-aware splits, and precise multi-clip transforms.
- 🔴 [Later] **Offline render:** shared engine render graph, project/selection/track/stem output, tails, normalization, deterministic seeds, cancellation, and accurate frame counts; WAV 16/24-bit and 32-bit float first.
- 🔴 [Later] **Dither/export:** independently implemented documented dither, sample-rate conversion, clipping/true-peak reports, realtime bounce for hardware paths, and stem alignment manifests.
- ⭕️ [Pending] Multi-input physical recording, measured loopback alignment, monitoring latency, and file recovery after abrupt interruption on each qualified backend.

**Exit:** record a multitrack session, edit/comp it, save/recover it, reopen it, and bounce aligned stems with verified sample positions. No invisible recording loss or playback stall during file operations.

## 8. P3 — MIDI production, built-in sounds, and patches

- 🟢 [Done] Piano-roll editing, multiple note/clip selection, project-time cross-clip edits, reference synthesis, basic quantization, and undoable arrangement commands.
- 🔴 [Later] **MIDI input/output:** port discovery, hotplug, timestamp-to-engine-clock conversion, latency calibration, MIDI keyboard recording, external MIDI tracks, monitoring, program/bank changes, SysEx, and loopback tests.
- 🔴 [Later] **Performance semantics:** sustain/sostenuto, pitch bend, channel pressure, CC state on seek/loop, chase policies, note-off ownership through transforms, and deterministic randomization.
- 🔴 [Later] **Piano roll:** playable keyboard/audition, draw/brush, repeat/flam, legato, humanize, fixed velocity, velocity scaling/limits, equal-pitch/time selection, note colors, and editable CC lanes.
- 🔴 [Later] **Quantization:** grid/triplet/dotted values, swing, strength, Q-range, length/velocity quantization, scale quantization, groove extraction/application, and non-destructive original timing.
- 🔴 [Later] **Event/transform editors:** raw event list, drum/controller lanes, conditions/actions, user transform presets, and undoable batch operations across selected regions.
- 🔴 [Later] **MIDI FX:** ordered bypassable transposer, velocity processor, scale quantizer, chord trigger, note repeater, arpeggiator, randomizer, and controller modulator; sample-correct note ownership and visible processed-note preview.
- 🔴 [Later] **Scripting:** bounded-operation, sandboxed MIDI scripts prepared outside realtime execution; deterministic seeds; scripts cannot allocate unpredictably or perform I/O in the callback.
- 🔴 [Later] **Initial synth:** production polyphonic subtractive synth with oscillator choices, pitch/modulation routing, filter, ADSR, LFO, voice budget, smoothing, denormal handling, and reproducible patch state.
- 🔴 [Later] **Initial sampler:** one-shot/classic playback, trim/loop, root key, velocity, interpolation/resampling quality, envelope/filter, and background sample preparation.
- 🔴 [Later] **Library:** searchable instrument/channel-strip patches, categories, favorites, user-save/delete, missing-content handling, and versioned parameter defaults.
- 🔴 [Later] **Generated players:** deterministic initial drum/bass/keyboard patterns, complexity/loudness, fills, follow-track groove, human-readable seeds, and conversion to editable MIDI.

**Exit:** record a MIDI keyboard performance, edit it across clips, apply transforms/patches, reopen it, and reproduce the same timing and sound under the declared deterministic configuration.

## 9. P4 — Mixer, routing, automation, and essential effects

- 🔴 [Later] **Routing:** aux/bus/output/master strips, pre/post sends, sidechains, summing stacks, channel mapping, multi-output instruments, sends-on-fader, and cycle detection.
- 🔴 [Later] **Mix control:** stereo balance versus true stereo pan, gain staging, pan laws, configurable insert/send slots, VCA, linked controls, attribute-based mix groups, and channel-strip presets.
- 🔴 [Later] **Summing:** specify floating-point precision, reduction order, clipping policy, headroom, and non-finite handling. Evaluate 64-bit accumulation against the serial reference; do not label the current mixer 64-bit without implementation.
- 🔴 [Later] **PDC:** latency contracts for every processor, sidechain/send/bus alignment, dynamic latency changes, bypass transitions, low-latency monitoring mode, and latency readouts.
- 🔴 [Later] **Essential DSP:** parametric EQ and analyzer, compressor, noise gate, limiter, overdrive, utility gain/polarity, sample delay, basic delay, and algorithmic reverb.
- 🔴 [Later] **Automation scope:** track and region lanes; gain, pan, sends, instrument/effect parameters, mute/bypass; stable parameter IDs; sample-position scheduling and latency policy.
- 🔴 [Later] **Automation modes:** Read/Touch/Latch/Write, trim/relative modes, return time, overwrite boundaries, thinning, scaling, copy/paste, and explicit write-arm protection.
- 🔴 [Later] **Curves:** step/linear, exponential, and two-handle Bezier segments with bounded evaluation, editable event lists, snap, and curve preview.
- 🔴 [Later] **Meters:** pre/post peak/RMS, peak hold, clipping, LUFS, true peak, correlation, loudness history, and analyzer decimation outside audio processing.
- 🔴 [Later] **Freeze/bounce in place:** deterministic instrument/FX capture, tail handling, dependency invalidation, reversible unfreeze, and aligned stem export.

**Exit:** mix and automate the W1 reference project with aligned buses/sidechains, verified PDC and bounce results, no audible state reset from unrelated edits, and qualified deadline margins.

## 10. P5 — Plugin hosting and fault containment

- 🔴 [Later] **Host contract:** audio/note/transport/event buses, parameter IDs, latency/tails, bypass, state/chunks, presets, multi-output routing, offline mode, and explicit unsupported-feature reports.
- 🔴 [Later] **Formats:** CLAP first; VST3 and macOS Audio Units after independent SDK/licensing/interop review. Keep format adapters outside the core graph model.
- 🔴 [Later] **Scanner:** separate process, timeout, blacklist/quarantine, fingerprint/cache invalidation, rescan, and architecture/version compatibility checks.
- 🔴 [Later] **Sandboxed processing:** isolated processes, bounded shared-memory buffers, pipeline latency included in PDC, heartbeat, deterministic deadline policy, safe silence/bypass, restart, and recoverable plugin state.
- 🔴 [Later] **Plugin UI:** native embedding/floating windows, focus/keyboard behavior, DPI handling, parameter mapping, automation, and state synchronization across windows.
- 🔴 [Later] **Adversarial fixtures:** crash, deadlock, runaway allocation, non-finite output, latency changes, stale buffers, malformed state, and UI hangs. The audio callback must not wait for a crashed process.
- 🔴 [Later] **Plugin manager:** manufacturer/format/version/latency, trust/quarantine status, sandbox settings, missing-plugin placeholders, and replacement mapping.
- ⭕️ [Pending] Third-party instrument/effect interoperability on each OS/architecture; unavailable commercial plugins remain pending, with open test fixtures used independently.

**Exit:** crash and stall hosted plugins during playback and recording without taking down the engine or corrupting the session. Supported plugin timing and state must pass documented interoperability tests.

## 11. P6 — Advanced sequencing, notation, expression, and controllers

- 🔴 [Later] **Step sequencer:** per-row pitch/instrument, step velocity/gate/chance/repeat/tie, row automation/length, forward/backward/ping-pong/random playback, seeded variations, and conversion to MIDI clips.
- 🔴 [Later] **Score:** staff styles, piano/drum/guitar tablature, rests/beams/ties, lyrics/chords, key/meter changes, score sets, layout, printing/PDF, and MusicXML roundtrip fixtures.
- 🔴 [Later] **MPE/MIDI 2.0:** per-note pitch/pressure/timbre, high-resolution controllers, expression editing, device capability negotiation, and explicit down-conversion rules.
- 🔴 [Later] **Articulations:** stable articulation IDs, key switches/channel/velocity mapping, editor display, recording, and instrument patch persistence.
- 🔴 [Later] **Smart controls:** macro knobs mapped to multiple parameters with ranges/curves, MIDI Learn, conflict detection, and saved mappings.
- 🔴 [Later] **Control surfaces:** Mackie-style transport/mixer support, bidirectional feedback, touch automation, reconnect, controller assignment presets, and latency/feedback-loop tests.
- 🔴 [Later] **Key commands:** searchable/customizable shortcuts, presets, focus-aware dispatch, conflict detection, localization, and a complete command registry that hides/disables unavailable operations.

**Exit:** sequencer, score, expression, and hardware-control edits compile to the same verified engine event representation and survive project reload.

## 12. P7 — Instrument and effect family coverage

Each family needs a stable parameter model, patch migration, latency/tail/reset contract, reference audio fixtures, bounded voice/resource use, and realtime/allocation tests. Algorithms, sounds, presets, and UI assets must be independently created or appropriately licensed.

### Instruments

- 🔴 [Later] **Multisampler:** key/velocity zones, groups, round robin, streaming, envelopes, filters, modulation matrix, key switching, and independently specified SFZ/DLS/EXS import where feasible.
- 🔴 [Later] **Quick sampler:** recording, one-shot/classic/slice modes, transient/beat/equal slicing, root-key/tempo detection, slice-to-MIDI, and reversible source edits.
- 🔴 [Later] **Drum machine:** 16/24/48-pad views, per-pad sampler/synth, mute/solo/choke groups, independent routing/inserts/sends, and kit presets.
- 🔴 [Later] **Acoustic drums:** multi-mic kits, overhead/room/bleed, articulation/velocity layers, round robin, drum replacement, and licensed/generated content provenance.
- 🔴 [Later] **Analog/vector/FM synths:** mono/poly modes, oscillator sync/FM, vector mixing, filters, modulation routing, portamento, unison, and anti-aliasing/oversampling budgets.
- 🔴 [Later] **Hybrid synthesis:** virtual analog plus sample sources initially; later additive, wavetable, granular, spectral sources, morph controls, sequencers/arpeggiators, and indexed patch libraries.
- 🔴 [Later] **Vintage keyboards:** drawbar organ with rotary speaker, clav-style plucked keyboard, electric piano, and tape-keyboard instruments using independent models/content.
- 🔴 [Later] **Physical modeling:** string/material/exciter models, morphing, stability across parameter extremes, and measured CPU bounds.
- 🔴 [Later] **Drum synthesis:** pitched/noise/FM/sample voices with integrated pattern sequencing, tuning, choke and multi-output support.
- 🔴 [Later] **Ensemble/session instruments:** strings, brass, bass/keyboard/drum players, articulation control, groove following, and editable generated MIDI.

### Effects and utilities

- 🔴 [Later] **Dynamics:** compressor character families, sidechain filters, parallel mix, gate/expander, limiter/look-ahead, multiband compressor, de-esser, and transient shaping.
- 🔴 [Later] **EQ/filter:** parametric/channel EQ, linear-phase EQ, graphic/tube/console-inspired EQ, single-band utilities, matching EQ, envelope/LFO filters, and explicit phase/latency behavior.
- 🔴 [Later] **Reverb:** convolution IR loading/streaming, early reflections, room/hall/plate algorithms, decay EQ, modulation, predelay, and tail preservation.
- 🔴 [Later] **Delay:** sample/stereo/tape/multitap delays, tempo sync, per-tap pitch/filter/pan, feedback safety, and seamless time-change policy.
- 🔴 [Later] **Modulation:** chorus, ensemble, flanger, phaser, tremolo, auto-pan, rotary/scanner, ring/frequency modulation, and stereo spreading.
- 🔴 [Later] **Distortion/guitar:** overdrive, clipping/bit reduction/phase distortion, pedalboard routing, amp/cabinet/microphone models, IR cabs, and oversampling-aware PDC.
- 🔴 [Later] **Pitch/voice:** pitch correction/shifting, formant/vocal transformation, harmonization, and documented quality/latency tradeoffs.
- 🔴 [Later] **Imaging/utility:** gain/polarity, channel matrices, stereo direction/width, correlation, spectrum/multimeter, tuner, test oscillator, and loudness/true-peak metering.
- 🔴 [Later] **Special processing:** excitation, sub-bass generation, spectral gating and creative resynthesis with bounded analysis/processing separation.
- 🔴 [Later] **Mastering assistant:** explainable analysis and suggested EQ/dynamics/limiter chains, configurable loudness/true-peak targets, audition/undo, and no unsolicited irreversible processing.

**Exit:** each advertised family has useful independently implemented sounds, documented resource/latency limits, repeatable state, and the same playback gates as the core engine. Plugin/preset count alone is insufficient.

## 13. P8 — Time, pitch, and tempo editing

- 🔴 [Later] **Analysis workers:** transient/onset detection, pitch tracking, confidence estimates, tempo/downbeat estimation, cancel/progress, and versioned cached analysis.
- 🔴 [Later] **Tempo workflows:** keep/adapt/automatic modes, single- and multi-track analysis, editable downbeats/tempo maps, beat mapping, compound meters, and reversible changes.
- 🔴 [Later] **Time stretch:** slicing/rhythmic/monophonic/polyphonic/creative/granular modes and linked speed/pitch; explicit quality, look-ahead, CPU, and latency budgets.
- 🔴 [Later] **Time editing:** transient markers, stretch handles, elastic quantization, groove extraction/following, phase-coherent multitrack edits, and offline high-quality rendering.
- 🔴 [Later] **Pitch editing:** per-note pitch/drift/vibrato/formant/gain, scale snapping, reference tuning, audio-to-MIDI, and confidence-aware suggestions.
- 🔴 [Later] **Validation:** preservation of duration/anchors/channel phase, tempo changes inside blocks, no analysis on the callback, and offline/realtime agreement for the selected quality mode.
- ⭕️ [Pending] Licensed/reference material and listening panels for artifacts across vocals, percussion, sustained instruments, polyphonic mixes, and unusual tuning.

**Exit:** correct a vocal and align phase-related multitrack audio without unexplained timing drift; reopen and bounce the same edit graph reproducibly.

## 14. P9 — Workspace, performance, loops, video, and synchronization

- 🔴 [Later] **Workspace:** Rust-owned panel layout/selection subscriptions, dock/undock/floating panels, saved workspaces, multi-monitor DPI, window restoration, and failure-safe reconnection.
- 🔴 [Later] **wgpu renderer:** native waveform/timeline/piano-roll rendering, visible-range caching, resource limits, GPU loss/recreation, and an accessible WebView control surface. GPU loss must not interrupt audio.
- 🔴 [Later] **Interface quality:** compact icon controls, contextual help, keyboard/screen-reader paths, predictable focus, safe-area handling, meaningful line breaks, Apple Human Interface Guidelines where applicable, platform conventions, and English/Japanese/Simplified Chinese parity.
- 🔴 [Later] **Loop/file browsers:** metadata for tempo/key/style/instrument, preview audition through a defined engine bus, favorites/search, folder browsing, and import tempo/key adaptation. Parsing a format never grants rights to redistribute its content.
- 🔴 [Later] **Live loops:** cells/scenes, quantized launch/stop, one-shot/loop modes, per-cell follow actions, queued-event cancellation, and sample-correct recording of a performance into Arrangement.
- 🔴 [Later] **Performance effects:** bounded X/Y macros for filter/gate/repeat/reverse/bit reduction; parameter smoothing and undoable performance automation.
- 🔴 [Later] **Video:** movie track, thumbnails, floating playback window, offset/frame-rate handling, frame-accurate navigation, audio-to-movie export, and explicit video-versus-audio clock policy.
- 🔴 [Later] **Synchronization:** MTC/MMC, MIDI clock, SMPTE/timecode displays, external clock lock/relock, drift reporting, and a deliberate master-clock policy.
- 🔴 [Later] **Multiple devices:** qualify input/output clock differences; explicit drift compensation/asynchronous SRC where needed; no assumption that nominally identical sample rates share a physical clock.
- ⭕️ [Pending] Control-surface, external synth, video/timecode, aggregate-device, and multi-display testing on actual hardware.

**Exit:** perform and record loop launches, automate performance controls, move/floating panels, and run video/controller workloads without degrading the qualified audio path.

## 15. P10 — Delivery, interchange, platforms, and release qualification

- 🔴 [Later] **Audio delivery:** project/selection/region/track/stem bounce, in-place bounce, tail policies, loudness reports, metadata/BWF timestamps, bit depths, dither, and lossless/encoded formats with codec verification.
- 🔴 [Later] **Interchange:** Standard MIDI Files, MusicXML, AAF and other documented interchange formats with tempo, position, offset, channel-map, and roundtrip fixtures.
- 🔴 [Later] **Project compatibility:** investigate `.logicx` and other formats using independent, legally reviewable specifications/fixtures. Never depend on copied donor seeds, reference code, or proprietary sample libraries. Report unsupported data rather than silently approximating it.
- 🔴 [Later] **Spatial delivery:** explicit multichannel bus model, surround/binaural monitoring, object metadata and ADM BWF after basic stereo stability/PDC gates pass.
- 🔴 [Later] **macOS:** current Apple Silicon/Intel build policy, CoreAudio qualification, device permissions, signing/notarization, native menu/dialog/focus tests, and sleep/wake handling.
- 🔴 [Later] **Windows:** WASAPI shared/exclusive and ASIO strategy, actual driver/buffer negotiation, signed installers, architecture policy, DPI/audio-device lifecycle tests, and loopback measurements.
- 🔴 [Later] **Linux:** ALSA baseline; independently qualified PipeWire/JACK paths; Wayland/X11 and GPU fallback; packages/install/uninstall; realtime permissions and actual driver behavior.
- 🔴 [Later] **Mobile:** separate iOS/Android input/output backend qualification, permissions, interruption/background policy, touch/safe areas, thermal limits, and explicit reduced workload profiles.
- 🔴 [Later] **Browser/WASM audio:** optional engine adaptation, AudioWorklet/shared-memory constraints, persistence and permission policies, reduced workload profiles, and independent latency/reliability reports. The current browser preview is not this product.
- 🔴 [Later] **Release pipeline:** reproducible build metadata, signed artifacts, dependency/license review, upgrade/rollback and project migration tests, crash diagnostics, user-controlled reporting, and published supported hardware configurations.
- ⭕️ [Pending] Every platform/driver/architecture without current hardware evidence; commercial plugin fixtures; signing credentials; external interchange fixtures and format conformance tools.

**Exit:** a release candidate passes its declared software gates and physical matrix; every unsupported or unqualified configuration is documented. Shipping on one OS does not mark other platforms qualified.

## 16. P11 — Optional services and longer-term extensions

These features must never be prerequisites for local playback, recording, project opening, or recovery.

- 🔴 [Later] **Remote control:** authenticated local/web remote, rate-limited commands, reconnectable state projections, user-controlled network exposure, and no remote ownership of the audio clock.
- 🔴 [Later] **Collaboration:** versioned document operations, conflict handling, media exchange, offline editing, provenance/history, and explicit separation between collaborative document convergence and live audio scheduling.
- 🔴 [Later] **AI/analysis:** optional stem separation, suggested mastering, assisted performance generation, and model/job cancellation. Require explicit media-upload consent, independent processing workers, and local fallback where feasible.
- 🔴 [Later] **Hosted architecture if needed:** XServer VPS Cloud for application/API, managed PostgreSQL, and NFS; independent AI/batch/crawler workers on ordinary VPS instances. Workers communicate through HTTPS APIs or a job queue, without depending on direct access to the managed database.
- 🔴 [Later] **Content ecosystem:** user patches, instruments, IR libraries, provenance/licensing, sandboxed extensions, and versioned package rollback.
- 🔴 [Later] **Advanced editing:** richer notation, spatial workflows, modular MIDI routing, performance-oriented stage views, and additional accessible/mobile interaction modes.

## 17. Work selection, verification, and release records

For every implemented item, record:

1. The user-visible workflow, parameter ranges, units, rounding, ownership, and discontinuity semantics.
2. Changed document/schema/command contracts and migration/backward-compatibility behavior.
3. Realtime effects: work/capacity budget, allocations, resource ownership, latency, tails, and overload policy.
4. Tests with expected sample/event results, failure injection, Undo/Redo, save/load, and UI/native evidence where applicable.
5. A measured workload result or an explicit pending qualification entry.
6. Documentation/localization updates and reproducible commands/artifact locations.

Current verification commands:

```sh
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets
npm run test:midi
npm run test:mixer
npm run build
```

Future CI should add the timing oracle/property suite, realtime instrumentation, offline/null fixtures, fuzz/migration tests, performance comparisons against versioned baselines, endurance runs, and native platform jobs. Do not list an absent script as an executable current check.

**Release vetoes:** unexplained timing error, stuck notes, callback allocation/free or blocking, unbounded overload, silent command loss, recording loss, project corruption, or a reproducible playback discontinuity introduced by an unrelated edit. A new feature remains unfinished until its applicable gates pass; unavailable physical tests remain pending.
