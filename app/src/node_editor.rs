use super::*;

#[derive(Clone)]
pub(crate) struct TrackNodeActivity {
    pub(super) output_pair_peaks: [f32; 8],
    pub(super) fx_input_peaks: Vec<f32>,
    pub(super) fx_output_peaks: Vec<f32>,
    pub(super) midi_in: f32,
    pub(super) midi_out: f32,
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

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(crate) enum NodeRouteKind {
    #[default]
    AudioSidechain,
    MidiToFx,
    AudioSend,
}

pub(super) fn default_sidechain_amount() -> f32 {
    0.7
}

pub(super) fn default_sidechain_attack_ms() -> f32 {
    8.0
}

pub(super) fn default_sidechain_release_ms() -> f32 {
    180.0
}

pub(super) fn default_sidechain_threshold_db() -> f32 {
    -30.0
}

pub(super) fn default_route_output_pair() -> usize {
    0
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub(crate) struct NodeRouteLink {
    pub(super) from_track: usize,
    #[serde(default = "default_route_output_pair")]
    pub(super) source_output_pair: usize,
    pub(super) to_track: usize,
    pub(super) to_fx: Option<usize>,
    pub(super) kind: NodeRouteKind,
    pub(super) enabled: bool,
    #[serde(default = "default_sidechain_amount")]
    pub(super) sidechain_amount: f32,
    #[serde(default = "default_sidechain_attack_ms")]
    pub(super) sidechain_attack_ms: f32,
    #[serde(default = "default_sidechain_release_ms")]
    pub(super) sidechain_release_ms: f32,
    #[serde(default = "default_sidechain_threshold_db")]
    pub(super) sidechain_threshold_db: f32,
}