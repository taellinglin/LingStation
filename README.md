# LingStation

LingStation is a Rust-based DAW with an egui/eframe UI, VST3 hosting, a piano roll, and automation tools.

## Features
- Arranger timeline with looping, snapping, and clip previews
- Piano roll editor with note tools and lane editor (velocity, pan, cutoff, resonance, MIDI CC)
- Track mixer with per-track level/mute/solo and peak meters
- VST3 instrument and effect hosting with native editor windows
- MIDI import/export and automation recording
- Audio clip support with gain/pitch/time controls
- Render/export to WAV, OGG, and FLAC

## Build
From the workspace root:

```
cargo build
```

## Run
```
cargo run -p lingstation-app
```

## Notes
- The VST3 SDK is included as a submodule at `vst3sdk`.
- On Linux, you need ALSA dev packages (`libasound2-dev`) to build.
- The UI font and startup sound are embedded in the binary.
