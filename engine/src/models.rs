use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use parking_lot::Mutex as PluginHostMutex;

use crate::hosts;
pub use crate::render::util::{RenderFormat, RenderTailMode, RenderWavBitDepth};

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq, Default)]
pub enum AudioStretchMode {
    #[default]
    Stretch,
    StretchFormant,
    StretchNeutral,
    StretchVocal,
    Speed,
}

pub const DRUM_MACHINE_PAD_COUNT: usize = 32;
pub const DRUM_MACHINE_BANK_SIZE: usize = 16;
pub const DRUM_MACHINE_BASE_NOTE: u8 = 36;
pub const DRUM_MACHINE_OUTPUT_PAIRS: usize = 16;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
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

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Clip {
    pub id: usize,
    pub track: usize,
    pub start_beats: f32,
    pub length_beats: f32,
    pub is_midi: bool,
    #[serde(default)]
    pub midi_notes: Vec<PianoRollNote>,
    #[serde(default)]
    pub midi_source_beats: Option<f32>,
    #[serde(default)]
    pub link_id: Option<usize>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub audio_path: Option<String>,
    #[serde(default)]
    pub audio_source_beats: Option<f32>,
    #[serde(default)]
    pub audio_offset_beats: f32,
    #[serde(default)]
    pub audio_gain: f32,
    #[serde(default)]
    pub audio_pitch_semitones: f32,
    #[serde(default)]
    pub audio_stretch_mode: AudioStretchMode,
    #[serde(default)]
    pub audio_time_mul: f32,
    #[serde(default)]
    pub audio_key: Option<u8>,
    #[serde(default)]
    pub audio_key_minor: bool,
    #[serde(default)]
    pub audio_key_source: Option<u8>,
    #[serde(default)]
    pub audio_bpm: Option<f32>,
    #[serde(default)]
    pub audio_fine_pitch_cents: f32,
    #[serde(default = "default_formant_scale")]
    pub audio_formant_scale: f32,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DrumMachinePad {
    pub name: String,
    pub path: Option<String>,
    pub root_note: u8,
    pub gain: f32,
    pub pan: f32,
    pub pitch_semitones: f32,
    pub attack_ms: f32,
    pub decay_ms: f32,
    pub sustain: f32,
    pub release_ms: f32,
    pub cutoff: f32,
    pub resonance: f32,
    pub output_pair: usize,
    pub sensitivity: f32,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DrumMachineState {
    pub pads: Vec<DrumMachinePad>,
    pub bank: usize,
    pub selected_pad: usize,
    pub gain: f32,
    pub pan: f32,
    pub cutoff: f32,
    pub resonance: f32,
}

impl Default for DrumMachineState {
    fn default() -> Self {
        let mut pads = Vec::with_capacity(DRUM_MACHINE_PAD_COUNT);
        for index in 0..DRUM_MACHINE_PAD_COUNT {
            let bank = index / DRUM_MACHINE_BANK_SIZE;
            let slot = index % DRUM_MACHINE_BANK_SIZE;
            let label = if bank == 0 { "A" } else { "B" };
            pads.push(DrumMachinePad {
                name: format!("{}{}", label, slot + 1),
                path: None,
                root_note: DRUM_MACHINE_BASE_NOTE.saturating_add(index as u8),
                gain: 1.0,
                pan: 0.0,
                pitch_semitones: 0.0,
                attack_ms: 2.0,
                decay_ms: 60.0,
                sustain: 1.0,
                release_ms: 80.0,
                cutoff: 1.0,
                resonance: 0.0,
                output_pair: 0,
                sensitivity: 1.0,
            });
        }
        Self {
            pads,
            bank: 0,
            selected_pad: 0,
            gain: 1.0,
            pan: 0.0,
            cutoff: 1.0,
            resonance: 0.0,
        }
    }
}

pub fn default_formant_scale() -> f32 {
    1.0
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, Default)]
pub struct Track {
    pub name: String,
    pub clips: Vec<Clip>,
    pub level: f32,
    pub muted: bool,
    pub solo: bool,
    #[serde(default)]
    pub output_pair_mix: Vec<TrackMixState>,
    pub midi_notes: Vec<PianoRollNote>,
    pub instrument_path: Option<String>,
    #[serde(default)]
    pub instrument_clap_id: Option<String>,
    pub effect_paths: Vec<String>,
    #[serde(default)]
    pub effect_clap_ids: Vec<Option<String>>,
    #[serde(default)]
    pub effect_bypass: Vec<bool>,
    #[serde(default)]
    pub effect_params: Vec<Vec<String>>,
    #[serde(default)]
    pub effect_param_ids: Vec<Vec<u32>>,
    #[serde(default)]
    pub effect_param_values: Vec<Vec<f32>>,
    pub params: Vec<String>,
    #[serde(default)]
    pub param_ids: Vec<u32>,
    #[serde(default)]
    pub param_values: Vec<f32>,
    #[serde(default)]
    pub plugin_state_component: Option<Vec<u8>>,
    #[serde(default)]
    pub plugin_state_controller: Option<Vec<u8>>,
    #[serde(default)]
    pub automation_lanes: Vec<AutomationLane>,
    pub automation_channels: Vec<String>,
    #[serde(default)]
    pub midi_cc_lanes: Vec<MidiCcLane>,
    #[serde(default)]
    pub midi_program: Option<u8>,
    #[serde(default)]
    pub treesynth: Option<TreeSynthState>,
    #[serde(default)]
    pub drum_machine: Option<DrumMachineState>,
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq, Default)]
pub enum TreeSynthMode {
    #[default]
    Random,
    Layer,
    Sequential,
    Morph,
    Reorder,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, Debug)]
