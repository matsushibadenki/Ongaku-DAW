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

## Eighth batch — streaming handoff and frame integrity

Date: 2026-11-06. The streaming foundation is exercised independently of native project playback, which currently uses in-memory PCM. No Pro-Wave source or assets were read or reused.

🟢 [Done] Two regression tests first reproduced stale audio after accepted seek and split stereo frame delivery against the previous implementation. Accepted requests now invalidate old data before decoder acknowledgement. Requested/applied generations, queued seek coalescing and stale in-flight decode rejection preserve the handoff. Queue rejection does not advance the accepted generation.

🟢 [Done] Complete-frame consumption, short-read carry, channel resynchronization, newer-generation lookahead and bounded stale draining prevent channel shifts. Aligned read-ahead is capped at two seconds. Consumer buffers are prepared off audio; allocation/free instrumentation includes stale stereo handoff. EOF is generation-tagged; seek waits, starvation and terminal decoder errors are separately observable. Invalid read counts, truncated final frames and failed seeks fault safely.

🟢 [Done] Rust 1.93.0 Debug and Release workspace suites pass **125 tests** each, including **14 streaming tests**. The PCM oracle covers variable reader/callback partitions; a gated decoder proves superseded reads cannot replay. Queue bounds, coalescing, rejection, generation exhaustion, fault visibility and known-EOF behavior pass. `cargo fmt --all --check` and `cargo clippy --workspace --all-targets --offline` complete; existing project/MIDI warnings remain, with no streaming warnings.

Reproduce using the installed pinned toolchain:

```sh
toolchain_bin="$(dirname "$(rustup which --toolchain 1.93.0 cargo)")"
export PATH="$toolchain_bin:$PATH"
cargo fmt --all --check
cargo test --workspace --offline
cargo test --workspace --release --offline
cargo clippy --workspace --all-targets --offline
```

🟠 [Next] Project/loop/graph streaming integration, timeline/readiness guarantees, multi-stream budgets, decoder cancellation and recovery remain open. Controller shutdown joins off audio but cannot interrupt blocking reader I/O. ⭕️ [Pending] Physical storage/device latency, loopback/listening and endurance qualification remain untested. See the [streaming contract](STREAMING_VALIDATION.md); these software tests do not close the P0 release gates.

## Ninth batch — native project streaming and timeline continuity

Date: 2026-11-06. Native project audio now uses immutable private backing files and shared position-addressed caches. The sequential streaming API remains separate. No Pro-Wave source or assets were read or reused.

🟢 [Done] Initial graph preparation spools embedded PCM off audio and warms a bounded cache. History/document clones and prepared graphs retain the same media identity, worker and ready pages. Seek/loop positions map to exact source frames inside callback chunks; half-open clip boundaries and source offsets are explicit. Old disk work may finish, but its page address cannot be reused as a sequential cursor at a new position. The latest pending missing-position demand supersedes older ones and takes priority over bounded prefetch hints.

🟢 [Done] Non-wrapping odd/even page versions and atomic sample bits prevent mixed stereo frames during eviction, including same-address ABA. Readers never spin or wait: a missing/concurrently replaced frame is zeroed and counted. Loop-head hints and forward lookahead remain nonblocking. Worker I/O/count/EOF faults and panics become observable terminal failures. Native publication status carries current-document streaming counters; diagnostics do not wait for preparation locks. Generic cache pages, payload duration and mailboxes have fixed limits.

🟢 [Done] Independent direct-PCM graph oracles cover timeline/clip edges, seeks, short loops, callback partitions, graph updates and edited/restored mixer targets. Actual application Undo/Redo, reorder, pending graph coalescing and adoption receipts preserve warm-cache page counts. Audible Gain/Pan/Mute history matches direct PCM and existing ramp behavior. Save/load creates fresh media identities with identical output; runtime state is not serialized. A cold project seek returns silence or the exact requested frame and recovers at the current source position. A blocked old read followed by new demands, concurrent eviction, short decoder reads, terminal storage faults and canceled preparation also pass.

🟢 [Done] Allocation/free instrumentation covers cache hits/misses/prefetch and full engine project-stream graph adoption, loop and seek callbacks. No allocations or frees are observed in these callback paths. Final resource destruction and worker joins remain on control/preparation/reclamation threads.

