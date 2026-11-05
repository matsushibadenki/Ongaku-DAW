# Cancelable background graph preparation

The Tauri desktop application uses `DawApplication::new_with_background_preparation()`. Valid edits, Undo and Redo commit the Rust document/history and request an immutable graph-preparation snapshot. A single Rust worker prepares audio updates without holding the application mutex. Audio continues using its current graph throughout preparation.

`DawApplication::new()` / `with_sample_rate()` retain the synchronous control contract for headless clients and deterministic callers. Initial engine startup still prepares its ordered baseline synchronously. The background worker handles subsequent desktop edits; project validation, snapshot cloning, file I/O and startup are not themselves asynchronous/cancelable in this batch.

## Three distinct identities

1. **Document revision:** the committed history state, which Undo/Redo can restore to an older value.
2. **Preparation revision:** a monotonically increasing request ID within an engine epoch. It exists before a prepared graph can enter the audio queue.
3. **Publication revision:** allocated only after the finished graph enters the bounded audio queue. Callback receipts then report actual adoption, cancellation or superseding as described in the [publication protocol](GRAPH_PUBLICATION.md).

Never use a preparation ID as a publication receipt cursor. Native status exposes the latest document, last accepted publication and optional preparation state separately:

```js
const status = await engineApi.publicationStatus(0);
console.log(status.documentRevision, status.requestedRevision);
if (status.preparation) {
  console.log(status.preparation.requestedPreparation,
    status.preparation.documentRevision, status.preparation.pending,
    status.preparation.publishedRevision, status.preparation.failure);
}
// Audio receipt cursor remains status.engine?.settledRevision.
```

A successful desktop editing command acknowledges the validated document and preparation request, **not** graph acceptance/adoption. `requestedRevision` remains the last accepted publication while preparation is pending. `preparation.publishedRevision` maps a completed preparation to its publication; `engine.adoptedRevision` and its receipt remain the audio authority. An ellipsis beside the audio revision indicates preparation-pipeline work still pending, even if the previous publication has already settled.

## Cancellation and resource bounds

The worker retains **one active snapshot and one latest waiting snapshot**. A new validated request cancels the active token and replaces the waiting snapshot. Replaced waiting requests never run. Invalid/no-op document edits do not cancel valid work; request-ID exhaustion fails before cancellation. This coalesces preparation work while retaining every applicable document history operation.

`PreparationToken` implements `PreparationCheck`. Project graph assembly checks it between tracks/clips and source stages. MIDI validation/collection, event generation, ownership-index generation and stable sorting check in batches of at most 256 elements. Sorting uses bounded chunk sorts and checkpointed stable merge passes; the completed ordering matches the serial reference. A canceled partial buffer/graph is discarded on a control worker and can never become an audio source.

Cancellation is cooperative: allocations, individual container operations and a currently executing bounded chunk finish before the next checkpoint. No thread is forcibly terminated. New processors must honor the preparation-check contract. Document cloning/validation, save/load, decoder/analysis jobs and the initial startup baseline need separate cancellation contracts; this implementation does not claim to preempt those operations.

## Publication fence and queue pressure

After construction, the worker takes its coordinator mutex and checks that the engine is open, the preparation request is still the latest, and its token is not canceled. Publication occurs within that same gate. Thus a request superseded after its final construction checkpoint still cannot publish. A new request accepted after publication instead uses the existing pending-graph coalescing path; publication already completed is not retroactively labeled preparation cancellation.

`EngineControl::try_submit_latest_graph` returns ownership of a rejected graph with its error. On transient `PublicationBusy`, the worker retains exactly that prepared graph and waits on a condition variable (5 ms retry bound, earlier wake on new work/stop). It retries publication without rerunning preparation. A newer request can cancel the ready graph while it waits. Queue capacity remains four; the existing callback cancellation cells, retirement bounds and one-live-adoption-per-callback rule are unchanged.

The publication revision is assigned only on successful insertion. Canceled/failed preparation consumes no publication ID and creates no callback adoption receipt. Actual mixer deadlines are still computed against the graph currently sounding at callback adoption, not a canceled preparation or an intermediate document. Retained events cannot lose deadlines, and a set→Undo→Redo→Undo result matching the audible document creates no ghost deadline.

## Failure, history and persistence

Background graph-construction/publication failures are explicit in `preparation.failure`, with preparation/document revisions and a message. The last accepted audio graph continues. The already committed document/history is retained: asynchronously rolling it back could overwrite newer edits or invalidate Undo/Redo. A valid later edit/Undo/Redo starts new work and clears the current failure; restart also rebuilds the latest document. Cancellation/stale-job failures are not surfaced as command errors. Build panics are converted to failures where unwinding is supported; this is not general processor/process fault isolation.

Saving while preparation is pending writes the committed document, not whichever older graph happens to be audible. Saved/dirty tracking remains based on document revisions. Graph-preparation IDs and runtime cursors are not persisted; `.ongaku` versions 1/2 are unchanged.

`PreparationStatus` reports requested, active/waiting and published preparation IDs, associated document/publication IDs, started/canceled counts, ready-for-publication waiting state, failure and up to 64 terminal outcomes (`canceled`, `published`, `failed`). `outcomesDropped` explicitly counts discarded old detail. These outcomes are recorded in completion order, not preparation-ID order; an old running job can acknowledge cancellation after a newer waiting job was already replaced. `published` means accepted by the audio queue, not adopted by the callback.

## Shutdown and interface

Engine stop/document replacement cancels active/waiting preparation and joins the worker on the control thread. A publication fence prevents old work from publishing into a later epoch. Restart creates fresh preparation state in the new engine epoch. No worker mutex, cancellation scan, condition wait, thread join or preparation allocation runs in the audio callback.

The existing serialized UI poll reads preparation state alongside engine receipts. Pending preparation is shown compactly with localized hover text in English/Japanese/Simplified Chinese. New preparation failures use the existing dismissible error notice once per preparation ID; identical polls do not redisplay a dismissed failure. Cancellation is not an error. Preview returns `preparation: null` and does not fabricate a native worker.

Permanent tests cover cancellation during construction/sorting, latest waiting-slot replacement, completion-after-cancel fencing, retained-graph retry without rebuild, bounded outcome retention, explicit failure/recovery and shutdown. Background application tests run audio while preparation is held, exercise queued Undo/Redo and save/reopen semantics, and compare sustained PCM bit-for-bit. Queue coalescing plus actual-sample deadline tests verify both retained past events and full reverts. Existing allocation/free callback and sample-partition suites remain required.

**Pending qualification:** hardware timing/listening, overload cancellation latency, device failure recovery and long-session endurance. Cancellation responsiveness is not a measured hard realtime guarantee. No Pro-Wave source was used.
