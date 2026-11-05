# Graph publication receipts, coalescing and edit deadlines

The audio callback remains the authority for graph adoption. Desktop editing commits the validated document and requests [background preparation](GRAPH_PREPARATION.md); graph acceptance occurs later. Synchronous Rust editing still accepts a complete prepared snapshot before committing. Neither path promises that every intermediate snapshot will become audible. Application edits and Undo/Redo use latest-wins publication. Ordered low-level publication remains available for updates that must not be implicitly skipped.

## Status and terminal outcomes

`engineApi.publicationStatus(afterRevision = 0)` calls native `get_graph_publication_status`. Its response includes `documentRevision`, `requestedRevision`, `engineEpoch`, optional `engine` status and optional `preparation` status.

```js
const status = await engineApi.publicationStatus(0);
if (status.engine) {
  console.log(status.engine.acceptedRevision, status.engine.adoptedRevision,
    status.engine.settledRevision, status.engine.pending);
  for (const receipt of status.engine.acknowledgements) {
    console.log(receipt.revision, receipt.documentRevision, receipt.outcome,
      receipt.adoptedSample, receipt.resolvedSample, receipt.deadline, receipt.late);
  }
  // Next query: publicationStatus(status.engine.settledRevision).
}
```

- **Document revision** identifies a history state. Undo can restore an older document revision; Redo restores the original one. Publication coalescing does not remove history entries or change saved/dirty-state semantics.
- **Publication revision** increases for every accepted graph, including Undo/Redo. Rejected synchronous submissions consume no ID and leave document/history/requested state intact. Background queue pressure retains and retries the same graph; an eventual preparation failure is explicit and does not roll back the already committed document. Zero is reserved. IDs and epochs are bounded by JavaScript's maximum safe integer; exhaustion returns an explicit error.
- **Engine epoch** increases on successful engine start. Stop/document replacement invalidates pending tokens; the next engine has fresh counters in a new epoch. Reset receipt cursors on an epoch change. If an old-epoch cursor was used for the first response, query again with zero for the new epoch's records.
- **Accepted revision** is the newest accepted token. **Adopted revision** is the last token actually installed as the active graph. **Settled revision** is the newest processed token, whether adopted, canceled or superseded. **Pending** compares acceptance to settlement. Adopted/accepted are IDs, not counts; do not infer that all smaller IDs were adopted.
- **Outcome** is `adopted`, `canceled` or `superseded`. Only adopted receipts have a non-null `adoptedSample` and can be late. `resolvedSample` records the callback's timeline position for every outcome. `canceledCount` and `supersededCount` count resolved skips, not cancellation requests. The cumulative late count increments once per late adoption.
- **Cursor:** query strictly after the previous `settledRevision`, not `adoptedRevision`, so a canceled latest publication does not replay forever. An exact receipt confirms its outcome. `settledRevision >= token` alone does not prove adoption.

The receipt payload now makes `adoptedSample` nullable for skipped graphs and adds `outcome`/`resolvedSample`. API consumers must check the outcome and advance by settlement. The status bar retains its adopted/accepted display. Preview returns `engine: null` and never fabricates native adoption.

## Cancellation and coalescing

Rust control APIs provide three submission modes:

- `submit_graph_revision` (and compatibility `submit_graph`) stays ordered and is not superseded implicitly. The compatibility wrapper tags document revision zero and has no explicit deadline.
- `submit_latest_graph_revision` accepts replaceable full snapshots with caller-supplied document revision/deadline metadata. The caller must describe the final snapshot's deadline, not merely the last small edit. Use the timeline-specific mode for automatic mixer comparison.
- `submit_latest_timeline` is the application path: it supersedes older pending replaceable snapshots and computes mixer deadlines against the graph actually sounding at adoption. The initial engine-start graph remains ordered to establish its baseline.

`cancel_pending_graph(token)` is a low-level Rust request. `true` means cancellation won an atomic race against callback claiming and that graph cannot be adopted; the terminal canceled receipt arrives when the callback processes it. `false` means it is no longer cancelable (claimed, settled, already canceled/superseded, or unknown). Cancellation does not undo the project or remove data from a future full snapshot. Consequently the application/Tauri layer uses document Undo/Redo plus automatic superseding, rather than exposing a cancellation command that could leave the document ahead of audio indefinitely.

