# LingStation DAW Boilerplate

This is a Rust workspace with a minimal audio engine crate and an egui GUI app.

## Crates
- engine: audio, MIDI, and VST host stubs
- app: egui GUI with a simple piano roll visualization

## Build
Run from the workspace root:

```
cargo build
```

## Run GUI
```
cargo run -p lingstation-app
```

## Notes
- VST hosting is a stub; add a VST3 dependency (for example, vst3-sys) and wire it in engine/src/vst.rs.
- MIDI import/export is a minimal SMF implementation for now.
- MIDI and audio backends are placeholders.
