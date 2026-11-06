# Output observations and processing deadlines

Date: 2026-11-06. Implemented independently of Pro-Wave.

## Timing domains

Engine metrics measure engine processing. `processDeadlineMissCount` explicitly names its deadline counter; `xrunCount` remains a compatible alias. Neither is a measured driver underrun count.

The nested `output` metrics measure the CPAL adapter body: initial clearing, source processing, conversion/normalization and fault notifications. They exclude observer recording, driver scheduling, wake-up delay and hardware delivery. Engine work is nested inside adapter work: do not add the durations or treat their difference as exact conversion overhead. Output deadlines use delivered frames divided by the requested sample rate, not an independently measured device clock.

`callbackCount` includes empty and rejected callbacks. `periodCount` includes only positive, frame-aligned callbacks with valid configuration; average/maximum duration, weighted CPU load and deadline misses use those periods. A positive aligned capacity rejection is an observed period even though the source is skipped. Empty and malformed callbacks do not distort duration/load statistics. Until `periodCount` is positive, zero timing fields mean unobserved; the UI displays a dash.

`minimumFrames` and `maximumFrames` describe positive aligned delivered periods. `lastFrames` is null before observation or after invalid configuration/incomplete frames, zero after an empty callback, and the delivered frame count otherwise. `bufferMismatchCount` counts positive aligned deliveries differing from the requested fixed buffer. A mismatch is diagnostic, not proof of a driver fault or transparent variable-period support. No statistic measures physical latency or actual sample rate.

## Fault classification and compatibility

`driverErrorCount` counts backend/driver error notifications; it is not a hardware xrun measurement. `adapterErrorCount` counts typed malformed-frame, capacity-exceeded and non-finite-sample faults, with individual subtype counters. Malformed frames take precedence over capacity faults; non-finite samples produce one notification per affected block. Existing `backendErrorCount` aggregates both domains for compatibility. A legacy event sink receives typed faults through its existing error method unless it overrides the new hook.

The mock/uninstrumented path does not invent output observations. Browser preview returns `output: null` and hides output rows. English, Japanese and Simplified Chinese labels distinguish process deadlines, output-body deadlines and notification counts; tooltips explain their limits.

## Realtime ownership and limits

Measurements use fixed atomic storage without allocation, deallocation, locks, resizing or I/O in the callback. Snapshots are live, independently sampled counters, not transactional audit receipts; adjacent fields may reflect adjacent callbacks. They never schedule audio or change musical state. Rust retains processing and statistics; JavaScript receives scalar projections only. Existing graph adoption, streaming, Transport, Undo/Redo and persistence regression gates remain applicable.

🟢 [Done] Separate process/output timing, delivered-frame observations, typed faults, compatible legacy counters and localized diagnostics with unknown-state handling.

🟠 [Next] Wake-up timestamps, actual driver xrun reporting, finer tail histograms, off-thread export, bounded variable-period recovery and device lifecycle handling.

⭕️ [Pending] Physical clock/latency, listening/loopback, real-driver variation, unplug/recovery and endurance qualification. Synthetic callback and browser fixtures do not qualify hardware.
