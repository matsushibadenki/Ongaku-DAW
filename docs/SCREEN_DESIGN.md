# DAW Screen Design

## Direction

The application is a dense, window-fitted editing workbench. The supplied Studio One screenshot is the primary visual reference: slate chrome, blue/cyan/green clips, a lower piano roll, a full-height right browser, and bottom transport. The React view reads project and transport state through the existing Tauri adapter; it does not own audio timing. Browser-only development uses the existing preview adapter.

## Region map

```text
Application menu
Workspace bar: selected track / Inspector / Editor / Browser icons
┌───────────────────┬───────────────────────────────┬─────────────┐
│ Track headers     │ Arrangement timeline          │ Project     │
│ Name / M / S      │ blue / cyan / green clips     │ browser     │
│ Level / dB        │ waveform and MIDI notes       │ tracks      │
│ Pan               │                               │ clips       │
├───────────────────┼───────────────────────────────┤ search      │
│ Note inspector    │ Piano roll / Automation tabs  │             │
│ Note / Velocity   │ keyboard + notes + velocity   │ selection   │
└───────────────────┴───────────────────────────────┴─────────────┘
Transport: session format / position / playback / tempo / meter / engine
Compact status
```

The optional track Inspector, detail editor, and project browser can be shown independently. Selecting a track in the arrangement or browser updates the shared selection. The browser lists actual project content, not placeholder plugin catalogs. An audio track uses a concise detail summary until an audio clip editor is implemented; MIDI tracks can switch between Piano Roll and Automation tabs.

## Design principles

- Preserve one-to-one alignment between track headers and timeline lanes.
- Use the existing horizontal MiniVolume and MiniSlider primitives in track rows and Inspector.
- Keep color concentrated in clip identities, the playhead, active state, and numeric level.
- Keep menus and settings in the application menu, away from the transport readouts.
- The page itself does not scroll. Arrangement and browser contents scroll within their panels when necessary.
- At widths up to 620px, the browser becomes a dismissible overlay, the note inspector is hidden, and transport uses two compact rows. The browser starts closed on these widths.
- Track lanes share one CSS height with Canvas hit-testing and drawing: 64px normally, 44px when window height is at most 600px.
- Text is localized in English, Japanese, and Simplified Chinese.

## Implementation boundary

This stage redesigns the UI shell and selected-track workflow. Shared MiniSlider/MiniVolume controls, a separate StudioBrowser component, and centralized color tokens remain reusable. Recording, clip editing, project persistence, and plugin hosting remain engine and product milestones, not visual-only controls. Timeline and MIDI rendering still use the existing WebView Canvas implementation; Rust/wgpu rendering and Rust-owned workspace state have not been implemented by this visual redesign. Panel toggles are not native docking or floating windows.

## Progress

- 🟢 [Done] Reference-led slate workspace, bottom transport, colored clips, keyboard/velocity visualization, aligned three-row track headers, independent panel toggles, searchable project browser, and three-language labels.
- 🟢 [Done] Browser-preview checks: track selection and reorder, browser search/toggle, editor tabs, and window-fitted layouts at desktop, 810×450, 375×667, and 320×568 sizes.
- 🟠 [Next] Rust-owned Document/State/Renderer boundaries, wgpu timeline rendering, and a reusable dock/floating-panel model.
- 🟠 [Next] Audio clip detail editing, timeline zoom/navigation, and persistent panel sizing.
- 🔴 [Later] Recording workbench and plugin chain.
- ⭕️ [Pending] Native Tauri window, audio-device behavior, and multi-window validation; browser preview is not native validation.

## Interface references

- User-provided `studio_one_beginner_main.png`: primary layout, density, and color reference for this revision. The application retains its own identity and only exposes implemented actions.
- [Ableton Live 12 Arrangement View](https://www.ableton.com/en/manual/arrangement-view/) documents configurable arrangement track controls and a collapsible mixer.
- [Logic Pro main window](https://support.apple.com/guide/logicpro/main-window-interface-lgcpe9cc403a/10.7/mac/11.0) documents a persistent tracks area, Inspector, toolbar, and editors.
