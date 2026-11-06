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

Callback processing performs no allocation, free, resizing, decoding or I/O. The integer oversized-buffer policy is explicit silence plus an error, not transparent recovery or gap-free variable-period support. Metadata/control acknowledgement can remain pending until a usable callback arrives. Device fault/recovery state and broader period handling remain roadmap work.

## Evidence and remaining work

Pure capability tests cover rate/channel/format selection, buffer endpoints, unknown bounds, invalid ranges, deterministic ranking and default buffer clamping without opening hardware. Application tests inject preflight, open, start and report failures, then retry successfully and exercise history. They also verify idempotent start, incompatible running requests, selected metadata and stop/unknown-device behavior.

Callback tests verify exact sample boundaries, non-finite handling, unsigned equilibrium, malformed/oversized buffers, recovery, empty callbacks and cleared scratch. Allocation instrumentation observes zero allocations/frees in F32, I16 and U16 normal/fault processing. See [P0 validation](P0_VALIDATION.md) for workspace results and commands.

- 🟠 [Next] Physical rate/wake-up observations, bounded variable-period handling, broader encodings/resource budgets, serialized device recovery and automatic fault handling.
- ⭕️ [Pending] Actual macOS/Windows/Linux device matrices, unplug/default-device changes, loopback/listening, physical latency and endurance. Software fixtures do not establish real-driver behavior.
