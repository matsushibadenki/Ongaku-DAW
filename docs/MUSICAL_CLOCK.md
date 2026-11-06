# Constant-tempo musical clock

Date: 2026-11-06. Independently implemented without reading or reusing Pro-Wave code.

## Conversion contract

`MusicalPosition::to_sample` converts an absolute, zero-based bar/beat/tick position to a sample position. A beat uses the time signature's denominator unit; each such beat has 960 ticks. Out-of-range beat/tick values carry arithmetically instead of being clipped. Position conversion rounds once to the nearest sample, with exact halves rounding upward, then saturates at `u64::MAX`. A zero sample rate produces zero.

Compatibility rules are unchanged: finite tempo is clamped to 20–400 BPM; non-finite tempo uses 120 BPM; numerator zero uses one; a denominator that is not a power of two uses four. The exact binary `f64` tempo supplied by the document is preserved. A decimal spelling such as 123.456 is not silently interpreted as a new decimal-rational tempo or quantized to micro-BPM.

The implementation first forms the complete musical tick count in `u128`. It decomposes the normalized tempo into its integer significand and binary exponent, then evaluates the rational sample count with quotient/remainder doubling. This avoids both premature floating-point rounding and an overflowing shifted numerator. Public input widths bound the initial numerator below 2^91 and the divisor below 2^70; the clamped tempo needs at most 48 doubling steps. Saturation occurs before an excessive quotient can grow. There is no allocation, I/O, lock or dependency on audio callbacks.

A reproducible regression is bar 1,188,693,771, beat 4, tick 294 at 48 kHz, 400 BPM and 7/8. The correct sample is 29,955,083,044,703; the former floating calculation returned 29,955,083,044,702. The new result follows an independent exact rational oracle. This long-position regression establishes numeric correctness, not a claim about typical session duration.

## Integration and limits

Single-clip legacy quantization now prepares a `MusicalGrid` rational period (sample rate × 240 / BPM / subdivision). It selects the nearest exact musical subdivision, with ties forward, then rounds that individual grid position to the nearest sample, also with ties forward. Each position is computed directly rather than multiplying a rounded sample step. The origin remains the clip's start; this command does not move notes between clips or quantize against a new project-wide origin.

For 44.1 kHz, 120 BPM and sixteenth notes, the exact period is 5512.5 samples. A note already at sample 88,200 stays there, rather than moving to 88,208 as with the former 5513-sample repeated step. A separate boundary regression at 399.8889197445154 BPM snaps to 1800 samples rather than 1801. Repeating quantization is idempotent, including fractional periods and saturation.

The grid accepts power-of-two whole-note subdivisions (quarter = 4, sixteenth = 16), rejects zero rate/invalid subdivision, and keeps a one-sample minimum period. Normalization of tempo matches musical conversion. Grid numerator < 2^88 and denominator < 2^60; index selection and final position arithmetic fit within `u128` even for `u64::MAX` sample positions. All work occurs during control-thread edits, without allocation in the Rust grid operation or new callback work.

Note IDs, clip bounds and expression preservation retain their contracts; clipping at the end may shorten duration as before. Existing multi-note/cross-clip editing APIs retain their explicit integer sample grids. No stored sample positions or project schemas are migrated or silently retimed: only a newly requested musical quantize command uses the corrected policy. Undo/Redo and save/load preserve the resulting integer positions. Browser preview uses an exact BigInt control-model equivalent, bounded by JavaScript safe integer positions, and does not own native audio timing.

Sample ranges remain half-open (`start <= sample < end`), with nonempty construction. Existing loop tests compare sample positions against an independent `u128` oracle across callback partitions and near integer limits. This change does not alter loop wrapping, graph adoption, streaming readiness or the audio clock.

## Evidence and remaining work

🟢 [Done] Seven new clock tests cover 583,872 rational-oracle combinations, exact half-sample ties and adjacent binary tempos, decimal-tempo external fixtures, long-position precision, invalid-input compatibility, saturation, tick carry/monotonicity and half-open range edges. The oracle uses direct rational multiplication/division, independent of the production float decomposition and iterative division. Decimal fixtures are computed with Python `Fraction(float)`.

🟢 [Done] Exact single-clip musical grids: 46,080 distance-oracle/idempotence combinations, fractional-period and midpoint fixtures, invalid/minimum/extreme positions, project note preservation and application graph-adoption/history/storage regression coverage.

🟠 [Next] A tempo-map/tick-domain document model, exact absolute-position quantization across clips, inverse sample-to-musical conversion, tempo-map edit continuity and exhaustive clip/event interval ownership tests. Constant-tempo conversion alone does not complete the P0 clock oracle roadmap.

⭕️ [Pending] Physical device-clock drift, external sync, loopback timing and endurance qualification. Software sample positions do not establish DAC delivery time.
