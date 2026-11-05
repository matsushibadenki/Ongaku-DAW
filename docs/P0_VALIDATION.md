# P0 playback validation — first implementation batch

Date: 2026-11-05. Scope: independently implemented Rust playback fixes. No Pro-Wave source or assets were used. The [roadmap](../ROADMAP.md#5-p0--playback-correctness-and-stability-baseline) remains authoritative; P0 is not complete.

## Priority audit and changes

Five focused regression tests failed against the previous implementation before fixes:

| Defect | Implemented behavior | Evidence |
| --- | --- | --- |
| An earlier same-key/channel Note Off silenced another overlapping note. | Scheduled events retain clip/note identity; only the owning voice is released. Full polyphony steals the oldest voice deterministically; its later Note Off cannot release its replacement. Zero-velocity notes do not steal audible voices. | Overlapping clips with matching local note IDs; 17-note stealing and silent-note cases. |
| Gain smoothing finished early with small callbacks. | The reusable DSP `GainNode` counts remaining samples across callbacks and reaches the exact target, including small target edits. | Identical ramps across partitions; 240-sample endpoint at 48 kHz. This is not yet track-fader smoothing across graph rebuilds. |
| A full command queue rejected Stop; queued Play could restart playback. | Stop uses an atomic generation barrier outside the ordinary queue. Older Play/record-start commands cannot cross it. Other queued commands, including tempo/meter/seek, are retained. | Queue saturation, pending-stop rejection, post-acknowledgement restart, and 2,000 repeated-stop cycles. |
| Loop offsets saturated before modulo near the integer limit. | Compute distance to the loop end before wrapping, avoiding overflowing the addition. | Independent `u128` sample-position oracle, including prefix playback, short loops, multiple wraps, and near-`u64::MAX` positions. |
| Callback deadlines assumed the configured block size. | Use actual frames/sample rate for each callback budget and aggregate CPU load. Correct histogram bucket boundaries; exclude empty callbacks. | 64-frame callback with a 128-frame configuration; mixed 64/256-frame budgets; exact powers of two and long durations. |

MIDI preparation also rejects duplicate clip IDs, duplicate note IDs within a clip, invalid note parameters/ranges, and overflowing clip bounds before publication. Frequency increments are prepared in a 128-entry table outside the callback, removing repeated frequency exponentiation from the sample loop. The reference instrument remains limited to 16 voices.

## Contracts and realtime effects

- Public MIDI event DTOs and `.ongaku` version 1 are unchanged. Existing clip/note IDs supply voice identity; Undo/Redo and save/load retain the existing project contract.
- Stop takes effect at a callback boundary and is acknowledged after callback processing. Play and record-start submitted while Stop is pending return an explicit error; callers must retry after acknowledgement. UI retry orchestration and general graph/edit revision acknowledgements are still unfinished. Stop does not interrupt a buffer already being rendered.
- Ordinary commands still use a bounded queue and can return queue-full errors. This is not a complete panic/controller protocol.
- Identity metadata, voice storage, gain state, frequency tables, and telemetry storage are prepared or fixed-size. Instrumented callback tests observe zero allocations **and** frees during overlapping MIDI, seek/loop, and saturated-stop processing, supplementing existing graph-swap and streaming checks.
- Timing telemetry measures processing duration against the actual callback period. P99.9 is a power-of-two histogram estimate, not a precise tail distribution. The xrun counter is not a driver-underrun or wake-up-delay measurement.

## Reproducible software checks

From the repository root:

```sh
cargo fmt --all --check
cargo test --workspace --offline
cargo test --workspace --release --offline
cargo clippy --workspace --all-targets --offline
```

🟢 [Done] Workspace Debug and Release tests pass with the environment's Homebrew Rust 1.99.1. The complete Debug workspace suite also passes with the pinned Rust 1.93.0 using `rustup run 1.93.0 cargo test --workspace --offline`. Clippy completes with existing warnings in unrelated oscillator-test and project-edit code; the warning-clean baseline remains unfinished.

🟢 [Done] Reference-instrument PCM is bit-identical across single-frame, 2,048-frame, and mixed 17/64/127/128/257/512/1,024-frame callbacks, with and without looping. Full-engine MIDI plus automation uses the same partition comparison and checks the final transport position. These fixtures establish partition invariance for the named paths, not every future source or graph update.

🟢 [Done] Transport oracle compares every sample and final advancement across 4,096 frames, loop lengths 1/2/17/130, prefix/start/end positions, normal and near-limit timelines, and fixed/mixed callback partitions. It does not validate musical-time floating-point conversion.

The initial five failing cases were recorded before implementation; permanent regression tests live beside `daw-midi`, `daw-dsp`, `daw-core`, and `audio-engine`, and run with the workspace suite. No frontend behavior changed in this batch.

## Remaining qualification and next work

- 🟠 [Next] **Graph continuity completion:** reference MIDI state transfer is implemented below. General processor/effect tails and streaming state still require explicit contracts; mixer publication ramps are implemented in the third batch below.
- 🟠 [Next] **Timing and lifetime completion:** independent musical-time conversion oracle; sustain/controller chase; deliberate seek/loop phase policy; explicit panic/all-notes-off behavior; bounded dense-event work and broader randomized fixtures.
- 🟠 [Next] **Control and parameters:** general transport/controller revisions, superseded preparation cancellation, UI retry behavior and plugin bypass ramps. Graph revision receipts, rollback and prepared mixer events are implemented below. Track mute/solo ramps are implemented below.
- 🟠 [Next] **Streaming/device failures:** generation-tagged read requests, stale-data rejection, disk-stall tests, device negotiation/recovery, and separate process/wake-up/driver counters.
- ⭕️ [Pending] Physical loopback, driver underruns, unplug/sleep/restart tests, measured latency, and current macOS/Windows/Linux hardware qualification.
- ⭕️ [Pending] 8-hour and 48-hour endurance runs. The 2,000-cycle deterministic test is not a soak qualification or a performance comparison with another DAW.

## Second batch — graph publication and Undo/Redo

🟢 [Done] Prepared timeline sources now expose an opt-in realtime state-inheritance contract. At publication the mixer matches stable track IDs, independent of track order, and the reference synth accepts only the same source type and audio configuration at a contiguous timeline position. Unknown source types and pull streams do not inherit state implicitly. The old graph continues through the existing off-callback retirement path.

The reference synth copies its fixed 16-voice runtime state while retaining the new schedule and automation. A prepared identity index supports note lookup without callback allocation. Compatible notes retain phase, original voice slot and start time; preserving slots also preserves floating-point summation order and subsequent free-slot choice. Deleted/ended notes release, velocity changes update the live voice, and end edits retain phase while honoring the new Note Off. Start/pitch/channel changes and newly active notes chase with fresh phase. Unchanged stolen notes do not return just because a graph was published. Undo restores the document and reconciles against the **current** runtime state; it never rewinds playback or restores a historical oscillator snapshot.

Seek/loop discontinuities retain the existing chase policy. Automation is evaluated from the new lane at the current sample, rather than copying stale lane values. Muted/solo-excluded timeline sources continue rendering, so unmuting does not restart their phase. Gain, pan, mute and solo changes take effect at publication; their audible level transitions are now smoothed by the third batch below.

### Added regression coverage

- 🟢 [Done] Exact PCM comparison of repeated prepared graph publications against uninterrupted playback for callback sizes 1/17/127/128/257/512, including instrument-level automation. Instrumented publication/render observes zero allocations and frees; preparation and reclamation occur outside instrumentation.
- 🟢 [Done] Application edit/Undo/Redo tests use the real candidate-publication path, a deterministic processor, variable callback sizes and a short loop. Unrelated gain edits, track reorder, and future-note insertion preserve exact PCM. Active instrument-track gain/pan/mute/solo and history changes match the continuously advancing reference with the expected mixer gains (absolute tolerance `1e-6`).
- 🟢 [Done] Synth tests cover duration/velocity changes, repitch, deletion/restoration, non-contiguous publication, and polyphony with both stolen voices and empty slots; unchanged synthesis after transfer is bit-identical.
- 🟠 [Next] Transfer cost scales with prepared track/note counts (track matching and schedule reconciliation); enforce resource/event-density budgets and expand stress fixtures before production qualification. The callback performs no allocation, locking, source ownership replacement or graph destruction during transfer.
- ⭕️ [Pending] Physical listening/loopback and long-session tests remain pending. These deterministic software fixtures do not establish click-free arbitrary parameter changes, effect-tail preservation, or hardware qualification.

No project schema, file format, UI text, or history format changed. Existing save/load and editing tests remain in the workspace suite. Loading a different document still stops/recreates the engine, and deliberately does not inherit voices from the previous document. No Pro-Wave code was read or reused.

Second-batch verification: `rustup run 1.93.0 cargo test --workspace --offline` and `rustup run 1.93.0 cargo test --workspace --release --offline` each pass **75 tests**. Pinned-toolchain Clippy (`rustup run 1.93.0 cargo clippy --workspace --all-targets --offline`), formatting, and documentation-link checks pass. Native hardware and endurance qualification remain pending as listed above.

## Third batch — Gain, Pan, Mute and Solo ramps

🟢 [Done] Track controls now interpolate their **effective channel coefficients** linearly over 5 ms (`round(sample_rate / 200)`, minimum one sample). Mute or Solo exclusion sets the target to zero; Pan retains the existing linear balance law, and channels beyond stereo keep the track gain. The low-level graph master Gain uses the same ramp contract; this adds no new master-fader editing command. Advancing occurs once per rendered frame, before applying that frame's coefficient; the final frame is assigned the exact target. Scheduling and transport positions are unchanged.

🟢 [Done] Prepared graph publication copies fixed-size ramp state by stable track ID, independently of source type, together with the existing compatible reference-MIDI state transfer. Unchanged targets do not restart ramps. Changed targets, including Undo/Redo and rapid reversal, start from the current audible coefficient and reach the new target after a fresh 5 ms interval. History restores target settings, never an old runtime cursor. Muted/solo-excluded sources still advance. While transport is stopped, rendering returns silence without advancing ramps; the pending target is applied on resumed rendering. Initial graph rendering adopts configured levels directly; newly inserted tracks in an existing compatible graph fade in from zero.

No heap storage, locking, graph destruction, or ownership transfer is added to the callback. Ramp storage is fixed per track and master, and the per-frame work is fixed per track/channel. PCM clips, pull sources and timeline sources use the same coefficients. Project format/version and command DTOs are unchanged; persistence continues to save target Gain/Pan/Mute/Solo settings only.

### Regression scope

- 🟢 [Done] Exact PCM equality for single-frame and mixed 17/64/127/128/257/512-frame callbacks versus large blocks, both with direct controls and with graph publication after every block, including publications that do not change a running ramp's target.
- 🟢 [Done] Constant sources isolate control-induced discontinuities: rapid Gain/Pan/Mute/Solo and master Gain retargets have bounded adjacent-frame differences. Mute reaches exact zero at the specified endpoint across mono/stereo/four-channel formats and sample rates 1/8,000/44,100/48,000/96,000 Hz.
- 🟢 [Done] Retarget tests preserve current level, keep an unchanged target's deadline, and check exact endpoints. Application-level Undo/Redo tests compare sustained reference MIDI to an independently calculated closed-form ramp, including rapid reversals, mute/unmute and Solo exclusions. This also detects oscillator phase resets.
- 🟢 [Done] Repeated active MIDI/mixer graph publications with rapid control changes observe zero allocations and frees inside the callback. Existing voice continuity, loop, note lifetime, stop-saturation and save/load tests remain in the workspace suite.
- 🟠 [Next] Arbitrary sample-position parameter event delivery, plugin bypass, track/source removal crossfades, and processor/effect/stream continuity remain unfinished. Updates currently begin at graph-publication/render boundaries; a ramp does not imply timestamped command delivery or smooth transport resets.
- ⭕️ [Pending] Physical listening/loopback, device matrix, and endurance tests. Linear ramps remove abrupt coefficient steps in these fixtures; they do not prevent clipping from high summing levels or certify all signals as inaudible transitions.

No Pro-Wave code was read or reused.

For this batch, put the pinned toolchain's binaries first on `PATH` so Cargo subcommands and their compiler drivers use the same toolchain even when Homebrew binaries are installed:

```sh
toolchain_bin="$(dirname "$(rustup which --toolchain 1.93.0 cargo)")"
export PATH="$toolchain_bin:$PATH"
rustc --version
cargo clippy --version
cargo test --workspace --offline
cargo test --workspace --release --offline
cargo clippy --workspace --all-targets --offline
```

Third-batch software verification: the Debug and Release workspace suites pass **79 tests**. Formatting, local documentation links and whitespace checks pass. Clippy completes with pre-existing project/MIDI-test warnings; no new mixer or application warnings remain. Hardware and endurance qualification stay pending.

## Fourth batch — sample-position mixer event timelines

🟢 [Done] Prepared Gain/Pan/Mute/Solo event lanes are evaluated at each rendered project sample before coefficient ramps advance. Exact boundary tests include 0/1/127/128/129/511; repeated targets preserve the original ramp deadline. Same-sample priority uses stable event IDs, and global Solo is evaluated before mixing any track. Definitions, limits, lateness and seek/loop semantics are specified in the [event API contract](MIXER_EVENTS.md).

🟢 [Done] Events use the existing atomic project-edit/history/publication path. Application tests insert, undo, redo, move and remove events before/after their scheduled samples while a sustained MIDI reference continues. Output matches an independent closed-form ramp oracle (`1e-6` absolute tolerance). Full publication queues reject edits and Undo without changing project, revision or history length.

🟢 [Done] Constant-source PCM is identical across large, single-frame and mixed 17/64/127/128/257/512-frame callbacks, with/without loops and repeated graph publication. Backward-seek tests reject stale Mute state. Existing instrumented graph-publication tests now execute all four scheduled parameter types inside callbacks and still observe zero allocations/frees.

🟢 [Done] Native storage tests roundtrip all parameter types as version 2, read legacy version 1 with omitted event fields, and reject invalid positions. Invalid edits, non-finite values, duplicate IDs and oversized event lists are rejected. Preview API tests cover validation, history and local storage; existing MIDI preview tests and the production frontend build also pass. Preview metadata tests do not qualify audio timing.

🟠 [Next] Graphical event lanes, external timestamped controller input, processor bypass and general stream/effect continuity remain unfinished. Graph adoption acknowledgements and deadline reporting are implemented in the fifth batch below. Event timing is exact after graph adoption; late publication cannot modify audio already rendered. Base faders remain base values and active timeline events override them per parameter.

⭕️ [Pending] Current-build physical timing/listening and endurance qualification remain pending. No Pro-Wave code was read or reused. The system Homebrew Node executable had a missing `simdjson` dependency; frontend checks use the already-installed Node 22.17.1 runtime instead, without modifying the system installation.

Fourth-batch verification (Rust 1.93.0 binaries first on `PATH`): `cargo test --workspace --offline` and `cargo test --workspace --release --offline` each pass **87 tests**. Clippy completes with the existing project/MIDI-test warnings; no new event, mixer, application or command warnings remain. With Node 22.17.1, `npm run test:mixer`, `npm run test:midi` and `npm run build` pass. Formatting, local documentation links and whitespace checks pass.

## Fifth batch — adoption receipts and late-edit notification

🟢 [Done] Graph acceptance now returns a monotonic publication ID and actual callback adoption writes a coherent receipt with document revision, adoption position, optional deadline and playback state. History document revisions may decrease on Undo while publication IDs continue increasing. Stop/restart uses a new engine epoch; old pending tokens are invalid across epochs.

🟢 [Done] Mixer schedule diffs supply the earliest inserted/removed/changed position; moves include the old position. Tests independently expect the affected sample and receipt metadata throughout the existing sustained-MIDI/history PCM oracle. Queue-full edits and Undo preserve document, revision, history and requested publication. Unrelated edits do not re-report old event deadlines.

🟢 [Done] Exact deadline equality, one-sample lateness, same-callback seek-before-adoption, paused/Stop priority, ID exhaustion and retired-graph backpressure have dedicated tests. Concurrent reader stress validates coherent fields across 10,000 writes and 64-record retention overrun. The existing instrumented graph-swap paths exercise receipt writes without callback allocations/frees.

🟢 [Done] Local UI QA uses a receipt fixture, not a fabricated native acknowledgement: pending adopted/accepted numbers update, new late counts notify, Dismiss remains dismissed on repeated polls, and a new late count notifies again. English/Japanese/Simplified Chinese notices and 1440×1000 / 390×844 layouts were checked; narrow notices were raised above the existing overlay browser panel so the Dismiss button stays accessible. Page identity, nonblank content, absence of framework overlays, and clean page/console error logs were checked at `http://127.0.0.1:5175/`.

Browser availability: the required Browser skill is not provided, so regular Playwright was used (`Browser plugin not available` workflow). Playwright 1.62.1 used an already installed Chromium headless binary with a temporary profile; the existing project dev server was reused and left running. UI fixtures do not qualify native IPC/device behavior or physical playback latency. Screenshots and temporary QA scripts are outside the repository in `/tmp/ongaku-adoption-*`.

🟠 [Next] Proactive deadline admission, superseded preparation cancellation, general transport/controller receipts, graphical event lanes and native push notifications remain unfinished. ⭕️ [Pending] Physical DAC/loopback timing, listening, device failures and endurance qualification remain pending. Detailed semantics are in the [graph publication contract](GRAPH_PUBLICATION.md). No Pro-Wave code was read or reused.

Fifth-batch verification: Rust 1.93.0 Debug and Release workspace suites each pass **93 tests**; workspace Clippy completes with existing project/MIDI-test warnings only. `npm run test:mixer` (including preview/no-false-adoption checks), `npm run test:midi`, and `npm run build` pass with Node 22.17.1. Formatting and whitespace checks pass. Software adoption receipts do not certify DAC timing; physical qualification remains pending.

## Sixth batch — pending snapshot cancellation and coalescing

🟢 [Done] Application edits/Undo/Redo use latest-wins prepared snapshot publication. Superseded or explicitly canceled pending graphs are skipped, retained until off-callback retirement, and never reported adopted. Ordered low-level publication remains available and is not implicitly superseded. Successful queue insertion precedes superseding; failed newest submissions leave accepted document/history state intact.

🟢 [Done] Fixed atomic claim/cancel cells prevent cancellation of an in-flight adoption. The callback examines at most four entries, adopts at most one, and stops safely on retirement fallback pressure. Native status polling reclaims retired graphs on the control thread so old-resource backpressure can clear without additional edits.

🟢 [Done] Terminal receipts include outcome, nullable adoption sample and resolution sample; settlement is distinct from adoption. Mixed adopted/canceled/superseded receipt stress runs 10,000 writes with coherent-field and watermark invariants. Cancellation/superseding does not increment late counts. UI polling advances by settlement instead of repeatedly reading canceled latest IDs.

🟢 [Done] The final prepared mixer schedule is compared to the actual audible graph with off-thread-prepared ID indexes and an allocation-free merge. Independent document-map diff tests cover inserts, removal, moves, parameter changes, cross-track IDs and reorder. A final unrelated edit retaining a skipped parent's past event still reports the deadline. Queued set→Undo→Redo→Undo returning to the audible document causes no ghost late warning and preserves exact sustained-MIDI PCM. A later Redo plus unrelated edit correctly restores the past deadline and matches the independent gain-ramp reference.

🟢 [Done] Cancellation/coalescing callbacks retain zero allocations and frees while variable buffers and repeated short-loop wraps match uninterrupted MIDI/automation PCM. Tests also cover ordered barriers, queue-full rollback, explicit cancellation/claim races and retirement stalls. Project formats and persistent history semantics are unchanged.

UI validation flow: app at `http://127.0.0.1:5175/` → engine-view receipt fixture with adopted revision 2 and settled/accepted revision 5 → next query cursor 5, no canceled-graph late warning → adopted late revision 6 → one warning → Dismiss remains dismissed. Playwright checks page identity, rendered content, console/page errors and screenshots at 1440×1000 and 390×844. Browser skill unavailable (`Browser plugin not available` fallback); existing project server reused. Fixture scripts/screenshots remain outside the repository in `/tmp/ongaku-coalescing-*`. This is UI evidence, not native hardware latency qualification.

🟠 [Next] Proactive admission estimates and cancellation of expensive preparation before enqueueing remain unfinished; queue capacity is still four and an unconsumed fifth submission can fail explicitly. ⭕️ [Pending] Native hardware timing/listening, device failures and endurance tests remain pending. No Pro-Wave code was read or reused.

Sixth-batch verification: Rust 1.93.0 Debug and Release workspace suites each pass **102 tests**. The allocation/free fixture was additionally strengthened with nonempty future mixer-event indexes and rerun in Debug/Release. Clippy completes with existing project/MIDI-test warnings only. Node 22.17.1 `npm run test:mixer`, `npm run test:midi` and `npm run build` pass. Formatting, documentation links and whitespace checks pass. UI receipts are deterministic browser fixtures; hardware qualification remains pending.

## Seventh batch — cancellation before graph publication

🟢 [Done] The Tauri desktop factory now enables a bounded graph-preparation worker. Valid document edits, Undo and Redo request immutable snapshots without holding the application mutex through construction. One active job and one newest waiting job are retained; replaced/unneeded work is canceled. Synchronous Rust APIs and initial startup remain compatible. The changed desktop success/failure contract and all ID domains are documented in [graph preparation](GRAPH_PREPARATION.md).

🟢 [Done] Cooperative checkpoints cover track/clip/source construction, MIDI validation, schedule collection, event generation and ownership indexes. Stable 256-element chunk sorts/checkpointed merge passes match an independent serial stable-sort reference; dense schedule tests stop during collection/sorting, and partial project preparation leaves the document unchanged.

🟢 [Done] A post-construction latest-request publication fence rejects completion after cancellation, even when a fixture deliberately returns a graph after its last checkpoint. A recoverable graph-submission API retains ownership on transient queue pressure; retries wait on a condition variable and never rebuild the same ready graph. New requests cancel ready-waiting work. Canceled/failed preparations produce no publication ID or adoption receipt.

🟢 [Done] Worker tests cover active/pending replacement, publication backpressure, actual-audible deadlines and queue coalescing, full reverts without ghost deadlines, failure/recovery, shutdown join and bounded 64-outcome retention (70 requested snapshots, only active/latest built, explicit dropped-detail count). Background app tests hold work while real processor/reference PCM continues, queue edit→Undo→Redo→Undo, reject an invalid edit without canceling valid work, save/load the committed document, and verify no stale publication, phase reset or dirty-state drift. Desktop-constructor/restart tests distinguish preparation, queue acceptance and actual adoption.

UI flow: `http://127.0.0.1:5175/` → preparation fixture pending with unchanged accepted/adopted publication → localized pending indicator → preparation published but not adopted → actual adoption → one preparation failure notice → Dismiss remains dismissed → stop clears status. Page identity, nonblank content, absent framework overlay, clean page/console logs and status visibility were checked with Playwright at 1440×1000 and 390×844 in English/Japanese/Simplified Chinese. Browser skill is unavailable (`Browser plugin not available` fallback). Existing server reused; temporary scripts/screenshots are outside the repository in `/tmp/ongaku-preparation-*`. These UI fixtures are not native device evidence.

🟠 [Next] Document validation/cloning, startup, import/decoder/analysis jobs and future processors need separate cancellation contracts. Allocation/container operations can complete before the next checkpoint. Proactive deadline admission and measured overload cancellation latency remain unfinished. ⭕️ [Pending] Physical timing/listening, device/failure matrix and endurance tests remain pending. No Pro-Wave source was read or reused.

Seventh-batch verification: Rust 1.93.0 Debug and Release workspace suites each pass **114 tests**. Existing callback allocation/free, MIDI/automation/loop/partition and adoption-revision gates remain passing. Clippy reports existing project/MIDI-test warnings only. Node 22.17.1 mixer/MIDI API tests and production frontend build pass; formatting, documentation links and whitespace checks pass.
