# Exact MIDI selection and cross-clip quantization

Date: 2026-11-08. Implemented independently; no Pro-Wave source or assets were read or reused.

## Command and timing contract

All three native musical quantization routes now use the same prepared constant-tempo rational grid:

| Command | Origin | Duration policy |
| --- | --- | --- |
| Legacy clip edit (`edit_midi_clip`, action `quantize`) | Clip-local zero | Existing endpoint clamp may shorten a note |
| Explicit selection (`edit_midi_selection`, `Quantize`) | Clip-local zero | Preserve duration, clamp start to the latest valid start |
| Composite cross-clip selection (`edit_midi_notes`, `Quantize`) | Project sample zero | Preserve duration and originating clip, clamp start within that clip |

Existing selection commands already accept a musical `division` (4, 8, 16 or 32), not an externally supplied integer sample period. This implementation corrects their internal timing calculation; their JSON/Rust command shapes do not change. The former current-contract text describing those APIs as explicit sample grids was inaccurate and is corrected. Low-level browser gesture helpers can still accept integer sample grids; that is a separate interface.

At 44.1 kHz / 120 BPM / sixteenths, the exact grid period is 5512.5 samples. A selected note at 88,200 stays at 88,200 instead of moving to 88,208. Cross-clip notes in clips beginning at 1,001 and 60,007 samples can both snap to project sample 88,200; their stored local offsets differ. This does not move them between clips.

The exact musical index is selected first, then that grid point is rounded to integer samples. Both ties go forward. The existing one-sample minimum and bounded `u128` arithmetic are described in [the musical clock contract](MUSICAL_CLOCK.md). No rounded step is repeatedly added. Clamp after snapping: a duration-preserving note that cannot reach a grid point within its clip stays on a legal edge, which need not itself lie on the grid. Repeating the edit is stable.

## Ownership and failure behavior

The control layer prepares and validates a grid once per command. Cross-clip editing validates composite identities and computes changes on a candidate project; duplicate/missing targets or unsupported divisions cannot leave a partial edit. Selected note IDs, key, channel, velocity and duration survive quantization; unselected notes remain unchanged. Notes are sorted using the existing `(start, id)` order. Each accepted edit is a single history transaction.

Only a newly requested quantization changes note positions. Loading old projects does not retime them, and the file schema is unchanged. Undo/Redo restores exact integer positions and publishes the corresponding graph revision. Save/load preserves the corrected positions. Whole-document load intentionally stops the current engine and discards its prior receipt state, following the existing lifecycle.

The new work runs entirely on the control thread. Callbacks continue to consume prepared schedules; there is no new callback allocation, lock, division or floating conversion. Browser preview prepares an exact BigInt equivalent within JavaScript's safe integer domain. Native playback and authoritative document state remain in Rust; no audio buffers cross the WebView boundary.

## Evidence and limits

🟢 [Done] Project tests cover aligned-note drift, two off-grid clip origins, distinct local offsets with a common absolute grid point, unchanged unselected notes/expression/duration, repeated edits, invalid-target/division atomicity and clip-edge clamping. An application test checks integer positions and document/adoption revisions through cross-clip Undo/Redo and storage. Preview tests cover all three quantization routes, history, storage and invalid-target rollback.

🟠 [Next] Tempo maps/tick-domain documents, inverse conversion, musical-time retiming policy for tempo edits and broader randomized interval coverage. Current grids use one constant project tempo; this does not implement tempo automation or groove/swing.

⭕️ [Pending] Physical device clock/latency, listening/loopback, external synchronization and endurance. Software note positions and adoption receipts do not certify DAC timing.