Final verification: **136 tests pass in each of Debug and Release**, including 19 streaming, 24 project, 21 application and 31 engine tests. No new warnings are reported for the added code; existing project/MIDI Clippy warnings remain. Verification commands use Rust 1.93.0 and the pinned-toolchain `PATH` shown in the eighth batch: Debug/Release workspace tests, `cargo fmt --all --check`, and `cargo clippy --workspace --all-targets --offline`. The existing cancellation fixture initially exceeded its five-second held-worker gate during parallel JSON save/load testing; isolated execution passed in 1.38 seconds. Its test-only deadlock guard is now 30 seconds, with no change to production audio timing or cancellation policy. This gate is not a latency benchmark.

🟠 [Next] Readiness acknowledgements/admission for scheduled cold seeks, large-loop prefetch, multi-stream resource/fairness budgets, interruptible I/O and automatic recovery. Full document PCM remains resident and is also spooled to temporary backing; import/storage that avoids that duplication is separate work. A retained document/history keeps idle media workers available. Worker faults currently require reopening the project for fresh media preparation.

⭕️ [Pending] Physical storage and device stalls, loopback/listening, measured atomic-cache overhead, cross-platform file lifecycle and endurance qualification. Warm-fixture PCM equality does not guarantee gap-free arbitrary cold seeks or playback under overload. See the [streaming contract](STREAMING_VALIDATION.md).

## Tenth batch — readiness receipts and safe seek/loop admission

Date: 2026-11-06. Scope: native project timeline streaming; independently implemented without reading or reusing Pro-Wave code/assets. See the [readiness contract](STREAMING_READINESS.md).

🟢 [Done] Before rendering, the graph collects the unique required-page union per immutable media across all tracks/views and retains the complete callback window. Unavailable data silences the whole callback and holds musical Transport, MIDI, automation and all mixer/master ramps. Partial leases survive loading polls. Capacity overflow and required-worker failure have explicit distinct results, and inactive failed clips do not block unrelated ranges. Ready results render once at the held position and advance by exactly the admitted frame count.

🟢 [Done] Atomic leases fence eviction for an admitted window. A fully retained cache keeps decoded scratch off audio rather than repeating disk I/O. Changed targets cancel unstarted old hints/demands; an already executing read can finish at its original source address. Loop-aware lookahead avoids loading unrelated pages past a short loop. Adoption releases previous waiting leases before retirement; canceled graphs preserve the current wait. New graphs and actual Undo/Redo re-evaluate readiness while sharing immutable media. Resident and streamed PCM now use identical mixing order with MIDI.

🟢 [Done] Fixed atomic last-callback receipts expose actual graph revision, applied transport generation, checked position, requested/rendered frames, required/ready pages and cumulative loading waits. Concurrent reader tests reject mixed snapshots. Native publication polling exposes the receipt; the compact status icon supports English/Japanese/Simplified Chinese, suppresses stale-graph receipts and makes no native-readiness claim in preview/stopped sessions. A ready receipt confirms a completed output window, not guaranteed residency of future windows.

🟢 [Done] Final Rust 1.93.0 Debug and Release workspace suites each pass **145 tests**, including **23 streaming**, **35 engine**, **22 application** and **24 project** tests. Tests cover retained-page eviction, single-decode blocked publication, partial-window progress, union capacity across shared views/tracks, blocked old reads followed by new targets, canceled/adopted graphs with delayed retirement, Stop/fault/inactive-clip behavior, exact wait/resume PCM/MIDI/ramp references, and actual native history. Callback allocator instrumentation covers collection/poll/read/release, waiting, adoption, resume, failure, capacity and Stop with zero allocations/frees. `cargo fmt --all --check` and `cargo clippy --workspace --all-targets --offline` complete; only existing project/MIDI warnings remain.

Native verification commands use the pinned-toolchain `PATH` from the eighth batch:

```sh
cargo fmt --all --check
cargo test --workspace --offline
cargo test --workspace --release --offline
cargo clippy --workspace --all-targets --offline
npm run test:mixer
npm run test:midi
npm run build
```

The two mixer API tests, one MIDI suite and production Vite build pass. Browser preview explicitly reports `streamReadiness: null`.

### Status presentation QA

Flow: `http://127.0.0.1:5175/` → start engine view with a synthetic native receipt → waiting/ready/failed/capacity transitions → stale graph suppression → language changes → stop hides readiness. Browser plugin not available (no Browser skill in the session); existing cached Playwright 1.60.0/Chromium was used without installing dependencies. Viewports: 1440×1000 and 390×844.

