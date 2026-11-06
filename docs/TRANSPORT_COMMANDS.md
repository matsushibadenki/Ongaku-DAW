# Transport command IDs and results

Date: 2026-11-06. Independently implemented without reading or reusing Pro-Wave source/assets.

## Acceptance and scope

`EngineControl::submit_transport(command)` returns a positive, monotonic command ID. The compatibility `send_transport` method still returns `Result<()>` but allocates/tracks the same IDs. Normal commands share the existing 64-entry FIFO. Invalid commands, full queues and Play/record requests during an unacknowledged Stop are rejected before allocating an ID. Tempo must be finite and within 20–400 BPM; loop ranges and meters must be valid.

Native application/Tauri Play, Pause/Stop, Seek and SetLoop calls return:

```json
{ "engineEpoch": 1, "commandId": 8 }
```

Always retain **both** fields. Numeric IDs restart with a new engine; application status/cancel APIs reject an old epoch before looking up its ID. Transport pause does not change the epoch; audio-engine shutdown ends receipt availability. Runtime IDs, results and cancellation requests are not serialized into project files or Undo/Redo.

IDs are exactly representable by WebView JSON numbers, up to `2^53 - 1`. They never wrap. At exhaustion, new acceptance fails and the engine must restart. A failed Stop acceptance still latches an out-of-band safety stop; it does not fabricate or reuse an ID.

Graph publication revisions, document revisions and Transport IDs are different namespaces. A larger Transport ID is not a guarantee that all smaller IDs have completed: Stop can overtake the normal FIFO, and concurrent producers can finish acceptance in a different order.

## Status and cancellation APIs

Rust application methods:

```text
submit_transport(command) -> TransportAcceptance
transport_command_status(engine_epoch, command_id) -> TransportCommandStatus
cancel_transport_command(engine_epoch, command_id) -> bool
```

Native Tauri commands are `get_transport_command_status` and `cancel_transport_command`, with `engineEpoch`/`commandId` arguments. JavaScript adapters:

```js
const accepted = await engineApi.seek(20_000);
if (accepted) { // Browser preview returns null; it creates no native IDs.
  const status = await engineApi.transportCommandStatus(accepted);
  // Optional: request cancellation, then poll the same acceptance for its result.
  const cancellationRequested = await engineApi.cancelTransportCommand(accepted);
}
```

A status contains `acceptedId` (the accepted high-water mark), `commandId`, `historyLost`, `updating`, and a nullable `result`. Only query IDs actually returned by acceptance; the high-water mark is not a contiguous completion/acceptance ledger for guessed IDs. The result includes:

- `kind` and `state`;
- `effectApplied` and nullable `appliedSample`, confirming whether Transport mutation already occurred;
- nullable `resolvedSample` and `cancelReason`;
- `graphRevision`, `transportGeneration` and the latest admission `readiness` page/state details.

`appliedSample` is the musical cursor immediately after the command's mutation, including loop normalization. It is not a hardware timestamp or scheduled execution deadline. Graph/generation fields describe the latest wait/resolution observation; `appliedSample` remains the original mutation position.

### Lifecycle

| State | Meaning |
| --- | --- |
| `accepted` | Queued/registered, not yet consumed by audio. |
| `pending` | Transport mutation completed; its output admission has not yet been observed. |
| `waiting` | Mutation completed, but the complete output window is not ready; musical time is held. |
| `applied` | Terminal success. For an admission-dependent command while playing, its first output window was admitted. For a stopped/metadata-only command, its Transport mutation completed without claiming audio readiness. |
| `cancelPending` | Cancellation won the control-side request race; audio must acknowledge it. |
| `canceled` | Terminal cancellation, with `explicit`, `superseded` or `stopped` reason. |
| `failed` | Terminal required-stream failure at admission. |
| `capacityExceeded` | Terminal rejection of an output window whose required-page union cannot fit. |

Play, Seek, SetLoop and SetRecording(true) own an output-admission context while playing. The **latest** such command replaces the previous unfinished context; the earlier result becomes `canceled/superseded`. Effects already applied by earlier commands are retained. For example, SetLoop followed by Seek can cancel the older loop command's output wait while leaving the configured loop in force. `effectApplied` prevents cancellation from being mistaken for rollback or “never executed.” This is not an atomic multi-command transaction.

