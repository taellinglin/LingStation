use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use eframe::egui;
use engine::midi::{export_midi, import_midi_channels, import_midi_tracks, MidiTrackData};
use engine::timeline::PianoRollNote;
use image::GenericImageView;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL_SAFE;
use base64::Engine;
use ed25519_dalek::{Signature, SigningKey, Verifier, VerifyingKey};
use midir::{Ignore, MidiInput, MidiInputConnection, MidiOutput};
use rand::RngCore;
use reqwest::blocking::Client;
use rodio::{Decoder, OutputStream, Sink, Source};
use rustfft::{num_complex::Complex, FftPlanner};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::f32::consts::TAU;
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    mpsc::{self, Receiver, Sender},
    Arc, Mutex,
};
use std::thread;

mod clap_host;
mod node_editor;
mod performance;
mod vst3;
mod entry;
mod models;
mod audio;
mod render;
mod daw_app;
mod daw_app_impl;

use daw_app::DawApp;
use models::{
    default_performance_launch_quantize_beats, performance_scene_matches, AudioStretchMode, Clip,
    GmCategory, GmParamValues, MidiDeviceConfig, MidiDeviceProfile, PluginCandidate,
    PluginCategory, PluginKind, PluginTarget, PluginUiTarget, ProjectAction, ProjectState,
    SettingsState, Track, TreeSynthMode, TreeSynthSample, TreeSynthState, Vst3PresetFile,
};
use audio::*;
use render::*;

use node_editor::{
    default_sidechain_amount, default_sidechain_attack_ms, default_sidechain_release_ms,
    default_sidechain_threshold_db, NodeRouteKind, NodeRouteLink, TrackNodeActivity,
};
use performance::{
    collect_performance_block_events_into,
    performance_audio_clip_for_block,
    performance_length_samples, PerformanceClipSettings, PerformanceRuntimeClip,
    PerformanceTriggerMode,
};

const BASE_UI_FONT_SIZE: f32 = 12.0;
const LICENSE_API_BASE: &str = "https://linglin.art";
const LICENSE_PUBLIC_KEY_B64: &str = "MC4CAQAwBQYDK2VwBCIEIJQEieMJP2VOEI8yM7BOpelBIUvC2AOUDm85k0OjtYNT";
const LICENSE_PRODUCT_CODE: &str = "LingStation";
const AUDIO_CLIP_CACHE_MAX_BYTES: usize = 512 * 1024 * 1024;
const AUDIO_CLIP_CACHE_MAX_ENTRIES: usize = 256;
const WAVEFORM_CACHE_MAX_ENTRIES: usize = 256;
const WAVEFORM_COLOR_CACHE_MAX_ENTRIES: usize = 256;
const WAVEFORM_LEN_CACHE_MAX_ENTRIES: usize = 512;
const TREESYNTH_MAX_VOICES: usize = 64;
const MAX_PLUGIN_OUTPUT_CHANNELS: usize = 16;
const MAX_CLAP_OUTPUT_CHANNELS: usize = 16;
static PLUGIN_PROCESS_FAILURES: AtomicU64 = AtomicU64::new(0);

fn main() -> eframe::Result<()> {
    entry::main()
}

#[allow(dead_code)]
struct ClipDragState {
    clip_id: usize,
    source_track: usize,
    origin_track: usize,
    offset_beats: f32,
    start_beats: f32,
    length_beats: f32,
    origin_start_beats: f32,
    origin_length_beats: f32,
    audio_offset_beats: f32,
    audio_source_beats: Option<f32>,
    kind: ClipDragKind,
    undo_pushed: bool,
    grabbed: bool,
    copy_mode: bool,
    group: Option<Vec<ClipDragGroupItem>>,
}

#[allow(dead_code)]
struct ClipDragGroupItem {
    clip_id: usize,
    source_track: usize,
    start_beats: f32,
    length_beats: f32,
    is_midi: bool,
}

struct TrackDragState {
    source_index: usize,
}