| Check | Result |
| --- | --- |
| Page identity | Pass: localhost URL and Ongaku title. |
| Meaningful content | Pass: Night Sketch workspace rendered. |
| Framework overlay | Pass: no Vite error overlay. |
| Console health | Pass: no page errors or error/warning messages. |
| Screenshot/layout | Pass: desktop/mobile reviewed; status icon remains inside footer. |
| Interaction | Pass: all four states, graph revision mismatch, EN/JA/简体中文 hover/accessibility text, Stop and preview exclusion. |

Temporary fixture script/screenshots are outside the repository at `/tmp/readiness-ui.mjs`, `/tmp/readiness-desktop.png`, `/tmp/readiness-mobile.png`, `/tmp/readiness-status.png`. The fixture overrides served view metadata only; it does not qualify real native I/O or hardware timing.

🟠 [Next] Enqueue-time command IDs, scheduled readiness/deadline admission, multiple-engine fairness and aggregate resource budgets, long-loop tuning, interruptible storage shutdown and automatic recovery. Leases/receipts and pending transport work remain runtime-only; project format versions 1/2 are unchanged.

⭕️ [Pending] Real storage/device faults, throughput and atomic-cache overhead, listening/click tests, hardware loopback and endurance qualification. Holding musical time inserts wall-clock silence and is not a claim of uninterrupted cold-seek playback.

## Eleventh batch — accepted Transport IDs and observable results

Date: 2026-11-06. Independently implemented without reading/reusing Pro-Wave code or assets. The [Transport contract](TRANSPORT_COMMANDS.md) defines acceptance, cancellation, output readiness, retention and engine epochs.

🟢 [Done] Every accepted Transport command receives a JSON-safe monotonic ID. Native application/Tauri actions return `{ engineEpoch, commandId }`; per-ID status/cancel APIs validate that epoch. Invalid commands, normal queue saturation and early restart after Stop do not consume normal IDs. Queued cancellation skips mutation; active-wait cancellation does not undo already changed state. Status distinguishes accepted/pending/waiting/applied/cancel-pending/canceled/failure/capacity, with explicit effect flags and positions. New output intents supersede older unfinished waits; terminal results stay immutable while retained.

🟢 [Done] Stop bypasses the normal producer lock and full queue. Coalesced/delayed Stop producers settle correctly, stale starts are canceled, and queued clock/seek/loop policy is preserved. IDs never wrap; exhaustion still latches a safety Stop while reporting failed acceptance. Fixed 256-slot receipt storage protects writers from slot reuse and exposes retention gaps/update contention without inventing a successful outcome. Control snapshots use bounded coherent atomic reads; callback results and cancellation have one linearization winner.

🟢 [Done] Pending IDs survive actual graph replacement and native Undo/Redo. Readiness is rechecked against the adopted graph, with ID/graph/generation correlation and exact PCM output after waiting. History changes do not allocate Transport IDs. Restart rejects old epoch/ID pairs even when the new session reuses the numeric ID. Zero-frame callbacks leave playing requests pending rather than falsely completing readiness. Project file versions 1/2 and history contents remain unchanged; receipts are runtime-only.

🟢 [Done] Rust 1.93.0 Debug and Release workspace suites each pass **155 tests**, including **44 engine**, **23 application**, **24 project** and **23 streaming** tests. New/exercised cases include queued and active cancellation, delayed Stop producers, protected receipt wraparound, Stop under queue/receipt pressure, cancellation/application races, same-state snapshot coherence, validation/ID exhaustion, zero-frame callbacks, loading/failure/capacity results, blocked graph adoption, actual history and stale epochs. Allocation/free instrumentation observes zero callback allocations/frees across those paths. Formatting and Clippy complete with only the existing project/MIDI warnings.

Commands (using the pinned-toolchain PATH documented above):

```sh
cargo fmt --all --check
cargo test --workspace --offline
cargo test --workspace --release --offline
cargo clippy --workspace --all-targets --offline
npm run test:transport
npm run test:mixer
npm run test:midi
npm run build
```

Two Transport tests, two mixer API tests, the MIDI suite and production Vite build pass. JavaScript tests cover epoch/ID validation, preview null receipts, ID-correlated hover labels, cancellation-after-effect text and unavailable results.

