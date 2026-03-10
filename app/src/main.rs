use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use eframe::egui;
use engine::midi::{export_midi, import_midi_channels, import_midi_tracks, MidiTrackData};
use engine::timeline::PianoRollNote;
use midir::{Ignore, MidiInput, MidiInputConnection};
use rodio::{Decoder, OutputStream, Sink, Source};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::backtrace::Backtrace;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::f32::consts::TAU;
use std::fs;
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    Arc, Mutex,
};

#[cfg(windows)]
use std::os::windows::io::AsRawHandle;

mod vst3;

fn main() -> eframe::Result<()> {
    install_crash_logger();
    init_windows_com();
    let mut viewport = egui::ViewportBuilder::default().with_inner_size([1280.0, 800.0]);
    if let Some(icon) = load_app_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native("LingStation", options, Box::new(|cc| {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        configure_fonts(&cc.egui_ctx);
        Box::new(DawApp::default())
    }))
}

fn load_app_icon() -> Option<egui::IconData> {
    let bytes = include_bytes!("../../icon.png");
    let image = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (width, height) = image.dimensions();
    Some(egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    })
}

fn configure_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    {
        let data = include_bytes!("../../font.ttf");
        fonts
            .font_data
            .insert("custom".to_string(), egui::FontData::from_static(data));
        fonts
            .families
            .insert(egui::FontFamily::Proportional, vec!["custom".to_string()]);
        fonts
            .families
            .insert(egui::FontFamily::Monospace, vec!["custom".to_string()]);
        ctx.set_fonts(fonts);
    }
    let mut style = (*ctx.style()).clone();
    for font_id in style.text_styles.values_mut() {
        font_id.size *= 1.0;
    }
    ctx.set_style(style);
    ctx.set_pixels_per_point(1.5);
    ctx.tessellation_options_mut(|t| {
        t.feathering = false;
    });
}

#[cfg(windows)]
fn init_windows_com() {
    use windows_sys::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    use windows_sys::Win32::System::Ole::OleInitialize;
    unsafe {
        CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED as u32);
        OleInitialize(std::ptr::null_mut());
    }
}

#[cfg(not(windows))]
fn init_windows_com() {}

fn install_crash_logger() {
    install_windows_crash_handler();
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Ok(mut file) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("crash.log")
        {
            let _ = writeln!(file, "---- crash ----");
            let _ = writeln!(file, "{info}");
            let bt = Backtrace::force_capture();
            let _ = writeln!(file, "{bt:?}");
        }
        default_hook(info);
    }));
}

#[cfg(windows)]
fn install_windows_crash_handler() {
    use windows_sys::Win32::System::Diagnostics::Debug::{
        MiniDumpWriteDump, SetUnhandledExceptionFilter, EXCEPTION_POINTERS,
        MINIDUMP_EXCEPTION_INFORMATION, MiniDumpNormal,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId,
    };
    const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
    const EXCEPTION_EXECUTE_HANDLER: i32 = 1;

    unsafe extern "system" fn handler(info: *const EXCEPTION_POINTERS) -> i32 {
        let file = match std::fs::File::create("crash.dmp") {
            Ok(file) => file,
            Err(_) => return EXCEPTION_CONTINUE_SEARCH,
        };
        let process = unsafe { GetCurrentProcess() };
        let pid = unsafe { GetCurrentProcessId() };
        let mut exception_info = MINIDUMP_EXCEPTION_INFORMATION {
            ThreadId: unsafe { GetCurrentThreadId() },
            ExceptionPointers: info as *mut EXCEPTION_POINTERS,
            ClientPointers: 0,
        };
        let ok = unsafe {
            MiniDumpWriteDump(
                process,
                pid,
                file.as_raw_handle() as isize,
                MiniDumpNormal,
                &mut exception_info as *mut _,
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        if ok == 0 {
            EXCEPTION_CONTINUE_SEARCH
        } else {
            EXCEPTION_EXECUTE_HANDLER
        }
    }

    unsafe {
        SetUnhandledExceptionFilter(Some(handler));
    }
}

#[cfg(not(windows))]
fn install_windows_crash_handler() {}

#[derive(Clone, Serialize, Deserialize)]
struct Clip {
    id: usize,
    track: usize,
    start_beats: f32,
    length_beats: f32,
    is_midi: bool,
    #[serde(default)]
    name: String,
    #[serde(default)]
    audio_path: Option<String>,
    #[serde(default)]
    audio_source_beats: Option<f32>,
    #[serde(default)]
    audio_offset_beats: f32,
    #[serde(default)]
    audio_gain: f32,
    #[serde(default)]
    audio_pitch_semitones: f32,
    #[serde(default)]
    audio_time_mul: f32,
}

#[derive(Clone, Serialize, Deserialize)]
struct Track {
    name: String,
    clips: Vec<Clip>,
    level: f32,
    muted: bool,
    solo: bool,
    midi_notes: Vec<PianoRollNote>,
    instrument_path: Option<String>,
    effect_paths: Vec<String>,
    #[serde(default)]
    effect_bypass: Vec<bool>,
    #[serde(default)]
    effect_params: Vec<Vec<String>>,
    #[serde(default)]
    effect_param_ids: Vec<Vec<u32>>,
    #[serde(default)]
    effect_param_values: Vec<Vec<f32>>,
    params: Vec<String>,
    #[serde(default)]
    param_ids: Vec<u32>,
    #[serde(default)]
    param_values: Vec<f32>,
    #[serde(default)]
    plugin_state_component: Option<Vec<u8>>,
    #[serde(default)]
    plugin_state_controller: Option<Vec<u8>>,
    #[serde(default)]
    automation_lanes: Vec<AutomationLane>,
    automation_channels: Vec<String>,
    #[serde(default)]
    midi_cc_lanes: Vec<MidiCcLane>,
    #[serde(default)]
    midi_program: Option<u8>,
}

#[derive(Serialize, Deserialize)]
struct ProjectState {
    name: String,
    #[serde(default)]
    artist: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    album: String,
    #[serde(default)]
    genre: String,
    #[serde(default)]
    year: String,
    #[serde(default)]
    comment: String,
    tempo_bpm: f32,
    tracks: Vec<Track>,
}

#[derive(Clone, Serialize, Deserialize)]
struct SettingsState {
    output_device: String,
    #[serde(default)]
    input_device: String,
    buffer_size: u32,
    sample_rate: u32,
    interpolation: String,
    midi_input: String,
    #[serde(default)]
    theme: String,
    #[serde(default)]
    triple_buffer: bool,
    #[serde(default)]
    safe_underruns: bool,
    #[serde(default)]
    adaptive_buffer: bool,
    #[serde(default)]
    smart_disable_plugins: bool,
    #[serde(default)]
    smart_suspend_tracks: bool,
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
            triple_buffer: false,
            safe_underruns: true,
            adaptive_buffer: true,
            smart_disable_plugins: true,
            smart_suspend_tracks: true,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum PluginTarget {
    Instrument(usize),
    Effect(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PluginUiTarget {
    Instrument(usize),
    Effect(usize, usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProjectAction {
    NewProject,
    OpenProject,
    ImportMidi,
}

#[derive(Clone)]
struct TrackAudioState {
    host: Option<Arc<Mutex<vst3::Vst3Host>>>,
    effect_hosts: Vec<Arc<Mutex<vst3::Vst3Host>>>,
    effect_bypass: Arc<Mutex<Vec<bool>>>,
    midi_events: Arc<Mutex<Vec<vst3::MidiEvent>>>,
    clip_notes: Arc<Mutex<Vec<PianoRollNote>>>,
    learned_cc: Arc<Mutex<std::collections::HashMap<(u8, u8), u32>>>,
    peak_bits: Arc<AtomicU32>,
    peak_l_bits: Arc<AtomicU32>,
    peak_r_bits: Arc<AtomicU32>,
    automation_lanes: Arc<Mutex<Vec<AutomationLane>>>,
    pending_param_changes: Arc<Mutex<Vec<PendingParamChange>>>,
    silent_blocks: Arc<AtomicU32>,
}

#[derive(Clone, Copy)]
enum PendingParamTarget {
    Instrument,
    Effect(usize),
}

#[derive(Clone, Copy)]
struct PendingParamChange {
    target: PendingParamTarget,
    param_id: u32,
    value: f64,
}

impl TrackAudioState {
    fn from_track(track: &Track) -> Self {
        Self {
            host: None,
            effect_hosts: Vec::new(),
            effect_bypass: Arc::new(Mutex::new(track.effect_bypass.clone())),
            midi_events: Arc::new(Mutex::new(Vec::new())),
            clip_notes: Arc::new(Mutex::new(track.midi_notes.clone())),
            learned_cc: Arc::new(Mutex::new(std::collections::HashMap::new())),
            peak_bits: Arc::new(AtomicU32::new(0.0f32.to_bits())),
            peak_l_bits: Arc::new(AtomicU32::new(0.0f32.to_bits())),
            peak_r_bits: Arc::new(AtomicU32::new(0.0f32.to_bits())),
            automation_lanes: Arc::new(Mutex::new(track.automation_lanes.clone())),
            pending_param_changes: Arc::new(Mutex::new(Vec::new())),
            silent_blocks: Arc::new(AtomicU32::new(0)),
        }
    }

    fn sync_notes(&self, track: &Track) {
        if let Ok(mut notes) = self.clip_notes.lock() {
            *notes = track.midi_notes.clone();
        }
    }

    fn sync_automation(&self, track: &Track) {
        if let Ok(mut lanes) = self.automation_lanes.lock() {
            *lanes = track.automation_lanes.clone();
        }
    }

    fn sync_effect_bypass(&self, track: &Track) {
        if let Ok(mut bypass) = self.effect_bypass.lock() {
            *bypass = track.effect_bypass.clone();
        }
    }
}

#[derive(Clone, Copy)]
struct TrackMixState {
    muted: bool,
    solo: bool,
    level: f32,
}

struct AudioClipData {
    samples: Vec<f32>,
    channels: usize,
    sample_rate: u32,
}

#[derive(Clone)]
struct AudioClipRender {
    path: String,
    track_index: usize,
    start_samples: u64,
    length_samples: u64,
    offset_samples: u64,
    gain: f32,
    time_mul: f32,
}

struct RenderPlan {
    path: String,
    sample_rate: u32,
    block_size: usize,
    tempo_bpm: f32,
    start_beats: f32,
    end_beats: f32,
    bitrate_kbps: u32,
    tracks: Vec<RenderTrack>,
    notes: Vec<PianoRollNote>,
    instrument_path: Option<String>,
    param_ids: Vec<u32>,
    param_values: Vec<f32>,
    plugin_state_component: Option<Vec<u8>>,
    plugin_state_controller: Option<Vec<u8>>,
    audio_clips: Vec<AudioClipRender>,
    audio_cache: HashMap<String, Arc<AudioClipData>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RenderTailMode {
    Wrap,
    Release,
    Cut,
}

#[derive(Clone)]
struct RenderTrack {
    notes: Vec<PianoRollNote>,
    instrument_path: Option<String>,
    param_ids: Vec<u32>,
    param_values: Vec<f32>,
    plugin_state_component: Option<Vec<u8>>,
    plugin_state_controller: Option<Vec<u8>>,
    effect_paths: Vec<String>,
    effect_bypass: Vec<bool>,
    automation_lanes: Vec<AutomationLane>,
    level: f32,
    active: bool,
}

struct RenderJob {
    done: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
    finished: Arc<AtomicBool>,
    result: Arc<Mutex<Option<Result<String, String>>>>,
}

struct FsEntry {
    name: String,
    path: PathBuf,
    is_dir: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct AutomationPoint {
    beat: f32,
    value: f32,
}

#[derive(Clone, Serialize, Deserialize)]
struct AutomationLane {
    name: String,
    param_id: u32,
    #[serde(default)]
    target: AutomationTarget,
    points: Vec<AutomationPoint>,
}

#[derive(Clone, Serialize, Deserialize)]
struct MidiCcLane {
    cc: u8,
    points: Vec<AutomationPoint>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum AutomationTarget {
    Instrument,
    Effect(usize),
}

impl Default for AutomationTarget {
    fn default() -> Self {
        AutomationTarget::Instrument
    }
}

#[derive(Clone)]
struct RecordedAutomationPoint {
    param_id: u32,
    target: AutomationTarget,
    beat: f32,
    value: f32,
}

struct RecordingBuffers {
    active: bool,
    track_index: usize,
    start_samples: u64,
    start_beats: f32,
    record_audio: bool,
    record_midi: bool,
    record_automation: bool,
    audio_samples: Vec<f32>,
    audio_channels: usize,
    audio_sample_rate: u32,
    midi_active: HashMap<u8, (f32, u8)>,
    midi_notes: Vec<PianoRollNote>,
    automation_points: Vec<RecordedAutomationPoint>,
}

struct DawApp {
    project_name: String,
    project_path: String,
    metadata_artist: String,
    metadata_title: String,
    metadata_album: String,
    metadata_genre: String,
    metadata_year: String,
    metadata_comment: String,
    tracks: Vec<Track>,
    selected_clip: Option<usize>,
    selected_track: Option<usize>,
    playhead_beats: f32,
    last_frame_time: Option<f64>,
    audio_running: bool,
    audio_stream: Option<cpal::Stream>,
    midi_conn: Option<MidiInputConnection<()>>,
    audio_stop: Arc<AtomicBool>,
    audio_callback_active: Arc<AtomicUsize>,
    midi_freq_bits: Arc<AtomicU32>,
    midi_gate: Arc<AtomicBool>,
    tempo_bits: Arc<AtomicU32>,
    transport_samples: Arc<AtomicU64>,
    master_peak_bits: Arc<AtomicU32>,
    master_peak_display: f32,
    last_output_channels: usize,
    track_audio: Vec<TrackAudioState>,
    track_mix: Arc<Mutex<Vec<TrackMixState>>>,
    selected_track_index: Arc<AtomicUsize>,
    midi_learn: Arc<Mutex<Option<(usize, u32)>>>,
    rename_buffer: String,
    show_rename_track: bool,
    project_name_buffer: String,
    show_rename_project: bool,
    show_settings: bool,
    show_project_info: bool,
    show_metadata: bool,
    show_sidebar: bool,
    show_mixer: bool,
    show_transport: bool,
    main_tab: MainTab,
    settings_tab: SettingsTab,
    show_hitboxes: bool,
    tempo_bpm: f32,
    arranger_pan: egui::Vec2,
    arranger_zoom: f32,
    piano_pan: egui::Vec2,
    piano_zoom_x: f32,
    piano_zoom_y: f32,
    piano_note_len: f32,
    piano_snap: f32,
    piano_roll_hovered: bool,
    piano_lane_mode: PianoLaneMode,
    piano_cc: u8,
    import_path: String,
    export_path: String,
    status: String,
    last_ui_param_change: Option<(u32, f32)>,
    startup_stream: Option<OutputStream>,
    startup_sink: Option<Sink>,
    settings: SettingsState,
    settings_path: String,
    show_plugin_picker: bool,
    show_plugin_ui: bool,
    plugin_ui_target: Option<PluginUiTarget>,
    project_dirty: bool,
    show_close_confirm: bool,
    pending_project_action: Option<ProjectAction>,
    pending_exit: bool,
    exit_confirmed: bool,
    show_render_dialog: bool,
    render_format: RenderFormat,
    render_sample_rate: u32,
    render_bitrate: u32,
    render_split_tracks: bool,
    render_target_dir: Option<PathBuf>,
    render_progress: Option<(u64, u64)>,
    render_job: Option<RenderJob>,
    render_range_start: f32,
    render_range_end: f32,
    render_tail_mode: RenderTailMode,
    render_release_seconds: f32,
    record_audio: bool,
    record_midi: bool,
    record_automation: bool,
    is_recording: bool,
    record_started_audio: bool,
    recording: Arc<Mutex<RecordingBuffers>>,
    audio_input_stream: Option<cpal::Stream>,
    plugin_candidates: Vec<String>,
    plugin_search: String,
    plugin_target: Option<PluginTarget>,
    show_midi_import: bool,
    midi_import_state: Option<MidiImportState>,
    undo_stack: Vec<UndoState>,
    redo_stack: Vec<UndoState>,
    clip_drag: Option<ClipDragState>,
    arranger_tool: ArrangerTool,
    arranger_select_start: Option<egui::Pos2>,
    arranger_select_add: bool,
    arranger_draw: Option<ArrangerDrawState>,
    clip_clipboard: Option<Clip>,
    waveform_cache: RefCell<HashMap<String, Vec<f32>>>,
    waveform_color_cache: RefCell<HashMap<String, Vec<[f32; 3]>>>,
    waveform_len_seconds_cache: RefCell<HashMap<String, f32>>,
    audio_clip_cache: Arc<Mutex<HashMap<String, Arc<AudioClipData>>>>,
    audio_clip_timeline: Arc<Mutex<Vec<AudioClipRender>>>,
    audio_preview_stream: Option<OutputStream>,
    audio_preview_sink: Option<Sink>,
    audio_preview_loop: bool,
    audio_preview_clip_id: Option<usize>,
    buffer_override: Option<u32>,
    adaptive_restart_requested: Arc<AtomicBool>,
    adaptive_buffer_size: Arc<AtomicU32>,
    last_overrun: Arc<AtomicBool>,
    piano_drag: Option<PianoDragState>,
    piano_tool: PianoTool,
    piano_selected: HashSet<usize>,
    piano_marquee_start: Option<egui::Pos2>,
    piano_marquee_add: bool,
    piano_cc_drag: Option<usize>,
    piano_roll_rect: Option<egui::Rect>,
    piano_roll_panel_height: f32,
    selected_clips: HashSet<usize>,
    plugin_ui: Option<PluginUiHost>,
    plugin_ui_hidden: bool,
    plugin_ui_resume_at: Option<std::time::Instant>,
    last_params_track: Option<usize>,
    fs_expanded: HashSet<String>,
    fs_selected: Option<String>,
    loop_start_beats: Option<f32>,
    loop_end_beats: Option<f32>,
    loop_start_samples: Arc<AtomicU64>,
    loop_end_samples: Arc<AtomicU64>,
    orphaned_hosts: Vec<Arc<Mutex<vst3::Vst3Host>>>,
    automation_active: Option<(usize, usize)>,
    automation_rows_expanded: HashSet<usize>,
}

struct PluginUiHost {
    hwnd: isize,
    child_hwnd: isize,
    editor: vst3::Vst3Editor,
    host: Arc<Mutex<vst3::Vst3Host>>,
    target: PluginUiTarget,
    close_requested: Arc<AtomicBool>,
}

struct CallbackGuard {
    counter: Arc<AtomicUsize>,
}

impl CallbackGuard {
    fn new(counter: Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self { counter }
    }
}

impl Drop for CallbackGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

#[derive(Clone)]
struct UndoState {
    project_name: String,
    tempo_bpm: f32,
    tracks: Vec<Track>,
    selected_clip: Option<usize>,
    selected_track: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClipDragKind {
    Move,
    ResizeStart,
    ResizeEnd,
    TrimStart,
    TrimEnd,
}

struct ClipDragState {
    clip_id: usize,
    source_track: usize,
    offset_beats: f32,
    start_beats: f32,
    length_beats: f32,
    audio_offset_beats: f32,
    audio_source_beats: Option<f32>,
    kind: ClipDragKind,
    undo_pushed: bool,
}

struct MidiImportState {
    path: String,
    tracks: Vec<MidiTrackData>,
    enabled: Vec<bool>,
    apply_program: Vec<bool>,
    instrument_plugin: String,
    percussion_plugin: String,
    import_portamento: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArrangerTool {
    Draw,
    Select,
    Move,
}

struct ArrangerDrawState {
    track_index: usize,
    start_beats: f32,
    start_pos: egui::Pos2,
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsTab {
    Audio,
    Midi,
    Theme,
}

struct PianoDragState {
    track_index: usize,
    note_index: usize,
    kind: PianoDragKind,
    offset_beats: f32,
}

impl Default for DawApp {
    fn default() -> Self {
        let clips = vec![
            Clip { id: 1, track: 0, start_beats: 0.0, length_beats: 4.0, is_midi: true, name: "Intro".to_string(), audio_path: None, audio_source_beats: None, audio_offset_beats: 0.0, audio_gain: 1.0, audio_pitch_semitones: 0.0, audio_time_mul: 1.0 },
            Clip { id: 2, track: 0, start_beats: 5.0, length_beats: 2.0, is_midi: true, name: "Verse".to_string(), audio_path: None, audio_source_beats: None, audio_offset_beats: 0.0, audio_gain: 1.0, audio_pitch_semitones: 0.0, audio_time_mul: 1.0 },
            Clip { id: 3, track: 1, start_beats: 1.0, length_beats: 6.0, is_midi: false, name: "Vox".to_string(), audio_path: None, audio_source_beats: None, audio_offset_beats: 0.0, audio_gain: 1.0, audio_pitch_semitones: 0.0, audio_time_mul: 1.0 },
        ];

        let tracks = vec![
            Track {
                name: "Piano".to_string(),
                clips: clips.iter().cloned().filter(|c| c.track == 0).collect(),
                level: 0.8,
                muted: false,
                solo: false,
                midi_notes: vec![
                    PianoRollNote::new(0.0, 1.0, 60, 100),
                    PianoRollNote::new(1.0, 0.5, 64, 100),
                    PianoRollNote::new(2.0, 1.5, 67, 100),
                ],
                instrument_path: None,
                effect_paths: Vec::new(),
                effect_bypass: Vec::new(),
                effect_params: Vec::new(),
                effect_param_ids: Vec::new(),
                effect_param_values: Vec::new(),
                params: default_midi_params(),
                param_ids: Vec::new(),
                param_values: Vec::new(),
                plugin_state_component: None,
                plugin_state_controller: None,
                automation_lanes: Vec::new(),
                automation_channels: Vec::new(),
                midi_cc_lanes: Vec::new(),
                midi_program: None,
            },
            Track {
                name: "Vox".to_string(),
                clips: clips.iter().cloned().filter(|c| c.track == 1).collect(),
                level: 0.7,
                muted: false,
                solo: false,
                midi_notes: Vec::new(),
                instrument_path: None,
                effect_paths: Vec::new(),
                effect_bypass: Vec::new(),
                effect_params: Vec::new(),
                effect_param_ids: Vec::new(),
                effect_param_values: Vec::new(),
                params: default_midi_params(),
                param_ids: Vec::new(),
                param_values: Vec::new(),
                plugin_state_component: None,
                plugin_state_controller: None,
                automation_lanes: Vec::new(),
                automation_channels: Vec::new(),
                midi_cc_lanes: Vec::new(),
                midi_program: None,
            },
            Track {
                name: "Drums".to_string(),
                clips: Vec::new(),
                level: 0.9,
                muted: false,
                solo: false,
                midi_notes: Vec::new(),
                instrument_path: None,
                effect_paths: Vec::new(),
                effect_bypass: Vec::new(),
                effect_params: Vec::new(),
                effect_param_ids: Vec::new(),
                effect_param_values: Vec::new(),
                params: default_midi_params(),
                param_ids: Vec::new(),
                param_values: Vec::new(),
                plugin_state_component: None,
                plugin_state_controller: None,
                automation_lanes: Vec::new(),
                automation_channels: Vec::new(),
                midi_cc_lanes: Vec::new(),
                midi_program: None,
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
        let selected_track_index = Some(0);
        let initial_selected_clip = Some(1);

        let mut app = Self {
            project_name: "LingStation Demo".to_string(),
            project_path: String::new(),
            metadata_artist: String::new(),
            metadata_title: String::new(),
            metadata_album: String::new(),
            metadata_genre: String::new(),
            metadata_year: String::new(),
            metadata_comment: String::new(),
            tracks,
            selected_clip: initial_selected_clip,
            selected_track: Some(0),
            playhead_beats: 0.0,
            last_frame_time: None,
            audio_running: false,
            audio_stream: None,
            midi_conn: None,
            audio_stop: Arc::new(AtomicBool::new(false)),
            audio_callback_active: Arc::new(AtomicUsize::new(0)),
            midi_freq_bits: Arc::new(AtomicU32::new(440.0f32.to_bits())),
            midi_gate: Arc::new(AtomicBool::new(false)),
            tempo_bits: Arc::new(AtomicU32::new(120.0f32.to_bits())),
            transport_samples: Arc::new(AtomicU64::new(0)),
            master_peak_bits: Arc::new(AtomicU32::new(0.0f32.to_bits())),
            master_peak_display: 0.0,
            last_output_channels: 2,
            track_audio,
            track_mix: Arc::new(Mutex::new(track_mix_states)),
            selected_track_index: Arc::new(AtomicUsize::new(
                selected_track_index.unwrap_or(usize::MAX),
            )),
            midi_learn: Arc::new(Mutex::new(None)),
            rename_buffer: String::new(),
            show_rename_track: false,
            project_name_buffer: String::new(),
            show_rename_project: false,
            show_settings: false,
            show_project_info: false,
            show_metadata: false,
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
            piano_lane_mode: PianoLaneMode::Velocity,
            piano_cc: 1,
            import_path: "project.mid".to_string(),
            export_path: "export.mid".to_string(),
            status: "Ready".to_string(),
            last_ui_param_change: None,
            startup_stream: None,
            startup_sink: None,
            settings: SettingsState::default(),
            settings_path: Self::default_settings_path(),
            show_plugin_picker: false,
            show_plugin_ui: false,
            plugin_ui_target: None,
            project_dirty: false,
            show_close_confirm: false,
            pending_project_action: None,
            pending_exit: false,
            exit_confirmed: false,
            show_render_dialog: false,
            render_format: RenderFormat::Wav,
            render_sample_rate: 48_000,
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
                audio_samples: Vec::new(),
                audio_channels: 0,
                audio_sample_rate: 0,
                midi_active: HashMap::new(),
                midi_notes: Vec::new(),
                automation_points: Vec::new(),
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
            arranger_tool: ArrangerTool::Move,
            arranger_select_start: None,
            arranger_select_add: false,
            arranger_draw: None,
            clip_clipboard: None,
            waveform_cache: RefCell::new(HashMap::new()),
            waveform_color_cache: RefCell::new(HashMap::new()),
            waveform_len_seconds_cache: RefCell::new(HashMap::new()),
            audio_clip_cache: Arc::new(Mutex::new(HashMap::new())),
            audio_clip_timeline: Arc::new(Mutex::new(Vec::new())),
            audio_preview_stream: None,
            audio_preview_sink: None,
            audio_preview_loop: false,
            audio_preview_clip_id: None,
            buffer_override: None,
            adaptive_restart_requested: Arc::new(AtomicBool::new(false)),
            adaptive_buffer_size: Arc::new(AtomicU32::new(0)),
            last_overrun: Arc::new(AtomicBool::new(false)),
            piano_drag: None,
            piano_tool: PianoTool::Pencil,
            piano_selected: HashSet::new(),
            piano_marquee_start: None,
            piano_marquee_add: false,
            piano_cc_drag: None,
            piano_roll_rect: None,
            piano_roll_panel_height: 0.0,
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
            fs_expanded: HashSet::new(),
            fs_selected: None,
            loop_start_beats: None,
            loop_end_beats: None,
            loop_start_samples: Arc::new(AtomicU64::new(0)),
            loop_end_samples: Arc::new(AtomicU64::new(0)),
            orphaned_hosts: Vec::new(),
            automation_active: None,
            automation_rows_expanded: HashSet::new(),
        };
        app.load_settings_or_default();
        if let Err(err) = app.play_startup_sound() {
            app.status = format!("Startup sound failed: {err}");
        }
        app
    }
}

impl eframe::App for DawApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.project_dirty {
                self.pending_exit = true;
                self.show_close_confirm = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        self.apply_theme(ctx);
        if self
            .adaptive_restart_requested
            .swap(false, Ordering::Relaxed)
        {
            let effective = self.adaptive_buffer_size.load(Ordering::Relaxed);
            if effective > 0 {
                let base = if self.settings.triple_buffer {
                    (effective / 3).max(1)
                } else {
                    effective
                };
                self.buffer_override = Some(base);
                if self.audio_running {
                    self.stop_audio_and_midi_internal(false);
                    if let Err(err) = self.start_audio_and_midi_internal(false) {
                        self.status = format!("Audio restart failed: {err}");
                    } else {
                        self.status = format!("Audio buffer increased to {effective} samples");
                    }
                }
            }
        }
        if let Some(when) = self.plugin_ui_resume_at {
            if std::time::Instant::now() >= when {
                self.plugin_ui_resume_at = None;
                if !self.audio_running {
                    if let Err(err) = self.start_audio_and_midi() {
                        self.status = format!("Audio resume failed: {err}");
                    }
                }
            }
        }
        self.sync_selected_track_index();
        self.handle_shortcuts(ctx);
        self.update_playhead(ctx);
        self.menu_bar(ctx);
        self.view_tabs(ctx);
        self.piano_roll_hovered = if matches!(self.main_tab, MainTab::Parameters | MainTab::PianoRoll) {
            let pointer_pos = ctx.input(|i| i.pointer.hover_pos());
            self.piano_roll_rect
                .and_then(|rect| pointer_pos.map(|pos| rect.contains(pos)))
                .unwrap_or(false)
        } else {
            false
        };
        if self.show_transport {
            self.toolbar(ctx);
        }
        if self.show_sidebar {
            self.left_sidebar(ctx);
        }
        if self.show_mixer {
            self.mixer_panel(ctx);
        }
        if self.show_project_info {
            self.project_info_panel(ctx);
        }
        match self.main_tab {
            MainTab::Arranger => self.center_arranger(ctx),
            MainTab::Parameters => self.center_parameters(ctx),
            MainTab::PianoRoll => self.center_piano_roll(ctx),
        }
        self.plugin_ui_window(ctx, frame);
        self.modals(ctx);
        if self.exit_confirmed {
            self.exit_confirmed = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if self.render_job.is_some() {
            ctx.request_repaint();
        }
        if self.audio_running {
            ctx.request_repaint();
        }
        if self.show_render_dialog {
            let mut open = self.show_render_dialog;
            let mut do_render = false;
            let mut close_requested = false;
            let project_end = self.project_end_beats().max(0.25);
            if self.render_range_end <= 0.0 {
                self.render_range_end = project_end;
            }
            if self.render_range_start < 0.0 {
                self.render_range_start = 0.0;
            }
            if let Some(job) = self.render_job.as_ref() {
                let done = job.done.load(Ordering::Relaxed);
                let total = job.total.load(Ordering::Relaxed);
                if total > 0 {
                    self.render_progress = Some((done, total));
                }
                if job.finished.load(Ordering::Relaxed) {
                    if let Ok(mut result) = job.result.lock() {
                        if let Some(result) = result.take() {
                            match result {
                                Ok(msg) => {
                                    self.status = msg;
                                    close_requested = true;
                                }
                                Err(err) => {
                                    self.status = format!("Render failed: {err}");
                                }
                            }
                        }
                    }
                    self.render_job = None;
                }
            }
            egui::Window::new("Render")
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.heading("Export Audio");
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label("Format");
                        let format_label = match self.render_format {
                            RenderFormat::Wav => "WAV",
                            RenderFormat::Ogg => "OGG",
                            RenderFormat::Flac => "FLAC",
                        };
                        ui.label(format_label);
                    });
                    ui.horizontal(|ui| {
                        ui.label("Sample Rate");
                        egui::ComboBox::from_id_source("render_sample_rate")
                            .selected_text(format!("{}", self.render_sample_rate))
                            .show_ui(ui, |ui| {
                                for rate in [44_100u32, 48_000, 96_000] {
                                    if ui.selectable_label(self.render_sample_rate == rate, format!("{}", rate)).clicked() {
                                        self.render_sample_rate = rate;
                                    }
                                }
                            });
                    });
                    ui.horizontal(|ui| {
                        ui.label("Bitrate");
                        egui::ComboBox::from_id_source("render_bitrate")
                            .selected_text(format!("{} kbps", self.render_bitrate))
                            .show_ui(ui, |ui| {
                                for rate in [96u32, 128, 192, 256, 320] {
                                    if ui.selectable_label(self.render_bitrate == rate, format!("{} kbps", rate)).clicked() {
                                        self.render_bitrate = rate;
                                    }
                                }
                            });
                    });
                    ui.horizontal(|ui| {
                        ui.label("Tail Mode");
                        let label = match self.render_tail_mode {
                            RenderTailMode::Wrap => "Wrap",
                            RenderTailMode::Release => "Release",
                            RenderTailMode::Cut => "Cut",
                        };
                        egui::ComboBox::from_id_source("render_tail_mode")
                            .selected_text(label)
                            .show_ui(ui, |ui| {
                                if ui.selectable_label(self.render_tail_mode == RenderTailMode::Wrap, "Wrap").clicked() {
                                    self.render_tail_mode = RenderTailMode::Wrap;
                                }
                                if ui.selectable_label(self.render_tail_mode == RenderTailMode::Release, "Release").clicked() {
                                    self.render_tail_mode = RenderTailMode::Release;
                                }
                                if ui.selectable_label(self.render_tail_mode == RenderTailMode::Cut, "Cut").clicked() {
                                    self.render_tail_mode = RenderTailMode::Cut;
                                }
                            });
                    });
                    if self.render_tail_mode == RenderTailMode::Release {
                        ui.horizontal(|ui| {
                            ui.label("Release Tail (s)");
                            ui.add(egui::DragValue::new(&mut self.render_release_seconds).speed(0.25));
                        });
                    }
                    ui.checkbox(&mut self.render_split_tracks, "Split tracks + Master");
                    ui.add_space(6.0);
                    if let Some((done, total)) = self.render_progress {
                        let progress = if total == 0 {
                            0.0
                        } else {
                            (done as f32 / total as f32).clamp(0.0, 1.0)
                        };
                        ui.add(egui::ProgressBar::new(progress).show_percentage());
                    }
                    ui.separator();
                    ui.label("Render Range (beats)");
                    ui.horizontal(|ui| {
                        ui.label("Start");
                        ui.add(egui::DragValue::new(&mut self.render_range_start).speed(0.25));
                        ui.label("End");
                        ui.add(egui::DragValue::new(&mut self.render_range_end).speed(0.25));
                        if ui
                            .add(egui::Button::image(
                                egui::Image::new(egui::include_image!("../../icons/repeat.svg"))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                            ))
                            .on_hover_text("Use Loop")
                            .clicked()
                        {
                            if let (Some(start), Some(end)) = (self.loop_start_beats, self.loop_end_beats) {
                                self.render_range_start = start.max(0.0);
                                self.render_range_end = end.max(start + 0.25);
                            }
                        }
                        if ui
                            .add(egui::Button::image(
                                egui::Image::new(egui::include_image!("../../icons/music.svg"))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                            ))
                            .on_hover_text("Full Song")
                            .clicked()
                        {
                            self.render_range_start = 0.0;
                            self.render_range_end = project_end;
                        }
                    });
                    if self.render_range_end <= self.render_range_start {
                        ui.label("Range end must be greater than start; end will default to song end.");
                    }
                    ui.horizontal(|ui| {
                        let dir_label = self
                            .render_target_dir
                            .as_ref()
                            .map(|d| d.to_string_lossy().to_string())
                            .unwrap_or_else(|| "(choose folder)".to_string());
                        if ui
                            .add(egui::Button::image(
                                egui::Image::new(egui::include_image!("../../icons/folder.svg"))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                            ))
                            .on_hover_text("Choose Folder")
                            .clicked()
                        {
                            if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                                self.render_target_dir = Some(folder);
                            }
                        }
                        ui.label(dir_label);
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let rendering = self.render_job.is_some();
                        let render_btn = ui.add_enabled(
                            !rendering,
                            egui::Button::image(
                                egui::Image::new(egui::include_image!("../../icons/disc.svg"))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                            ),
                        );
                        let render_btn = render_btn.on_hover_text("Render");
                        if render_btn.clicked() {
                            do_render = true;
                        }
                        if ui
                            .add(egui::Button::image(
                                egui::Image::new(egui::include_image!("../../icons/x.svg"))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                            ))
                            .on_hover_text("Cancel")
                            .clicked()
                        {
                            close_requested = true;
                        }
                    });
                    if self.render_job.is_some() {
                        ui.label("Rendering in background...");
                    }
                });

            if do_render {
                let folder = if let Some(folder) = self.render_target_dir.clone() {
                    folder
                } else if let Some(default_dir) = self.default_render_dir() {
                    default_dir
                } else {
                    PathBuf::from(".")
                };
                if let Err(err) = self.render_with_options(&folder) {
                    self.status = format!("Render failed: {err}");
                }
            }
            if close_requested {
                self.render_progress = None;
                open = false;
            }
            self.show_render_dialog = open;
        }
    }
}

impl Drop for DawApp {
    fn drop(&mut self) {
        self.show_plugin_ui = false;
        self.destroy_plugin_ui();
        self.stop_audio_and_midi();
        self.leak_hosts_on_exit();
        self.startup_sink = None;
        self.startup_stream = None;
    }
}

impl DawApp {
    const UNDO_LIMIT: usize = 4096;

    fn leak_hosts_on_exit(&mut self) {
        let mut hosts: Vec<Arc<Mutex<vst3::Vst3Host>>> = Vec::new();
        for state in self.track_audio.iter_mut() {
            if let Some(host) = state.host.take() {
                if let Ok(mut host) = host.lock() {
                    host.prepare_for_drop();
                }
                hosts.push(host);
            }
            for host in state.effect_hosts.drain(..) {
                if let Ok(mut host) = host.lock() {
                    host.prepare_for_drop();
                }
                hosts.push(host);
            }
        }
        hosts.extend(self.orphaned_hosts.drain(..));
        for host in hosts {
            std::mem::forget(host);
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() {
            return;
        }
        let input = ctx.input(|i| i.clone());
        if input.modifiers.ctrl && input.key_pressed(egui::Key::Z) {
            self.undo();
        }
        if input.modifiers.ctrl && input.key_pressed(egui::Key::Y) {
            self.redo();
        }
        if input.modifiers.ctrl && input.key_pressed(egui::Key::S) {
            let _ = self.save_project_or_prompt();
        }
        if input.modifiers.ctrl && input.modifiers.shift && input.key_pressed(egui::Key::S) {
            let _ = self.save_project_dialog();
        }
        if input.modifiers.ctrl && input.key_pressed(egui::Key::O) {
            self.request_project_action(ProjectAction::OpenProject);
        }
        if input.modifiers.ctrl && input.key_pressed(egui::Key::N) {
            self.request_project_action(ProjectAction::NewProject);
        }
        if input.modifiers.ctrl && input.key_pressed(egui::Key::Comma) {
            self.show_settings = true;
        }
        if input.key_pressed(egui::Key::Space) {
            if self.audio_running {
                if self.is_recording {
                    let _ = self.end_recording();
                } else {
                    self.stop_audio_and_midi();
                }
            } else {
                let _ = self.start_audio_and_midi();
            }
        }
        if input.key_pressed(egui::Key::R) {
            self.toggle_recording();
        }
        if input.key_pressed(egui::Key::Delete) || input.key_pressed(egui::Key::Backspace) {
            if let Some(clip_id) = self.selected_clip {
                self.push_undo_state();
                self.remove_clip_by_id(clip_id);
                self.selected_clip = None;
            }
        }
        if input.modifiers.ctrl && input.key_pressed(egui::Key::D) {
            self.duplicate_selected_track();
        }
        if input.modifiers.ctrl && input.key_pressed(egui::Key::E) {
            self.show_render_dialog = true;
        }
    }

    fn sync_selected_track_index(&self) {
        let index = self.selected_track.unwrap_or(usize::MAX);
        self.selected_track_index.store(index, Ordering::Relaxed);
    }

    fn mark_dirty(&mut self) {
        self.project_dirty = true;
    }

    fn clear_dirty(&mut self) {
        self.project_dirty = false;
    }

    fn request_project_action(&mut self, action: ProjectAction) {
        if self.project_dirty {
            self.pending_project_action = Some(action);
            self.show_close_confirm = true;
        } else {
            self.perform_project_action(action);
        }
    }

    fn perform_project_action(&mut self, action: ProjectAction) {
        match action {
            ProjectAction::NewProject => {
                self.new_project();
            }
            ProjectAction::OpenProject => {
                if let Err(err) = self.open_project_dialog() {
                    self.status = format!("Open failed: {err}");
                }
            }
            ProjectAction::ImportMidi => {
                if let Err(err) = self.import_midi_dialog() {
                    self.status = format!("Import failed: {err}");
                }
            }
        }
    }

    fn sync_track_audio_states(&mut self) {
        if self.track_audio.len() != self.tracks.len() {
            self.track_audio = self
                .tracks
                .iter()
                .map(TrackAudioState::from_track)
                .collect();
        } else {
            for (index, track) in self.tracks.iter().enumerate() {
                if let Some(state) = self.track_audio.get(index) {
                    state.sync_notes(track);
                    state.sync_automation(track);
                    state.sync_effect_bypass(track);
                }
            }
        }
        self.sync_track_mix();
    }

    fn sync_track_mix(&mut self) {
        if let Ok(mut mix) = self.track_mix.lock() {
            mix.clear();
            for track in &self.tracks {
                mix.push(TrackMixState {
                    muted: track.muted,
                    solo: track.solo,
                    level: track.level,
                });
            }
        }
    }

    fn sync_track_audio_notes(&mut self, index: usize) {
        if let Some(track) = self.tracks.get(index) {
            if let Some(state) = self.track_audio.get(index) {
                state.sync_notes(track);
            }
        }
    }

    fn selected_track_host(&self) -> Option<Arc<Mutex<vst3::Vst3Host>>> {
        let index = self.selected_track?;
        self.track_audio.get(index).and_then(|state| state.host.clone())
    }

    fn ensure_track_host(&mut self, index: usize, channels: usize) -> Option<Arc<Mutex<vst3::Vst3Host>>> {
        let path = self.tracks.get(index).and_then(|t| t.instrument_path.clone())?;
        let state = self.track_audio.get_mut(index)?;
        if let Some(host) = state.host.as_ref() {
            return Some(host.clone());
        }
        let host = vst3::Vst3Host::load(
            &path,
            self.settings.sample_rate as f64,
            self.settings.buffer_size as usize,
            channels.max(1),
        )
        .ok()?;
        let host = Arc::new(Mutex::new(host));
        state.host = Some(host.clone());
        Some(host)
    }

    fn ensure_effect_host(
        &mut self,
        track_index: usize,
        effect_index: usize,
        channels: usize,
    ) -> Option<Arc<Mutex<vst3::Vst3Host>>> {
        let track = self.tracks.get(track_index)?;
        let paths = track.effect_paths.clone();
        let state = self.track_audio.get_mut(track_index)?;
        if state.effect_hosts.len() != paths.len() {
            for host in state.effect_hosts.drain(..) {
                if let Ok(mut host) = host.lock() {
                    host.prepare_for_drop();
                }
                self.orphaned_hosts.push(host);
            }
            for path in &paths {
                if let Ok(host) = vst3::Vst3Host::load_with_input(
                    path,
                    self.settings.sample_rate as f64,
                    self.settings.buffer_size as usize,
                    channels,
                    channels,
                ) {
                    state.effect_hosts.push(Arc::new(Mutex::new(host)));
                }
            }
        }
        state.effect_hosts.get(effect_index).cloned()
    }

    fn plugin_ui_matches(&self, target: PluginUiTarget) -> bool {
        self.plugin_ui
            .as_ref()
            .map(|ui| ui.target == target)
            .unwrap_or(false)
    }

    fn update_playhead(&mut self, ctx: &egui::Context) {
        self.tempo_bits.store(self.tempo_bpm.to_bits(), Ordering::Relaxed);
        let now = ctx.input(|i| i.time);
        if self.audio_running {
            let samples = self.transport_samples.load(Ordering::Relaxed) as f32;
            let sample_rate = self.settings.sample_rate.max(1) as f32;
            let seconds = samples / sample_rate;
            self.playhead_beats = seconds * (self.tempo_bpm / 60.0);
            self.last_frame_time = Some(now);
        } else {
            self.last_frame_time = None;
        }
        if let (Some(start), Some(end)) = (self.loop_start_beats, self.loop_end_beats) {
            if end > start && self.playhead_beats >= end {
                self.seek_playhead(start);
            }
        }
        self.update_loop_samples();
    }

    fn update_loop_samples(&mut self) {
        if let (Some(start), Some(end)) = (self.loop_start_beats, self.loop_end_beats) {
            if end > start {
                let start_samples = self.beats_to_samples(start, self.settings.sample_rate);
                let end_samples = self.beats_to_samples(end, self.settings.sample_rate);
                self.loop_start_samples.store(start_samples, Ordering::Relaxed);
                self.loop_end_samples.store(end_samples.max(start_samples + 1), Ordering::Relaxed);
                return;
            }
        }
        self.loop_start_samples.store(0, Ordering::Relaxed);
        self.loop_end_samples.store(0, Ordering::Relaxed);
    }

    fn seek_playhead(&mut self, beats: f32) {
        let beats = beats.max(0.0);
        self.playhead_beats = beats;
        let tempo = self.tempo_bpm.max(1.0);
        let seconds = beats * 60.0 / tempo;
        let samples = (seconds * self.settings.sample_rate as f32).max(0.0) as u64;
        self.transport_samples.store(samples, Ordering::Relaxed);
        self.last_frame_time = None;
    }

    fn beats_from_pos(&self, pos_x: f32, row_left: f32, beat_width: f32) -> f32 {
        ((pos_x - row_left) / beat_width).max(0.0)
    }

    fn play_startup_sound(&mut self) -> Result<(), String> {
        let bytes = include_bytes!("../../startup.wav");
        let reader = BufReader::new(std::io::Cursor::new(bytes));
        let (stream, handle) = OutputStream::try_default().map_err(|e| e.to_string())?;
        let sink = Sink::try_new(&handle).map_err(|e| e.to_string())?;
        let source = Decoder::new(reader).map_err(|e| e.to_string())?;
        sink.append(source);
        self.startup_stream = Some(stream);
        self.startup_sink = Some(sink);
        Ok(())
    }

    fn snapshot_state(&self) -> UndoState {
        UndoState {
            project_name: self.project_name.clone(),
            tempo_bpm: self.tempo_bpm,
            tracks: self.tracks.clone(),
            selected_clip: self.selected_clip,
            selected_track: self.selected_track,
        }
    }

    fn restore_state(&mut self, state: UndoState) {
        self.project_name = state.project_name;
        self.tempo_bpm = state.tempo_bpm;
        self.tracks = state.tracks;
        self.selected_clip = state.selected_clip;
        self.selected_track = state.selected_track;
    }

    fn push_undo_state(&mut self) {
        if self.undo_stack.len() >= Self::UNDO_LIMIT {
            self.undo_stack.remove(0);
        }
        self.undo_stack.push(self.snapshot_state());
        self.redo_stack.clear();
        self.mark_dirty();
    }

    fn undo(&mut self) {
        if let Some(state) = self.undo_stack.pop() {
            let current = self.snapshot_state();
            self.redo_stack.push(current);
            self.restore_state(state);
            self.status = "Undo".to_string();
        }
    }

    fn redo(&mut self) {
        if let Some(state) = self.redo_stack.pop() {
            let current = self.snapshot_state();
            self.undo_stack.push(current);
            self.restore_state(state);
            self.status = "Redo".to_string();
        }
    }

    fn remove_clip_by_id(&mut self, clip_id: usize) -> Option<Clip> {
        for track in &mut self.tracks {
            if let Some(pos) = track.clips.iter().position(|c| c.id == clip_id) {
                return Some(track.clips.remove(pos));
            }
        }
        None
    }

    fn move_clip_by_id(&mut self, clip_id: usize, target_track: usize, start_beats: f32) {
        let mut clip = match self.remove_clip_by_id(clip_id) {
            Some(clip) => clip,
            None => return,
        };
        let safe_track = target_track.min(self.tracks.len().saturating_sub(1));
        clip.track = safe_track;
        clip.start_beats = start_beats.max(0.0);
        if let Some(track) = self.tracks.get_mut(safe_track) {
            track.clips.push(clip);
        }
    }

    fn shift_midi_notes_for_clip_move(
        &mut self,
        source_track: usize,
        target_track: usize,
        start_beats: f32,
        length_beats: f32,
        delta_beats: f32,
    ) {
        let end_beats = start_beats + length_beats;
        if source_track == target_track {
            if delta_beats.abs() <= f32::EPSILON {
                return;
            }
            if let Some(track) = self.tracks.get_mut(source_track) {
                for note in &mut track.midi_notes {
                    let note_end = note.start_beats + note.length_beats;
                    if note.start_beats < end_beats && note_end > start_beats {
                        note.start_beats = (note.start_beats + delta_beats).max(0.0);
                    }
                }
            }
            self.sync_track_audio_notes(source_track);
            return;
        }

        let mut moved = Vec::new();
        if let Some(track) = self.tracks.get_mut(source_track) {
            let mut index = 0;
            while index < track.midi_notes.len() {
                let note = &track.midi_notes[index];
                let note_end = note.start_beats + note.length_beats;
                if note.start_beats < end_beats && note_end > start_beats {
                    let mut note = track.midi_notes.remove(index);
                    note.start_beats = (note.start_beats + delta_beats).max(0.0);
                    moved.push(note);
                } else {
                    index += 1;
                }
            }
        }
        if let Some(track) = self.tracks.get_mut(target_track) {
            if !moved.is_empty() {
                track.midi_notes.extend(moved);
                track.midi_notes.sort_by(|a, b| {
                    a.start_beats
                        .partial_cmp(&b.start_beats)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
        }
        self.sync_track_audio_notes(source_track);
        self.sync_track_audio_notes(target_track);
    }

    fn update_clip_by_id<F>(&mut self, clip_id: usize, mut apply: F)
    where
        F: FnMut(&mut Clip),
    {
        for track in &mut self.tracks {
            if let Some(clip) = track.clips.iter_mut().find(|c| c.id == clip_id) {
                apply(clip);
                return;
            }
        }
    }

    fn clip_palette_color(&self, index: usize) -> egui::Color32 {
        let palette = [
            egui::Color32::from_rgb(237, 74, 55),
            egui::Color32::from_rgb(247, 148, 30),
            egui::Color32::from_rgb(247, 216, 70),
            egui::Color32::from_rgb(69, 200, 112),
            egui::Color32::from_rgb(59, 170, 235),
            egui::Color32::from_rgb(74, 100, 216),
            egui::Color32::from_rgb(154, 83, 214),
        ];
        palette[index % palette.len()]
    }

    fn track_color(&self, track_index: usize) -> egui::Color32 {
        self.clip_palette_color(track_index)
    }

    fn colored_slider(
        ui: &mut egui::Ui,
        value: &mut f32,
        range: std::ops::RangeInclusive<f32>,
        color: Option<egui::Color32>,
    ) -> egui::Response {
        if let Some(color) = color {
            ui.scope(|ui| {
                let mut visuals = ui.visuals().clone();
                visuals.widgets.inactive.bg_fill = color.linear_multiply(0.35);
                visuals.widgets.hovered.bg_fill = color.linear_multiply(0.5);
                visuals.widgets.active.bg_fill = color.linear_multiply(0.8);
                visuals.widgets.inactive.fg_stroke.color = egui::Color32::from_gray(200);
                visuals.widgets.hovered.fg_stroke.color = egui::Color32::from_gray(230);
                visuals.widgets.active.fg_stroke.color = egui::Color32::from_gray(240);
                ui.style_mut().visuals = visuals;
                ui.add(egui::Slider::new(value, range).show_value(false))
            })
            .inner
        } else {
            ui.add(egui::Slider::new(value, range).show_value(false))
        }
    }

    fn ensure_live_params(&mut self) {
        let Some(index) = self.selected_track else {
            return;
        };
        let host = self.selected_track_host();
        let Some(track) = self.tracks.get_mut(index) else {
            return;
        };
        if !track.param_ids.is_empty() && track.param_ids.len() == track.params.len() {
            return;
        }
        let Some(host) = host else {
            return;
        };
        let params = match host.try_lock() {
            Ok(host) => host.enumerate_params(),
            Err(_) => return,
        };
        if params.is_empty() {
            return;
        }
        track.params = params.iter().map(|p| p.name.clone()).collect();
        track.param_ids = params.iter().map(|p| p.id).collect();
        track.param_values = params.iter().map(|p| p.default_value as f32).collect();
    }

    fn tint(color: egui::Color32, amount: f32) -> egui::Color32 {
        let r = (color.r() as f32 * amount).min(255.0) as u8;
        let g = (color.g() as f32 * amount).min(255.0) as u8;
        let b = (color.b() as f32 * amount).min(255.0) as u8;
        egui::Color32::from_rgb(r, g, b)
    }

    fn apply_theme(&self, ctx: &egui::Context) {
        let mut visuals = egui::Visuals::dark();
        match self.settings.theme.as_str() {
            "Black" => {
                visuals.window_fill = egui::Color32::from_rgb(0, 0, 0);
                visuals.panel_fill = egui::Color32::from_rgb(0, 0, 0);
                visuals.faint_bg_color = egui::Color32::from_rgb(10, 10, 10);
                visuals.extreme_bg_color = egui::Color32::from_rgb(0, 0, 0);
                visuals.override_text_color = Some(egui::Color32::from_rgb(245, 245, 245));
                visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(14, 14, 14);
                visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(22, 22, 22);
                visuals.widgets.active.bg_fill = egui::Color32::from_rgb(36, 36, 36);
                visuals.selection.bg_fill = egui::Color32::from_rgb(60, 60, 60);
                visuals.selection.stroke.color = egui::Color32::from_rgb(240, 240, 240);
            }
            "Dark" => {
                visuals = egui::Visuals::dark();
            }
            _ => {}
        }
        ctx.set_visuals(visuals);
    }

    fn outlined_text(
        painter: &egui::Painter,
        pos: egui::Pos2,
        align: egui::Align2,
        text: &str,
        font: egui::FontId,
        color: egui::Color32,
    ) {
        let outline = egui::Color32::from_rgba_premultiplied(0, 0, 0, 150);
        let offsets = [
            egui::vec2(-0.75, 0.0),
            egui::vec2(0.75, 0.0),
            egui::vec2(0.0, -0.75),
            egui::vec2(0.0, 0.75),
            egui::vec2(-0.6, -0.6),
            egui::vec2(0.6, -0.6),
            egui::vec2(-0.6, 0.6),
            egui::vec2(0.6, 0.6),
        ];
        for offset in offsets {
            painter.text(pos + offset, align, text, font.clone(), outline);
        }
        painter.text(pos, align, text, font, color);
    }

    fn draw_midi_preview(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        notes: &[PianoRollNote],
        clip_start: f32,
        clip_len: f32,
        clip_left: f32,
        beat_width: f32,
    ) {
        let clip_len = clip_len.max(0.001);
        let painter = painter.with_clip_rect(rect);
        let mut pitch_set: HashSet<u8> = HashSet::new();
        for note in notes {
            if note.start_beats + note.length_beats < clip_start {
                continue;
            }
            if note.start_beats > clip_start + clip_len {
                continue;
            }
            pitch_set.insert(note.midi_note);
        }
        if pitch_set.is_empty() {
            return;
        }
        let mut pitch_rows: Vec<u8> = pitch_set.into_iter().collect();
        pitch_rows.sort_unstable();
        let mut pitch_map: HashMap<u8, usize> = HashMap::new();
        for (index, pitch) in pitch_rows.iter().enumerate() {
            pitch_map.insert(*pitch, index);
        }
        let row_count = pitch_rows.len().max(1) as f32;
        let note_height = (rect.height() / row_count).max(2.0);
        for (index, note) in notes.iter().enumerate() {
            let note_end = note.start_beats + note.length_beats;
            if note_end < clip_start || note.start_beats > clip_start + clip_len {
                continue;
            }
            let local_start = (note.start_beats - clip_start).max(0.0);
            let local_len = note.length_beats.min(clip_len - local_start).max(0.0);
            let x = clip_left + local_start * beat_width;
            let w = (local_len * beat_width).max(2.0);
            let row_index = pitch_map.get(&note.midi_note).copied().unwrap_or(0) as f32;
            let y = rect.bottom() - (row_index + 1.0) * note_height;
            let note_rect = egui::Rect::from_min_size(
                egui::pos2(x, y),
                egui::vec2(w, note_height * 0.9),
            );
            let base = if index % 2 == 0 {
                egui::Color32::from_rgb(88, 210, 180)
            } else {
                egui::Color32::from_rgb(120, 130, 240)
            };
            let vel = (note.velocity as f32 / 127.0).clamp(0.0, 1.0);
            let alpha = (vel * 200.0 + 30.0).clamp(40.0, 230.0) as u8;
            let pan = note.pan.clamp(-1.0, 1.0);
            let pan_red = (pan.max(0.0) * 80.0) as u8;
            let pan_blue = ((-pan).max(0.0) * 80.0) as u8;
            let cutoff_green = (note.cutoff.clamp(0.0, 1.0) * 80.0) as u8;
            let r = (base.r() as u16 + pan_red as u16).min(255) as u8;
            let g = (base.g() as u16 + cutoff_green as u16).min(255) as u8;
            let b = (base.b() as u16 + pan_blue as u16).min(255) as u8;
            let color = egui::Color32::from_rgba_premultiplied(r, g, b, alpha);
            painter.rect_filled(note_rect, 2.0, color);
        }
    }

    fn draw_audio_preview(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        seed: usize,
        waveform: Option<&[f32]>,
        waveform_color: Option<&[[f32; 3]]>,
        clip: &Clip,
        timeline: Option<(f32, f32)>,
    ) {
        let mid_y = rect.center().y;
        if let Some(waveform) = waveform {
            let count = waveform.len().max(1);
            let step = rect.width() / count as f32;
            let time_mul = clip.audio_time_mul.max(0.01);
            let clip_len = clip.length_beats.max(0.001);
            let source_beats = self
                .get_waveform_seconds_for_clip(clip)
                .map(|seconds| (seconds * self.tempo_bpm.max(1.0) / 60.0).max(0.001))
                .unwrap_or_else(|| {
                    clip.audio_source_beats
                        .unwrap_or(clip_len / time_mul)
                        .max(0.001)
                });
            let offset_beats = clip.audio_offset_beats.max(0.0);
            let source_span = (clip_len / time_mul).max(0.0);
            for index in 0..count {
                let amp = if let Some((row_left, beat_width)) = timeline {
                    let x = rect.left() + index as f32 * step;
                    let beat = (x - row_left) / beat_width;
                    let local_beat = beat - clip.start_beats;
                    if local_beat < 0.0 || local_beat > clip_len {
                        0.0
                    } else {
                        let src_beat = (offset_beats + local_beat) / time_mul;
                        if src_beat < 0.0 || src_beat > source_beats {
                            0.0
                        } else {
                            let src_pos = if source_beats > 0.0 {
                                (src_beat / source_beats) * (count as f32 - 1.0)
                            } else {
                                index as f32
                            };
                            let left = src_pos.floor().clamp(0.0, (count - 1) as f32) as usize;
                            let right = (left + 1).min(count - 1);
                            let frac = src_pos - left as f32;
                            let amp = waveform
                                .get(left)
                                .copied()
                                .unwrap_or(0.0)
                                + (waveform.get(right).copied().unwrap_or(0.0)
                                    - waveform.get(left).copied().unwrap_or(0.0))
                                    * frac;
                            amp
                        }
                    }
                } else {
                    let t = if count > 1 {
                        index as f32 / (count as f32 - 1.0)
                    } else {
                        0.0
                    };
                    let src_beat = (offset_beats + t * clip_len) / time_mul;
                    if src_beat < 0.0 || src_beat > source_beats {
                        0.0
                    } else {
                        let src_pos = if source_beats > 0.0 {
                            (src_beat / source_beats) * (count as f32 - 1.0)
                        } else {
                            index as f32
                        };
                        let left = src_pos.floor().clamp(0.0, (count - 1) as f32) as usize;
                        let right = (left + 1).min(count - 1);
                        let frac = src_pos - left as f32;
                        let amp = waveform
                            .get(left)
                            .copied()
                            .unwrap_or(0.0)
                            + (waveform.get(right).copied().unwrap_or(0.0)
                                - waveform.get(left).copied().unwrap_or(0.0))
                                * frac;
                        amp
                    }
                };
                let x = rect.left() + index as f32 * step;
                let amp = amp.clamp(0.0, 1.0) * rect.height() * 0.45;
                let top = mid_y - amp;
                let bottom = mid_y + amp;
                let color = if let Some(bands) = waveform_color {
                    let (low, mid, high) = if let Some((row_left, beat_width)) = timeline {
                        let x = rect.left() + index as f32 * step;
                        let beat = (x - row_left) / beat_width;
                        let local_beat = beat - clip.start_beats;
                        if local_beat < 0.0 || local_beat > clip_len {
                            (0.0, 0.0, 0.0)
                        } else {
                            let src_beat = (offset_beats + local_beat) / time_mul;
                            if src_beat < 0.0 || src_beat > source_beats {
                                (0.0, 0.0, 0.0)
                            } else {
                                let src_pos = if source_beats > 0.0 {
                                    (src_beat / source_beats) * (bands.len() as f32 - 1.0)
                                } else {
                                    index as f32
                                };
                                let left = src_pos.floor().clamp(0.0, (bands.len() - 1) as f32) as usize;
                                let right = (left + 1).min(bands.len() - 1);
                                let frac = src_pos - left as f32;
                                let l = bands[left];
                                let r = bands[right];
                                (
                                    l[0] + (r[0] - l[0]) * frac,
                                    l[1] + (r[1] - l[1]) * frac,
                                    l[2] + (r[2] - l[2]) * frac,
                                )
                            }
                        }
                    } else {
                        let t = if bands.len() > 1 {
                            index as f32 / (bands.len() as f32 - 1.0)
                        } else {
                            0.0
                        };
                        let src_beat = (offset_beats + t * clip_len) / time_mul;
                        let src_pos = if source_beats > 0.0 {
                            (src_beat / source_beats) * (bands.len() as f32 - 1.0)
                        } else {
                            t * (bands.len() as f32 - 1.0)
                        };
                        let left = src_pos.floor().clamp(0.0, (bands.len() - 1) as f32) as usize;
                        let right = (left + 1).min(bands.len() - 1);
                        let frac = src_pos - left as f32;
                        let l = bands[left];
                        let r = bands[right];
                        (
                            l[0] + (r[0] - l[0]) * frac,
                            l[1] + (r[1] - l[1]) * frac,
                            l[2] + (r[2] - l[2]) * frac,
                        )
                    };
                    let alpha = ((amp / rect.height()) * 220.0 + 30.0).clamp(40.0, 230.0) as u8;
                    let r = (low * 255.0).clamp(0.0, 255.0) as u8;
                    let g = (high * 255.0).clamp(0.0, 255.0) as u8;
                    let b = (mid * 255.0).clamp(0.0, 255.0) as u8;
                    egui::Color32::from_rgba_premultiplied(r, g, b, alpha)
                } else {
                    egui::Color32::from_rgba_premultiplied(200, 220, 255, 200)
                };
                painter.line_segment(
                    [egui::pos2(x, top), egui::pos2(x, bottom)],
                    egui::Stroke::new(1.0, color),
                );
            }
            return;
        }
        let step = (rect.width() / 48.0).max(4.0);
        let mut x = rect.left();
        let mut points = Vec::new();
        let seed_f = (seed as f32 * 13.7).sin().abs().max(0.2);
        while x <= rect.right() {
            let t = (x - rect.left()) / rect.width() * 6.28 * 3.0;
            let amp = (t.sin() * 0.6 + (t * 0.5 + seed_f).sin() * 0.4) * rect.height() * 0.25;
            points.push(egui::pos2(x, mid_y + amp));
            x += step;
        }
        painter.add(egui::Shape::line(
            points,
            egui::Stroke::new(1.2, egui::Color32::from_rgba_premultiplied(255, 255, 255, 180)),
        ));
    }

    fn resolve_clip_audio_path(&self, clip: &Clip) -> Option<PathBuf> {
        let rel = clip.audio_path.as_ref()?;
        let path = PathBuf::from(rel);
        if path.is_absolute() {
            return Some(path);
        }
        if !self.project_path.trim().is_empty() {
            return Some(PathBuf::from(self.project_path.trim()).join(rel));
        }
        self.default_project_dir().map(|dir| dir.join(rel))
    }

    fn get_waveform_for_clip(&self, clip: &Clip) -> Option<Vec<f32>> {
        let path = self.resolve_clip_audio_path(clip)?;
        let key = path.to_string_lossy().to_string();
        {
            let mut cache = self.waveform_cache.borrow_mut();
            if !cache.contains_key(&key) {
                if let Some(data) = Self::build_waveform(&path, 768) {
                    cache.insert(key.clone(), data);
                }
            }
            cache.get(&key).cloned()
        }
    }

    fn get_waveform_color_for_clip(&self, clip: &Clip) -> Option<Vec<[f32; 3]>> {
        let path = self.resolve_clip_audio_path(clip)?;
        let key = path.to_string_lossy().to_string();
        {
            let mut cache = self.waveform_color_cache.borrow_mut();
            if !cache.contains_key(&key) {
                if let Some(data) = Self::build_waveform_color(&path, 768) {
                    cache.insert(key.clone(), data);
                }
            }
            cache.get(&key).cloned()
        }
    }

    fn get_waveform_seconds_for_clip(&self, clip: &Clip) -> Option<f32> {
        let path = self.resolve_clip_audio_path(clip)?;
        let key = path.to_string_lossy().to_string();
        {
            let mut cache = self.waveform_len_seconds_cache.borrow_mut();
            if !cache.contains_key(&key) {
                if let Some(seconds) = Self::wav_length_seconds(&path) {
                    cache.insert(key.clone(), seconds);
                }
            }
            cache.get(&key).copied()
        }
    }

    fn automation_value_at(points: &[AutomationPoint], beat: f32) -> Option<f32> {
        if points.is_empty() {
            return None;
        }
        if points.len() == 1 {
            return Some(points[0].value);
        }
        let mut prev: Option<&AutomationPoint> = None;
        for point in points {
            if point.beat >= beat {
                if let Some(prev) = prev {
                    let span = (point.beat - prev.beat).max(0.0001);
                    let t = ((beat - prev.beat) / span).clamp(0.0, 1.0);
                    return Some(prev.value + (point.value - prev.value) * t);
                }
                return Some(point.value);
            }
            prev = Some(point);
        }
        prev.map(|p| p.value)
    }

    fn build_waveform(path: &Path, buckets: usize) -> Option<Vec<f32>> {
        if path.extension().and_then(|s| s.to_str()).map(|e| !e.eq_ignore_ascii_case("wav")).unwrap_or(true) {
            return None;
        }
        let mut reader = hound::WavReader::open(path).ok()?;
        let spec = reader.spec();
        let channels = spec.channels.max(1) as usize;
        let total_samples = reader.duration() as usize;
        let total_frames = total_samples / channels;
        if total_frames == 0 {
            return None;
        }
        let bucket_count = buckets.max(1).min(total_frames);
        let frames_per_bucket = (total_frames as f32 / bucket_count as f32).ceil() as usize;
        let mut peaks = vec![0.0f32; bucket_count];

        match spec.sample_format {
            hound::SampleFormat::Float => {
                for (index, sample) in reader.samples::<f32>().enumerate() {
                    let sample = sample.ok()?.abs();
                    let frame = index / channels;
                    let bucket = (frame / frames_per_bucket).min(bucket_count - 1);
                    if sample > peaks[bucket] {
                        peaks[bucket] = sample;
                    }
                }
            }
            hound::SampleFormat::Int => {
                if spec.bits_per_sample <= 16 {
                    let max = i16::MAX as f32;
                    for (index, sample) in reader.samples::<i16>().enumerate() {
                        let sample = (sample.ok()? as f32 / max).abs();
                        let frame = index / channels;
                        let bucket = (frame / frames_per_bucket).min(bucket_count - 1);
                        if sample > peaks[bucket] {
                            peaks[bucket] = sample;
                        }
                    }
                } else {
                    let max = i32::MAX as f32;
                    for (index, sample) in reader.samples::<i32>().enumerate() {
                        let sample = (sample.ok()? as f32 / max).abs();
                        let frame = index / channels;
                        let bucket = (frame / frames_per_bucket).min(bucket_count - 1);
                        if sample > peaks[bucket] {
                            peaks[bucket] = sample;
                        }
                    }
                }
            }
        }

        Some(peaks)
    }

    fn build_waveform_color(path: &Path, buckets: usize) -> Option<Vec<[f32; 3]>> {
        if path.extension().and_then(|s| s.to_str()).map(|e| !e.eq_ignore_ascii_case("wav")).unwrap_or(true) {
            return None;
        }
        let mut reader = hound::WavReader::open(path).ok()?;
        let spec = reader.spec();
        let channels = spec.channels.max(1) as usize;
        let sample_rate = spec.sample_rate.max(1) as f32;
        let total_samples = reader.duration() as usize;
        let total_frames = total_samples / channels;
        if total_frames == 0 {
            return None;
        }
        let bucket_count = buckets.max(1).min(total_frames);
        let frames_per_bucket = (total_frames as f32 / bucket_count as f32).ceil() as usize;
        let mut low_sum = vec![0.0f32; bucket_count];
        let mut mid_sum = vec![0.0f32; bucket_count];
        let mut high_sum = vec![0.0f32; bucket_count];
        let mut counts = vec![0u32; bucket_count];

        let low_cut = 200.0;
        let high_cut = 2000.0;
        let alpha_low = (1.0 - (-2.0 * std::f32::consts::PI * low_cut / sample_rate).exp())
            .clamp(0.0, 1.0);
        let alpha_high = (1.0 - (-2.0 * std::f32::consts::PI * high_cut / sample_rate).exp())
            .clamp(0.0, 1.0);

        let mut low = 0.0f32;
        let mut high = 0.0f32;
        let mut frame_index = 0usize;
        let mut frame_sum = 0.0f32;
        let mut frame_count = 0usize;

        let mut push_frame = |frame_value: f32| {
            let x = frame_value;
            low += alpha_low * (x - low);
            high += alpha_high * (x - high);
            let low_band = low;
            let mid_band = (high - low).max(-1.0).min(1.0);
            let high_band = x - high;
            let bucket = (frame_index / frames_per_bucket).min(bucket_count - 1);
            low_sum[bucket] += low_band * low_band;
            mid_sum[bucket] += mid_band * mid_band;
            high_sum[bucket] += high_band * high_band;
            counts[bucket] += 1;
            frame_index += 1;
        };

        match spec.sample_format {
            hound::SampleFormat::Float => {
                for sample in reader.samples::<f32>() {
                    let sample = sample.ok()?;
                    frame_sum += sample;
                    frame_count += 1;
                    if frame_count == channels {
                        let mono = (frame_sum / channels as f32).clamp(-1.0, 1.0);
                        push_frame(mono);
                        frame_sum = 0.0;
                        frame_count = 0;
                    }
                }
            }
            hound::SampleFormat::Int => {
                if spec.bits_per_sample <= 16 {
                    let max = i16::MAX as f32;
                    for sample in reader.samples::<i16>() {
                        let sample = sample.ok()? as f32 / max;
                        frame_sum += sample;
                        frame_count += 1;
                        if frame_count == channels {
                            let mono = (frame_sum / channels as f32).clamp(-1.0, 1.0);
                            push_frame(mono);
                            frame_sum = 0.0;
                            frame_count = 0;
                        }
                    }
                } else {
                    let max = i32::MAX as f32;
                    for sample in reader.samples::<i32>() {
                        let sample = sample.ok()? as f32 / max;
                        frame_sum += sample;
                        frame_count += 1;
                        if frame_count == channels {
                            let mono = (frame_sum / channels as f32).clamp(-1.0, 1.0);
                            push_frame(mono);
                            frame_sum = 0.0;
                            frame_count = 0;
                        }
                    }
                }
            }
        }

        let mut bands = Vec::with_capacity(bucket_count);
        let mut max_val = 0.001f32;
        for i in 0..bucket_count {
            let count = counts[i].max(1) as f32;
            let low = (low_sum[i] / count).sqrt();
            let mid = (mid_sum[i] / count).sqrt();
            let high = (high_sum[i] / count).sqrt();
            max_val = max_val.max(low.max(mid).max(high));
            bands.push([low, mid, high]);
        }
        for band in &mut bands {
            band[0] = (band[0] / max_val).clamp(0.0, 1.0);
            band[1] = (band[1] / max_val).clamp(0.0, 1.0);
            band[2] = (band[2] / max_val).clamp(0.0, 1.0);
        }
        Some(bands)
    }

    fn beats_to_samples(&self, beats: f32, sample_rate: u32) -> u64 {
        let bpm = self.tempo_bpm.max(1.0);
        let samples_per_beat = sample_rate as f64 * 60.0 / bpm as f64;
        (beats.max(0.0) as f64 * samples_per_beat).round().max(0.0) as u64
    }

    fn build_audio_clip_timeline(&self, sample_rate: u32) -> Vec<AudioClipRender> {
        let mut renders = Vec::new();
        for (track_index, track) in self.tracks.iter().enumerate() {
            for clip in &track.clips {
                if clip.is_midi {
                    continue;
                }
                let Some(path) = self.resolve_clip_audio_path(clip) else {
                    continue;
                };
                let path_str = path.to_string_lossy().to_string();
                let start_samples = self.beats_to_samples(clip.start_beats, sample_rate);
                let length_samples = self.beats_to_samples(clip.length_beats, sample_rate).max(1);
                let offset_samples = self.beats_to_samples(clip.audio_offset_beats, sample_rate);
                renders.push(AudioClipRender {
                    path: path_str,
                    track_index,
                    start_samples,
                    length_samples,
                    offset_samples,
                    gain: clip.audio_gain,
                    time_mul: clip.audio_time_mul.max(0.01),
                });
            }
        }
        renders
    }

    fn build_audio_clip_render_data(
        &self,
        sample_rate: u32,
        track_filter: Option<usize>,
    ) -> (Vec<AudioClipRender>, HashMap<String, Arc<AudioClipData>>) {
        let mut renders = Vec::new();
        let mut cache = HashMap::new();
        for (track_index, track) in self.tracks.iter().enumerate() {
            if let Some(filter) = track_filter {
                if filter != track_index {
                    continue;
                }
            }
            for clip in &track.clips {
                if clip.is_midi {
                    continue;
                }
                let Some(path) = self.resolve_clip_audio_path(clip) else {
                    continue;
                };
                let path_str = path.to_string_lossy().to_string();
                let start_samples = self.beats_to_samples(clip.start_beats, sample_rate);
                let length_samples = self.beats_to_samples(clip.length_beats, sample_rate).max(1);
                let offset_samples = self.beats_to_samples(clip.audio_offset_beats, sample_rate);
                renders.push(AudioClipRender {
                    path: path_str.clone(),
                    track_index,
                    start_samples,
                    length_samples,
                    offset_samples,
                    gain: clip.audio_gain,
                    time_mul: clip.audio_time_mul.max(0.01),
                });
                if !cache.contains_key(&path_str) {
                    if let Some(data) = Self::load_audio_clip_data(&path) {
                        cache.insert(path_str.clone(), Arc::new(data));
                    }
                }
            }
        }
        (renders, cache)
    }

    fn preload_audio_clips(&self, cache: &Arc<Mutex<HashMap<String, Arc<AudioClipData>>>>) {
        for track in &self.tracks {
            for clip in &track.clips {
                if clip.is_midi {
                    continue;
                }
                let Some(path) = self.resolve_clip_audio_path(clip) else {
                    continue;
                };
                let key = path.to_string_lossy().to_string();
                let mut guard = match cache.lock() {
                    Ok(guard) => guard,
                    Err(_) => continue,
                };
                if guard.contains_key(&key) {
                    continue;
                }
                if let Some(data) = Self::load_audio_clip_data(&path) {
                    guard.insert(key, Arc::new(data));
                }
            }
        }
    }

    fn load_audio_clip_data(path: &Path) -> Option<AudioClipData> {
        if path.extension().and_then(|s| s.to_str()).map(|e| !e.eq_ignore_ascii_case("wav")).unwrap_or(true) {
            return None;
        }
        let mut reader = hound::WavReader::open(path).ok()?;
        let spec = reader.spec();
        let channels = spec.channels.max(1) as usize;
        let mut samples = Vec::new();
        match spec.sample_format {
            hound::SampleFormat::Float => {
                for sample in reader.samples::<f32>() {
                    samples.push(sample.ok()?);
                }
            }
            hound::SampleFormat::Int => {
                if spec.bits_per_sample <= 16 {
                    let max = i16::MAX as f32;
                    for sample in reader.samples::<i16>() {
                        samples.push(sample.ok()? as f32 / max);
                    }
                } else {
                    let max = i32::MAX as f32;
                    for sample in reader.samples::<i32>() {
                        samples.push(sample.ok()? as f32 / max);
                    }
                }
            }
        }
        Some(AudioClipData {
            samples,
            channels,
            sample_rate: spec.sample_rate,
        })
    }

    fn start_audio_preview(&mut self, clip: &Clip) -> Result<(), String> {
        self.stop_audio_preview();
        let path = self
            .resolve_clip_audio_path(clip)
            .ok_or_else(|| "Clip has no audio file".to_string())?;
        let file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
        let reader = BufReader::new(file);
        let (stream, handle) = OutputStream::try_default().map_err(|e| e.to_string())?;
        let sink = Sink::try_new(&handle).map_err(|e| e.to_string())?;
        let source = Decoder::new(reader).map_err(|e| e.to_string())?;
        let source = source.convert_samples::<f32>().amplify(clip.audio_gain.max(0.0));
        let source: Box<dyn Source<Item = f32> + Send> = if self.audio_preview_loop {
            Box::new(source.repeat_infinite())
        } else {
            Box::new(source)
        };
        sink.append(source);
        self.audio_preview_stream = Some(stream);
        self.audio_preview_sink = Some(sink);
        self.audio_preview_clip_id = Some(clip.id);
        Ok(())
    }

    fn stop_audio_preview(&mut self) {
        self.audio_preview_sink = None;
        self.audio_preview_stream = None;
        self.audio_preview_clip_id = None;
    }

    fn menu_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.scope(|ui| {
                    let mut style = ui.style().as_ref().clone();
                    style
                        .text_styles
                        .insert(egui::TextStyle::Button, egui::FontId::proportional(9.0));
                    ui.set_style(style);

                    let icon_size = egui::vec2(12.0, 12.0);
                    let menu_text = |text: &str| {
                        egui::RichText::new(text).size(9.0)
                    };
                    let file_color = egui::Color32::from_rgb(235, 64, 52);
                    let edit_color = egui::Color32::from_rgb(255, 140, 40);
                    let view_color = egui::Color32::from_rgb(245, 205, 70);
                    let transport_color = egui::Color32::from_rgb(80, 200, 120);
                    let help_color = egui::Color32::from_rgb(120, 80, 210);
                    ui.menu_button("File", |ui| {
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../icons/file-plus.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(file_color),
                                menu_text("New Project"),
                            ))
                            .clicked()
                        {
                        self.request_project_action(ProjectAction::NewProject);
                        ui.close_menu();
                        }
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../icons/folder.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(file_color),
                                menu_text("Open Project"),
                            ))
                            .clicked()
                        {
                        self.request_project_action(ProjectAction::OpenProject);
                        ui.close_menu();
                        }
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../icons/edit-3.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(file_color),
                                menu_text("Rename Project..."),
                            ))
                            .clicked()
                        {
                        self.begin_rename_project();
                        ui.close_menu();
                        }
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../icons/save.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(file_color),
                                menu_text("Save Project"),
                            ))
                            .clicked()
                        {
                        if let Err(err) = self.save_project_or_prompt() {
                            self.status = format!("Save failed: {err}");
                        }
                        ui.close_menu();
                        }
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../icons/save.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(file_color),
                                menu_text("Save Project As..."),
                            ))
                            .clicked()
                        {
                        if let Err(err) = self.save_project_dialog() {
                            self.status = format!("Save failed: {err}");
                        }
                        ui.close_menu();
                        }
                        ui.separator();
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../icons/download.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(file_color),
                                menu_text("Import MIDI"),
                            ))
                            .clicked()
                        {
                        self.request_project_action(ProjectAction::ImportMidi);
                        ui.close_menu();
                        }
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../icons/upload.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(file_color),
                                menu_text("Export MIDI"),
                            ))
                            .clicked()
                        {
                        if let Err(err) = self.export_midi_dialog() {
                            self.status = format!("Export failed: {err}");
                        }
                        ui.close_menu();
                        }
                        ui.separator();
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../icons/disc.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(file_color),
                                menu_text("Render to WAV..."),
                            ))
                            .clicked()
                        {
                        self.render_format = RenderFormat::Wav;
                        self.show_render_dialog = true;
                        ui.close_menu();
                        }
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../icons/disc.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(file_color),
                                menu_text("Render to OGG..."),
                            ))
                            .clicked()
                        {
                        self.render_format = RenderFormat::Ogg;
                        self.show_render_dialog = true;
                        ui.close_menu();
                        }
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../icons/disc.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(file_color),
                                menu_text("Render to FLAC..."),
                            ))
                            .clicked()
                        {
                        self.render_format = RenderFormat::Flac;
                        self.show_render_dialog = true;
                        ui.close_menu();
                        }
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../icons/settings.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(file_color),
                                menu_text("Settings..."),
                            ))
                            .clicked()
                        {
                        self.show_settings = true;
                        ui.close_menu();
                        }
                    });
                    ui.menu_button("Edit", |ui| {
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../icons/corner-left-up.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(edit_color),
                                menu_text("Undo"),
                            ))
                            .clicked()
                        {
                        self.undo();
                        }
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../icons/corner-right-up.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(edit_color),
                                menu_text("Redo"),
                            ))
                            .clicked()
                        {
                        self.redo();
                        }
                        ui.separator();
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../icons/scissors.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(edit_color),
                                menu_text("Cut"),
                            ))
                            .clicked()
                        {
                        self.status = "Cut".to_string();
                        }
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../icons/copy.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(edit_color),
                                menu_text("Copy"),
                            ))
                            .clicked()
                        {
                        self.status = "Copy".to_string();
                        }
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../icons/clipboard.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(edit_color),
                                menu_text("Paste"),
                            ))
                            .clicked()
                        {
                        self.status = "Paste".to_string();
                        }
                    });
                    ui.menu_button("View", |ui| {
                        let mut show = self.show_project_info;
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::Image::new(egui::include_image!("../../icons/info.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(view_color),
                            );
                            if ui.checkbox(&mut show, "Project Info").changed() {
                                self.show_project_info = show;
                            }
                        });
                        let mut show_meta = self.show_metadata;
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::Image::new(egui::include_image!("../../icons/tag.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(view_color),
                            );
                            if ui.checkbox(&mut show_meta, "Metadata").changed() {
                                self.show_metadata = show_meta;
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::Image::new(egui::include_image!("../../icons/crosshair.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(view_color),
                            );
                            ui.checkbox(&mut self.show_hitboxes, "Debug Hitboxes");
                        });
                    });
                    ui.menu_button("Transport", |ui| {
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../icons/play.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(transport_color),
                                menu_text("Play"),
                            ))
                            .clicked()
                        {
                        if let Err(err) = self.start_audio_and_midi() {
                            self.status = format!("Play failed: {err}");
                        }
                        }
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../icons/stop-circle.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(transport_color),
                                menu_text("Stop"),
                            ))
                            .clicked()
                        {
                        if self.is_recording {
                            if let Err(err) = self.end_recording() {
                                self.status = format!("Stop recording failed: {err}");
                            }
                        } else {
                            self.stop_audio_and_midi();
                            self.status = "Stop".to_string();
                        }
                        }
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../icons/circle.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(transport_color),
                                menu_text("Record"),
                            ))
                            .clicked()
                        {
                        self.toggle_recording();
                        }
                    });
                    ui.menu_button("Help", |ui| {
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../icons/help-circle.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(help_color),
                                menu_text("About LingStation"),
                            ))
                            .clicked()
                        {
                        self.status = "About".to_string();
                        }
                    });
                });
            });
        });
    }

    fn view_tabs(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("view_tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Views");
                ui.toggle_value(&mut self.show_sidebar, "Sidebar");
                ui.toggle_value(&mut self.show_mixer, "Mixer");
                ui.toggle_value(&mut self.show_transport, "Transport");
                ui.separator();
                ui.label("Editor");
                ui.selectable_value(&mut self.main_tab, MainTab::Arranger, "Arranger");
                ui.selectable_value(&mut self.main_tab, MainTab::Parameters, "Parameters");
                ui.selectable_value(&mut self.main_tab, MainTab::PianoRoll, "Piano Roll");
            });
        });
    }

    fn toolbar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let play_icon = egui::Image::new(egui::include_image!("../../icons/play.svg"))
                    .fit_to_exact_size(egui::vec2(16.0, 16.0));
                if ui
                    .add(egui::Button::image(play_icon))
                    .on_hover_text("Play")
                    .clicked()
                {
                    if let Err(err) = self.start_audio_and_midi() {
                        self.status = format!("Play failed: {err}");
                    }
                }
                let stop_icon = egui::Image::new(egui::include_image!("../../icons/stop-circle.svg"))
                    .fit_to_exact_size(egui::vec2(16.0, 16.0));
                if ui
                    .add(egui::Button::image(stop_icon))
                    .on_hover_text("Stop")
                    .clicked()
                {
                    self.stop_audio_and_midi();
                    self.status = "Stop".to_string();
                }
                let rec_icon = egui::Image::new(egui::include_image!("../../icons/circle.svg"))
                    .fit_to_exact_size(egui::vec2(14.0, 14.0));
                if ui
                    .add(egui::Button::image(rec_icon))
                    .on_hover_text("Rec")
                    .clicked()
                {
                    self.toggle_recording();
                }
                if ui
                    .add(egui::Button::image(
                        egui::Image::new(egui::include_image!("../../icons/repeat.svg"))
                            .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                    ))
                    .on_hover_text("Loop Song")
                    .clicked()
                {
                    if let Some((start, end)) = self.project_clip_range() {
                        self.loop_start_beats = Some(start);
                        self.loop_end_beats = Some(end);
                        self.status = "Loop: song range".to_string();
                    } else {
                        self.status = "Loop: no clips".to_string();
                    }
                }
                ui.label("Record:");
                ui.checkbox(&mut self.record_audio, "Audio");
                ui.checkbox(&mut self.record_midi, "MIDI");
                ui.checkbox(&mut self.record_automation, "Automation");
                ui.separator();
                ui.label("Tempo");
                ui.add(egui::DragValue::new(&mut self.tempo_bpm).speed(1.0));
                ui.separator();
                ui.label(&self.status);
                if self.show_hitboxes {
                    if let Some(host) = self.selected_track_host() {
                        if let Ok(host) = host.try_lock() {
                            let last = host.debug_last_param_change();
                            let count = host.debug_last_process_param_count();
                            let (param_count, id_count) = self
                                .selected_track
                                .and_then(|i| self.tracks.get(i))
                                .map(|t| (t.params.len(), t.param_ids.len()))
                                .unwrap_or((0, 0));
                            ui.separator();
                            let ui_change = self
                                .last_ui_param_change
                                .map(|(id, value)| format!("ui {id}={value:.3}"))
                                .unwrap_or_else(|| "ui none".to_string());
                            if let Some((id, value)) = last {
                                ui.label(format!(
                                    "Param {id}={value:.3} | block {count} | {ui_change} | params {param_count} ids {id_count}"
                                ));
                            } else {
                                ui.label(format!(
                                    "Param none | block {count} | {ui_change} | params {param_count} ids {id_count}"
                                ));
                            }
                        }
                    }
                }
            });

            let raw_peak = f32::from_bits(self.master_peak_bits.load(Ordering::Relaxed));
            self.master_peak_display = (self.master_peak_display * 0.92).max(raw_peak);
            let meter_value = self.master_peak_display.clamp(0.0, 1.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label("Master");
                let meter_size = egui::vec2(160.0, 14.0);
                let (rect, _) = ui.allocate_exact_size(meter_size, egui::Sense::hover());
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 3.0, egui::Color32::from_rgb(24, 28, 32));
                let fill_w = rect.width() * meter_value;
                if fill_w > 0.0 {
                    let fill_rect = egui::Rect::from_min_size(rect.min, egui::vec2(fill_w, rect.height()));
                    let color = if meter_value > 0.9 {
                        egui::Color32::from_rgb(255, 90, 64)
                    } else if meter_value > 0.7 {
                        egui::Color32::from_rgb(250, 200, 80)
                    } else {
                        egui::Color32::from_rgb(90, 210, 120)
                    };
                    painter.rect_filled(fill_rect, 3.0, color);
                }
            });
        });
    }

    fn plugin_ui_window(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(ui_host) = self.plugin_ui.as_ref() {
            if ui_host.close_requested.swap(false, Ordering::Relaxed) {
                self.show_plugin_ui = false;
                self.plugin_ui_hidden = true;
                ui_host.editor.set_focus(false);
                hide_plugin_window(ui_host.hwnd);
                release_mouse_capture();
                ctx.request_repaint();
                return;
            }
        }
        if let Some(ui_host) = self.plugin_ui.as_ref() {
            if !is_window_visible(ui_host.hwnd) {
                if self.show_plugin_ui {
                    if self.plugin_ui_hidden {
                        show_plugin_window(ui_host.hwnd);
                        self.plugin_ui_hidden = false;
                    } else {
                        self.show_plugin_ui = false;
                        self.plugin_ui_hidden = true;
                        ctx.request_repaint();
                        return;
                    }
                } else {
                    self.plugin_ui_hidden = true;
                    ctx.request_repaint();
                    return;
                }
            }
        }
        if !self.show_plugin_ui {
            if let Some(ui_host) = self.plugin_ui.as_ref() {
                ui_host.editor.set_focus(false);
                hide_plugin_window(ui_host.hwnd);
            }
            if self.plugin_ui.is_some() {
                self.plugin_ui_hidden = true;
            }
            ctx.request_repaint();
            return;
        }

        if let Some(ui_host) = self.plugin_ui.as_ref() {
            if !is_window_alive(ui_host.hwnd) {
                self.destroy_plugin_ui();
                self.show_plugin_ui = false;
                self.plugin_ui_hidden = false;
                ctx.request_repaint();
                return;
            }
        }

        if let Some(ui_host) = self.plugin_ui.as_ref() {
            if self.plugin_ui_hidden {
                show_plugin_window(ui_host.hwnd);
                self.plugin_ui_hidden = false;
            }
            pump_plugin_messages(ui_host.hwnd);
            show_plugin_window(ui_host.hwnd);
            bring_window_to_front(ui_host.hwnd);
            ui_host.editor.set_focus(true);
            if let Some((cw, ch)) = client_window_size(ui_host.child_hwnd) {
                ui_host.editor.set_size(cw, ch);
            }
            invalidate_plugin_window(ui_host.child_hwnd);
            invalidate_plugin_window(ui_host.hwnd);
        }
        self.ensure_plugin_ui();

        let mut open = self.show_plugin_ui;
        let mut close_editor = false;
        egui::Window::new("Plugin UI")
            .open(&mut open)
            .default_size(egui::vec2(520.0, 200.0))
            .show(ctx, |ui| {
                ui.label("Plugin editor is in a native window.");
                if ui
                    .add(egui::Button::image(
                        egui::Image::new(egui::include_image!("../../icons/external-link.svg"))
                            .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                    ))
                    .on_hover_text("Bring To Front")
                    .clicked()
                {
                    if let Some(ui_host) = self.plugin_ui.as_ref() {
                        bring_window_to_front(ui_host.hwnd);
                        ui_host.editor.set_focus(true);
                    }
                }
                if ui
                    .add(egui::Button::image(
                        egui::Image::new(egui::include_image!("../../icons/x.svg"))
                            .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                    ))
                    .on_hover_text("Close Editor")
                    .clicked()
                {
                    close_editor = true;
                }
            });
        if close_editor {
            if let Some(ui_host) = self.plugin_ui.as_ref() {
                ui_host.editor.set_focus(false);
                hide_plugin_window(ui_host.hwnd);
                release_mouse_capture();
            }
            self.destroy_plugin_ui();
            open = false;
            self.plugin_ui_hidden = false;
            ctx.request_repaint();
        }
        self.show_plugin_ui = open;
        if !self.show_plugin_ui {
            if let Some(ui_host) = self.plugin_ui.as_ref() {
                ui_host.editor.set_focus(false);
                hide_plugin_window(ui_host.hwnd);
            }
            if self.plugin_ui.is_some() {
                self.plugin_ui_hidden = true;
            }
            ctx.request_repaint();
        }
    }

    fn ensure_plugin_ui(&mut self) {
        vst3::init_windows_com_for_thread();
        let target = self
            .plugin_ui_target
            .or_else(|| self.selected_track.map(PluginUiTarget::Instrument));
        if let Some(ui_host) = self.plugin_ui.as_ref() {
            if let Some(target) = target {
                if ui_host.target == target {
                    return;
                }
            }
            self.destroy_plugin_ui();
        }
        let Some(target) = target else {
            self.status = "No track selected".to_string();
            return;
        };

        let host = match target {
            PluginUiTarget::Instrument(index) => self
                .selected_track_host()
                .or_else(|| self.ensure_track_host(index, 2)),
            PluginUiTarget::Effect(track_index, fx_index) => {
                self.ensure_effect_host(track_index, fx_index, 2)
            }
        };
        let Some(host) = host else {
            self.status = "No VST3 host for UI".to_string();
            return;
        };
        let mut editor = {
            let host_guard = match host.try_lock() {
                Ok(host) => host,
                Err(_) => {
                    self.status = "Plugin busy; try again".to_string();
                    return;
                }
            };
            match host_guard.create_editor() {
                Some(editor) => editor,
                None => {
                    self.status = "Plugin has no UI".to_string();
                    return;
                }
            }
        };
        let (w, h) = editor.get_size().unwrap_or((520, 360));
        eprintln!("Plugin UI size hint: {w}x{h}");
        let hwnd = match create_plugin_top_window(w, h) {
            Some(hwnd) => hwnd,
            None => {
                self.status = "Failed to create plugin UI window".to_string();
                return;
            }
        };
        eprintln!("Plugin UI window hwnd={hwnd}");
        let mut child_hwnd = match create_plugin_child_window(hwnd) {
            Some(child_hwnd) => child_hwnd,
            None => {
                self.status = "Failed to create plugin UI child window".to_string();
                destroy_plugin_child_window(hwnd);
                return;
            }
        };
        move_plugin_child_window(child_hwnd, 0, 0, w.max(200), h.max(120));
        let mut attached = editor.attach_hwnd(child_hwnd).is_ok();
        if !attached {
            destroy_plugin_child_window(child_hwnd);
            child_hwnd = hwnd;
            attached = editor.attach_hwnd(child_hwnd).is_ok();
        }
        if !attached {
            self.status = "VST3 view attach failed".to_string();
            destroy_plugin_child_window(hwnd);
            return;
        }
        eprintln!("Plugin UI attached");
        let (cw, ch) = client_window_size(child_hwnd).unwrap_or((w, h));
        editor.set_size(cw, ch);
        editor.set_focus(true);
        bring_window_to_front(hwnd);
        invalidate_plugin_window(child_hwnd);
        invalidate_plugin_window(hwnd);
        let close_requested = Arc::new(AtomicBool::new(false));
        set_plugin_close_flag(hwnd, &close_requested);
        self.plugin_ui = Some(PluginUiHost {
            hwnd,
            child_hwnd,
            editor,
            host: host.clone(),
            target,
            close_requested,
        });
    }

    fn destroy_plugin_ui(&mut self) {
        let Some(mut ui_host) = self.plugin_ui.take() else {
            return;
        };
        ui_host.editor.removed();
        if ui_host.child_hwnd != ui_host.hwnd && is_window_alive(ui_host.child_hwnd) {
            destroy_plugin_child_window(ui_host.child_hwnd);
        }
        if is_window_alive(ui_host.hwnd) {
            destroy_plugin_child_window(ui_host.hwnd);
        }
        release_mouse_capture();
        self.plugin_ui_target = None;
        self.plugin_ui_hidden = false;
    }

    fn left_sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("project_browser")
            .default_width(220.0)
            .resizable(true)
            .show(ctx, |ui| {
            ui.heading("Project");
            ui.separator();
            let root = if !self.project_path.trim().is_empty() {
                PathBuf::from(self.project_path.trim())
            } else {
                self.default_project_dir().unwrap_or_else(|| PathBuf::from("."))
            };
            let root_key = Self::fs_key(&root);
            if self.fs_expanded.is_empty() {
                self.fs_expanded.insert(root_key.clone());
            }
            let root_label = root
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| root.to_string_lossy().to_string());
            self.render_fs_row(ui, &root_label, &root_key, 0, true, true);
            ui.add_space(4.0);
            egui::ScrollArea::vertical().show(ui, |ui| {
                let entries = self.list_project_entries(&root);
                if entries.is_empty() {
                    ui.label("(no files)");
                    return;
                }
                for entry in entries {
                    self.render_fs_tree(ui, entry, 1);
                }
            });
        });
    }

    fn fs_key(path: &Path) -> String {
        path.to_string_lossy().to_string()
    }

    fn list_project_entries(&self, root: &Path) -> Vec<FsEntry> {
        let mut dirs: Vec<FsEntry> = Vec::new();
        let mut files: Vec<FsEntry> = Vec::new();
        let entries = match fs::read_dir(root) {
            Ok(entries) => entries,
            Err(_) => return Vec::new(),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                dirs.push(FsEntry {
                    name,
                    path,
                    is_dir: true,
                });
            } else {
                files.push(FsEntry {
                    name,
                    path,
                    is_dir: false,
                });
            }
        }
        dirs.sort_by(|a, b| a.name.cmp(&b.name));
        files.sort_by(|a, b| a.name.cmp(&b.name));
        dirs.extend(files);
        dirs
    }

    fn render_fs_tree(&mut self, ui: &mut egui::Ui, entry: FsEntry, depth: usize) {
        let key = Self::fs_key(&entry.path);
        let is_open = self.fs_expanded.contains(&key);
        let toggled = self.render_fs_row(ui, &entry.name, &key, depth, entry.is_dir, is_open);
        if entry.is_dir {
            if toggled {
                if is_open {
                    self.fs_expanded.remove(&key);
                } else {
                    self.fs_expanded.insert(key.clone());
                }
            }
            if self.fs_expanded.contains(&key) {
                for child in self.list_project_entries(&entry.path) {
                    self.render_fs_tree(ui, child, depth + 1);
                }
            }
        }
    }

    fn render_fs_row(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        key: &str,
        depth: usize,
        is_dir: bool,
        is_open: bool,
    ) -> bool {
        let row_h = 20.0;
        let full_w = ui.available_width();
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(full_w, row_h), egui::Sense::click());
        let selected = self.fs_selected.as_deref() == Some(key);
        let hovered = response.hovered();
        if selected || hovered {
            let color = if selected {
                egui::Color32::from_rgb(38, 52, 76)
            } else {
                egui::Color32::from_rgb(30, 36, 44)
            };
            ui.painter().rect_filled(rect, 4.0, color);
        }

        let indent = 12.0;
        let x = rect.min.x + indent * depth as f32 + 6.0;
        let center_y = rect.center().y;
        let icon_color = if is_dir {
            egui::Color32::from_rgb(110, 150, 255)
        } else {
            egui::Color32::from_rgb(120, 200, 140)
        };

        if is_dir {
            let tri_size = 8.0;
            let tri_x = x;
            let tri_y = center_y - tri_size * 0.5;
            let points = if is_open {
                vec![
                    egui::pos2(tri_x, tri_y + 1.0),
                    egui::pos2(tri_x + tri_size, tri_y + 1.0),
                    egui::pos2(tri_x + tri_size * 0.5, tri_y + tri_size + 1.0),
                ]
            } else {
                vec![
                    egui::pos2(tri_x, tri_y),
                    egui::pos2(tri_x, tri_y + tri_size),
                    egui::pos2(tri_x + tri_size, tri_y + tri_size * 0.5),
                ]
            };
            ui.painter()
                .add(egui::Shape::convex_polygon(points, icon_color, egui::Stroke::NONE));
        }

        let icon_x = if is_dir { x + 12.0 } else { x + 4.0 };
        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(icon_x, center_y),
            egui::vec2(10.0, 10.0),
        );
        ui.painter()
            .rect_filled(icon_rect, 2.0, icon_color.linear_multiply(0.9));

        let text_x = icon_rect.max.x + 6.0;
        let font_id = egui::FontId::proportional(13.0);
        let text_color = if selected {
            egui::Color32::from_rgb(220, 230, 244)
        } else {
            egui::Color32::from_rgb(190, 200, 210)
        };
        let galley = ui
            .fonts(|f| f.layout_no_wrap(label.to_string(), font_id.clone(), text_color));
        let text_pos = egui::pos2(text_x, center_y - galley.size().y * 0.5);
        ui.painter().galley(text_pos, galley.clone(), text_color);

        if !is_dir {
            if let Some(ext) = label.rsplit('.').next() {
                if ext.len() > 0 && ext.len() <= 6 {
                    let badge_text = ext.to_ascii_uppercase();
                    let badge_font = egui::FontId::proportional(10.0);
                    let badge_galley = ui.fonts(|f| {
                        f.layout_no_wrap(
                            badge_text.clone(),
                            badge_font.clone(),
                            egui::Color32::from_rgb(40, 44, 48),
                        )
                    });
                    let badge_size = egui::vec2(badge_galley.size().x + 8.0, 12.0);
                    let badge_x = (text_pos.x + galley.size().x + 6.0)
                        .min(rect.max.x - badge_size.x - 4.0);
                    if badge_x > text_pos.x + 6.0 {
                        let badge_rect = egui::Rect::from_min_size(
                            egui::pos2(badge_x, center_y - badge_size.y * 0.5),
                            badge_size,
                        );
                        ui.painter().rect_filled(
                            badge_rect,
                            6.0,
                            egui::Color32::from_rgb(170, 190, 210),
                        );
                        let badge_pos = egui::pos2(
                            badge_rect.min.x + 4.0,
                            center_y - badge_galley.size().y * 0.5,
                        );
                        ui.painter().galley(
                            badge_pos,
                            badge_galley,
                            egui::Color32::from_rgb(40, 44, 48),
                        );
                    }
                }
            }
        }

        if response.clicked() {
            self.fs_selected = Some(key.to_string());
        }

        is_dir && response.clicked()
    }

    fn project_info_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("info_panel")
            .default_width(260.0)
            .resizable(true)
            .show(ctx, |ui| {
            ui.heading("Project Info");
            ui.separator();
            ui.label(format!("Name: {}", self.project_name));
            ui.label("Sample Rate: 48 kHz");
            ui.label(format!("Tracks: {}", self.tracks.len()));
            ui.separator();
            ui.heading("Track List");
            let mut selected_index: Option<usize> = None;
            for (index, track) in self.tracks.iter().enumerate() {
                let selected = self.selected_track == Some(index);
                if ui
                    .selectable_label(selected, format!("{}  {}", index + 1, track.name))
                    .clicked()
                {
                    selected_index = Some(index);
                }
            }

            if let Some(index) = selected_index {
                self.selected_track = Some(index);
                self.refresh_params_for_selected_track(true);
            }
        });
    }

    fn mixer_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("mixer_panel")
            .default_width(300.0)
            .min_width(200.0)
            .max_width(350.0)
            .resizable(true)
            .show(ctx, |ui| {
            let mut style = ui.style().as_ref().clone();
            style
                .text_styles
                .insert(egui::TextStyle::Heading, egui::FontId::proportional(12.0));
            style
                .text_styles
                .insert(egui::TextStyle::Body, egui::FontId::proportional(10.0));
            style
                .text_styles
                .insert(egui::TextStyle::Button, egui::FontId::proportional(10.0));
            ui.set_style(style);

            ui.heading("Mixer");
            let show_hitboxes = self.show_hitboxes;
            ui.spacing_mut().item_spacing = egui::vec2(3.0, 3.0);
            let button_h = 16.0;
            let row_spacing = ui.spacing().item_spacing.x;
            let (top_row_rect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), button_h),
                egui::Sense::hover(),
            );
            let button_w = ((top_row_rect.width() - row_spacing * 4.0) / 5.0).max(40.0);
            let top_colors = [
                egui::Color32::from_rgb(235, 64, 52),
                egui::Color32::from_rgb(255, 140, 40),
                egui::Color32::from_rgb(245, 205, 70),
                egui::Color32::from_rgb(80, 200, 120),
                egui::Color32::from_rgb(60, 120, 220),
                egui::Color32::from_rgb(120, 80, 210),
                egui::Color32::from_rgb(200, 90, 180),
            ];
            let mut x = top_row_rect.left();
            if show_hitboxes {
                ui.painter().rect_stroke(
                    top_row_rect,
                    0.0,
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(80, 140, 255)),
                );
            }
            let top_color = top_colors[0];
            let top_fill = egui::Color32::from_rgba_premultiplied(
                top_color.r(),
                top_color.g(),
                top_color.b(),
                80,
            );
            if ui
                .put(
                    egui::Rect::from_min_size(
                        egui::pos2(x, top_row_rect.top()),
                        egui::vec2(button_w, button_h),
                    ),
                    egui::Button::image(
                        egui::Image::new(egui::include_image!("../../icons/plus.svg"))
                            .fit_to_exact_size(egui::vec2(12.0, 12.0))
                            .tint(top_color),
                    )
                    .fill(top_fill),
                )
                .on_hover_text("Add")
                .clicked()
            {
                self.add_track();
            }
            if show_hitboxes {
                let rect = egui::Rect::from_min_size(
                    egui::pos2(x, top_row_rect.top()),
                    egui::vec2(button_w, button_h),
                );
                ui.painter().rect_stroke(
                    rect,
                    0.0,
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 120, 120)),
                );
            }
            x += button_w + row_spacing;
            let top_color = top_colors[1];
            let top_fill = egui::Color32::from_rgba_premultiplied(
                top_color.r(),
                top_color.g(),
                top_color.b(),
                80,
            );
            if ui
                .put(
                    egui::Rect::from_min_size(
                        egui::pos2(x, top_row_rect.top()),
                        egui::vec2(button_w, button_h),
                    ),
                    egui::Button::image(
                        egui::Image::new(egui::include_image!("../../icons/copy.svg"))
                            .fit_to_exact_size(egui::vec2(12.0, 12.0))
                            .tint(top_color),
                    )
                    .fill(top_fill),
                )
                .on_hover_text("Copy")
                .clicked()
            {
                self.duplicate_selected_track();
            }
            if show_hitboxes {
                let rect = egui::Rect::from_min_size(
                    egui::pos2(x, top_row_rect.top()),
                    egui::vec2(button_w, button_h),
                );
                ui.painter().rect_stroke(
                    rect,
                    0.0,
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 120, 120)),
                );
            }
            x += button_w + row_spacing;
            let top_color = top_colors[2];
            let top_fill = egui::Color32::from_rgba_premultiplied(
                top_color.r(),
                top_color.g(),
                top_color.b(),
                80,
            );
            if ui
                .put(
                    egui::Rect::from_min_size(
                        egui::pos2(x, top_row_rect.top()),
                        egui::vec2(button_w, button_h),
                    ),
                    egui::Button::image(
                        egui::Image::new(egui::include_image!("../../icons/layers.svg"))
                            .fit_to_exact_size(egui::vec2(12.0, 12.0))
                            .tint(top_color),
                    )
                    .fill(top_fill),
                )
                .on_hover_text("Clone")
                .clicked()
            {
                self.clone_selected_track();
            }
            if show_hitboxes {
                let rect = egui::Rect::from_min_size(
                    egui::pos2(x, top_row_rect.top()),
                    egui::vec2(button_w, button_h),
                );
                ui.painter().rect_stroke(
                    rect,
                    0.0,
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 120, 120)),
                );
            }
            x += button_w + row_spacing;
            let top_color = top_colors[3];
            let top_fill = egui::Color32::from_rgba_premultiplied(
                top_color.r(),
                top_color.g(),
                top_color.b(),
                80,
            );
            if ui
                .put(
                    egui::Rect::from_min_size(
                        egui::pos2(x, top_row_rect.top()),
                        egui::vec2(button_w, button_h),
                    ),
                    egui::Button::image(
                        egui::Image::new(egui::include_image!("../../icons/edit-3.svg"))
                            .fit_to_exact_size(egui::vec2(12.0, 12.0))
                            .tint(top_color),
                    )
                    .fill(top_fill),
                )
                .on_hover_text("Rename")
                .clicked()
            {
                self.begin_rename_selected_track();
            }
            if show_hitboxes {
                let rect = egui::Rect::from_min_size(
                    egui::pos2(x, top_row_rect.top()),
                    egui::vec2(button_w, button_h),
                );
                ui.painter().rect_stroke(
                    rect,
                    0.0,
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 120, 120)),
                );
            }
            x += button_w + row_spacing;
            let top_color = top_colors[4];
            let top_fill = egui::Color32::from_rgba_premultiplied(
                top_color.r(),
                top_color.g(),
                top_color.b(),
                80,
            );
            if ui
                .put(
                    egui::Rect::from_min_size(
                        egui::pos2(x, top_row_rect.top()),
                        egui::vec2(button_w, button_h),
                    ),
                    egui::Button::image(
                        egui::Image::new(egui::include_image!("../../icons/trash-2.svg"))
                            .fit_to_exact_size(egui::vec2(12.0, 12.0))
                            .tint(top_color),
                    )
                    .fill(top_fill),
                )
                .on_hover_text("Remove")
                .clicked()
            {
                self.remove_selected_track();
            }
            if show_hitboxes {
                let rect = egui::Rect::from_min_size(
                    egui::pos2(x, top_row_rect.top()),
                    egui::vec2(button_w, button_h),
                );
                ui.painter().rect_stroke(
                    rect,
                    0.0,
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 120, 120)),
                );
            }
            ui.separator();
            #[derive(Clone, Copy)]
            enum MixerAction {
                Select(usize),
                PickInstrument(usize),
                ClearInstrument(usize),
                AddFx(usize),
                RemoveFx(usize, usize),
                MoveFx(usize, usize, i32),
            }

            let mut action: Option<MixerAction> = None;
            let mut selected_track = self.selected_track;
            let mut mix_dirty = false;

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                ui.set_width(ui.available_width());
                for index in 0..self.tracks.len() {
                    let selected = selected_track == Some(index);
                    let track_color = self.track_color(index);
                    let track = &mut self.tracks[index];
                    let group_response = ui.push_id(index, |ui| {
                        let strip_fill = if selected {
                            Self::tint(track_color, 0.2)
                        } else {
                            egui::Color32::from_rgba_premultiplied(
                                track_color.r(),
                                track_color.g(),
                                track_color.b(),
                                40,
                            )
                        };
                        let strip_response = egui::Frame::none()
                            .fill(strip_fill)
                            .rounding(egui::Rounding::same(0.0))
                            .inner_margin(egui::Margin::symmetric(6.0, 0.0))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.visuals_mut().override_text_color =
                                    Some(egui::Color32::from_gray(240));
                                let label = if selected {
                                    format!("> {}", track.name)
                                } else {
                                    track.name.clone()
                                };
                                let label_fill = if selected {
                                    Self::tint(track_color, 0.25)
                                } else {
                                    egui::Color32::from_rgba_premultiplied(
                                        track_color.r(),
                                        track_color.g(),
                                        track_color.b(),
                                        90,
                                    )
                                };
                                let (label_rect, _) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), 18.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().rect_filled(label_rect, 0.0, label_fill);
                                Self::outlined_text(
                                    ui.painter(),
                                    egui::pos2(label_rect.left() + 6.0, label_rect.center().y),
                                    egui::Align2::LEFT_CENTER,
                                    &label,
                                    egui::FontId::proportional(12.0),
                                    egui::Color32::from_gray(240),
                                );
                                let swatch_rect = egui::Rect::from_min_max(
                                    egui::pos2(label_rect.left(), label_rect.top()),
                                    egui::pos2(
                                        label_rect.left() + 4.0,
                                        label_rect.bottom(),
                                    ),
                                );
                                ui.painter().rect_filled(swatch_rect, 2.0, track_color);
                                if show_hitboxes {
                                    ui.painter().rect_stroke(
                                        label_rect,
                                        0.0,
                                        egui::Stroke::new(1.0, egui::Color32::from_rgb(80, 200, 140)),
                                    );
                                }
                                let (ms_row_rect, _) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), 14.0),
                                    egui::Sense::hover(),
                                );
                                let mute_rect = egui::Rect::from_min_size(
                                    egui::pos2(ms_row_rect.left(), ms_row_rect.top()),
                                    egui::vec2(24.0, 18.0),
                                );
                                let solo_rect = egui::Rect::from_min_size(
                                    egui::pos2(mute_rect.right() + row_spacing, ms_row_rect.top()),
                                    egui::vec2(24.0, 18.0),
                                );
                                let mute_id = egui::Id::new(format!("mixer_mute_{}", index));
                                let solo_id = egui::Id::new(format!("mixer_solo_{}", index));
                                let mute_resp = ui.interact(mute_rect, mute_id, egui::Sense::click());
                                let solo_resp = ui.interact(solo_rect, solo_id, egui::Sense::click());
                                let mute_bg = if track.muted {
                                    Self::tint(track_color, 0.6)
                                } else {
                                    egui::Color32::from_rgba_premultiplied(
                                        track_color.r(),
                                        track_color.g(),
                                        track_color.b(),
                                        50,
                                    )
                                };
                                let solo_bg = if track.solo {
                                    Self::tint(track_color, 0.85)
                                } else {
                                    egui::Color32::from_rgba_premultiplied(
                                        track_color.r(),
                                        track_color.g(),
                                        track_color.b(),
                                        70,
                                    )
                                };
                                ui.painter().rect_filled(mute_rect, 3.0, mute_bg);
                                ui.painter().rect_filled(solo_rect, 3.0, solo_bg);
                                Self::outlined_text(
                                    ui.painter(),
                                    mute_rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    "M",
                                    egui::FontId::proportional(11.0),
                                    egui::Color32::from_gray(220),
                                );
                                Self::outlined_text(
                                    ui.painter(),
                                    solo_rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    "S",
                                    egui::FontId::proportional(11.0),
                                    egui::Color32::from_gray(220),
                                );
                                let mute_clicked = mute_resp.clicked();
                                let solo_clicked = solo_resp.clicked();
                                if show_hitboxes {
                                    ui.painter().rect_stroke(
                                        ms_row_rect,
                                        0.0,
                                        egui::Stroke::new(1.0, egui::Color32::from_rgb(160, 120, 255)),
                                    );
                                }
                                if mute_clicked {
                                    track.muted = !track.muted;
                                    mix_dirty = true;
                                }
                                if solo_clicked {
                                    track.solo = !track.solo;
                                    mix_dirty = true;
                                }
                                let level_response = ui.add_sized(
                                    [ui.available_width(), 12.0],
                                    egui::Slider::new(&mut track.level, 0.0..=1.0).text("Level"),
                                );
                                if level_response.changed() || level_response.dragged() {
                                    mix_dirty = true;
                                }
                                let meter_height = 12.0;
                                let (meter_rect, _) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), meter_height),
                                    egui::Sense::hover(),
                                );
                                ui.painter().rect_filled(
                                    meter_rect,
                                    3.0,
                                    egui::Color32::from_rgb(16, 20, 24),
                                );
                                let (peak_l, peak_r) = self
                                    .track_audio
                                    .get(index)
                                    .map(|s| {
                                        (
                                            f32::from_bits(s.peak_l_bits.load(Ordering::Relaxed)),
                                            f32::from_bits(s.peak_r_bits.load(Ordering::Relaxed)),
                                        )
                                    })
                                    .unwrap_or((0.0, 0.0));
                                let peak_l = peak_l.clamp(0.0, 1.0);
                                let peak_r = peak_r.clamp(0.0, 1.0);
                                let bar_h = (meter_rect.height() - 2.0) * 0.5;
                                let left_rect = egui::Rect::from_min_size(
                                    meter_rect.min + egui::vec2(0.0, 1.0),
                                    egui::vec2(meter_rect.width(), bar_h),
                                );
                                let right_rect = egui::Rect::from_min_size(
                                    egui::pos2(meter_rect.left(), meter_rect.top() + 1.0 + bar_h),
                                    egui::vec2(meter_rect.width(), bar_h),
                                );
                                let fill_l = left_rect.width() * peak_l;
                                let fill_r = right_rect.width() * peak_r;
                                if fill_l > 0.0 {
                                    let color = if peak_l > 0.9 {
                                        egui::Color32::from_rgb(255, 90, 64)
                                    } else if peak_l > 0.7 {
                                        egui::Color32::from_rgb(250, 200, 80)
                                    } else {
                                        egui::Color32::from_rgb(90, 210, 120)
                                    };
                                    let fill_rect = egui::Rect::from_min_size(
                                        left_rect.min,
                                        egui::vec2(fill_l, left_rect.height()),
                                    );
                                    ui.painter().rect_filled(fill_rect, 2.0, color);
                                }
                                if fill_r > 0.0 {
                                    let color = if peak_r > 0.9 {
                                        egui::Color32::from_rgb(255, 90, 64)
                                    } else if peak_r > 0.7 {
                                        egui::Color32::from_rgb(250, 200, 80)
                                    } else {
                                        egui::Color32::from_rgb(90, 210, 120)
                                    };
                                    let fill_rect = egui::Rect::from_min_size(
                                        right_rect.min,
                                        egui::vec2(fill_r, right_rect.height()),
                                    );
                                    ui.painter().rect_filled(fill_rect, 2.0, color);
                                }
                                ui.separator();
                                ui.label("Effects");
                                let mut bypass_dirty = false;
                                for (fx_index, fx) in track.effect_paths.iter().enumerate() {
                                    ui.horizontal(|ui| {
                                        ui.label(format!("{}:", fx_index + 1));
                                        ui.label(Self::plugin_display_name(fx));
                                        if let Some(bypass) = track.effect_bypass.get_mut(fx_index) {
                                            if ui.checkbox(bypass, "Byp").changed() {
                                                bypass_dirty = true;
                                            }
                                        }
                                        if ui
                                            .add(
                                                egui::Button::image(
                                                    egui::Image::new(egui::include_image!(
                                                        "../../icons/chevron-up.svg"
                                                    ))
                                                    .fit_to_exact_size(egui::vec2(12.0, 12.0))
                                                    .tint(track_color),
                                                ),
                                            )
                                            .on_hover_text("Up")
                                            .clicked()
                                        {
                                            selected_track = Some(index);
                                            action = Some(MixerAction::MoveFx(index, fx_index, -1));
                                        }
                                        if ui
                                            .add(
                                                egui::Button::image(
                                                    egui::Image::new(egui::include_image!(
                                                        "../../icons/chevron-down.svg"
                                                    ))
                                                    .fit_to_exact_size(egui::vec2(12.0, 12.0))
                                                    .tint(track_color),
                                                ),
                                            )
                                            .on_hover_text("Down")
                                            .clicked()
                                        {
                                            selected_track = Some(index);
                                            action = Some(MixerAction::MoveFx(index, fx_index, 1));
                                        }
                                        if ui
                                            .add(
                                                egui::Button::image(
                                                    egui::Image::new(egui::include_image!(
                                                        "../../icons/eye.svg"
                                                    ))
                                                    .fit_to_exact_size(egui::vec2(12.0, 12.0))
                                                    .tint(track_color),
                                                ),
                                            )
                                            .on_hover_text("View")
                                            .clicked()
                                        {
                                            selected_track = Some(index);
                                            self.plugin_ui_target =
                                                Some(PluginUiTarget::Effect(index, fx_index));
                                            self.show_plugin_ui = true;
                                        }
                                        if ui
                                            .add(
                                                egui::Button::image(
                                                    egui::Image::new(egui::include_image!(
                                                        "../../icons/trash-2.svg"
                                                    ))
                                                    .fit_to_exact_size(egui::vec2(12.0, 12.0))
                                                    .tint(track_color),
                                                ),
                                            )
                                            .on_hover_text("Remove")
                                            .clicked()
                                        {
                                            selected_track = Some(index);
                                            action = Some(MixerAction::RemoveFx(index, fx_index));
                                        }
                                    });
                                }
                                if bypass_dirty {
                                    if let Some(state) = self.track_audio.get(index) {
                                        state.sync_effect_bypass(track);
                                    }
                                }
                                let mut add_rect = None;
                                ui.horizontal(|ui| {
                                    ui.set_height(button_h);
                                    let add = ui
                                        .add(egui::Button::image(
                                            egui::Image::new(egui::include_image!(
                                                "../../icons/plus.svg"
                                            ))
                                            .fit_to_exact_size(egui::vec2(12.0, 12.0))
                                            .tint(track_color),
                                        ))
                                        .on_hover_text("Add FX");
                                    add_rect = Some(add.rect);
                                    if add.clicked() {
                                        selected_track = Some(index);
                                        action = Some(MixerAction::AddFx(index));
                                    }
                                });
                                if show_hitboxes {
                                    if let Some(rect) = add_rect {
                                        ui.painter().rect_stroke(
                                            rect,
                                            0.0,
                                            egui::Stroke::new(1.0, egui::Color32::from_rgb(80, 200, 255)),
                                        );
                                    }
                                }
                            })
                            .response;
                        if strip_response.hovered()
                            && ui.input(|i| i.pointer.primary_clicked())
                        {
                            selected_track = Some(index);
                            action = Some(MixerAction::Select(index));
                        }
                    });
                    if show_hitboxes {
                        ui.painter().rect_stroke(
                            group_response.response.rect,
                            0.0,
                            egui::Stroke::new(1.0, egui::Color32::from_rgb(80, 160, 255)),
                        );
                    }
                }
            });

            if let Some(action) = action {
                match action {
                    MixerAction::Select(index) => {
                        self.selected_track = Some(index);
                        self.refresh_params_for_selected_track(true);
                    }
                    MixerAction::PickInstrument(index) => {
                        self.open_plugin_picker(PluginTarget::Instrument(index));
                    }
                    MixerAction::ClearInstrument(index) => {
                        if self.plugin_ui_matches(PluginUiTarget::Instrument(index)) {
                            self.show_plugin_ui = false;
                            self.destroy_plugin_ui();
                        }
                        if let Some(track) = self.tracks.get_mut(index) {
                            track.instrument_path = None;
                            track.params = default_midi_params();
                            track.param_ids.clear();
                            track.param_values.clear();
                        }
                        if let Some(state) = self.track_audio.get_mut(index) {
                            if let Some(host) = state.host.take() {
                                if let Ok(mut host) = host.lock() {
                                    host.prepare_for_drop();
                                }
                                self.orphaned_hosts.push(host);
                            }
                        }
                        self.reinit_audio_if_running();
                    }
                    MixerAction::AddFx(index) => {
                        self.open_plugin_picker(PluginTarget::Effect(index));
                    }
                    MixerAction::RemoveFx(index, fx_index) => {
                        if self.plugin_ui_matches(PluginUiTarget::Effect(index, fx_index)) {
                            self.show_plugin_ui = false;
                            self.destroy_plugin_ui();
                        }
                        if let Some(track) = self.tracks.get_mut(index) {
                            if fx_index < track.effect_paths.len() {
                                track.effect_paths.remove(fx_index);
                            }
                            if fx_index < track.effect_bypass.len() {
                                track.effect_bypass.remove(fx_index);
                            }
                            if fx_index < track.effect_params.len() {
                                track.effect_params.remove(fx_index);
                            }
                            if fx_index < track.effect_param_ids.len() {
                                track.effect_param_ids.remove(fx_index);
                            }
                            if fx_index < track.effect_param_values.len() {
                                track.effect_param_values.remove(fx_index);
                            }
                        }
                        if let Some(state) = self.track_audio.get_mut(index) {
                            if fx_index < state.effect_hosts.len() {
                                let host = state.effect_hosts.remove(fx_index);
                                if let Ok(mut host) = host.lock() {
                                    host.prepare_for_drop();
                                }
                                self.orphaned_hosts.push(host);
                            }
                        }
                        self.reinit_audio_if_running();
                    }
                    MixerAction::MoveFx(index, fx_index, direction) => {
                        let target_index = if direction < 0 {
                            fx_index.saturating_sub(1)
                        } else {
                            fx_index + 1
                        };
                        let mut moved = false;
                        if let Some(track) = self.tracks.get_mut(index) {
                            if target_index < track.effect_paths.len() {
                                track.effect_paths.swap(fx_index, target_index);
                                if target_index < track.effect_bypass.len() {
                                    track.effect_bypass.swap(fx_index, target_index);
                                }
                                if target_index < track.effect_params.len() {
                                    track.effect_params.swap(fx_index, target_index);
                                }
                                if target_index < track.effect_param_ids.len() {
                                    track.effect_param_ids.swap(fx_index, target_index);
                                }
                                if target_index < track.effect_param_values.len() {
                                    track.effect_param_values.swap(fx_index, target_index);
                                }
                                moved = true;
                            }
                        }
                        if moved {
                            if let Some(state) = self.track_audio.get_mut(index) {
                                if target_index < state.effect_hosts.len() {
                                    state.effect_hosts.swap(fx_index, target_index);
                                }
                                if let Some(track) = self.tracks.get(index) {
                                    state.sync_effect_bypass(track);
                                }
                            }
                            if let Some(target) = self.plugin_ui_target {
                                if matches!(
                                    target,
                                    PluginUiTarget::Effect(ti, fi)
                                        if ti == index && (fi == fx_index || fi == target_index)
                                ) {
                                    self.show_plugin_ui = false;
                                    self.destroy_plugin_ui();
                                }
                            }
                        }
                    }
                }
            }
            if mix_dirty {
                self.sync_track_mix();
            }
        });
    }

    fn center_arranger(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Arranger");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Tools");
                let tool_size = egui::vec2(110.0, 22.0);
                let icon_size = egui::vec2(14.0, 14.0);
                let button_bg = egui::Color32::from_rgba_premultiplied(18, 20, 24, 220);
                let button_on = egui::Color32::from_rgba_premultiplied(46, 94, 130, 220);
                let icon_tint = egui::Color32::from_gray(220);
                let mut tool_button = |tool: ArrangerTool, icon: egui::ImageSource<'static>, label: &str| {
                    let selected = self.arranger_tool == tool;
                    let mut button = egui::Button::image_and_text(
                        egui::Image::new(icon).fit_to_exact_size(icon_size).tint(icon_tint),
                        label,
                    )
                    .min_size(tool_size)
                    .fill(if selected { button_on } else { button_bg });
                    if ui.add(button).clicked() {
                        self.arranger_tool = tool;
                    }
                };
                tool_button(
                    ArrangerTool::Draw,
                    egui::include_image!("../../icons/arranger-write.svg"),
                    "Draw MIDI",
                );
                tool_button(
                    ArrangerTool::Select,
                    egui::include_image!("../../icons/arranger-box-select.svg"),
                    "Select (Box)",
                );
                tool_button(
                    ArrangerTool::Move,
                    egui::include_image!("../../icons/arranger-move.svg"),
                    "Move",
                );
            });
            ui.add_space(6.0);
            ui.add_space(6.0);
            let row_height = 52.0;
            let beat_width = 22.0 * self.arranger_zoom;
            let header_height = 24.0;
            let lane_label_w = 160.0;
            #[derive(Clone, Copy)]
            enum ArrangerRow {
                Track { track_index: usize },
                Automation { track_index: usize, lane_index: usize },
            }
            let mut rows: Vec<ArrangerRow> = Vec::new();
            let mut track_row_indices = vec![0usize; self.tracks.len()];
            for (track_index, track) in self.tracks.iter().enumerate() {
                track_row_indices[track_index] = rows.len();
                rows.push(ArrangerRow::Track { track_index });
                if self.automation_rows_expanded.contains(&track_index) {
                    for (lane_index, _lane) in track.automation_lanes.iter().enumerate() {
                        rows.push(ArrangerRow::Automation {
                            track_index,
                            lane_index,
                        });
                    }
                }
            }
            let (rect, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
            let pointer_pos = response
                .hover_pos()
                .or_else(|| ctx.input(|i| i.pointer.hover_pos()));
            let over_arranger = pointer_pos
                .map(|pos| rect.contains(pos))
                .unwrap_or(false);
            if over_arranger && !self.piano_roll_hovered {
                let input = ctx.input(|i| i.clone());
                if input.modifiers.ctrl {
                    let zoom = input.zoom_delta();
                    if (zoom - 1.0).abs() > f32::EPSILON {
                        self.arranger_zoom = (self.arranger_zoom * zoom).clamp(0.3, 4.0);
                    } else {
                        let mut delta = input.smooth_scroll_delta;
                        if delta == egui::Vec2::ZERO {
                            delta = input.raw_scroll_delta;
                        }
                        let zoom_delta = (delta.x + delta.y) * 0.01;
                        self.arranger_zoom = (self.arranger_zoom + zoom_delta).clamp(0.3, 4.0);
                    }
                } else {
                    let mut delta = input.smooth_scroll_delta;
                    if delta == egui::Vec2::ZERO {
                        delta = input.raw_scroll_delta;
                    }
                    if input.modifiers.shift && delta.x.abs() < f32::EPSILON {
                        delta = egui::vec2(delta.y, 0.0);
                    }
                    self.arranger_pan += egui::vec2(-delta.x, -delta.y);
                }
            }
            let mut max_end_beats = self.playhead_beats.max(4.0);
            for track in &self.tracks {
                for clip in &track.clips {
                    let end = clip.start_beats + clip.length_beats;
                    if end > max_end_beats {
                        max_end_beats = end;
                    }
                }
            }
            let view_width = (rect.width() - lane_label_w - 8.0).max(1.0);
            let content_width = max_end_beats * beat_width + 160.0;
            let min_pan_x = (view_width - content_width).min(0.0);
            let view_height = rect.height().max(1.0);
            let content_height = header_height + rows.len().max(1) as f32 * row_height + 8.0;
            // Allow extra vertical pan only while the piano roll panel is in use.
            let piano_roll_open = self.selected_clip.is_some();
            let piano_roll_margin = if piano_roll_open {
                self.piano_roll_panel_height
            } else {
                0.0
            };
            let min_pan_y = (view_height - content_height - piano_roll_margin).min(0.0);
            self.arranger_pan.x = self.arranger_pan.x.clamp(min_pan_x, 0.0);
            self.arranger_pan.y = self.arranger_pan.y.clamp(min_pan_y, 0.0);
            let row_top_offset = header_height + self.arranger_pan.y;
            let track_for_pos = |pos: egui::Pos2| -> Option<usize> {
                let row_index = ((pos.y - rect.top() - row_top_offset) / row_height).floor() as i32;
                if row_index < 0 {
                    return None;
                }
                rows.get(row_index as usize).and_then(|row| match *row {
                    ArrangerRow::Track { track_index } => Some(track_index),
                    ArrangerRow::Automation { track_index, .. } => Some(track_index),
                })
            };
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(8, 9, 11));
            let header_rect = egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.top()),
                egui::pos2(rect.right(), rect.top() + header_height),
            );
            let timeline_bottom = header_rect.bottom();
            let row_left = rect.left() + lane_label_w + self.arranger_pan.x;
            let header_id = egui::Id::new("arranger_timeline");
            let header_response = ui.interact(header_rect, header_id, egui::Sense::click());
            let header_pos = header_response.interact_pointer_pos();
            let header_clicked = header_response.clicked();
            let menu_color = self
                .selected_track
                .map(|index| self.track_color(index))
                .unwrap_or_else(|| egui::Color32::from_gray(200));
            if header_clicked {
                if let Some(pos) = header_pos {
                    let beats = self.beats_from_pos(pos.x, row_left, beat_width);
                    self.seek_playhead(beats);
                }
            }
            header_response.context_menu(|ui| {
                let Some(pos) = header_pos else {
                    ui.label("No cursor position");
                    return;
                };
                let beats = self.beats_from_pos(pos.x, row_left, beat_width);
                if ui
                    .add(egui::Button::image_and_text(
                        egui::Image::new(egui::include_image!("../../icons/flag.svg"))
                            .fit_to_exact_size(egui::vec2(12.0, 12.0))
                            .tint(menu_color),
                        egui::RichText::new("Set Loop Start").color(menu_color),
                    ))
                    .clicked()
                {
                    self.loop_start_beats = Some(beats);
                    if let Some(end) = self.loop_end_beats {
                        if end < beats {
                            self.loop_end_beats = Some(beats);
                        }
                    }
                    ui.close_menu();
                }
                if ui
                    .add(egui::Button::image_and_text(
                        egui::Image::new(egui::include_image!("../../icons/flag.svg"))
                            .fit_to_exact_size(egui::vec2(12.0, 12.0))
                            .tint(menu_color),
                        egui::RichText::new("Set Loop End").color(menu_color),
                    ))
                    .clicked()
                {
                    self.loop_end_beats = Some(beats);
                    if let Some(start) = self.loop_start_beats {
                        if beats < start {
                            self.loop_start_beats = Some(beats);
                            self.loop_end_beats = Some(start);
                        }
                    }
                    ui.close_menu();
                }
                if ui
                    .add(egui::Button::image_and_text(
                        egui::Image::new(egui::include_image!("../../icons/move.svg"))
                            .fit_to_exact_size(egui::vec2(12.0, 12.0))
                            .tint(menu_color),
                        egui::RichText::new("Move Loop Point Here").color(menu_color),
                    ))
                    .clicked()
                {
                    let beats = beats.max(0.0);
                    match (self.loop_start_beats, self.loop_end_beats) {
                        (Some(start), Some(end)) => {
                            let dist_start = (beats - start).abs();
                            let dist_end = (beats - end).abs();
                            if dist_start <= dist_end {
                                let new_start = beats.min(end - 0.25).max(0.0);
                                self.loop_start_beats = Some(new_start);
                            } else {
                                let new_end = beats.max(start + 0.25);
                                self.loop_end_beats = Some(new_end);
                            }
                        }
                        (Some(_start), None) => {
                            self.loop_start_beats = Some(beats);
                        }
                        (None, Some(_end)) => {
                            self.loop_end_beats = Some(beats.max(0.25));
                        }
                        (None, None) => {
                            self.loop_start_beats = Some(beats);
                            self.loop_end_beats = Some((beats + 4.0).max(beats + 0.25));
                        }
                    }
                    ui.close_menu();
                }
                if ui
                    .add(egui::Button::image_and_text(
                        egui::Image::new(egui::include_image!("../../icons/x.svg"))
                            .fit_to_exact_size(egui::vec2(12.0, 12.0))
                            .tint(menu_color),
                        egui::RichText::new("Clear Loop").color(menu_color),
                    ))
                    .clicked()
                {
                    self.loop_start_beats = None;
                    self.loop_end_beats = None;
                    ui.close_menu();
                }
            });
            let playhead_x = row_left + self.playhead_beats * beat_width;
            let grid_top = (rect.top() + row_top_offset).max(header_rect.bottom());
            let grid_bottom = rect.bottom() - 8.0;
            let grid_left = rect.left() + lane_label_w;
            let grid_right = rect.right() - 8.0;
            let grid_clip = egui::Rect::from_min_max(
                egui::pos2(grid_left, grid_top),
                egui::pos2(grid_right, grid_bottom),
            );
            let grid_painter = painter.with_clip_rect(grid_clip);
            let clip_painter = painter.with_clip_rect(grid_clip);
            let shelf_clip = egui::Rect::from_min_max(
                egui::pos2(rect.left(), grid_top),
                egui::pos2(grid_left, grid_bottom),
            );
            let shelf_painter = painter.with_clip_rect(shelf_clip);
            let mut beat_index = 0;
            let mut x = row_left;
            while x <= grid_right {
                let major = beat_index % 4 == 0;
                let line_x = x.round() + 0.5;
                let line_width = if major { 2.0 } else { 1.0 };
                let color = if major {
                    egui::Color32::from_rgba_premultiplied(20, 22, 26, 110)
                } else {
                    egui::Color32::from_rgba_premultiplied(14, 16, 20, 90)
                };
                grid_painter.line_segment(
                    [egui::pos2(line_x, grid_top), egui::pos2(line_x, grid_bottom)],
                    egui::Stroke::new(line_width, color),
                );
                if major {
                    let band_rect = egui::Rect::from_min_max(
                        egui::pos2(x, grid_top),
                        egui::pos2(x + beat_width * 4.0, grid_bottom),
                    );
                    let band_color = if (beat_index / 4) % 2 == 0 {
                        egui::Color32::from_rgba_premultiplied(0, 0, 0, 0)
                    } else {
                        egui::Color32::from_rgba_premultiplied(4, 6, 8, 120)
                    };
                    grid_painter.rect_filled(band_rect, 0.0, band_color);
                }
                beat_index += 1;
                x += beat_width;
            }
            if let (Some(start), Some(end)) = (self.loop_start_beats, self.loop_end_beats) {
                if end > start {
                    let loop_x1 = row_left + start * beat_width;
                    let loop_x2 = row_left + end * beat_width;
                    let loop_rect = egui::Rect::from_min_max(
                        egui::pos2(loop_x1, grid_top),
                        egui::pos2(loop_x2, grid_bottom),
                    );
                    grid_painter.rect_filled(
                        loop_rect,
                        0.0,
                        egui::Color32::from_rgba_premultiplied(90, 120, 220, 36),
                    );
                    grid_painter.line_segment(
                        [egui::pos2(loop_x1, grid_top), egui::pos2(loop_x1, grid_bottom)],
                        egui::Stroke::new(1.2, egui::Color32::from_rgb(140, 180, 255)),
                    );
                    grid_painter.line_segment(
                        [egui::pos2(loop_x2, grid_top), egui::pos2(loop_x2, grid_bottom)],
                        egui::Stroke::new(1.2, egui::Color32::from_rgb(140, 180, 255)),
                    );
                }
            }

            let shelf_rect = egui::Rect::from_min_max(
                egui::pos2(rect.left(), grid_top),
                egui::pos2(rect.left() + lane_label_w, grid_bottom),
            );
            shelf_painter.rect_filled(shelf_rect, 0.0, egui::Color32::from_rgb(0, 0, 0));
            let timeline_clip = egui::Rect::from_min_max(
                egui::pos2(row_left, header_rect.top()),
                egui::pos2(header_rect.right(), header_rect.bottom()),
            );
            let header_painter = painter.with_clip_rect(timeline_clip);

            let dropped_files = ctx.input(|i| i.raw.dropped_files.clone());
            if !dropped_files.is_empty() {
                let pointer = response
                    .hover_pos()
                    .or_else(|| ctx.input(|i| i.pointer.latest_pos()))
                    .or_else(|| ctx.input(|i| i.pointer.hover_pos()));
                let mut target_track = self.selected_track.unwrap_or(0).min(self.tracks.len().saturating_sub(1));
                let mut start_beats = self.playhead_beats.max(0.0);
                if let Some(pos) = pointer {
                    if rect.contains(pos) {
                        if let Some(track_index) = track_for_pos(pos) {
                            target_track = track_index;
                        }
                        start_beats = ((pos.x - row_left) / beat_width).max(0.0);
                    }
                }
                self.push_undo_state();
                for (index, file) in dropped_files.iter().enumerate() {
                    let Some(path) = file.path.as_ref() else {
                        continue;
                    };
                    let offset = index as f32 * 0.5;
                    match self.add_audio_clip_from_path(target_track, start_beats + offset, path) {
                        Ok(()) => {
                            self.status = format!("Added clip: {}", path.to_string_lossy());
                        }
                        Err(err) => {
                            self.status = format!("Drop import failed: {err}");
                        }
                    }
                }
            }

            let mut pending_select: Option<(usize, usize)> = None;
            let mut pending_delete: Option<usize> = None;
            let mut pending_drag_start: Option<ClipDragState> = None;
            let mut pending_track_select: Option<usize> = None;
            let mut over_clip = false;
            let mut switch_to_move = false;

            let mut pending_lane_edit: Vec<(usize, usize, f32, f32)> = Vec::new();
            for (row_index, row) in rows.iter().enumerate() {
                let y = rect.top() + row_top_offset + row_index as f32 * row_height;
                let label_rect = egui::Rect::from_min_max(
                    egui::pos2(rect.left(), y),
                    egui::pos2(rect.left() + lane_label_w, y + row_height),
                );
                let row_rect = egui::Rect::from_min_max(
                    egui::pos2(label_rect.right(), y),
                    egui::pos2(rect.right() - 8.0, y + row_height),
                );
                let row_click_rect = egui::Rect::from_min_max(
                    egui::pos2(rect.left(), y),
                    egui::pos2(rect.right() - 8.0, y + row_height),
                );
                let row_click_top = row_click_rect.top().max(timeline_bottom);
                let row_click_rect = egui::Rect::from_min_max(
                    egui::pos2(row_click_rect.left(), row_click_top),
                    row_click_rect.max,
                );
                if row_click_rect.height() <= 0.0 {
                    continue;
                }
                match *row {
                    ArrangerRow::Track { track_index } => {
                        let track = &self.tracks[track_index];
                        clip_painter.rect_filled(row_rect, 0.0, egui::Color32::from_rgb(0, 0, 0));
                        let row_id = egui::Id::new(format!("arranger_track_row_{}", track_index));
                        let row_response = ui.interact(row_click_rect, row_id, egui::Sense::click());
                        if row_response.clicked() {
                            pending_track_select = Some(track_index);
                        }
                        clip_painter.rect_stroke(
                            row_rect,
                            0.0,
                            egui::Stroke::new(1.0, egui::Color32::from_rgb(0, 0, 0)),
                        );
                        for clip in &track.clips {
                            let clip_x = row_left + clip.start_beats * beat_width;
                            let clip_w = (clip.length_beats * beat_width).max(1.0);
                            let clip_left = clip_x.max(row_rect.left());
                            let clip_right = (clip_x + clip_w).min(row_rect.right());
                            if clip_right <= clip_left {
                                continue;
                            }
                            let clip_rect = egui::Rect::from_min_max(
                                egui::pos2(clip_left, row_rect.top()),
                                egui::pos2(clip_right, row_rect.bottom()),
                            );
                            let clip_interact_top = clip_rect.top().max(timeline_bottom);
                            if clip_interact_top >= clip_rect.bottom() {
                                continue;
                            }
                            let clip_interact_rect = egui::Rect::from_min_max(
                                egui::pos2(clip_rect.left(), clip_interact_top),
                                clip_rect.max,
                            );
                            let selected = self.selected_clip == Some(clip.id);
                            let base = self.track_color(track_index);
                            let header_h = 14.0;
                            let header_rect = egui::Rect::from_min_size(
                                clip_rect.min,
                                egui::vec2(clip_rect.width(), header_h),
                            );
                            let body_rect = egui::Rect::from_min_max(
                                egui::pos2(clip_rect.left(), clip_rect.top() + header_h),
                                clip_rect.max,
                            );
                            let header_alpha = if selected { 220 } else { 170 };
                            let header_color = egui::Color32::from_rgba_premultiplied(
                                base.r(),
                                base.g(),
                                base.b(),
                                header_alpha,
                            );
                            let body_color = egui::Color32::from_rgba_premultiplied(
                                base.r(),
                                base.g(),
                                base.b(),
                                70,
                            );
                            clip_painter.rect_filled(body_rect, 0.0, body_color);
                            clip_painter.rect_filled(header_rect, 0.0, header_color);
                            let block_beats = 4.0;
                            let clip_start = clip.start_beats.max(0.0);
                            let clip_end = (clip.start_beats + clip.length_beats).max(clip_start);
                            let mut block_start = (clip_start / block_beats).floor() * block_beats;
                            while block_start < clip_end {
                                let block_end = block_start + block_beats;
                                let seg_start = clip_start.max(block_start);
                                let seg_end = clip_end.min(block_end);
                                if seg_end > seg_start {
                                    let x1 = row_left + seg_start * beat_width;
                                    let x2 = row_left + seg_end * beat_width;
                                    let overlay_rect = egui::Rect::from_min_max(
                                        egui::pos2(x1, clip_rect.top()),
                                        egui::pos2(x2, clip_rect.bottom()),
                                    );
                                    let block_index = (block_start / block_beats) as i32;
                                    let overlay = if block_index % 2 == 0 {
                                        egui::Color32::from_rgba_premultiplied(0, 0, 0, 0)
                                    } else {
                                        egui::Color32::from_rgba_premultiplied(0, 0, 0, 28)
                                    };
                                    clip_painter.rect_filled(overlay_rect, 0.0, overlay);
                                }
                                block_start = block_end;
                            }
                            clip_painter.rect_stroke(
                                clip_rect,
                                0.0,
                                egui::Stroke::new(1.0, Self::tint(base, 0.7)),
                            );
                            let name = if clip.name.trim().is_empty() {
                                if clip.is_midi { "MIDI" } else { "Audio" }
                            } else {
                                clip.name.as_str()
                            };
                            let header_text = ui.fonts(|f| {
                                let font = egui::FontId::proportional(11.0);
                                let max_width = (header_rect.width() - 10.0).max(4.0);
                                let mut text = name.to_string();
                                while text.len() > 1
                                    && f
                                        .layout_no_wrap(text.clone(), font.clone(), egui::Color32::WHITE)
                                        .size()
                                        .x
                                        > max_width
                                {
                                    text.pop();
                                }
                                if text.len() < name.len() {
                                    if text.len() > 3 {
                                        text.truncate(text.len().saturating_sub(3));
                                    }
                                    text.push_str("...");
                                }
                                text
                            });
                            Self::outlined_text(
                                &clip_painter,
                                egui::pos2(header_rect.left() + 4.0, header_rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                &header_text,
                                egui::FontId::proportional(11.0),
                                egui::Color32::WHITE,
                            );
                            if clip.is_midi {
                                let preview_rect = body_rect.shrink2(egui::vec2(6.0, 6.0));
                                self.draw_midi_preview(
                                    &clip_painter,
                                    preview_rect,
                                    &track.midi_notes,
                                    clip.start_beats,
                                    clip.length_beats,
                                    clip_x,
                                    beat_width,
                                );
                            } else {
                                let preview_rect = body_rect.shrink2(egui::vec2(6.0, 8.0));
                                let waveform = self.get_waveform_for_clip(clip);
                                let waveform_color = self.get_waveform_color_for_clip(clip);
                                self.draw_audio_preview(
                                    &clip_painter,
                                    preview_rect,
                                    clip.id,
                                    waveform.as_deref(),
                                    waveform_color.as_deref(),
                                    clip,
                                    Some((row_left, beat_width)),
                                );
                            }

                            let handle_w = 8.0;
                            let trim_h = 6.0;
                            let header_left = egui::Rect::from_min_size(
                                egui::pos2(header_rect.left(), header_rect.top()),
                                egui::vec2(handle_w, header_rect.height()),
                            );
                            let header_right = egui::Rect::from_min_size(
                                egui::pos2(header_rect.right() - handle_w, header_rect.top()),
                                egui::vec2(handle_w, header_rect.height()),
                            );
                            let trim_left = egui::Rect::from_min_size(
                                egui::pos2(body_rect.left(), clip_rect.bottom() - trim_h),
                                egui::vec2(handle_w, trim_h),
                            );
                            let trim_right = egui::Rect::from_min_size(
                                egui::pos2(body_rect.right() - handle_w, clip_rect.bottom() - trim_h),
                                egui::vec2(handle_w, trim_h),
                            );
                            clip_painter.rect_filled(
                                trim_left,
                                0.0,
                                egui::Color32::from_rgba_premultiplied(0, 0, 0, 80),
                            );
                            clip_painter.rect_filled(
                                trim_right,
                                0.0,
                                egui::Color32::from_rgba_premultiplied(0, 0, 0, 80),
                            );

                            let header_left_id = egui::Id::new(format!("clip_header_left_{}", clip.id));
                            let header_right_id = egui::Id::new(format!("clip_header_right_{}", clip.id));
                            let trim_left_id = egui::Id::new(format!("clip_trim_left_{}", clip.id));
                            let trim_right_id = egui::Id::new(format!("clip_trim_right_{}", clip.id));
                            let header_visible = header_rect.top() >= timeline_bottom;
                            let header_left_resp = header_visible.then(|| {
                                ui.interact(header_left, header_left_id, egui::Sense::click_and_drag())
                            });
                            let header_right_resp = header_visible.then(|| {
                                ui.interact(header_right, header_right_id, egui::Sense::click_and_drag())
                            });
                            let trim_left_resp = header_visible.then(|| {
                                ui.interact(trim_left, trim_left_id, egui::Sense::click_and_drag())
                            });
                            let trim_right_resp = header_visible.then(|| {
                                ui.interact(trim_right, trim_right_id, egui::Sense::click_and_drag())
                            });

                            let clip_id = egui::Id::new(format!("clip_{}", clip.id));
                            let clip_response =
                                ui.interact(clip_interact_rect, clip_id, egui::Sense::click_and_drag());
                            if clip_response.hovered() {
                                over_clip = true;
                            }
                            if clip_response.double_clicked() {
                                pending_select = Some((clip.id, track_index));
                                self.main_tab = MainTab::PianoRoll;
                            }
                            let clip_header_clicked = header_left_resp
                                .as_ref()
                                .map_or(false, |resp| resp.clicked())
                                || header_right_resp
                                    .as_ref()
                                    .map_or(false, |resp| resp.clicked());
                            if !header_clicked
                                && self.arranger_tool != ArrangerTool::Draw
                                && (clip_response.clicked() || clip_header_clicked)
                            {
                                pending_select = Some((clip.id, track_index));
                                if self.arranger_tool == ArrangerTool::Select {
                                    switch_to_move = true;
                                }
                            }

                            let mut start_drag = |kind: ClipDragKind, pos: Option<egui::Pos2>| {
                                if let Some(pos) = pos {
                                    let offset_beats = (pos.x - row_left) / beat_width - clip.start_beats;
                                    pending_drag_start = Some(ClipDragState {
                                        clip_id: clip.id,
                                        source_track: track_index,
                                        offset_beats,
                                        start_beats: clip.start_beats,
                                        length_beats: clip.length_beats,
                                        audio_offset_beats: clip.audio_offset_beats,
                                        audio_source_beats: clip.audio_source_beats,
                                        kind,
                                        undo_pushed: false,
                                    });
                                }
                            };

                            if self.arranger_tool == ArrangerTool::Move {
                                if let Some(resp) = header_left_resp.as_ref() {
                                    if resp.drag_started() {
                                        start_drag(ClipDragKind::ResizeStart, resp.interact_pointer_pos());
                                    }
                                }
                                if let Some(resp) = header_right_resp.as_ref() {
                                    if resp.drag_started() {
                                        start_drag(ClipDragKind::ResizeEnd, resp.interact_pointer_pos());
                                    }
                                }
                                if let Some(resp) = trim_left_resp.as_ref() {
                                    if resp.drag_started() {
                                        start_drag(ClipDragKind::TrimStart, resp.interact_pointer_pos());
                                    }
                                }
                                if let Some(resp) = trim_right_resp.as_ref() {
                                    if resp.drag_started() {
                                        start_drag(ClipDragKind::TrimEnd, resp.interact_pointer_pos());
                                    }
                                }
                                if clip_response.drag_started() {
                                    start_drag(ClipDragKind::Move, clip_response.interact_pointer_pos());
                                }
                            }

                            clip_response.context_menu(|ui| {
                                if ui
                                    .add(egui::Button::image_and_text(
                                        egui::Image::new(egui::include_image!(
                                            "../../icons/trash-2.svg"
                                        ))
                                        .fit_to_exact_size(egui::vec2(12.0, 12.0))
                                        .tint(base),
                                        egui::RichText::new("Delete Clip").color(base),
                                    ))
                                    .clicked()
                                {
                                    pending_delete = Some(clip.id);
                                    ui.close_menu();
                                }
                            });
                        }
                    }
                    ArrangerRow::Automation { track_index, lane_index } => {
                        let track = &self.tracks[track_index];
                        let Some(lane) = track.automation_lanes.get(lane_index) else {
                            continue;
                        };
                        let is_active = self.automation_active == Some((track_index, lane_index));
                        let row_color = if is_active {
                            egui::Color32::from_rgb(10, 12, 18)
                        } else {
                            egui::Color32::from_rgb(6, 8, 12)
                        };
                        clip_painter.rect_filled(row_rect, 0.0, row_color);
                        clip_painter.rect_stroke(
                            row_rect,
                            0.0,
                            egui::Stroke::new(1.0, egui::Color32::from_rgb(0, 0, 0)),
                        );
                        let lane_id = egui::Id::new(format!("automation_lane_row_{}_{}", track_index, lane.param_id));
                        let lane_resp = ui.interact(row_click_rect, lane_id, egui::Sense::click_and_drag());
                        let mut queue_lane_edit = |pos: egui::Pos2| {
                            let beat = ((pos.x - row_left) / beat_width).max(0.0);
                            let value = (1.0 - (pos.y - row_rect.top()) / row_rect.height())
                                .clamp(0.0, 1.0);
                            pending_lane_edit.push((track_index, lane_index, beat, value));
                        };
                        if lane_resp.clicked() {
                            self.automation_active = Some((track_index, lane_index));
                            if let Some(pos) = lane_resp.interact_pointer_pos() {
                                queue_lane_edit(pos);
                            }
                        }
                        if lane_resp.dragged() {
                            self.automation_active = Some((track_index, lane_index));
                            if let Some(pos) = lane_resp.interact_pointer_pos() {
                                queue_lane_edit(pos);
                            }
                        }
                        if !lane.points.is_empty() {
                            let mut points = Vec::new();
                            for point in &lane.points {
                                let x = row_left + point.beat * beat_width;
                                if x < row_rect.left() - 2.0 || x > row_rect.right() + 2.0 {
                                    continue;
                                }
                                let y = row_rect.bottom() - point.value * row_rect.height();
                                points.push(egui::pos2(x, y));
                            }
                            if points.len() >= 2 {
                                clip_painter.add(egui::Shape::line(
                                    points,
                                    egui::Stroke::new(1.2, egui::Color32::from_rgb(180, 200, 255)),
                                ));
                            } else if points.len() == 1 {
                                clip_painter.circle_filled(
                                    points[0],
                                    2.5,
                                    egui::Color32::from_rgb(200, 220, 255),
                                );
                            }
                        }
                        Self::outlined_text(
                            &shelf_painter,
                            egui::pos2(label_rect.left() + 18.0, label_rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            &format!("• {}", lane.name),
                            egui::FontId::proportional(11.0),
                            egui::Color32::from_rgb(140, 160, 200),
                        );
                    }
                }
            }

            let mut marquee_rect: Option<egui::Rect> = None;
            let mut draw_rect: Option<egui::Rect> = None;
            if let Some(pos) = response.interact_pointer_pos() {
                let in_grid = grid_clip.contains(pos);
                if self.arranger_tool == ArrangerTool::Select && in_grid && !over_clip {
                    if response.drag_started() {
                        self.arranger_select_start = Some(pos);
                    }
                }
                if self.arranger_tool == ArrangerTool::Draw && in_grid && !over_clip {
                    if response.drag_started() {
                        let target_track = track_for_pos(pos)
                            .unwrap_or(0)
                            .min(self.tracks.len().saturating_sub(1));
                        let start_beats = ((pos.x - row_left) / beat_width).max(0.0);
                        self.arranger_draw = Some(ArrangerDrawState {
                            track_index: target_track,
                            start_beats,
                            start_pos: pos,
                        });
                    }
                }
            }

            if let Some(start) = self.arranger_select_start {
                if response.dragged() {
                    if let Some(pos) = response.interact_pointer_pos() {
                        marquee_rect = Some(egui::Rect::from_two_pos(start, pos));
                    }
                }
                if response.drag_stopped() {
                    if let Some(end) = response.interact_pointer_pos() {
                        let select_rect = egui::Rect::from_two_pos(start, end);
                        let mut hit: Option<(usize, usize)> = None;
                        for (track_index, track) in self.tracks.iter().enumerate() {
                            let row_index = track_row_indices.get(track_index).copied().unwrap_or(track_index);
                            let y = rect.top() + row_top_offset + row_index as f32 * row_height;
                            let row_rect = egui::Rect::from_min_max(
                                egui::pos2(rect.left() + lane_label_w + 16.0, y),
                                egui::pos2(rect.right() - 8.0, y + row_height),
                            );
                            for clip in &track.clips {
                                let clip_x = row_left + clip.start_beats * beat_width;
                                let clip_w = (clip.length_beats * beat_width).max(1.0);
                                let clip_left = clip_x.max(row_rect.left());
                                let clip_right = (clip_x + clip_w).min(row_rect.right());
                                if clip_right <= clip_left {
                                    continue;
                                }
                                let clip_rect = egui::Rect::from_min_max(
                                    egui::pos2(clip_left, row_rect.top()),
                                    egui::pos2(clip_right, row_rect.bottom()),
                                );
                                if select_rect.intersects(clip_rect) {
                                    hit = Some((clip.id, track_index));
                                    break;
                                }
                            }
                            if hit.is_some() {
                                break;
                            }
                        }
                        if let Some((clip_id, track_index)) = hit {
                            pending_select = Some((clip_id, track_index));
                            switch_to_move = true;
                        }
                    }
                    self.arranger_select_start = None;
                }
            }

            if response.dragged() {
                if let Some(draw) = self.arranger_draw.as_ref() {
                    if let Some(pos) = response.interact_pointer_pos() {
                        let end_beats = ((pos.x - row_left) / beat_width).max(0.0);
                        let start_beats = draw.start_beats;
                        let snap = self.piano_snap.max(0.25);
                        let snapped_start = (start_beats / snap).round() * snap;
                        let snapped_end = (end_beats / snap).round() * snap;
                        let left = row_left + snapped_start.min(snapped_end) * beat_width;
                        let right = row_left + snapped_start.max(snapped_end) * beat_width;
                        let row_index = track_row_indices.get(draw.track_index).copied().unwrap_or(draw.track_index);
                        let y = rect.top() + row_top_offset + row_index as f32 * row_height;
                        draw_rect = Some(egui::Rect::from_min_max(
                            egui::pos2(left, y),
                            egui::pos2(right, y + row_height),
                        ));
                    }
                }
            }
            if response.drag_stopped() {
                if let Some(draw) = self.arranger_draw.take() {
                    if let Some(pos) = response.interact_pointer_pos() {
                        let end_beats = ((pos.x - row_left) / beat_width).max(0.0);
                        let snap = self.piano_snap.max(0.25);
                        let mut start = (draw.start_beats / snap).round() * snap;
                        let mut end = (end_beats / snap).round() * snap;
                        if (end - start).abs() < snap {
                            end = start + snap;
                        }
                        if end < start {
                            std::mem::swap(&mut start, &mut end);
                        }
                        let track_index = draw.track_index;
                        let clip_id = self.next_clip_id();
                        self.push_undo_state();
                        if let Some(track) = self.tracks.get_mut(track_index) {
                            track.clips.push(Clip {
                                id: clip_id,
                                track: track_index,
                                start_beats: start,
                                length_beats: (end - start).max(snap),
                                is_midi: true,
                                name: "MIDI Clip".to_string(),
                                audio_path: None,
                                audio_source_beats: None,
                                audio_offset_beats: 0.0,
                                audio_gain: 1.0,
                                audio_pitch_semitones: 0.0,
                                audio_time_mul: 1.0,
                            });
                            self.selected_track = Some(track_index);
                            self.selected_clip = Some(clip_id);
                        }
                    }
                }
            }

            if let Some(rect) = marquee_rect {
                painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.2, egui::Color32::from_rgb(120, 170, 255)));
                painter.rect_filled(rect, 0.0, egui::Color32::from_rgba_premultiplied(80, 120, 200, 40));
            }
            if let Some(rect) = draw_rect {
                painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.2, egui::Color32::from_rgb(120, 220, 160)));
                painter.rect_filled(rect, 0.0, egui::Color32::from_rgba_premultiplied(60, 140, 90, 40));
            }

            if playhead_x >= row_left && playhead_x <= rect.right() - 8.0 {
                painter.line_segment(
                    [
                        egui::pos2(playhead_x, rect.top() + 4.0),
                        egui::pos2(playhead_x, rect.bottom() - 4.0),
                    ],
                    egui::Stroke::new(1.4, egui::Color32::from_rgb(255, 86, 70)),
                );
            }

            painter.rect_filled(header_rect, 0.0, egui::Color32::from_rgb(0, 0, 0));
            painter.line_segment(
                [
                    egui::pos2(header_rect.left(), header_rect.bottom()),
                    egui::pos2(header_rect.right(), header_rect.bottom()),
                ],
                egui::Stroke::new(1.0, egui::Color32::from_rgb(28, 30, 34)),
            );
            let mut bar_index = 0;
            let mut bar_x = row_left;
            while bar_x <= rect.right() - 8.0 {
                if bar_index % 4 == 0 {
                    let bar = bar_index / 4 + 1;
                    Self::outlined_text(
                        &header_painter,
                        egui::pos2(bar_x + 4.0, header_rect.top() + 2.0),
                        egui::Align2::LEFT_TOP,
                        &format!("{bar}"),
                        egui::FontId::proportional(10.0),
                        egui::Color32::from_gray(160),
                    );
                }
                bar_index += 1;
                bar_x += beat_width;
            }

            // Overlay timeline bar and grid/loop/playhead lines above clips.
            let timeline_overlay_rect = header_rect;
            painter.rect_filled(timeline_overlay_rect, 0.0, egui::Color32::from_rgb(0, 0, 0));
            let overlay_painter = painter.with_clip_rect(timeline_clip);
            let mut overlay_x = row_left;
            let mut overlay_index = 0;
            while overlay_x <= rect.right() - 8.0 {
                let major = overlay_index % 4 == 0;
                let line_x = overlay_x.round() + 0.5;
                let line_width = if major { 2.0 } else { 1.0 };
                let color = if major {
                    egui::Color32::from_rgba_premultiplied(48, 52, 60, 170)
                } else {
                    egui::Color32::from_rgba_premultiplied(32, 36, 44, 140)
                };
                if major {
                    let bar_rect = egui::Rect::from_min_max(
                        egui::pos2(overlay_x, timeline_overlay_rect.top()),
                        egui::pos2(
                            (overlay_x + beat_width * 4.0).min(timeline_overlay_rect.right()),
                            timeline_overlay_rect.bottom(),
                        ),
                    );
                    let shade = if (overlay_index / 4) % 2 == 0 {
                        egui::Color32::from_rgb(8, 8, 8)
                    } else {
                        egui::Color32::from_rgb(0, 0, 0)
                    };
                    overlay_painter.rect_filled(bar_rect, 0.0, shade);
                }
                grid_painter.line_segment(
                    [egui::pos2(line_x, grid_top), egui::pos2(line_x, grid_bottom)],
                    egui::Stroke::new(line_width, color),
                );
                if major {
                    let bar = overlay_index / 4 + 1;
                    Self::outlined_text(
                        &overlay_painter,
                        egui::pos2(overlay_x + 4.0, timeline_overlay_rect.top() + 2.0),
                        egui::Align2::LEFT_TOP,
                        &format!("{bar}"),
                        egui::FontId::proportional(12.0),
                        egui::Color32::from_gray(200),
                    );
                }
                overlay_index += 1;
                overlay_x += beat_width;
            }
            painter.line_segment(
                [
                    egui::pos2(timeline_overlay_rect.left(), timeline_overlay_rect.bottom()),
                    egui::pos2(timeline_overlay_rect.right(), timeline_overlay_rect.bottom()),
                ],
                egui::Stroke::new(1.2, egui::Color32::from_rgb(28, 30, 34)),
            );
            if let (Some(start), Some(end)) = (self.loop_start_beats, self.loop_end_beats) {
                if end > start {
                    let loop_x1 = row_left + start * beat_width;
                    let loop_x2 = row_left + end * beat_width;
                    painter.line_segment(
                        [egui::pos2(loop_x1, grid_top), egui::pos2(loop_x1, grid_bottom)],
                        egui::Stroke::new(1.4, egui::Color32::from_rgb(150, 190, 255)),
                    );
                    painter.line_segment(
                        [egui::pos2(loop_x2, grid_top), egui::pos2(loop_x2, grid_bottom)],
                        egui::Stroke::new(1.4, egui::Color32::from_rgb(150, 190, 255)),
                    );
                }
            }
            if playhead_x >= row_left && playhead_x <= rect.right() - 8.0 {
                painter.line_segment(
                    [
                        egui::pos2(playhead_x, rect.top() + 4.0),
                        egui::pos2(playhead_x, rect.bottom() - 4.0),
                    ],
                    egui::Stroke::new(1.6, egui::Color32::from_rgb(255, 96, 80)),
                );
            }

            for (track_index, track) in self.tracks.iter().enumerate() {
                let row_index = track_row_indices.get(track_index).copied().unwrap_or(track_index);
                let y = rect.top() + row_top_offset + row_index as f32 * row_height;
                let label_rect = egui::Rect::from_min_max(
                    egui::pos2(rect.left() + 8.0, y),
                    egui::pos2(rect.left() + lane_label_w, y + row_height),
                );
                let tile_rect = label_rect;
                let is_selected = self.selected_track == Some(track_index);
                let base = self.track_color(track_index);
                let has_automation = !track.automation_lanes.is_empty();
                let tile_color = if is_selected {
                    Self::tint(base, 0.35)
                } else {
                    egui::Color32::from_rgba_premultiplied(base.r(), base.g(), base.b(), 90)
                };
                shelf_painter.rect_filled(tile_rect, 0.0, tile_color);
                let expanded = self.automation_rows_expanded.contains(&track_index);
                let mut toggle_response = None;
                let mut toggle_rect_opt: Option<egui::Rect> = None;
                if has_automation {
                    let toggle_rect = egui::Rect::from_min_size(
                        egui::pos2(tile_rect.left() + 4.0, tile_rect.center().y - 6.0),
                        egui::vec2(12.0, 12.0),
                    );
                    toggle_rect_opt = Some(toggle_rect);
                    let toggle_icon = if expanded {
                        egui::include_image!("../../icons/chevron-down.svg")
                    } else {
                        egui::include_image!("../../icons/chevron-right.svg")
                    };
                    let response = ui.put(
                        toggle_rect,
                        egui::ImageButton::new(
                            egui::Image::new(toggle_icon).fit_to_exact_size(toggle_rect.size()),
                        )
                        .frame(false),
                    );
                    if response.clicked() {
                        if expanded {
                            self.automation_rows_expanded.remove(&track_index);
                        } else {
                            self.automation_rows_expanded.insert(track_index);
                        }
                    }
                    toggle_response = Some(response);
                }
                if let (Some(rect), Some(resp)) = (toggle_rect_opt, toggle_response.as_ref()) {
                    if resp.hovered() {
                        shelf_painter.rect_filled(
                            rect,
                            2.0,
                            egui::Color32::from_rgba_premultiplied(255, 255, 255, 30),
                        );
                    }
                }
                let label_click_rect = if has_automation {
                    egui::Rect::from_min_max(
                        egui::pos2(tile_rect.left() + 20.0, tile_rect.top()),
                        tile_rect.max,
                    )
                } else {
                    tile_rect
                };
                if label_click_rect.top() >= grid_top {
                    let label_id = egui::Id::new(format!("arranger_tracklist_{}", track_index));
                    let label_response =
                        ui.interact(label_click_rect, label_id, egui::Sense::click());
                    if label_response.clicked()
                        && !toggle_response.as_ref().map_or(false, |resp| resp.clicked())
                    {
                        pending_track_select = Some(track_index);
                    }
                }
                let name_rect = egui::Rect::from_min_max(
                    egui::pos2(
                        tile_rect.left() + if has_automation { 22.0 } else { 6.0 },
                        tile_rect.top(),
                    ),
                    egui::pos2(tile_rect.right() - 46.0, tile_rect.bottom()),
                );
                let name_color = if is_selected {
                    egui::Color32::from_rgb(220, 235, 255)
                } else {
                    egui::Color32::from_gray(220)
                };
                Self::outlined_text(
                    &shelf_painter,
                    egui::pos2(name_rect.left(), name_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    &track.name,
                    egui::FontId::proportional(12.0),
                    name_color,
                );
                let meter_rect = egui::Rect::from_center_size(
                    egui::pos2(tile_rect.right() - 24.0, tile_rect.center().y),
                    egui::vec2(36.0, 8.0),
                );
                shelf_painter.rect_filled(meter_rect, 3.0, egui::Color32::from_rgb(16, 20, 24));
                let peak = self
                    .track_audio
                    .get(track_index)
                    .map(|s| f32::from_bits(s.peak_bits.load(Ordering::Relaxed)))
                    .unwrap_or(0.0)
                    .clamp(0.0, 1.0);
                let fill_w = meter_rect.width() * peak;
                if fill_w > 0.0 {
                    let fill_rect = egui::Rect::from_min_size(
                        meter_rect.min,
                        egui::vec2(fill_w, meter_rect.height()),
                    );
                    let color = if peak > 0.9 {
                        egui::Color32::from_rgb(255, 90, 64)
                    } else if peak > 0.7 {
                        egui::Color32::from_rgb(250, 200, 80)
                    } else {
                        egui::Color32::from_rgb(90, 210, 120)
                    };
                    shelf_painter.rect_filled(fill_rect, 3.0, color);
                }
            }

            for (track_index, lane_index, beat, value) in pending_lane_edit {
                if let Some(track) = self.tracks.get_mut(track_index) {
                    if let Some(lane) = track.automation_lanes.get_mut(lane_index) {
                        let mut updated = false;
                        for point in lane.points.iter_mut() {
                            if (point.beat - beat).abs() <= 0.1 {
                                point.beat = beat;
                                point.value = value;
                                updated = true;
                                break;
                            }
                        }
                        if !updated {
                            lane.points.push(AutomationPoint { beat, value });
                        }
                        lane.points.sort_by(|a, b| a.beat.partial_cmp(&b.beat).unwrap());
                        if let Some(state) = self.track_audio.get(track_index) {
                            if let Ok(mut lanes) = state.automation_lanes.lock() {
                                *lanes = track.automation_lanes.clone();
                            }
                        }
                    }
                }
            }

            let has_pending_drag = pending_drag_start.is_some();
            let mut selection_changed = false;
            if let Some((clip_id, track_index)) = pending_select {
                self.selected_clip = Some(clip_id);
                self.selected_track = Some(track_index);
                selection_changed = true;
            }
            if let Some(drag) = pending_drag_start {
                let clip_id = drag.clip_id;
                let source_track = drag.source_track;
                self.clip_drag = Some(drag);
                self.selected_clip = Some(clip_id);
                self.selected_track = Some(source_track);
                selection_changed = true;
            }
            if pending_select.is_none() && !has_pending_drag {
                if let Some(track_index) = pending_track_select {
                    self.selected_track = Some(track_index);
                    selection_changed = true;
                }
            }
            if selection_changed {
                self.refresh_params_for_selected_track(false);
                self.piano_selected.clear();
            }
            if switch_to_move {
                self.arranger_tool = ArrangerTool::Move;
            }
            if let Some(clip_id) = pending_delete {
                self.push_undo_state();
                self.remove_clip_by_id(clip_id);
                if self.selected_clip == Some(clip_id) {
                    self.selected_clip = None;
                }
            }

            if let Some(mut drag) = self.clip_drag.take() {
                if response.dragged() {
                    if let Some(pos) = response.interact_pointer_pos() {
                        if !drag.undo_pushed {
                            self.push_undo_state();
                            drag.undo_pushed = true;
                        }
                        let min_len = 0.25;
                        let target_track = track_for_pos(pos)
                            .unwrap_or(0)
                            .min(self.tracks.len().saturating_sub(1));
                        let cursor_beats = (pos.x - row_left) / beat_width;

                        match drag.kind {
                            ClipDragKind::Move => {
                                let new_start = (cursor_beats - drag.offset_beats).max(0.0);
                                let delta = new_start - drag.start_beats;
                                let is_midi = self
                                    .tracks
                                    .get(drag.source_track)
                                    .and_then(|track| track.clips.iter().find(|c| c.id == drag.clip_id))
                                    .map(|clip| clip.is_midi)
                                    .unwrap_or(false);
                                if is_midi
                                    && (delta.abs() > f32::EPSILON
                                        || target_track != drag.source_track)
                                {
                                    self.shift_midi_notes_for_clip_move(
                                        drag.source_track,
                                        target_track,
                                        drag.start_beats,
                                        drag.length_beats,
                                        delta,
                                    );
                                }
                                self.move_clip_by_id(drag.clip_id, target_track, new_start);
                                drag.source_track = target_track;
                                drag.start_beats = new_start;
                            }
                            ClipDragKind::ResizeStart => {
                                let end = drag.start_beats + drag.length_beats;
                                let new_start = cursor_beats.min(end - min_len).max(0.0);
                                let new_len = (end - new_start).max(min_len);
                                self.update_clip_by_id(drag.clip_id, |clip| {
                                    clip.start_beats = new_start;
                                    clip.length_beats = new_len;
                                });
                            }
                            ClipDragKind::ResizeEnd => {
                                let new_len = (cursor_beats - drag.start_beats).max(min_len);
                                self.update_clip_by_id(drag.clip_id, |clip| {
                                    clip.length_beats = new_len;
                                });
                            }
                            ClipDragKind::TrimStart => {
                                let new_start = cursor_beats.min(drag.start_beats + drag.length_beats - min_len);
                                let delta = (new_start - drag.start_beats).max(0.0);
                                let new_len = (drag.length_beats - delta).max(min_len);
                                self.update_clip_by_id(drag.clip_id, |clip| {
                                    clip.start_beats = new_start;
                                    clip.length_beats = new_len;
                                    let mut offset = (drag.audio_offset_beats + delta).max(0.0);
                                    if let Some(source) = clip.audio_source_beats {
                                        if source > 0.0 {
                                            offset %= source;
                                        }
                                    }
                                    clip.audio_offset_beats = offset;
                                });
                            }
                            ClipDragKind::TrimEnd => {
                                let new_len = (cursor_beats - drag.start_beats).max(min_len);
                                self.update_clip_by_id(drag.clip_id, |clip| {
                                    clip.length_beats = new_len;
                                    if let Some(source) = clip.audio_source_beats {
                                        if source > 0.0 {
                                            clip.audio_offset_beats %= source;
                                        }
                                    }
                                });
                            }
                        }
                    }
                }
                if response.drag_stopped() {
                    self.clip_drag = None;
                } else {
                    self.clip_drag = Some(drag);
                }
            }
        });
    }

    fn center_empty(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.centered_and_justified(|ui| {
                ui.label("Arranger hidden");
            });
        });
    }


    fn center_parameters(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            self.render_params_roll_panel(ctx, ui, true, false);
        });
    }

    fn center_piano_roll(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            self.render_params_roll_panel(ctx, ui, false, true);
        });
    }

    fn render_params_roll_panel(
        &mut self,
        ctx: &egui::Context,
        ui: &mut egui::Ui,
        show_params: bool,
        show_roll: bool,
    ) {
        self.piano_roll_panel_height = ui.max_rect().height();
        self.piano_roll_hovered = false;
        let mut selected_clip_info = None;
        if let Some(clip_id) = self.selected_clip {
            for (track_index, track) in self.tracks.iter().enumerate() {
                if let Some(clip_index) = track.clips.iter().position(|c| c.id == clip_id) {
                    selected_clip_info = Some((track_index, clip_index));
                    break;
                }
            }
        }
        let is_audio_clip = selected_clip_info
            .and_then(|(ti, ci)| self.tracks.get(ti).and_then(|t| t.clips.get(ci)))
            .map(|c| !c.is_midi)
            .unwrap_or(false);

        if show_roll {
            ui.horizontal(|ui| {
                ui.heading(if is_audio_clip { "Audio Clip" } else { "Piano Roll" });
                if let Some(clip_id) = self.selected_clip {
                    ui.label(format!("Clip {}", clip_id));
                } else {
                    ui.label("No clip selected");
                }
            });
            if !is_audio_clip {
                ui.horizontal(|ui| {
                    ui.label("Tools");
                    let tool_size = egui::vec2(90.0, 22.0);
                    let icon_size = egui::vec2(14.0, 14.0);
                    let button_bg = egui::Color32::from_rgba_premultiplied(18, 20, 24, 220);
                    let button_on = egui::Color32::from_rgba_premultiplied(46, 94, 130, 220);
                    let icon_tint = egui::Color32::from_gray(220);
                    let mut tool_button = |tool: PianoTool, icon: egui::ImageSource<'static>, label: &str| {
                        let selected = self.piano_tool == tool;
                        let mut button = egui::Button::image_and_text(
                            egui::Image::new(icon).fit_to_exact_size(icon_size).tint(icon_tint),
                            label,
                        )
                        .min_size(tool_size)
                        .fill(if selected { button_on } else { button_bg });
                        if ui.add(button).clicked() {
                            self.piano_tool = tool;
                        }
                    };
                    tool_button(
                        PianoTool::Pencil,
                        egui::include_image!("../../icons/pen-tool.svg"),
                        "Draw",
                    );
                    tool_button(
                        PianoTool::Select,
                        egui::include_image!("../../icons/mouse-pointer.svg"),
                        "Select",
                    );
                });
            }
            ui.add_space(4.0);
        }

        if show_params && !show_roll {
            let selected_track_index = self.selected_track;
            let track_color = selected_track_index.map(|i| self.track_color(i));
            let mut pending_automation_record: Vec<(usize, RecordedAutomationPoint)> = Vec::new();
            let mut pending_lane_delete: Option<(usize, usize)> = None;
            let mut pending_active_lane: Option<(usize, usize)> = None;
            let columns_height = ui.available_height();

            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(260.0);
                    ui.set_min_height(columns_height);
                    ui.heading("Parameters");
                    ui.separator();
                    if is_audio_clip {
                        if let Some((ti, ci)) = selected_clip_info {
                            if let Some(clip) =
                                self.tracks.get_mut(ti).and_then(|t| t.clips.get_mut(ci))
                            {
                                ui.label("Clip Properties");
                                ui.add_space(6.0);
                                ui.horizontal(|ui| {
                                    ui.label("Gain");
                                    Self::colored_slider(ui, &mut clip.audio_gain, 0.0..=2.0, track_color);
                                });
                                ui.horizontal(|ui| {
                                    ui.label("Pitch");
                                    Self::colored_slider(
                                        ui,
                                        &mut clip.audio_pitch_semitones,
                                        -24.0..=24.0,
                                        track_color,
                                    );
                                });
                                ui.horizontal(|ui| {
                                    ui.label("Time Mul");
                                    Self::colored_slider(
                                        ui,
                                        &mut clip.audio_time_mul,
                                        0.25..=4.0,
                                        track_color,
                                    );
                                });
                                ui.horizontal(|ui| {
                                    ui.label("Offset");
                                    ui.add(
                                        egui::DragValue::new(&mut clip.audio_offset_beats).speed(0.1),
                                    );
                                });
                                ui.add_space(6.0);
                                if ui
                                    .add(egui::Button::image(
                                        egui::Image::new(egui::include_image!("../../icons/refresh-cw.svg"))
                                            .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                    ))
                                    .on_hover_text("Fit To Tempo")
                                    .clicked()
                                {
                                    if let Some(source) = clip.audio_source_beats {
                                        if source > 0.0 && clip.length_beats > 0.0 {
                                            clip.audio_time_mul = source / clip.length_beats;
                                        }
                                    }
                                }
                                if ui
                                    .add(egui::Button::image(
                                        egui::Image::new(egui::include_image!("../../icons/rotate-ccw.svg"))
                                            .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                    ))
                                    .on_hover_text("Reset Audio Props")
                                    .clicked()
                                {
                                    clip.audio_gain = 1.0;
                                    clip.audio_pitch_semitones = 0.0;
                                    clip.audio_time_mul = 1.0;
                                    clip.audio_offset_beats = 0.0;
                                }
                            }
                        }
                    } else {
                        let track = selected_track_index.and_then(|i| self.tracks.get(i));
                        let name = track.map(|t| t.name.as_str()).unwrap_or("None");
                        ui.label(format!("Track: {name}"));
                        let plugin = track
                            .and_then(|t| t.instrument_path.as_deref())
                            .map(Self::plugin_display_name)
                            .unwrap_or_else(|| "No instrument".to_string());
                        ui.label(format!("Plugin: {plugin}"));
                        ui.add_space(6.0);
                        ui.label("Instrument");
                        ui.horizontal(|ui| {
                            let choose = ui
                                .add(egui::Button::image(
                                    egui::Image::new(egui::include_image!("../../icons/folder-plus.svg"))
                                        .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                ))
                                .on_hover_text("Choose");
                            let open = ui
                                .add(egui::Button::image(
                                    egui::Image::new(egui::include_image!("../../icons/external-link.svg"))
                                        .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                ))
                                .on_hover_text("Open UI");
                            let clear = ui
                                .add(egui::Button::image(
                                    egui::Image::new(egui::include_image!("../../icons/x.svg"))
                                        .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                ))
                                .on_hover_text("Clear");
                            if let Some(index) = selected_track_index {
                                if choose.clicked() {
                                    self.open_plugin_picker(PluginTarget::Instrument(index));
                                }
                                if open.clicked() {
                                    self.plugin_ui_target = Some(PluginUiTarget::Instrument(index));
                                    self.show_plugin_ui = true;
                                }
                                if clear.clicked() {
                                    if self.plugin_ui_matches(PluginUiTarget::Instrument(index)) {
                                        self.show_plugin_ui = false;
                                        self.destroy_plugin_ui();
                                    }
                                    if let Some(track) = self.tracks.get_mut(index) {
                                        track.instrument_path = None;
                                        track.params = default_midi_params();
                                        track.param_ids.clear();
                                        track.param_values.clear();
                                    }
                                    if let Some(state) = self.track_audio.get_mut(index) {
                                        if let Some(host) = state.host.take() {
                                            if let Ok(mut host) = host.lock() {
                                                host.prepare_for_drop();
                                            }
                                            self.orphaned_hosts.push(host);
                                        }
                                    }
                                    self.reinit_audio_if_running();
                                }
                            }
                        });
                        ui.add_space(6.0);
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            self.ensure_live_params();
                            let host_change = if let Some(host) = self.selected_track_host() {
                                if let Ok(mut host) = host.try_lock() {
                                    host.take_last_param_change()
                                } else {
                                    None
                                }
                            } else {
                                None
                            };
                            let menu_color = selected_track_index
                                .map(|index| self.track_color(index))
                                .unwrap_or_else(|| egui::Color32::from_gray(200));
                            let mut pending_status: Option<String> = None;
                            let mut pending_midi_learn: Option<(usize, u32, String)> = None;
                            let mut pending_active_lane: Option<(usize, usize)> = None;
                            if let Some(track) = selected_track_index.and_then(|i| self.tracks.get_mut(i)) {
                                if let Some((param_id, value)) = host_change {
                                    if let Some(pos) = track.param_ids.iter().position(|id| *id == param_id) {
                                        track.param_values[pos] = value as f32;
                                        self.last_ui_param_change = Some((param_id, value as f32));
                                    }
                                }
                                if track.param_values.len() != track.params.len() {
                                    track.param_values.resize(track.params.len(), 0.0);
                                }
                                if let Some(program_index) = track
                                    .params
                                    .iter()
                                    .position(|name| {
                                        let name = name.to_ascii_lowercase();
                                        name.contains("program") || name.contains("preset")
                                    })
                                {
                                    let current = (track.param_values[program_index] * 127.0)
                                        .round()
                                        .clamp(0.0, 127.0) as u8;
                                    let mut selected = current;
                                    egui::ComboBox::from_label("Preset")
                                        .selected_text(format!(
                                            "{:03} {}",
                                            selected + 1,
                                            gm_program_name(selected)
                                        ))
                                        .show_ui(ui, |ui| {
                                            for program in 0u8..=127 {
                                                let label = format!(
                                                    "{:03} {}",
                                                    program + 1,
                                                    gm_program_name(program)
                                                );
                                                if ui
                                                    .selectable_label(program == selected, label)
                                                    .clicked()
                                                {
                                                    selected = program;
                                                }
                                            }
                                        });
                                    if selected != current {
                                        let value = (selected as f32 / 127.0).clamp(0.0, 1.0);
                                        track.param_values[program_index] = value;
                                        if let Some(param_id) = track.param_ids.get(program_index).copied() {
                                            if let Some(state) =
                                                selected_track_index.and_then(|i| self.track_audio.get(i))
                                            {
                                                if let Ok(mut pending) =
                                                    state.pending_param_changes.lock()
                                                {
                                                    pending.push(PendingParamChange {
                                                        target: PendingParamTarget::Instrument,
                                                        param_id,
                                                        value: value as f64,
                                                    });
                                                }
                                            }
                                            if self.is_recording && self.record_automation {
                                                if let Some(track_index) = selected_track_index {
                                                    pending_automation_record.push((
                                                        track_index,
                                                        RecordedAutomationPoint {
                                                            param_id,
                                                            target: AutomationTarget::Instrument,
                                                            beat: self.playhead_beats,
                                                            value,
                                                        },
                                                    ));
                                                }
                                            }
                                        }
                                    }
                                    ui.add_space(6.0);
                                }
                                for index in 0..track.params.len() {
                                    let label = track.params[index].clone();
                                    let value = &mut track.param_values[index];
                                    let slider = ui.push_id(format!("param_{}", label), |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(&label);
                                                    Self::colored_slider(ui, value, 0.0..=1.0, track_color)
                                        })
                                        .inner
                                    });
                                    let response = slider.response;
                                    let slider_response = slider.inner;
                                    let changed = slider_response.changed()
                                        || slider_response.dragged()
                                        || response.dragged();
                                    if changed {
                                        let param_id = track.param_ids.get(index).copied();
                                        let debug_id = param_id.unwrap_or(u32::MAX);
                                        self.last_ui_param_change = Some((debug_id, *value));
                                        if let Some(param_id) = param_id {
                                            if let Some(state) =
                                                selected_track_index.and_then(|i| self.track_audio.get(i))
                                            {
                                                if let Some(host) = state.host.as_ref() {
                                                    if let Ok(host) = host.try_lock() {
                                                        if let Some((channel, controller)) =
                                                            host.param_to_cc(param_id)
                                                        {
                                                            if let Ok(mut events) = state.midi_events.lock() {
                                                                let cc_value = (*value * 127.0).round() as i32;
                                                                let cc_value = cc_value.clamp(0, 127) as u8;
                                                                events.push(vst3::MidiEvent::control_change(
                                                                    channel,
                                                                    controller,
                                                                    cc_value,
                                                                ));
                                                            }
                                                        }
                                                    }
                                                }
                                                if let Ok(mut pending) =
                                                    state.pending_param_changes.lock()
                                                {
                                                    pending.push(PendingParamChange {
                                                        target: PendingParamTarget::Instrument,
                                                        param_id,
                                                        value: *value as f64,
                                                    });
                                                }
                                            }
                                            if self.is_recording && self.record_automation {
                                                if let Some(track_index) = selected_track_index {
                                                    pending_automation_record.push((
                                                        track_index,
                                                        RecordedAutomationPoint {
                                                            param_id,
                                                            target: AutomationTarget::Instrument,
                                                            beat: self.playhead_beats,
                                                            value: *value,
                                                        },
                                                    ));
                                                }
                                            }
                                        }
                                    }
                                    response.context_menu(|ui| {
                                        if ui
                                            .add(egui::Button::image_and_text(
                                                egui::Image::new(egui::include_image!(
                                                    "../../icons/target.svg"
                                                ))
                                                .fit_to_exact_size(egui::vec2(12.0, 12.0))
                                                .tint(menu_color),
                                                egui::RichText::new("MIDI Learn").color(menu_color),
                                            ))
                                            .clicked()
                                        {
                                            if let Some(param_id) = track.param_ids.get(index).copied() {
                                                if let Some(track_index) = selected_track_index {
                                                    pending_midi_learn =
                                                        Some((track_index, param_id, label.clone()));
                                                }
                                            }
                                            ui.close_menu();
                                        }
                                        if ui
                                            .add(egui::Button::image_and_text(
                                                egui::Image::new(egui::include_image!(
                                                    "../../icons/activity.svg"
                                                ))
                                                .fit_to_exact_size(egui::vec2(12.0, 12.0))
                                                .tint(menu_color),
                                                egui::RichText::new("Create Automation Lane")
                                                    .color(menu_color),
                                            ))
                                            .clicked()
                                        {
                                            if let Some(param_id) = track.param_ids.get(index).copied() {
                                                if !track
                                                    .automation_lanes
                                                    .iter()
                                                    .any(|l| l.param_id == param_id)
                                                {
                                                    track.automation_lanes.push(AutomationLane {
                                                        name: label.clone(),
                                                        param_id,
                                                        target: AutomationTarget::Instrument,
                                                        points: Vec::new(),
                                                    });
                                                }
                                                if let Some(pos) = track
                                                    .automation_lanes
                                                    .iter()
                                                    .position(|l| l.param_id == param_id)
                                                {
                                                    if let Some(track_index) = selected_track_index {
                                                        pending_active_lane = Some((track_index, pos));
                                                    }
                                                }
                                            }
                                            ui.close_menu();
                                        }
                                    });
                                }
                                if let Some((track_index, param_id, label)) = pending_midi_learn.take() {
                                    if let Ok(mut learn) = self.midi_learn.lock() {
                                        *learn = Some((track_index, param_id));
                                    }
                                    pending_status = Some(format!("MIDI Learn armed for {}", label));
                                }
                                if let Some(status) = pending_status.take() {
                                    self.status = status;
                                }
                                if let Some(active) = pending_active_lane.take() {
                                    self.automation_active = Some(active);
                                }

                                if ui
                                        .add(egui::Button::image(
                                            egui::Image::new(egui::include_image!("../../icons/shuffle.svg"))
                                                .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                        ))
                                        .on_hover_text("Randomize Params")
                                        .clicked()
                                    {
                                    let seed = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_nanos() as u64)
                                        .unwrap_or(0x1234_5678);
                                    let mut rng = seed;
                                    for idx in 0..track.param_values.len() {
                                        rng ^= rng << 13;
                                        rng ^= rng >> 7;
                                        rng ^= rng << 17;
                                        let value = (rng as f64 / u64::MAX as f64) as f32;
                                        track.param_values[idx] = value;
                                        if let Some(param_id) = track.param_ids.get(idx).copied() {
                                            if let Some(state) = selected_track_index
                                                .and_then(|i| self.track_audio.get(i))
                                            {
                                                if let Ok(mut pending) =
                                                    state.pending_param_changes.lock()
                                                {
                                                    pending.push(PendingParamChange {
                                                        target: PendingParamTarget::Instrument,
                                                        param_id,
                                                        value: value as f64,
                                                    });
                                                }
                                            }
                                        }
                                    }
                                }

                            }
                        });
                    }
                });

                ui.separator();

                ui.vertical(|ui| {
                    ui.set_width(240.0);
                    ui.set_min_height(columns_height);
                    ui.heading("Automation");
                    ui.separator();
                    let Some(track_index) = selected_track_index else {
                        ui.label("No track selected");
                        return;
                    };
                    let Some(track) = self.tracks.get(track_index) else {
                        ui.label("No track selected");
                        return;
                    };
                    if track.automation_lanes.is_empty() {
                        ui.label("No automation lanes");
                    } else {
                        for (lane_index, lane) in track.automation_lanes.iter().enumerate() {
                            let selected = self
                                .automation_active
                                .map(|(ai, li)| ai == track_index && li == lane_index)
                                .unwrap_or(false);
                            ui.horizontal(|ui| {
                                let lane_response = ui.selectable_label(selected, &lane.name);
                                if lane_response.clicked() {
                                    pending_active_lane = Some((track_index, lane_index));
                                }
                                if ui
                                    .add(egui::Button::image(
                                        egui::Image::new(egui::include_image!("../../icons/trash-2.svg"))
                                            .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                    ))
                                    .on_hover_text("Delete")
                                    .clicked()
                                {
                                    pending_lane_delete = Some((track_index, lane_index));
                                }
                            });
                        }
                    }
                });

                ui.separator();

                ui.vertical(|ui| {
                    ui.set_width(240.0);
                    ui.set_min_height(columns_height);
                    ui.heading("Routing");
                    ui.separator();
                    ui.label("-Mappings");
                    ui.label("No mappings");
                    ui.add_space(8.0);
                    ui.label("-Macros");
                    ui.label("No macros");
                });
            });

            if let Some((track_index, lane_index)) = pending_active_lane {
                self.automation_active = Some((track_index, lane_index));
            }
            for (track_index, point) in pending_automation_record {
                self.record_automation_point(
                    track_index,
                    point.target,
                    point.param_id,
                    point.beat,
                    point.value,
                );
            }
            if let Some((track_index, lane_index)) = pending_lane_delete {
                if let Some(track) = self.tracks.get_mut(track_index) {
                    if lane_index < track.automation_lanes.len() {
                        track.automation_lanes.remove(lane_index);
                    }
                }
                if let Some(state) = self.track_audio.get(track_index) {
                    if let Ok(mut lanes) = state.automation_lanes.lock() {
                        *lanes = self
                            .tracks
                            .get(track_index)
                            .map(|t| t.automation_lanes.clone())
                            .unwrap_or_default();
                    }
                }
                if let Some((active_track, active_lane)) = self.automation_active {
                    if active_track == track_index {
                        if active_lane == lane_index {
                            self.automation_active = None;
                        } else if active_lane > lane_index {
                            self.automation_active = Some((track_index, active_lane - 1));
                        }
                    }
                }
            }
            return;
        }
        if !is_audio_clip {
            let note_button_size = egui::vec2(20.0, 20.0);
            let note_icon_tint = egui::Color32::from_gray(220);
            let note_button_bg = egui::Color32::from_rgba_premultiplied(18, 20, 24, 200);
            let note_button_on = egui::Color32::from_rgba_premultiplied(46, 94, 130, 220);
            ui.horizontal(|ui| {
                ui.label("Note Length");
                let lengths = [
                    (1.0 / 32.0, "1/32"),
                    (1.0 / 16.0, "1/16"),
                    (1.0 / 8.0, "1/8"),
                    (1.0 / 4.0, "1/4"),
                    (1.0 / 2.0, "1/2"),
                    (1.0, "1"),
                ];
                for (value, label) in lengths {
                    let selected = (self.piano_note_len - value).abs() < f32::EPSILON;
                    let icon = egui::Image::new(Self::note_icon_source(value))
                        .fit_to_exact_size(egui::vec2(14.0, 14.0))
                        .tint(note_icon_tint);
                    let mut button = egui::Button::image(icon)
                        .min_size(note_button_size)
                        .fill(if selected { note_button_on } else { note_button_bg });
                    if ui.add_sized(note_button_size, button).on_hover_text(label).clicked() {
                        self.piano_note_len = value;
                    }
                }
            });
            ui.horizontal(|ui| {
                let grid_icon = egui::Image::new(egui::include_image!("../../icons/grid.svg"))
                    .fit_to_exact_size(egui::vec2(14.0, 14.0));
                ui.add(grid_icon);
                ui.label("Snap");
                let snaps = [
                    (1.0 / 32.0, "1/32"),
                    (1.0 / 16.0, "1/16"),
                    (1.0 / 8.0, "1/8"),
                    (1.0 / 4.0, "1/4"),
                    (1.0 / 2.0, "1/2"),
                    (1.0, "1"),
                ];
                for (value, label) in snaps {
                    let selected = (self.piano_snap - value).abs() < f32::EPSILON;
                    let icon = egui::Image::new(Self::note_icon_source(value))
                        .fit_to_exact_size(egui::vec2(14.0, 14.0))
                        .tint(note_icon_tint);
                    let mut button = egui::Button::image(icon)
                        .min_size(note_button_size)
                        .fill(if selected { note_button_on } else { note_button_bg });
                    if ui.add_sized(note_button_size, button).on_hover_text(label).clicked() {
                        self.piano_snap = value;
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("Lane");
                egui::ComboBox::from_id_source("piano_lane_mode")
                    .selected_text(match self.piano_lane_mode {
                        PianoLaneMode::Velocity => "Velocity",
                        PianoLaneMode::Pan => "Pan",
                        PianoLaneMode::Cutoff => "Cutoff",
                        PianoLaneMode::Resonance => "Resonance",
                        PianoLaneMode::MidiCc => "MIDI CC",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.piano_lane_mode,
                            PianoLaneMode::Velocity,
                            "Velocity",
                        );
                        ui.selectable_value(&mut self.piano_lane_mode, PianoLaneMode::Pan, "Pan");
                        ui.selectable_value(
                            &mut self.piano_lane_mode,
                            PianoLaneMode::Cutoff,
                            "Cutoff",
                        );
                        ui.selectable_value(
                            &mut self.piano_lane_mode,
                            PianoLaneMode::Resonance,
                            "Resonance",
                        );
                        ui.selectable_value(
                            &mut self.piano_lane_mode,
                            PianoLaneMode::MidiCc,
                            "MIDI CC",
                        );
                    });
                if self.piano_lane_mode == PianoLaneMode::MidiCc {
                    ui.label("CC");
                    ui.add(
                        egui::DragValue::new(&mut self.piano_cc)
                            .clamp_range(0..=127)
                            .speed(1.0),
                    );
                }
            });
        }
        ui.add_space(4.0);

                egui::SidePanel::left("piano_params")
                    .default_width(220.0)
                    .resizable(true)
                    .show_inside(ui, |ui| {
                        if !show_params {
                            return;
                        }
                        ui.heading(if is_audio_clip { "Audio" } else { "Parameters" });
                        ui.separator();
                        let track_color = self.selected_track.map(|i| self.track_color(i));
                        if is_audio_clip {
                            if let Some((ti, ci)) = selected_clip_info {
                                if let Some(clip) = self.tracks.get_mut(ti).and_then(|t| t.clips.get_mut(ci)) {
                                    ui.label("Clip Properties");
                                    ui.add_space(6.0);
                                    ui.horizontal(|ui| {
                                        ui.label("Gain");
                                        Self::colored_slider(ui, &mut clip.audio_gain, 0.0..=2.0, track_color);
                                    });
                                    ui.horizontal(|ui| {
                                        ui.label("Pitch");
                                        Self::colored_slider(
                                            ui,
                                            &mut clip.audio_pitch_semitones,
                                            -24.0..=24.0,
                                            track_color,
                                        );
                                    });
                                    ui.horizontal(|ui| {
                                        ui.label("Time Mul");
                                        Self::colored_slider(
                                            ui,
                                            &mut clip.audio_time_mul,
                                            0.25..=4.0,
                                            track_color,
                                        );
                                    });
                                    ui.horizontal(|ui| {
                                        ui.label("Offset");
                                        ui.add(egui::DragValue::new(&mut clip.audio_offset_beats).speed(0.1));
                                    });
                                    ui.add_space(6.0);
                                    if ui
                                        .add(egui::Button::image(
                                            egui::Image::new(egui::include_image!("../../icons/refresh-cw.svg"))
                                                .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                        ))
                                        .on_hover_text("Fit To Tempo")
                                        .clicked()
                                    {
                                        if let Some(source) = clip.audio_source_beats {
                                            if source > 0.0 && clip.length_beats > 0.0 {
                                                clip.audio_time_mul = source / clip.length_beats;
                                            }
                                        }
                                    }
                                    if ui
                                        .add(egui::Button::image(
                                            egui::Image::new(egui::include_image!("../../icons/rotate-ccw.svg"))
                                                .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                        ))
                                        .on_hover_text("Reset Audio Props")
                                        .clicked()
                                    {
                                        clip.audio_gain = 1.0;
                                        clip.audio_pitch_semitones = 0.0;
                                        clip.audio_time_mul = 1.0;
                                        clip.audio_offset_beats = 0.0;
                                    }
                                }
                            }
                        } else {
                            let track = self.selected_track.and_then(|i| self.tracks.get(i));
                            let name = track.map(|t| t.name.as_str()).unwrap_or("None");
                            ui.label(format!("Track: {name}"));
                            let plugin = track
                                .and_then(|t| t.instrument_path.as_deref())
                                .map(Self::plugin_display_name)
                                .unwrap_or_else(|| "No instrument".to_string());
                            ui.label(format!("Plugin: {plugin}"));
                            ui.add_space(6.0);
                            ui.label("Instrument");
                            ui.horizontal(|ui| {
                                let choose = ui
                                    .add(egui::Button::image(
                                        egui::Image::new(egui::include_image!("../../icons/folder-plus.svg"))
                                            .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                    ))
                                    .on_hover_text("Choose");
                                let open = ui
                                    .add(egui::Button::image(
                                        egui::Image::new(egui::include_image!("../../icons/external-link.svg"))
                                            .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                    ))
                                    .on_hover_text("Open UI");
                                let clear = ui
                                    .add(egui::Button::image(
                                        egui::Image::new(egui::include_image!("../../icons/x.svg"))
                                            .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                    ))
                                    .on_hover_text("Clear");
                                if let Some(index) = self.selected_track {
                                    if choose.clicked() {
                                        self.open_plugin_picker(PluginTarget::Instrument(index));
                                    }
                                    if open.clicked() {
                                        self.plugin_ui_target = Some(PluginUiTarget::Instrument(index));
                                        self.show_plugin_ui = true;
                                    }
                                    if clear.clicked() {
                                        if self.plugin_ui_matches(PluginUiTarget::Instrument(index)) {
                                            self.show_plugin_ui = false;
                                            self.destroy_plugin_ui();
                                        }
                                        if let Some(track) = self.tracks.get_mut(index) {
                                            track.instrument_path = None;
                                            track.params = default_midi_params();
                                            track.param_ids.clear();
                                            track.param_values.clear();
                                        }
                                        if let Some(state) = self.track_audio.get_mut(index) {
                                            if let Some(host) = state.host.take() {
                                                if let Ok(mut host) = host.lock() {
                                                    host.prepare_for_drop();
                                                }
                                                self.orphaned_hosts.push(host);
                                            }
                                        }
                                        self.reinit_audio_if_running();
                                    }
                                }
                            });
                            ui.add_space(6.0);
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                self.ensure_live_params();
                                let host_change = if let Some(host) = self.selected_track_host() {
                                    if let Ok(mut host) = host.try_lock() {
                                        host.take_last_param_change()
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                };
                                let selected_track_index = self.selected_track;
                                let track_color = selected_track_index.map(|i| self.track_color(i));
                                let menu_color = selected_track_index
                                    .map(|index| self.track_color(index))
                                    .unwrap_or_else(|| egui::Color32::from_gray(200));
                                let mut pending_automation_record: Vec<(usize, RecordedAutomationPoint)> = Vec::new();
                                let mut pending_lane_delete: Option<(usize, usize)> = None;
                                let mut pending_status: Option<String> = None;
                                let mut pending_midi_learn: Option<(usize, u32, String)> = None;
                                let mut pending_active_lane: Option<(usize, usize)> = None;
                                if let Some(track) = selected_track_index.and_then(|i| self.tracks.get_mut(i)) {
                                    if let Some((param_id, value)) = host_change {
                                        if let Some(pos) = track.param_ids.iter().position(|id| *id == param_id) {
                                            track.param_values[pos] = value as f32;
                                            self.last_ui_param_change = Some((param_id, value as f32));
                                        }
                                    }
                                    if track.param_values.len() != track.params.len() {
                                        track.param_values.resize(track.params.len(), 0.0);
                                    }
                                    if let Some(program_index) = track
                                        .params
                                        .iter()
                                        .position(|name| {
                                            let name = name.to_ascii_lowercase();
                                            name.contains("program") || name.contains("preset")
                                        })
                                    {
                                        let current = (track.param_values[program_index] * 127.0)
                                            .round()
                                            .clamp(0.0, 127.0) as u8;
                                        let mut selected = current;
                                        egui::ComboBox::from_label("Preset")
                                            .selected_text(format!(
                                                "{:03} {}",
                                                selected + 1,
                                                gm_program_name(selected)
                                            ))
                                            .show_ui(ui, |ui| {
                                                for program in 0u8..=127 {
                                                    let label = format!(
                                                        "{:03} {}",
                                                        program + 1,
                                                        gm_program_name(program)
                                                    );
                                                    if ui
                                                        .selectable_label(program == selected, label)
                                                        .clicked()
                                                    {
                                                        selected = program;
                                                    }
                                                }
                                            });
                                        if selected != current {
                                            let value = (selected as f32 / 127.0).clamp(0.0, 1.0);
                                            track.param_values[program_index] = value;
                                            if let Some(param_id) = track.param_ids.get(program_index).copied() {
                                                if let Some(state) = selected_track_index
                                                    .and_then(|i| self.track_audio.get(i))
                                                {
                                                    if let Ok(mut pending) =
                                                        state.pending_param_changes.lock()
                                                    {
                                                        pending.push(PendingParamChange {
                                                            target: PendingParamTarget::Instrument,
                                                            param_id,
                                                            value: value as f64,
                                                        });
                                                    }
                                                }
                                                if self.is_recording && self.record_automation {
                                                    if let Some(track_index) = selected_track_index {
                                                        pending_automation_record.push((
                                                            track_index,
                                                            RecordedAutomationPoint {
                                                                param_id,
                                                                target: AutomationTarget::Instrument,
                                                                beat: self.playhead_beats,
                                                                value,
                                                            },
                                                        ));
                                                    }
                                                }
                                            }
                                        }
                                        ui.add_space(6.0);
                                    }
                                    for index in 0..track.params.len() {
                                        let label = track.params[index].clone();
                                        let value = &mut track.param_values[index];
                                        let slider = ui.push_id(format!("param_{}", label), |ui| {
                                            ui.horizontal(|ui| {
                                                ui.label(&label);
                                                Self::colored_slider(ui, value, 0.0..=1.0, track_color)
                                            })
                                            .inner
                                        });
                                        let response = slider.response;
                                        let slider_response = slider.inner;
                                        let changed = slider_response.changed()
                                            || slider_response.dragged()
                                            || response.dragged();
                                        if changed {
                                            let param_id = track.param_ids.get(index).copied();
                                            let debug_id = param_id.unwrap_or(u32::MAX);
                                            self.last_ui_param_change = Some((debug_id, *value));
                                            if let Some(param_id) = param_id {
                                                if let Some(state) = selected_track_index
                                                    .and_then(|i| self.track_audio.get(i))
                                                {
                                                        if let Some(host) = state.host.as_ref() {
                                                            if let Ok(host) = host.try_lock() {
                                                                if let Some((channel, controller)) =
                                                                    host.param_to_cc(param_id)
                                                                {
                                                                    if let Ok(mut events) =
                                                                        state.midi_events.lock()
                                                                    {
                                                                        let cc_value =
                                                                            (*value * 127.0).round() as i32;
                                                                        let cc_value =
                                                                            cc_value.clamp(0, 127) as u8;
                                                                        events.push(
                                                                            vst3::MidiEvent::control_change(
                                                                                channel,
                                                                                controller,
                                                                                cc_value,
                                                                            ),
                                                                        );
                                                                    }
                                                                }
                                                            }
                                                        }
                                                        if let Ok(mut pending) =
                                                            state.pending_param_changes.lock()
                                                        {
                                                            pending.push(PendingParamChange {
                                                                target: PendingParamTarget::Instrument,
                                                                param_id,
                                                                value: *value as f64,
                                                            });
                                                        }
                                                }
                                                if self.is_recording && self.record_automation {
                                                    if let Some(track_index) = selected_track_index {
                                                        pending_automation_record.push((
                                                            track_index,
                                                            RecordedAutomationPoint {
                                                                param_id,
                                                                target: AutomationTarget::Instrument,
                                                                beat: self.playhead_beats,
                                                                value: *value,
                                                            },
                                                        ));
                                                    }
                                                }
                                            }
                                        }
                                        response.context_menu(|ui| {
                                            if ui
                                                .add(egui::Button::image_and_text(
                                                    egui::Image::new(egui::include_image!(
                                                        "../../icons/target.svg"
                                                    ))
                                                    .fit_to_exact_size(egui::vec2(12.0, 12.0))
                                                    .tint(menu_color),
                                                    egui::RichText::new("MIDI Learn")
                                                        .color(menu_color),
                                                ))
                                                .clicked()
                                            {
                                                if let Some(param_id) = track.param_ids.get(index).copied() {
                                                    if let Some(track_index) = selected_track_index {
                                                        pending_midi_learn =
                                                            Some((track_index, param_id, label.clone()));
                                                    }
                                                }
                                                ui.close_menu();
                                            }
                                            if ui
                                                .add(egui::Button::image_and_text(
                                                    egui::Image::new(egui::include_image!(
                                                        "../../icons/activity.svg"
                                                    ))
                                                    .fit_to_exact_size(egui::vec2(12.0, 12.0))
                                                    .tint(menu_color),
                                                    egui::RichText::new("Create Automation Lane")
                                                        .color(menu_color),
                                                ))
                                                .clicked()
                                            {
                                                if let Some(param_id) = track.param_ids.get(index).copied() {
                                                    if !track.automation_lanes.iter().any(|l| l.param_id == param_id) {
                                                        track.automation_lanes.push(AutomationLane {
                                                            name: label.clone(),
                                                            param_id,
                                                            target: AutomationTarget::Instrument,
                                                            points: Vec::new(),
                                                        });
                                                    }
                                                    if let Some(pos) = track
                                                        .automation_lanes
                                                        .iter()
                                                        .position(|l| l.param_id == param_id)
                                                    {
                                                        if let Some(track_index) = selected_track_index {
                                                            pending_active_lane = Some((track_index, pos));
                                                        }
                                                    }
                                                }
                                                ui.close_menu();
                                            }
                                        });
                                    }

                                    if ui
                                        .add(egui::Button::image(
                                            egui::Image::new(egui::include_image!("../../icons/shuffle.svg"))
                                                .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                        ))
                                        .on_hover_text("Randomize Params")
                                        .clicked()
                                    {
                                        let seed = std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .map(|d| d.as_nanos() as u64)
                                            .unwrap_or(0x1234_5678);
                                        let mut rng = seed;
                                        for idx in 0..track.param_values.len() {
                                            rng ^= rng << 13;
                                            rng ^= rng >> 7;
                                            rng ^= rng << 17;
                                            let value = (rng as f64 / u64::MAX as f64) as f32;
                                            track.param_values[idx] = value;
                                            if let Some(param_id) = track.param_ids.get(idx).copied() {
                                                if let Some(state) = selected_track_index
                                                    .and_then(|i| self.track_audio.get(i))
                                                {
                                                        if let Ok(mut pending) =
                                                            state.pending_param_changes.lock()
                                                        {
                                                            pending.push(PendingParamChange {
                                                                target: PendingParamTarget::Instrument,
                                                                param_id,
                                                                value: value as f64,
                                                            });
                                                        }
                                                }
                                            }
                                        }
                                    }

                                    if !track.effect_paths.is_empty() {
                                        ui.separator();
                                        ui.label("Effects Params");
                                        for (fx_index, fx_path) in track.effect_paths.iter().enumerate() {
                                            let title = format!(
                                                "FX {}: {}",
                                                fx_index + 1,
                                                Self::plugin_display_name(fx_path)
                                            );
                                            ui.collapsing(title, |ui| {
                                                let params = track
                                                    .effect_params
                                                    .get(fx_index)
                                                    .cloned()
                                                    .unwrap_or_default();
                                                if params.is_empty() {
                                                    ui.label("(no parameters)");
                                                    return;
                                                }
                                                if let Some(values) = track.effect_param_values.get_mut(fx_index) {
                                                    if values.len() != params.len() {
                                                        values.resize(params.len(), 0.0);
                                                    }
                                                }
                                                let ids = track
                                                    .effect_param_ids
                                                    .get(fx_index)
                                                    .cloned()
                                                    .unwrap_or_default();
                                                for param_index in 0..params.len() {
                                                    let label = params[param_index].clone();
                                                    let value = track
                                                        .effect_param_values
                                                        .get_mut(fx_index)
                                                        .and_then(|vals| vals.get_mut(param_index));
                                                    let Some(value) = value else {
                                                        continue;
                                                    };
                                                    let slider = ui.push_id(
                                                        format!("fx{}_param_{}", fx_index, label),
                                                        |ui| {
                                                            ui.horizontal(|ui| {
                                                                ui.label(&label);
                                                                Self::colored_slider(
                                                                    ui,
                                                                    value,
                                                                    0.0..=1.0,
                                                                    track_color,
                                                                )
                                                            })
                                                            .inner
                                                        },
                                                    );
                                                    let response = slider.response;
                                                    let slider_response = slider.inner;
                                                    let changed = slider_response.changed()
                                                        || slider_response.dragged()
                                                        || response.dragged();
                                                    if changed {
                                                        if let Some(param_id) = ids.get(param_index).copied() {
                                                            if let Some(state) = selected_track_index
                                                                .and_then(|i| self.track_audio.get(i))
                                                            {
                                                                if state.effect_hosts.get(fx_index).is_some() {
                                                                    if let Ok(mut pending) =
                                                                        state.pending_param_changes.lock()
                                                                    {
                                                                        pending.push(PendingParamChange {
                                                                            target: PendingParamTarget::Effect(
                                                                                fx_index,
                                                                            ),
                                                                            param_id,
                                                                            value: *value as f64,
                                                                        });
                                                                    }
                                                                }
                                                            }
                                                            if self.is_recording && self.record_automation {
                                                                if let Some(track_index) = selected_track_index {
                                                                    pending_automation_record.push((
                                                                        track_index,
                                                                        RecordedAutomationPoint {
                                                                            param_id,
                                                                            target: AutomationTarget::Effect(
                                                                                fx_index,
                                                                            ),
                                                                            beat: self.playhead_beats,
                                                                            value: *value,
                                                                        },
                                                                    ));
                                                                }
                                                            }
                                                        }
                                                    }
                                                    response.context_menu(|ui| {
                                                        if ui
                                                            .add(egui::Button::image_and_text(
                                                                egui::Image::new(egui::include_image!(
                                                                    "../../icons/activity.svg"
                                                                ))
                                                                .fit_to_exact_size(egui::vec2(12.0, 12.0))
                                                                .tint(menu_color),
                                                                egui::RichText::new("Create Automation Lane")
                                                                    .color(menu_color),
                                                            ))
                                                            .clicked()
                                                        {
                                                            if let Some(param_id) = ids.get(param_index).copied() {
                                                                if !track.automation_lanes.iter().any(|l| {
                                                                    l.param_id == param_id
                                                                        && l.target
                                                                            == AutomationTarget::Effect(fx_index)
                                                                }) {
                                                                    track.automation_lanes.push(AutomationLane {
                                                                        name: format!(
                                                                            "{}: {}",
                                                                            Self::plugin_display_name(fx_path),
                                                                            label
                                                                        ),
                                                                        param_id,
                                                                        target: AutomationTarget::Effect(
                                                                            fx_index,
                                                                        ),
                                                                        points: Vec::new(),
                                                                    });
                                                                }
                                                            }
                                                            ui.close_menu();
                                                        }
                                                    });
                                                }
                                            });
                                        }
                                    }

                                    if let Some((track_index, param_id, label)) = pending_midi_learn.take() {
                                        if let Ok(mut learn) = self.midi_learn.lock() {
                                            *learn = Some((track_index, param_id));
                                        }
                                        pending_status =
                                            Some(format!("MIDI Learn armed for {}", label));
                                    }
                                    if let Some(status) = pending_status.take() {
                                        self.status = status;
                                    }
                                    if let Some(active) = pending_active_lane.take() {
                                        self.automation_active = Some(active);
                                    }

                                    if !track.automation_lanes.is_empty() {
                                        ui.separator();
                                        ui.label("Automation Lanes");
                                        for (lane_index, lane) in track.automation_lanes.iter().enumerate() {
                                            ui.horizontal(|ui| {
                                                let selected = selected_track_index
                                                    .and_then(|ti| self.automation_active.map(|(ai, li)| (ti, ai, li)))
                                                    .map(|(ti, ai, li)| ti == ai && li == lane_index)
                                                    .unwrap_or(false);
                                                let lane_response = ui.selectable_label(
                                                    selected,
                                                    format!("• {}", lane.name),
                                                );
                                                if lane_response.clicked() {
                                                    if let Some(track_index) = selected_track_index {
                                                        self.automation_active = Some((track_index, lane_index));
                                                    }
                                                }
                                                if ui
                                                    .add(egui::Button::image(
                                                        egui::Image::new(egui::include_image!("../../icons/trash-2.svg"))
                                                            .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                                    ))
                                                    .on_hover_text("Delete")
                                                    .clicked()
                                                {
                                                    if let Some(track_index) = selected_track_index {
                                                        pending_lane_delete = Some((track_index, lane_index));
                                                    }
                                                }
                                            });
                                        }
                                    }
                                }
                                for (track_index, point) in pending_automation_record {
                                    self.record_automation_point(
                                        track_index,
                                        point.target,
                                        point.param_id,
                                        point.beat,
                                        point.value,
                                    );
                                }
                                if let Some((track_index, lane_index)) = pending_lane_delete {
                                    if let Some(track) = self.tracks.get_mut(track_index) {
                                        if lane_index < track.automation_lanes.len() {
                                            track.automation_lanes.remove(lane_index);
                                        }
                                    }
                                    if let Some(state) = self.track_audio.get(track_index) {
                                        if let Ok(mut lanes) = state.automation_lanes.lock() {
                                            *lanes = self
                                                .tracks
                                                .get(track_index)
                                                .map(|t| t.automation_lanes.clone())
                                                .unwrap_or_default();
                                        }
                                    }
                                    if let Some((active_track, active_lane)) = self.automation_active {
                                        if active_track == track_index {
                                            if active_lane == lane_index {
                                                self.automation_active = None;
                                            } else if active_lane > lane_index {
                                                self.automation_active = Some((track_index, active_lane - 1));
                                            }
                                        }
                                    }
                                }
                            });
                        }
                    });

                egui::CentralPanel::default().show_inside(ui, |ui| {
                    if !show_roll {
                        self.piano_roll_hovered = false;
                        self.piano_roll_rect = None;
                        ui.centered_and_justified(|ui| {
                            ui.label("Parameters");
                        });
                        return;
                    }
                    if is_audio_clip {
                        let (rect, response) =
                            ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
                        self.piano_roll_hovered = response.hovered();
                        self.piano_roll_rect = Some(rect);
                        let painter = ui.painter_at(rect);
                        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(12, 14, 16));
                        let preview_rect = rect.shrink2(egui::vec2(12.0, 28.0));
                        let selected_clip = selected_clip_info
                            .and_then(|(ti, ci)| self.tracks.get(ti).and_then(|t| t.clips.get(ci)));
                        let waveform = selected_clip.and_then(|clip| self.get_waveform_for_clip(clip));
                        let waveform_color =
                            selected_clip.and_then(|clip| self.get_waveform_color_for_clip(clip));
                        if let Some(clip) = selected_clip {
                            self.draw_audio_preview(
                                &painter,
                                preview_rect,
                                self.selected_clip.unwrap_or(0),
                                waveform.as_deref(),
                                waveform_color.as_deref(),
                                clip,
                                None,
                            );
                        }
                        let controls_rect = egui::Rect::from_min_max(
                            egui::pos2(rect.left() + 12.0, rect.bottom() - 24.0),
                            egui::pos2(rect.right() - 12.0, rect.bottom() - 6.0),
                        );
                        let mut x = controls_rect.left();
                        let button_w = 64.0;
                        let gap = 8.0;
                        let play_rect = egui::Rect::from_min_size(
                            egui::pos2(x, controls_rect.top()),
                            egui::vec2(button_w, controls_rect.height()),
                        );
                        x += button_w + gap;
                        let stop_rect = egui::Rect::from_min_size(
                            egui::pos2(x, controls_rect.top()),
                            egui::vec2(button_w, controls_rect.height()),
                        );
                        x += button_w + gap;
                        let loop_rect = egui::Rect::from_min_size(
                            egui::pos2(x, controls_rect.top()),
                            egui::vec2(button_w, controls_rect.height()),
                        );
                        if ui
                            .put(
                                play_rect,
                                egui::Button::image(
                                    egui::Image::new(egui::include_image!("../../icons/play.svg"))
                                        .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                ),
                            )
                            .on_hover_text("Play")
                            .clicked()
                        {
                            if let Some((ti, ci)) = selected_clip_info {
                                if let Some(clip) = self.tracks.get(ti).and_then(|t| t.clips.get(ci)).cloned() {
                                    if let Err(err) = self.start_audio_preview(&clip) {
                                        self.status = format!("Audio preview failed: {err}");
                                    } else {
                                        self.status = "Audio preview: play".to_string();
                                    }
                                }
                            }
                        }
                        if ui
                            .put(
                                stop_rect,
                                egui::Button::image(
                                    egui::Image::new(egui::include_image!("../../icons/stop-circle.svg"))
                                        .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                ),
                            )
                            .on_hover_text("Stop")
                            .clicked()
                        {
                            self.stop_audio_preview();
                            self.status = "Audio preview: stop".to_string();
                        }
                        let loop_label = if self.audio_preview_loop { "Loop On" } else { "Loop Off" };
                        if ui
                            .put(
                                loop_rect,
                                egui::Button::image(
                                    egui::Image::new(egui::include_image!("../../icons/repeat.svg"))
                                        .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                ),
                            )
                            .on_hover_text(loop_label)
                            .clicked()
                        {
                            self.audio_preview_loop = !self.audio_preview_loop;
                            if let Some((ti, ci)) = selected_clip_info {
                                if let Some(clip) = self.tracks.get(ti).and_then(|t| t.clips.get(ci)).cloned() {
                                    if self.audio_preview_sink.is_some() && self.audio_preview_clip_id == Some(clip.id) {
                                        let _ = self.start_audio_preview(&clip);
                                    }
                                }
                            }
                        }
                        return;
                    }
                    let total_size = ui.available_size();
                    let lane_height = 160.0;
                    let roll_height = (total_size.y - lane_height).max(80.0);
                    let lane_height = (total_size.y - roll_height).max(0.0);
                    let (roll_rect, roll_response) = ui.allocate_exact_size(
                        egui::vec2(total_size.x, roll_height),
                        egui::Sense::click_and_drag(),
                    );
                    let (lane_rect, lane_response) = ui.allocate_exact_size(
                        egui::vec2(total_size.x, lane_height),
                        egui::Sense::click_and_drag(),
                    );
                    self.piano_roll_hovered = roll_response.hovered();
                    let keyboard_w = 56.0;
                    let header_height = 20.0;
                    let header_rect = egui::Rect::from_min_max(
                        egui::pos2(roll_rect.left(), roll_rect.top()),
                        egui::pos2(roll_rect.right(), roll_rect.top() + header_height),
                    );
                    let keyboard_rect = egui::Rect::from_min_max(
                        egui::pos2(roll_rect.left(), header_rect.bottom()),
                        egui::pos2(roll_rect.left() + keyboard_w, roll_rect.bottom()),
                    );
                    let roll_rect = egui::Rect::from_min_max(
                        egui::pos2(keyboard_rect.right(), header_rect.bottom()),
                        roll_rect.max,
                    );
                    let pointer_pos = ctx.input(|i| i.pointer.hover_pos());
                    let over_header = pointer_pos
                        .map(|pos| header_rect.contains(pos))
                        .unwrap_or(false);
                    let roll_or_header_hovered = roll_response.hovered()
                        || lane_response.hovered()
                        || over_header;
                    self.piano_roll_hovered = roll_or_header_hovered;
                    let piano_rect = egui::Rect::from_min_max(
                        egui::pos2(keyboard_rect.left(), header_rect.top()),
                        egui::pos2(roll_rect.right(), lane_rect.bottom().max(roll_rect.bottom())),
                    );
                    self.piano_roll_rect = Some(piano_rect);
                    if roll_or_header_hovered {
                        let input = ctx.input(|i| i.clone());
                        if input.modifiers.ctrl {
                            let zoom = input.zoom_delta();
                            if (zoom - 1.0).abs() > f32::EPSILON {
                                self.piano_zoom_x = (self.piano_zoom_x * zoom).clamp(0.3, 6.0);
                            } else {
                                let mut delta = input.smooth_scroll_delta;
                                if delta == egui::Vec2::ZERO {
                                    delta = input.raw_scroll_delta;
                                }
                                let zoom_delta = (delta.x + delta.y) * 0.001;
                                self.piano_zoom_x = (self.piano_zoom_x + zoom_delta).clamp(0.3, 6.0);
                            }
                        } else if input.modifiers.shift {
                            let mut delta = input.smooth_scroll_delta;
                            if delta == egui::Vec2::ZERO {
                                delta = input.raw_scroll_delta;
                            }
                            let pan_delta = if delta.x.abs() > f32::EPSILON {
                                delta.x
                            } else if delta.y.abs() > f32::EPSILON {
                                delta.y
                            } else {
                                0.0
                            };
                            self.piano_pan.x += pan_delta;
                        } else {
                            let mut delta = input.smooth_scroll_delta;
                            if delta == egui::Vec2::ZERO {
                                delta = input.raw_scroll_delta;
                            }
                            if delta.x.abs() > f32::EPSILON {
                                self.piano_pan.x += delta.x;
                            }
                            if delta.y.abs() > f32::EPSILON {
                                self.piano_pan.y += delta.y;
                            }
                        }
                    }
                    let paint_rect = egui::Rect::from_min_max(
                        egui::pos2(keyboard_rect.left(), header_rect.top()),
                        egui::pos2(roll_rect.right(), roll_rect.bottom()),
                    );
                    let painter = ui.painter_at(paint_rect);
                    painter.rect_filled(roll_rect, 0.0, egui::Color32::from_rgb(12, 14, 16));
                    painter.rect_filled(keyboard_rect, 0.0, egui::Color32::from_rgb(10, 12, 14));
                    let beat_width = 24.0 * self.piano_zoom_x;
                    let note_height = 10.0 * self.piano_zoom_y;
                    let header_id = egui::Id::new("piano_roll_timeline");
                    let header_response = ui.interact(header_rect, header_id, egui::Sense::click());
                    if header_response.clicked() {
                        if let Some(pos) = header_response.interact_pointer_pos() {
                            let beats = self.beats_from_pos(pos.x, roll_rect.left() + self.piano_pan.x, beat_width);
                            self.seek_playhead(beats);
                        }
                    }
                    let mut x = roll_rect.left() + self.piano_pan.x;
                    let mut beat_idx = 0;
                    while x <= roll_rect.right() {
                        let major = beat_idx % 4 == 0;
                        let color = if major {
                            egui::Color32::from_rgba_premultiplied(26, 28, 32, 180)
                        } else {
                            egui::Color32::from_rgba_premultiplied(18, 20, 24, 160)
                        };
                        painter.line_segment(
                            [egui::pos2(x, roll_rect.top()), egui::pos2(x, roll_rect.bottom())],
                            egui::Stroke::new(1.0, color),
                        );
                        beat_idx += 1;
                        x += beat_width;
                    }
                    for note in 0u8..=127 {
                        let y = roll_rect.bottom() + self.piano_pan.y
                            - (note as f32 - 40.0) * note_height;
                        if y < roll_rect.top() || y > roll_rect.bottom() {
                            continue;
                        }
                        let is_c = note % 12 == 0;
                        let grid_color = if is_c {
                            egui::Color32::from_rgba_premultiplied(60, 64, 72, 220)
                        } else {
                            egui::Color32::from_rgba_premultiplied(20, 22, 26, 160)
                        };
                        let grid_width = if is_c { 1.6 } else { 1.0 };
                        painter.line_segment(
                            [egui::pos2(roll_rect.left(), y), egui::pos2(roll_rect.right(), y)],
                            egui::Stroke::new(grid_width, grid_color),
                        );
                    }
                    for note in 0u8..=127 {
                        let y = roll_rect.bottom() + self.piano_pan.y
                            - (note as f32 - 40.0) * note_height;
                        let key_rect = egui::Rect::from_min_max(
                            egui::pos2(keyboard_rect.left(), y - note_height),
                            egui::pos2(keyboard_rect.right(), y),
                        );
                        if key_rect.bottom() < roll_rect.top() || key_rect.top() > roll_rect.bottom() {
                            continue;
                        }
                        let is_black = matches!(note % 12, 1 | 3 | 6 | 8 | 10);
                        let key_color = if is_black {
                            egui::Color32::from_rgb(24, 26, 30)
                        } else {
                            egui::Color32::from_rgb(200, 200, 200)
                        };
                        painter.rect_filled(key_rect, 0.0, key_color);
                        painter.rect_stroke(
                            key_rect,
                            0.0,
                            egui::Stroke::new(1.0, egui::Color32::from_rgb(8, 10, 12)),
                        );
                        let is_c = note % 12 == 0;
                        if is_c {
                            let octave = (note / 12) as i32 - 1;
                            Self::outlined_text(
                                &painter,
                                egui::pos2(keyboard_rect.left() + 4.0, y - note_height + 2.0),
                                egui::Align2::LEFT_TOP,
                                &format!("C{octave}"),
                                egui::FontId::proportional(9.0),
                                egui::Color32::from_gray(120),
                            );
                        }
                    }
                    let mut hovered_note: Option<(usize, egui::Rect)> = None;
                    if let Some(track_index) = self.selected_track {
                        if let Some(track) = self.tracks.get(track_index) {
                            if self.selected_clip.is_some() && !track.midi_notes.is_empty() {
                                for (index, note) in track.midi_notes.iter().enumerate() {
                                    let x = roll_rect.left() + self.piano_pan.x + note.start_beats * beat_width;
                                    let y = roll_rect.bottom() + self.piano_pan.y
                                        - (note.midi_note as f32 - 40.0) * note_height;
                                    let w = (note.length_beats * beat_width).max(12.0);
                                    let note_rect = egui::Rect::from_min_size(
                                        egui::pos2(x, y - note_height),
                                        egui::vec2(w, note_height),
                                    );
                                    let base = if index % 2 == 0 {
                                        egui::Color32::from_rgb(88, 210, 180)
                                    } else {
                                        egui::Color32::from_rgb(120, 130, 240)
                                    };
                                    let vel = (note.velocity as f32 / 127.0).clamp(0.0, 1.0);
                                    let alpha = (vel * 200.0 + 30.0).clamp(40.0, 230.0) as u8;
                                    let pan = note.pan.clamp(-1.0, 1.0);
                                    let pan_red = (pan.max(0.0) * 80.0) as u8;
                                    let pan_blue = ((-pan).max(0.0) * 80.0) as u8;
                                    let cutoff_green = (note.cutoff.clamp(0.0, 1.0) * 80.0) as u8;
                                    let r = (base.r() as u16 + pan_red as u16).min(255) as u8;
                                    let g = (base.g() as u16 + cutoff_green as u16).min(255) as u8;
                                    let b = (base.b() as u16 + pan_blue as u16).min(255) as u8;
                                    let color = egui::Color32::from_rgba_premultiplied(r, g, b, alpha);
                                    painter.rect_filled(note_rect, 0.0, color);
                                    if self.piano_selected.contains(&index) {
                                        painter.rect_stroke(
                                            note_rect,
                                            0.0,
                                            egui::Stroke::new(1.4, egui::Color32::from_rgb(230, 240, 255)),
                                        );
                                    }
                                    if roll_response.hovered() {
                                        if let Some(pos) = roll_response.hover_pos() {
                                            if pos.x >= roll_rect.left() && note_rect.contains(pos) {
                                                hovered_note = Some((index, note_rect));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if roll_response.hovered() {
                        if let Some((_, note_rect)) = hovered_note {
                            if let Some(pos) = roll_response.hover_pos() {
                                if pos.x >= roll_rect.left() {
                                    let right_edge = note_rect.right();
                                    let icon = if (right_edge - pos.x).abs() <= 6.0 {
                                        egui::CursorIcon::ResizeHorizontal
                                    } else {
                                        egui::CursorIcon::Grab
                                    };
                                    roll_response.clone().on_hover_cursor(icon);
                                }
                            }
                        }
                    }

                    if self.selected_clip.is_none() {
                        Self::outlined_text(
                            &painter,
                            roll_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "Select a MIDI clip to edit",
                            egui::FontId::proportional(14.0),
                            egui::Color32::from_gray(160),
                        );
                    }

                    let input = ctx.input(|i| i.clone());
                    let ctrl = input.modifiers.ctrl;
                    let shift = input.modifiers.shift;
                    let alt = input.modifiers.alt;
                    let mut marquee_rect: Option<egui::Rect> = None;
                    if ctrl && roll_response.drag_started() {
                        if let Some(pos) = roll_response.interact_pointer_pos() {
                            if pos.x >= roll_rect.left() {
                                self.piano_marquee_start = Some(pos);
                                self.piano_marquee_add = shift;
                            }
                        }
                    }
                    if let Some(start) = self.piano_marquee_start {
                        if roll_response.dragged() {
                            if let Some(pos) = roll_response.interact_pointer_pos() {
                                marquee_rect = Some(egui::Rect::from_two_pos(start, pos));
                            }
                        }
                        if roll_response.drag_stopped() {
                            if let Some(end) = roll_response.interact_pointer_pos() {
                                let select_rect = egui::Rect::from_two_pos(start, end);
                                if let Some(track_index) = self.selected_track {
                                    if let Some(track) = self.tracks.get(track_index) {
                                        let mut hits: Vec<usize> = Vec::new();
                                        for (index, note) in track.midi_notes.iter().enumerate() {
                                            let x = roll_rect.left()
                                                + self.piano_pan.x
                                                + note.start_beats * beat_width;
                                            let y = roll_rect.bottom() + self.piano_pan.y
                                                - (note.midi_note as f32 - 40.0) * note_height;
                                            let w = (note.length_beats * beat_width).max(12.0);
                                            let note_rect = egui::Rect::from_min_size(
                                                egui::pos2(x, y - note_height),
                                                egui::vec2(w, note_height),
                                            );
                                            if select_rect.intersects(note_rect) {
                                                hits.push(index);
                                            }
                                        }
                                        if !self.piano_marquee_add {
                                            self.piano_selected.clear();
                                        }
                                        for index in hits {
                                            self.piano_selected.insert(index);
                                        }
                                    }
                                }
                            }
                            self.piano_marquee_start = None;
                            self.piano_marquee_add = false;
                        }
                    }

                    let quantize = self.piano_snap.max(0.03125);
                    if ctrl && roll_response.clicked_by(egui::PointerButton::Primary) {
                        if let Some(pos) = roll_response.interact_pointer_pos() {
                            if pos.x < roll_rect.left() {
                                return;
                            }
                            if let Some((note_index, _)) = hovered_note {
                                if !shift {
                                    self.piano_selected.clear();
                                }
                                self.piano_selected.insert(note_index);
                            } else if !shift {
                                self.piano_selected.clear();
                            }
                        }
                    } else if roll_response.clicked_by(egui::PointerButton::Primary) {
                        if let Some(pos) = roll_response.interact_pointer_pos() {
                            if pos.x < roll_rect.left() {
                                return;
                            }
                            if let Some(track_index) = self.selected_track {
                                if let Some(track) = self.tracks.get_mut(track_index) {
                                    if hovered_note.is_none() {
                                        let beat = (pos.x - roll_rect.left() - self.piano_pan.x) / beat_width;
                                        let snapped = if alt {
                                            beat
                                        } else {
                                            (beat / quantize).round() * quantize
                                        };
                                        let pitch_f = (roll_rect.bottom() + self.piano_pan.y - pos.y) / note_height;
                                        let pitch = (40.0 + pitch_f).floor() as i32;
                                        let pitch = pitch.clamp(0, 127) as u8;
                                        if shift {
                                            track.midi_notes.retain(|note| {
                                                note.midi_note != pitch
                                                    || note.start_beats + note.length_beats <= snapped
                                                    || note.start_beats >= snapped + self.piano_note_len
                                            });
                                        }
                                        track
                                            .midi_notes
                                            .push(PianoRollNote::new(
                                                snapped.max(0.0),
                                                self.piano_note_len,
                                                pitch,
                                                100,
                                            ));
                                        if let Some(index) = track.midi_notes.len().checked_sub(1) {
                                            self.piano_selected.clear();
                                            self.piano_selected.insert(index);
                                        }
                                        self.sync_track_audio_notes(track_index);
                                    }
                                }
                            }
                        }
                    }

                    if roll_response.clicked_by(egui::PointerButton::Secondary) {
                        if let Some((note_index, _)) = hovered_note {
                            if let Some(track_index) = self.selected_track {
                                if let Some(track) = self.tracks.get_mut(track_index) {
                                    if note_index < track.midi_notes.len() {
                                        track.midi_notes.remove(note_index);
                                        self.piano_selected.remove(&note_index);
                                        let shifted: HashSet<usize> = self
                                            .piano_selected
                                            .iter()
                                            .map(|idx| if *idx > note_index { idx - 1 } else { *idx })
                                            .collect();
                                        self.piano_selected = shifted;
                                        self.sync_track_audio_notes(track_index);
                                    }
                                }
                            }
                        }
                    }

                    if roll_response.drag_started() && !ctrl {
                        if let Some((note_index, note_rect)) = hovered_note {
                            if let Some(pos) = roll_response.interact_pointer_pos() {
                                if pos.x < roll_rect.left() {
                                    return;
                                }
                                let right_edge = note_rect.right();
                                let kind = if (right_edge - pos.x).abs() <= 6.0 {
                                    PianoDragKind::Resize
                                } else {
                                    PianoDragKind::Move
                                };
                                let offset_beats = (pos.x - roll_rect.left() - self.piano_pan.x) / beat_width;
                                self.piano_drag = Some(PianoDragState {
                                    track_index: self.selected_track.unwrap_or(0),
                                    note_index,
                                    kind,
                                    offset_beats,
                                });
                            }
                        }
                    }

                    if roll_response.dragged() && !ctrl {
                        if let Some(drag) = &self.piano_drag {
                            if let Some(pos) = roll_response.interact_pointer_pos() {
                                if pos.x < roll_rect.left() {
                                    return;
                                }
                                if let Some(track) = self.tracks.get_mut(drag.track_index) {
                                    if let Some(note) = track.midi_notes.get_mut(drag.note_index) {
                                        let beat = (pos.x - roll_rect.left() - self.piano_pan.x) / beat_width;
                                        match drag.kind {
                                            PianoDragKind::Move => {
                                                let snapped = if alt {
                                                    beat - drag.offset_beats
                                                } else {
                                                    ((beat - drag.offset_beats) / quantize).round() * quantize
                                                };
                                                note.start_beats = snapped.max(0.0);
                                            }
                                            PianoDragKind::Resize => {
                                                let length = beat - note.start_beats;
                                                let snapped = if alt {
                                                    length
                                                } else {
                                                    (length / quantize).round() * quantize
                                                };
                                                note.length_beats = snapped.max(quantize);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if roll_response.drag_stopped() {
                        if let Some(drag) = self.piano_drag.take() {
                            self.sync_track_audio_notes(drag.track_index);
                        }
                    }

                    if let Some(rect) = marquee_rect {
                        painter.rect_stroke(
                            rect,
                            0.0,
                            egui::Stroke::new(1.2, egui::Color32::from_rgb(120, 170, 255)),
                        );
                        painter.rect_filled(
                            rect,
                            0.0,
                            egui::Color32::from_rgba_premultiplied(80, 120, 200, 40),
                        );
                    }

                    let playhead_x = roll_rect.left() + self.piano_pan.x + self.playhead_beats * beat_width;
                    if playhead_x >= roll_rect.left() && playhead_x <= roll_rect.right() {
                        painter.line_segment(
                            [
                                egui::pos2(playhead_x, roll_rect.top() + 2.0),
                                egui::pos2(playhead_x, roll_rect.bottom() - 4.0),
                            ],
                            egui::Stroke::new(1.2, egui::Color32::from_rgb(255, 86, 70)),
                        );
                    }
                    painter.rect_filled(header_rect, 0.0, egui::Color32::from_rgb(0, 0, 0));
                    painter.line_segment(
                        [
                            egui::pos2(header_rect.left(), header_rect.bottom()),
                            egui::pos2(header_rect.right(), header_rect.bottom()),
                        ],
                        egui::Stroke::new(1.0, egui::Color32::from_rgb(28, 30, 34)),
                    );
                    let mut beat_index = 0;
                    let mut header_x = roll_rect.left() + self.piano_pan.x;
                    while header_x <= header_rect.right() {
                        if beat_index % 4 == 0 {
                            let bar = beat_index / 4 + 1;
                            Self::outlined_text(
                                &painter,
                                egui::pos2(header_x + 4.0, header_rect.top() + 2.0),
                                egui::Align2::LEFT_TOP,
                                &format!("{bar}"),
                                egui::FontId::proportional(10.0),
                                egui::Color32::from_gray(160),
                            );
                        }
                        beat_index += 1;
                        header_x += beat_width;
                    }

                    if lane_rect.height() > 4.0 {
                        let lane_painter = ui.painter_at(lane_rect);
                        lane_painter.rect_filled(
                            lane_rect,
                            0.0,
                            egui::Color32::from_rgb(8, 9, 11),
                        );
                        lane_painter.line_segment(
                            [
                                egui::pos2(lane_rect.left(), lane_rect.top()),
                                egui::pos2(lane_rect.right(), lane_rect.top()),
                            ],
                            egui::Stroke::new(1.0, egui::Color32::from_rgb(24, 26, 30)),
                        );

                        let mut x = roll_rect.left() + self.piano_pan.x;
                        let mut beat_idx = 0;
                        while x <= lane_rect.right() {
                            let major = beat_idx % 4 == 0;
                            let color = if major {
                                egui::Color32::from_rgba_premultiplied(24, 26, 30, 160)
                            } else {
                                egui::Color32::from_rgba_premultiplied(18, 20, 24, 140)
                            };
                            lane_painter.line_segment(
                                [egui::pos2(x, lane_rect.top()), egui::pos2(x, lane_rect.bottom())],
                                egui::Stroke::new(1.0, color),
                            );
                            beat_idx += 1;
                            x += beat_width;
                        }

                        if let Some(track_index) = self.selected_track {
                            if let Some(track) = self.tracks.get(track_index) {
                                match self.piano_lane_mode {
                                    PianoLaneMode::Velocity => {
                                        for note in &track.midi_notes {
                                            let value = (note.velocity as f32 / 127.0).clamp(0.0, 1.0);
                                            let h = lane_rect.height() * value;
                                            let x = roll_rect.left() + self.piano_pan.x + note.start_beats * beat_width;
                                            let w = (note.length_beats * beat_width).max(6.0);
                                            let bar_rect = egui::Rect::from_min_size(
                                                egui::pos2(x, lane_rect.bottom() - h),
                                                egui::vec2(w, h),
                                            );
                                            lane_painter.rect_filled(
                                                bar_rect,
                                                0.0,
                                                egui::Color32::from_rgba_premultiplied(180, 200, 220, 200),
                                            );
                                        }
                                    }
                                    PianoLaneMode::Pan => {
                                        let center_y = lane_rect.center().y;
                                        lane_painter.line_segment(
                                            [
                                                egui::pos2(lane_rect.left(), center_y),
                                                egui::pos2(lane_rect.right(), center_y),
                                            ],
                                            egui::Stroke::new(1.0, egui::Color32::from_rgb(32, 36, 40)),
                                        );
                                        for note in &track.midi_notes {
                                            let pan = note.pan.clamp(-1.0, 1.0);
                                            let h = lane_rect.height() * 0.5 * pan.abs();
                                            let x = roll_rect.left() + self.piano_pan.x + note.start_beats * beat_width;
                                            let w = (note.length_beats * beat_width).max(6.0);
                                            let (y, color) = if pan >= 0.0 {
                                                (center_y - h, egui::Color32::from_rgb(210, 80, 80))
                                            } else {
                                                (center_y, egui::Color32::from_rgb(80, 120, 210))
                                            };
                                            let bar_rect = egui::Rect::from_min_size(
                                                egui::pos2(x, y),
                                                egui::vec2(w, h.max(2.0)),
                                            );
                                            lane_painter.rect_filled(bar_rect, 0.0, color);
                                        }
                                    }
                                    PianoLaneMode::Cutoff => {
                                        for note in &track.midi_notes {
                                            let value = note.cutoff.clamp(0.0, 1.0);
                                            let h = lane_rect.height() * value;
                                            let x = roll_rect.left() + self.piano_pan.x + note.start_beats * beat_width;
                                            let w = (note.length_beats * beat_width).max(6.0);
                                            let bar_rect = egui::Rect::from_min_size(
                                                egui::pos2(x, lane_rect.bottom() - h),
                                                egui::vec2(w, h.max(2.0)),
                                            );
                                            lane_painter.rect_filled(
                                                bar_rect,
                                                0.0,
                                                egui::Color32::from_rgb(90, 200, 120),
                                            );
                                        }
                                    }
                                    PianoLaneMode::Resonance => {
                                        for note in &track.midi_notes {
                                            let value = note.resonance.clamp(0.0, 1.0);
                                            let h = lane_rect.height() * value;
                                            let x = roll_rect.left() + self.piano_pan.x + note.start_beats * beat_width;
                                            let w = (note.length_beats * beat_width).max(6.0);
                                            let bar_rect = egui::Rect::from_min_size(
                                                egui::pos2(x, lane_rect.bottom() - h),
                                                egui::vec2(w, h.max(2.0)),
                                            );
                                            lane_painter.rect_filled(
                                                bar_rect,
                                                0.0,
                                                egui::Color32::from_rgb(210, 180, 80),
                                            );
                                        }
                                    }
                                    PianoLaneMode::MidiCc => {
                                        if let Some(lane) = track
                                            .midi_cc_lanes
                                            .iter()
                                            .find(|lane| lane.cc == self.piano_cc)
                                        {
                                            let mut points = lane.points.clone();
                                            points.sort_by(|a, b| a.beat.partial_cmp(&b.beat).unwrap_or(std::cmp::Ordering::Equal));
                                            for window in points.windows(2) {
                                                let a = &window[0];
                                                let b = &window[1];
                                                let x1 = roll_rect.left() + self.piano_pan.x + a.beat * beat_width;
                                                let x2 = roll_rect.left() + self.piano_pan.x + b.beat * beat_width;
                                                let y1 = lane_rect.bottom() - a.value.clamp(0.0, 1.0) * lane_rect.height();
                                                let y2 = lane_rect.bottom() - b.value.clamp(0.0, 1.0) * lane_rect.height();
                                                lane_painter.line_segment(
                                                    [egui::pos2(x1, y1), egui::pos2(x2, y2)],
                                                    egui::Stroke::new(1.2, egui::Color32::from_rgb(150, 180, 230)),
                                                );
                                            }
                                            for point in &points {
                                                let x = roll_rect.left() + self.piano_pan.x + point.beat * beat_width;
                                                let y = lane_rect.bottom() - point.value.clamp(0.0, 1.0) * lane_rect.height();
                                                lane_painter.circle_filled(
                                                    egui::pos2(x, y),
                                                    3.0,
                                                    egui::Color32::from_rgb(180, 200, 240),
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        if lane_response.hovered() {
                            if let Some(pos) = lane_response.interact_pointer_pos() {
                                if pos.x >= roll_rect.left() {
                                    match self.piano_lane_mode {
                                        PianoLaneMode::MidiCc => {
                                            if let Some(track_index) = self.selected_track {
                                                if let Some(track) = self.tracks.get_mut(track_index) {
                                                    let lane_index = track
                                                        .midi_cc_lanes
                                                        .iter()
                                                        .position(|lane| lane.cc == self.piano_cc)
                                                        .unwrap_or_else(|| {
                                                            track.midi_cc_lanes.push(MidiCcLane {
                                                                cc: self.piano_cc,
                                                                points: Vec::new(),
                                                            });
                                                            track.midi_cc_lanes.len() - 1
                                                        });
                                                    let lane = &mut track.midi_cc_lanes[lane_index];
                                                    let beat = (pos.x - roll_rect.left() - self.piano_pan.x)
                                                        / beat_width;
                                                    let value = (lane_rect.bottom() - pos.y)
                                                        / lane_rect.height();
                                                    let value = value.clamp(0.0, 1.0);

                                                    if lane_response.drag_started() || lane_response.clicked() {
                                                        let mut closest: Option<(usize, f32)> = None;
                                                        for (idx, point) in lane.points.iter().enumerate() {
                                                            let px = roll_rect.left()
                                                                + self.piano_pan.x
                                                                + point.beat * beat_width;
                                                            let py = lane_rect.bottom()
                                                                - point.value.clamp(0.0, 1.0) * lane_rect.height();
                                                            let dx = px - pos.x;
                                                            let dy = py - pos.y;
                                                            let dist = dx * dx + dy * dy;
                                                            if dist < 64.0 {
                                                                if closest.map_or(true, |(_, best)| dist < best) {
                                                                    closest = Some((idx, dist));
                                                                }
                                                            }
                                                        }
                                                        if let Some((idx, _)) = closest {
                                                            self.piano_cc_drag = Some(idx);
                                                        } else {
                                                            lane.points.push(AutomationPoint { beat, value });
                                                            self.piano_cc_drag = Some(lane.points.len() - 1);
                                                        }
                                                    }

                                                    if lane_response.dragged() {
                                                        if let Some(idx) = self.piano_cc_drag {
                                                            if let Some(point) = lane.points.get_mut(idx) {
                                                                point.beat = beat.max(0.0);
                                                                point.value = value;
                                                            }
                                                        }
                                                    }
                                                    if lane_response.drag_stopped()
                                                        || ctx.input(|i| i.pointer.any_released())
                                                    {
                                                        self.piano_cc_drag = None;
                                                    }
                                                }
                                            }
                                        }
                                        _ => {
                                            if self.selected_clip.is_some() {
                                                if let Some(track_index) = self.selected_track {
                                                    let beat = (pos.x - roll_rect.left() - self.piano_pan.x)
                                                        / beat_width;
                                                    if beat >= 0.0 {
                                                        if let Some(track) = self.tracks.get_mut(track_index) {
                                                            if let Some(note_index) = track.midi_notes.iter().position(|note| {
                                                                beat >= note.start_beats
                                                                    && beat <= note.start_beats + note.length_beats
                                                            }) {
                                                                let value = (lane_rect.bottom() - pos.y)
                                                                    / lane_rect.height();
                                                                let value = value.clamp(0.0, 1.0);
                                                                if let Some(note) = track.midi_notes.get_mut(note_index) {
                                                                    match self.piano_lane_mode {
                                                                        PianoLaneMode::Velocity => {
                                                                            note.velocity = (value * 127.0).round() as u8;
                                                                        }
                                                                        PianoLaneMode::Pan => {
                                                                            let pan = (lane_rect.center().y - pos.y)
                                                                                / (lane_rect.height() * 0.5);
                                                                            note.pan = pan.clamp(-1.0, 1.0);
                                                                        }
                                                                        PianoLaneMode::Cutoff => {
                                                                            note.cutoff = value;
                                                                        }
                                                                        PianoLaneMode::Resonance => {
                                                                            note.resonance = value;
                                                                        }
                                                                        PianoLaneMode::MidiCc => {}
                                                                    }
                                                                    self.sync_track_audio_notes(track_index);
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                });
    }

    fn modals(&mut self, ctx: &egui::Context) {
        if self.show_close_confirm {
            let mut open = self.show_close_confirm;
            let mut proceed_action: Option<ProjectAction> = None;
            let mut close_requested = false;
            let mut confirm_exit = false;
            egui::Window::new("Unsaved Changes")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("Current project has unsaved changes.");
                    ui.label("Save before continuing?");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::Button::image(
                                egui::Image::new(egui::include_image!("../../icons/save.svg"))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                            ))
                            .on_hover_text("Save & Continue")
                            .clicked()
                        {
                            match self.save_project_or_prompt() {
                                Ok(_) => {
                                    self.clear_dirty();
                                    proceed_action = self.pending_project_action.take();
                                    if self.pending_exit {
                                        confirm_exit = true;
                                    }
                                    close_requested = true;
                                }
                                Err(err) => {
                                    self.status = format!("Save failed: {err}");
                                }
                            }
                        }
                        if ui
                            .add(egui::Button::image(
                                egui::Image::new(egui::include_image!("../../icons/trash-2.svg"))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                            ))
                            .on_hover_text("Discard")
                            .clicked()
                        {
                            self.clear_dirty();
                            proceed_action = self.pending_project_action.take();
                            if self.pending_exit {
                                confirm_exit = true;
                            }
                            close_requested = true;
                        }
                        if ui
                            .add(egui::Button::image(
                                egui::Image::new(egui::include_image!("../../icons/x.svg"))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                            ))
                            .on_hover_text("Cancel")
                            .clicked()
                        {
                            self.pending_exit = false;
                            close_requested = true;
                        }
                    });
                });
            if close_requested {
                open = false;
            }
            if !open {
                self.show_close_confirm = false;
                self.pending_exit = false;
                if proceed_action.is_none() {
                    self.pending_project_action = None;
                }
            }
            if confirm_exit {
                self.pending_exit = false;
                self.exit_confirmed = true;
            }
            if let Some(action) = proceed_action {
                self.perform_project_action(action);
            }
        }

        if self.show_settings {
            let mut open = self.show_settings;
            egui::Window::new("Settings")
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.settings_tab, SettingsTab::Audio, "Audio");
                        ui.selectable_value(&mut self.settings_tab, SettingsTab::Midi, "MIDI");
                        ui.selectable_value(&mut self.settings_tab, SettingsTab::Theme, "Theme");
                    });
                    ui.separator();

                    match self.settings_tab {
                        SettingsTab::Audio => {
                            ui.heading("Audio");
                            ui.separator();
                            let devices = self.list_output_devices();
                            egui::ComboBox::from_label("Soundcard")
                                .selected_text(self.settings.output_device.clone())
                                .show_ui(ui, |ui| {
                                    for name in &devices {
                                        if ui
                                            .selectable_label(self.settings.output_device == *name, name)
                                            .clicked()
                                        {
                                            self.settings.output_device = name.to_string();
                                        }
                                    }
                                });
                            let inputs = self.list_input_devices();
                            egui::ComboBox::from_label("Input Device")
                                .selected_text(self.settings.input_device.clone())
                                .show_ui(ui, |ui| {
                                    for name in &inputs {
                                        if ui
                                            .selectable_label(self.settings.input_device == *name, name)
                                            .clicked()
                                        {
                                            self.settings.input_device = name.to_string();
                                        }
                                    }
                                });
                            ui.horizontal(|ui| {
                                ui.label("Buffer Size");
                                egui::ComboBox::from_id_source("buffer_size")
                                    .selected_text(format!("{}", self.settings.buffer_size))
                                    .show_ui(ui, |ui| {
                                        for size in [128u32, 256, 512, 1024, 2048] {
                                            if ui
                                                .selectable_label(
                                                    self.settings.buffer_size == size,
                                                    format!("{}", size),
                                                )
                                                .clicked()
                                            {
                                                self.settings.buffer_size = size;
                                            }
                                        }
                                    });
                            });
                            ui.horizontal(|ui| {
                                ui.label("Sample Rate");
                                egui::ComboBox::from_id_source("sample_rate")
                                    .selected_text(format!("{}", self.settings.sample_rate))
                                    .show_ui(ui, |ui| {
                                        for rate in [44_100u32, 48_000, 96_000] {
                                            if ui
                                                .selectable_label(
                                                    self.settings.sample_rate == rate,
                                                    format!("{}", rate),
                                                )
                                                .clicked()
                                            {
                                                self.settings.sample_rate = rate;
                                            }
                                        }
                                    });
                            });
                            ui.horizontal(|ui| {
                                ui.label("Interpolation");
                                egui::ComboBox::from_id_source("interpolation")
                                    .selected_text(self.settings.interpolation.clone())
                                    .show_ui(ui, |ui| {
                                        for mode in ["linear", "cubic", "sinc"] {
                                            if ui
                                                .selectable_label(self.settings.interpolation == mode, mode)
                                                .clicked()
                                            {
                                                self.settings.interpolation = mode.to_string();
                                            }
                                        }
                                    });
                            });
                            ui.checkbox(&mut self.settings.triple_buffer, "Triple buffer");
                            ui.checkbox(&mut self.settings.safe_underruns, "Safe underruns");
                            ui.checkbox(&mut self.settings.adaptive_buffer, "Adaptive buffer");
                            ui.checkbox(&mut self.settings.smart_disable_plugins, "Smart disable plugins");
                            ui.checkbox(&mut self.settings.smart_suspend_tracks, "Smart suspend tracks");
                        }
                        SettingsTab::Midi => {
                            ui.heading("MIDI");
                            ui.separator();
                            let midi_inputs = self.list_midi_inputs();
                            egui::ComboBox::from_label("MIDI Input")
                                .selected_text(self.settings.midi_input.clone())
                                .show_ui(ui, |ui| {
                                    for name in &midi_inputs {
                                        if ui
                                            .selectable_label(self.settings.midi_input == *name, name)
                                            .clicked()
                                        {
                                            self.settings.midi_input = name.to_string();
                                        }
                                    }
                                });
                        }
                        SettingsTab::Theme => {
                            ui.heading("Theme");
                            ui.separator();
                            ui.label("Color Scheme");
                            egui::ComboBox::from_id_source("theme_scheme")
                                .selected_text(self.settings.theme.clone())
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut self.settings.theme,
                                        "Black".to_string(),
                                        "Black (White on Black)",
                                    );
                                    ui.selectable_value(
                                        &mut self.settings.theme,
                                        "Dark".to_string(),
                                        "Dark Gray",
                                    );
                                });
                        }
                    }

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::Button::image(
                                egui::Image::new(egui::include_image!("../../icons/save.svg"))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                            ))
                            .on_hover_text("Save Settings")
                            .clicked()
                        {
                            if let Err(err) = self.save_settings() {
                                self.status = format!("Settings save failed: {err}");
                            } else {
                                self.status = "Settings saved".to_string();
                            }
                        }
                        if ui
                            .add(egui::Button::image(
                                egui::Image::new(egui::include_image!("../../icons/rotate-cw.svg"))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                            ))
                            .on_hover_text("Reload")
                            .clicked()
                        {
                            self.load_settings_or_default();
                            self.status = "Settings reloaded".to_string();
                        }
                    });
                });
            self.show_settings = open;
        }

        if self.show_plugin_picker {
            let mut open = self.show_plugin_picker;
            let mut chosen: Option<String> = None;
            let mut refresh = false;
            egui::Window::new("VST3 Plugin Picker")
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.label("Scanning: C:\\Program Files\\Common Files\\VST3");
                    ui.horizontal(|ui| {
                        ui.label("Search");
                        ui.text_edit_singleline(&mut self.plugin_search);
                        if ui
                            .add(egui::Button::image(
                                egui::Image::new(egui::include_image!("../../icons/refresh-cw.svg"))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                            ))
                            .on_hover_text("Refresh")
                            .clicked()
                        {
                            refresh = true;
                        }
                    });
                    ui.separator();

                    let search = self.plugin_search.to_ascii_lowercase();
                    egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                        for path in &self.plugin_candidates {
                            let display = Self::plugin_display_name(path);
                            if !search.is_empty()
                                && !path.to_ascii_lowercase().contains(&search)
                                && !display.to_ascii_lowercase().contains(&search)
                            {
                                continue;
                            }
                            if ui.selectable_label(false, display).clicked() {
                                chosen = Some(path.clone());
                            }
                        }
                    });
                });

            if refresh {
                self.plugin_candidates = self.scan_vst3_plugins();
            }

            if let Some(path) = chosen {
                if let Some(target) = self.plugin_target {
                    match target {
                        PluginTarget::Instrument(index) => {
                            self.replace_instrument(index, path);
                        }
                        PluginTarget::Effect(index) => {
                            let was_running = self.audio_running;
                            if was_running {
                                self.stop_audio_and_midi();
                            }
                            if let Some(track) = self.tracks.get_mut(index) {
                                track.effect_paths.push(path);
                                track.effect_bypass.push(false);
                                track.effect_params.push(Vec::new());
                                track.effect_param_ids.push(Vec::new());
                                track.effect_param_values.push(Vec::new());
                            }
                            if let Some(state) = self.track_audio.get_mut(index) {
                                for host in state.effect_hosts.drain(..) {
                                    if let Ok(mut host) = host.lock() {
                                        host.prepare_for_drop();
                                    }
                                    self.orphaned_hosts.push(host);
                                }
                            }
                            if was_running {
                                if let Err(err) = self.start_audio_and_midi() {
                                    self.status = format!("Audio restart failed: {err}");
                                } else {
                                    self.status = "Audio restarted for new VST3".to_string();
                                }
                            }
                            self.refresh_params_for_selected_track(true);
                        }
                    }
                }
                open = false;
            }

            self.show_plugin_picker = open;
            if !open {
                self.plugin_target = None;
            }
        }

        if self.show_midi_import {
            let mut open = self.show_midi_import;
            let mut do_import = false;
            let mut close_requested = false;
            if let Some(state) = self.midi_import_state.as_mut() {
                egui::Window::new("Import MIDI")
                    .open(&mut open)
                    .default_size(egui::vec2(520.0, 420.0))
                    .show(ctx, |ui| {
                        let file_label = Path::new(&state.path)
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or(state.path.as_str());
                        ui.label(format!("File: {file_label}"));
                        ui.separator();
                        ui.label("Tracks");
                        ui.horizontal(|ui| {
                            if ui
                                .add(egui::Button::image(
                                    egui::Image::new(egui::include_image!("../../icons/check-square.svg"))
                                        .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                ))
                                .on_hover_text("All")
                                .clicked()
                            {
                                for enabled in &mut state.enabled {
                                    *enabled = true;
                                }
                            }
                            if ui
                                .add(egui::Button::image(
                                    egui::Image::new(egui::include_image!("../../icons/x-square.svg"))
                                        .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                ))
                                .on_hover_text("None")
                                .clicked()
                            {
                                for enabled in &mut state.enabled {
                                    *enabled = false;
                                }
                            }
                        });
                        egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                            for (index, track_data) in state.tracks.iter().enumerate() {
                                let enabled = state.enabled.get_mut(index).unwrap();
                                let apply_program = state.apply_program.get_mut(index).unwrap();
                                let track_name = match track_data.program {
                                    Some(program) if track_data.has_drums => gm_drum_kit_name(program)
                                        .unwrap_or("Drum Kit")
                                        .to_string(),
                                    Some(program) => gm_program_name(program).to_string(),
                                    None => format!("Track {}", track_data.track_index + 1),
                                };
                                let label = format!("Track {} - {}", track_data.track_index + 1, track_name);
                                ui.horizontal(|ui| {
                                    ui.checkbox(enabled, label);
                                    ui.add_enabled_ui(track_data.program.is_some(), |ui| {
                                        ui.checkbox(apply_program, "Use patch");
                                    });
                                });
                            }
                        });

                        ui.separator();
                        let instrument_options = [
                            "None",
                            "MiceSynth",
                            "FishSynth",
                            "SannySynth",
                            "LingSynth",
                            "DogSynth",
                        ];
                        egui::ComboBox::from_label("Instrument Plugin")
                            .selected_text(state.instrument_plugin.clone())
                            .show_ui(ui, |ui| {
                                for name in instrument_options {
                                    if ui.selectable_label(state.instrument_plugin == name, name).clicked() {
                                        state.instrument_plugin = name.to_string();
                                    }
                                }
                            });
                        let percussion_options = ["None", "Catsynth"]; 
                        egui::ComboBox::from_label("Percussion Plugin")
                            .selected_text(state.percussion_plugin.clone())
                            .show_ui(ui, |ui| {
                                for name in percussion_options {
                                    if ui.selectable_label(state.percussion_plugin == name, name).clicked() {
                                        state.percussion_plugin = name.to_string();
                                    }
                                }
                            });
                        ui.checkbox(&mut state.import_portamento, "Import Portamento (CC65)");
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui
                                .add(egui::Button::image(
                                    egui::Image::new(egui::include_image!("../../icons/download.svg"))
                                        .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                ))
                                .on_hover_text("Import")
                                .clicked()
                            {
                                do_import = true;
                            }
                            if ui
                                .add(egui::Button::image(
                                    egui::Image::new(egui::include_image!("../../icons/x.svg"))
                                        .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                ))
                                .on_hover_text("Cancel")
                                .clicked()
                            {
                                close_requested = true;
                            }
                        });
                    });
            } else {
                open = false;
            }
            if close_requested {
                open = false;
            }
            self.show_midi_import = open;
            if do_import {
                if let Err(err) = self.apply_midi_import() {
                    self.status = format!("MIDI import failed: {err}");
                }
            }
            if !open {
                self.midi_import_state = None;
            }
        }

        if self.show_rename_track {
            let mut open = self.show_rename_track;
            let mut close_requested = false;
            egui::Window::new("Rename Track")
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.label("Track Name");
                    ui.text_edit_singleline(&mut self.rename_buffer);
                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::Button::image(
                                egui::Image::new(egui::include_image!("../../icons/check.svg"))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                            ))
                            .on_hover_text("Apply")
                            .clicked()
                        {
                            self.apply_rename();
                            close_requested = true;
                        }
                        if ui
                            .add(egui::Button::image(
                                egui::Image::new(egui::include_image!("../../icons/x.svg"))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                            ))
                            .on_hover_text("Cancel")
                            .clicked()
                        {
                            close_requested = true;
                        }
                    });
                });
            if close_requested {
                open = false;
            }
            self.show_rename_track = open;
        }

        if self.show_rename_project {
            let mut open = self.show_rename_project;
            let mut close_requested = false;
            egui::Window::new("Rename Project")
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.label("Project Name");
                    ui.text_edit_singleline(&mut self.project_name_buffer);
                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::Button::image(
                                egui::Image::new(egui::include_image!("../../icons/check.svg"))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                            ))
                            .on_hover_text("Apply")
                            .clicked()
                        {
                            self.apply_rename_project();
                            close_requested = true;
                        }
                        if ui
                            .add(egui::Button::image(
                                egui::Image::new(egui::include_image!("../../icons/x.svg"))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                            ))
                            .on_hover_text("Cancel")
                            .clicked()
                        {
                            close_requested = true;
                        }
                    });
                });
            if close_requested {
                open = false;
            }
            self.show_rename_project = open;
        }

        if self.show_project_info {
            let mut open = self.show_project_info;
            egui::Window::new("Project Info")
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.label(format!("Project: {}", self.project_name));
                    ui.label(format!("Tempo: {} BPM", self.tempo_bpm));
                    ui.label("Time Signature: 4/4");
                    ui.label("Sample Rate: 48 kHz");
                });
            self.show_project_info = open;
        }

        if self.show_metadata {
            let mut open = self.show_metadata;
            egui::Window::new("Metadata")
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.label("Artist");
                    ui.text_edit_singleline(&mut self.metadata_artist);
                    ui.label("Title");
                    ui.text_edit_singleline(&mut self.metadata_title);
                    ui.label("Album");
                    ui.text_edit_singleline(&mut self.metadata_album);
                    ui.label("Genre");
                    ui.text_edit_singleline(&mut self.metadata_genre);
                    ui.label("Year");
                    ui.text_edit_singleline(&mut self.metadata_year);
                    ui.label("Comment");
                    ui.text_edit_multiline(&mut self.metadata_comment);
                });
            self.show_metadata = open;
        }
    }

    fn new_project(&mut self) {
        self.prepare_for_project_change();

        self.project_name = "Untitled Project".to_string();
        self.project_path = String::new();
        self.metadata_artist.clear();
        self.metadata_title.clear();
        self.metadata_album.clear();
        self.metadata_genre.clear();
        self.metadata_year.clear();
        self.metadata_comment.clear();
        self.tracks = vec![Track {
            name: "Track 1".to_string(),
            clips: Vec::new(),
            level: 0.8,
            muted: false,
            solo: false,
            midi_notes: Vec::new(),
            instrument_path: None,
            effect_paths: Vec::new(),
            effect_bypass: Vec::new(),
            effect_params: Vec::new(),
            effect_param_ids: Vec::new(),
            effect_param_values: Vec::new(),
            params: default_midi_params(),
            param_ids: Vec::new(),
            param_values: Vec::new(),
            plugin_state_component: None,
            plugin_state_controller: None,
            automation_lanes: Vec::new(),
            automation_channels: Vec::new(),
            midi_cc_lanes: Vec::new(),
            midi_program: None,
        }];
        self.selected_clip = None;
        self.selected_track = Some(0);
        self.playhead_beats = 0.0;
        self.sync_track_audio_states();
        self.clear_dirty();
        self.status = "New project".to_string();
    }

    fn prepare_for_project_change(&mut self) {
        if self.is_recording {
            let _ = self.end_recording();
        }
        self.stop_audio_preview();
        self.plugin_ui_resume_at = None;
        self.show_plugin_ui = false;
        let plugin_hwnd = self.plugin_ui.as_ref().map(|ui_host| ui_host.hwnd);
        if let Some(ui_host) = self.plugin_ui.as_ref() {
            ui_host.editor.set_focus(false);
            hide_plugin_window(ui_host.hwnd);
            release_mouse_capture();
        }
        self.destroy_plugin_ui();
        if let Some(hwnd) = plugin_hwnd {
            pump_plugin_messages(hwnd);
        }
        self.plugin_ui_hidden = false;
        if self.audio_running {
            self.stop_audio_and_midi();
        }
        let mut hosts: Vec<Arc<Mutex<vst3::Vst3Host>>> = Vec::new();
        for state in self.track_audio.iter_mut() {
            if let Some(host) = state.host.take() {
                if let Ok(mut host) = host.lock() {
                    host.prepare_for_drop();
                }
                hosts.push(host);
            }
            for host in state.effect_hosts.drain(..) {
                if let Ok(mut host) = host.lock() {
                    host.prepare_for_drop();
                }
                hosts.push(host);
            }
        }
        self.orphaned_hosts.extend(hosts);
        self.track_audio.clear();
    }

    fn add_track(&mut self) {
        let index = self.tracks.len() + 1;
        self.tracks.push(Track {
            name: format!("Track {}", index),
            clips: Vec::new(),
            level: 0.8,
            muted: false,
            solo: false,
            midi_notes: Vec::new(),
            instrument_path: None,
            effect_paths: Vec::new(),
            effect_bypass: Vec::new(),
            effect_params: Vec::new(),
            effect_param_ids: Vec::new(),
            effect_param_values: Vec::new(),
            params: default_midi_params(),
            param_ids: Vec::new(),
            param_values: Vec::new(),
            plugin_state_component: None,
            plugin_state_controller: None,
            automation_lanes: Vec::new(),
            automation_channels: Vec::new(),
            midi_cc_lanes: Vec::new(),
            midi_program: None,
        });
        self.selected_track = Some(self.tracks.len().saturating_sub(1));
        self.refresh_params_for_selected_track(true);
        if let Some(track) = self.tracks.last() {
            self.track_audio.push(TrackAudioState::from_track(track));
        }
        self.sync_track_mix();
        self.mark_dirty();
        self.status = "Track added".to_string();
    }

    fn remove_selected_track(&mut self) {
        if let Some(index) = self.selected_track {
            if self.tracks.len() > 1 {
                if self
                    .plugin_ui
                    .as_ref()
                    .map(|ui| matches!(ui.target, PluginUiTarget::Instrument(ti) | PluginUiTarget::Effect(ti, _) if ti == index))
                    .unwrap_or(false)
                {
                    self.show_plugin_ui = false;
                    self.destroy_plugin_ui();
                }
                self.tracks.remove(index);
                if index < self.track_audio.len() {
                    let mut state = self.track_audio.remove(index);
                    if let Some(host) = state.host.take() {
                        if let Ok(mut host) = host.lock() {
                            host.prepare_for_drop();
                        }
                        self.orphaned_hosts.push(host);
                    }
                    for host in state.effect_hosts.drain(..) {
                        if let Ok(mut host) = host.lock() {
                            host.prepare_for_drop();
                        }
                        self.orphaned_hosts.push(host);
                    }
                }
                let next = index.saturating_sub(1).min(self.tracks.len().saturating_sub(1));
                self.selected_track = Some(next);
                self.sync_track_mix();
                self.mark_dirty();
                self.status = "Track removed".to_string();
            } else {
                self.status = "At least one track required".to_string();
            }
        }
    }

    fn duplicate_selected_track(&mut self) {
        if let Some(index) = self.selected_track {
            if let Some(track) = self.tracks.get(index).cloned() {
                let mut dup = track.clone();
                dup.name = format!("{} Copy", track.name);
                self.tracks.insert(index + 1, dup);
                let state = TrackAudioState::from_track(&track);
                self.track_audio.insert(index + 1, state);
                self.selected_track = Some(index + 1);
                self.sync_track_mix();
                self.mark_dirty();
                self.status = "Track duplicated".to_string();
            }
        }
    }

    fn clone_selected_track(&mut self) {
        if let Some(index) = self.selected_track {
            if let Some(track) = self.tracks.get(index).cloned() {
                let mut clone = track.clone();
                clone.clips.clear();
                clone.name = format!("{} Clone", clone.name);
                self.tracks.insert(index + 1, clone);
                let state = TrackAudioState::from_track(&track);
                self.track_audio.insert(index + 1, state);
                self.selected_track = Some(index + 1);
                self.sync_track_mix();
                self.mark_dirty();
                self.status = "Track cloned".to_string();
            }
        }
    }

    fn begin_rename_selected_track(&mut self) {
        if let Some(index) = self.selected_track {
            if let Some(track) = self.tracks.get(index) {
                self.rename_buffer = track.name.clone();
                self.show_rename_track = true;
            }
        }
    }

    fn apply_rename(&mut self) {
        if let Some(index) = self.selected_track {
            if let Some(track) = self.tracks.get_mut(index) {
                let name = self.rename_buffer.trim();
                if !name.is_empty() {
                    track.name = name.to_string();
                    self.mark_dirty();
                    self.status = "Track renamed".to_string();
                }
            }
        }
    }

    fn capture_plugin_states(&mut self) {
        for (index, track) in self.tracks.iter_mut().enumerate() {
            let Some(state) = self.track_audio.get(index) else {
                continue;
            };
            let Some(host) = state.host.as_ref() else {
                continue;
            };
            let Ok(host) = host.lock() else {
                continue;
            };
            let (component, controller) = host.get_state_bytes();
            track.plugin_state_component = if component.is_empty() { None } else { Some(component) };
            track.plugin_state_controller = if controller.is_empty() { None } else { Some(controller) };
        }
    }

    fn save_project(&mut self) -> Result<(), String> {
        if self.project_path.trim().is_empty() {
            if let Some(folder) = self.default_project_dir() {
                return self.save_project_to_folder(&folder);
            }
            return Err("Default project folder unavailable".to_string());
        }
        let path = self.project_path.clone();
        self.save_project_to_folder(Path::new(&path))
    }

    fn save_project_to_folder(&mut self, folder: &Path) -> Result<(), String> {
        self.capture_plugin_states();
        let state = ProjectState {
            name: self.project_name.clone(),
            artist: self.metadata_artist.clone(),
            title: self.metadata_title.clone(),
            album: self.metadata_album.clone(),
            genre: self.metadata_genre.clone(),
            year: self.metadata_year.clone(),
            comment: self.metadata_comment.clone(),
            tempo_bpm: self.tempo_bpm,
            tracks: self.tracks.clone(),
        };
        let folder = Self::normalize_windows_path(folder);
        fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        let midi_dir = folder.join("midi");
        let samples_dir = folder.join("samples");
        let audio_dir = folder.join("audio");
        let renders_dir = folder.join("renders");
        fs::create_dir_all(&midi_dir).map_err(|e| e.to_string())?;
        fs::create_dir_all(&samples_dir).map_err(|e| e.to_string())?;
        fs::create_dir_all(&audio_dir).map_err(|e| e.to_string())?;
        fs::create_dir_all(&renders_dir).map_err(|e| e.to_string())?;

        let json = serde_json::to_string_pretty(&state).map_err(|e| e.to_string())?;
        let manifest_path = folder.join("project.json");
        fs::write(&manifest_path, json).map_err(|e| e.to_string())?;

        for (index, track) in self.tracks.iter().enumerate() {
            let safe_track = Self::sanitize_folder_name(&track.name);
            let mut wrote_clip = false;
            for clip in &track.clips {
                if !clip.is_midi {
                    continue;
                }
                let clip_start = clip.start_beats;
                let clip_end = clip.start_beats + clip.length_beats;
                let mut notes = Vec::new();
                for note in &track.midi_notes {
                    let note_end = note.start_beats + note.length_beats;
                    if note_end < clip_start || note.start_beats > clip_end {
                        continue;
                    }
                    let mut adjusted = note.clone();
                    adjusted.start_beats = (adjusted.start_beats - clip_start).max(0.0);
                    notes.push(adjusted);
                }
                if notes.is_empty() {
                    continue;
                }
                let safe_clip = Self::sanitize_folder_name(&clip.name);
                let file_name = if safe_clip.is_empty() {
                    format!("{:02}_{}_clip{}.mid", index + 1, safe_track, clip.id)
                } else {
                    format!("{:02}_{}_{}.mid", index + 1, safe_track, safe_clip)
                };
                let midi_path = midi_dir.join(file_name);
                export_midi(midi_path.to_string_lossy().as_ref(), &notes, 480)?;
                wrote_clip = true;
            }

            if !wrote_clip && !track.midi_notes.is_empty() {
                let file_name = format!("{:02}_{}.mid", index + 1, safe_track);
                let midi_path = midi_dir.join(file_name);
                export_midi(midi_path.to_string_lossy().as_ref(), &track.midi_notes, 480)?;
            }
        }

        self.project_path = folder.to_string_lossy().to_string();
        if self.project_name.trim().is_empty() {
            if let Some(name) = self.project_name_from_path() {
                self.project_name = name;
            }
        }
        self.clear_dirty();
        self.status = format!("Saved {}", self.project_path);
        Ok(())
    }

    fn load_project(&mut self) -> Result<(), String> {
        let path = self.project_path.clone();
        self.load_project_from_folder(Path::new(&path))
    }

    fn load_project_from_folder(&mut self, folder: &Path) -> Result<(), String> {
        self.prepare_for_project_change();
        let manifest_path = folder.join("project.json");
        let data = fs::read_to_string(&manifest_path).map_err(|e| e.to_string())?;
        let state: ProjectState = serde_json::from_str(&data).map_err(|e| e.to_string())?;
        self.project_name = state.name;
        self.metadata_artist = state.artist;
        self.metadata_title = state.title;
        self.metadata_album = state.album;
        self.metadata_genre = state.genre;
        self.metadata_year = state.year;
        self.metadata_comment = state.comment;
        self.tempo_bpm = state.tempo_bpm;
        self.tracks = state.tracks;
        self.selected_clip = None;
        self.project_path = folder.to_string_lossy().to_string();
        self.load_midi_from_folder(folder)?;
        self.sync_track_audio_states();
        if self.project_name.trim().is_empty() {
            if let Some(name) = self.project_name_from_path() {
                self.project_name = name;
            }
        }
        self.clear_dirty();
        self.status = format!("Loaded {}", self.project_path);
        Ok(())
    }

    fn open_project_dialog(&mut self) -> Result<(), String> {
        let folder = rfd::FileDialog::new().pick_folder();
        if let Some(folder) = folder {
            return self.load_project_from_folder(&folder);
        }
        Ok(())
    }

    fn save_project_dialog(&mut self) -> Result<(), String> {
        let folder = rfd::FileDialog::new().pick_folder();
        if let Some(folder) = folder {
            return self.save_project_to_folder(&folder);
        }
        Ok(())
    }

    fn begin_rename_project(&mut self) {
        self.project_name_buffer = self.project_name.clone();
        self.show_rename_project = true;
    }

    fn apply_rename_project(&mut self) {
        let name = self.project_name_buffer.trim();
        if !name.is_empty() {
            self.project_name = name.to_string();
            self.mark_dirty();
            self.status = "Project renamed".to_string();
        }
    }

    fn project_name_from_path(&self) -> Option<String> {
        let path = Path::new(&self.project_path);
        path.file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.replace('_', " "))
    }

    fn save_project_or_prompt(&mut self) -> Result<(), String> {
        if self.project_path.trim().is_empty() {
            if let Some(folder) = self.default_project_dir() {
                return self.save_project_to_folder(&folder);
            }
            return Err("Default project folder unavailable".to_string());
        }
        self.save_project()
    }

    fn sanitize_folder_name(name: &str) -> String {
        let mut cleaned = String::new();
        for ch in name.chars() {
            let safe = match ch {
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
                _ => ch,
            };
            cleaned.push(safe);
        }
        let trimmed = cleaned.trim();
        if trimmed.is_empty() {
            "LingStationProject".to_string()
        } else {
            trimmed.to_string()
        }
    }

    fn render_base_name(&self) -> String {
        let artist = self.metadata_artist.trim();
        let title = self.metadata_title.trim();
        let base = if !artist.is_empty() && !title.is_empty() {
            format!("{} - {}", artist, title)
        } else if !title.is_empty() {
            title.to_string()
        } else if !artist.is_empty() {
            artist.to_string()
        } else if !self.project_name.trim().is_empty() {
            self.project_name.clone()
        } else {
            "render".to_string()
        };
        Self::sanitize_folder_name(&base)
    }

    fn note_icon_source(value: f32) -> egui::ImageSource<'static> {
        if (value - 1.0 / 32.0).abs() < f32::EPSILON {
            egui::include_image!("../../icons/note-thirtysecond.svg")
        } else if (value - 1.0 / 16.0).abs() < f32::EPSILON {
            egui::include_image!("../../icons/note-sixteenth.svg")
        } else if (value - 1.0 / 8.0).abs() < f32::EPSILON {
            egui::include_image!("../../icons/note-eighth.svg")
        } else if (value - 1.0 / 4.0).abs() < f32::EPSILON {
            egui::include_image!("../../icons/note-quarter.svg")
        } else if (value - 1.0 / 2.0).abs() < f32::EPSILON {
            egui::include_image!("../../icons/note-half.svg")
        } else {
            egui::include_image!("../../icons/note-whole.svg")
        }
    }

    fn ensure_project_folder(&mut self) -> Result<PathBuf, String> {
        if self.project_path.trim().is_empty() {
            let folder = self
                .default_project_dir()
                .ok_or_else(|| "Default project folder unavailable".to_string())?;
            fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
            self.project_path = folder.to_string_lossy().to_string();
            return Ok(folder);
        }
        let folder = PathBuf::from(self.project_path.trim());
        fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        Ok(folder)
    }

    fn add_audio_clip_from_path(
        &mut self,
        track_index: usize,
        start_beats: f32,
        source: &Path,
    ) -> Result<(), String> {
        if track_index >= self.tracks.len() {
            return Err("Invalid track for dropped clip".to_string());
        }
        let project_folder = self.ensure_project_folder()?;
        let audio_dir = project_folder.join("audio");
        fs::create_dir_all(&audio_dir).map_err(|e| e.to_string())?;

        let _file_name = source
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| "Invalid file name".to_string())?;
        let stem = source
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Audio");
        let ext = source.extension().and_then(|s| s.to_str()).unwrap_or("wav");
        let safe_stem = Self::sanitize_folder_name(stem);
        let mut target = audio_dir.join(format!("{}.{}", safe_stem, ext));
        let mut counter = 1;
        while target.exists() {
            target = audio_dir.join(format!("{}_{}.{}", safe_stem, counter, ext));
            counter += 1;
        }
        fs::copy(source, &target).map_err(|e| e.to_string())?;

        let source_beats = if ext.eq_ignore_ascii_case("wav") {
            Self::wav_length_beats(&target, self.tempo_bpm)
        } else {
            None
        };
        let clip_len = source_beats.unwrap_or(4.0).max(0.25);

        let clip_id = self.next_clip_id();
        if let Some(track) = self.tracks.get_mut(track_index) {
            track.clips.push(Clip {
                id: clip_id,
                track: track_index,
                start_beats: start_beats.max(0.0),
                length_beats: clip_len,
                is_midi: false,
                name: safe_stem.clone(),
                audio_path: Some(format!("audio/{}", target.file_name().unwrap().to_string_lossy())),
                audio_source_beats: source_beats,
                audio_offset_beats: 0.0,
                audio_gain: 1.0,
                audio_pitch_semitones: 0.0,
                audio_time_mul: 1.0,
            });
        }
        self.selected_track = Some(track_index);
        self.selected_clip = Some(clip_id);
        Ok(())
    }

    fn wav_length_beats(path: &Path, tempo_bpm: f32) -> Option<f32> {
        let reader = hound::WavReader::open(path).ok()?;
        let spec = reader.spec();
        let samples = reader.duration() as f32;
        let channels = spec.channels.max(1) as f32;
        if spec.sample_rate == 0 {
            return None;
        }
        let frames = samples / channels;
        let seconds = frames / spec.sample_rate as f32;
        let beats = seconds * tempo_bpm.max(1.0) / 60.0;
        Some(beats.max(0.0))
    }

    fn wav_length_seconds(path: &Path) -> Option<f32> {
        if path
            .extension()
            .and_then(|s| s.to_str())
            .map(|e| !e.eq_ignore_ascii_case("wav"))
            .unwrap_or(true)
        {
            return None;
        }
        let reader = hound::WavReader::open(path).ok()?;
        let spec = reader.spec();
        let samples = reader.duration() as f32;
        let channels = spec.channels.max(1) as f32;
        if spec.sample_rate == 0 {
            return None;
        }
        let frames = samples / channels;
        let seconds = frames / spec.sample_rate as f32;
        Some(seconds.max(0.0))
    }

    fn load_midi_from_folder(&mut self, folder: &Path) -> Result<(), String> {
        let midi_dir = folder.join("midi");
        if !midi_dir.exists() {
            return Ok(());
        }
        let mut entries: Vec<PathBuf> = fs::read_dir(&midi_dir)
            .map_err(|e| e.to_string())?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .and_then(|s| s.to_str())
                    .map(|ext| ext.eq_ignore_ascii_case("mid") || ext.eq_ignore_ascii_case("midi"))
                    .unwrap_or(false)
            })
            .collect();
        entries.sort();

        for path in entries {
            let file_name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            let Some(track_index) = Self::track_index_from_filename(file_name) else {
                continue;
            };
            if track_index >= self.tracks.len() {
                continue;
            }
            let path_str = path.to_string_lossy().to_string();
            let channels = import_midi_channels(&path_str)?;
            let notes = channels
                .into_iter()
                .find(|c| !c.notes.is_empty())
                .map(|c| c.notes)
                .unwrap_or_default();
            if notes.is_empty() {
                continue;
            }
            let next_clip_id = self.next_clip_id();
            if let Some(track) = self.tracks.get_mut(track_index) {
                track.midi_notes = notes;
                let clip_name = Path::new(file_name)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("MIDI")
                    .replace('_', " ");
                if track.clips.is_empty() {
                    let max_end: f32 = track
                        .midi_notes
                        .iter()
                        .map(|n| n.start_beats + n.length_beats)
                        .fold(1.0, |a, b| a.max(b));
                    track.clips.push(Clip {
                        id: next_clip_id,
                        track: track_index,
                        start_beats: 0.0,
                        length_beats: max_end.max(1.0),
                        is_midi: true,
                        name: clip_name,
                        audio_path: None,
                        audio_source_beats: None,
                        audio_offset_beats: 0.0,
                        audio_gain: 1.0,
                        audio_pitch_semitones: 0.0,
                        audio_time_mul: 1.0,
                    });
                } else if track.clips[0].name.trim().is_empty() {
                    track.clips[0].name = clip_name;
                }
            }
        }

        Ok(())
    }

    fn track_index_from_filename(file_name: &str) -> Option<usize> {
        let mut digits = String::new();
        for ch in file_name.chars() {
            if ch.is_ascii_digit() {
                digits.push(ch);
            } else {
                break;
            }
        }
        if digits.is_empty() {
            return None;
        }
        let index: usize = digits.parse().ok()?;
        index.checked_sub(1)
    }

    fn import_midi_dialog(&mut self) -> Result<(), String> {
        let path = rfd::FileDialog::new()
            .add_filter("MIDI", &["mid", "midi"])
            .pick_file();
        if let Some(path) = path {
            let path_str = path.to_string_lossy().to_string();
            self.begin_midi_import(path_str)?;
        }
        Ok(())
    }

    fn begin_midi_import(&mut self, path_str: String) -> Result<(), String> {
        let tracks = import_midi_tracks(&path_str)?;
        if tracks.is_empty() {
            self.status = "No MIDI tracks found".to_string();
            return Ok(());
        }
        let enabled = vec![true; tracks.len()];
        let apply_program = tracks.iter().map(|t| t.program.is_some()).collect();
        self.midi_import_state = Some(MidiImportState {
            path: path_str,
            tracks,
            enabled,
            apply_program,
            instrument_plugin: "FishSynth".to_string(),
            percussion_plugin: "Catsynth".to_string(),
            import_portamento: true,
        });
        self.show_midi_import = true;
        Ok(())
    }

    fn apply_midi_import(&mut self) -> Result<(), String> {
        let Some(state) = self.midi_import_state.take() else {
            return Ok(());
        };
        let was_running = self.audio_running;
        self.prepare_for_project_change();

        let mut next_id = self.next_clip_id();
        let mut tracks = Vec::new();
        let mut missing_plugins: HashSet<String> = HashSet::new();
        for (index, track_data) in state.tracks.iter().enumerate() {
            if !state.enabled.get(index).copied().unwrap_or(true) {
                continue;
            }
            if track_data.notes.is_empty() {
                continue;
            }
            let is_drums = track_data.has_drums;
            let plugin_name = if is_drums {
                state.percussion_plugin.as_str()
            } else {
                state.instrument_plugin.as_str()
            };
            let instrument_path = if plugin_name == "None" {
                None
            } else {
                let path = self.find_vst3_plugin_by_name(plugin_name);
                if path.is_none() {
                    missing_plugins.insert(plugin_name.to_string());
                }
                path
            };
            let params = if instrument_path.is_some() {
                default_instrument_params()
            } else {
                default_midi_params()
            };
            let max_end: f32 = track_data
                .notes
                .iter()
                .map(|n| n.start_beats + n.length_beats)
                .fold(1.0f32, |a, b| a.max(b));
            let clip = Clip {
                id: next_id,
                track: tracks.len(),
                start_beats: 0.0,
                length_beats: max_end.max(1.0),
                is_midi: true,
                name: format!("Track {}", track_data.track_index + 1),
                audio_path: None,
                audio_source_beats: None,
                audio_offset_beats: 0.0,
                audio_gain: 1.0,
                audio_pitch_semitones: 0.0,
                audio_time_mul: 1.0,
            };
            next_id += 1;
            let track_name = match track_data.program {
                Some(program) if is_drums => gm_drum_kit_name(program)
                    .unwrap_or("Drum Kit")
                    .to_string(),
                Some(program) => gm_program_name(program).to_string(),
                None => format!("Track {}", track_data.track_index + 1),
            };
            let mut midi_cc_lanes = Vec::new();
            if state.import_portamento {
                let mut points = Vec::new();
                for event in &track_data.cc_events {
                    if event.cc == 65 {
                        points.push(AutomationPoint {
                            beat: event.beat,
                            value: event.value,
                        });
                    }
                }
                if !points.is_empty() {
                    points.sort_by(|a, b| a.beat.partial_cmp(&b.beat).unwrap());
                    midi_cc_lanes.push(MidiCcLane { cc: 65, points });
                }
            }
            let midi_program = if state.apply_program.get(index).copied().unwrap_or(true) {
                track_data.program
            } else {
                None
            };
            tracks.push(Track {
                name: track_name,
                clips: vec![clip],
                level: 0.8,
                muted: false,
                solo: false,
                midi_notes: track_data.notes.clone(),
                instrument_path,
                effect_paths: Vec::new(),
                effect_bypass: Vec::new(),
                effect_params: Vec::new(),
                effect_param_ids: Vec::new(),
                effect_param_values: Vec::new(),
                params,
                param_ids: Vec::new(),
                param_values: Vec::new(),
                plugin_state_component: None,
                plugin_state_controller: None,
                automation_lanes: Vec::new(),
                automation_channels: Vec::new(),
                midi_cc_lanes,
                midi_program,
            });
        }

        if tracks.is_empty() {
            self.status = "No MIDI tracks imported".to_string();
        } else {
            self.tracks = tracks;
            self.selected_track = Some(0);
            self.selected_clip = self.tracks.get(0).and_then(|t| t.clips.first()).map(|c| c.id);
            self.sync_track_audio_states();
            for index in 0..self.tracks.len() {
                let mut should_refresh = false;
                if let Some(track) = self.tracks.get(index) {
                    if track.instrument_path.is_some() && track.midi_program.is_some() {
                        should_refresh = true;
                    }
                }
                if should_refresh {
                    self.refresh_track_params(index);
                }
            }
            if missing_plugins.is_empty() {
                self.status = "MIDI imported".to_string();
            } else {
                let mut missing: Vec<String> = missing_plugins.into_iter().collect();
                missing.sort();
                self.status = format!("MIDI imported (missing: {})", missing.join(", "));
            }
        }
        self.import_path = state.path;
        self.show_midi_import = false;
        self.mark_dirty();
        if was_running {
            if let Err(err) = self.start_audio_and_midi() {
                self.status = format!("Audio restart failed: {err}");
            }
        }
        Ok(())
    }

    fn export_midi_dialog(&mut self) -> Result<(), String> {
        let path = rfd::FileDialog::new()
            .add_filter("MIDI", &["mid", "midi"])
            .set_file_name("export.mid")
            .save_file();
        if let Some(path) = path {
            let path_str = path.to_string_lossy().to_string();
            let notes = self
                .selected_track
                .and_then(|index| self.tracks.get(index))
                .map(|track| track.midi_notes.as_slice())
                .unwrap_or(&[]);
            export_midi(&path_str, notes, 480)?;
            self.export_path = path_str;
            self.status = "MIDI exported".to_string();
        }
        Ok(())
    }

    fn render_dialog(&mut self, format: RenderFormat) -> Result<(), String> {
        let (_label, ext) = match format {
            RenderFormat::Wav => ("WAV", "wav"),
            RenderFormat::Ogg => ("OGG", "ogg"),
            RenderFormat::Flac => ("FLAC", "flac"),
        };
        let default_name = format!("{}.{}", self.render_base_name(), ext);
        let mut dialog = rfd::FileDialog::new();
        if let Some(dir) = self.default_render_dir() {
            dialog = dialog.set_directory(dir);
        }
        let folder = dialog.pick_folder();
        if let Some(folder) = folder {
            let folder = Self::normalize_windows_path(&folder);
            if let Err(err) = fs::create_dir_all(&folder) {
                return Err(format!("Render folder create failed: {err}"));
            }
            let path = folder.join(default_name);
            if let Some(parent) = path.parent() {
                if let Err(err) = fs::create_dir_all(parent) {
                    return Err(format!("Render folder create failed: {err}"));
                }
            }
            let path = Self::normalize_windows_path(&path);
            let path_str = path.to_string_lossy().to_string();
            if let Err(err) = std::fs::File::create(&path) {
                return Err(format!("Render file create failed: {err} ({path_str})"));
            }
            match format {
                RenderFormat::Wav => {
                    self.render_to_wav(&path_str)
                        .map_err(|err| format!("{err} ({path_str})"))?;
                    self.status = format!("Rendered WAV: {}", path_str);
                }
                RenderFormat::Ogg => {
                    self.status = "OGG render uses the Render window".to_string();
                    return Err("Use the Render window for OGG".to_string());
                }
                RenderFormat::Flac => {
                    self.status = "FLAC render uses the Render window".to_string();
                    return Err("Use the Render window for FLAC".to_string());
                }
            }
        }
        Ok(())
    }

    fn render_with_options(&mut self, folder: &Path) -> Result<(), String> {
        if self.render_job.is_some() {
            return Ok(());
        }
        self.capture_plugin_states();
        let folder = Self::normalize_windows_path(folder);
        fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        let sample_rate = self.render_sample_rate.max(1);
        let format = self.render_format;
        let base_name = self.render_base_name();

        let project_end = self.project_end_beats().max(0.0);
        let range_start = self.render_range_start.max(0.0);
        let mut range_end = self.render_range_end.max(0.0);
        if range_end <= range_start {
            range_end = project_end.max(range_start + 0.25);
        }

        let master_name = match format {
            RenderFormat::Wav => format!("{base_name}.wav"),
            RenderFormat::Ogg => format!("{base_name}.ogg"),
            RenderFormat::Flac => format!("{base_name}.flac"),
        };
        let master_path = folder.join(master_name);
        let master_plan = self.build_master_render_plan(
            &master_path,
            sample_rate,
            range_start,
            range_end,
        );
        let mut plans = vec![master_plan];
        if self.render_split_tracks {
            for (index, track) in self.tracks.iter().enumerate() {
                let safe_name = Self::sanitize_folder_name(&track.name);
                let ext = match format {
                    RenderFormat::Wav => "wav",
                    RenderFormat::Ogg => "ogg",
                    RenderFormat::Flac => "flac",
                };
                let file_name = format!("{} - {:02}_{}.{}", base_name, index + 1, safe_name, ext);
                let path = folder.join(file_name);
                plans.push(self.build_render_plan_for_track(
                    index,
                    &path,
                    sample_rate,
                    range_start,
                    range_end,
                ));
            }
        }

        let done = Arc::new(AtomicU64::new(0));
        let total = Arc::new(AtomicU64::new(1));
        let finished = Arc::new(AtomicBool::new(false));
        let result = Arc::new(Mutex::new(None));
        self.render_progress = Some((0, 1));
        self.render_job = Some(RenderJob {
            done: done.clone(),
            total: total.clone(),
            finished: finished.clone(),
            result: result.clone(),
        });

        std::thread::spawn(move || {
            let mut final_status = Ok("Render complete".to_string());
            for plan in plans {
                done.store(0, Ordering::Relaxed);
                total.store(1, Ordering::Relaxed);
                let res = match format {
                    RenderFormat::Wav => render_plan_to_wav(plan, &done, &total),
                    RenderFormat::Ogg => render_plan_to_ogg(plan, &done, &total),
                    RenderFormat::Flac => render_plan_to_flac(plan, &done, &total),
                };
                if let Err(err) = res {
                    final_status = Err(err);
                    break;
                }
            }
            if let Ok(mut guard) = result.lock() {
                *guard = Some(final_status);
            }
            finished.store(true, Ordering::Relaxed);
        });

        Ok(())
    }

    fn build_master_render_plan(
        &self,
        path: &Path,
        sample_rate: u32,
        start_beats: f32,
        end_beats: f32,
    ) -> RenderPlan {
        let block_size = self.settings.buffer_size.max(64) as usize;
        let has_solo = self.tracks.iter().any(|t| t.solo);
        let (audio_clips, audio_cache) = self.build_audio_clip_render_data(sample_rate, None);
        let tracks = self
            .tracks
            .iter()
            .map(|track| RenderTrack {
                notes: track.midi_notes.clone(),
                instrument_path: track.instrument_path.clone(),
                param_ids: track.param_ids.clone(),
                param_values: track.param_values.clone(),
                plugin_state_component: track.plugin_state_component.clone(),
                plugin_state_controller: track.plugin_state_controller.clone(),
                effect_paths: track.effect_paths.clone(),
                effect_bypass: track.effect_bypass.clone(),
                automation_lanes: track.automation_lanes.clone(),
                level: track.level,
                active: !track.muted && (!has_solo || track.solo),
            })
            .collect::<Vec<_>>();
        RenderPlan {
            path: path.to_string_lossy().to_string(),
            sample_rate,
            block_size,
            tempo_bpm: self.tempo_bpm.max(1.0),
            start_beats: start_beats.max(0.0),
            end_beats: end_beats.max(start_beats + 0.25),
            bitrate_kbps: self.render_bitrate,
            tracks,
            notes: Vec::new(),
            instrument_path: None,
            param_ids: Vec::new(),
            param_values: Vec::new(),
            plugin_state_component: None,
            plugin_state_controller: None,
            audio_clips,
            audio_cache,
        }
    }

    fn build_render_plan_for_track(
        &self,
        index: usize,
        path: &Path,
        sample_rate: u32,
        start_beats: f32,
        end_beats: f32,
    ) -> RenderPlan {
        let block_size = self.settings.buffer_size.max(64) as usize;
        let (notes, instrument_path, param_ids, param_values, component, controller, automation_lanes) = self
            .tracks
            .get(index)
            .map(|track| {
                (
                    track.midi_notes.clone(),
                    track.instrument_path.clone(),
                    track.param_ids.clone(),
                    track.param_values.clone(),
                    track.plugin_state_component.clone(),
                    track.plugin_state_controller.clone(),
                    track.automation_lanes.clone(),
                )
            })
            .unwrap_or_else(|| (Vec::new(), None, Vec::new(), Vec::new(), None, None, Vec::new()));
        let (effect_paths, effect_bypass) = self
            .tracks
            .get(index)
            .map(|track| (track.effect_paths.clone(), track.effect_bypass.clone()))
            .unwrap_or_else(|| (Vec::new(), Vec::new()));
        let (audio_clips, audio_cache) =
            self.build_audio_clip_render_data(sample_rate, Some(index));
        let track = RenderTrack {
            notes,
            instrument_path,
            param_ids,
            param_values,
            plugin_state_component: component,
            plugin_state_controller: controller,
            effect_paths,
            effect_bypass,
            automation_lanes,
            level: 1.0,
            active: true,
        };
        RenderPlan {
            path: path.to_string_lossy().to_string(),
            sample_rate,
            block_size,
            tempo_bpm: self.tempo_bpm.max(1.0),
            start_beats: start_beats.max(0.0),
            end_beats: end_beats.max(start_beats + 0.25),
            bitrate_kbps: self.render_bitrate,
            tracks: vec![track],
            notes: Vec::new(),
            instrument_path: None,
            param_ids: Vec::new(),
            param_values: Vec::new(),
            plugin_state_component: None,
            plugin_state_controller: None,
            audio_clips,
            audio_cache,
        }
    }

    fn default_render_dir(&self) -> Option<PathBuf> {
        if !self.project_path.trim().is_empty() {
            let path = PathBuf::from(self.project_path.trim());
            if path.exists() {
                return Some(path);
            }
        }
        if let Some(default_dir) = self.default_project_dir() {
            let _ = fs::create_dir_all(&default_dir);
            if default_dir.exists() {
                return Some(default_dir);
            }
        }
        if let Ok(home) = std::env::var("USERPROFILE") {
            let home_path = PathBuf::from(home);
            let music_path = home_path.join("Music");
            if !music_path.exists() {
                let _ = fs::create_dir_all(&music_path);
            }
            if music_path.exists() {
                return Some(music_path);
            }
            if home_path.exists() {
                return Some(home_path);
            }
        }
        None
    }

    fn default_project_dir(&self) -> Option<PathBuf> {
        let base = std::env::current_exe().ok().and_then(|p| {
            let dir = p.parent()?.to_path_buf();
            Some(dir)
        })?;
        let name = if self.project_name.trim().is_empty() {
            "LingStationProject"
        } else {
            self.project_name.trim()
        };
        let folder = Self::sanitize_folder_name(name);
        Some(base.join(folder))
    }

    fn default_settings_path() -> String {
        #[cfg(windows)]
        {
            if let Ok(appdata) = std::env::var("APPDATA") {
                let dir = PathBuf::from(appdata).join("LingStation");
                let _ = fs::create_dir_all(&dir);
                return dir.join("settings.ling.json").to_string_lossy().to_string();
            }
        }
        #[cfg(not(windows))]
        {
            if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
                let dir = PathBuf::from(xdg).join("LingStation");
                let _ = fs::create_dir_all(&dir);
                return dir.join("settings.ling.json").to_string_lossy().to_string();
            }
            if let Ok(home) = std::env::var("HOME") {
                let dir = PathBuf::from(home).join(".config").join("LingStation");
                let _ = fs::create_dir_all(&dir);
                return dir.join("settings.ling.json").to_string_lossy().to_string();
            }
        }
        "settings.ling.json".to_string()
    }

    fn normalize_windows_path(path: &Path) -> PathBuf {
        #[cfg(windows)]
        {
            let raw = path.to_string_lossy();
            if let Some(stripped) = raw.strip_prefix(r"\\?\") {
                return PathBuf::from(stripped);
            }
        }
        path.to_path_buf()
    }

    fn render_to_wav(&mut self, path: &str) -> Result<(), String> {
        let sample_rate = self.settings.sample_rate.max(1);
        self.render_to_wav_with_rate(path, sample_rate)
    }

    fn render_to_wav_with_rate(&mut self, path: &str, sample_rate: u32) -> Result<(), String> {
        let channels = 2u16;
        let tempo = self.tempo_bpm.max(1.0);
        let beats = self.project_end_beats().max(1.0);
        let samples_per_beat = sample_rate as f64 * 60.0 / tempo as f64;
        let total_samples = (beats as f64 * samples_per_beat).ceil() as usize;
        let block_size = self.settings.buffer_size.max(64) as usize;
        let total_samples_u64 = total_samples as u64;
        self.render_progress = Some((0, total_samples_u64));

        let spec = hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
        let mut writer = hound::WavWriter::new(file, spec).map_err(|e| e.to_string())?;

        let mut host = if let Some(path) = self.active_instrument_path() {
            vst3::Vst3Host::load(
                &path,
                sample_rate as f64,
                block_size,
                channels as usize,
            )
            .ok()
        } else {
            None
        };

        let notes = self.active_midi_notes();
        let mut cursor = 0usize;
        while cursor < total_samples {
            let frames = (total_samples - cursor).min(block_size);
            let block_start = cursor as u64;
            let block_end = (cursor + frames) as u64;
            let events = collect_block_events(&notes, block_start, block_end, samples_per_beat);
            let mut output = vec![0.0f32; frames * channels as usize];
            if let Some(host) = host.as_mut() {
                let _ = host.process_f32(&mut output, channels as usize, &events);
            }
            for sample in output {
                writer.write_sample(sample).map_err(|e| e.to_string())?;
            }
            cursor += frames;
            self.render_progress = Some((cursor as u64, total_samples_u64));
        }

        writer.finalize().map_err(|e| e.to_string())?;
        self.render_progress = Some((total_samples_u64, total_samples_u64));
        Ok(())
    }

    fn project_end_beats(&self) -> f32 {
        let mut max_beat = 0.0f32;
        for track in &self.tracks {
            for clip in &track.clips {
                max_beat = max_beat.max(clip.start_beats + clip.length_beats);
            }
            for note in &track.midi_notes {
                max_beat = max_beat.max(note.start_beats + note.length_beats);
            }
        }
        max_beat
    }

    fn project_clip_range(&self) -> Option<(f32, f32)> {
        let mut min_start = f32::MAX;
        let mut max_end = 0.0f32;
        let mut found = false;
        for track in &self.tracks {
            for clip in &track.clips {
                min_start = min_start.min(clip.start_beats);
                max_end = max_end.max(clip.start_beats + clip.length_beats);
                found = true;
            }
        }
        if found {
            Some((min_start.max(0.0), max_end.max(min_start + 0.25)))
        } else {
            None
        }
    }

    fn active_instrument_path(&self) -> Option<String> {
        if let Some(index) = self.selected_track {
            self.tracks
                .get(index)
                .and_then(|track| track.instrument_path.clone())
        } else {
            self.tracks
                .first()
                .and_then(|track| track.instrument_path.clone())
        }
    }

    fn active_midi_notes(&self) -> Vec<PianoRollNote> {
        if let Some(index) = self.selected_track {
            self.tracks
                .get(index)
                .map(|track| track.midi_notes.clone())
                .unwrap_or_default()
        } else {
            self.tracks
                .first()
                .map(|track| track.midi_notes.clone())
                .unwrap_or_default()
        }
    }

    fn active_track_snapshot(
        &self,
    ) -> Option<(
        Vec<PianoRollNote>,
        Option<String>,
        Vec<u32>,
        Vec<f32>,
        Option<Vec<u8>>,
        Option<Vec<u8>>,
    )> {
        let index = self.selected_track.unwrap_or(0);
        let track = self.tracks.get(index)?;
        Some((
            track.midi_notes.clone(),
            track.instrument_path.clone(),
            track.param_ids.clone(),
            track.param_values.clone(),
            track.plugin_state_component.clone(),
            track.plugin_state_controller.clone(),
        ))
    }

    fn toggle_recording(&mut self) {
        if self.is_recording {
            if let Err(err) = self.end_recording() {
                self.status = format!("Stop recording failed: {err}");
            }
        } else if let Err(err) = self.begin_recording() {
            self.status = format!("Record failed: {err}");
        }
    }

    fn begin_recording(&mut self) -> Result<(), String> {
        if self.is_recording {
            return Ok(());
        }
        let track_index = self.selected_track.unwrap_or(0).min(self.tracks.len().saturating_sub(1));
        let start_beats = self.playhead_beats.max(0.0);
        let start_samples = self.transport_samples.load(Ordering::Relaxed);
        if !self.audio_running {
            self.start_audio_and_midi()?;
            self.seek_playhead(start_beats);
            self.record_started_audio = true;
        } else {
            self.record_started_audio = false;
        }
        if let Ok(mut rec) = self.recording.lock() {
            rec.active = true;
            rec.track_index = track_index;
            rec.start_samples = start_samples;
            rec.start_beats = start_beats;
            rec.record_audio = self.record_audio;
            rec.record_midi = self.record_midi;
            rec.record_automation = self.record_automation;
            rec.audio_samples.clear();
            rec.audio_channels = 0;
            rec.audio_sample_rate = self.settings.sample_rate.max(1);
            rec.midi_active.clear();
            rec.midi_notes.clear();
            rec.automation_points.clear();
        }
        if self.record_audio {
            self.start_audio_input_stream()?;
        }
        self.is_recording = true;
        self.status = "Recording...".to_string();
        Ok(())
    }

    fn end_recording(&mut self) -> Result<(), String> {
        if !self.is_recording {
            return Ok(());
        }
        self.is_recording = false;
        let _stream = self.audio_input_stream.take();
        let mut rec = self.recording.lock().map_err(|_| "Recording lock failed".to_string())?;
        rec.active = false;
        let track_index = rec.track_index;
        let start_beats = rec.start_beats;
        let record_audio = rec.record_audio;
        let record_midi = rec.record_midi;
        let record_automation = rec.record_automation;
        let audio_samples = std::mem::take(&mut rec.audio_samples);
        let audio_channels = rec.audio_channels.max(1);
        let audio_sample_rate = rec.audio_sample_rate.max(1);
        let midi_notes = std::mem::take(&mut rec.midi_notes);
        let automation_points = std::mem::take(&mut rec.automation_points);
        drop(rec);

        if record_audio && !audio_samples.is_empty() {
            self.finalize_audio_recording(track_index, start_beats, audio_channels, audio_sample_rate, audio_samples)?;
        }
        if record_midi && !midi_notes.is_empty() {
            self.finalize_midi_recording(track_index, start_beats, midi_notes);
        }
        if record_automation && !automation_points.is_empty() {
            self.apply_recorded_automation(track_index, automation_points);
        }

        if self.record_started_audio {
            self.stop_audio_and_midi();
            self.record_started_audio = false;
        }
        self.status = "Recording stopped".to_string();
        Ok(())
    }

    fn start_audio_input_stream(&mut self) -> Result<(), String> {
        let host = cpal::default_host();
        let device = if self.settings.input_device.trim().is_empty() {
            host.default_input_device()
        } else {
            host.input_devices()
                .ok()
                .and_then(|mut devices| {
                    devices.find(|d| d.name().ok().as_deref() == Some(self.settings.input_device.as_str()))
                })
                .or_else(|| host.default_input_device())
        }
        .ok_or("No input device")?;
        let config = device.default_input_config().map_err(|e| e.to_string())?;
        let channels = config.channels() as usize;
        let mut stream_config: cpal::StreamConfig = config.clone().into();
        stream_config.sample_rate = cpal::SampleRate(self.settings.sample_rate.max(1));
        stream_config.buffer_size = cpal::BufferSize::Fixed(self.effective_buffer_size());
        let recording = self.recording.clone();

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &stream_config,
                move |data: &[f32], _| {
                    if let Ok(mut rec) = recording.lock() {
                        if !rec.active || !rec.record_audio {
                            return;
                        }
                        rec.audio_channels = channels;
                        rec.audio_sample_rate = stream_config.sample_rate.0;
                        rec.audio_samples.extend_from_slice(data);
                    }
                },
                move |err| {
                    eprintln!("audio input error: {err}");
                },
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                &stream_config,
                move |data: &[i16], _| {
                    if let Ok(mut rec) = recording.lock() {
                        if !rec.active || !rec.record_audio {
                            return;
                        }
                        rec.audio_channels = channels;
                        rec.audio_sample_rate = stream_config.sample_rate.0;
                        rec.audio_samples.extend(data.iter().map(|s| *s as f32 / i16::MAX as f32));
                    }
                },
                move |err| {
                    eprintln!("audio input error: {err}");
                },
                None,
            ),
            cpal::SampleFormat::U16 => device.build_input_stream(
                &stream_config,
                move |data: &[u16], _| {
                    if let Ok(mut rec) = recording.lock() {
                        if !rec.active || !rec.record_audio {
                            return;
                        }
                        rec.audio_channels = channels;
                        rec.audio_sample_rate = stream_config.sample_rate.0;
                        let norm = u16::MAX as f32;
                        rec.audio_samples.extend(data.iter().map(|s| (*s as f32 / norm) * 2.0 - 1.0));
                    }
                },
                move |err| {
                    eprintln!("audio input error: {err}");
                },
                None,
            ),
            _ => return Err("Unsupported input sample format".to_string()),
        }
        .map_err(|e| e.to_string())?;

        stream.play().map_err(|e| e.to_string())?;
        self.audio_input_stream = Some(stream);
        Ok(())
    }

    fn finalize_audio_recording(
        &mut self,
        track_index: usize,
        start_beats: f32,
        channels: usize,
        sample_rate: u32,
        samples: Vec<f32>,
    ) -> Result<(), String> {
        let project_folder = self.ensure_project_folder()?;
        let audio_dir = project_folder.join("audio");
        fs::create_dir_all(&audio_dir).map_err(|e| e.to_string())?;
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let file_name = format!("recording_{timestamp}.wav");
        let path = audio_dir.join(&file_name);
        let spec = hound::WavSpec {
            channels: channels as u16,
            sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
        let mut writer = hound::WavWriter::new(file, spec).map_err(|e| e.to_string())?;
        for sample in samples.iter() {
            writer.write_sample(*sample).map_err(|e| e.to_string())?;
        }
        writer.finalize().map_err(|e| e.to_string())?;

        let frames = samples.len().saturating_sub(0) / channels.max(1);
        let seconds = frames as f32 / sample_rate.max(1) as f32;
        let beats = (seconds * self.tempo_bpm.max(1.0) / 60.0).max(0.25);
        let clip_id = self.next_clip_id();
        if let Some(track) = self.tracks.get_mut(track_index) {
            track.clips.push(Clip {
                id: clip_id,
                track: track_index,
                start_beats,
                length_beats: beats,
                is_midi: false,
                name: "Recording".to_string(),
                audio_path: Some(format!("audio/{file_name}")),
                audio_source_beats: Some(beats),
                audio_offset_beats: 0.0,
                audio_gain: 1.0,
                audio_pitch_semitones: 0.0,
                audio_time_mul: 1.0,
            });
        }
        if self.audio_running {
            let timeline = self.build_audio_clip_timeline(self.settings.sample_rate);
            if let Ok(mut guard) = self.audio_clip_timeline.lock() {
                *guard = timeline;
            }
            self.preload_audio_clips(&self.audio_clip_cache);
        }
        self.selected_track = Some(track_index);
        self.selected_clip = Some(clip_id);
        Ok(())
    }

    fn finalize_midi_recording(
        &mut self,
        track_index: usize,
        start_beats: f32,
        notes: Vec<PianoRollNote>,
    ) {
        let clip_id = self.next_clip_id();
        if let Some(track) = self.tracks.get_mut(track_index) {
            let mut max_end = start_beats + 0.25;
            for note in &notes {
                max_end = max_end.max(note.start_beats + note.length_beats);
            }
            track.midi_notes.extend(notes);
            track.clips.push(Clip {
                id: clip_id,
                track: track_index,
                start_beats,
                length_beats: (max_end - start_beats).max(0.25),
                is_midi: true,
                name: "MIDI Rec".to_string(),
                audio_path: None,
                audio_source_beats: None,
                audio_offset_beats: 0.0,
                audio_gain: 1.0,
                audio_pitch_semitones: 0.0,
                audio_time_mul: 1.0,
            });
            self.sync_track_audio_notes(track_index);
        }
    }

    fn apply_recorded_automation(&mut self, track_index: usize, points: Vec<RecordedAutomationPoint>) {
        if let Some(track) = self.tracks.get_mut(track_index) {
            let mut grouped: HashMap<(i32, u32), Vec<AutomationPoint>> = HashMap::new();
            for point in points {
                let key = match point.target {
                    AutomationTarget::Instrument => -1,
                    AutomationTarget::Effect(index) => index as i32,
                };
                grouped
                    .entry((key, point.param_id))
                    .or_default()
                    .push(AutomationPoint {
                        beat: point.beat,
                        value: point.value,
                    });
            }
            for ((target_key, param_id), mut new_points) in grouped {
                let target = if target_key < 0 {
                    AutomationTarget::Instrument
                } else {
                    AutomationTarget::Effect(target_key as usize)
                };
                Self::coalesce_automation_points(&mut new_points, 0.02);
                let lane_index = if let Some(index) = track
                    .automation_lanes
                    .iter()
                    .position(|lane| lane.param_id == param_id && lane.target == target)
                {
                    index
                } else {
                    let name = match target {
                        AutomationTarget::Instrument => track
                            .param_ids
                            .iter()
                            .position(|id| *id == param_id)
                            .and_then(|i| track.params.get(i).cloned())
                            .unwrap_or_else(|| format!("Param {}", param_id)),
                        AutomationTarget::Effect(fx_index) => {
                            let fx_name = track
                                .effect_paths
                                .get(fx_index)
                                .map(|p| Self::plugin_display_name(p))
                                .unwrap_or_else(|| format!("FX {}", fx_index + 1));
                            let param_name = track
                                .effect_param_ids
                                .get(fx_index)
                                .and_then(|ids| ids.iter().position(|id| *id == param_id))
                                .and_then(|i| track.effect_params.get(fx_index).and_then(|p| p.get(i)).cloned())
                                .unwrap_or_else(|| format!("Param {}", param_id));
                            format!("{}: {}", fx_name, param_name)
                        }
                    };
                    track.automation_lanes.push(AutomationLane {
                        name,
                        param_id,
                        target,
                        points: Vec::new(),
                    });
                    track.automation_lanes.len() - 1
                };
                if let Some(lane) = track.automation_lanes.get_mut(lane_index) {
                    if new_points.is_empty() {
                        continue;
                    }
                    let range_start = new_points.first().map(|p| p.beat).unwrap_or(0.0);
                    let range_end = new_points.last().map(|p| p.beat).unwrap_or(range_start);
                    let mut merged: Vec<AutomationPoint> = lane
                        .points
                        .iter()
                        .cloned()
                        .filter(|p| p.beat < range_start - 0.02 || p.beat > range_end + 0.02)
                        .collect();
                    merged.extend(new_points.into_iter());
                    Self::coalesce_automation_points(&mut merged, 0.02);
                    lane.points = merged;
                }
            }
            if let Some(state) = self.track_audio.get(track_index) {
                if let Ok(mut lanes) = state.automation_lanes.lock() {
                    *lanes = track.automation_lanes.clone();
                }
            }
        }
    }

    fn record_automation_point(
        &mut self,
        track_index: usize,
        target: AutomationTarget,
        param_id: u32,
        beat: f32,
        value: f32,
    ) {
        if let Some(track) = self.tracks.get_mut(track_index) {
            let lane_index = if let Some(index) = track
                .automation_lanes
                .iter()
                .position(|lane| lane.param_id == param_id && lane.target == target)
            {
                index
            } else {
                let name = match target {
                    AutomationTarget::Instrument => track
                        .param_ids
                        .iter()
                        .position(|id| *id == param_id)
                        .and_then(|i| track.params.get(i).cloned())
                        .unwrap_or_else(|| format!("Param {}", param_id)),
                    AutomationTarget::Effect(fx_index) => {
                        let fx_name = track
                            .effect_paths
                            .get(fx_index)
                            .map(|p| Self::plugin_display_name(p))
                            .unwrap_or_else(|| format!("FX {}", fx_index + 1));
                        let param_name = track
                            .effect_param_ids
                            .get(fx_index)
                            .and_then(|ids| ids.iter().position(|id| *id == param_id))
                            .and_then(|i| track.effect_params.get(fx_index).and_then(|p| p.get(i)).cloned())
                            .unwrap_or_else(|| format!("Param {}", param_id));
                        format!("{}: {}", fx_name, param_name)
                    }
                };
                track.automation_lanes.push(AutomationLane {
                    name,
                    param_id,
                    target,
                    points: Vec::new(),
                });
                track.automation_lanes.len() - 1
            };
            if let Some(lane) = track.automation_lanes.get_mut(lane_index) {
                let mut updated = false;
                for point in lane.points.iter_mut() {
                    if (point.beat - beat).abs() <= 0.02 {
                        point.beat = beat;
                        point.value = value;
                        updated = true;
                        break;
                    }
                }
                if !updated {
                    lane.points.push(AutomationPoint { beat, value });
                }
                Self::coalesce_automation_points(&mut lane.points, 0.02);
            }
            if let Some(state) = self.track_audio.get(track_index) {
                if let Ok(mut lanes) = state.automation_lanes.lock() {
                    *lanes = track.automation_lanes.clone();
                }
            }
        }
    }

    fn coalesce_automation_points(points: &mut Vec<AutomationPoint>, epsilon: f32) {
        points.sort_by(|a, b| a.beat.partial_cmp(&b.beat).unwrap());
        let mut merged: Vec<AutomationPoint> = Vec::with_capacity(points.len());
        for point in points.drain(..) {
            if let Some(last) = merged.last_mut() {
                if (last.beat - point.beat).abs() <= epsilon {
                    *last = point;
                    continue;
                }
            }
            merged.push(point);
        }
        *points = merged;
    }

    fn base_buffer_size(&self) -> u32 {
        self.buffer_override
            .unwrap_or(self.settings.buffer_size)
            .max(1)
    }

    fn effective_buffer_size(&self) -> u32 {
        let base = self.base_buffer_size();
        if self.settings.triple_buffer {
            base.saturating_mul(3).max(1)
        } else {
            base
        }
    }


    fn start_audio_and_midi(&mut self) -> Result<(), String> {
        self.start_audio_and_midi_internal(true)
    }

    fn start_audio_and_midi_internal(&mut self, reset_transport: bool) -> Result<(), String> {
        if self.audio_running {
            return Ok(());
        }
        self.audio_stop.store(false, Ordering::Relaxed);
        let host = cpal::default_host();
        let device = if self.settings.output_device.trim().is_empty() {
            host.default_output_device()
        } else {
            host.output_devices()
                .ok()
                .and_then(|mut devices| {
                    devices.find(|d| d.name().ok().as_deref() == Some(self.settings.output_device.as_str()))
                })
                .or_else(|| host.default_output_device())
        }
        .ok_or("No output device")?;
        let config = device.default_output_config().map_err(|e| e.to_string())?;
        let sample_rate = self.settings.sample_rate.max(1) as f32;
        let channels = config.channels() as usize;
        self.last_output_channels = channels.max(1);
        let effective_buffer = self.effective_buffer_size();
        let buffer_size_usize = effective_buffer as usize;
        let freq_bits = self.midi_freq_bits.clone();
        let gate = self.midi_gate.clone();
        let master_peak_bits = self.master_peak_bits.clone();
        self.adaptive_buffer_size
            .store(effective_buffer, Ordering::Relaxed);
        self.last_overrun.store(false, Ordering::Relaxed);
        if reset_transport {
            self.transport_samples.store(0, Ordering::Relaxed);
        }
        self.tempo_bits.store(self.tempo_bpm.to_bits(), Ordering::Relaxed);
        self.sync_track_audio_states();
        let timeline = self.build_audio_clip_timeline(self.settings.sample_rate);
        if let Ok(mut guard) = self.audio_clip_timeline.lock() {
            *guard = timeline;
        }
        self.preload_audio_clips(&self.audio_clip_cache);
        for index in 0..self.tracks.len() {
            let path = self.tracks[index].instrument_path.clone();
            let effect_paths = self.tracks[index].effect_paths.clone();
            let state = match self.track_audio.get_mut(index) {
                Some(state) => state,
                None => continue,
            };
            if let Some(path) = path {
                if state.host.is_none() {
                match vst3::Vst3Host::load(
                    &path,
                    self.settings.sample_rate as f64,
                    buffer_size_usize,
                    channels,
                ) {
                    Ok(host) => {
                        let params = host.enumerate_params();
                        if let Some(track) = self.tracks.get_mut(index) {
                            if !params.is_empty() {
                                let next_ids: Vec<u32> = params.iter().map(|p| p.id).collect();
                                let reuse = track.param_ids == next_ids
                                    && track.param_values.len() == params.len();
                                let next_values = if reuse {
                                    track.param_values.clone()
                                } else {
                                    params.iter().map(|p| p.default_value as f32).collect()
                                };
                                track.params = params.iter().map(|p| p.name.clone()).collect();
                                track.param_ids = next_ids;
                                track.param_values = next_values;
                                Self::apply_program_param(track);
                            }
                        }
                        let host = Arc::new(Mutex::new(host));
                        state.host = Some(host.clone());
                        if let Some(track) = self.tracks.get(index) {
                            let component = track.plugin_state_component.clone();
                            let controller = track.plugin_state_controller.clone();
                            let has_state = component
                                .as_ref()
                                .map(|v| !v.is_empty())
                                .unwrap_or(false)
                                || controller
                                    .as_ref()
                                    .map(|v| !v.is_empty())
                                    .unwrap_or(false);
                            if let Ok(mut host) = host.lock() {
                                if has_state {
                                    let _ = host.set_state_bytes(
                                        component.as_deref(),
                                        controller.as_deref(),
                                    );
                                } else if !track.param_ids.is_empty() {
                                    for (param_id, value) in
                                        track.param_ids.iter().zip(track.param_values.iter())
                                    {
                                        host.push_param_change(*param_id, *value as f64);
                                    }
                                }
                            }
                        }
                    }
                    Err(err) => {
                        self.status = format!("VST3 host error: {err}");
                    }
                }
                }
            } else {
                state.host = None;
            }
            if state.effect_hosts.len() != effect_paths.len() {
                for host in state.effect_hosts.drain(..) {
                    if let Ok(mut host) = host.lock() {
                        host.prepare_for_drop();
                    }
                    self.orphaned_hosts.push(host);
                }
                for fx_path in effect_paths.iter() {
                    match vst3::Vst3Host::load_with_input(
                        fx_path,
                        self.settings.sample_rate as f64,
                        buffer_size_usize,
                        channels,
                        channels,
                    ) {
                        Ok(host) => {
                            state.effect_hosts.push(Arc::new(Mutex::new(host)));
                        }
                        Err(err) => {
                            self.status = format!("FX host error: {err}");
                        }
                    }
                }
            }
            if let Some(track) = self.tracks.get_mut(index) {
                if track.effect_bypass.len() != effect_paths.len() {
                    track.effect_bypass.resize(effect_paths.len(), false);
                }
                if track.effect_params.len() != effect_paths.len() {
                    track.effect_params.resize(effect_paths.len(), Vec::new());
                    track.effect_param_ids.resize(effect_paths.len(), Vec::new());
                    track.effect_param_values.resize(effect_paths.len(), Vec::new());
                }
                for (fx_index, fx_host) in state.effect_hosts.iter().enumerate() {
                    if let Ok(host) = fx_host.lock() {
                        let params = host.enumerate_params();
                        if !params.is_empty() {
                            let next_ids: Vec<u32> = params.iter().map(|p| p.id).collect();
                            let reuse = track
                                .effect_param_ids
                                .get(fx_index)
                                .map(|ids| *ids == next_ids)
                                .unwrap_or(false)
                                && track
                                    .effect_param_values
                                    .get(fx_index)
                                    .map(|vals| vals.len() == params.len())
                                    .unwrap_or(false);
                            let next_values = if reuse {
                                track.effect_param_values[fx_index].clone()
                            } else {
                                params.iter().map(|p| p.default_value as f32).collect()
                            };
                            if let Some(slot) = track.effect_params.get_mut(fx_index) {
                                *slot = params.iter().map(|p| p.name.clone()).collect();
                            }
                            if let Some(slot) = track.effect_param_ids.get_mut(fx_index) {
                                *slot = next_ids;
                            }
                            if let Some(slot) = track.effect_param_values.get_mut(fx_index) {
                                *slot = next_values;
                            }
                        }
                    }
                }
                state.sync_effect_bypass(track);
            }
        }
        let track_audio = self.track_audio.clone();
        let track_mix = self.track_mix.clone();
        let tempo_bits = self.tempo_bits.clone();
        let transport_samples = self.transport_samples.clone();
        let loop_start_samples = self.loop_start_samples.clone();
        let loop_end_samples = self.loop_end_samples.clone();
        let audio_stop = self.audio_stop.clone();
        let audio_callback_active = self.audio_callback_active.clone();
        let audio_clip_cache = self.audio_clip_cache.clone();
        let audio_clip_timeline = self.audio_clip_timeline.clone();
        let adaptive_enabled = self.settings.adaptive_buffer;
        let safe_underruns = self.settings.safe_underruns;
        let smart_disable_plugins = self.settings.smart_disable_plugins;
        let smart_suspend_tracks = self.settings.smart_suspend_tracks;
        let adaptive_restart_requested = self.adaptive_restart_requested.clone();
        let adaptive_buffer_size = self.adaptive_buffer_size.clone();
        let last_overrun = self.last_overrun.clone();

        let mut stream_config: cpal::StreamConfig = config.clone().into();
        stream_config.sample_rate = cpal::SampleRate(self.settings.sample_rate);
        stream_config.buffer_size = cpal::BufferSize::Fixed(effective_buffer);

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                let track_audio = track_audio.clone();
                let track_mix = track_mix.clone();
                let tempo_bits = tempo_bits.clone();
                let transport_samples = transport_samples.clone();
                device.build_output_stream(
                    &stream_config,
                    move |data: &mut [f32], _| {
                        let _guard = CallbackGuard::new(audio_callback_active.clone());
                        if audio_stop.load(Ordering::Relaxed) {
                            data.fill(0.0);
                            update_master_peak_f32(data, &master_peak_bits);
                            return;
                        }
                        if safe_underruns && last_overrun.swap(false, Ordering::Relaxed) {
                            data.fill(0.0);
                            update_master_peak_f32(data, &master_peak_bits);
                            return;
                        }
                        let started = std::time::Instant::now();
                        data.fill(0.0);
                        let processed = mix_track_hosts(
                            data,
                            channels,
                            sample_rate,
                            &tempo_bits,
                            &transport_samples,
                            &loop_start_samples,
                            &loop_end_samples,
                            &track_audio,
                            &track_mix,
                            &audio_clip_timeline,
                            &audio_clip_cache,
                            smart_disable_plugins,
                            smart_suspend_tracks,
                        );
                        if !processed {
                            render_sine(data, channels, sample_rate, &freq_bits, &gate);
                        }
                        for sample in data.iter_mut() {
                            *sample = sample.clamp(-1.0, 1.0);
                        }
                        update_master_peak_f32(data, &master_peak_bits);
                        let elapsed = started.elapsed().as_secs_f32();
                        let buffer_secs = (data.len() / channels) as f32 / sample_rate.max(1.0);
                        if elapsed > buffer_secs {
                            if safe_underruns {
                                last_overrun.store(true, Ordering::Relaxed);
                            }
                            if adaptive_enabled {
                                let current = adaptive_buffer_size.load(Ordering::Relaxed);
                                let next = (current.saturating_mul(2)).min(8192).max(current);
                                if next > current {
                                    adaptive_buffer_size.store(next, Ordering::Relaxed);
                                    adaptive_restart_requested.store(true, Ordering::Relaxed);
                                }
                            }
                        }
                    },
                    move |err| {
                        eprintln!("audio error: {err}");
                    },
                    None,
                )
            }
            cpal::SampleFormat::I16 => {
                let track_audio = track_audio.clone();
                let track_mix = track_mix.clone();
                let tempo_bits = tempo_bits.clone();
                let transport_samples = transport_samples.clone();
                let audio_stop = audio_stop.clone();
                let audio_callback_active = audio_callback_active.clone();
                let mut temp = Vec::<f32>::new();
                device.build_output_stream(
                    &stream_config,
                    move |data: &mut [i16], _| {
                        let _guard = CallbackGuard::new(audio_callback_active.clone());
                        if audio_stop.load(Ordering::Relaxed) {
                            data.fill(0);
                            update_master_peak_i16(data, &master_peak_bits);
                            return;
                        }
                        if safe_underruns && last_overrun.swap(false, Ordering::Relaxed) {
                            data.fill(0);
                            update_master_peak_i16(data, &master_peak_bits);
                            return;
                        }
                        let started = std::time::Instant::now();
                        temp.resize(data.len(), 0.0);
                        let processed = mix_track_hosts(
                            &mut temp,
                            channels,
                            sample_rate,
                            &tempo_bits,
                            &transport_samples,
                            &loop_start_samples,
                            &loop_end_samples,
                            &track_audio,
                            &track_mix,
                            &audio_clip_timeline,
                            &audio_clip_cache,
                            smart_disable_plugins,
                            smart_suspend_tracks,
                        );
                        if !processed {
                            render_sine(&mut temp, channels, sample_rate, &freq_bits, &gate);
                        }
                        for sample in temp.iter_mut() {
                            *sample = sample.clamp(-1.0, 1.0);
                        }
                        for (out, sample) in data.iter_mut().zip(temp.iter()) {
                            *out = cpal::Sample::from_sample(*sample);
                        }
                        update_master_peak_f32(&temp, &master_peak_bits);
                        let elapsed = started.elapsed().as_secs_f32();
                        let buffer_secs = (data.len() / channels) as f32 / sample_rate.max(1.0);
                        if elapsed > buffer_secs {
                            if safe_underruns {
                                last_overrun.store(true, Ordering::Relaxed);
                            }
                            if adaptive_enabled {
                                let current = adaptive_buffer_size.load(Ordering::Relaxed);
                                let next = (current.saturating_mul(2)).min(8192).max(current);
                                if next > current {
                                    adaptive_buffer_size.store(next, Ordering::Relaxed);
                                    adaptive_restart_requested.store(true, Ordering::Relaxed);
                                }
                            }
                        }
                    },
                    move |err| {
                        eprintln!("audio error: {err}");
                    },
                    None,
                )
            }
            cpal::SampleFormat::U16 => {
                let track_audio = track_audio.clone();
                let track_mix = track_mix.clone();
                let tempo_bits = tempo_bits.clone();
                let transport_samples = transport_samples.clone();
                let audio_stop = audio_stop.clone();
                let audio_callback_active = audio_callback_active.clone();
                let mut temp = Vec::<f32>::new();
                device.build_output_stream(
                    &stream_config,
                    move |data: &mut [u16], _| {
                        let _guard = CallbackGuard::new(audio_callback_active.clone());
                        if audio_stop.load(Ordering::Relaxed) {
                            let silence = u16::MAX / 2;
                            data.fill(silence);
                            update_master_peak_u16(data, &master_peak_bits);
                            return;
                        }
                        if safe_underruns && last_overrun.swap(false, Ordering::Relaxed) {
                            let silence = u16::MAX / 2;
                            data.fill(silence);
                            update_master_peak_u16(data, &master_peak_bits);
                            return;
                        }
                        let started = std::time::Instant::now();
                        temp.resize(data.len(), 0.0);
                        let processed = mix_track_hosts(
                            &mut temp,
                            channels,
                            sample_rate,
                            &tempo_bits,
                            &transport_samples,
                            &loop_start_samples,
                            &loop_end_samples,
                            &track_audio,
                            &track_mix,
                            &audio_clip_timeline,
                            &audio_clip_cache,
                            smart_disable_plugins,
                            smart_suspend_tracks,
                        );
                        if !processed {
                            render_sine(&mut temp, channels, sample_rate, &freq_bits, &gate);
                        }
                        for sample in temp.iter_mut() {
                            *sample = sample.clamp(-1.0, 1.0);
                        }
                        for (out, sample) in data.iter_mut().zip(temp.iter()) {
                            *out = cpal::Sample::from_sample(*sample);
                        }
                        update_master_peak_f32(&temp, &master_peak_bits);
                        let elapsed = started.elapsed().as_secs_f32();
                        let buffer_secs = (data.len() / channels) as f32 / sample_rate.max(1.0);
                        if elapsed > buffer_secs {
                            if safe_underruns {
                                last_overrun.store(true, Ordering::Relaxed);
                            }
                            if adaptive_enabled {
                                let current = adaptive_buffer_size.load(Ordering::Relaxed);
                                let next = (current.saturating_mul(2)).min(8192).max(current);
                                if next > current {
                                    adaptive_buffer_size.store(next, Ordering::Relaxed);
                                    adaptive_restart_requested.store(true, Ordering::Relaxed);
                                }
                            }
                        }
                    },
                    move |err| {
                        eprintln!("audio error: {err}");
                    },
                    None,
                )
            }
            _ => return Err("Unsupported sample format".to_string()),
        }
        .map_err(|e| e.to_string())?;

        stream.play().map_err(|e| e.to_string())?;
        self.audio_stream = Some(stream);

        let mut midi_in = MidiInput::new("LingStation")
            .map_err(|e| e.to_string())?;
        midi_in.ignore(Ignore::None);
        let ports = midi_in.ports();
        let selected_port = if self.settings.midi_input.trim().is_empty() {
            ports.first().cloned()
        } else {
            ports
                .iter()
                .find(|p| midi_in.port_name(p).ok().as_deref() == Some(self.settings.midi_input.as_str()))
                .cloned()
        };

        if let Some(port) = selected_port {
            let freq_bits = self.midi_freq_bits.clone();
            let gate = self.midi_gate.clone();
            let track_audio = self.track_audio.clone();
            let selected_track_index = self.selected_track_index.clone();
            let midi_learn = self.midi_learn.clone();
            let recording = self.recording.clone();
            let tempo_bits = self.tempo_bits.clone();
            let transport_samples = self.transport_samples.clone();
            let record_sample_rate = self.settings.sample_rate.max(1) as f32;
            let conn = midi_in.connect(
                &port,
                "lingstation-midi",
                move |_stamp, message, _| {
                    if message.len() < 3 {
                        return;
                    }
                    let status = message[0] & 0xF0;
                    let channel = message[0] & 0x0F;
                    let note = message[1];
                    let vel = message[2];
                    let index = selected_track_index.load(Ordering::Relaxed);
                    let state = if index == usize::MAX {
                        None
                    } else {
                        track_audio.get(index)
                    };
                    let bpm = f32::from_bits(tempo_bits.load(Ordering::Relaxed)).max(1.0);
                    let samples = transport_samples.load(Ordering::Relaxed) as f32;
                    let beat = (samples / record_sample_rate) * (bpm / 60.0);
                    if status == 0x90 && vel > 0 {
                        let freq = 440.0f32 * 2.0f32.powf((note as f32 - 69.0) / 12.0);
                        freq_bits.store(freq.to_bits(), Ordering::Relaxed);
                        gate.store(true, Ordering::Relaxed);
                        if let Some(state) = state {
                            if let Ok(mut events) = state.midi_events.lock() {
                                events.push(vst3::MidiEvent::note_on(channel, note, vel));
                            }
                        }
                            if let Ok(mut rec) = recording.lock() {
                                if rec.active && rec.record_midi {
                                    rec.midi_active.insert(note, (beat, vel));
                                }
                            }
                    } else if status == 0x80 || (status == 0x90 && vel == 0) {
                        gate.store(false, Ordering::Relaxed);
                        if let Some(state) = state {
                            if let Ok(mut events) = state.midi_events.lock() {
                                events.push(vst3::MidiEvent::note_off(channel, note, vel));
                            }
                        }
                            if let Ok(mut rec) = recording.lock() {
                                if rec.active && rec.record_midi {
                                    if let Some((start, start_vel)) = rec.midi_active.remove(&note) {
                                        let length = (beat - start).max(0.05);
                                        let velocity = if start_vel > 0 { start_vel } else { vel };
                                        rec.midi_notes.push(PianoRollNote::new(start, length, note, velocity));
                                    }
                                }
                            }
                    } else if status == 0xB0 {
                        if let Ok(mut learn) = midi_learn.lock() {
                            if let Some((learn_index, param_id)) = *learn {
                                if learn_index == index {
                                    if let Some(state) = track_audio.get(learn_index) {
                                        if let Ok(mut map) = state.learned_cc.lock() {
                                            map.insert((channel, note), param_id);
                                        }
                                    }
                                    *learn = None;
                                    return;
                                }
                            }
                        }
                        if let Some(state) = state {
                            if let Ok(mut events) = state.midi_events.lock() {
                                events.push(vst3::MidiEvent::control_change(channel, note, vel));
                            }
                            if let Ok(map) = state.learned_cc.lock() {
                                if let Some(param_id) = map.get(&(channel, note)).copied() {
                                    if let Ok(mut rec) = recording.lock() {
                                        if rec.active && rec.record_automation {
                                            let value = (vel as f32 / 127.0).clamp(0.0, 1.0);
                                            rec.automation_points.push(RecordedAutomationPoint {
                                                param_id,
                                                target: AutomationTarget::Instrument,
                                                beat,
                                                value,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
                (),
            )
            .map_err(|e| e.to_string())?;
            self.midi_conn = Some(conn);
        } else {
            self.status = "No MIDI input devices found".to_string();
        }

        self.audio_running = true;
        Ok(())
    }

    fn reinit_audio_if_running(&mut self) {
        if !self.audio_running {
            return;
        }
        self.stop_audio_and_midi();
        if let Err(err) = self.start_audio_and_midi() {
            self.status = format!("Audio restart failed: {err}");
        } else {
            self.status = "Audio restarted for new VST3".to_string();
        }
    }

    fn stop_audio_and_midi(&mut self) {
        self.stop_audio_and_midi_internal(true);
    }

    fn stop_audio_and_midi_internal(&mut self, reset_transport: bool) {
        self.audio_stop.store(true, Ordering::Relaxed);
        self.audio_running = false;
        self.midi_conn = None;
        let _stream = self.audio_stream.take();
        let _input = self.audio_input_stream.take();
        let start = std::time::Instant::now();
        while self.audio_callback_active.load(Ordering::Relaxed) > 0 {
            if start.elapsed() > std::time::Duration::from_millis(1000) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        if reset_transport {
            self.send_midi_stop_to_hosts();
        }
        // Keep the host alive on Stop; dropping here can crash some plugins.
        self.midi_gate.store(false, Ordering::Relaxed);
        if reset_transport {
            self.transport_samples.store(0, Ordering::Relaxed);
        }
        for state in &self.track_audio {
            if let Ok(mut events) = state.midi_events.lock() {
                events.clear();
            }
        }
        if reset_transport {
            self.playhead_beats = 0.0;
            self.last_frame_time = None;
        }
    }

    fn send_midi_stop_to_hosts(&mut self) {
        let channels = self.last_output_channels.max(1);
        let mut buffer = vec![0.0f32; channels];
        let mut events = Vec::with_capacity(16 * 128);
        for channel in 0u8..16 {
            for note in 0u8..=127 {
                events.push(vst3::MidiEvent::note_off_at(channel, note, 0, 0));
            }
        }
        for state in &self.track_audio {
            let Some(host) = state.host.as_ref() else {
                continue;
            };
            if let Ok(mut host) = host.lock() {
                let _ = host.process_f32(&mut buffer, channels, &events);
            }
        }
    }

    fn settings_path(&self) -> &str {
        &self.settings_path
    }

    fn save_settings(&mut self) -> Result<(), String> {
        let json = serde_json::to_string_pretty(&self.settings).map_err(|e| e.to_string())?;
        fs::write(self.settings_path(), json).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn load_settings_or_default(&mut self) {
        let path = self.settings_path.to_string();
        let data = fs::read_to_string(&path).ok();
        if let Some(data) = data {
            if let Ok(settings) = serde_json::from_str::<SettingsState>(&data) {
                self.settings = settings;
                return;
            }
        }
        self.settings = SettingsState::default();
    }

    fn list_output_devices(&self) -> Vec<String> {
        let host = cpal::default_host();
        let mut names = Vec::new();
        if let Ok(devices) = host.output_devices() {
            for dev in devices {
                if let Ok(name) = dev.name() {
                    names.push(name);
                }
            }
        }
        if names.is_empty() {
            names.push("Default".to_string());
        }
        names
    }

    fn list_input_devices(&self) -> Vec<String> {
        let host = cpal::default_host();
        let mut names = Vec::new();
        if let Ok(devices) = host.input_devices() {
            for dev in devices {
                if let Ok(name) = dev.name() {
                    names.push(name);
                }
            }
        }
        if names.is_empty() {
            names.push("Default".to_string());
        }
        names
    }

    fn list_midi_inputs(&self) -> Vec<String> {
        let mut names = Vec::new();
        if let Ok(midi_in) = MidiInput::new("LingStation") {
            for port in midi_in.ports() {
                if let Ok(name) = midi_in.port_name(&port) {
                    names.push(name);
                }
            }
        }
        if names.is_empty() {
            names.push("Default".to_string());
        }
        names
    }
    fn pick_vst_file(&self) -> Option<String> {
        let path = rfd::FileDialog::new()
            .add_filter("VST3", &["vst3"])
            .pick_file();
        path.map(|p| p.to_string_lossy().to_string())
    }

    fn open_plugin_picker(&mut self, target: PluginTarget) {
        self.plugin_target = Some(target);
        if self.plugin_candidates.is_empty() {
            self.plugin_candidates = self.scan_vst3_plugins();
        }
        self.show_plugin_picker = true;
    }

    fn scan_vst3_plugins(&self) -> Vec<String> {
        let root = PathBuf::from("C:\\Program Files\\Common Files\\VST3");
        let local = PathBuf::from("synths");
        let mut found = Vec::new();
        self.scan_dir(&root, &mut found);
        self.scan_dir(&local, &mut found);
        found.sort();
        found
    }

    fn plugin_display_name(path: &str) -> String {
        let candidate = Path::new(path)
            .file_stem()
            .or_else(|| Path::new(path).file_name())
            .and_then(|s| s.to_str())
            .unwrap_or(path);
        candidate.replace('_', " ")
    }

    fn find_vst3_plugin_by_name(&mut self, name: &str) -> Option<String> {
        if self.plugin_candidates.is_empty() {
            self.plugin_candidates = self.scan_vst3_plugins();
        }
        let needle = name.to_ascii_lowercase();
        self.plugin_candidates
            .iter()
            .find(|path| {
                let display = Self::plugin_display_name(path).to_ascii_lowercase();
                display == needle || display.contains(&needle)
            })
            .cloned()
    }

    fn apply_program_param(track: &mut Track) {
        let Some(program) = track.midi_program else {
            return;
        };
        let program_index = track.params.iter().position(|name| {
            let name = name.to_ascii_lowercase();
            name.contains("program") || name.contains("patch") || name.contains("preset")
        });
        if let Some(index) = program_index {
            if let Some(value) = track.param_values.get_mut(index) {
                *value = (program as f32 / 127.0).clamp(0.0, 1.0);
            }
        }
    }

    fn scan_dir(&self, dir: &Path, out: &mut Vec<String>) {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                self.scan_dir(&path, out);
            } else if let Some(ext) = path.extension() {
                if ext.to_ascii_lowercase() == "vst3" {
                    out.push(path.to_string_lossy().to_string());
                }
            }
        }
    }

    fn refresh_track_params(&mut self, index: usize) {
        if let Some(track) = self.tracks.get_mut(index) {
            if let Some(path) = track.instrument_path.as_deref() {
                let params_result = self
                    .track_audio
                    .get(index)
                    .and_then(|state| state.host.as_ref())
                    .and_then(|host| host.try_lock().ok().map(|host| host.enumerate_params()))
                    .map(Ok)
                    .unwrap_or_else(|| vst3::enumerate_params(path));
                match params_result {
                    Ok(params) if !params.is_empty() => {
                        let next_ids: Vec<u32> = params.iter().map(|p| p.id).collect();
                        let reuse_values = track.param_ids == next_ids
                            && track.param_values.len() == params.len();
                        let next_values = if reuse_values {
                            track.param_values.clone()
                        } else {
                            params.iter().map(|p| p.default_value as f32).collect()
                        };
                        track.params = params.iter().map(|p| p.name.clone()).collect();
                        track.param_ids = next_ids;
                        track.param_values = next_values;
                        Self::apply_program_param(track);
                        if track.automation_lanes.is_empty() && !track.automation_channels.is_empty() {
                            let mut lanes = Vec::new();
                            for name in &track.automation_channels {
                                if let Some((idx, param_id)) = track
                                    .params
                                    .iter()
                                    .enumerate()
                                    .find(|(_, n)| *n == name)
                                    .and_then(|(i, _)| track.param_ids.get(i).copied().map(|id| (i, id)))
                                {
                                    let _ = idx;
                                    lanes.push(AutomationLane {
                                        name: name.clone(),
                                        param_id,
                                        target: AutomationTarget::Instrument,
                                        points: Vec::new(),
                                    });
                                }
                            }
                            if !lanes.is_empty() {
                                track.automation_lanes = lanes;
                            }
                        }
                    }
                    Ok(_) => {
                        track.params = default_instrument_params();
                        track.param_ids.clear();
                        track.param_values.clear();
                    }
                    Err(err) => {
                        track.params = default_instrument_params();
                        track.param_ids.clear();
                        track.param_values.clear();
                        self.status = format!("VST3 params unavailable: {err}");
                    }
                }
            } else {
                track.params = default_midi_params();
                track.param_ids.clear();
                track.param_values.clear();
            }
        }
    }

    fn refresh_params_for_selected_track(&mut self, force: bool) {
        let Some(index) = self.selected_track else {
            return;
        };
        if self.last_params_track != Some(index) {
            self.reset_midi_for_selected_track();
        }
        if !force && self.last_params_track == Some(index) {
            return;
        }
        self.refresh_track_params(index);
        self.last_params_track = Some(index);
    }

    fn reset_midi_for_selected_track(&mut self) {
        self.midi_gate.store(false, Ordering::Relaxed);
        let Some(index) = self.selected_track else {
            return;
        };
        if let Some(state) = self.track_audio.get(index) {
            if let Ok(mut events) = state.midi_events.lock() {
                events.clear();
                for note in 0u8..=127 {
                    events.push(vst3::MidiEvent::note_off(0, note, 0));
                }
            }
        }
        self.sync_track_audio_notes(index);
    }

    fn replace_instrument(&mut self, index: usize, path: String) {
        let mut reopen_ui = false;
        if self
            .plugin_ui
            .as_ref()
            .map_or(false, |ui| ui.target == PluginUiTarget::Instrument(index))
        {
            reopen_ui = self.show_plugin_ui;
            self.show_plugin_ui = false;
            self.destroy_plugin_ui();
        }
        let was_running = self.audio_running;
        if was_running {
            self.stop_audio_and_midi();
        }
        if let Some(track) = self.tracks.get_mut(index) {
            track.instrument_path = Some(path);
            track.params = default_instrument_params();
            track.param_ids.clear();
            track.param_values.clear();
        }
        if let Some(state) = self.track_audio.get_mut(index) {
            if let Some(host) = state.host.take() {
                if let Ok(mut host) = host.lock() {
                    host.prepare_for_drop();
                }
                self.orphaned_hosts.push(host);
            }
        }
        if was_running {
            if let Err(err) = self.start_audio_and_midi() {
                self.status = format!("Instrument reload failed: {err}");
            } else {
                self.status = "Instrument reloaded".to_string();
            }
        }
        if reopen_ui {
            self.plugin_ui_target = Some(PluginUiTarget::Instrument(index));
            self.plugin_ui_hidden = false;
            self.show_plugin_ui = true;
        }
        self.refresh_params_for_selected_track(true);
    }

    fn next_clip_id(&self) -> usize {
        self.tracks
            .iter()
            .flat_map(|track| track.clips.iter().map(|clip| clip.id))
            .max()
            .unwrap_or(0)
            + 1
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
fn get_plugin_close_flag(_hwnd: isize) -> Option<&'static AtomicBool> {
    None
}

#[cfg(windows)]
fn pump_plugin_messages(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, PeekMessageW, TranslateMessage, PM_REMOVE, MSG,
    };
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        let target = if hwnd == 0 { 0 } else { 0 };
        while PeekMessageW(&mut msg, target, 0, 0, PM_REMOVE) != 0 {
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

fn try_process_vst3(
    output: &mut [f32],
    channels: usize,
    host: &Option<Arc<Mutex<vst3::Vst3Host>>>,
    midi_events: &Arc<Mutex<Vec<vst3::MidiEvent>>>,
) -> bool {
    let host = match host {
        Some(host) => host,
        None => return false,
    };
    let mut host = match host.lock() {
        Ok(host) => host,
        Err(_) => return false,
    };
    let events = match midi_events.lock() {
        Ok(mut guard) => guard.drain(..).collect::<Vec<_>>(),
        Err(_) => Vec::new(),
    };
    host.process_f32(output, channels, &events).is_ok()
}

fn render_plan_to_wav(
    plan: RenderPlan,
    done: &AtomicU64,
    total: &AtomicU64,
) -> Result<(), String> {
    let channels = 2u16;
    let tempo = plan.tempo_bpm.max(1.0);
    let start_beats = plan.start_beats.max(0.0);
    let end_beats = plan.end_beats.max(start_beats + 0.25);
    let samples_per_beat = plan.sample_rate as f64 * 60.0 / tempo as f64;
    let start_samples = (start_beats as f64 * samples_per_beat).round().max(0.0) as u64;
    let end_samples = (end_beats as f64 * samples_per_beat).round().max(start_samples as f64) as u64;
    let total_samples = end_samples.saturating_sub(start_samples) as usize;
    let total_samples_u64 = total_samples as u64;
    total.store(total_samples_u64.max(1), Ordering::Relaxed);

    let spec = hound::WavSpec {
        channels,
        sample_rate: plan.sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    if let Some(parent) = Path::new(&plan.path).parent() {
        if let Err(err) = fs::create_dir_all(parent) {
            return Err(format!("Render folder create failed: {err}"));
        }
    }
    let file = std::fs::File::create(&plan.path).map_err(|e| e.to_string())?;
    let mut writer = hound::WavWriter::new(file, spec).map_err(|e| e.to_string())?;

    let mut track_hosts: Vec<(RenderTrack, Option<vst3::Vst3Host>, Vec<vst3::Vst3Host>)> = Vec::new();
    if !plan.tracks.is_empty() {
        for track in plan.tracks {
            let host = if let Some(path) = track.instrument_path.as_ref() {
                vst3::Vst3Host::load(
                    path,
                    plan.sample_rate as f64,
                    plan.block_size,
                    channels as usize,
                )
                .ok()
            } else {
                None
            };
            let mut fx_hosts = Vec::new();
            for fx_path in &track.effect_paths {
                if let Ok(fx) = vst3::Vst3Host::load_with_input(
                    fx_path,
                    plan.sample_rate as f64,
                    plan.block_size,
                    channels as usize,
                    channels as usize,
                ) {
                    fx_hosts.push(fx);
                }
            }
            track_hosts.push((track, host, fx_hosts));
        }
    } else {
        let host = if let Some(path) = plan.instrument_path.as_ref() {
            vst3::Vst3Host::load(
                path,
                plan.sample_rate as f64,
                plan.block_size,
                channels as usize,
            )
            .ok()
        } else {
            None
        };
        let single = RenderTrack {
            notes: plan.notes.clone(),
            instrument_path: plan.instrument_path.clone(),
            param_ids: plan.param_ids.clone(),
            param_values: plan.param_values.clone(),
            plugin_state_component: plan.plugin_state_component.clone(),
            plugin_state_controller: plan.plugin_state_controller.clone(),
            effect_paths: Vec::new(),
            effect_bypass: Vec::new(),
            automation_lanes: Vec::new(),
            level: 1.0,
            active: true,
        };
        track_hosts.push((single, host, Vec::new()));
    }

    for (track, host, _) in track_hosts.iter_mut() {
        let Some(host) = host.as_mut() else {
            continue;
        };
        let has_state = track
            .plugin_state_component
            .as_ref()
            .map(|v| !v.is_empty())
            .unwrap_or(false)
            || track
                .plugin_state_controller
                .as_ref()
                .map(|v| !v.is_empty())
                .unwrap_or(false);
        if has_state {
            let _ = host.set_state_bytes(
                track.plugin_state_component.as_deref(),
                track.plugin_state_controller.as_deref(),
            );
        } else if !track.param_ids.is_empty() {
            for (param_id, value) in track.param_ids.iter().zip(track.param_values.iter()) {
                host.push_param_change(*param_id, *value as f64);
            }
        }
    }

    let mut cursor = 0usize;
    while cursor < total_samples {
        let frames = (total_samples - cursor).min(plan.block_size);
        let block_start = start_samples + cursor as u64;
        let block_end = start_samples + (cursor + frames) as u64;
        let mut output = vec![0.0f32; frames * channels as usize];
        let mut temp = vec![0.0f32; frames * channels as usize];
        let mut fx_temp = vec![0.0f32; frames * channels as usize];
        for (track, host, fx_hosts) in track_hosts.iter_mut() {
            if !track.active {
                continue;
            }
            temp.fill(0.0);
            let block_beat = (block_start as f64 / samples_per_beat) as f32;
            for lane in &track.automation_lanes {
                if let Some(value) = DawApp::automation_value_at(&lane.points, block_beat) {
                    match lane.target {
                        AutomationTarget::Instrument => {
                            if let Some(host) = host.as_mut() {
                                host.push_param_change(lane.param_id, value as f64);
                            }
                        }
                        AutomationTarget::Effect(fx_index) => {
                            if let Some(fx) = fx_hosts.get_mut(fx_index) {
                                fx.push_param_change(lane.param_id, value as f64);
                            }
                        }
                    }
                }
            }
            let events = collect_block_events(&track.notes, block_start, block_end, samples_per_beat);
            if let Some(host) = host.as_mut() {
                let _ = host.process_f32(&mut temp, channels as usize, &events);
            }
            let mut current = &mut temp;
            let mut scratch = &mut fx_temp;
            for (fx_index, fx) in fx_hosts.iter_mut().enumerate() {
                if track
                    .effect_bypass
                    .get(fx_index)
                    .copied()
                    .unwrap_or(false)
                {
                    continue;
                }
                scratch.fill(0.0);
                if fx
                    .process_f32_with_input(
                        current.as_slice(),
                        scratch.as_mut_slice(),
                        channels as usize,
                        &events,
                    )
                    .is_ok()
                {
                    std::mem::swap(&mut current, &mut scratch);
                }
            }
            let level = track.level.clamp(0.0, 1.0);
            for (out, sample) in output.iter_mut().zip(current.iter()) {
                *out += *sample * level;
            }
        }
        if !plan.audio_clips.is_empty() {
            for clip in plan.audio_clips.iter() {
                let clip_end = clip.start_samples + clip.length_samples;
                if block_end <= clip.start_samples || block_start >= clip_end {
                    continue;
                }
                let Some(data) = plan.audio_cache.get(&clip.path) else {
                    continue;
                };
                let src_channels = data.channels.max(1);
                let src_frames = data.samples.len() / src_channels;
                if src_frames == 0 {
                    continue;
                }
                let rate_ratio = data.sample_rate as f64 / plan.sample_rate as f64;
                let time_mul = clip.time_mul.max(0.01) as f64;
                let start_in_block = block_start.max(clip.start_samples) - block_start;
                let end_in_block = block_end.min(clip_end) - block_start;
                for i in start_in_block..end_in_block {
                    let clip_pos = i + block_start - clip.start_samples;
                    let pos = ((clip_pos as f64 + clip.offset_samples as f64) * rate_ratio / time_mul)
                        .max(0.0);
                    let len = src_frames as f64;
                    let src_pos = if len > 0.0 { pos % len } else { pos };
                    let base = src_pos.floor() as usize;
                    let frac = (src_pos - base as f64) as f32;
                    let next = (base + 1).min(src_frames.saturating_sub(1));
                    for ch in 0..channels as usize {
                        let src_ch = if src_channels == 1 { 0 } else { ch.min(src_channels - 1) };
                        let idx0 = base * src_channels + src_ch;
                        let idx1 = next * src_channels + src_ch;
                        let s0 = data.samples.get(idx0).copied().unwrap_or(0.0);
                        let s1 = data.samples.get(idx1).copied().unwrap_or(0.0);
                        let sample = s0 + (s1 - s0) * frac;
                        let out_index = i as usize * channels as usize + ch;
                        if out_index < output.len() {
                            output[out_index] += sample * clip.gain;
                        }
                    }
                }
            }
        }
        for sample in output {
            writer.write_sample(sample).map_err(|e| e.to_string())?;
        }
        cursor += frames;
        done.store(cursor as u64, Ordering::Relaxed);
    }

    writer.finalize().map_err(|e| e.to_string())?;
    done.store(total_samples_u64, Ordering::Relaxed);
    Ok(())
}

fn render_plan_for_each_block<F>(
    plan: &RenderPlan,
    done: &AtomicU64,
    progress_offset: u64,
    mut on_block: F,
) -> Result<usize, String>
where
    F: FnMut(&[f32], usize) -> Result<(), String>,
{
    let channels = 2u16;
    let tempo = plan.tempo_bpm.max(1.0);
    let start_beats = plan.start_beats.max(0.0);
    let end_beats = plan.end_beats.max(start_beats + 0.25);
    let samples_per_beat = plan.sample_rate as f64 * 60.0 / tempo as f64;
    let start_samples = (start_beats as f64 * samples_per_beat).round().max(0.0) as u64;
    let end_samples = (end_beats as f64 * samples_per_beat)
        .round()
        .max(start_samples as f64) as u64;
    let total_samples = end_samples.saturating_sub(start_samples) as usize;
    let total_samples_u64 = total_samples as u64;

    let mut track_hosts: Vec<(RenderTrack, Option<vst3::Vst3Host>, Vec<vst3::Vst3Host>)> = Vec::new();
    if !plan.tracks.is_empty() {
        for track in plan.tracks.iter().cloned() {
            let host = if let Some(path) = track.instrument_path.as_ref() {
                vst3::Vst3Host::load(
                    path,
                    plan.sample_rate as f64,
                    plan.block_size,
                    channels as usize,
                )
                .ok()
            } else {
                None
            };
            let mut fx_hosts = Vec::new();
            for fx_path in &track.effect_paths {
                if let Ok(fx) = vst3::Vst3Host::load_with_input(
                    fx_path,
                    plan.sample_rate as f64,
                    plan.block_size,
                    channels as usize,
                    channels as usize,
                ) {
                    fx_hosts.push(fx);
                }
            }
            track_hosts.push((track, host, fx_hosts));
        }
    } else {
        let host = if let Some(path) = plan.instrument_path.as_ref() {
            vst3::Vst3Host::load(
                path,
                plan.sample_rate as f64,
                plan.block_size,
                channels as usize,
            )
            .ok()
        } else {
            None
        };
        let single = RenderTrack {
            notes: plan.notes.clone(),
            instrument_path: plan.instrument_path.clone(),
            param_ids: plan.param_ids.clone(),
            param_values: plan.param_values.clone(),
            plugin_state_component: plan.plugin_state_component.clone(),
            plugin_state_controller: plan.plugin_state_controller.clone(),
            effect_paths: Vec::new(),
            effect_bypass: Vec::new(),
            automation_lanes: Vec::new(),
            level: 1.0,
            active: true,
        };
        track_hosts.push((single, host, Vec::new()));
    }

    for (track, host, _) in track_hosts.iter_mut() {
        let Some(host) = host.as_mut() else {
            continue;
        };
        let has_state = track
            .plugin_state_component
            .as_ref()
            .map(|v| !v.is_empty())
            .unwrap_or(false)
            || track
                .plugin_state_controller
                .as_ref()
                .map(|v| !v.is_empty())
                .unwrap_or(false);
        if has_state {
            let _ = host.set_state_bytes(
                track.plugin_state_component.as_deref(),
                track.plugin_state_controller.as_deref(),
            );
        } else if !track.param_ids.is_empty() {
            for (param_id, value) in track.param_ids.iter().zip(track.param_values.iter()) {
                host.push_param_change(*param_id, *value as f64);
            }
        }
    }

    let mut cursor = 0usize;
    while cursor < total_samples {
        let frames = (total_samples - cursor).min(plan.block_size);
        let block_start = start_samples + cursor as u64;
        let block_end = start_samples + (cursor + frames) as u64;
        let mut output = vec![0.0f32; frames * channels as usize];
        let mut temp = vec![0.0f32; frames * channels as usize];
        let mut fx_temp = vec![0.0f32; frames * channels as usize];
        for (track, host, fx_hosts) in track_hosts.iter_mut() {
            if !track.active {
                continue;
            }
            temp.fill(0.0);
            let block_beat = (block_start as f64 / samples_per_beat) as f32;
            for lane in &track.automation_lanes {
                if let Some(value) = DawApp::automation_value_at(&lane.points, block_beat) {
                    match lane.target {
                        AutomationTarget::Instrument => {
                            if let Some(host) = host.as_mut() {
                                host.push_param_change(lane.param_id, value as f64);
                            }
                        }
                        AutomationTarget::Effect(fx_index) => {
                            if let Some(fx) = fx_hosts.get_mut(fx_index) {
                                fx.push_param_change(lane.param_id, value as f64);
                            }
                        }
                    }
                }
            }
            let events = collect_block_events(&track.notes, block_start, block_end, samples_per_beat);
            if let Some(host) = host.as_mut() {
                let _ = host.process_f32(&mut temp, channels as usize, &events);
            }
            let mut current = &mut temp;
            let mut scratch = &mut fx_temp;
            for (fx_index, fx) in fx_hosts.iter_mut().enumerate() {
                if track
                    .effect_bypass
                    .get(fx_index)
                    .copied()
                    .unwrap_or(false)
                {
                    continue;
                }
                scratch.fill(0.0);
                if fx
                    .process_f32_with_input(
                        current.as_slice(),
                        scratch.as_mut_slice(),
                        channels as usize,
                        &events,
                    )
                    .is_ok()
                {
                    std::mem::swap(&mut current, &mut scratch);
                }
            }
            let level = track.level.clamp(0.0, 1.0);
            for (out, sample) in output.iter_mut().zip(current.iter()) {
                *out += *sample * level;
            }
        }
        if !plan.audio_clips.is_empty() {
            for clip in plan.audio_clips.iter() {
                let clip_end = clip.start_samples + clip.length_samples;
                if block_end <= clip.start_samples || block_start >= clip_end {
                    continue;
                }
                let Some(data) = plan.audio_cache.get(&clip.path) else {
                    continue;
                };
                let src_channels = data.channels.max(1);
                let src_frames = data.samples.len() / src_channels;
                if src_frames == 0 {
                    continue;
                }
                let rate_ratio = data.sample_rate as f64 / plan.sample_rate as f64;
                let time_mul = clip.time_mul.max(0.01) as f64;
                let start_in_block = block_start.max(clip.start_samples) - block_start;
                let end_in_block = block_end.min(clip_end) - block_start;
                for i in start_in_block..end_in_block {
                    let clip_pos = i + block_start - clip.start_samples;
                    let pos = ((clip_pos as f64 + clip.offset_samples as f64) * rate_ratio / time_mul)
                        .max(0.0);
                    let len = src_frames as f64;
                    let src_pos = if len > 0.0 { pos % len } else { pos };
                    let base = src_pos.floor() as usize;
                    let frac = (src_pos - base as f64) as f32;
                    let next = (base + 1).min(src_frames.saturating_sub(1));
                    for ch in 0..channels as usize {
                        let src_ch = if src_channels == 1 { 0 } else { ch.min(src_channels - 1) };
                        let idx0 = base * src_channels + src_ch;
                        let idx1 = next * src_channels + src_ch;
                        let s0 = data.samples.get(idx0).copied().unwrap_or(0.0);
                        let s1 = data.samples.get(idx1).copied().unwrap_or(0.0);
                        let sample = s0 + (s1 - s0) * frac;
                        let out_index = i as usize * channels as usize + ch;
                        if out_index < output.len() {
                            output[out_index] += sample * clip.gain;
                        }
                    }
                }
            }
        }
        on_block(&output, frames)?;
        cursor += frames;
        done.store(progress_offset + cursor as u64, Ordering::Relaxed);
    }

    done.store(progress_offset + total_samples_u64, Ordering::Relaxed);
    Ok(total_samples)
}

fn render_plan_to_f32(
    plan: RenderPlan,
    done: &AtomicU64,
    total: &AtomicU64,
) -> Result<Vec<f32>, String> {
    let channels = 2u16;
    let tempo = plan.tempo_bpm.max(1.0);
    let start_beats = plan.start_beats.max(0.0);
    let end_beats = plan.end_beats.max(start_beats + 0.25);
    let samples_per_beat = plan.sample_rate as f64 * 60.0 / tempo as f64;
    let start_samples = (start_beats as f64 * samples_per_beat).round().max(0.0) as u64;
    let end_samples = (end_beats as f64 * samples_per_beat).round().max(start_samples as f64) as u64;
    let total_samples = end_samples.saturating_sub(start_samples) as usize;
    let total_samples_u64 = total_samples as u64;
    total.store(total_samples_u64.max(1), Ordering::Relaxed);

    let mut track_hosts: Vec<(RenderTrack, Option<vst3::Vst3Host>, Vec<vst3::Vst3Host>)> = Vec::new();
    if !plan.tracks.is_empty() {
        for track in plan.tracks {
            let host = if let Some(path) = track.instrument_path.as_ref() {
                vst3::Vst3Host::load(
                    path,
                    plan.sample_rate as f64,
                    plan.block_size,
                    channels as usize,
                )
                .ok()
            } else {
                None
            };
            let mut fx_hosts = Vec::new();
            for fx_path in &track.effect_paths {
                if let Ok(fx) = vst3::Vst3Host::load_with_input(
                    fx_path,
                    plan.sample_rate as f64,
                    plan.block_size,
                    channels as usize,
                    channels as usize,
                ) {
                    fx_hosts.push(fx);
                }
            }
            track_hosts.push((track, host, fx_hosts));
        }
    } else {
        let host = if let Some(path) = plan.instrument_path.as_ref() {
            vst3::Vst3Host::load(
                path,
                plan.sample_rate as f64,
                plan.block_size,
                channels as usize,
            )
            .ok()
        } else {
            None
        };
        let single = RenderTrack {
            notes: plan.notes.clone(),
            instrument_path: plan.instrument_path.clone(),
            param_ids: plan.param_ids.clone(),
            param_values: plan.param_values.clone(),
            plugin_state_component: plan.plugin_state_component.clone(),
            plugin_state_controller: plan.plugin_state_controller.clone(),
            effect_paths: Vec::new(),
            effect_bypass: Vec::new(),
            automation_lanes: Vec::new(),
            level: 1.0,
            active: true,
        };
        track_hosts.push((single, host, Vec::new()));
    }

    for (track, host, _) in track_hosts.iter_mut() {
        let Some(host) = host.as_mut() else {
            continue;
        };
        let has_state = track
            .plugin_state_component
            .as_ref()
            .map(|v| !v.is_empty())
            .unwrap_or(false)
            || track
                .plugin_state_controller
                .as_ref()
                .map(|v| !v.is_empty())
                .unwrap_or(false);
        if has_state {
            let _ = host.set_state_bytes(
                track.plugin_state_component.as_deref(),
                track.plugin_state_controller.as_deref(),
            );
        } else if !track.param_ids.is_empty() {
            for (param_id, value) in track.param_ids.iter().zip(track.param_values.iter()) {
                host.push_param_change(*param_id, *value as f64);
            }
        }
    }

    let mut output_all = Vec::with_capacity(total_samples * channels as usize);
    let mut cursor = 0usize;
    while cursor < total_samples {
        let frames = (total_samples - cursor).min(plan.block_size);
        let block_start = start_samples + cursor as u64;
        let block_end = start_samples + (cursor + frames) as u64;
        let mut output = vec![0.0f32; frames * channels as usize];
        let mut temp = vec![0.0f32; frames * channels as usize];
        let mut fx_temp = vec![0.0f32; frames * channels as usize];
        for (track, host, fx_hosts) in track_hosts.iter_mut() {
            if !track.active {
                continue;
            }
            temp.fill(0.0);
            let block_beat = (block_start as f64 / samples_per_beat) as f32;
            for lane in &track.automation_lanes {
                if let Some(value) = DawApp::automation_value_at(&lane.points, block_beat) {
                    match lane.target {
                        AutomationTarget::Instrument => {
                            if let Some(host) = host.as_mut() {
                                host.push_param_change(lane.param_id, value as f64);
                            }
                        }
                        AutomationTarget::Effect(fx_index) => {
                            if let Some(fx) = fx_hosts.get_mut(fx_index) {
                                fx.push_param_change(lane.param_id, value as f64);
                            }
                        }
                    }
                }
            }
            let events = collect_block_events(&track.notes, block_start, block_end, samples_per_beat);
            if let Some(host) = host.as_mut() {
                let _ = host.process_f32(&mut temp, channels as usize, &events);
            }
            let mut current = &mut temp;
            let mut scratch = &mut fx_temp;
            for (fx_index, fx) in fx_hosts.iter_mut().enumerate() {
                if track
                    .effect_bypass
                    .get(fx_index)
                    .copied()
                    .unwrap_or(false)
                {
                    continue;
                }
                scratch.fill(0.0);
                if fx
                    .process_f32_with_input(
                        current.as_slice(),
                        scratch.as_mut_slice(),
                        channels as usize,
                        &events,
                    )
                    .is_ok()
                {
                    std::mem::swap(&mut current, &mut scratch);
                }
            }
            let level = track.level.clamp(0.0, 1.0);
            for (out, sample) in output.iter_mut().zip(current.iter()) {
                *out += *sample * level;
            }
        }
        if !plan.audio_clips.is_empty() {
            for clip in plan.audio_clips.iter() {
                let clip_end = clip.start_samples + clip.length_samples;
                if block_end <= clip.start_samples || block_start >= clip_end {
                    continue;
                }
                let Some(data) = plan.audio_cache.get(&clip.path) else {
                    continue;
                };
                let src_channels = data.channels.max(1);
                let src_frames = data.samples.len() / src_channels;
                if src_frames == 0 {
                    continue;
                }
                let rate_ratio = data.sample_rate as f64 / plan.sample_rate as f64;
                let time_mul = clip.time_mul.max(0.01) as f64;
                let start_in_block = block_start.max(clip.start_samples) - block_start;
                let end_in_block = block_end.min(clip_end) - block_start;
                for i in start_in_block..end_in_block {
                    let clip_pos = i + block_start - clip.start_samples;
                    let pos = ((clip_pos as f64 + clip.offset_samples as f64) * rate_ratio / time_mul)
                        .max(0.0);
                    let len = src_frames as f64;
                    let src_pos = if len > 0.0 { pos % len } else { pos };
                    let base = src_pos.floor() as usize;
                    let frac = (src_pos - base as f64) as f32;
                    let next = (base + 1).min(src_frames.saturating_sub(1));
                    for ch in 0..channels as usize {
                        let src_ch = if src_channels == 1 { 0 } else { ch.min(src_channels - 1) };
                        let idx0 = base * src_channels + src_ch;
                        let idx1 = next * src_channels + src_ch;
                        let s0 = data.samples.get(idx0).copied().unwrap_or(0.0);
                        let s1 = data.samples.get(idx1).copied().unwrap_or(0.0);
                        let sample = s0 + (s1 - s0) * frac;
                        let out_index = i as usize * channels as usize + ch;
                        if out_index < output.len() {
                            output[out_index] += sample * clip.gain;
                        }
                    }
                }
            }
        }
        output_all.extend_from_slice(&output);
        cursor += frames;
        done.store(cursor as u64, Ordering::Relaxed);
    }

    done.store(total_samples_u64, Ordering::Relaxed);
    Ok(output_all)
}

fn render_plan_to_ogg(
    plan: RenderPlan,
    done: &AtomicU64,
    total: &AtomicU64,
) -> Result<(), String> {
    let path = plan.path.clone();
    let sample_rate = plan.sample_rate;
    let bitrate = plan.bitrate_kbps;
    let mut samples = render_plan_to_f32(plan, done, total)?;
    if samples.is_empty() {
        return Ok(());
    }
    let channels = 2u32;
    let sample_rate = sample_rate as u64;
    let quality = match bitrate {
        0..=96 => 0.25,
        97..=128 => 0.35,
        129..=192 => 0.5,
        193..=256 => 0.65,
        _ => 0.8,
    };
    let mut encoder = vorbis_encoder::Encoder::new(channels, sample_rate, quality)
        .map_err(|e| format!("Vorbis encoder init failed: {e}"))?;
    let mut pcm_i16 = Vec::with_capacity(samples.len());
    for sample in samples.drain(..) {
        let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        pcm_i16.push(value);
    }
    let data = encoder
        .encode(&pcm_i16)
        .map_err(|e| format!("Vorbis encode failed: {e}"))?;
    let tail = encoder
        .flush()
        .map_err(|e| format!("Vorbis flush failed: {e}"))?;
    let mut file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    use std::io::Write;
    file.write_all(&data).map_err(|e| e.to_string())?;
    file.write_all(&tail).map_err(|e| e.to_string())?;
    Ok(())
}

fn render_plan_to_flac(
    mut plan: RenderPlan,
    done: &AtomicU64,
    total: &AtomicU64,
) -> Result<(), String> {
    use flacenc::component::BitRepr;
    use flacenc::error::Verify;
    use flacenc::bitsink::ByteSink;
    use flacenc::encode_fixed_size_frame;
    use flacenc::component::StreamInfo;
    use flacenc::constant::{MAX_BLOCK_SIZE, MIN_BLOCK_SIZE};
    use flacenc::source::{Context, FrameBuf, Fill};

    let path = plan.path.clone();
    let sample_rate = plan.sample_rate;
    let channels = 2usize;
    let bits_per_sample = 16usize;
    plan.block_size = plan.block_size.clamp(MIN_BLOCK_SIZE, MAX_BLOCK_SIZE);
    let tempo = plan.tempo_bpm.max(1.0);
    let start_beats = plan.start_beats.max(0.0);
    let end_beats = plan.end_beats.max(start_beats + 0.25);
    let samples_per_beat = plan.sample_rate as f64 * 60.0 / tempo as f64;
    let start_samples = (start_beats as f64 * samples_per_beat).round().max(0.0) as u64;
    let end_samples = (end_beats as f64 * samples_per_beat)
        .round()
        .max(start_samples as f64) as u64;
    let expected_samples = end_samples.saturating_sub(start_samples) as u64;
    total.store(expected_samples.saturating_mul(2).max(1), Ordering::Relaxed);
    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|e| format!("FLAC config error: {e:?}"))?;
    let sample_rate = usize::try_from(sample_rate)
        .map_err(|_| "FLAC sample rate out of range".to_string())?;
    let block_size = plan.block_size;

    let mut ctx = Context::new(bits_per_sample, channels);
    let mut framebuf = FrameBuf::with_size(channels, block_size)
        .map_err(|e| format!("FLAC frame buffer error: {e:?}"))?;
    let stream_info_probe = StreamInfo::new(sample_rate, channels, bits_per_sample)
        .map_err(|e| format!("FLAC stream info error: {e:?}"))?;
    let mut min_frame_size = usize::MAX;
    let mut max_frame_size = 0usize;
    let mut min_block_size = usize::MAX;
    let mut max_block_size = 0usize;
    let mut frame_number = 0usize;

    let total_samples = render_plan_for_each_block(
        &plan,
        done,
        0,
        |output, frames| {
            let mut pcm_i32 = Vec::with_capacity(output.len());
            for sample in output {
                let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i32;
                pcm_i32.push(value);
            }
            ctx.fill_interleaved(&pcm_i32)
                .map_err(|e| format!("FLAC md5 update failed: {e}"))?;
            framebuf
                .fill_interleaved(&pcm_i32)
                .map_err(|e| format!("FLAC frame fill failed: {e}"))?;
            let frame = encode_fixed_size_frame(
                &config,
                &framebuf,
                frame_number,
                &stream_info_probe,
            )
            .map_err(|e| format!("FLAC frame encode failed: {e}"))?;
            let frame_size = frame.count_bits() / 8;
            min_frame_size = min_frame_size.min(frame_size);
            max_frame_size = max_frame_size.max(frame_size);
            min_block_size = min_block_size.min(frames);
            max_block_size = max_block_size.max(frames);
            frame_number = frame_number.saturating_add(1);
            Ok(())
        },
    )?;
    if total_samples == 0 {
        return Ok(());
    }

    let mut stream_info = StreamInfo::new(sample_rate, channels, bits_per_sample)
        .map_err(|e| format!("FLAC stream info error: {e:?}"))?;
    let min_block_size = if min_block_size == usize::MAX {
        block_size
    } else {
        min_block_size
    };
    let max_block_size = if max_block_size == 0 { block_size } else { max_block_size };
    let _ = stream_info.set_block_sizes(min_block_size, max_block_size);
    let min_frame_size = if min_frame_size == usize::MAX { 0 } else { min_frame_size };
    let max_frame_size = if max_frame_size == 0 { min_frame_size } else { max_frame_size };
    let _ = stream_info.set_frame_sizes(min_frame_size, max_frame_size);
    stream_info.set_total_samples(ctx.total_samples());
    stream_info.set_md5_digest(&ctx.md5_digest());

    let stream = flacenc::component::Stream::with_stream_info(stream_info.clone());
    let mut sink = ByteSink::new();
    stream
        .write(&mut sink)
        .map_err(|e| format!("FLAC header write failed: {e}"))?;

    let mut file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    use std::io::Write;
    file.write_all(sink.as_slice()).map_err(|e| e.to_string())?;

    let mut framebuf = FrameBuf::with_size(channels, block_size)
        .map_err(|e| format!("FLAC frame buffer error: {e:?}"))?;
    let mut frame_number = 0usize;
    render_plan_for_each_block(
        &plan,
        done,
        total_samples as u64,
        |output, _frames| {
            let mut pcm_i32 = Vec::with_capacity(output.len());
            for sample in output {
                let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i32;
                pcm_i32.push(value);
            }
            framebuf
                .fill_interleaved(&pcm_i32)
                .map_err(|e| format!("FLAC frame fill failed: {e}"))?;
            let frame = encode_fixed_size_frame(
                &config,
                &framebuf,
                frame_number,
                &stream_info,
            )
            .map_err(|e| format!("FLAC frame encode failed: {e}"))?;
            let mut frame_sink = ByteSink::new();
            frame
                .write(&mut frame_sink)
                .map_err(|e| format!("FLAC frame write failed: {e}"))?;
            file.write_all(frame_sink.as_slice()).map_err(|e| e.to_string())?;
            frame_number = frame_number.saturating_add(1);
            Ok(())
        },
    )?;
    Ok(())
}

thread_local! {
    static MIX_TEMP: RefCell<Vec<f32>> = RefCell::new(Vec::new());
    static FX_TEMP: RefCell<Vec<f32>> = RefCell::new(Vec::new());
}

fn mix_track_hosts(
    output: &mut [f32],
    channels: usize,
    sample_rate: f32,
    tempo_bits: &AtomicU32,
    transport_samples: &AtomicU64,
    loop_start_samples: &AtomicU64,
    loop_end_samples: &AtomicU64,
    track_audio: &[TrackAudioState],
    track_mix: &Arc<Mutex<Vec<TrackMixState>>>,
    audio_clips: &Arc<Mutex<Vec<AudioClipRender>>>,
    audio_cache: &Arc<Mutex<HashMap<String, Arc<AudioClipData>>>>,
    smart_disable_plugins: bool,
    smart_suspend_tracks: bool,
) -> bool {
    let frames = output.len() / channels;
    if frames == 0 || channels == 0 {
        return false;
    }
    let bpm = f32::from_bits(tempo_bits.load(Ordering::Relaxed)).max(1.0);
    let samples_per_beat = sample_rate as f64 * 60.0 / bpm as f64;
    let mut block_start = transport_samples.fetch_add(frames as u64, Ordering::Relaxed);
    let mut block_end = block_start + frames as u64;
    let loop_start = loop_start_samples.load(Ordering::Relaxed);
    let loop_end = loop_end_samples.load(Ordering::Relaxed);
    let mut loop_wrapped = false;
    if loop_end > loop_start {
        if block_start >= loop_end || block_end > loop_end {
            block_start = loop_start;
            block_end = block_start + frames as u64;
            transport_samples.store(block_end, Ordering::Relaxed);
            loop_wrapped = true;
        }
    }
    let block_beat = (block_start as f64 / samples_per_beat) as f32;

    let mix_snapshot = track_mix.lock().ok().map(|m| m.clone()).unwrap_or_default();
    let any_solo = mix_snapshot.iter().any(|m| m.solo);
    let track_count = track_audio.len();
    let mut track_has_audio = vec![false; track_count];
    let mut per_track_clips: Vec<Vec<(AudioClipRender, Arc<AudioClipData>)>> =
        vec![Vec::new(); track_count];
    if let Ok(clips) = audio_clips.lock() {
        for clip in clips.iter() {
            if clip.track_index >= track_count {
                continue;
            }
            let clip_end = clip.start_samples + clip.length_samples;
            if block_end <= clip.start_samples || block_start >= clip_end {
                continue;
            }
            track_has_audio[clip.track_index] = true;
            let data = {
                let cache = match audio_cache.lock() {
                    Ok(cache) => cache,
                    Err(_) => continue,
                };
                cache.get(&clip.path).cloned()
            };
            let Some(data) = data else {
                continue;
            };
            per_track_clips[clip.track_index].push((clip.clone(), data));
        }
    }

    let mut per_track_buffers: Vec<Vec<f32>> = vec![vec![0.0; output.len()]; track_count];
    let processed_any = Arc::new(AtomicBool::new(false));

    per_track_buffers
        .par_iter_mut()
        .enumerate()
        .for_each(|(index, temp)| {
            let mix = mix_snapshot.get(index).copied().unwrap_or(TrackMixState {
                muted: false,
                solo: false,
                level: 1.0,
            });
            let state = match track_audio.get(index) {
                Some(state) => state,
                None => return,
            };
            if mix.muted || (any_solo && !mix.solo) {
                state.peak_bits.store(0.0f32.to_bits(), Ordering::Relaxed);
                state.peak_l_bits.store(0.0f32.to_bits(), Ordering::Relaxed);
                state.peak_r_bits.store(0.0f32.to_bits(), Ordering::Relaxed);
                return;
            }

            let notes = match state.clip_notes.lock() {
                Ok(guard) => guard.clone(),
                Err(_) => Vec::new(),
            };
            let has_notes = !notes.is_empty();
            let has_audio = track_has_audio.get(index).copied().unwrap_or(false);
            let learned_map = state
                .learned_cc
                .lock()
                .ok()
                .map(|map| map.clone())
                .unwrap_or_default();
            let automation = state
                .automation_lanes
                .lock()
                .ok()
                .map(|lanes| lanes.clone())
                .unwrap_or_default();
            let bypass = state
                .effect_bypass
                .lock()
                .ok()
                .map(|b| b.clone())
                .unwrap_or_default();
            let queued_len = state
                .midi_events
                .lock()
                .ok()
                .map(|q| q.len())
                .unwrap_or(0);
            let should_suspend = smart_suspend_tracks
                && !has_notes
                && !has_audio
                && queued_len == 0
                && automation.is_empty();
            if should_suspend {
                let blocks = state.silent_blocks.fetch_add(1, Ordering::Relaxed) + 1;
                if blocks >= 4 {
                    state.peak_bits.store(0.0f32.to_bits(), Ordering::Relaxed);
                    state.peak_l_bits.store(0.0f32.to_bits(), Ordering::Relaxed);
                    state.peak_r_bits.store(0.0f32.to_bits(), Ordering::Relaxed);
                    return;
                }
            } else {
                state.silent_blocks.store(0, Ordering::Relaxed);
            }

            temp.fill(0.0);
            let mut track_processed = false;
            if let Some(host) = state.host.as_ref() {
                let mut events =
                    collect_block_events(&notes, block_start, block_end, samples_per_beat);
                if loop_wrapped {
                    events.extend((0u8..=127).map(|note| vst3::MidiEvent::note_off(0, note, 0)));
                }
                if let Ok(mut queued) = state.midi_events.lock() {
                    events.extend(queued.drain(..));
                }
                let pending_params = state
                    .pending_param_changes
                    .lock()
                    .ok()
                    .map(|mut pending| pending.drain(..).collect::<Vec<_>>())
                    .unwrap_or_default();
                if let Ok(mut host) = host.lock() {
                    let mut remaining_params: Vec<PendingParamChange> = Vec::new();
                    for pending in &pending_params {
                        match pending.target {
                            PendingParamTarget::Instrument => {
                                host.push_param_change(pending.param_id, pending.value);
                            }
                            PendingParamTarget::Effect(_) => {
                                remaining_params.push(*pending);
                            }
                        }
                    }
                    for lane in &automation {
                        if let Some(value) = DawApp::automation_value_at(&lane.points, block_beat)
                        {
                            if lane.target == AutomationTarget::Instrument {
                                host.push_param_change(lane.param_id, value as f64);
                            }
                        }
                    }
                    let mut filtered = Vec::with_capacity(events.len());
                    for event in events {
                        match event {
                            vst3::MidiEvent::ControlChange {
                                channel,
                                controller,
                                value,
                            } => {
                                if let Some(param_id) = learned_map.get(&(channel, controller)) {
                                    let norm = (value as f64 / 127.0).clamp(0.0, 1.0);
                                    host.push_param_change(*param_id, norm);
                                } else {
                                    filtered.push(event);
                                }
                            }
                            _ => filtered.push(event),
                        }
                    }
                    if host.process_f32(temp, channels, &filtered).is_ok() {
                        let temp_len = temp.len();
                        let mut scratch: Vec<f32> = vec![0.0; temp_len];
                        let mut use_temp = true;
                        {
                            let mut current: &mut [f32] = temp;
                            let mut scratch_slice: &mut [f32] = &mut scratch;
                            let skip_fx = smart_disable_plugins && !has_notes && !has_audio;
                            for (fx_index, fx_host) in state.effect_hosts.iter().enumerate() {
                                if skip_fx || bypass.get(fx_index).copied().unwrap_or(false) {
                                    continue;
                                }
                                scratch_slice.fill(0.0);
                                if let Ok(mut fx_host) = fx_host.lock() {
                                    let mut still_pending: Vec<PendingParamChange> = Vec::new();
                                    for pending in remaining_params.drain(..) {
                                        match pending.target {
                                            PendingParamTarget::Effect(target_index)
                                                if target_index == fx_index =>
                                            {
                                                fx_host.push_param_change(pending.param_id, pending.value);
                                            }
                                            _ => still_pending.push(pending),
                                        }
                                    }
                                    remaining_params = still_pending;
                                    for lane in &automation {
                                        if let Some(value) =
                                            DawApp::automation_value_at(&lane.points, block_beat)
                                        {
                                            if lane.target == AutomationTarget::Effect(fx_index) {
                                                fx_host.push_param_change(lane.param_id, value as f64);
                                            }
                                        }
                                    }
                                    if fx_host
                                        .process_f32_with_input(current, scratch_slice, channels, &filtered)
                                        .is_ok()
                                    {
                                        std::mem::swap(&mut current, &mut scratch_slice);
                                        use_temp = !use_temp;
                                    }
                                }
                            }
                        }
                        if !remaining_params.is_empty() {
                            if let Ok(mut pending) = state.pending_param_changes.lock() {
                                pending.extend(remaining_params);
                            }
                        }
                        if !use_temp {
                            temp.copy_from_slice(&scratch);
                        }
                        track_processed = true;
                    }
                }
            }

            if let Some(clips) = per_track_clips.get(index) {
                for (clip, data) in clips {
                    let clip_end = clip.start_samples + clip.length_samples;
                    if block_end <= clip.start_samples || block_start >= clip_end {
                        continue;
                    }
                    let src_channels = data.channels.max(1);
                    let src_frames = data.samples.len() / src_channels;
                    if src_frames == 0 {
                        continue;
                    }
                    let rate_ratio = data.sample_rate as f64 / sample_rate as f64;
                    let time_mul = clip.time_mul.max(0.01) as f64;
                    let start_in_block = block_start.max(clip.start_samples) - block_start;
                    let end_in_block = block_end.min(clip_end) - block_start;
                    for i in start_in_block..end_in_block {
                        let clip_pos = i + block_start - clip.start_samples;
                        let pos = ((clip_pos as f64 + clip.offset_samples as f64) * rate_ratio / time_mul)
                            .max(0.0);
                        let src_pos = if src_frames > 0 {
                            let len = src_frames as f64;
                            pos % len
                        } else {
                            pos
                        };
                        let base = src_pos.floor() as usize;
                        let frac = (src_pos - base as f64) as f32;
                        let next = (base + 1).min(src_frames.saturating_sub(1));
                        for ch in 0..channels {
                            let src_ch = if src_channels == 1 { 0 } else { ch.min(src_channels - 1) };
                            let idx0 = base * src_channels + src_ch;
                            let idx1 = next * src_channels + src_ch;
                            let s0 = data.samples.get(idx0).copied().unwrap_or(0.0);
                            let s1 = data.samples.get(idx1).copied().unwrap_or(0.0);
                            let sample = s0 + (s1 - s0) * frac;
                            let out_index = i as usize * channels + ch;
                            if out_index < temp.len() {
                                temp[out_index] += sample * clip.gain;
                            }
                        }
                    }
                    track_processed = true;
                }
            }

            let mut peak_l = 0.0f32;
            let mut peak_r = 0.0f32;
            if channels >= 2 {
                for frame in temp.chunks_exact(channels) {
                    peak_l = peak_l.max(frame[0].abs());
                    peak_r = peak_r.max(frame[1].abs());
                }
            } else {
                for sample in temp.iter() {
                    let v = sample.abs();
                    peak_l = peak_l.max(v);
                    peak_r = peak_r.max(v);
                }
            }
            state.peak_l_bits.store(peak_l.to_bits(), Ordering::Relaxed);
            state.peak_r_bits.store(peak_r.to_bits(), Ordering::Relaxed);
            state.peak_bits.store(peak_l.max(peak_r).to_bits(), Ordering::Relaxed);

            if track_processed {
                processed_any.store(true, Ordering::Relaxed);
            }
        });

    for (index, buffer) in per_track_buffers.iter().enumerate() {
        let mix = mix_snapshot.get(index).copied().unwrap_or(TrackMixState {
            muted: false,
            solo: false,
            level: 1.0,
        });
        if mix.muted || (any_solo && !mix.solo) {
            continue;
        }
        for (out, sample) in output.iter_mut().zip(buffer.iter()) {
            *out += *sample * mix.level;
        }
    }

    processed_any.load(Ordering::Relaxed)
}

fn update_master_peak_f32(output: &[f32], peak_bits: &AtomicU32) {
    let mut peak = 0.0f32;
    for sample in output {
        let value = sample.abs();
        if value > peak {
            peak = value;
        }
    }
    peak_bits.store(peak.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
}

fn update_master_peak_i16(output: &[i16], peak_bits: &AtomicU32) {
    let mut peak = 0.0f32;
    for sample in output {
        let value = (*sample as f32 / i16::MAX as f32).abs();
        if value > peak {
            peak = value;
        }
    }
    peak_bits.store(peak.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
}

fn update_master_peak_u16(output: &[u16], peak_bits: &AtomicU32) {
    let mut peak = 0.0f32;
    for sample in output {
        let value = (*sample as f32 / u16::MAX as f32) * 2.0 - 1.0;
        let abs_value = value.abs();
        if abs_value > peak {
            peak = abs_value;
        }
    }
    peak_bits.store(peak.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
}

fn enqueue_clip_events(
    frames: usize,
    sample_rate: f32,
    tempo_bits: &AtomicU32,
    transport_samples: &AtomicU64,
    clip_notes: &Arc<Mutex<Vec<PianoRollNote>>>,
    midi_events: &Arc<Mutex<Vec<vst3::MidiEvent>>>,
) {
    if frames == 0 || sample_rate <= 0.0 {
        return;
    }
    let bpm = f32::from_bits(tempo_bits.load(Ordering::Relaxed)).max(1.0);
    let samples_per_beat = sample_rate as f64 * 60.0 / bpm as f64;
    let block_start = transport_samples.fetch_add(frames as u64, Ordering::Relaxed);
    let block_end = block_start + frames as u64;

    let notes = match clip_notes.lock() {
        Ok(guard) => guard.clone(),
        Err(_) => return,
    };
    let mut events = match midi_events.lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };

    for note in notes {
        let start_sample = (note.start_beats as f64 * samples_per_beat).round() as u64;
        let end_sample = ((note.start_beats + note.length_beats) as f64 * samples_per_beat)
            .round() as u64;
        if start_sample >= block_start && start_sample < block_end {
            let offset = (start_sample - block_start) as i32;
            events.push(vst3::MidiEvent::note_on_at(
                0,
                note.midi_note,
                100,
                offset,
            ));
        }
        if end_sample >= block_start && end_sample < block_end {
            let offset = (end_sample - block_start) as i32;
            events.push(vst3::MidiEvent::note_off_at(
                0,
                note.midi_note,
                0,
                offset,
            ));
        }
    }
}

fn render_sine<T: cpal::Sample + cpal::FromSample<f32>>(
    output: &mut [T],
    channels: usize,
    sample_rate: f32,
    freq_bits: &AtomicU32,
    gate: &AtomicBool,
) {
    static mut PHASE: f32 = 0.0;
    let freq = f32::from_bits(freq_bits.load(Ordering::Relaxed));
    let active = gate.load(Ordering::Relaxed);
    let step = TAU * freq / sample_rate;
    for frame in output.chunks_mut(channels) {
        let sample = if active {
            unsafe {
                let value = (PHASE).sin() * 0.2;
                PHASE = (PHASE + step) % TAU;
                value
            }
        } else {
            0.0
        };
        let value: T = cpal::Sample::from_sample(sample);
        for out in frame.iter_mut() {
            *out = value;
        }
    }
}

fn collect_block_events(
    notes: &[PianoRollNote],
    block_start: u64,
    block_end: u64,
    samples_per_beat: f64,
) -> Vec<vst3::MidiEvent> {
    let mut events = Vec::new();
    for note in notes {
        let start_sample = (note.start_beats as f64 * samples_per_beat).round() as u64;
        let end_sample = ((note.start_beats + note.length_beats) as f64 * samples_per_beat)
            .round() as u64;
        if start_sample >= block_start && start_sample < block_end {
            let offset = (start_sample - block_start) as i32;
            events.push(vst3::MidiEvent::note_on_at(0, note.midi_note, note.velocity, offset));
        }
        if end_sample >= block_start && end_sample < block_end {
            let offset = (end_sample - block_start) as i32;
            events.push(vst3::MidiEvent::note_off_at(0, note.midi_note, 0, offset));
        }
    }
    events
}

#[derive(Clone, Copy)]
enum RenderFormat {
    Wav,
    Ogg,
    Flac,
}

fn default_midi_params() -> Vec<String> {
    vec![
        "CC1 Modwheel".to_string(),
        "CC7 Volume".to_string(),
        "CC10 Pan".to_string(),
        "CC11 Expression".to_string(),
        "CC64 Sustain".to_string(),
    ]
}

fn gm_program_name(program: u8) -> &'static str {
    const GM_NAMES: [&str; 128] = [
        "Acoustic Grand Piano",
        "Bright Acoustic Piano",
        "Electric Grand Piano",
        "Honky-tonk Piano",
        "Electric Piano 1",
        "Electric Piano 2",
        "Harpsichord",
        "Clavinet",
        "Celesta",
        "Glockenspiel",
        "Music Box",
        "Vibraphone",
        "Marimba",
        "Xylophone",
        "Tubular Bells",
        "Dulcimer",
        "Drawbar Organ",
        "Percussive Organ",
        "Rock Organ",
        "Church Organ",
        "Reed Organ",
        "Accordion",
        "Harmonica",
        "Tango Accordion",
        "Acoustic Guitar (nylon)",
        "Acoustic Guitar (steel)",
        "Electric Guitar (jazz)",
        "Electric Guitar (clean)",
        "Electric Guitar (muted)",
        "Overdriven Guitar",
        "Distortion Guitar",
        "Guitar Harmonics",
        "Acoustic Bass",
        "Electric Bass (finger)",
        "Electric Bass (pick)",
        "Fretless Bass",
        "Slap Bass 1",
        "Slap Bass 2",
        "Synth Bass 1",
        "Synth Bass 2",
        "Violin",
        "Viola",
        "Cello",
        "Contrabass",
        "Tremolo Strings",
        "Pizzicato Strings",
        "Orchestral Harp",
        "Timpani",
        "String Ensemble 1",
        "String Ensemble 2",
        "Synth Strings 1",
        "Synth Strings 2",
        "Choir Aahs",
        "Voice Oohs",
        "Synth Voice",
        "Orchestra Hit",
        "Trumpet",
        "Trombone",
        "Tuba",
        "Muted Trumpet",
        "French Horn",
        "Brass Section",
        "Synth Brass 1",
        "Synth Brass 2",
        "Soprano Sax",
        "Alto Sax",
        "Tenor Sax",
        "Baritone Sax",
        "Oboe",
        "English Horn",
        "Bassoon",
        "Clarinet",
        "Piccolo",
        "Flute",
        "Recorder",
        "Pan Flute",
        "Blown Bottle",
        "Shakuhachi",
        "Whistle",
        "Ocarina",
        "Lead 1 (square)",
        "Lead 2 (sawtooth)",
        "Lead 3 (calliope)",
        "Lead 4 (chiff)",
        "Lead 5 (charang)",
        "Lead 6 (voice)",
        "Lead 7 (fifths)",
        "Lead 8 (bass + lead)",
        "Pad 1 (new age)",
        "Pad 2 (warm)",
        "Pad 3 (polysynth)",
        "Pad 4 (choir)",
        "Pad 5 (bowed)",
        "Pad 6 (metallic)",
        "Pad 7 (halo)",
        "Pad 8 (sweep)",
        "FX 1 (rain)",
        "FX 2 (soundtrack)",
        "FX 3 (crystal)",
        "FX 4 (atmosphere)",
        "FX 5 (brightness)",
        "FX 6 (goblins)",
        "FX 7 (echoes)",
        "FX 8 (sci-fi)",
        "Sitar",
        "Banjo",
        "Shamisen",
        "Koto",
        "Kalimba",
        "Bag pipe",
        "Fiddle",
        "Shanai",
        "Tinkle Bell",
        "Agogo",
        "Steel Drums",
        "Woodblock",
        "Taiko Drum",
        "Melodic Tom",
        "Synth Drum",
        "Reverse Cymbal",
        "Guitar Fret Noise",
        "Breath Noise",
        "Seashore",
        "Bird Tweet",
        "Telephone Ring",
        "Helicopter",
        "Applause",
        "Gunshot",
    ];
    GM_NAMES[program.min(127) as usize]
}

fn gm_drum_kit_name(program: u8) -> Option<&'static str> {
    match program {
        0 => Some("Standard Kit"),
        8 => Some("Room Kit"),
        16 => Some("Power Kit"),
        24 => Some("Electronic Kit"),
        25 => Some("TR-808 Kit"),
        32 => Some("Jazz Kit"),
        40 => Some("Brush Kit"),
        48 => Some("Orchestra Kit"),
        56 => Some("Sound FX Kit"),
        _ => None,
    }
}

fn default_instrument_params() -> Vec<String> {
    vec![
        "Gain".to_string(),
        "Cutoff".to_string(),
        "Resonance".to_string(),
        "Attack".to_string(),
        "Release".to_string(),
    ]
}