pub struct TreeSynthSample {
    pub path: String,
    pub name: String,
    pub root_note: u8,
    pub gain: f32,
    pub pan: f32,
    pub start: f32,
    pub end: f32,
    pub color: [u8; 3],
}

#[derive(Clone, serde::Serialize, serde::Deserialize, Debug)]
pub struct TreeSynthState {
    pub folder: Option<String>,
    pub samples: Vec<TreeSynthSample>,
    pub mode: TreeSynthMode,
    pub morph: f32,
    pub reorder: f32,
    pub gain: f32,
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
    pub vibrato_rate: f32,
    pub vibrato_depth: f32,
    pub tremolo_rate: f32,
    pub tremolo_depth: f32,
    pub reverb_mix: f32,
    pub pitch_bend_range: f32,
    pub portamento_ms: f32,
    pub legato: bool,
}

impl Default for TreeSynthState {
    fn default() -> Self {
        Self {
            folder: None,
            samples: Vec::new(),
            mode: TreeSynthMode::Random,
            morph: 0.0,
            reorder: 0.0,
            gain: 1.0,
            attack: 0.005,
            decay: 0.12,
            sustain: 0.75,
            release: 0.25,
            vibrato_rate: 5.0,
            vibrato_depth: 0.0,
            tremolo_rate: 5.0,
            tremolo_depth: 0.0,
            reverb_mix: 0.1,
            pitch_bend_range: 2.0,
            portamento_ms: 0.0,
            legato: false,
        }
    }
}

/// JSON preset bundle saved by the app (VST3 + optional TreeSynth state).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Vst3PresetFile {
    pub version: u32,
    pub name: String,
    pub plugin: String,
    pub param_names: Vec<String>,
    pub param_ids: Vec<u32>,
    pub param_values: Vec<f32>,
    pub component_state: String,
    pub controller_state: String,
    #[serde(default)]
    pub treesynth: Option<TreeSynthState>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, Default)]