struct MidiImportState {
    path: String,
    tracks: Vec<MidiTrackData>,
    enabled: Vec<bool>,
    apply_program: Vec<bool>,
    instrument_plugin: String,
    percussion_plugin: String,
    import_portamento: bool,
    mode: MidiImportMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArrangerTool {
    Draw,
    Select,
    Move,
    Slice,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AutoSliceMode {
    Smart,
    Bar,
    Phrase,
}

impl AutoSliceMode {
    fn label(self) -> &'static str {
        match self {
            AutoSliceMode::Smart => "Smart Sections",
            AutoSliceMode::Bar => "By Bar",
            AutoSliceMode::Phrase => "By Phrase",
        }
    }

    fn interval_beats(self) -> f32 {
        match self {
            AutoSliceMode::Smart => 16.0,
            AutoSliceMode::Bar => 4.0,
            AutoSliceMode::Phrase => 16.0,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct PerformanceSectionAnalysis {
    start_beats: f32,
    length_beats: f32,
    loop_unit_beats: Option<f32>,
}

#[derive(Clone, Debug, Default)]
struct AutoPerformanceBuildSummary {
    sections: usize,
    slices_created: usize,
    configured_clips: usize,
    loop_clips: usize,
}

impl AutoPerformanceBuildSummary {
    fn changed(&self) -> bool {
        self.slices_created > 0 || self.configured_clips > 0
    }

    fn status_message(&self) -> String {
        format!(
            "Smart performance layout built {} section(s), created {} slice(s), configured {} clip(s), and enabled {} loop pad(s)",
            self.sections,
            self.slices_created,
            self.configured_clips,
            self.loop_clips,
        )
    }
}

#[allow(dead_code)]
struct ArrangerDrawState {
    track_index: usize,
    start_beats: f32,
    start_pos: egui::Pos2,
}

#[derive(Clone, Copy, Debug)]
struct ArrangerSliceDragState {
    beat: f32,
    start_track: usize,
    end_track: usize,
    free_snap: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PianoDragKind {
    Move,
    Resize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PianoTool {
    Pencil,
    Select,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PianoLaneMode {
    Velocity,
    Pan,
    Cutoff,
    Resonance,
    MidiCc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MainTab {
    Arranger,
    Parameters,
    PianoRoll,
    NodeEditor,
    Performance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsTab {
    General,
    Audio,
    Midi,
    Devices,
    Theme,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SidebarTab {
    Project,
    Browser,
}

struct PianoDragState {
    track_index: usize,
    note_index: usize,
    kind: PianoDragKind,
    offset_beats: f32,
    start_beats: f32,
    start_length: f32,
    start_pitch: u8,
    start_pos_y: f32,
    selected_notes: Vec<(usize, f32, u8, f32)>,
}

struct PianoScaleDragState {
    track_index: usize,
    anchor_start: f32,
    anchor_end: f32,
    selected_notes: Vec<(usize, f32, u8, f32)>,
}

struct PianoZoomDragState {
    start_pos: egui::Pos2,
    start_zoom_x: f32,
    start_zoom_y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FsSource {
    Project,
    Browser,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FsDragKind {
    Audio,
    Midi,
}

struct FsDragState {
    path: PathBuf,
    kind: FsDragKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum MidiImportMode {
    ReplaceProject,
    AppendTracks { start_beats: f32 },
}

impl Default for DawApp {
    fn default() -> Self {
        let clips = vec![
            Clip {
                id: 1,
                track: 0,
                start_beats: 0.0,
                length_beats: 64.0,
                is_midi: true,
                midi_notes: [
                    (0.0, 2.0, 62),
                    (2.0, 2.0, 65),
                    (4.0, 2.0, 69),
                    (6.0, 2.0, 72),
                    (8.0, 2.0, 74),
                    (10.0, 2.0, 72),
                    (12.0, 2.0, 69),
                    (14.0, 2.0, 65),
                    (16.0, 2.0, 67),
                    (18.0, 2.0, 70),
                    (20.0, 2.0, 69),
                    (22.0, 2.0, 65),
                    (24.0, 2.0, 64),
                    (26.0, 2.0, 65),
                    (28.0, 4.0, 62),
                    (32.0, 2.0, 62),
                    (34.0, 2.0, 65),
                    (36.0, 2.0, 69),
                    (38.0, 2.0, 74),
                    (40.0, 2.0, 72),
                    (42.0, 2.0, 70),
                    (44.0, 2.0, 69),
                    (46.0, 2.0, 67),
                    (48.0, 2.0, 65),
                    (50.0, 2.0, 64),
                    (52.0, 2.0, 62),
                    (54.0, 2.0, 65),
                    (56.0, 2.0, 69),
                    (58.0, 2.0, 72),
                    (60.0, 4.0, 62),
                ]
                .iter()
                .copied()
                .map(|(start, length, note)| PianoRollNote::new(start, length, note, 100))
                .collect(),
                midi_source_beats: Some(64.0),
                link_id: None,
                name: "FishSynth".to_string(),
                audio_path: None,
                audio_source_beats: None,
                audio_offset_beats: 0.0,
                audio_gain: 1.0,
                audio_pitch_semitones: 0.0,
                audio_stretch_mode: AudioStretchMode::Stretch,
                audio_time_mul: 1.0,
                audio_key: None,
                audio_key_minor: false,
                audio_key_source: None,
                audio_bpm: None,
                audio_fine_pitch_cents: 0.0,
                audio_formant_scale: 1.0,
            },
            Clip {
                id: 2,
                track: 1,
                start_beats: 0.0,
                length_beats: 8.0,
                is_midi: true,
                midi_notes: Vec::new(),
                midi_source_beats: Some(8.0),
                link_id: None,
                name: "CatSynth".to_string(),
                audio_path: None,
                audio_source_beats: None,
                audio_offset_beats: 0.0,
                audio_gain: 1.0,
                audio_pitch_semitones: 0.0,
                audio_stretch_mode: AudioStretchMode::Stretch,
                audio_time_mul: 1.0,
                audio_key: None,
                audio_key_minor: false,
                audio_key_source: None,
                audio_bpm: None,
                audio_fine_pitch_cents: 0.0,
                audio_formant_scale: 1.0,
            },
            Clip {
                id: 3,
                track: 2,
                start_beats: 0.0,
                length_beats: 8.0,
                is_midi: true,
                midi_notes: Vec::new(),
                midi_source_beats: Some(8.0),
                link_id: None,
                name: "SannySynth".to_string(),
                audio_path: None,
                audio_source_beats: None,
                audio_offset_beats: 0.0,
                audio_gain: 1.0,
                audio_pitch_semitones: 0.0,
                audio_stretch_mode: AudioStretchMode::Stretch,
                audio_time_mul: 1.0,
                audio_key: None,
                audio_key_minor: false,
                audio_key_source: None,
                audio_bpm: None,
                audio_fine_pitch_cents: 0.0,
                audio_formant_scale: 1.0,
            },
            Clip {
                id: 4,
                track: 3,
                start_beats: 0.0,
                length_beats: 8.0,
                is_midi: true,
                midi_notes: Vec::new(),
                midi_source_beats: Some(8.0),
                link_id: None,
                name: "DogSynth".to_string(),
                audio_path: None,
                audio_source_beats: None,
                audio_offset_beats: 0.0,
                audio_gain: 1.0,
                audio_pitch_semitones: 0.0,
                audio_stretch_mode: AudioStretchMode::Stretch,
                audio_time_mul: 1.0,
                audio_key: None,
                audio_key_minor: false,
                audio_key_source: None,
                audio_bpm: None,
                audio_fine_pitch_cents: 0.0,
                audio_formant_scale: 1.0,
            },
            Clip {
                id: 5,
                track: 4,
                start_beats: 0.0,
                length_beats: 8.0,
                is_midi: true,
                midi_notes: Vec::new(),
                midi_source_beats: Some(8.0),
                link_id: None,
                name: "LingSynth".to_string(),
                audio_path: None,
                audio_source_beats: None,
                audio_offset_beats: 0.0,
                audio_gain: 1.0,
                audio_pitch_semitones: 0.0,
                audio_stretch_mode: AudioStretchMode::Stretch,
                audio_time_mul: 1.0,
                audio_key: None,
                audio_key_minor: false,
                audio_key_source: None,
                audio_bpm: None,
                audio_fine_pitch_cents: 0.0,
                audio_formant_scale: 1.0,
            },
            Clip {
                id: 6,
                track: 5,
                start_beats: 0.0,
                length_beats: 8.0,
                is_midi: true,
                midi_notes: Vec::new(),
                midi_source_beats: Some(8.0),
                link_id: None,
                name: "MiceSynth".to_string(),
                audio_path: None,
                audio_source_beats: None,
                audio_offset_beats: 0.0,
                audio_gain: 1.0,
                audio_pitch_semitones: 0.0,
                audio_stretch_mode: AudioStretchMode::Stretch,
                audio_time_mul: 1.0,
                audio_key: None,
                audio_key_minor: false,
                audio_key_source: None,
                audio_bpm: None,
                audio_fine_pitch_cents: 0.0,
                audio_formant_scale: 1.0,
            },
        ];

        let tracks = vec![
            Track {
                name: "FishSynth".to_string(),
                clips: clips.iter().cloned().filter(|c| c.track == 0).collect(),
                level: 0.8,
                muted: false,
                solo: false,
                midi_notes: Vec::new(),
                instrument_path: Some("synths/FishSynth/FishSynth.vst3".to_string()),
                instrument_clap_id: None,
                effect_paths: Vec::new(),
                effect_clap_ids: Vec::new(),
                effect_bypass: Vec::new(),
                effect_params: Vec::new(),
                effect_param_ids: Vec::new(),
                effect_param_values: Vec::new(),
                params: default_instrument_params(),
                param_ids: Vec::new(),
                param_values: Vec::new(),
                plugin_state_component: None,
                plugin_state_controller: None,
                automation_lanes: Vec::new(),
                automation_channels: Vec::new(),
                midi_cc_lanes: Vec::new(),
                midi_program: None,
                treesynth: None,
            },
            Track {
                name: "CatSynth".to_string(),
                clips: clips.iter().cloned().filter(|c| c.track == 1).collect(),
                level: 0.7,
                muted: false,
                solo: false,
                midi_notes: Vec::new(),
                instrument_path: Some("synths/CatSynth/CatSynth.vst3".to_string()),
                instrument_clap_id: None,
                effect_paths: Vec::new(),
                effect_clap_ids: Vec::new(),
                effect_bypass: Vec::new(),
                effect_params: Vec::new(),
                effect_param_ids: Vec::new(),
                effect_param_values: Vec::new(),
                params: default_instrument_params(),
                param_ids: Vec::new(),
                param_values: Vec::new(),
                plugin_state_component: None,
                plugin_state_controller: None,
                automation_lanes: Vec::new(),
                automation_channels: Vec::new(),
                midi_cc_lanes: Vec::new(),
                midi_program: None,
                treesynth: None,
            },
            Track {
                name: "SannySynth".to_string(),
                clips: clips.iter().cloned().filter(|c| c.track == 2).collect(),
                level: 0.8,
                muted: false,
                solo: false,
                midi_notes: Vec::new(),
                instrument_path: Some("synths/SannySynth/SannySynth.vst3".to_string()),
                instrument_clap_id: None,
                effect_paths: Vec::new(),
                effect_clap_ids: Vec::new(),
                effect_bypass: Vec::new(),
                effect_params: Vec::new(),
                effect_param_ids: Vec::new(),
                effect_param_values: Vec::new(),
                params: default_instrument_params(),
                param_ids: Vec::new(),
                param_values: Vec::new(),
                plugin_state_component: None,
                plugin_state_controller: None,
                automation_lanes: Vec::new(),
                automation_channels: Vec::new(),
                midi_cc_lanes: Vec::new(),
                midi_program: None,
                treesynth: None,
            },
            Track {
                name: "DogSynth".to_string(),
                clips: clips.iter().cloned().filter(|c| c.track == 3).collect(),
                level: 0.8,
                muted: false,
                solo: false,
                midi_notes: Vec::new(),
                instrument_path: Some("synths/DogSynth/DogSynth.vst3".to_string()),
                instrument_clap_id: None,
                effect_paths: Vec::new(),
                effect_clap_ids: Vec::new(),
                effect_bypass: Vec::new(),
                effect_params: Vec::new(),
                effect_param_ids: Vec::new(),
                effect_param_values: Vec::new(),
                params: default_instrument_params(),
                param_ids: Vec::new(),
                param_values: Vec::new(),
                plugin_state_component: None,
                plugin_state_controller: None,
                automation_lanes: Vec::new(),
                automation_channels: Vec::new(),
                midi_cc_lanes: Vec::new(),
                midi_program: None,
                treesynth: None,
            },
            Track {
                name: "LingSynth".to_string(),
                clips: clips.iter().cloned().filter(|c| c.track == 4).collect(),
                level: 0.8,
                muted: false,
                solo: false,
                midi_notes: Vec::new(),
                instrument_path: Some("synths/LingSynth/LingSynth.vst3".to_string()),
                instrument_clap_id: None,
                effect_paths: Vec::new(),
                effect_clap_ids: Vec::new(),
                effect_bypass: Vec::new(),
                effect_params: Vec::new(),
                effect_param_ids: Vec::new(),
                effect_param_values: Vec::new(),
                params: default_instrument_params(),
                param_ids: Vec::new(),
                param_values: Vec::new(),
                plugin_state_component: None,
                plugin_state_controller: None,
                automation_lanes: Vec::new(),
                automation_channels: Vec::new(),
                midi_cc_lanes: Vec::new(),
                midi_program: None,
                treesynth: None,
            },
            Track {
                name: "MiceSynth".to_string(),
                clips: clips.iter().cloned().filter(|c| c.track == 5).collect(),
                level: 0.8,
                muted: false,
                solo: false,
                midi_notes: Vec::new(),
                instrument_path: Some("synths/MiceSynth/MiceSynth.vst3".to_string()),
                instrument_clap_id: None,
                effect_paths: Vec::new(),
                effect_clap_ids: Vec::new(),
                effect_bypass: Vec::new(),
                effect_params: Vec::new(),
                effect_param_ids: Vec::new(),
                effect_param_values: Vec::new(),
                params: default_instrument_params(),
                param_ids: Vec::new(),
                param_values: Vec::new(),
                plugin_state_component: None,
                plugin_state_controller: None,
                automation_lanes: Vec::new(),
                automation_channels: Vec::new(),
                midi_cc_lanes: Vec::new(),
                midi_program: None,
                treesynth: None,
            },
        ];

        let track_audio: Vec<TrackAudioState> = tracks
            .iter()
            .map(TrackAudioState::from_track)
            .collect();
        let track_mix_states: Vec<TrackMixState> = tracks
            .iter()
            .map(|track| TrackMixState {
                muted: track.muted,
                solo: track.solo,
                level: track.level,
            })
            .collect();
        let node_activity_states: Vec<TrackNodeActivity> =
            tracks.iter().map(|_| TrackNodeActivity::default()).collect();
        let selected_track_index = Some(0);
        let initial_selected_clip = Some(1);

        let mut app = Self {
            node_activity_rt: Arc::new(Mutex::new(node_activity_states)),
            project_name: "LingStation Demo".to_string(),
            project_path: String::new(),
            metadata_artist: String::new(),
            metadata_title: String::new(),
            metadata_album: String::new(),
            metadata_genre: String::new(),
            metadata_year: String::new(),
            metadata_comment: String::new(),
            project_key: None,
            project_key_minor: false,
            tracks,
            selected_clip: initial_selected_clip,
            selected_track: Some(0),
            playhead_beats: 0.0,
            last_frame_time: None,
            audio_running: false,
            audio_stream: None,
            midi_conns: Vec::new(),
            audio_stop: Arc::new(AtomicBool::new(false)),
            audio_callback_active: Arc::new(AtomicUsize::new(0)),
            playback_panic: Arc::new(AtomicBool::new(false)),
            playback_fade_in: Arc::new(AtomicBool::new(false)),
            midi_freq_bits: Arc::new(AtomicU32::new(440.0f32.to_bits())),
            midi_gate: Arc::new(AtomicBool::new(false)),
            tempo_bits: Arc::new(AtomicU32::new(120.0f32.to_bits())),
            transport_samples: Arc::new(AtomicU64::new(0)),
            master_peak_bits: Arc::new(AtomicU32::new(0.0f32.to_bits())),
            master_peak_display: 0.0,
            master_settings: Arc::new(Mutex::new(MasterCompSettings::default())),
            master_comp_state: Arc::new(Mutex::new(MasterCompState::default())),
            last_output_channels: 2,
            track_audio,
            track_mix: Arc::new(Mutex::new(track_mix_states)),
            selected_track_index: Arc::new(AtomicUsize::new(
                selected_track_index.unwrap_or(usize::MAX),
            )),
            midi_learn: Arc::new(Mutex::new(None)),
            rename_buffer: String::new(),
            rename_clip_buffer: String::new(),
            rename_clip_target: None,
            show_rename_track: false,
            show_rename_clip: false,
            project_name_buffer: String::new(),
            show_rename_project: false,
            show_settings: false,
            show_project_info: false,
            show_metadata: false,
            show_help_about: false,
            show_help_license: false,
            show_help_shortcuts: false,
            show_help_general: false,
            show_sidebar: true,
            show_mixer: true,
            show_transport: true,
            main_tab: MainTab::Arranger,
            settings_tab: SettingsTab::Audio,
            show_hitboxes: false,
            tempo_bpm: 120.0,
            arranger_pan: egui::vec2(0.0, 0.0),
            arranger_zoom: 1.0,
            piano_pan: egui::vec2(0.0, 0.0),
            piano_zoom_x: 1.0,
            piano_zoom_y: 1.0,
            piano_note_len: 1.0,
            piano_snap: 0.25,
            piano_roll_hovered: false,
            piano_key_down: None,
            piano_lane_mode: PianoLaneMode::Velocity,
            piano_cc: 1,
            import_path: "project.mid".to_string(),
            export_path: "export.mid".to_string(),
            status: "Ready".to_string(),
            license_identifier: String::new(),
            license_password: String::new(),
            license_serial: String::new(),
            license_device_label: "Main PC".to_string(),
            license_status: "Unregistered".to_string(),
            license_job: None,
            last_ui_param_change: None,
            preset_name_buffer: String::new(),
            startup_stream: None,
            startup_sink: None,
            settings: SettingsState::default(),
            settings_path: Self::default_settings_path(),
            show_plugin_picker: false,
            show_plugin_ui: false,
            plugin_ui_target: None,
            project_dirty: false,
            last_autosave_at: None,
            show_close_confirm: false,
            pending_project_action: None,
            pending_exit: false,
            exit_confirmed: false,
            show_render_dialog: false,
            render_format: RenderFormat::Wav,
            render_sample_rate: 48_000,
            render_wav_bit_depth: RenderWavBitDepth::Float32,
            render_bitrate: 320,
            render_split_tracks: false,
            render_target_dir: None,
            render_progress: None,
            render_job: None,
            render_range_start: 0.0,
            render_range_end: 0.0,
            render_tail_mode: RenderTailMode::Release,
            render_release_seconds: 2.0,
            record_audio: false,
            record_midi: true,
            record_automation: false,
            record_performance: false,
            is_recording: false,
            record_started_audio: false,
            recording: Arc::new(Mutex::new(RecordingBuffers {
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
                audio_sample_rate: 0,
                midi_active: HashMap::new(),
                midi_notes: Vec::new(),
                automation_points: Vec::new(),
                performance_active: HashMap::new(),
                performance_takes: Vec::new(),
            })),
            audio_input_stream: None,
            plugin_candidates: Vec::new(),
            plugin_search: String::new(),
            plugin_target: None,
            show_midi_import: false,
            midi_import_state: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            clip_drag: None,
            track_drag: None,
            arranger_tool: ArrangerTool::Move,
            arranger_select_start: None,
            arranger_select_add: false,
            arranger_draw: None,
            arranger_slice_drag: None,
            _clip_clipboard: None,
            waveform_cache: RefCell::new(HashMap::new()),
            waveform_color_cache: RefCell::new(HashMap::new()),
            waveform_len_seconds_cache: RefCell::new(HashMap::new()),
            waveform_cache_order: RefCell::new(VecDeque::new()),
            waveform_color_cache_order: RefCell::new(VecDeque::new()),
            waveform_len_seconds_cache_order: RefCell::new(VecDeque::new()),
            audio_clip_cache: Arc::new(Mutex::new(AudioClipCache::new(
                AUDIO_CLIP_CACHE_MAX_BYTES,
                AUDIO_CLIP_CACHE_MAX_ENTRIES,
            ))),
            audio_clip_timeline: Arc::new(Mutex::new(Vec::new())),
            analysis_sender: None,
            analysis_receiver: None,
            analysis_pending: HashSet::new(),
            audio_preview_stream: None,
            audio_preview_sink: None,
            audio_preview_loop: false,
            audio_preview_clip_id: None,
            audio_stats: Arc::new(AudioRuntimeStats::new()),
            ui_frame_last_ms: 0.0,
            ui_frame_max_ms: 0.0,
            ui_arranger_last_ms: 0.0,
            ui_arranger_max_ms: 0.0,
            buffer_override: None,
            adaptive_restart_requested: Arc::new(AtomicBool::new(false)),
            adaptive_buffer_size: Arc::new(AtomicU32::new(0)),
            adaptive_restart_pending: false,
            last_overrun: Arc::new(AtomicBool::new(false)),
            piano_drag: None,
            piano_scale_drag: None,
            piano_zoom_drag: None,
            piano_tool: PianoTool::Pencil,
            arranger_snap_beats: 1.0,
            piano_selected: HashSet::new(),
            piano_marquee_start: None,
            piano_marquee_add: false,
            piano_cc_drag: None,
            piano_roll_rect: None,
            piano_roll_panel_height: 0.0,
            piano_focus_beats: None,
            selected_clips: {
                let mut set = HashSet::new();
                if let Some(clip_id) = initial_selected_clip {
                    set.insert(clip_id);
                }
                set
            },
            plugin_ui: None,
            plugin_ui_hidden: false,
            plugin_ui_resume_at: None,
            last_params_track: None,
            last_viewport_maximized: None,
            last_viewport_rect: None,
            pending_startup_maximize: true,
            seen_nonzero_viewport: false,
            pending_viewport_focus: false,
            pending_repaint_frames: 0,
            wallpaper_texture: None,
            wallpaper_texture_path: String::new(),
            fs_expanded: HashSet::new(),
            fs_selected: None,
            browser_expanded: HashSet::new(),
            browser_selected: None,
            sidebar_tab: SidebarTab::Project,
            fs_drag: None,
            loop_start_beats: None,
            loop_end_beats: None,
            loop_start_samples: Arc::new(AtomicU64::new(0)),
            loop_end_samples: Arc::new(AtomicU64::new(0)),
            orphaned_hosts: Vec::new(),
            automation_active: None,
            automation_rows_expanded: HashSet::new(),
            node_routes: Vec::new(),
            node_routes_rt: Arc::new(Mutex::new(Vec::new())),
            node_route_kind: NodeRouteKind::AudioSend,
            node_route_from_track: 0,
            node_route_source_output_pair: 0,
            node_route_to_track: 0,
            node_route_to_fx: 0,
            node_view_pan: egui::Vec2::ZERO,
            node_view_zoom: 1.0,
            node_map_height: 560.0,
            performance_clip_settings: HashMap::new(),
            performance_launch_quantize_beats: default_performance_launch_quantize_beats(),
            performance_selected_clip: None,
            performance_runtime: Arc::new(Mutex::new(Vec::new())),
            arrangement_playback_enabled: Arc::new(AtomicBool::new(false)),
            gm_presets_generated: false,
        };
        app.load_settings_or_default();
        app.ensure_device_id();
        app.refresh_license_status();
        if app.settings.play_startup_sound {
            if let Err(err) = app.play_startup_sound() {
                app.status = format!("Startup sound failed: {err}");
            }
        }
        app
    }
}


#[cfg(windows)]
fn create_plugin_child_window(parent: isize) -> Option<isize> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, RegisterClassExW, WNDCLASSEXW, CS_HREDRAW, CS_OWNDC,
        CS_VREDRAW, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_VISIBLE,
    };

    let class_name: Vec<u16> = OsStr::new("LingStationPluginChild")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let title: Vec<u16> = OsStr::new("")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let hinstance = unsafe { GetModuleHandleW(std::ptr::null()) };
    unsafe {
        let wnd_class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW | CS_OWNDC,
            lpfnWndProc: Some(DefWindowProcW),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: 0,
            hCursor: 0,
            hbrBackground: 0,
            lpszMenuName: std::ptr::null(),
            lpszClassName: class_name.as_ptr(),
            hIconSm: 0,
        };
        let atom = RegisterClassExW(&wnd_class);
        if atom == 0 {
            let _ = GetLastError();
        }
    }
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
            0,
            0,
            100,
            100,
            parent as isize,
            0,
            hinstance,
            std::ptr::null_mut(),
        )
    };
    if hwnd == 0 {
        None
    } else {
        Some(hwnd)
    }
}

#[cfg(windows)]
fn create_plugin_top_window(width: i32, height: i32) -> Option<isize> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, RegisterClassExW, ShowWindow, WNDCLASSEXW, CS_HREDRAW, CS_VREDRAW,
        CS_OWNDC, SW_SHOW, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_APPWINDOW,
        WS_EX_CONTROLPARENT, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
    };

    let class_name: Vec<u16> = OsStr::new("LingStationPluginHost")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let title: Vec<u16> = OsStr::new("Plugin Editor")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let hinstance = unsafe { GetModuleHandleW(std::ptr::null()) };
    unsafe {
        let wnd_class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW | CS_OWNDC,
            lpfnWndProc: Some(plugin_host_wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: 0,
            hCursor: 0,
            hbrBackground: 0,
            lpszMenuName: std::ptr::null(),
            lpszClassName: class_name.as_ptr(),
            hIconSm: 0,
        };
        let atom = RegisterClassExW(&wnd_class);
        if atom == 0 {
            let _ = GetLastError();
        }
    }
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_APPWINDOW | WS_EX_CONTROLPARENT,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
            100,
            100,
            width.max(200),
            height.max(120),
            0,
            0,
            hinstance,
            std::ptr::null_mut(),
        )
    };
    if hwnd == 0 {
        None
    } else {
        unsafe { ShowWindow(hwnd, SW_SHOW) };
        Some(hwnd)
    }
}

#[cfg(not(windows))]
fn create_plugin_top_window(_width: i32, _height: i32) -> Option<isize> {
    None
}

#[cfg(windows)]
unsafe extern "system" fn plugin_host_wndproc(
    hwnd: isize,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DefWindowProcW, ShowWindow, SW_HIDE, WM_CLOSE, WM_NCDESTROY,
    };
    if msg == WM_CLOSE {
        if let Some(flag) = get_plugin_close_flag(hwnd) {
            flag.store(true, Ordering::Relaxed);
        }
        ShowWindow(hwnd, SW_HIDE);
        release_mouse_capture();
        return 0;
    }
    if msg == WM_NCDESTROY {
        if let Some(flag) = get_plugin_close_flag(hwnd) {
            drop(Arc::from_raw(flag as *const AtomicBool));
        }
        clear_plugin_close_flag(hwnd);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

#[cfg(not(windows))]
fn create_plugin_child_window(_parent: isize) -> Option<isize> {
    None
}

#[cfg(windows)]
fn move_plugin_child_window(hwnd: isize, x: i32, y: i32, w: i32, h: i32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER,
    };
    unsafe {
        SetWindowPos(hwnd, 0, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

#[cfg(not(windows))]
fn move_plugin_child_window(_hwnd: isize, _x: i32, _y: i32, _w: i32, _h: i32) {}

#[cfg(windows)]
fn resize_plugin_top_window(hwnd: isize, client_w: i32, client_h: i32) {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AdjustWindowRectEx, GetWindowLongW, SetWindowPos, GWL_EXSTYLE, GWL_STYLE, SWP_NOACTIVATE,
        SWP_NOMOVE, SWP_NOZORDER,
    };
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: client_w.max(1),
        bottom: client_h.max(1),
    };
    unsafe {
        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        AdjustWindowRectEx(&mut rect, style, 0, ex_style);
        let width = (rect.right - rect.left).max(1);
        let height = (rect.bottom - rect.top).max(1);
        SetWindowPos(
            hwnd,
            0,
            0,
            0,
            width,
            height,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOMOVE,
        );
    }
}

#[cfg(not(windows))]
fn resize_plugin_top_window(_hwnd: isize, _client_w: i32, _client_h: i32) {}

#[cfg(windows)]
fn destroy_plugin_child_window(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow;
    unsafe {
        DestroyWindow(hwnd);
    }
}

#[cfg(not(windows))]
fn destroy_plugin_child_window(_hwnd: isize) {}

#[cfg(windows)]
fn bring_window_to_front(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetForegroundWindow, ShowWindow, SW_SHOW,
    };
    unsafe {
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
    }
}

#[cfg(not(windows))]
fn bring_window_to_front(_hwnd: isize) {}

#[cfg(windows)]
fn hide_plugin_window(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
    unsafe {
        ShowWindow(hwnd, SW_HIDE);
    }
}

#[cfg(not(windows))]
fn hide_plugin_window(_hwnd: isize) {}

#[cfg(windows)]
fn show_plugin_window(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_SHOW};
    unsafe {
        ShowWindow(hwnd, SW_SHOW);
    }
}

#[cfg(not(windows))]
fn show_plugin_window(_hwnd: isize) {}

#[cfg(windows)]
fn invalidate_plugin_window(hwnd: isize) {
    use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
    unsafe {
        InvalidateRect(hwnd, std::ptr::null(), 1);
    }
}

#[cfg(not(windows))]
fn invalidate_plugin_window(_hwnd: isize) {}

#[cfg(windows)]
fn set_plugin_close_flag(hwnd: isize, flag: &Arc<AtomicBool>) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowLongPtrW, GWLP_USERDATA};
    let ptr = Arc::into_raw(flag.clone()) as isize;
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr);
    }
}