### Transport result presentation QA

Flow: local app → engine view with synthetic native result → accepted/pending/waiting/applied/cancel-pending/canceled/failure/capacity hover text → mismatched ID suppressed → unavailable receipt → EN/JA/简体中文 → stopped/preview exclusion. Browser plugin not available (no Browser skill in session); preinstalled Playwright 1.60.0/Chromium used without installing dependencies. Environment: `http://127.0.0.1:5175/`, 1440×1000 and 390×844.

| Check | Result |
| --- | --- |
| Page identity | Pass: expected localhost and Ongaku title. |
| Nonblank screen | Pass: Night Sketch workspace rendered. |
| Framework overlay | Pass: none. |
| Console health | Pass: no page/console errors or warnings. |
| Screenshot/layout | Pass: desktop/mobile reviewed; existing footer layout retained. |
| Interaction | Pass: all lifecycle states, ID mismatch, retention loss, three languages, stopped and preview behavior. |

Temporary artifacts remain outside the repository: `/tmp/transport-ui.mjs`, `/tmp/transport-desktop.png`, `/tmp/transport-mobile.png`, `/tmp/transport-status.png`. The served-view fixture qualifies presentation only; Rust regressions qualify software protocol behavior, and neither qualifies physical audio latency.

🟠 [Next] Scheduled deadline admission, atomic command batches, exportable/durable audit history, shutdown outcomes beyond the current engine, controller/panic coverage, multi-stream fairness, interruptible I/O and automatic recovery. A terminal command result is its first resolution, not a guarantee that later playback windows remain ready.

⭕️ [Pending] Hardware/driver timing, listening, real storage/device faults, measured receipt-contention/overload latency and endurance qualification. No comparative DAW performance or sample-timed future execution claim is made.

## Twelfth batch — output negotiation and PCM callback boundaries

Date: 2026-11-06. Scope: advertised output validation, startup failure integrity and prepared sample adapters. No Pro-Wave source/assets were read or reused. See the [output contract](OUTPUT_CONFIGURATION.md).

🟢 [Done] CPAL preflight checks the requested sample rate, channel count and fixed buffer against advertised ranges, then revalidates at open and selects the matching encoding. It no longer uses an unrelated default sample format. Default buffer discovery respects advertised bounds; unknown bounds remain explicitly unknown. Native publication status reports selected device, requested/validated configuration, encoding and optional advertised range. This does not claim a measured physical driver period.

🟢 [Done] Unsupported settings are rejected before graph preparation. Injected preflight/open/start/report failures preserve the project, revision, Undo/Redo branch and uncommitted engine epoch; a subsequent valid startup succeeds. Mismatched reports are rejected before playback starts. Idempotent running Start requires the same active configuration and a matching explicitly requested device; incompatible settings are rejected without replacing the live stream. Selection metadata survives history and clears on stop. The mock rejects unknown IDs; browser preview reports null output metadata.

🟢 [Done] F32/I16/U16 output boundaries initialize silence, preserve frame alignment, normalize finite/clipped samples and replace non-finite data with silence. Exact integer fixtures confirm signed -32768/0/32767, unsigned 0/32768/65535 and both half scales, removing the previous one-step endpoint/equilibrium biases. Prepared integer scratch is cleared and never resized in the callback. Oversized or incomplete converted frames silence/report errors without calling the source; valid subsequent buffers recover. Empty callbacks preserve the zero-frame Transport policy. Invalid samples report one aggregate backend error per block.

🟢 [Done] Rust 1.93.0 Debug and Release workspace suites each pass **165 tests**, including **8 CPAL adapter**, **25 application**, **44 engine**, **24 project** and **23 streaming** tests. Capability tests use pure CPAL range fixtures; application tests inject four failure stages, retry, verify history and check running-start/device behavior. Adapter tests cover exact conversion, non-finite data, malformed/oversized/empty buffers, recovery and scratch clearing. Instrumented F32/I16/U16 normal/fault paths observe **zero callback allocations and frees**. The final boolean simplification of the running-device guard was also rechecked with both application output tests in Debug/Release. Formatting and Clippy complete with only existing project/MIDI warnings.

Commands, using the pinned-toolchain PATH described above:

```sh
cargo fmt --all --check
cargo test --workspace --offline
cargo test --workspace --release --offline
cargo test -p daw-app --offline output_tests
cargo test -p daw-app --release --offline output_tests
cargo clippy --workspace --all-targets --offline
npm run test:mixer
npm run test:midi
npm run test:transport
npm run build
```

The existing two mixer, two Transport and one MIDI JavaScript tests and the production Vite build pass. The preview API regression checks that output selection is null; no rendered UI surface changed in this batch.

🟠 [Next] Measured requested-versus-observed driver settings, separate adapter/driver error counters, bounded variable-period handling, more sample encodings, resource limits and device fault/recovery lifecycle. Oversized integer buffers intentionally silence and notify; they do not promise transparent variable-period playback. Backend-error counts include adapter faults and are not physical xrun counts.

⭕️ [Pending] Actual macOS/Windows/Linux device/format matrices, hot unplug/default-device transitions, loopback/listening, physical latency and endurance qualification. No hardware output was opened by these pure/injected regression tests.

## Thirteenth batch — output observability and truthful diagnostics

🟢 [Done] Fixed atomic output observations separate adapter-body timing from engine processing. Synthetic delivered periods test weighted load/deadlines, requested-size mismatches, min/max/last frames and unobserved states. Invalid/empty deliveries cannot inflate positive-period duration statistics. Driver notifications, typed adapter fault subtypes and process deadline misses remain separate while preserving legacy aggregate/alias fields. Legacy event sinks retain fault notifications.

🟢 [Done] Debug and Release workspace suites each pass **169 tests** (47 engine, 1 backend, 8 CPAL adapter, 25 application, 24 project, 23 streaming, plus other workspace tests). Existing graph/Undo/Redo, scheduled edits, Transport, streaming and persistence regressions remain passing. Normal/fault F32/I16/U16 paths retain zero callback allocations/frees with observations enabled. Rust 1.93.0 formatting and Clippy complete; only existing project/MIDI warnings remain.

Node 22.17.1 `npm run test:output`, `npm run test:transport`, `npm run test:mixer`, `npm run test:midi` pass (seven top-level tests total); `npm run build` passes. Native commands remain `cargo test --workspace --offline`, `cargo test --workspace --release --offline`, `cargo clippy --workspace --all-targets --offline` and `cargo fmt --all --check`, with the pinned toolchain binaries first on PATH.

| Browser QA | Evidence |
| --- | --- |
| Environment | Local Vite, cached Playwright Chromium; Browser plugin unavailable, so Playwright fallback; no dependency installation |
| EN / JA / Simplified Chinese | Observed 64–512 (256) frames, 12.50% output load, 13 diagnostic rows; explicit process count overrides legacy alias |
| Unknown/absent observations | Dash for unobserved load; absent output removes five output rows; no fabricated native observations |
| Layout | 1440×1000 and 390×844; compact preferences uses a single column and scrollable diagnostics; corrected inherited grid-area placement |
| Runtime | No page errors; `/tmp/observed-ui.mjs`, `/tmp/observed-desktop.png`, `/tmp/observed-mobile.png` are local QA artifacts |

These browser values are injected presentation fixtures, not native timing measurements. See [output telemetry](OUTPUT_TELEMETRY.md) for field semantics and limits.

🟠 [Next] Wake-up/physical driver-xrun observations, finer histograms, off-thread export, variable-period recovery and device lifecycle/resource limits.

⭕️ [Pending] Actual-device callback behavior, clock/latency, listening/loopback, unplug/recovery and endurance. Software and browser tests do not qualify physical playback.

## Fourteenth batch — exact musical conversion and quantization boundary

🟢 [Done] Constant-tempo bar/beat/tick conversion now uses bounded `u128` rational arithmetic with exact binary-tempo decomposition and quotient/remainder doubling. It rounds nearest with ties upward and saturates at the sample-position limit; normalization of invalid tempo/signature inputs is preserved. The reproduced 48 kHz, 400 BPM, 7/8 long-timeline case now resolves to sample 29,955,083,044,703 instead of the former one-sample-short result.

🟢 [Done] Legacy single-clip quantization computes grid width through the same conversion. At 399.8889197445154 BPM the sixteenth-note width is correctly 1800 rather than 1801 samples; an integration regression checks the resulting note start, unchanged duration and velocity. No stored sample positions or project schema are migrated. Explicit sample-grid multi-note/clip operations keep their existing semantics.

