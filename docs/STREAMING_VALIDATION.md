# Streaming handoff and frame integrity

Date: 2026-11-06. Scope: independently implemented sequential streaming and project-integrated, position-addressed media streaming. Native audio clips use private file backing and a bounded cache; embedded PCM remains in the document for editing and save compatibility.

## Sequential pull-stream contract

- A successful `StreamController::seek(frame)` invalidates prefetched audio immediately. Frames are complete interleaved audio frames, not individual channel samples. Queue-full, worker-fault and generation-exhaustion errors leave the last accepted generation unchanged.
- Requested and applied generations are separate. Applied means the decoder seek completed; it does not promise that audio is buffered or audible. Pending seeks coalesce before decoder I/O. An already executing read/seek may finish, but obsolete read results cannot be relabeled as current audio.
- Consumers return only complete frames. Partial decoder reads carry remaining channels into the next read; callback output shorter than one frame consumes nothing. Incomplete output tails and unavailable audio are zeroed. Non-finite decoded samples are replaced with zero.
- Old queue entries are drained with a per-call bound of twice the output sample count (at least one frame). A newer-generation entry encountered during a concurrent handoff is retained for the next call. A generation change observed during consumption silences that callback. Old queued data can therefore require multiple callbacks to drain; this foundation provides stale-data rejection, not a scheduled seek deadline guarantee.
- Queue and chunk sizes must be frame-aligned; read-ahead is limited to two seconds of samples at the declared format. Consumer frame storage and decoder scratch are allocated before callback use. No decoding, seek, allocation, free, wait, or controller destruction occurs inside `read_interleaved`.
- EOF is tagged by generation. Known EOF and pending-seek waits are distinct from starvation. Snapshot counters expose stale samples, seek waits, starvation and worker errors; terminal worker failure silences consumption and rejects subsequent seeks. Overreported read counts and truncated final frames fault explicitly rather than corrupting channel order.
- One controller owns the seek producer. Controller destruction signals shutdown and joins the worker off audio. Blocking reader I/O remains cooperative and cannot be forcibly interrupted. Fixed reader format is required throughout its lifetime.

## Regression evidence

Two tests failed against the preceding implementation: accepted seek returned prefetched old audio before decoder acknowledgement, and an incomplete stereo frame returned one channel and shifted subsequent callback channels. Both now pass.

The streaming suite also covers variable short reads and callback partitions with an exact stereo PCM oracle, a gated decode completing after seek acceptance, coalesced seek targets, queue rejection/generation overflow, bounded stale draining, undersized output, future-generation lookahead, decoder contract faults, failed seeks, generation-aware EOF, and configuration bounds. Instrumented callback tests check zero allocations and frees during ordinary consumption and stale/stereo seek handoff. Gates and readiness deadlines coordinate tests; they are not disk-latency benchmarks.

## Project timeline integration

`Project::build_audio_graph[_cancellable]` prepares `StreamedAudioClip` views rather than direct PCM views for native project audio. Each immutable clip media identity owns a shared lazy cache. The first preparation spools its embedded PCM into an unnamed private temporary file; subsequent graphs and document/history clones reuse that file, worker and ready pages. MIDI keeps its existing timeline source and can coexist on the same track. The general sequential `StreamController` remains available for pull sources; project transport uses the position-addressed path below, not its sequential cursor.

### Addressing and continuity