Metadata-only commands do not replace the output wait. Zero-frame callbacks do not complete a playing output context. Commands applied while stopped complete with inactive readiness; they do not promise that a subsequent Play will avoid loading. Recording commands describe the existing Transport flag, not implemented audio capture.

Applied/canceled/failed/capacity results are terminal and remain unchanged while retained. A later graph change can make current playback ready without rewriting an earlier command's terminal failure. Use the live readiness receipt for ongoing playback health.

### Cancellation semantics

- `true` means cancellation was requested, not that audio has already acknowledged it. Poll for terminal `canceled`.
- A queued cancellation skips mutation (`effectApplied: false`). Canceling the active loading context stops that wait at its already applied position; it does not restore an old cursor, loop, project or history state.
- Cancellation affects its own ID. Newer queued/playback requests may still run; use Stop for the global playback barrier.
- A result already being claimed/applied by audio reports a retryable update error; terminal requests return `false`. Audio never waits for the canceling thread. Once a render claim wins, cancellation cannot be falsely acknowledged for that admitted block.
- Stop itself cannot be explicitly canceled. Expired/unavailable IDs cannot be canceled by guessing a new slot occupant.

## Stop priority and races

Stop does not take the normal producer mutex and is accepted even when that queue is full. Repeated unprocessed Stops coalesce into the latest barrier; older unprocessed Stop IDs resolve as superseded. A delayed older producer is settled on a subsequent callback even if a newer barrier already ran. Stops actually applied before another Stop remain successful terminal results.

The barrier cancels an active output wait and invalidates stale Play/record-start requests. As before, queued seeks, loop/tempo/meter changes are retained and may update the stopped state. Play is rejected until the pending barrier has been acknowledged. Stop application/results remain allocation-free in the audio callback.

## Graph, streaming and history consistency

An unfinished command keeps its accepted ID across graph preparation/adoption and Undo/Redo. Admission is evaluated against the **actually adopted** graph after command processing; a waiting result updates its graph revision, then resolves against the graph whose complete stream window can render. Graph edits/history do not allocate new Transport IDs or transplant a ready result from the previous topology. Canceled/superseded graph updates do not falsely complete a waiting command.

`PublicationStatus.transportCommands` exposes the accepted high-water mark, applied Stop ID, receipt-registration loss count and latest retained command status. `streamReadiness.transportCommandId` links the last callback to its current output-context owner (zero before one exists). The existing graph/readiness fields retain their own meaning; a terminal command result confirms its first resolution, while last-callback readiness describes later output windows.

The compact Audio R hover text displays the latest ID/state, retention/update conditions and cancellation reason in English, Japanese and Simplified Chinese. Post-mutation cancellation explains that changed Transport state is retained. Stopped engine and browser-preview sessions make no native-ID claim. Preview methods return null IDs/status and false cancellation rather than inventing acknowledgements.

## Bounds and evidence

There are 256 fixed receipt slots. Reuse is guarded by ID-tagged atomic claims, atomic payloads and a coherent version check; an old writer cannot overwrite a new occupant. Control readers make at most eight snapshot attempts and report `updating` rather than accepting a torn result. Cancellation and audio application have one linearization winner.

Normal retention can expire old results. In rare slot-writing contention at wraparound, acceptance still preserves the Stop safety path but a new receipt can be unavailable immediately; `historyLost`/`lostReceipts` make this explicit. The loss counter counts registration gaps, not every normal eviction. No absent result is interpreted as applied or canceled. Receipts are bounded diagnostics, not a durable audit log.

The callback uses fixed storage, bounded command draining/Stop scans, and atomic claims; it neither allocates nor locks to write results. Tests instrument callbacks while queued cancellation, loading, graph adoption, resume, faults, capacity and Stop execute. Tests also cover zero-frame callbacks, real Undo/Redo with a held cache, coherent concurrent snapshots, cancellation/application races, protected slot reuse, queue rejection, delayed Stop producers, ID exhaustion and stale epochs after restart.

- 🟠 [Next] Scheduled deadline admission, atomic command batches, durable/exportable audit history, lifecycle/shutdown outcomes beyond the current engine session, multi-stream fairness, interruptible I/O and automatic recovery.
- ⭕️ [Pending] Physical device/driver timing, listening, measured contention/overload latency and endurance qualification. IDs confirm software lifecycle, not sample-timed future execution or uninterrupted cold seeks.
