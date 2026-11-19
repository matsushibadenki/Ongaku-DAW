# Output configuration validation and sample boundaries

Date: 2026-11-06. Independently implemented; no Pro-Wave source/assets were read or reused.

## Configuration selection

The CPAL backend checks the requested sample rate, channel count and fixed buffer size against `supported_output_configs` before preparing a project graph, then checks again against the device selected when opening the stream. Selection uses the matching range's sample format rather than the device's unrelated default sample format. There is no automatic project resampling, channel conversion, device substitution or buffer-size fallback for an explicit request.

Supported output encodings are F32, I16 and U16. Among matching ranges, known buffer bounds take priority, followed by F32, I16 and U16, then deterministic range tie-breaks. An unsupported encoding is ignored only when another matching supported encoding exists; a device with no usable matching tuple is rejected. These are stream encodings, not claims about physical DAC bit depth.

Default configuration discovery preserves the device default rate/channels and chooses a supported buffer nearest 128 frames, preferring advertised bounds. It does not return a fixed 128-frame request outside known bounds. Malformed ranges, invalid configurations and unsupported tuples are rejected.

`SupportedBufferSize::Unknown` cannot verify a period in advance. Such a range remains eligible when no better bounded choice matches, and the fixed request must still pass CPAL stream construction. Unknown bounds are reported as unknown, not as a verified hardware period. A default-device change between preflight and open is revalidated at open; the opened device identity is reported.

## Application state and reporting

`AudioBackend::validate_output_config` is a control-thread preflight hook. CPAL implements capability checks; the default hook only checks the basic config for compatible custom backends. `AudioOutputStream::output_configuration` optionally reports the selected stream. A custom backend returning no report remains explicitly unknown.

The application preflights before graph/streaming preparation and rejects a reported requested/validated format mismatch before starting audio. It commits engine state, epoch and output metadata only after successful startup. Preflight/open/start/report failures leave the document, saved/history state and active configuration unchanged; a later valid startup remains possible. Open/start failures after graph preparation can leave useful immutable media caches warm without changing the document.

An already running engine only accepts an idempotent Start with the same active format and, when explicitly requested, a verifiably matching device ID. A different format/device request fails rather than reporting success while continuing the old stream. Changing output settings requires stopping first. Selecting the default again does not silently switch a running engine to a newly changed OS default device.

Native `PublicationStatus.outputConfiguration` contains:

```text
deviceId                 identity selected at open
requested                requested sampleRate/channels/bufferSize
validated                matching configuration passed to the stream builder
sampleFormat             f32, i16 or u16
advertisedBufferRange    { min, max }, or null for unknown driver bounds
```

Requested and validated values are equal by policy. This is **not** an independently measured actual rate/period or a driver-negotiated buffer observation. Actual callbacks may vary. Existing engine metrics budget actual processed frame counts; delivered-frame observations are now available in [output telemetry](OUTPUT_TELEMETRY.md); physical rate and scheduling measurements remain unfinished.

The report survives ordinary edits and Undo/Redo, clears when the engine stops, and is not serialized into `.ongaku` or transferred as audio frame data. Browser preview reports null output metadata. The mock reports its simulated F32 output explicitly and rejects unknown mock device IDs; it does not qualify hardware.

## Callback boundaries

- F32 output is initialized to silence before source processing; non-finite values become silence, finite values are clamped to the normalized range.
- Integer outputs use the sample conversion contract exported by the locked CPAL dependency. Independent endpoint fixtures verify I16 -32768/0/32767 and U16 0/**32768**/65535, including positive/negative half scale. This removes the previous one-step signed full-scale and unsigned silence biases.
- Prepared integer scratch never grows in the callback and is cleared before processing, preventing unwritten tails from replaying old data.
- Incomplete interleaved frames, or integer output exceeding prepared scratch, produce full-format silence and report a backend error. The source is not called, so its musical clock/state is not advanced behind discarded output. A subsequent valid block resumes normal processing.
- Empty, frame-aligned buffers are forwarded; the engine's zero-frame command/readiness policy remains intact.
- Invalid samples report one aggregate backend error per affected block. Driver notifications and typed adapter faults now have separate counters and retain the existing aggregate backend-error counter; none is a physical driver-xrun measurement.

Callback processing performs no allocation, free, resizing, decoding or I/O. The integer oversized-buffer policy is explicit silence plus an error, not transparent recovery or gap-free variable-period support. Metadata/control acknowledgement can remain pending until a usable callback arrives. Failed-stream isolation and manual stop/restart are described below. Broader period handling and automatic recovery remain roadmap work.

## Output failure lifecycle