pub struct AiScorePromptEntry {
    pub prompt: String,
    pub at_beats: f32,
    #[serde(default)]
    pub response: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct ProjectState {
    pub name: String,
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub album: String,
    #[serde(default)]
    pub genre: String,
    #[serde(default)]
    pub year: String,
    #[serde(default)]
    pub comment: String,
    #[serde(default)]
    pub project_key: Option<u8>,
    #[serde(default)]
    pub project_key_minor: bool,
    pub tempo_bpm: f32,
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub ai_score_journal: Vec<AiScorePromptEntry>,
    #[serde(default)]
    pub node_routes: Vec<NodeRouteLink>,
    #[serde(default)]
    pub performance_clip_settings: HashMap<usize, PerformanceClipSettings>,
    #[serde(default)]
    pub master_settings: MasterCompSettings,
    #[serde(default = "default_performance_launch_quantize_beats")]
    pub performance_launch_quantize_beats: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingParamTarget {
    Instrument,
    Effect(usize),
}

#[derive(Clone, Copy, Debug)]
pub struct PendingParamChange {
    pub target: PendingParamTarget,
    pub param_id: u32,
    pub value: f64,
}

#[derive(Clone, Debug)]
pub struct AudioClipRender {
    pub clip_id: usize,
    pub path: String,
    pub track_index: usize,
    pub start_samples: u64,
    pub length_samples: u64,
    pub offset_samples: u64,
    pub gain: f32,
    pub time_mul: f32,
    pub pitch_semitones: f32,
    pub stretch_mode: AudioStretchMode,
    pub formant_scale: f32,
}

impl AudioClipRender {
    pub fn from_clip(clip: &Clip, track_index: usize) -> Self {
        Self {
            clip_id: clip.id,
            path: clip.audio_path.clone().unwrap_or_default(),
            track_index,
            start_samples: 0,  // Needs calculation in context
            length_samples: 0, // Needs calculation in context
            offset_samples: 0, // Needs calculation in context
            gain: clip.audio_gain,
            time_mul: clip.audio_time_mul,
            pitch_semitones: clip.audio_pitch_semitones,
            stretch_mode: clip.audio_stretch_mode,
            formant_scale: clip.audio_formant_scale,
        }
    }
}

pub fn default_performance_launch_quantize_beats() -> f32 {
    1.0
}

/// True when two beat positions refer to the same performance scene marker.
#[inline]
pub fn performance_scene_matches(a_beats: f32, b_beats: f32) -> bool {
    const EPS_BEATS: f32 = 1e-3;
    (a_beats - b_beats).abs() <= EPS_BEATS
}

#[derive(Clone, serde::Serialize, serde::Deserialize, Debug)]
pub struct AutomationPoint {
    pub beat: f32,
    pub value: f32,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, Debug)]
pub struct AutomationLane {
    pub name: String,
    pub param_id: u32,
    #[serde(default)]
    pub target: AutomationTarget,
    pub points: Vec<AutomationPoint>,
}

impl AutomationLane {
    pub fn value_at(&self, beat: f32) -> Option<f32> {
        if self.points.is_empty() {
            return None;
        }

        // Binary search for the segment containing 'beat'
        let idx = self.points.partition_point(|p| p.beat < beat);

        if idx == 0 {
            return Some(self.points[0].value);
        }

        if idx >= self.points.len() {
            return Some(self.points[self.points.len() - 1].value);
        }

        let prev = &self.points[idx - 1];
        let next = &self.points[idx];

        let span = (next.beat - prev.beat).max(0.0001);
        let t = ((beat - prev.beat) / span).clamp(0.0, 1.0);

        Some(prev.value + (next.value - prev.value) * t)
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize, Debug)]
pub struct MidiCcLane {
    pub cc: u8,
    pub points: Vec<AutomationPoint>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum AutomationTarget {
    #[default]
    Instrument,
    Effect(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum NodeRouteKind {
    #[default]
    AudioSidechain,
    MidiToFx,
    AudioSend,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, Default)]
pub struct NodeRouteLink {
    pub from_track: usize,
    #[serde(default)]
    pub source_output_pair: usize,
    pub to_track: usize,
    pub to_fx: Option<usize>,
    pub kind: NodeRouteKind,
    pub enabled: bool,
    #[serde(default = "default_sidechain_amount")]
    pub sidechain_amount: f32,
    #[serde(default = "default_sidechain_attack_ms")]
    pub sidechain_attack_ms: f32,
    #[serde(default = "default_sidechain_release_ms")]
    pub sidechain_release_ms: f32,
    #[serde(default = "default_sidechain_threshold_db")]
    pub sidechain_threshold_db: f32,
}

pub fn default_sidechain_amount() -> f32 {
    0.7
}

pub fn default_sidechain_attack_ms() -> f32 {
    8.0
}

pub fn default_sidechain_release_ms() -> f32 {
    180.0
}

pub fn default_sidechain_threshold_db() -> f32 {
    -30.0
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq, Default)]
pub enum PerformanceTriggerMode {
    #[default]
    Gate,
    Toggle,
    OneShot,
    Loop,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, Default)]
pub struct PerformanceClipSettings {
    #[serde(default)]
    pub trigger_mode: PerformanceTriggerMode,
    #[serde(default)]
    pub loop_enabled: bool,
    #[serde(default)]
    pub auto_follow: bool,
    #[serde(default)]
    pub next_clip_id: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct ActivePerformanceTake {
    pub source_clip_id: usize,
    pub start_beat: f32,
    pub trigger_mode: PerformanceTriggerMode,
    pub loop_enabled: bool,
}

#[derive(Clone, Debug)]
pub struct RecordedPerformanceTake {
    pub track_index: usize,
    pub source_clip_id: usize,
    pub start_beat: f32,
    pub end_beat: f32,
    pub loop_enabled: bool,
}

#[derive(Clone, Debug)]
pub struct RecordedAutomationPoint {
    pub param_id: u32,
    pub target: AutomationTarget,
    pub beat: f32,
    pub value: f32,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, Debug)]
pub struct MasterCompSettings {
    pub enabled: bool,
    pub threshold_db: f32,
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub makeup_db: f32,
    pub level: f32,
}

impl Default for MasterCompSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold_db: -18.0,
            ratio: 2.0,
            attack_ms: 10.0,
            release_ms: 120.0,
            makeup_db: 0.0,
            level: 1.0,
        }
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct SettingsState {
    pub output_device: String,
    #[serde(default)]
    pub input_device: String,
    pub buffer_size: u32,
    pub sample_rate: u32,
    pub interpolation: String,
    pub midi_input: String,
    #[serde(default)]
    pub theme: String,
    #[serde(default)]
    pub key_display_format: String,
    #[serde(default)]
    pub device_id: String,
    #[serde(default)]
    pub device_salt: String,
    #[serde(default)]
    pub auth_token: String,
    #[serde(default)]
    pub registered_to: String,
    #[serde(default)]
    pub license_file: String,
    #[serde(default)]
    pub license_monthly_activations: Option<u64>,
    #[serde(default)]
    pub license_remaining_activations: Option<u64>,
    #[serde(default)]
    pub triple_buffer: bool,
    #[serde(default)]
    pub safe_underruns: bool,
    #[serde(default)]
    pub adaptive_buffer: bool,
    #[serde(default)]
    pub smart_disable_plugins: bool,
    #[serde(default)]
    pub smart_suspend_tracks: bool,
    #[serde(default)]
    pub recent_projects: Vec<String>,
    #[serde(default)]
    pub autosave_minutes: u32,
    #[serde(default)]
    pub load_last_project: bool,
    #[serde(default = "default_startup_sound")]
    pub play_startup_sound: bool,
    #[serde(default)]
    pub browser_folders: Vec<String>,
    #[serde(default = "default_show_clip_labels")]
    pub show_clip_labels: bool,
    #[serde(default)]
    pub midi_devices: Vec<MidiDeviceConfig>,
    #[serde(default)]
    pub wallpaper_path: String,
    #[serde(default = "default_wallpaper_opacity")]
    pub wallpaper_opacity: f32,
}

