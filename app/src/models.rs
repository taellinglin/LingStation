use super::*;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) enum AudioStretchMode {
    Stretch,
    StretchFormant,
    StretchNeutral,
    StretchVocal,
    Speed,
}

impl Default for AudioStretchMode {
    fn default() -> Self {
        Self::Stretch
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Clip {
    pub(crate) id:  usize,
    pub(crate) track:  usize,
    pub(crate) start_beats:  f32,
    pub(crate) length_beats:  f32,
    pub(crate) is_midi:  bool,
    #[serde(default)]
    pub(crate) midi_notes:  Vec<PianoRollNote>,
    #[serde(default)]
    pub(crate) midi_source_beats:  Option<f32>,
    #[serde(default)]
    pub(crate) link_id:  Option<usize>,
    #[serde(default)]
    pub(crate) name:  String,
    #[serde(default)]
    pub(crate) audio_path:  Option<String>,
    #[serde(default)]
    pub(crate) audio_source_beats:  Option<f32>,
    #[serde(default)]
    pub(crate) audio_offset_beats:  f32,
    #[serde(default)]
    pub(crate) audio_gain:  f32,
    #[serde(default)]
    pub(crate) audio_pitch_semitones:  f32,
    #[serde(default)]
    pub(crate) audio_stretch_mode:  AudioStretchMode,
    #[serde(default)]
    pub(crate) audio_time_mul:  f32,
    #[serde(default)]
    pub(crate) audio_key:  Option<u8>,
    #[serde(default)]
    pub(crate) audio_key_minor:  bool,
    #[serde(default)]
    pub(crate) audio_key_source:  Option<u8>,
    #[serde(default)]
    pub(crate) audio_bpm:  Option<f32>,
    #[serde(default)]
    pub(crate) audio_fine_pitch_cents:  f32,
    #[serde(default = "default_formant_scale")]
    pub(crate) audio_formant_scale:  f32,
}

pub(crate) fn default_formant_scale() -> f32 {
    1.0
}

pub(crate) fn default_performance_launch_quantize_beats() -> f32 {
    1.0
}

pub(crate) fn performance_scene_matches(a: f32, b: f32) -> bool {
    (a - b).abs() <= 0.01
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Track {
    pub(crate) name:  String,
    pub(crate) clips:  Vec<Clip>,
    pub(crate) level:  f32,
    pub(crate) muted:  bool,
    pub(crate) solo:  bool,
    pub(crate) midi_notes:  Vec<PianoRollNote>,
    pub(crate) instrument_path:  Option<String>,
    #[serde(default)]
    pub(crate) instrument_clap_id:  Option<String>,
    pub(crate) effect_paths:  Vec<String>,
    #[serde(default)]
    pub(crate) effect_clap_ids:  Vec<Option<String>>,
    #[serde(default)]
    pub(crate) effect_bypass:  Vec<bool>,
    #[serde(default)]
    pub(crate) effect_params:  Vec<Vec<String>>,
    #[serde(default)]
    pub(crate) effect_param_ids:  Vec<Vec<u32>>,
    #[serde(default)]
    pub(crate) effect_param_values:  Vec<Vec<f32>>,
    pub(crate) params:  Vec<String>,
    #[serde(default)]
    pub(crate) param_ids:  Vec<u32>,
    #[serde(default)]
    pub(crate) param_values:  Vec<f32>,
    #[serde(default)]
    pub(crate) plugin_state_component:  Option<Vec<u8>>,
    #[serde(default)]
    pub(crate) plugin_state_controller:  Option<Vec<u8>>,
    #[serde(default)]
    pub(crate) automation_lanes:  Vec<AutomationLane>,
    pub(crate) automation_channels:  Vec<String>,
    #[serde(default)]
    pub(crate) midi_cc_lanes:  Vec<MidiCcLane>,
    #[serde(default)]
    pub(crate) midi_program:  Option<u8>,
    #[serde(default)]
    pub(crate) treesynth:  Option<TreeSynthState>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) enum TreeSynthMode {
    Random,
    Layer,
    Sequential,
    Morph,
    Reorder,
}

impl Default for TreeSynthMode {
    fn default() -> Self {
        Self::Random
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct TreeSynthSample {
    pub(crate) path:  String,
    pub(crate) name:  String,
    pub(crate) root_note:  u8,
    pub(crate) gain:  f32,
    pub(crate) pan:  f32,
    pub(crate) start:  f32,
    pub(crate) end:  f32,
    pub(crate) color:  [u8; 3],
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct TreeSynthState {
    pub(crate) folder:  Option<String>,
    pub(crate) samples:  Vec<TreeSynthSample>,
    pub(crate) mode:  TreeSynthMode,
    pub(crate) morph:  f32,
    pub(crate) reorder:  f32,
    pub(crate) gain:  f32,
    pub(crate) attack:  f32,
    pub(crate) decay:  f32,
    pub(crate) sustain:  f32,
    pub(crate) release:  f32,
    pub(crate) vibrato_rate:  f32,
    pub(crate) vibrato_depth:  f32,
    pub(crate) tremolo_rate:  f32,
    pub(crate) tremolo_depth:  f32,
    pub(crate) reverb_mix:  f32,
    pub(crate) pitch_bend_range:  f32,
    pub(crate) portamento_ms:  f32,
    pub(crate) legato:  bool,
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


#[derive(Serialize, Deserialize)]
pub(crate) struct ProjectState {
    pub(crate) name:  String,
    #[serde(default)]
    pub(crate) artist:  String,
    #[serde(default)]
    pub(crate) title:  String,
    #[serde(default)]
    pub(crate) album:  String,
    #[serde(default)]
    pub(crate) genre:  String,
    #[serde(default)]
    pub(crate) year:  String,
    #[serde(default)]
    pub(crate) comment:  String,
    #[serde(default)]
    pub(crate) project_key:  Option<u8>,
    #[serde(default)]
    pub(crate) project_key_minor:  bool,
    pub(crate) tempo_bpm:  f32,
    pub(crate) tracks:  Vec<Track>,
    #[serde(default)]
    pub(crate) node_routes:  Vec<NodeRouteLink>,
    #[serde(default)]
    pub(crate) performance_clip_settings:  HashMap<usize, PerformanceClipSettings>,
    #[serde(default = "default_performance_launch_quantize_beats")]
    pub(crate) performance_launch_quantize_beats:  f32,
    #[serde(default)]
    pub(crate) master_settings:  MasterCompSettings,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Vst3PresetFile {
    pub(crate) version:  u32,
    pub(crate) name:  String,
    pub(crate) plugin:  String,
    #[serde(default)]
    pub(crate) param_names:  Vec<String>,
    #[serde(default)]
    pub(crate) param_ids:  Vec<u32>,
    #[serde(default)]
    pub(crate) param_values:  Vec<f32>,
    #[serde(default)]
    pub(crate) component_state:  String,
    #[serde(default)]
    pub(crate) controller_state:  String,
    #[serde(default)]
    pub(crate) treesynth:  Option<TreeSynthState>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct SettingsState {
    pub(crate) output_device:  String,
    #[serde(default)]
    pub(crate) input_device:  String,
    pub(crate) buffer_size:  u32,
    pub(crate) sample_rate:  u32,
    pub(crate) interpolation:  String,
    pub(crate) midi_input:  String,
    #[serde(default)]
    pub(crate) theme:  String,
    #[serde(default)]
    pub(crate) key_display_format:  String,
    #[serde(default)]
    pub(crate) device_id:  String,
    #[serde(default)]
    pub(crate) device_salt:  String,
    #[serde(default)]
    pub(crate) auth_token:  String,
    #[serde(default)]
    pub(crate) registered_to:  String,
    #[serde(default)]
    pub(crate) license_file:  String,
    #[serde(default)]
    pub(crate) license_monthly_activations:  Option<u64>,
    #[serde(default)]
    pub(crate) license_remaining_activations:  Option<u64>,
    #[serde(default)]
    pub(crate) triple_buffer:  bool,
    #[serde(default)]
    pub(crate) safe_underruns:  bool,
    #[serde(default)]
    pub(crate) adaptive_buffer:  bool,
    #[serde(default)]
    pub(crate) smart_disable_plugins:  bool,
    #[serde(default)]
    pub(crate) smart_suspend_tracks:  bool,
    #[serde(default)]
    pub(crate) recent_projects:  Vec<String>,
    #[serde(default)]
    pub(crate) autosave_minutes:  u32,
    #[serde(default)]
    pub(crate) load_last_project:  bool,
    #[serde(default = "default_startup_sound")]
    pub(crate) play_startup_sound:  bool,
    #[serde(default)]
    pub(crate) browser_folders:  Vec<String>,
    #[serde(default = "default_show_clip_labels")]
    pub(crate) show_clip_labels:  bool,
    #[serde(default)]
    pub(crate) midi_devices:  Vec<MidiDeviceConfig>,
    #[serde(default)]
    pub(crate) wallpaper_path:  String,
    #[serde(default = "default_wallpaper_opacity")]
    pub(crate) wallpaper_opacity:  f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MidiDeviceProfile {
    Keyboard,
    Launchpad,
    Apc,
    PadController,
    ControlSurface,
    Generic,
}

impl MidiDeviceProfile {
    pub(crate) fn label(self) -> &'static str {
        match self {
            MidiDeviceProfile::Keyboard => "Keyboard",
            MidiDeviceProfile::Launchpad => "Launchpad",
            MidiDeviceProfile::Apc => "APC",
            MidiDeviceProfile::PadController => "Pad Controller",
            MidiDeviceProfile::ControlSurface => "Control Surface",
            MidiDeviceProfile::Generic => "Generic MIDI",
        }
    }
}

impl Default for MidiDeviceProfile {
    fn default() -> Self {
        Self::Keyboard
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct MidiDeviceConfig {
    #[serde(default)]
    pub(crate) name:  String,
    #[serde(default)]
    pub(crate) profile:  MidiDeviceProfile,
    #[serde(default = "default_enabled_true")]
    pub(crate) enabled:  bool,
    #[serde(default)]
    pub(crate) input_port:  String,
    #[serde(default)]
    pub(crate) output_port:  String,
    #[serde(default)]
    pub(crate) midi_channel:  u8,
}

impl MidiDeviceConfig {
    pub(crate) fn display_name(&self) -> String {
        if !self.name.trim().is_empty() {
            self.name.clone()
        } else if !self.input_port.trim().is_empty() {
            self.input_port.clone()
        } else {
            self.profile.label().to_string()
        }
    }
}

impl Default for MidiDeviceConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            profile: MidiDeviceProfile::Keyboard,
            enabled: true,
            input_port: String::new(),
            output_port: String::new(),
            midi_channel: 0,
        }
    }
}

pub(crate) fn default_enabled_true() -> bool {
    true
}

pub(crate) fn default_startup_sound() -> bool {
    true
}

pub(crate) fn default_show_clip_labels() -> bool {
    true
}

pub(crate) fn default_wallpaper_opacity() -> f32 {
    0.18
}

impl Default for SettingsState {
    fn default() -> Self {
        Self {
            output_device: String::new(),
            input_device: String::new(),
            buffer_size: 512,
            sample_rate: 44_100,
            interpolation: "linear".to_string(),
            midi_input: String::new(),
            theme: "Black".to_string(),
            key_display_format: "camelot".to_string(),
            device_id: String::new(),
            device_salt: String::new(),
            auth_token: String::new(),
            registered_to: String::new(),
            license_file: String::new(),
            license_monthly_activations: None,
            license_remaining_activations: None,
            triple_buffer: false,
            safe_underruns: true,
            adaptive_buffer: true,
            smart_disable_plugins: true,
            smart_suspend_tracks: true,
            recent_projects: Vec::new(),
            autosave_minutes: 5,
            load_last_project: false,
            play_startup_sound: default_startup_sound(),
            browser_folders: Vec::new(),
            show_clip_labels: default_show_clip_labels(),
            midi_devices: Vec::new(),
            wallpaper_path: String::new(),
            wallpaper_opacity: default_wallpaper_opacity(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum PluginTarget {
    Instrument(usize),
    Effect(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PluginKind {
    Native,
    Vst3,
    Clap,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PluginCategory {
    Native,
    Bundled,
    System,
}

impl PluginCategory {
    pub(crate) fn label(self) -> &'static str {
        match self {
            PluginCategory::Native => "Native",
            PluginCategory::Bundled => "Bundled",
            PluginCategory::System => "System",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PluginCandidate {
    pub(crate) path:  String,
    pub(crate) kind:  PluginKind,
    pub(crate) clap_id:  Option<String>,
    pub(crate) display:  String,
    pub(crate) category:  PluginCategory,
    pub(crate) instrument_only:  bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PluginUiTarget {
    Instrument(usize),
    Effect(usize, usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProjectAction {
    NewProject,
    OpenProject,
    OpenProjectPath(String),
    ImportMidi,
    NewFromTemplate(String),
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum GmCategory {
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
    pub(crate) fn from_program(program: u8) -> Self {
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
pub(crate) struct GmParamValues {
    pub(crate) gain:  f32,
    pub(crate) attack:  f32,
    pub(crate) decay:  f32,
    pub(crate) sustain:  f32,
    pub(crate) release:  f32,
    pub(crate) cutoff:  f32,
    pub(crate) resonance:  f32,
    pub(crate) vibrato_rate:  f32,
    pub(crate) vibrato_intensity:  f32,
    pub(crate) tremolo_rate:  f32,
    pub(crate) tremolo_intensity:  f32,
}

impl GmParamValues {
    pub(crate) fn from_category(category: GmCategory) -> Self {
        match category {
            GmCategory::Piano => Self::new(0.85, 0.12, 0.35, 0.6, 0.35, 0.55, 0.25),
            GmCategory::Chromatic => Self::new(0.85, 0.08, 0.3, 0.55, 0.4, 0.65, 0.25),
            GmCategory::Organ => Self::new(0.9, 0.02, 0.25, 0.8, 0.25, 0.6, 0.2),
            GmCategory::Guitar => Self::new(0.8, 0.06, 0.3, 0.5, 0.35, 0.6, 0.25),
            GmCategory::Bass => Self::new(0.85, 0.03, 0.25, 0.45, 0.2, 0.35, 0.2),
            GmCategory::Strings => Self::new(0.8, 0.45, 0.4, 0.75, 0.7, 0.55, 0.3),
            GmCategory::Ensemble => Self::new(0.8, 0.35, 0.4, 0.75, 0.7, 0.55, 0.3),
            GmCategory::Brass => Self::new(0.85, 0.2, 0.35, 0.6, 0.4, 0.65, 0.3),
            GmCategory::Reed => Self::new(0.8, 0.15, 0.35, 0.6, 0.45, 0.6, 0.3),
            GmCategory::Pipe => Self::new(0.8, 0.35, 0.4, 0.7, 0.6, 0.55, 0.25),
            GmCategory::SynthLead => Self::new(0.9, 0.05, 0.25, 0.6, 0.3, 0.75, 0.35),
            GmCategory::SynthPad => Self::new(0.75, 0.6, 0.45, 0.8, 0.8, 0.5, 0.25),
            GmCategory::SynthFx => Self::new(0.75, 0.3, 0.4, 0.65, 0.7, 0.7, 0.6),
            GmCategory::Ethnic => Self::new(0.8, 0.15, 0.35, 0.6, 0.45, 0.6, 0.25),
            GmCategory::Percussive => Self::new(0.85, 0.02, 0.2, 0.3, 0.15, 0.7, 0.25),
            GmCategory::SoundFx => Self::new(0.7, 0.25, 0.35, 0.6, 0.8, 0.7, 0.5),
        }
    }

    pub(crate) fn new(
        gain: f32,
        attack: f32,
        decay: f32,
        sustain: f32,
        release: f32,
        cutoff: f32,
        resonance: f32,
    ) -> Self {
        Self {
            gain,
            attack,
            decay,
            sustain,
            release,
            cutoff,
            resonance,
            vibrato_rate: 0.35,
            vibrato_intensity: 0.25,
            tremolo_rate: 0.3,
            tremolo_intensity: 0.2,
        }
    }
}
