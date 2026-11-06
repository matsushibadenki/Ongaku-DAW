# Streaming readiness and transport admission

Date: 2026-11-06. Independently implemented; no Pro-Wave code or assets were read or reused.

## Playback policy

Native project playback admits a **complete callback window**, including every streamed clip/channel and every loop wrap inside that window, before rendering anything. It maps Transport positions to source frames, groups all views of the same immutable media across tracks, and retains the union of required cache pages.

If any required page is unavailable, the engine outputs silence for the whole callback and holds the musical Transport position. MIDI voices, mixer/master ramps, automation and pull sources do not advance. Hardware callbacks and their metrics continue; `Transport.playing` remains the requested play state. This intentionally defers musical time rather than skipping audio and resuming late data at a different position. Silence inserted during waiting is not a hardware xrun measurement.

When the complete window becomes available, it is rendered once at its held position and Transport advances by exactly the admitted frame count. Loop mapping uses the same integer positions for admission and rendering, including multiple wraps in one callback. Lookahead follows Transport through loop boundaries instead of loading unrelated media past the loop tail.

This policy changes the previous cold-cache behavior, which advanced Transport past missing frames. It does **not** guarantee uninterrupted wall-clock playback during a cold seek, a storage stall or an oversized request, and it does not add seek/pause crossfades.

## Retained windows

`Arc<TimelineStream>::window()` creates a fixed-capacity `StreamWindow` without heap allocation. `require_frame` collects unique page requirements; `poll` returns readiness and retains available pages. Partial leases survive waiting callbacks for the same position/loop/output-size key, preventing ready pages from being evicted while missing pages load. One graph groups all views of a media into one window, so individually small overlapping clips cannot exceed that media's capacity unnoticed.

The writer claims an unleased slot with a bounded atomic compare-and-exchange scan. It never overwrites leased data. A fully leased cache retains decoded scratch off audio until a slot becomes available, or a newer demand supersedes that work; it does not repeatedly decode the blocked page. Existing page addresses and non-wrapping publication versions still protect ordinary unleased cache reads.

A ready window reads only retained pages. A worker fault racing after admission does not corrupt that already admitted block; the next poll reports failure. Lease release performs atomic decrements/reference-count updates; graph-owned media remain alive, so callback-side release cannot close a backing file or join its worker. Final media destruction remains off audio. Independent engines or external lease owners sharing a cache must release their windows; cross-engine fairness/resource arbitration is not implemented.

## States

| State | Meaning | Native engine action |
| --- | --- | --- |
| `inactive` | Playback is stopped or there are no complete output frames. | Release leases; render no timeline frames. |
| `ready` | Every required page for this callback is retained. A window with no active streamed clips requires zero pages. | Render once, then release leases and advance Transport. |
| `waiting` | One or more required pages are missing or temporarily unavailable. | Request pages, retain partial leases, silence and hold Transport. |
| `failed` | A required media worker has a terminal fault. | Release leases, silence and hold Transport. |
| `capacityExceeded` | The unique required-page union exceeds one media cache. | Release leases, silence and hold Transport; do not present this as ordinary loading. |

`requiredPages` is a lower bound when capacity is exceeded (capacity plus at least one). An inactive failed clip does not block an unrelated window beyond its timeline interval. Muted streamed tracks are conservatively included so unmute/solo changes do not silently change readiness policy.

Stop cancels a wait through the existing out-of-band barrier. A new seek or loop range/output size replaces the old requirements. A valid new target/graph can recover from a capacity result. A failed worker currently requires fresh media preparation, normally by reopening the project; automatic recovery and interruptible blocking I/O remain open.

## Graph, history and file semantics

A graph adoption releases the previous graph's waiting leases **before retirement**, even if retired graph reclamation is delayed. Canceled/superseded graph updates leave the active graph and its current wait intact. The adopted graph re-evaluates its own clip ranges and media requirements; an old ready result is never transplanted into a new topology.

Document/history clones retain immutable media/cache identity through edits and Undo/Redo. Mixer/MIDI state still follows the existing graph handoff contracts, while cache readiness is freshly evaluated. Loading another project creates new media identities. Neither readiness, leases, callback counters nor pending transport work is serialized into `.ongaku`; versions 1/2 and Undo/Redo contents are unchanged.

`AudioGraph::process_with_readiness` is the direct Rust admission API. Callers owning a Transport must advance it only for `ready`. The legacy `process` entry point emits silence on unavailable data but returns no admission result; the native engine uses the explicit API.

## Observable receipt

`EngineControl::stream_readiness()` and native `PublicationStatus.streamReadiness` return the **last completed callback** receipt, or `null` before a callback/when a bounded concurrent snapshot cannot be obtained:

```text
graphRevision        actual adopted graph, in the enclosing engineEpoch
transportGeneration  diagnostic generation of applied transport commands/Stop barriers
position             start of the checked musical window
requestedFrames      complete output frames requested
renderedFrames       requestedFrames only for ready; otherwise zero
waitingCallbacks     cumulative callbacks held specifically for loading
readiness            { state, requiredPages, readyPages }
```

The generation is not an enqueue-time command ID. `send_transport` still acknowledges queue acceptance; it does not synchronously acknowledge page readiness. Receipt fields use fixed atomic storage and a coherent, bounded control-side snapshot. Publication status components can be sampled across adjacent callbacks; the UI suppresses a receipt whose graph revision differs from the sampled adopted revision.

A ready receipt confirms the window that just rendered. Its leases have been released; it does not promise that later ranges or an entire large loop will remain resident. General command IDs/cancellation receipts, scheduled deadline admission and advance readiness promises remain future work.

The status-bar icon exposes ready/loading/failure/capacity states with English, Japanese and Simplified Chinese hover/accessibility text. Stopped and browser-preview sessions make no native readiness claim.

## Verification and remaining qualification

Software regressions cover retained-page eviction, blocked publication without repeated decode, partial-window progress, union-capacity rejection, blocked old reads, new targets while waiting, canceled/adopted graphs, delayed retirement, short loops without tail-prefetch churn, required/inactive faults, Stop, concurrent receipt reads and native Undo/Redo. PCM/MIDI/ramp output after waiting matches a direct-PCM reference exactly. Resident and streamed PCM now use the same summation/multiplication order, including mixed MIDI tracks.

Allocator instrumentation covers window collection/poll/read/release and complete engine callbacks during waiting, adoption, resume, faults, capacity and Stop. It observes zero allocations and frees in those paths. See [P0 validation](P0_VALIDATION.md) for final counts and commands.

- 🟠 [Next] Scheduled readiness/deadline admission, command request IDs, multi-stream/engine fairness and budgets, long-loop prefetch tuning, interruptible storage I/O, bounded shutdown and automatic recovery.
- ⭕️ [Pending] Physical storage/device stalls, listening/click qualification, measured atomic-cache throughput, hardware loopback and long-session endurance. Browser fixtures qualify status presentation only, not native timing.