pub fn default_startup_sound() -> bool {
    true
}

pub fn default_show_clip_labels() -> bool {
    true
}

pub fn default_wallpaper_opacity() -> f32 {
    0.18
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum MidiDeviceProfile {
    #[default]
    Keyboard,
    Launchpad,
    Apc,
    PadController,
    ControlSurface,
    Generic,
}

impl MidiDeviceProfile {
    pub fn label(self) -> &'static str {
        match self {
            MidiDeviceProfile::Keyboard => "Keyboard",
            MidiDeviceProfile::Launchpad => "Launchpad",
            MidiDeviceProfile::Apc => "APC",
            MidiDeviceProfile::PadController => "Pad controller",
            MidiDeviceProfile::ControlSurface => "Control surface",
            MidiDeviceProfile::Generic => "Generic",
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, Default)]
pub struct MidiDeviceConfig {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub profile: MidiDeviceProfile,
    #[serde(default = "default_enabled_true")]
    pub enabled: bool,
    #[serde(default)]
    pub input_port: String,
    #[serde(default)]
    pub output_port: String,
    #[serde(default)]
    pub midi_channel: u8,
}

impl MidiDeviceConfig {
    pub fn display_name(&self) -> String {
        if !self.name.trim().is_empty() {
            return self.name.clone();
        }
        if !self.input_port.trim().is_empty() {
            return self.input_port.clone();
        }
        "MIDI device".to_string()
    }
}

pub fn default_enabled_true() -> bool {
    true
}

#[derive(Clone, Copy, Debug)]
pub enum PluginTarget {
    Instrument(usize),
    Effect(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginKind {
    Native,
    Vst3,
    Clap,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginCategory {
    Native,
    Bundled,
    System,
}

impl PluginCategory {
    pub fn label(self) -> &'static str {
        match self {
            PluginCategory::Native => "Native",
            PluginCategory::Bundled => "Bundled",
            PluginCategory::System => "System",
        }
    }
}

#[derive(Clone, Debug)]
pub struct PluginCandidate {
    pub path: String,
    pub kind: PluginKind,
    pub clap_id: Option<String>,
    pub display: String,
    pub category: PluginCategory,
    pub instrument_only: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginUiTarget {
    Instrument(usize),
    Effect(usize, usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectAction {
    NewProject,
    OpenProject,
    OpenProjectPath(String),
    ImportMidi,
    NewFromTemplate(String),
}

#[derive(Clone, Copy, Debug)]
pub enum GmCategory {
    Piano,
    Chromatic,
    Organ,
    Guitar,
    Bass,
    Strings,
    Ensemble,
    Brass,
    Reed,
    Pipe,
    SynthLead,
    SynthPad,
    SynthFx,
    Ethnic,
    Percussive,
    SoundFx,
}

impl GmCategory {
    pub fn from_program(program: u8) -> Self {
        match program {
            0..=7 => GmCategory::Piano,
            8..=15 => GmCategory::Chromatic,
            16..=23 => GmCategory::Organ,
            24..=31 => GmCategory::Guitar,
            32..=39 => GmCategory::Bass,
            40..=47 => GmCategory::Strings,
            48..=55 => GmCategory::Ensemble,
            56..=63 => GmCategory::Brass,
            64..=71 => GmCategory::Reed,
            72..=79 => GmCategory::Pipe,
            80..=87 => GmCategory::SynthLead,
            88..=95 => GmCategory::SynthPad,
            96..=103 => GmCategory::SynthFx,
            104..=111 => GmCategory::Ethnic,
            112..=119 => GmCategory::Percussive,
            _ => GmCategory::SoundFx,
        }
    }
}

#[derive(Clone)]
pub struct GmParamValues {
    pub gain: f32,
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
    pub cutoff: f32,
    pub resonance: f32,
    pub vibrato_rate: f32,
    pub vibrato_intensity: f32,
    pub tremolo_rate: f32,
    pub tremolo_intensity: f32,
}

impl GmParamValues {
    pub fn from_category(_category: GmCategory) -> Self {
        Self {
            gain: 0.85,
            attack: 0.02,
            decay: 0.18,
            sustain: 0.78,
            release: 0.22,
            cutoff: 0.62,
            resonance: 0.18,
            vibrato_rate: 0.35,
            vibrato_intensity: 0.12,
            tremolo_rate: 0.22,
            tremolo_intensity: 0.06,
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TrackMixState {
    pub muted: bool,
    pub solo: bool,
    pub level: f32,
    pub active: bool,
}

impl Default for TrackMixState {
    fn default() -> Self {
        Self {
            muted: false,
            solo: false,
            level: 1.0,
            active: true,
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct MasterCompState {
    pub gain: f32,
}

impl Default for MasterCompState {
    fn default() -> Self {
        Self { gain: 1.0 }
    }
}

#[derive(Clone)]
pub enum PluginHostHandle {
    Vst3(Arc<PluginHostMutex<hosts::vst3::Vst3Host>>),
    Clap(Arc<PluginHostMutex<hosts::clap::ClapHost>>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipDragKind {
    Move,
    ResizeStart,
    ResizeEnd,
    TrimStart,
    TrimEnd,
}

pub enum PluginUiEditor {
    Vst3(hosts::vst3::Vst3Editor),
    Clap(Arc<PluginHostMutex<hosts::clap::ClapHost>>),
}

pub struct PluginUiHost {
    pub hwnd: isize,
    pub child_hwnd: isize,
    pub editor: PluginUiEditor,
    pub host: PluginHostHandle,
    pub target: PluginUiTarget,
    pub close_requested: Arc<AtomicBool>,
    pub floating: bool,
}

#[derive(Clone, Debug)]
pub struct LicenseJob {
    pub finished: Arc<AtomicBool>,
    pub result: Arc<Mutex<Option<LicenseJobResult>>>,
}

#[derive(Clone, Debug)]
pub struct LicenseJobResult {
    pub status: Result<String, String>,
    pub token: Option<String>,
    pub license_file: Option<String>,
    pub registered_to: Option<String>,
    pub remaining_activations: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct LicensePayloadInfo {
    pub registered_to: Option<String>,
    pub license_type: Option<String>,
    pub max_activations: Option<u64>,
    pub monthly_activations: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct RenderJob {
    pub done: Arc<AtomicU64>,
    pub total: Arc<AtomicU64>,
    pub finished: Arc<AtomicBool>,
    pub result: Arc<Mutex<Option<Result<String, String>>>>,
}

#[derive(Clone, Debug, Default)]
pub struct AudioAnalysis {
    pub bpm: Option<f32>,
    pub key: Option<(u8, bool)>,
    pub fine_pitch_cents: Option<f32>,
}

pub struct AudioAnalysisRequest {
    pub clip_id: usize,
    pub path: PathBuf,
}

pub struct AudioAnalysisResult {
    pub clip_id: usize,
    pub path: PathBuf,
    pub analysis: Option<AudioAnalysis>,
}

pub struct RecordingBuffers {
    pub active: bool,
    pub track_index: usize,
    pub start_samples: u64,
    pub start_beats: f32,
    pub record_audio: bool,
    pub record_midi: bool,
    pub record_automation: bool,
    pub record_performance: bool,
    pub audio_samples: Vec<f32>,
    pub audio_channels: usize,
    pub audio_sample_rate: u32,
    pub midi_active: HashMap<u8, (f32, u8)>,
    pub midi_notes: Vec<PianoRollNote>,
    pub automation_points: Vec<RecordedAutomationPoint>,
    pub performance_active: HashMap<usize, ActivePerformanceTake>,
    pub performance_takes: Vec<RecordedPerformanceTake>,
}

impl Default for RecordingBuffers {
    fn default() -> Self {
        Self::new()
    }
}

impl RecordingBuffers {
    pub fn new() -> Self {
        Self {
            active: false,
            track_index: 0,
            start_samples: 0,
            start_beats: 0.0,
            record_audio: false,
            record_midi: false,
            record_automation: false,
            record_performance: false,
            audio_samples: Vec::new(),
            audio_channels: 0,
            audio_sample_rate: 44_100,
            midi_active: HashMap::new(),
            midi_notes: Vec::new(),
            automation_points: Vec::new(),
            performance_active: HashMap::new(),
            performance_takes: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct UndoState {
    pub project_name: String,
    pub tempo_bpm: f32,
    pub tracks: Vec<Track>,
    pub selected_clip: Option<usize>,
    pub selected_track: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct ClipDragGroupItem {
    pub clip_id: usize,
    pub source_track: usize,
    pub start_beats: f32,
    pub length_beats: f32,
    pub is_midi: bool,
}

#[derive(Clone, Debug)]
pub struct ClipDragState {
    pub clip_id: usize,
    pub source_track: usize,
    pub origin_track: usize,
    pub offset_beats: f32,
    pub start_beats: f32,
    pub length_beats: f32,
    pub origin_start_beats: f32,
    pub origin_length_beats: f32,
    pub audio_offset_beats: f32,
    pub audio_source_beats: Option<f32>,
    pub kind: ClipDragKind,
    pub undo_pushed: bool,
    pub grabbed: bool,
    pub copy_mode: bool,
    pub group: Option<Vec<ClipDragGroupItem>>,
}

#[derive(Clone, Debug)]
pub struct TrackDragState {
    pub track_index: usize,
    pub origin_index: usize,
    pub offset_y: f32,
    pub source_index: usize,
}

pub struct FsEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MidiImportMode {
    ReplaceProject,
    AppendTracks { start_beats: f32 },
}

pub struct CallbackGuard {
    active: Arc<AtomicUsize>,
}

impl CallbackGuard {
    pub fn new(active: Arc<AtomicUsize>) -> Self {
        active.fetch_add(1, Ordering::SeqCst);
        Self { active }
    }
}

impl Drop for CallbackGuard {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrangerTool {
    Draw,
    Select,
    Move,
    Slice,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoSliceMode {
    Smart,
    Bar,
    Phrase,
}

impl AutoSliceMode {
    pub fn label(self) -> &'static str {
        match self {
            AutoSliceMode::Smart => "Smart Sections",
            AutoSliceMode::Bar => "By Bar",
            AutoSliceMode::Phrase => "By Phrase",
        }
    }

    pub fn interval_beats(self) -> f32 {
        match self {
            AutoSliceMode::Smart => 16.0,
            AutoSliceMode::Bar => 4.0,
            AutoSliceMode::Phrase => 16.0,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct PerformanceSectionAnalysis {
    pub start_beats: f32,
    pub length_beats: f32,
    pub loop_unit_beats: Option<f32>,
}

#[derive(Clone, Debug, Default)]
pub struct AutoPerformanceBuildSummary {
    pub sections: usize,
    pub slices_created: usize,
    pub configured_clips: usize,
    pub loop_clips: usize,
}

impl AutoPerformanceBuildSummary {
    pub fn changed(&self) -> bool {
        self.slices_created > 0 || self.configured_clips > 0
    }

    pub fn status_message(&self) -> String {
        format!(
            "Smart performance layout built {} section(s), created {} slice(s), configured {} clip(s), and enabled {} loop pad(s)",
            self.sections,
            self.slices_created,
            self.configured_clips,
            self.loop_clips,
        )
    }
}

#[derive(Clone, Debug)]
pub struct ArrangerDrawState {
    pub track_index: usize,
    pub start_beats: f32,
    pub start_pos: [f32; 2],
}

#[derive(Clone, Copy, Debug)]
pub struct ArrangerSliceDragState {
    pub beat: f32,
    pub start_track: usize,
    pub end_track: usize,
    pub free_snap: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PianoDragKind {
    Move,
    Resize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PianoTool {
    Pencil,
    Select,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PianoLaneMode {
    Velocity,
    Pan,
    Cutoff,
    Resonance,
    MidiCc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MainTab {
    Arranger,
    Parameters,
    PianoRoll,
    NodeEditor,
    Performance,
    AiScores,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsTab {
    General,
    Audio,
    Midi,
    Devices,
    Theme,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarTab {
    Project,
    Browser,
}

pub struct PianoDragState {
    pub track_index: usize,
    pub note_index: usize,
    pub kind: PianoDragKind,
    pub offset_beats: f32,
    pub start_beats: f32,
    pub start_length: f32,
    pub start_pitch: u8,
    pub start_pos_y: f32,
    pub selected_notes: Vec<(usize, f32, u8, f32)>,
}

pub struct PianoScaleDragState {
    pub track_index: usize,
    pub anchor_start: f32,
    pub anchor_end: f32,
    pub selected_notes: Vec<(usize, f32, u8, f32)>,
}

pub struct PianoZoomDragState {
    pub start_pos: [f32; 2],
    pub start_zoom_x: f32,
    pub start_zoom_y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FsSource {
    Project,
    Browser,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FsDragKind {
    Audio,
    Midi,
}

pub struct FsDragState {
    pub path: PathBuf,
    pub kind: FsDragKind,
}

pub struct MidiImportState {
    pub path: String,
    pub tracks: Vec<crate::midi::MidiTrackData>,
    pub enabled: Vec<bool>,
    pub apply_program: Vec<bool>,
    pub instrument_plugin: String,
    pub percussion_plugin: String,
    pub import_portamento: bool,
    pub mode: MidiImportMode,
}