Seven new core tests exercise **583,872** direct rational-oracle combinations, exact half-sample rounding and adjacent binary tempos, external exact decimal-tempo fixtures, tick carry/monotonicity, invalid-input compatibility, saturation and half-open range endpoints. The independent oracle neither decodes the tempo bits nor uses production long division. Existing partitioned loop-oracle, graph, streaming, Transport, history and storage suites remain regression gates. The project adds one quantization integration test. See [musical clock contract](MUSICAL_CLOCK.md) for scope and numeric bounds.

Final native verification: the full workspace suites each pass **176 tests in Debug and Release**; the subsequently added project boundary regression also passes separately in both profiles (**177 covered tests in total per profile**, 12 core and 25 project). The final implementation passes workspace Clippy with existing MIDI/project warnings only; the floating conversion's two project cast warnings are removed. Formatting, whitespace and local documentation links pass. No callback allocation/free or continuity regression is observed in the existing instrumented suites.

JavaScript MIDI (one), mixer (two), Transport (two) and output diagnostics (two) tests pass; the production Vite build passes with Node 22.17.1. No rendered UI changed in this batch. Commands use Rust 1.93.0 binaries first on PATH, with offline workspace Debug/Release tests, Clippy and formatting checks as documented above.

🟠 [Next] Tempo-map/tick-domain document model, inverse conversion, absolute-position musical quantization across clips and broader clip/event interval ownership tests. Repeated rounded-grid spacing is explicitly unchanged; constant-tempo conversion does not complete all musical-clock work.

⭕️ [Pending] Physical clock drift, synchronization hardware, loopback/listening and endurance qualification. Pro-Wave code/assets were not read or reused.

## Fifteenth batch — fractional-period musical quantization

🟢 [Done] `MusicalGrid` prepares an exact rational whole-note subdivision and selects a musical index before rounding that individual position to samples. This removes repeated rounded-step drift from legacy single-clip quantization. At 44.1 kHz, 120 BPM and sixteenths, sample 88,200 no longer moves to 88,208. The clip-local origin, note targeting, bounds and expression remain intact; explicit integer-grid multi-note/cross-clip commands retain their API.

Three new core tests cover **46,080** independent distance-oracle/idempotence combinations, fractional positions, forward midpoint ties, invalid grids/tempo normalization, one-sample minimum, monotonicity at integer limits and saturation. A project regression preserves an exactly aligned note and repeats quantization without changes. An application regression quantizes 88,100 to 88,200, verifies duration, adopts graph revisions through Undo/Redo, saves/loads the edited document and checks graph adoption after restart. Loading deliberately clears the prior engine/receipt state; the fixture follows that lifecycle rather than asserting a surviving stopped engine.

Preview uses a BigInt equivalent without native callback ownership. `npm run test:clock` adds two tests for fractional/exact-tempo boundaries, safe-integer/idempotence handling and the preview edit/Undo/Redo/storage workflow. Existing MIDI, mixer, Transport and output tests remain passing, as does the production build. No rendered UI or localization strings changed.


Final verification: **182 workspace tests pass in both Debug and Release** (15 core, 26 project, 26 application). The native suite includes unchanged callback allocation/free, sustained MIDI, graph history, streaming admission and Transport regression gates. Clippy completes with existing project/MIDI warnings only; no new grid/application warnings. JavaScript tests pass nine top-level cases in total, and the production Vite build passes. Formatting, whitespace and local documentation-link checks pass.
Reproducible checks (Rust 1.93.0 / Node 22.17.1, toolchain binaries first on PATH):

```sh
cargo test --workspace --offline
cargo test --workspace --release --offline
cargo clippy --workspace --all-targets --offline
cargo fmt --all --check
npm run test:clock
npm run test:midi
npm run test:mixer
npm run test:transport
npm run test:output
npm run build
```

The Rust grid uses no allocation, lock or I/O; all new conversion work runs on the control thread. Prepared callback work is unchanged. See [musical grid arithmetic, ownership and bounds](MUSICAL_CLOCK.md).

🟠 [Next] Project-origin musical quantization across clips, tempo maps/tick-domain documents, inverse conversion and tempo-edit continuity. The current correction applies when requesting musical single-clip quantization; existing stored notes are never silently retimed.

⭕️ [Pending] Physical timing, listening/loopback, external sync and endurance. No Pro-Wave code/assets were read or reused.