New submissions supersede old pending replaceable entries **only after successful queue insertion**. Already claimed updates finish adoption; new edits cannot interrupt a callback or reverse audio already rendered. Ordered entries remain barriers: the callback can skip preceding canceled/superseded entries, but adopts at most one live graph per callback. Mixed ordered/replaceable streams retain that ordering contract.

The queue still holds four entries. Superseded entries keep ownership until callback retirement, so a fifth low-level/synchronous submission can still fail before any callback consumes the queue. The desktop preparation worker retains a busy ready graph and retries without rebuilding. It does not invalidate the fourth accepted graph. This bounded backpressure is intentional; unbounded ownership queues are not implemented. Off-thread preparation cancellation is now implemented with one active and one latest waiting snapshot.

## Deadline and continuity semantics

A coalesced application graph is compared to the **actual active prepared mixer schedule**, not the last accepted document. Prepared definitions are indexed by stable event ID outside audio processing; callback comparison merges those indexes without allocation. Track matching uses stable IDs. Conservative deadlines include changed/inserted/removed definitions and both positions of moves.

This prevents two opposite errors: an unrelated final edit cannot hide an event introduced by a skipped parent, and a set→Undo→Redo→Undo burst returning to the audible schedule does not report a ghost deadline. Low-level ordered/explicit-deadline APIs retain caller metadata. If no timeline baseline exists, initial whole-graph publication has no automatically inferred edit deadline.

Transport commands are applied first. A live adoption is late when playback is running and `adoptedSample > deadline`; equality is on time. Paused adoption and skipped graphs are never counted late. Positions are project samples in the current loop pass, not wall-clock estimates. Late graphs chase current values with the existing 5 ms ramps and compatible MIDI state transfer, without stopping playback, rewinding history, moving events or altering already rendered audio.

Skipped graphs never inherit or advance live voices/ramps. Only the final adopted graph inherits current runtime state. Graph publication coalesces complete snapshots, not individual timeline events: all events retained in the final snapshot remain scheduled at their original samples, including across loops and seeks.

## Ownership, bounded observation and service

Five fixed atomic cancellation cells cover four queued entries plus one in-flight entry. A cell cannot be reused until callback settlement; graphs themselves stay in the ownership queues. Atomic claiming/canceling has one winner, without callback mutexes or spinning.

At most four queued entries are examined per callback. Skipped graphs and the previous live graph enter the existing retired queue; a full retired queue uses the single owned fallback and stops draining. No skipped graph is destroyed in the callback. Backpressure preserves pending ownership and does not produce a false adoption.

Native status polling calls `DawApplication::service_publications`, reclaiming retired/skipped graphs on the control thread before reading status. This allows reclamation backpressure to clear even without another edit. Low-level users must continue calling `reclaim_retired`; `graph_publication_status` itself is a pure observation API.

A fixed 64-record atomic ring stores every terminal outcome. Stamped, sequentially consistent fields reject overwritten/torn records; observation allocates its bounded result only on the control thread. `historyLost` explicitly reports missing detail. Current adopted/settled counters and cumulative outcome counts remain observable; counters are sampled concurrently, not a transaction over the whole engine. Records themselves are coherent. Do not reconstruct missing per-token outcomes from aggregate counters.

## Interface and qualification

Polling stays serialized and outside audio scheduling. Canceled/superseded graphs do not create late warnings; the UI advances through their settlement watermark. New late adoptions and missing receipt detail retain the existing dismissible English/Japanese/Simplified Chinese notices. Engine stop/document replacement clears session status. There is no new cancellation widget.

Regression tests cover latest-only adoption, explicit cancel/claim races, ordered barriers, rejected-newest rollback, retirement stalls, final-snapshot deadlines, independent document-map versus prepared-index diffs, queued Undo/Redo, and mixed terminal-record readers. MIDI/automation/loop PCM matches uninterrupted reference output under variable buffers, with zero callback allocations/frees during cancellation/coalescing.

**Still unfinished:** proactive deadline admission, cancellation of document validation/startup/import/analysis work, event/transport/controller protocols beyond prepared snapshots, graphical event lanes, native push notifications, physical DAC/loopback timing and endurance/device qualification. No Pro-Wave source was used.
