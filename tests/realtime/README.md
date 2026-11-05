# Realtime test boundary

The executable realtime-path tests live beside their owning crates so `cargo test --workspace`
runs them on every platform. Device-independent integration coverage uses `MockAudioBackend` in
`crates/daw-app`; physical CPAL output is intentionally a manual host test.