- Every frame is requested by absolute **source** frame after mapping the half-open timeline clip interval and source offset. `Transport::sample_at_offset` supplies exact loop/seek positions even inside a callback or graph processing chunk. Cache results are never played later to compensate for a miss.
- Shared cache identity is tied to an immutable document media object, not just a track/clip ID, file name, current cursor or graph revision. Edit, reorder, Undo and Redo graph snapshots reuse it. Graph publication prepares new timeline ranges and inherits existing mixer ramps/MIDI state; it does not reset or transfer a sequential stream cursor.
- A loaded/reconfigured document creates new media identities and backing files, even when its public IDs match the previous project. Runtime caches are not serialized. Self-contained file versions 1/2 and their PCM contents remain unchanged.
- Each cached page has an absolute address and a non-wrapping odd/even publication version. Atomic sample bits and sequentially consistent version/payload accesses permit bounded readers without locks, allocation or unsafe memory access. Readers reject a frame if eviction overlaps its read. Versions prevent ABA when the same page is evicted and reloaded during a preempted reader; tags alone would not suffice.
- A raw unleased cache miss produces a silent frame and increments `underrun_frames`. Native project playback now uses [complete-window admission](STREAMING_READINESS.md): it retains pages before rendering and holds musical time while waiting, rather than advancing through missing frames. The latest missing position replaces an older waiting demand and is prioritized over bounded prefetch hints. An in-flight old read may finish and remain useful in cache, but cannot be emitted at another position. Loop-head hints and forward lookahead are submitted without blocking.

### Resources and failures

- Production pages contain `clamp(sample_rate / 8, 1, 4096)` frames, with 16 pages per media object; the generic cache accepts 2–32 pages, each at most 4096 frames, and never more than two seconds of payload. Initial pages are warmed off audio before graph publication. This bounds cache storage, not the retained document PCM, temporary file size or total project resources.
- Page decode, seek, scratch allocation and temporary-file I/O run only during off-audio preparation or on the media worker. Preparation checks cancellation between spool chunks, pages and short reads. A started blocking system read/seek cannot be forcibly interrupted. Readers publish no partial page; count errors, premature EOF, I/O failure and worker panics become terminal faults. Subsequent reads silence and graph preparation reports a failed cached worker; reopening the project creates a fresh cache for recovery.
- `PublicationStatus.streaming` exposes prepared-media count, page reads, missing-frame count, worker errors and failed-media count for the current document. Status sampling uses a nonblocking control-side cache lookup and can omit media still preparing. Counts are cumulative for shared media, not graph-revision acknowledgements or buffer-readiness guarantees. The new `streamReadiness` receipt reports complete-window admission and loading holds; `Transport.playing` remains the requested state. This is not a scheduled seek-deadline acknowledgement.
- Document/history and graph resources retain cache ownership. Graph adoption borrows their ready caches without replacing/dropping resource owners. Existing retired-graph reclamation destroys them off audio. Final cache destruction signals stop and joins its worker, then closes the anonymous backing file. Idle workers remain available while media is retained by the document/history; aggregate worker/resource budgets remain open.

### Integration evidence

- Exact stereo PCM oracle against direct `AudioClip` rendering covers clip edges, seeks, short loops, callback partitions and graph publications with edited/restored mixer state. Cache identity and unchanged page-read counts prove that history snapshots reuse warm media.
- Native application tests exercise actual Undo/Redo, reorder, pending-graph coalescing, adoption receipts, looping, seeking and save/load. Audible Gain/Pan/Mute history is compared to an independent direct-PCM graph, including ongoing ramps.
- A gated old read followed by multiple new positions proves latest pending demand coalescing and no stale replay. Concurrent eviction yields coherent stereo frames or silence. Runtime I/O failure, short decoder reads and canceled preparation have deterministic tests.
- Allocator instrumentation covers cache hits/misses/prefetch and the full project engine callback during loop/seek and graph adoption: zero allocations and frees are observed. These are software invariants, not hardware throughput measurements.

## Remaining work

- 🟠 [Next] Per-callback readiness receipts/admission are implemented in the tenth batch. Scheduled deadline admission, seamless cold-seek and large-loop prefetch policy, multi-stream fairness/resource budgets, and measured cache/atomic overhead under load.
- 🟠 [Next] Interruptible decoder I/O, bounded shutdown under storage stalls and automatic fault recovery. Audio import and file-backed document storage that avoid retaining full embedded PCM remain separate work.
- ⭕️ [Pending] Real storage stalls, hardware loopback/listening, device failure and long-session qualification. Cold or evicted native windows now hold musical time with silent callbacks until admitted; this work does not claim gap-free arbitrary seeks/loops under storage overload.

See the [P0 validation record](P0_VALIDATION.md) for commands and workspace results. No Pro-Wave source or assets were read or reused.
