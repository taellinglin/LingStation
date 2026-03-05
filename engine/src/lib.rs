pub mod audio;
pub mod midi;
pub mod timeline;
pub mod vst;

pub struct DawEngine {
    pub audio: audio::AudioEngine,
    pub midi: midi::MidiEngine,
    pub vst: vst::VstHost,
}

impl DawEngine {
    pub fn new(sample_rate: f32, channels: u16) -> Self {
        Self {
            audio: audio::AudioEngine::new(sample_rate, channels),
            midi: midi::MidiEngine::new(),
            vst: vst::VstHost::new(),
        }
    }
}