#[cfg(not(windows))]
fn set_plugin_close_flag(_hwnd: isize, _flag: &Arc<AtomicBool>) {}

#[cfg(windows)]
fn get_plugin_close_flag(hwnd: isize) -> Option<&'static AtomicBool> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowLongPtrW, GWLP_USERDATA};
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const AtomicBool;
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { &*ptr })
    }
}

#[cfg(windows)]
fn clear_plugin_close_flag(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowLongPtrW, GWLP_USERDATA};
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
    }
}

#[cfg(not(windows))]
#[allow(dead_code)]
fn get_plugin_close_flag(_hwnd: isize) -> Option<&'static AtomicBool> {
    None
}

#[cfg(windows)]
fn pump_plugin_messages(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, PeekMessageW, TranslateMessage, PM_REMOVE, MSG,
    };
    if hwnd == 0 {
        return;
    }
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while PeekMessageW(&mut msg, hwnd, 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

#[cfg(not(windows))]
fn pump_plugin_messages(_hwnd: isize) {}

#[cfg(windows)]
fn release_mouse_capture() {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
    unsafe {
        ReleaseCapture();
    }
}

#[cfg(not(windows))]
fn release_mouse_capture() {}

#[cfg(windows)]
fn client_window_size(hwnd: isize) -> Option<(i32, i32)> {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let ok = unsafe { GetClientRect(hwnd, &mut rect) };
    if ok == 0 {
        return None;
    }
    Some(((rect.right - rect.left).max(0), (rect.bottom - rect.top).max(0)))
}

#[cfg(not(windows))]
fn client_window_size(_hwnd: isize) -> Option<(i32, i32)> {
    None
}


#[cfg(windows)]
fn is_window_alive(hwnd: isize) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::IsWindow;
    unsafe { IsWindow(hwnd) != 0 }
}

#[cfg(not(windows))]
fn is_window_alive(_hwnd: isize) -> bool {
    false
}

#[cfg(windows)]
fn is_window_visible(hwnd: isize) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible;
    unsafe { IsWindowVisible(hwnd) != 0 }
}

#[cfg(not(windows))]
fn is_window_visible(_hwnd: isize) -> bool {
    false
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_join_rejects_traversal() {
        let base = Path::new("/tmp/lingstation_base");
        assert!(DawApp::safe_join_within_base(base, "../x").is_err());

        let child = DawApp::safe_join_within_base(base, "render").unwrap();
        assert!(child.starts_with(base));
    }
}
