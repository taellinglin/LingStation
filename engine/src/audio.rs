pub struct AudioEngine {
    pub sample_rate: f32,
    pub channels: u16,
}

impl AudioEngine {
    pub fn new(sample_rate: f32, channels: u16) -> Self {
        Self {
            sample_rate,
            channels,
        }
    }

    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        if output.len() != input.len() {
            return;
        }
        output.copy_from_slice(input);
    }
}