The CPAL stream owns a sticky atomic failure flag shared with its error callback and a guard around the engine callback. Device loss, stream invalidation and other errors requiring rebuild set the flag before notifying the existing backend-error sink. CPAL's Xrun, DeviceChanged and RealtimeDenied notifications keep the stream active. These notifications remain included in driver/backend metrics. This classification deliberately treats all remaining and future unknown error kinds as requiring an explicit restart.

After a callback observes the flag, it emits silence and skips the engine. F32 and I16 silence is zero, and U16 silence is 32768. Transport, voices, graph adoption, ramps and readiness commands receive no further engine callbacks on that stream. An already executing callback can finish when an error arrives concurrently. The flag and guard use atomic loads/stores and existing prepared buffers without callback allocation, free, locks or I/O.

`AudioOutputStream::is_faulted()` exposes stream health with a default false implementation for existing adapters. Native `DawApplication::state()` returns Faulted when the stream reports failure. Publication status includes `outputFaulted`; preview reports false. Start on that failed session and new Transport requests return errors. Failed output retains its original selected configuration for inspection. A DeviceChanged notification retains the original selection report until the stream is reopened; live route metadata refresh is follow-up work.

Project edits, Undo/Redo and saves remain available in the faulted session. Such edits update the Rust document/history and defer new graph preparation until restart. Previously queued preparation may finish; the failed callback guard prevents subsequent adoption. Pending receipts remain inspectable until Stop releases the session. Durable shutdown outcomes are follow-up work.

The persistent three-language notice offers Stop audio. CPAL Stop skips pause on a known failed device and releases the stream through the existing control-thread cleanup. Restart opens a fresh stream with a fresh flag and engine epoch, prepares the latest document and rejects old-epoch Transport IDs. This explicit stop/restart follows the existing initial Transport-position behavior. The implementation currently relies on user-triggered restart.

A failure detected during startup returns an error before committing the stream/control/epoch. The initialized document and history remain available for retry. UI Transport controls disable during the fault while the stop/power controls remain usable. The recovery banner is layered above the narrow-screen Browser panel.

## Safety stop and Transport receipts when audio is unavailable

Backend buffer rejection and failed-stream callbacks invoke `AudioCallback::output_unavailable()` on the same output thread as `process`. The hook is allocation-free and serialized with rendering. Existing callback implementations receive a default no-op hook. CPAL calls it for malformed F32 frames, malformed/oversized converted output, and callbacks gated by a failed stream.

The engine applies its out-of-band Stop barrier, stops Transport without advancing position, cancels stream requests and settles queued/waiting Transport commands with `OutputUnavailable`. Explicit cancellation already requested retains its reason. A Stop actually handled by the engine is reported as applied. A later valid callback can accept fresh commands after a transient buffer rejection. Direct malformed engine buffers retain the earlier normal-command hold policy and still apply safety Stop.

Confirmed stream failure sets a sticky engine admission flag through control-side status service. Result queries scan the bounded receipt store with atomic result guards and cancel unfinished commands even if audio callbacks cease. Previously terminal results are preserved. A receipt being written concurrently is retried on subsequent status service. Callbacks check the failure flag before rendering and stop at their next admission check. An executing callback may complete before observing failure.

The control-side snapshot reports playback/recording stopped. Unexecuted Stop commands receive the unavailable cancellation outcome when engine callbacks have ceased. Result metadata retains any earlier applied effect, and settlement never rolls it back. Transport/result/cancellation queries remain available during failure so UI polling can obtain the fault and receipt notifications. Source failure notices and the existing error counters continue separately.

Receipt retention remains bounded to the current engine session. Stop/disposal and restart follow the existing epoch policy. Durable receipt history across disposal and graph publication shutdown outcomes remain follow-up work.

## Evidence and remaining work

Pure capability tests cover rate/channel/format selection, buffer endpoints, unknown bounds, invalid ranges, deterministic ranking and default buffer clamping without opening hardware. Application tests inject preflight, open, start and report failures, then retry successfully and exercise history. They also verify idempotent start, incompatible running requests, selected metadata and stop/unknown-device behavior.

Callback tests verify exact sample boundaries, non-finite handling, unsigned equilibrium, malformed/oversized buffers, recovery, empty callbacks and cleared scratch. Allocation instrumentation observes zero allocations/frees in F32, I16 and U16 normal/fault processing. See [P0 validation](P0_VALIDATION.md) for workspace results and commands.

- 🟠 [Next] Physical rate/wake-up observations, bounded variable-period handling, broader encodings/resource budgets, device enumeration/route refresh, automatic recovery and sleep/wake handling.
- ⭕️ [Pending] Actual macOS/Windows/Linux device matrices, unplug/default-device changes, loopback/listening, physical latency and endurance. Software fixtures do not establish real-driver behavior.
