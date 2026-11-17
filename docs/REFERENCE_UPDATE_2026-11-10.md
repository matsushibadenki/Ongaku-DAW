# Pro-Wave reference refresh — 2026-11-10

## Scope and provenance

Reviewed the user-supplied local reference at upstream HEAD `af32fce566ea93d127ed6b8ad29f82b41bea6ed7` (commit date 2026-11-09). Sources: its README, Unreleased and 0.2.2 changelog entries, roadmap status since 0.2.2, status overview, commit subjects and bundled Tracks/Mixer and Piano Roll screenshots. No application source, presets, audio, IRs or binary seeds were copied or incorporated. Pro-Wave was not built or run, and its test claims were not independently reproduced.

The local reference has staged deletions of its CI/release workflows and `.gitignore`; this review leaves them untouched. The upstream revision identifies documentation provenance, not a claim that the local tree is pristine or that those workflows ran here. The bundled screenshots were last committed on 2026-10-19: they illustrate the established workspace, not visual proof of every newly documented feature. There is no recorded hash for the earlier reference copy, so this is a review of the documented 0.2.2-to-Unreleased changes, not an exact diff against that earlier copy.

This record is self-contained and remains useful after the ignored `Pro-Wave/` directory is deleted. [Ongaku's roadmap](../ROADMAP.md) remains authoritative. Reference status never means implementation or qualification in Ongaku.

## Documented updates and independent implementation implications

| Reference reports | Important boundary | Ongaku plan |
| --- | --- | --- |
| Master-strip insert effects and convolution reverb controls | Master FX automation, user IR loading and tempo-synced predelay remain incomplete; its documented reverb latency is not an Ongaku latency contract | Define processor latency/tail/state ownership and impulse/PDC tests before adding master processing. Independently design effects and IR assets |
| Realtime mix/section/track bounce, bounce in place and bounce/join | Playback-speed capture, not a demonstrated offline engine; transport cancellation and prior-state restoration are part of the workflow | Shared render path with exact spans/frame counts, tail policy, isolated temporary render state, cancellation and atomic file/document commit |
| Send-level automation and editable mix groups | Send/routing latency and linked edits can affect multiple tracks | Stable parameter IDs; one history transaction for linked edits; smoothed sample-position parameters and send/bus compensation |
| Audio import from menu, MIDI SMPTE timing/markers/program preservation, MusicXML export | MusicXML import is unfinished; timing-domain conversion and feature preservation need explicit fixtures | Worker-based audio import; exact MIDI frame/tick conversion including 29.97 policy; preservation/unsupported-data reports, bounded parser tests |
| GarageBand `.band` audio extraction | Audio files only; tempo, signature and region placement are not reconstructed; zipped mobile projects require extraction in the reference | Separate media extraction from full project reconstruction; never describe extracted media as a faithfully imported arrangement |
| SoundFont and Hydrogen drum kits | External file paths, velocity layers and sample assets require resource, ownership and missing-media policies | Independent parsers, licensed/user media, bounded loading, cancellation, relink/portable-project behavior |
| Undo-history navigation, rename/reveal project and newly active menu entries | Displayed menus alone do not prove an operable transaction | Rust revision-backed history browser; correct dirty state and graph receipts on jumps; only expose enabled actions with working commands |
| Disk-worker load meter | Worker busy time is not storage latency, readiness, underruns or device xruns | Separate worker utilization, decode latency, page readiness and starvation; label observed values and unknown states honestly |
| Short-window mixer fixes | Bundled screenshots predate those fixes | Reserve essential strip controls, use explicit scrolling/minimum sizes, preserve selection context across panels; verify current UI at compact sizes |
| Denormal handling and MIDI note ownership fixes described in 0.2.2 | Linux software/virtual-port evidence is not real-device qualification or cross-platform proof | Qualify numerical tails/finite output and old-route Note Off ownership; retain physical testing as pending |

## Workspace observations

The reference screenshots show a persistent top transport/ruler, a left library and context inspector, a central arrangement, and a lower editor or mixer. The piano roll keeps time/scale quantization and velocity editing close to the notes. Channel strips group input/instrument, inserts, sends, output/group, automation, pan, meter/fader and mute/solo controls vertically.

Useful principles are stable control locations, selection-driven inspectors, direct access to common actions, and a lower work area that changes purpose without losing arrangement context. Ongaku should apply these principles through its own compact icon/tooltip system and English/Japanese/Simplified Chinese text. Rust must own document/workspace state; future docked/floating Tauri windows are projections, and heavy rendering stays in Rust/wgpu. No screenshot, artwork, widget implementation or effect-window design is transplanted.

## Priority and verification decisions

- 🟢 [Done] Reference documentation and two bundled screenshots reviewed; limitations and source provenance recorded; independent requirements reconciled into the roadmap. This status describes the review only.
- 🟠 [Next] Continue P0: device lifecycle and observable faults, numerical/tail robustness, MIDI controller/panic ownership, tempo-map timing and streaming resource/admission limits. Retain existing graph/history and callback allocation/free gates.
- 🔴 [Later] After applicable P0 gates: useful audio import/recording and reliable render/export; then master FX/sends/groups and production instruments. Require aligned impulses, tail continuity, cancellation and storage/history recovery for those additions.
- ⭕️ [Pending] Reference runtime behavior, current-platform visual behavior, physical playback/recording/loopback, external MIDI devices and cross-platform endurance. This documentation review does not qualify any of them.

No runtime change or new test result is claimed by this refresh. Existing software validation remains in [P0 validation](P0_VALIDATION.md).
