# Phase 1 Validation

Validated on 2026-08-24 using the macOS CPAL/CoreAudio path and the `外部ヘッドフォン`
output device.

## Automated verification

- [Done] `cargo test --release --workspace`
- [Done] `cargo clippy --workspace --all-targets -- -D warnings`
- [Done] `npm run build`
- [Done] `npm run tauri build`
- [Done] Realtime callback performs no allocation or free under the tracking allocator
- [Done] Mock backend renders a finite, non-silent 440 Hz signal
- [Done] 100,500 concurrent Gain updates do not block offline processing
- [Done] UI has no horizontal overflow at 320, 375, 414, or 768 CSS pixels
- [Done] English, Japanese, and Simplified Chinese fit at 320 CSS pixels

## Physical output observation

The app ran at the device's default 96 kHz sample rate with a 128-frame buffer. After 15,841
callbacks and 80 rapid UI Gain changes:

```text
average callback duration: 1 us
maximum callback duration: 8 us
callback deadline: 1,333 us
reported CPU load: 0.10%
xruns: 0
backend errors: 0
```

These figures are a development-machine observation, not a product performance guarantee.

## Roadmap

- [Done] Phase 1 realtime signal path and minimal control UI
- [Next] Phase 2 transport and immutable graph snapshot architecture
- [Later] Track, mixer, audio clip, and file-streaming implementation
