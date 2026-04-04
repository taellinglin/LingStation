#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TrackNodeActivity {
    pub output_pair_peaks: [f32; 8],
    pub fx_input_peaks: Vec<f32>,
    pub fx_output_peaks: Vec<f32>,
    pub midi_in: f32,
    pub midi_out: f32,
}

impl Default for TrackNodeActivity {
    fn default() -> Self {
        Self {
            output_pair_peaks: [0.0; 8],
            fx_input_peaks: Vec::new(),
            fx_output_peaks: Vec::new(),
            midi_in: 0.0,
            midi_out: 0.0,
        }
    }
}
