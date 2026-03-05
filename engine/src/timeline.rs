use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PianoRollNote {
    pub start_beats: f32,
    pub length_beats: f32,
    pub midi_note: u8,
    #[serde(default = "default_velocity")]
    pub velocity: u8,
    #[serde(default = "default_pan")]
    pub pan: f32,
    #[serde(default = "default_cutoff")]
    pub cutoff: f32,
    #[serde(default = "default_resonance")]
    pub resonance: f32,
}

fn default_velocity() -> u8 {
    100
}

fn default_pan() -> f32 {
    0.0
}

fn default_cutoff() -> f32 {
    0.5
}

fn default_resonance() -> f32 {
    0.0
}

impl PianoRollNote {
    pub fn new(start_beats: f32, length_beats: f32, midi_note: u8, velocity: u8) -> Self {
        Self {
            start_beats,
            length_beats,
            midi_note,
            velocity,
            pan: default_pan(),
            cutoff: default_cutoff(),
            resonance: default_resonance(),
        }
    }
}
