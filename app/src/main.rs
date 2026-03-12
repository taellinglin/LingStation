use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use eframe::egui;
use engine::midi::{export_midi, import_midi_channels, import_midi_tracks, MidiTrackData};
use engine::timeline::PianoRollNote;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use midir::{Ignore, MidiInput, MidiInputConnection};
use rodio::{Decoder, OutputStream, Sink, Source};
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

mod clap_host;
mod vst3;

const BASE_UI_FONT_SIZE: f32 = 9.0;

fn main() -> eframe::Result<()> {
    install_crash_logger();
    init_windows_com();
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1280.0, 800.0]);
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
        font_id.size = BASE_UI_FONT_SIZE;
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

#[derive(Clone, Serialize, Deserialize)]
struct Clip {
    id: usize,
    track: usize,
    start_beats: f32,
    length_beats: f32,
    is_midi: bool,
    #[serde(default)]
    midi_notes: Vec<PianoRollNote>,
    #[serde(default)]
    midi_source_beats: Option<f32>,
    #[serde(default)]
    link_id: Option<usize>,
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
    #[serde(default)]
    instrument_clap_id: Option<String>,
    effect_paths: Vec<String>,
    #[serde(default)]
    effect_clap_ids: Vec<Option<String>>,
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
    #[serde(default)]
    master_settings: MasterCompSettings,
}

#[derive(Serialize, Deserialize)]
struct Vst3PresetFile {
    version: u32,
    name: String,
    plugin: String,
    #[serde(default)]
    param_names: Vec<String>,
    #[serde(default)]
    param_ids: Vec<u32>,
    #[serde(default)]
    param_values: Vec<f32>,
    #[serde(default)]
    component_state: String,
    #[serde(default)]
    controller_state: String,
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
    #[serde(default)]
    recent_projects: Vec<String>,
    #[serde(default)]
    autosave_minutes: u32,
    #[serde(default)]
    load_last_project: bool,
    #[serde(default = "default_startup_sound")]
    play_startup_sound: bool,
    #[serde(default)]
    browser_folders: Vec<String>,
}

fn default_startup_sound() -> bool {
    true
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
            recent_projects: Vec::new(),
            autosave_minutes: 5,
            load_last_project: false,
            play_startup_sound: default_startup_sound(),
            browser_folders: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum PluginTarget {
    Instrument(usize),
    Effect(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PluginKind {
    Vst3,
    Clap,
}

#[derive(Clone, Debug)]
struct PluginCandidate {
    path: String,
    kind: PluginKind,
    clap_id: Option<String>,
    display: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PluginUiTarget {
    Instrument(usize),
    Effect(usize, usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ProjectAction {
    NewProject,
    OpenProject,
    OpenProjectPath(String),
    ImportMidi,
    NewFromTemplate(String),
}

#[derive(Clone, Copy, Debug)]
enum GmCategory {
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
    fn from_program(program: u8) -> Self {
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

struct GmParamValues {
    gain: f32,
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
    cutoff: f32,
    resonance: f32,
    vibrato_rate: f32,
    vibrato_intensity: f32,
    tremolo_rate: f32,
    tremolo_intensity: f32,
}

impl GmParamValues {
    fn from_category(category: GmCategory) -> Self {
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

    fn new(
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

#[derive(Clone)]
struct TrackAudioState {
    host: Option<PluginHostHandle>,
    effect_hosts: Vec<PluginHostHandle>,
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

#[derive(Clone)]
enum PluginHostHandle {
    Vst3(Arc<Mutex<vst3::Vst3Host>>),
    Clap(Arc<Mutex<clap_host::ClapHost>>),
}

impl PluginHostHandle {
    fn enumerate_params(&self) -> Vec<vst3::ParamInfo> {
        match self {
            PluginHostHandle::Vst3(host) => host
                .lock()
                .ok()
                .map(|host| host.enumerate_params())
                .unwrap_or_default(),
            PluginHostHandle::Clap(host) => host
                .lock()
                .ok()
                .map(|mut host| {
                    host.enumerate_params()
                        .into_iter()
                        .map(|param| vst3::ParamInfo {
                            id: param.id,
                            name: param.name,
                            default_value: param.default_value,
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    fn push_param_change(&self, param_id: u32, value: f64) {
        match self {
            PluginHostHandle::Vst3(host) => {
                if let Ok(mut host) = host.lock() {
                    host.push_param_change(param_id, value);
                }
            }
            PluginHostHandle::Clap(host) => {
                if let Ok(mut host) = host.lock() {
                    host.push_param_change(param_id, value);
                }
            }
        }
    }

    fn get_param_normalized(&self, param_id: u32) -> Option<f64> {
        match self {
            PluginHostHandle::Vst3(host) => host.lock().ok().and_then(|host| host.get_param_normalized(param_id)),
            PluginHostHandle::Clap(_) => None,
        }
    }

    fn get_state_bytes(&self) -> (Vec<u8>, Vec<u8>) {
        match self {
            PluginHostHandle::Vst3(host) => host
                .lock()
                .ok()
                .map(|host| host.get_state_bytes())
                .unwrap_or_default(),
            PluginHostHandle::Clap(host) => host
                .lock()
                .ok()
                .map(|mut host| (host.get_state_bytes(), Vec::new()))
                .unwrap_or_default(),
        }
    }

    fn set_state_bytes(
        &self,
        component_state: Option<&[u8]>,
        controller_state: Option<&[u8]>,
    ) -> Result<(), String> {
        match self {
            PluginHostHandle::Vst3(host) => host
                .lock()
                .map_err(|_| "Plugin lock failed".to_string())?
                .set_state_bytes(component_state, controller_state),
            PluginHostHandle::Clap(host) => {
                let bytes = component_state.unwrap_or(&[]);
                host.lock()
                    .map_err(|_| "Plugin lock failed".to_string())?
                    .set_state_bytes(bytes)
            }
        }
    }

    fn prepare_for_drop(&self) {
        match self {
            PluginHostHandle::Vst3(host) => {
                if let Ok(mut host) = host.lock() {
                    host.prepare_for_drop();
                }
            }
            PluginHostHandle::Clap(host) => {
                if let Ok(mut host) = host.lock() {
                    host.prepare_for_drop();
                }
            }
        }
    }

    fn process_f32(
        &self,
        output: &mut [f32],
        channels: usize,
        midi_events: &[vst3::MidiEvent],
    ) -> Result<(), String> {
        match self {
            PluginHostHandle::Vst3(host) => host
                .lock()
                .map_err(|_| "Plugin lock failed".to_string())?
                .process_f32(output, channels, midi_events),
            PluginHostHandle::Clap(host) => host
                .lock()
                .map_err(|_| "Plugin lock failed".to_string())?
                .process_f32(output, channels, midi_events),
        }
    }

    fn process_f32_with_input(
        &self,
        input: &[f32],
        output: &mut [f32],
        channels: usize,
        midi_events: &[vst3::MidiEvent],
    ) -> Result<(), String> {
        match self {
            PluginHostHandle::Vst3(host) => host
                .lock()
                .map_err(|_| "Plugin lock failed".to_string())?
                .process_f32_with_input(input, output, channels, midi_events),
            PluginHostHandle::Clap(host) => host
                .lock()
                .map_err(|_| "Plugin lock failed".to_string())?
                .process_f32_with_input(input, output, channels, midi_events),
        }
    }
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
    wav_bit_depth: RenderWavBitDepth,
    render_tail_mode: RenderTailMode,
    render_release_seconds: f32,
    tracks: Vec<RenderTrack>,
    notes: Vec<PianoRollNote>,
    instrument_path: Option<String>,
    param_ids: Vec<u32>,
    param_values: Vec<f32>,
    plugin_state_component: Option<Vec<u8>>,
    plugin_state_controller: Option<Vec<u8>>,
    audio_clips: Vec<AudioClipRender>,
    audio_cache: HashMap<String, Arc<AudioClipData>>,
    master_settings: MasterCompSettings,
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
    instrument_clap_id: Option<String>,
    param_ids: Vec<u32>,
    param_values: Vec<f32>,
    plugin_state_component: Option<Vec<u8>>,
    plugin_state_controller: Option<Vec<u8>>,
    effect_paths: Vec<String>,
    effect_clap_ids: Vec<Option<String>>,
    effect_bypass: Vec<bool>,
    automation_lanes: Vec<AutomationLane>,
    level: f32,
    active: bool,
}

enum RenderHost {
    Vst3(vst3::Vst3Host),
    Clap(clap_host::ClapHost),
}

impl RenderHost {
    fn push_param_change(&mut self, param_id: u32, value: f64) {
        match self {
            RenderHost::Vst3(host) => host.push_param_change(param_id, value),
            RenderHost::Clap(host) => host.push_param_change(param_id, value),
        }
    }

    fn set_state_bytes(
        &mut self,
        component_state: Option<&[u8]>,
        controller_state: Option<&[u8]>,
    ) -> Result<(), String> {
        match self {
            RenderHost::Vst3(host) => host.set_state_bytes(component_state, controller_state),
            RenderHost::Clap(host) => host.set_state_bytes(component_state.unwrap_or(&[])),
        }
    }

    fn apply_state_for_render(
        &mut self,
        component_state: Option<&[u8]>,
        controller_state: Option<&[u8]>,
    ) -> Result<(), String> {
        match self {
            RenderHost::Vst3(host) => host.apply_state_for_render(component_state, controller_state),
            RenderHost::Clap(host) => host.set_state_bytes(component_state.unwrap_or(&[])),
        }
    }

    fn process_f32(
        &mut self,
        output: &mut [f32],
        channels: usize,
        midi_events: &[vst3::MidiEvent],
    ) -> Result<(), String> {
        match self {
            RenderHost::Vst3(host) => host.process_f32(output, channels, midi_events),
            RenderHost::Clap(host) => host.process_f32(output, channels, midi_events),
        }
    }

    fn process_f32_with_input(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        channels: usize,
        midi_events: &[vst3::MidiEvent],
    ) -> Result<(), String> {
        match self {
            RenderHost::Vst3(host) => host.process_f32_with_input(input, output, channels, midi_events),
            RenderHost::Clap(host) => host.process_f32_with_input(input, output, channels, midi_events),
        }
    }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingParamTarget {
    Instrument,
    Effect(usize),
}

#[derive(Clone, Copy, Debug)]
struct PendingParamChange {
    target: PendingParamTarget,
    param_id: u32,
    value: f64,
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

#[derive(Clone, Serialize, Deserialize)]
struct MasterCompSettings {
    enabled: bool,
    threshold_db: f32,
    ratio: f32,
    attack_ms: f32,
    release_ms: f32,
    makeup_db: f32,
    level: f32,
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

#[derive(Clone, Copy)]
struct MasterCompState {
    gain: f32,
}

impl Default for MasterCompState {
    fn default() -> Self {
        Self { gain: 1.0 }
    }
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
    playback_panic: Arc<AtomicBool>,
    playback_fade_in: Arc<AtomicBool>,
    midi_freq_bits: Arc<AtomicU32>,
    midi_gate: Arc<AtomicBool>,
    tempo_bits: Arc<AtomicU32>,
    transport_samples: Arc<AtomicU64>,
    master_peak_bits: Arc<AtomicU32>,
    master_peak_display: f32,
    master_settings: Arc<Mutex<MasterCompSettings>>,
    master_comp_state: Arc<Mutex<MasterCompState>>,
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
    piano_key_down: Option<u8>,
    piano_lane_mode: PianoLaneMode,
    piano_cc: u8,
    import_path: String,
    export_path: String,
    status: String,
    last_ui_param_change: Option<(u32, f32)>,
    preset_name_buffer: String,
    startup_stream: Option<OutputStream>,
    startup_sink: Option<Sink>,
    settings: SettingsState,
    settings_path: String,
    show_plugin_picker: bool,
    show_plugin_ui: bool,
    plugin_ui_target: Option<PluginUiTarget>,
    project_dirty: bool,
    last_autosave_at: Option<std::time::Instant>,
    show_close_confirm: bool,
    pending_project_action: Option<ProjectAction>,
    pending_exit: bool,
    exit_confirmed: bool,
    show_render_dialog: bool,
    render_format: RenderFormat,
    render_sample_rate: u32,
    render_wav_bit_depth: RenderWavBitDepth,
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
    plugin_candidates: Vec<PluginCandidate>,
    plugin_search: String,
    plugin_target: Option<PluginTarget>,
    show_midi_import: bool,
    midi_import_state: Option<MidiImportState>,
    undo_stack: Vec<UndoState>,
    redo_stack: Vec<UndoState>,
    clip_drag: Option<ClipDragState>,
    track_drag: Option<TrackDragState>,
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
    piano_scale_drag: Option<PianoScaleDragState>,
    piano_tool: PianoTool,
    arranger_snap_beats: f32,
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
    last_viewport_maximized: Option<bool>,
    last_viewport_rect: Option<egui::Rect>,
    pending_startup_maximize: bool,
    seen_nonzero_viewport: bool,
    fs_expanded: HashSet<String>,
    fs_selected: Option<String>,
    browser_expanded: HashSet<String>,
    browser_selected: Option<String>,
    sidebar_tab: SidebarTab,
    fs_drag: Option<FsDragState>,
    loop_start_beats: Option<f32>,
    loop_end_beats: Option<f32>,
    loop_start_samples: Arc<AtomicU64>,
    loop_end_samples: Arc<AtomicU64>,
    orphaned_hosts: Vec<PluginHostHandle>,
    automation_active: Option<(usize, usize)>,
    automation_rows_expanded: HashSet<usize>,
    gm_presets_generated: bool,
}

enum PluginUiEditor {
    Vst3(vst3::Vst3Editor),
    Clap,
}

struct PluginUiHost {
    hwnd: isize,
    child_hwnd: isize,
    editor: PluginUiEditor,
    host: PluginHostHandle,
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
                audio_time_mul: 1.0,
            },
            Clip { id: 2, track: 1, start_beats: 0.0, length_beats: 8.0, is_midi: true, midi_notes: Vec::new(), midi_source_beats: Some(8.0), link_id: None, name: "CatSynth".to_string(), audio_path: None, audio_source_beats: None, audio_offset_beats: 0.0, audio_gain: 1.0, audio_pitch_semitones: 0.0, audio_time_mul: 1.0 },
            Clip { id: 3, track: 2, start_beats: 0.0, length_beats: 8.0, is_midi: true, midi_notes: Vec::new(), midi_source_beats: Some(8.0), link_id: None, name: "SannySynth".to_string(), audio_path: None, audio_source_beats: None, audio_offset_beats: 0.0, audio_gain: 1.0, audio_pitch_semitones: 0.0, audio_time_mul: 1.0 },
            Clip { id: 4, track: 3, start_beats: 0.0, length_beats: 8.0, is_midi: true, midi_notes: Vec::new(), midi_source_beats: Some(8.0), link_id: None, name: "DogSynth".to_string(), audio_path: None, audio_source_beats: None, audio_offset_beats: 0.0, audio_gain: 1.0, audio_pitch_semitones: 0.0, audio_time_mul: 1.0 },
            Clip { id: 5, track: 4, start_beats: 0.0, length_beats: 8.0, is_midi: true, midi_notes: Vec::new(), midi_source_beats: Some(8.0), link_id: None, name: "LingSynth".to_string(), audio_path: None, audio_source_beats: None, audio_offset_beats: 0.0, audio_gain: 1.0, audio_pitch_semitones: 0.0, audio_time_mul: 1.0 },
            Clip { id: 6, track: 5, start_beats: 0.0, length_beats: 8.0, is_midi: true, midi_notes: Vec::new(), midi_source_beats: Some(8.0), link_id: None, name: "MiceSynth".to_string(), audio_path: None, audio_source_beats: None, audio_offset_beats: 0.0, audio_gain: 1.0, audio_pitch_semitones: 0.0, audio_time_mul: 1.0 },
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
            piano_key_down: None,
            piano_lane_mode: PianoLaneMode::Velocity,
            piano_cc: 1,
            import_path: "project.mid".to_string(),
            export_path: "export.mid".to_string(),
            status: "Ready".to_string(),
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
            track_drag: None,
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
            piano_scale_drag: None,
            piano_tool: PianoTool::Pencil,
            arranger_snap_beats: 1.0,
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
            last_viewport_maximized: None,
            last_viewport_rect: None,
            pending_startup_maximize: true,
            seen_nonzero_viewport: false,
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
            gm_presets_generated: false,
        };
        app.load_settings_or_default();
        if app.settings.play_startup_sound {
            if let Err(err) = app.play_startup_sound() {
                app.status = format!("Startup sound failed: {err}");
            }
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
        let viewport = ctx.input(|i| i.viewport().clone());
        let viewport_has_size = viewport
            .outer_rect
            .map(|rect| rect.width() > 0.0 && rect.height() > 0.0)
            .unwrap_or(false);
        if viewport_has_size {
            self.seen_nonzero_viewport = true;
        }
        if self.pending_startup_maximize
            && self.seen_nonzero_viewport
            && viewport.maximized != Some(true)
        {
            self.pending_startup_maximize = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
            ctx.request_repaint();
        }
        if self.last_viewport_maximized != viewport.maximized {
            self.last_viewport_maximized = viewport.maximized;
            ctx.request_repaint();
        }
        if self.last_viewport_rect != viewport.outer_rect {
            self.last_viewport_rect = viewport.outer_rect;
            ctx.request_repaint();
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
        self.update_autosave();
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
                                for rate in [44_100u32, 48_000, 88_200, 96_000, 176_400, 192_000] {
                                    if ui.selectable_label(self.render_sample_rate == rate, format!("{}", rate)).clicked() {
                                        self.render_sample_rate = rate;
                                    }
                                }
                            });
                    });
                    if self.render_format == RenderFormat::Wav {
                        ui.horizontal(|ui| {
                            ui.label("Bit Depth");
                            let label = self.render_wav_bit_depth.label();
                            egui::ComboBox::from_id_source("render_wav_bit_depth")
                                .selected_text(label)
                                .show_ui(ui, |ui| {
                                    for depth in RenderWavBitDepth::all() {
                                        let depth_label = depth.label();
                                        if ui
                                            .selectable_label(self.render_wav_bit_depth == depth, depth_label)
                                            .clicked()
                                        {
                                            self.render_wav_bit_depth = depth;
                                        }
                                    }
                                });
                        });
                    }
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
        let mut hosts: Vec<PluginHostHandle> = Vec::new();
        for state in self.track_audio.iter_mut() {
            if let Some(host) = state.host.take() {
                host.prepare_for_drop();
                hosts.push(host);
            }
            for host in state.effect_hosts.drain(..) {
                host.prepare_for_drop();
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
        if input.modifiers.ctrl && input.modifiers.shift && input.key_pressed(egui::Key::A) {
            self.piano_selected.clear();
            self.selected_clips.clear();
            self.selected_clip = None;
            return;
        }
        if input.modifiers.ctrl && input.key_pressed(egui::Key::A) {
            if self.piano_roll_hovered {
                if let Some(clip_id) = self.selected_clip {
                    if let Some((track_index, clip_index)) = self.find_clip_indices_by_id(clip_id) {
                        self.piano_selected.clear();
                        if let Some(clip) = self
                            .tracks
                            .get(track_index)
                            .and_then(|t| t.clips.get(clip_index))
                        {
                            for index in 0..clip.midi_notes.len() {
                                self.piano_selected.insert(index);
                            }
                        }
                    }
                }
            } else {
                self.selected_clips.clear();
                let mut last_clip = None;
                let mut last_track = None;
                for track in &self.tracks {
                    for clip in &track.clips {
                        self.selected_clips.insert(clip.id);
                        last_clip = Some(clip.id);
                        last_track = Some(clip.track);
                    }
                }
                self.selected_clip = last_clip;
                if let Some(track_index) = last_track {
                    self.selected_track = Some(track_index);
                }
            }
            return;
        }
        let has_piano_selection = !self.piano_selected.is_empty();
        if has_piano_selection {
            if input.key_pressed(egui::Key::Delete) || input.key_pressed(egui::Key::Backspace) {
                if let Some(clip_id) = self.selected_clip {
                    if let Some((track_index, clip_index)) = self.find_clip_indices_by_id(clip_id) {
                        let mut indices: Vec<usize> = self.piano_selected.iter().copied().collect();
                        indices.sort_unstable_by(|a, b| b.cmp(a));
                        self.push_undo_state();
                        if let Some(clip) = self
                            .tracks
                            .get_mut(track_index)
                            .and_then(|t| t.clips.get_mut(clip_index))
                        {
                            for index in indices {
                                if index < clip.midi_notes.len() {
                                    clip.midi_notes.remove(index);
                                }
                            }
                        }
                        self.piano_selected.clear();
                        self.sync_track_audio_notes(track_index);
                    }
                }
                return;
            }
            let nudge_beats = 1.0;
            let nudge_pitch = if input.modifiers.shift { 12 } else { 1 };
            let mut beat_delta = 0.0f32;
            let mut pitch_delta = 0i32;
            if input.key_pressed(egui::Key::ArrowLeft) {
                beat_delta = -nudge_beats;
            } else if input.key_pressed(egui::Key::ArrowRight) {
                beat_delta = nudge_beats;
            } else if input.key_pressed(egui::Key::ArrowUp) {
                pitch_delta = nudge_pitch;
            } else if input.key_pressed(egui::Key::ArrowDown) {
                pitch_delta = -nudge_pitch;
            }
            if (beat_delta.abs() > f32::EPSILON || pitch_delta != 0)
                && self.selected_track.is_some()
            {
                if let Some(clip_id) = self.selected_clip {
                    if let Some((track_index, clip_index)) = self.find_clip_indices_by_id(clip_id) {
                        let mut indices: Vec<usize> = self.piano_selected.iter().copied().collect();
                        indices.sort_unstable();
                        self.push_undo_state();
                        if let Some(clip) = self
                            .tracks
                            .get_mut(track_index)
                            .and_then(|t| t.clips.get_mut(clip_index))
                        {
                            for index in indices {
                                if let Some(note) = clip.midi_notes.get_mut(index) {
                                    if beat_delta.abs() > f32::EPSILON {
                                        note.start_beats = (note.start_beats + beat_delta).max(0.0);
                                    }
                                    if pitch_delta != 0 {
                                        let next_pitch = (note.midi_note as i32 + pitch_delta)
                                            .clamp(0, 127) as u8;
                                        note.midi_note = next_pitch;
                                    }
                                }
                            }
                        }
                        self.sync_track_audio_notes(track_index);
                    }
                }
                return;
            }
        }
        if self.selected_clip.is_some() {
            let nudge_beats = if input.modifiers.shift {
                4.0
            } else {
                self.piano_snap.max(0.25)
            };
            let mut beat_delta = 0.0f32;
            let mut track_delta: i32 = 0;
            if input.key_pressed(egui::Key::ArrowLeft) {
                beat_delta = -nudge_beats;
            } else if input.key_pressed(egui::Key::ArrowRight) {
                beat_delta = nudge_beats;
            } else if input.key_pressed(egui::Key::ArrowUp) {
                track_delta = -1;
            } else if input.key_pressed(egui::Key::ArrowDown) {
                track_delta = 1;
            }
            if beat_delta.abs() > f32::EPSILON || track_delta != 0 {
                if let Some(clip_id) = self.selected_clip {
                    let mut clip_info = None;
                    for (track_index, track) in self.tracks.iter().enumerate() {
                        if let Some(clip) = track.clips.iter().find(|c| c.id == clip_id) {
                            clip_info = Some((
                                track_index,
                                clip.start_beats,
                                clip.length_beats,
                                clip.is_midi,
                            ));
                            break;
                        }
                    }
                    if let Some((track_index, start_beats, _length_beats, is_midi)) = clip_info {
                        let target_track = if track_delta != 0 {
                            let next = track_index as i32 + track_delta;
                            if next >= 0 && next < self.tracks.len() as i32 {
                                next as usize
                            } else {
                                track_index
                            }
                        } else {
                            track_index
                        };
                        let new_start = (start_beats + beat_delta).max(0.0);
                        if target_track != track_index || (new_start - start_beats).abs() > f32::EPSILON {
                            self.push_undo_state();
                            if is_midi
                                && (beat_delta.abs() > f32::EPSILON
                                    || target_track != track_index)
                            {
                                self.shift_clip_notes_by_delta(clip_id, new_start - start_beats);
                            }
                            self.move_clip_by_id(clip_id, target_track, new_start);
                            if is_midi {
                                self.sync_track_audio_notes(track_index);
                                if target_track != track_index {
                                    self.sync_track_audio_notes(target_track);
                                }
                            }
                            self.selected_track = Some(target_track);
                        }
                    }
                }
                return;
            }
        }
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
            if !self.selected_clips.is_empty() {
                let mut ids: Vec<usize> = self.selected_clips.iter().copied().collect();
                ids.sort_unstable();
                self.push_undo_state();
                for clip_id in ids {
                    self.remove_clip_and_notes_by_id(clip_id);
                }
                self.selected_clips.clear();
                self.selected_clip = None;
            } else if let Some(clip_id) = self.selected_clip {
                self.push_undo_state();
                self.remove_clip_and_notes_by_id(clip_id);
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
        if self.last_autosave_at.is_none() {
            self.last_autosave_at = Some(std::time::Instant::now());
        }
    }

    fn clear_dirty(&mut self) {
        self.project_dirty = false;
        self.last_autosave_at = None;
    }

    fn update_autosave(&mut self) {
        let minutes = self.settings.autosave_minutes;
        if minutes == 0 || !self.project_dirty {
            return;
        }
        let interval = std::time::Duration::from_secs(minutes.saturating_mul(60) as u64);
        let now = std::time::Instant::now();
        let last = self.last_autosave_at.unwrap_or(now);
        if now.duration_since(last) >= interval {
            let result = self.save_project();
            if let Err(err) = result {
                self.status = format!("Autosave failed: {err}");
            } else {
                self.status = format!("Autosaved {}", self.project_path);
            }
            self.last_autosave_at = Some(now);
        } else if self.last_autosave_at.is_none() {
            self.last_autosave_at = Some(now);
        }
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
            ProjectAction::OpenProjectPath(path) => {
                if let Err(err) = self.open_project_from_path(&path) {
                    self.status = format!("Open failed: {err}");
                }
            }
            ProjectAction::ImportMidi => {
                if let Err(err) = self.import_midi_dialog() {
                    self.status = format!("Import failed: {err}");
                }
            }
            ProjectAction::NewFromTemplate(path) => {
                if let Err(err) = self.load_template_from_path(&path) {
                    self.status = format!("Template failed: {err}");
                }
            }
        }
    }

    fn sync_track_audio_states(&mut self) {
        self.rebuild_all_track_midi_notes();
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

    fn master_settings_snapshot(&self) -> MasterCompSettings {
        self.master_settings
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default()
    }

    fn rebuild_all_track_midi_notes(&mut self) {
        for index in 0..self.tracks.len() {
            self.rebuild_track_midi_notes(index);
        }
    }

    fn midi_loop_len_for_clip(clip: &Clip) -> Option<f32> {
        if !clip.is_midi {
            return None;
        }
        let loop_len = clip.midi_source_beats.unwrap_or(clip.length_beats);
        if loop_len <= 0.0 {
            return None;
        }
        let clip_start = clip.start_beats;
        let loop_end = clip_start + loop_len;
        let has_outside = clip
            .midi_notes
            .iter()
            .any(|note| note.start_beats < clip_start || note.start_beats >= loop_end);
        if has_outside {
            return None;
        }
        Some(loop_len)
    }

    fn rebuild_track_midi_notes(&mut self, index: usize) {
        let Some(track) = self.tracks.get_mut(index) else {
            return;
        };
        track.midi_notes.clear();
        for clip in &track.clips {
            if !clip.is_midi || clip.midi_notes.is_empty() {
                continue;
            }
            let loop_len = Self::midi_loop_len_for_clip(clip);
            if let Some(loop_len) = loop_len {
                let clip_start = clip.start_beats;
                let clip_end = clip.start_beats + clip.length_beats;
                if clip.length_beats > loop_len + 0.0001 {
                    for note in &clip.midi_notes {
                        let rel = note.start_beats - clip_start;
                        if rel < 0.0 || rel >= loop_len {
                            continue;
                        }
                        let mut t = clip_start + rel;
                        while t < clip_end {
                            let mut cloned = note.clone();
                            cloned.start_beats = t;
                            track.midi_notes.push(cloned);
                            t += loop_len;
                        }
                    }
                    continue;
                }
            }
            track.midi_notes.extend(clip.midi_notes.iter().cloned());
        }
        if !track.midi_notes.is_empty() {
            track.midi_notes.sort_by(|a, b| {
                a.start_beats
                    .partial_cmp(&b.start_beats)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
    }

    fn sync_track_audio_notes(&mut self, index: usize) {
        self.rebuild_track_midi_notes(index);
        if let Some(track) = self.tracks.get(index) {
            if let Some(state) = self.track_audio.get(index) {
                state.sync_notes(track);
            }
        }
    }

    fn selected_track_host(&self) -> Option<PluginHostHandle> {
        let index = self.selected_track?;
        self.track_audio.get(index).and_then(|state| state.host.clone())
    }

    fn ensure_track_host(&mut self, index: usize, channels: usize) -> Option<PluginHostHandle> {
        let path = self.tracks.get(index).and_then(|t| t.instrument_path.clone())?;
        let state = self.track_audio.get_mut(index)?;
        if let Some(host) = state.host.as_ref() {
            return Some(host.clone());
        }
        let kind = Self::plugin_kind_from_path(&path);
        let host = match kind {
            PluginKind::Vst3 => {
                let host = vst3::Vst3Host::load(
                    &path,
                    self.settings.sample_rate as f64,
                    self.settings.buffer_size as usize,
                    channels.max(1),
                )
                .ok()?;
                PluginHostHandle::Vst3(Arc::new(Mutex::new(host)))
            }
            PluginKind::Clap => {
                let clap_id = self
                    .tracks
                    .get(index)
                    .and_then(|t| t.instrument_clap_id.clone())
                    .or_else(|| clap_host::default_plugin_id(&path).ok())?;
                if let Some(track) = self.tracks.get_mut(index) {
                    track.instrument_clap_id = Some(clap_id.clone());
                }
                let host = clap_host::ClapHost::load(
                    &path,
                    &clap_id,
                    self.settings.sample_rate as f64,
                    self.settings.buffer_size as u32,
                    channels.max(1),
                    channels.max(1),
                )
                .ok()?;
                PluginHostHandle::Clap(Arc::new(Mutex::new(host)))
            }
        };
        state.host = Some(host.clone());
        Some(host)
    }

    fn ensure_effect_host(
        &mut self,
        track_index: usize,
        effect_index: usize,
        channels: usize,
    ) -> Option<PluginHostHandle> {
        let state = self.track_audio.get_mut(track_index)?;
        let (paths, clap_ids) = {
            let track = self.tracks.get(track_index)?;
            (track.effect_paths.clone(), track.effect_clap_ids.clone())
        };
        if state.effect_hosts.len() != paths.len() {
            for host in state.effect_hosts.drain(..) {
                host.prepare_for_drop();
                self.orphaned_hosts.push(host);
            }
            for (slot, path) in paths.iter().enumerate() {
                let kind = Self::plugin_kind_from_path(path);
                let host = match kind {
                    PluginKind::Vst3 => vst3::Vst3Host::load_with_input(
                        path,
                        self.settings.sample_rate as f64,
                        self.settings.buffer_size as usize,
                        channels,
                        channels,
                    )
                    .ok()
                    .map(|host| PluginHostHandle::Vst3(Arc::new(Mutex::new(host)))),
                    PluginKind::Clap => {
                        let clap_id = clap_ids
                            .get(slot)
                            .and_then(|id| id.clone())
                            .or_else(|| clap_host::default_plugin_id(path).ok());
                        clap_id.and_then(|clap_id| {
                            if let Some(track) = self.tracks.get_mut(track_index) {
                                if track.effect_clap_ids.len() < paths.len() {
                                    track.effect_clap_ids.resize(paths.len(), None);
                                }
                                track.effect_clap_ids[slot] = Some(clap_id.clone());
                            }
                            clap_host::ClapHost::load(
                                path,
                                &clap_id,
                                self.settings.sample_rate as f64,
                                self.settings.buffer_size as u32,
                                channels,
                                channels,
                            )
                            .ok()
                            .map(|host| PluginHostHandle::Clap(Arc::new(Mutex::new(host))))
                        })
                    }
                };
                if let Some(host) = host {
                    state.effect_hosts.push(host);
                }
            }
        }
        state.effect_hosts.get(effect_index).cloned()
    }

    fn draw_effect_params_panel(
        &mut self,
        ui: &mut egui::Ui,
        track_index: usize,
        track_color: Option<egui::Color32>,
        pending_automation_record: &mut Vec<(usize, RecordedAutomationPoint)>,
    ) {
        let (effect_paths, needs_params) = if let Some(track) = self.tracks.get(track_index) {
            let paths = track.effect_paths.clone();
            let needs = paths
                .iter()
                .enumerate()
                .map(|(fx_index, _)| {
                    track
                        .effect_params
                        .get(fx_index)
                        .map(|p| p.is_empty())
                        .unwrap_or(true)
                })
                .collect::<Vec<_>>();
            (paths, needs)
        } else {
            return;
        };
        let mut fx_updates: Vec<(usize, Vec<String>, Vec<u32>, Vec<f32>)> = Vec::new();
        for (fx_index, _) in effect_paths.iter().enumerate() {
            if !needs_params.get(fx_index).copied().unwrap_or(true) {
                continue;
            }
            if let Some(host) = self.ensure_effect_host(track_index, fx_index, 2) {
                let params = host.enumerate_params();
                if !params.is_empty() {
                    let names = params.iter().map(|p| p.name.clone()).collect();
                    let ids = params.iter().map(|p| p.id).collect();
                    let values = params.iter().map(|p| p.default_value as f32).collect();
                    fx_updates.push((fx_index, names, ids, values));
                }
            }
        }
        if let Some(track) = self.tracks.get_mut(track_index) {
            for (fx_index, names, ids, values) in fx_updates {
                if track.effect_params.len() <= fx_index {
                    track.effect_params.resize(fx_index + 1, Vec::new());
                    track.effect_param_ids.resize(fx_index + 1, Vec::new());
                    track.effect_param_values.resize(fx_index + 1, Vec::new());
                }
                track.effect_params[fx_index] = names;
                track.effect_param_ids[fx_index] = ids;
                if track.effect_param_values[fx_index].is_empty() {
                    track.effect_param_values[fx_index] = values;
                }
            }
            ui.separator();
            ui.label("Effects Params");
            if track.effect_paths.is_empty() {
                ui.label("(no effects on this track)");
                return;
            }
            let menu_color = track_color.unwrap_or(egui::Color32::from_rgb(120, 160, 220));
            for (fx_index, fx_path) in track.effect_paths.iter().enumerate() {
                let title = format!(
                    "FX {}: {}",
                    fx_index + 1,
                    Self::plugin_display_name(fx_path)
                );
                egui::CollapsingHeader::new(title)
                    .default_open(true)
                    .show(ui, |ui| {
                    if ui
                        .add(egui::Button::image(
                            egui::Image::new(egui::include_image!("../../icons/eye.svg"))
                                .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                        ))
                        .on_hover_text("Open UI")
                        .clicked()
                    {
                        self.plugin_ui_target = Some(PluginUiTarget::Effect(track_index, fx_index));
                        self.show_plugin_ui = true;
                    }
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
                                    Self::colored_slider(ui, value, 0.0..=1.0, track_color)
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
                                if let Some(state) = self.track_audio.get(track_index) {
                                    if state.effect_hosts.get(fx_index).is_some() {
                                        if let Ok(mut pending) = state.pending_param_changes.lock() {
                                            pending.push(PendingParamChange {
                                                target: PendingParamTarget::Effect(fx_index),
                                                param_id,
                                                value: *value as f64,
                                            });
                                        }
                                    }
                                }
                                if self.is_recording && self.record_automation {
                                    pending_automation_record.push((
                                        track_index,
                                        RecordedAutomationPoint {
                                            param_id,
                                            target: AutomationTarget::Effect(fx_index),
                                            beat: self.playhead_beats,
                                            value: *value,
                                        },
                                    ));
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
                                if let Some(param_id) = ids.get(param_index).copied() {
                                    if let Ok(mut learn) = self.midi_learn.lock() {
                                        *learn = Some((track_index, param_id));
                                    }
                                    self.status = format!("MIDI Learn armed for {}", label);
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
                                if let Some(param_id) = ids.get(param_index).copied() {
                                    if !track.automation_lanes.iter().any(|l| {
                                        l.param_id == param_id
                                            && l.target == AutomationTarget::Effect(fx_index)
                                    }) {
                                        track.automation_lanes.push(AutomationLane {
                                            name: format!(
                                                "{}: {}",
                                                Self::plugin_display_name(fx_path),
                                                label
                                            ),
                                            param_id,
                                            target: AutomationTarget::Effect(fx_index),
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

    fn remove_clip_and_notes_by_id(&mut self, clip_id: usize) -> Option<Clip> {
        for (track_index, track) in self.tracks.iter_mut().enumerate() {
            if let Some(pos) = track.clips.iter().position(|c| c.id == clip_id) {
                let clip = track.clips.remove(pos);
                if clip.is_midi {
                    self.sync_track_audio_notes(track_index);
                    self.send_all_notes_off(track_index);
                }
                return Some(clip);
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

    fn next_clip_link_id(&self) -> usize {
        self.tracks
            .iter()
            .flat_map(|track| track.clips.iter().filter_map(|clip| clip.link_id))
            .max()
            .unwrap_or(0)
            + 1
    }

    fn ensure_clip_link_id(&mut self, track_index: usize, clip_id: usize) -> Option<usize> {
        let existing = self
            .tracks
            .get(track_index)
            .and_then(|track| track.clips.iter().find(|c| c.id == clip_id))
            .and_then(|clip| clip.link_id);
        if let Some(link_id) = existing {
            return Some(link_id);
        }
        let new_id = self.next_clip_link_id();
        if let Some(track) = self.tracks.get_mut(track_index) {
            if let Some(clip) = track.clips.iter_mut().find(|c| c.id == clip_id) {
                clip.link_id = Some(new_id);
            }
        }
        Some(new_id)
    }

    fn unique_clip_name(&self, track_index: usize, base: &str, exclude_id: usize) -> String {
        let base = if base.trim().is_empty() { "Clip" } else { base.trim() };
        let mut suffix = 2usize;
        loop {
            let candidate = format!("{} {}", base, suffix);
            let exists = self
                .tracks
                .get(track_index)
                .map(|track| {
                    track
                        .clips
                        .iter()
                        .any(|c| c.id != exclude_id && c.name.eq_ignore_ascii_case(&candidate))
                })
                .unwrap_or(false);
            if !exists {
                return candidate;
            }
            suffix = suffix.saturating_add(1);
        }
    }

    fn make_clip_unique(&mut self, track_index: usize, clip_id: usize) {
        let needs_update = self
            .tracks
            .get(track_index)
            .and_then(|track| track.clips.iter().find(|c| c.id == clip_id))
            .map(|clip| clip.link_id.is_some())
            .unwrap_or(false);
        if !needs_update {
            return;
        }
        let current_name = self
            .tracks
            .get(track_index)
            .and_then(|track| track.clips.iter().find(|c| c.id == clip_id))
            .map(|clip| clip.name.clone())
            .unwrap_or_else(|| "Clip".to_string());
        let next_name = self.unique_clip_name(track_index, &current_name, clip_id);
        if let Some(track) = self.tracks.get_mut(track_index) {
            if let Some(clip) = track.clips.iter_mut().find(|c| c.id == clip_id) {
                clip.name = next_name;
                clip.link_id = None;
            }
        }
        self.mark_dirty();
    }

    fn sync_linked_clips_for_clip(&mut self, track_index: usize, source_clip_id: usize) {
        let (link_id, source_start, source_len, notes_snapshot) = {
            let track = match self.tracks.get(track_index) {
                Some(track) => track,
                None => return,
            };
            let Some(source_clip) = track.clips.iter().find(|c| c.id == source_clip_id) else {
                return;
            };
            let Some(link_id) = source_clip.link_id else {
                return;
            };
            (
                link_id,
                source_clip.start_beats,
                source_clip.length_beats,
                source_clip.midi_notes.clone(),
            )
        };

        let source_end = source_start + source_len;
        let mut pattern_notes: Vec<PianoRollNote> = Vec::new();
        for note in &notes_snapshot {
            let note_end = note.start_beats + note.length_beats;
            if note.start_beats < source_end && note_end > source_start {
                let mut relative = note.clone();
                relative.start_beats = (relative.start_beats - source_start).max(0.0);
                pattern_notes.push(relative);
            }
        }
        if pattern_notes.is_empty() {
            return;
        }

        let Some(track) = self.tracks.get_mut(track_index) else {
            return;
        };
        for target in track
            .clips
            .iter_mut()
            .filter(|c| c.link_id == Some(link_id) && c.id != source_clip_id)
        {
            let target_start = target.start_beats;
            let mut shifted_notes = Vec::new();
            for note in &pattern_notes {
                let mut shifted = note.clone();
                shifted.start_beats = (shifted.start_beats + target_start).max(0.0);
                shifted_notes.push(shifted);
            }
            target.midi_notes = shifted_notes;
        }
        self.sync_track_audio_notes(track_index);
        self.mark_dirty();
    }

    fn sync_linked_notes_after_edit(&mut self, track_index: usize) {
        let Some(clip_id) = self.selected_clip else {
            return;
        };
        let mut found_track = None;
        for (ti, track) in self.tracks.iter().enumerate() {
            if track.clips.iter().any(|c| c.id == clip_id) {
                found_track = Some(ti);
                break;
            }
        }
        if found_track == Some(track_index) {
            self.sync_linked_clips_for_clip(track_index, clip_id);
        }
    }

    fn clone_clips_by_ids(&mut self, clip_ids: &[usize]) {
        let mut copies: Vec<(Clip, usize)> = Vec::new();
        for clip_id in clip_ids {
            for (track_index, track) in self.tracks.iter().enumerate() {
                if let Some(clip) = track.clips.iter().find(|c| c.id == *clip_id) {
                    copies.push((clip.clone(), track_index));
                    break;
                }
            }
        }
        if copies.is_empty() {
            return;
        }
        self.push_undo_state();
        let mut new_ids = Vec::new();
        let mut last_track = None;
        for (mut clip, track_index) in copies {
            let link_id = self.ensure_clip_link_id(track_index, clip.id);
            let new_id = self.next_clip_id();
            clip.id = new_id;
            clip.track = track_index;
            clip.link_id = link_id;
            if let Some(track) = self.tracks.get_mut(track_index) {
                track.clips.push(clip.clone());
            }
            if clip.is_midi {
                self.sync_track_audio_notes(track_index);
            }
            new_ids.push(new_id);
            last_track = Some(track_index);
        }
        self.selected_clips.clear();
        for id in &new_ids {
            self.selected_clips.insert(*id);
        }
        self.selected_clip = new_ids.last().copied();
        if let Some(track_index) = last_track {
            self.selected_track = Some(track_index);
        }
        self.refresh_params_for_selected_track(false);
    }

    fn can_merge_selected_clips(&self) -> bool {
        if self.selected_clips.len() < 2 {
            return false;
        }
        let mut clips: Vec<Clip> = Vec::new();
        let mut track_index: Option<usize> = None;
        for clip_id in &self.selected_clips {
            let mut found = None;
            for (ti, track) in self.tracks.iter().enumerate() {
                if let Some(clip) = track.clips.iter().find(|c| c.id == *clip_id) {
                    found = Some((ti, clip.clone()));
                    break;
                }
            }
            let Some((ti, clip)) = found else {
                return false;
            };
            if !clip.is_midi {
                return false;
            }
            if let Some(expected) = track_index {
                if expected != ti {
                    return false;
                }
            } else {
                track_index = Some(ti);
            }
            clips.push(clip);
        }
        clips.sort_by(|a, b| a.start_beats.partial_cmp(&b.start_beats).unwrap());
        for pair in clips.windows(2) {
            let prev = &pair[0];
            let next = &pair[1];
            let prev_end = prev.start_beats + prev.length_beats;
            if (next.start_beats - prev_end).abs() > 0.001 {
                return false;
            }
        }
        true
    }

    fn merge_selected_clips(&mut self) {
        if !self.can_merge_selected_clips() {
            return;
        }
        let mut clips: Vec<Clip> = Vec::new();
        let mut track_index: Option<usize> = None;
        for clip_id in &self.selected_clips {
            let mut found = None;
            for (ti, track) in self.tracks.iter().enumerate() {
                if let Some(clip) = track.clips.iter().find(|c| c.id == *clip_id) {
                    found = Some((ti, clip.clone()));
                    break;
                }
            }
            let Some((ti, clip)) = found else {
                return;
            };
            track_index = track_index.or(Some(ti));
            clips.push(clip);
        }
        let Some(track_index) = track_index else {
            return;
        };
        clips.sort_by(|a, b| a.start_beats.partial_cmp(&b.start_beats).unwrap());
        let first = clips.first().cloned();
        let last = clips.last().cloned();
        let (Some(first), Some(last)) = (first, last) else {
            return;
        };
        let start = first.start_beats;
        let end = last.start_beats + last.length_beats;
        let mut merged = first.clone();
        merged.id = self.next_clip_id();
        merged.track = track_index;
        merged.start_beats = start;
        merged.length_beats = (end - start).max(0.0);
        merged.name = if merged.name.trim().is_empty() {
            "Merged".to_string()
        } else {
            merged.name.clone()
        };
        if merged.is_midi {
            let mut merged_notes: Vec<PianoRollNote> = Vec::new();
            for clip in &clips {
                merged_notes.extend(clip.midi_notes.iter().cloned());
            }
            if !merged_notes.is_empty() {
                merged_notes.sort_by(|a, b| {
                    a.start_beats
                        .partial_cmp(&b.start_beats)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
            merged.midi_notes = merged_notes;
            merged.midi_source_beats = Some(merged.length_beats.max(0.25));
        }
        self.push_undo_state();
        for clip in clips {
            self.remove_clip_by_id(clip.id);
        }
        if let Some(track) = self.tracks.get_mut(track_index) {
            track.clips.push(merged.clone());
        }
        if merged.is_midi {
            self.sync_track_audio_notes(track_index);
        }
        self.selected_clips.clear();
        self.selected_clips.insert(merged.id);
        self.selected_clip = Some(merged.id);
        self.selected_track = Some(track_index);
        self.refresh_params_for_selected_track(false);
    }

    fn crop_clip_notes_to_clip_range(&mut self, clip_id: usize, new_start: f32, new_len: f32) {
        let new_end = new_start + new_len;
        let Some((track_index, clip_index)) = self.find_clip_indices_by_id(clip_id) else {
            return;
        };
        let Some(clip) = self
            .tracks
            .get_mut(track_index)
            .and_then(|t| t.clips.get_mut(clip_index))
        else {
            return;
        };
        let mut index = 0usize;
        while index < clip.midi_notes.len() {
            let note = &mut clip.midi_notes[index];
            let note_end = note.start_beats + note.length_beats;
            if note_end <= new_start || note.start_beats >= new_end {
                clip.midi_notes.remove(index);
                continue;
            }
            let clamped_start = note.start_beats.max(new_start);
            let clamped_end = note_end.min(new_end);
            let next_len = clamped_end - clamped_start;
            if next_len <= 0.0 {
                clip.midi_notes.remove(index);
                continue;
            }
            note.start_beats = clamped_start;
            note.length_beats = next_len;
            index += 1;
        }
        self.sync_track_audio_notes(track_index);
        self.send_all_notes_off(track_index);
    }

    fn send_all_notes_off(&self, track_index: usize) {
        let Some(state) = self.track_audio.get(track_index) else {
            return;
        };
        if let Ok(mut events) = state.midi_events.lock() {
            events.extend((0u8..=127).map(|note| vst3::MidiEvent::note_off(0, note, 0)));
        }
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

    fn find_clip_indices_by_id(&self, clip_id: usize) -> Option<(usize, usize)> {
        for (track_index, track) in self.tracks.iter().enumerate() {
            if let Some(clip_index) = track.clips.iter().position(|c| c.id == clip_id) {
                return Some((track_index, clip_index));
            }
        }
        None
    }

    fn shift_clip_notes_by_delta(&mut self, clip_id: usize, delta_beats: f32) {
        if delta_beats.abs() <= f32::EPSILON {
            return;
        }
        let Some((track_index, clip_index)) = self.find_clip_indices_by_id(clip_id) else {
            return;
        };
        if let Some(clip) = self.tracks.get_mut(track_index).and_then(|t| t.clips.get_mut(clip_index)) {
            for note in &mut clip.midi_notes {
                note.start_beats = (note.start_beats + delta_beats).max(0.0);
            }
        }
        self.sync_track_audio_notes(track_index);
    }

    fn remap_track_index(index: usize, from: usize, to: usize) -> usize {
        if from == to {
            return index;
        }
        if index == from {
            return to;
        }
        if from < to {
            if index > from && index <= to {
                return index - 1;
            }
        } else if index >= to && index < from {
            return index + 1;
        }
        index
    }

    fn move_track_order(&mut self, from: usize, to: usize) {
        if from == to || from >= self.tracks.len() || to >= self.tracks.len() {
            return;
        }
        self.push_undo_state();

        let track = self.tracks.remove(from);
        self.tracks.insert(to, track);
        if from < self.track_audio.len() {
            let state = self.track_audio.remove(from);
            if to <= self.track_audio.len() {
                self.track_audio.insert(to, state);
            } else {
                self.track_audio.push(state);
            }
        }

        for (index, track) in self.tracks.iter_mut().enumerate() {
            for clip in &mut track.clips {
                clip.track = index;
            }
        }

        if let Some(selected) = self.selected_track {
            self.selected_track = Some(Self::remap_track_index(selected, from, to));
        }
        if let Some((track_index, lane_index)) = self.automation_active {
            let new_index = Self::remap_track_index(track_index, from, to);
            self.automation_active = Some((new_index, lane_index));
        }
        let mut remapped = HashSet::new();
        for index in &self.automation_rows_expanded {
            remapped.insert(Self::remap_track_index(*index, from, to));
        }
        self.automation_rows_expanded = remapped;

        if let Ok(mut learn) = self.midi_learn.lock() {
            if let Some((track_index, param_id)) = learn.take() {
                let new_index = Self::remap_track_index(track_index, from, to);
                *learn = Some((new_index, param_id));
            }
        }

        if let Ok(mut recording) = self.recording.lock() {
            if recording.track_index != usize::MAX {
                recording.track_index = Self::remap_track_index(recording.track_index, from, to);
            }
        }

        self.sync_track_mix();
        self.sync_selected_track_index();
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
        let params = host.enumerate_params();
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
        clip: &Clip,
        clip_left: f32,
        beat_width: f32,
    ) {
        let notes = &clip.midi_notes;
        if notes.is_empty() {
            return;
        }
        let clip_start = clip.start_beats;
        let clip_len = clip.length_beats.max(0.001);
        let loop_len = self.clip_loop_len_beats(clip).unwrap_or(clip_len);
        let painter = painter.with_clip_rect(rect);
        let mut min_note: Option<u8> = None;
        let mut max_note: Option<u8> = None;
        for note in notes {
            if note.start_beats + note.length_beats < clip_start {
                continue;
            }
            if note.start_beats > clip_start + clip_len {
                continue;
            }
            min_note = Some(min_note.map_or(note.midi_note, |v| v.min(note.midi_note)));
            max_note = Some(max_note.map_or(note.midi_note, |v| v.max(note.midi_note)));
        }
        let (min_note, max_note) = match (min_note, max_note) {
            (Some(min_note), Some(max_note)) => (min_note, max_note),
            _ => return,
        };
        let row_count = (max_note.saturating_sub(min_note) as f32 + 1.0).max(1.0);
        let note_height = (rect.height() / row_count).max(1.0);
        let clip_end = clip_start + clip_len;
        for (index, note) in notes.iter().enumerate() {
            let rel = note.start_beats - clip_start;
            if rel < 0.0 || rel >= loop_len {
                continue;
            }
            let mut t = clip_start + rel;
            while t < clip_end {
                let note_end = t + note.length_beats;
                if note_end < clip_start || t > clip_end {
                    t += loop_len;
                    continue;
                }
                let local_start = (t - clip_start).max(0.0);
                let local_len = note.length_beats.min(clip_len - local_start).max(0.0);
                let x = clip_left + local_start * beat_width;
                let w = (local_len * beat_width).max(2.0);
                let row_index = note.midi_note.saturating_sub(min_note) as f32;
                let y = rect.bottom() - (row_index + 1.0) * note_height;
                let note_rect = egui::Rect::from_min_size(
                    egui::pos2(x, y),
                    egui::vec2(w, (note_height * 0.9).max(1.0)),
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
                t += loop_len;
            }
        }
    }

    fn audio_source_beats(&self, clip: &Clip) -> Option<f32> {
        let tempo = self.tempo_bpm.max(1.0);
        self.get_waveform_seconds_for_clip(clip)
            .map(|seconds| (seconds * tempo / 60.0).max(0.001))
            .or_else(|| clip.audio_source_beats.map(|beats| beats.max(0.001)))
    }

    fn clip_loop_len_beats(&self, clip: &Clip) -> Option<f32> {
        if clip.is_midi {
            return Self::midi_loop_len_for_clip(clip);
        }
        let time_mul = clip.audio_time_mul.max(0.01);
        let source_beats = self.audio_source_beats(clip)?;
        let loop_len = source_beats * time_mul;
        if loop_len > 0.0 {
            Some(loop_len)
        } else {
            None
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
            for index in 0..count {
                let amp = if let Some((row_left, beat_width)) = timeline {
                    let x = rect.left() + index as f32 * step;
                    let beat = (x - row_left) / beat_width;
                    let local_beat = beat - clip.start_beats;
                    if local_beat < 0.0 || local_beat > clip_len {
                        0.0
                    } else {
                        let mut src_beat = (offset_beats + local_beat) / time_mul;
                        if source_beats > 0.0 {
                            src_beat = src_beat.rem_euclid(source_beats);
                        }
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
                } else {
                    let t = if count > 1 {
                        index as f32 / (count as f32 - 1.0)
                    } else {
                        0.0
                    };
                    let mut src_beat = (offset_beats + t * clip_len) / time_mul;
                    if source_beats > 0.0 {
                        src_beat = src_beat.rem_euclid(source_beats);
                    }
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
                            let mut src_beat = (offset_beats + local_beat) / time_mul;
                            if source_beats > 0.0 {
                                src_beat = src_beat.rem_euclid(source_beats);
                            }
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
                    } else {
                        let t = if bands.len() > 1 {
                            index as f32 / (bands.len() as f32 - 1.0)
                        } else {
                            0.0
                        };
                        let mut src_beat = (offset_beats + t * clip_len) / time_mul;
                        if source_beats > 0.0 {
                            src_beat = src_beat.rem_euclid(source_beats);
                        }
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
                if let Some(seconds) = Self::audio_length_seconds(&path) {
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
        let (samples, channels, sample_rate) = Self::decode_audio_samples(path)?;
        if sample_rate == 0 || channels == 0 {
            return None;
        }
        Some(AudioClipData {
            samples,
            channels,
            sample_rate,
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
                    style.text_styles.insert(
                        egui::TextStyle::Button,
                        egui::FontId::proportional(BASE_UI_FONT_SIZE),
                    );
                    ui.set_style(style);

                    let icon_size = egui::vec2(12.0, 12.0);
                    let menu_text = |text: &str| egui::RichText::new(text).size(BASE_UI_FONT_SIZE);
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
                        let new_template_resp = ui.menu_button(menu_text("New From Template"), |ui| {
                            let templates = self.list_templates();
                            if templates.is_empty() {
                                ui.label("No templates found");
                            } else {
                                for (name, path) in templates {
                                    let button = egui::Button::new(name).frame(false);
                                    if ui.add(button).clicked() {
                                        self.request_project_action(ProjectAction::NewFromTemplate(path));
                                        ui.close_menu();
                                    }
                                }
                            }
                        });
                        if new_template_resp.response.rect.width() > 0.0 {
                            let icon_rect = egui::Rect::from_min_max(
                                egui::pos2(
                                    new_template_resp.response.rect.right() - 16.0,
                                    new_template_resp.response.rect.top(),
                                ),
                                egui::pos2(
                                    new_template_resp.response.rect.right() - 4.0,
                                    new_template_resp.response.rect.bottom(),
                                ),
                            );
                            let bg = if new_template_resp.response.hovered() {
                                ui.visuals().widgets.hovered.bg_fill
                            } else {
                                ui.visuals().panel_fill
                            };
                            let fg = ui.visuals().widgets.inactive.fg_stroke.color;
                            ui.painter().rect_filled(icon_rect, 0.0, bg);
                            ui.put(
                                icon_rect,
                                egui::Image::new(egui::include_image!("../../icons/chevron-right.svg"))
                                    .fit_to_exact_size(icon_rect.size())
                                    .tint(fg),
                            );
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
                        let open_recent_resp = ui.menu_button(menu_text("Open Recent"), |ui| {
                            if self.settings.recent_projects.is_empty() {
                                ui.label("No recent projects");
                            } else {
                                for path in self.settings.recent_projects.clone() {
                                    let display = Path::new(&path)
                                        .file_name()
                                        .and_then(|s| s.to_str())
                                        .unwrap_or(path.as_str())
                                        .to_string();
                                    let exists = Path::new(&path).exists();
                                    ui.add_enabled_ui(exists, |ui| {
                                        let button = egui::Button::new(display).frame(false);
                                        if ui.add(button).on_hover_text(&path).clicked() {
                                            self.request_project_action(
                                                ProjectAction::OpenProjectPath(path),
                                            );
                                            ui.close_menu();
                                        }
                                    });
                                }
                            }
                        });
                        if open_recent_resp.response.rect.width() > 0.0 {
                            let icon_rect = egui::Rect::from_min_max(
                                egui::pos2(
                                    open_recent_resp.response.rect.right() - 16.0,
                                    open_recent_resp.response.rect.top(),
                                ),
                                egui::pos2(
                                    open_recent_resp.response.rect.right() - 4.0,
                                    open_recent_resp.response.rect.bottom(),
                                ),
                            );
                            let bg = if open_recent_resp.response.hovered() {
                                ui.visuals().widgets.hovered.bg_fill
                            } else {
                                ui.visuals().panel_fill
                            };
                            let fg = ui.visuals().widgets.inactive.fg_stroke.color;
                            ui.painter().rect_filled(icon_rect, 0.0, bg);
                            ui.put(
                                icon_rect,
                                egui::Image::new(egui::include_image!("../../icons/chevron-right.svg"))
                                    .fit_to_exact_size(icon_rect.size())
                                    .tint(fg),
                            );
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
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../icons/save.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(file_color),
                                menu_text("Save New Version"),
                            ))
                            .clicked()
                        {
                        if let Err(err) = self.save_project_new_version() {
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
                    .add(egui::Button::image(if self.audio_running {
                        egui::Image::new(egui::include_image!("../../icons/pause.svg"))
                            .fit_to_exact_size(egui::vec2(16.0, 16.0))
                    } else {
                        play_icon.clone()
                    }))
                    .on_hover_text(if self.audio_running { "Pause" } else { "Play" })
                    .clicked()
                {
                    if self.audio_running {
                        self.pause_audio_and_midi();
                        self.status = "Paused".to_string();
                    } else {
                        self.seek_playhead(self.playhead_beats);
                        if let Err(err) = self.start_audio_and_midi_internal(false) {
                            self.status = format!("Play failed: {err}");
                        }
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
                    if let Some(PluginHostHandle::Vst3(host)) = self.selected_track_host() {
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
                if is_window_alive(ui_host.hwnd) {
                    if let PluginUiEditor::Vst3(editor) = &ui_host.editor {
                        editor.set_focus(false);
                    }
                    hide_plugin_window(ui_host.hwnd);
                } else {
                    self.destroy_plugin_ui();
                }
                ctx.request_repaint();
                return;
            }
        }
        let should_close_hidden = self
            .plugin_ui
            .as_ref()
            .map(|ui_host| !is_window_visible(ui_host.hwnd))
            .unwrap_or(false)
            && self.show_plugin_ui
            && !self.plugin_ui_hidden;
        if should_close_hidden {
            self.show_plugin_ui = false;
            if let Some(ui_host) = self.plugin_ui.as_ref() {
                self.plugin_ui_hidden = true;
                if is_window_alive(ui_host.hwnd) {
                    if let PluginUiEditor::Vst3(editor) = &ui_host.editor {
                        editor.set_focus(false);
                    }
                    hide_plugin_window(ui_host.hwnd);
                } else {
                    self.destroy_plugin_ui();
                }
            }
            ctx.request_repaint();
            return;
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
                if let PluginUiEditor::Vst3(editor) = &ui_host.editor {
                    editor.set_focus(false);
                }
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
            if ui_host.child_hwnd != ui_host.hwnd && !is_window_alive(ui_host.child_hwnd) {
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
            match &ui_host.editor {
                PluginUiEditor::Vst3(editor) => {
                    editor.set_focus(true);
                    if let Some((cw, ch)) = client_window_size(ui_host.child_hwnd) {
                        editor.set_size(cw, ch);
                    }
                }
                PluginUiEditor::Clap => {
                    if let PluginHostHandle::Clap(host) = &ui_host.host {
                        if let Ok(mut host) = host.lock() {
                            if let Some((gw, gh)) = host.take_gui_resize() {
                                move_plugin_child_window(
                                    ui_host.child_hwnd,
                                    0,
                                    0,
                                    gw.max(200),
                                    gh.max(120),
                                );
                                resize_plugin_top_window(ui_host.hwnd, gw.max(200), gh.max(120));
                            }
                            host.show_gui();
                        }
                    }
                }
            }
            invalidate_plugin_window(ui_host.child_hwnd);
            invalidate_plugin_window(ui_host.hwnd);
        }
        if self.show_plugin_ui {
            let desired_target = self
                .plugin_ui_target
                .or_else(|| self.selected_track.map(PluginUiTarget::Instrument));
            let needs_open = match (self.plugin_ui.as_ref(), desired_target) {
                (None, Some(_)) => true,
                (Some(ui_host), Some(target)) => ui_host.target != target,
                _ => false,
            };
            if needs_open {
                self.ensure_plugin_ui();
            }
        }

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
                        if let PluginUiEditor::Vst3(editor) = &ui_host.editor {
                            editor.set_focus(true);
                        }
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
                if let PluginUiEditor::Vst3(editor) = &ui_host.editor {
                    editor.set_focus(false);
                }
                hide_plugin_window(ui_host.hwnd);
                release_mouse_capture();
            }
            open = false;
            if self.plugin_ui.is_some() {
                self.plugin_ui_hidden = true;
            }
            ctx.request_repaint();
        }
        self.show_plugin_ui = open;
        if !self.show_plugin_ui {
            if let Some(ui_host) = self.plugin_ui.as_ref() {
                if let PluginUiEditor::Vst3(editor) = &ui_host.editor {
                    editor.set_focus(false);
                }
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
            self.status = "No plugin host for UI".to_string();
            return;
        };

        match &host {
            PluginHostHandle::Vst3(vst_host) => {
                let mut editor = {
                    let host_guard = match vst_host.try_lock() {
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
                resize_plugin_top_window(hwnd, w.max(200), h.max(120));
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
                    editor: PluginUiEditor::Vst3(editor),
                    host: host.clone(),
                    target,
                    close_requested,
                });
            }
            PluginHostHandle::Clap(clap_host) => {
                let (w, h) = clap_host
                    .lock()
                    .ok()
                    .and_then(|host| host.gui_size())
                    .unwrap_or((520, 360));
                eprintln!("CLAP UI size hint: {w}x{h}");
                let hwnd = match create_plugin_top_window(w, h) {
                    Some(hwnd) => hwnd,
                    None => {
                        self.status = "Failed to create plugin UI window".to_string();
                        return;
                    }
                };
                resize_plugin_top_window(hwnd, w.max(200), h.max(120));
                let mut child_hwnd = match create_plugin_child_window(hwnd) {
                    Some(child_hwnd) => child_hwnd,
                    None => {
                        self.status = "Failed to create plugin UI child window".to_string();
                        destroy_plugin_child_window(hwnd);
                        return;
                    }
                };
                move_plugin_child_window(child_hwnd, 0, 0, w.max(200), h.max(120));
                let mut attached = false;
                if let Ok(mut host) = clap_host.lock() {
                    attached = host.open_gui(child_hwnd).is_ok();
                    if !attached {
                        destroy_plugin_child_window(child_hwnd);
                        child_hwnd = hwnd;
                        attached = host.open_gui(hwnd).is_ok();
                    }
                    if attached {
                        if let Some((gw, gh)) = host.gui_size() {
                            let target_hwnd = if host.gui_embedded() { child_hwnd } else { hwnd };
                            move_plugin_child_window(target_hwnd, 0, 0, gw.max(200), gh.max(120));
                            resize_plugin_top_window(hwnd, gw.max(200), gh.max(120));
                        }
                    }
                }
                if !attached {
                    self.status = "CLAP view attach failed".to_string();
                    destroy_plugin_child_window(hwnd);
                    self.show_plugin_ui = false;
                    self.plugin_ui_hidden = false;
                    return;
                }
                bring_window_to_front(hwnd);
                invalidate_plugin_window(child_hwnd);
                invalidate_plugin_window(hwnd);
                let close_requested = Arc::new(AtomicBool::new(false));
                set_plugin_close_flag(hwnd, &close_requested);
                self.plugin_ui = Some(PluginUiHost {
                    hwnd,
                    child_hwnd,
                    editor: PluginUiEditor::Clap,
                    host: host.clone(),
                    target,
                    close_requested,
                });
            }
        }
    }

    fn destroy_plugin_ui(&mut self) {
        let Some(mut ui_host) = self.plugin_ui.take() else {
            return;
        };
        match &mut ui_host.editor {
            PluginUiEditor::Vst3(editor) => {
                editor.removed();
            }
            PluginUiEditor::Clap => {
                if let PluginHostHandle::Clap(host) = &ui_host.host {
                    if let Ok(mut host) = host.lock() {
                        host.destroy_gui();
                    }
                }
            }
        }
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
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(self.sidebar_tab == SidebarTab::Project, "Project")
                    .clicked()
                {
                    self.sidebar_tab = SidebarTab::Project;
                }
                if ui
                    .selectable_label(self.sidebar_tab == SidebarTab::Browser, "Browser")
                    .clicked()
                {
                    self.sidebar_tab = SidebarTab::Browser;
                }
            });
            ui.separator();

            match self.sidebar_tab {
                SidebarTab::Project => {
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
                    self.render_fs_row(
                        ui,
                        &root_label,
                        &root_key,
                        0,
                        true,
                        true,
                        FsSource::Project,
                        &root,
                    );
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        let entries = self.list_project_entries(&root);
                        if entries.is_empty() {
                            ui.label("(no files)");
                            return;
                        }
                        for entry in entries {
                            self.render_fs_tree(ui, entry, 1, FsSource::Project);
                        }
                    });
                }
                SidebarTab::Browser => {
                    ui.horizontal(|ui| {
                        if ui.button("Add Folder").clicked() {
                            if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                                let folder = Self::normalize_windows_path(&folder);
                                let key = folder.to_string_lossy().to_string();
                                if !self.settings.browser_folders.iter().any(|p| p == &key) {
                                    self.settings.browser_folders.push(key.clone());
                                    let _ = self.save_settings();
                                    self.status = format!("Browser folder added: {key}");
                                }
                            }
                        }
                    });
                    ui.add_space(4.0);
                    if self.settings.browser_folders.is_empty() {
                        ui.label("(no folders)");
                        return;
                    }
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        let roots = self.settings.browser_folders.clone();
                        for root_str in roots {
                            let root = PathBuf::from(root_str);
                            if !root.exists() {
                                continue;
                            }
                            let root_key = Self::fs_key(&root);
                            if !self.browser_expanded.contains(&root_key) {
                                self.browser_expanded.insert(root_key.clone());
                            }
                            let root_label = root
                                .file_name()
                                .map(|name| name.to_string_lossy().to_string())
                                .unwrap_or_else(|| root.to_string_lossy().to_string());
                            self.render_fs_row(
                                ui,
                                &root_label,
                                &root_key,
                                0,
                                true,
                                true,
                                FsSource::Browser,
                                &root,
                            );
                            for entry in self.list_project_entries(&root) {
                                self.render_fs_tree(ui, entry, 1, FsSource::Browser);
                            }
                            ui.add_space(2.0);
                        }
                    });
                }
            }
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

    fn fs_drag_kind_for_path(path: &Path) -> Option<FsDragKind> {
        let ext = path.extension().and_then(|s| s.to_str())?.to_ascii_lowercase();
        if matches!(ext.as_str(), "mid" | "midi") {
            return Some(FsDragKind::Midi);
        }
        if matches!(ext.as_str(), "wav" | "ogg" | "flac" | "mp3" | "aiff" | "aif") {
            return Some(FsDragKind::Audio);
        }
        None
    }

    fn invalidate_audio_caches_for_path(&self, path: &Path) {
        let key = path.to_string_lossy().to_string();
        self.waveform_cache.borrow_mut().remove(&key);
        self.waveform_color_cache.borrow_mut().remove(&key);
        self.waveform_len_seconds_cache.borrow_mut().remove(&key);
        if let Ok(mut cache) = self.audio_clip_cache.lock() {
            cache.remove(&key);
        }
    }

    fn delete_fs_path(&mut self, path: &Path) -> Result<(), String> {
        if path.is_dir() {
            fs::remove_dir_all(path).map_err(|e| e.to_string())?;
        } else {
            fs::remove_file(path).map_err(|e| e.to_string())?;
            self.invalidate_audio_caches_for_path(path);
        }
        Ok(())
    }

    fn duplicate_fs_path(&mut self, path: &Path) -> Result<PathBuf, String> {
        if path.is_dir() {
            return Err("Duplicate only supports files".to_string());
        }
        let parent = path.parent().ok_or_else(|| "Missing parent folder".to_string())?;
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("copy");
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let mut counter = 1;
        let mut candidate = if ext.is_empty() {
            parent.join(format!("{stem} copy"))
        } else {
            parent.join(format!("{stem} copy.{ext}"))
        };
        while candidate.exists() {
            counter += 1;
            candidate = if ext.is_empty() {
                parent.join(format!("{stem} copy {counter}"))
            } else {
                parent.join(format!("{stem} copy {counter}.{ext}"))
            };
        }
        fs::copy(path, &candidate).map_err(|e| e.to_string())?;
        Ok(candidate)
    }

    fn show_in_explorer(&mut self, path: &Path) {
        #[cfg(target_os = "windows")]
        {
            let mut cmd = std::process::Command::new("explorer");
            if path.is_file() {
                cmd.arg("/select,").arg(path);
            } else {
                cmd.arg(path);
            }
            let _ = cmd.spawn();
        }
        #[cfg(not(target_os = "windows"))]
        {
            self.status = format!("Open path: {}", path.to_string_lossy());
        }
    }

    fn remove_browser_folder(&mut self, path: &Path) {
        let key = path.to_string_lossy().to_string();
        self.settings.browser_folders.retain(|p| p != &key);
        let _ = self.save_settings();
        self.status = format!("Browser folder removed: {key}");
    }

    fn render_fs_tree(&mut self, ui: &mut egui::Ui, entry: FsEntry, depth: usize, source: FsSource) {
        let key = Self::fs_key(&entry.path);
        let is_open = match source {
            FsSource::Project => self.fs_expanded.contains(&key),
            FsSource::Browser => self.browser_expanded.contains(&key),
        };
        let toggled = self.render_fs_row(
            ui,
            &entry.name,
            &key,
            depth,
            entry.is_dir,
            is_open,
            source,
            &entry.path,
        );
        if entry.is_dir {
            if toggled {
                if is_open {
                    match source {
                        FsSource::Project => {
                            self.fs_expanded.remove(&key);
                        }
                        FsSource::Browser => {
                            self.browser_expanded.remove(&key);
                        }
                    }
                } else {
                    match source {
                        FsSource::Project => {
                            self.fs_expanded.insert(key.clone());
                        }
                        FsSource::Browser => {
                            self.browser_expanded.insert(key.clone());
                        }
                    }
                }
            }
            let open = match source {
                FsSource::Project => self.fs_expanded.contains(&key),
                FsSource::Browser => self.browser_expanded.contains(&key),
            };
            if open {
                for child in self.list_project_entries(&entry.path) {
                    self.render_fs_tree(ui, child, depth + 1, source);
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
        source: FsSource,
        path: &Path,
    ) -> bool {
        let row_h = 20.0;
        let full_w = ui.available_width();
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(full_w, row_h), egui::Sense::click_and_drag());
        let selected = match source {
            FsSource::Project => self.fs_selected.as_deref() == Some(key),
            FsSource::Browser => self.browser_selected.as_deref() == Some(key),
        };
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
        let font_id = egui::FontId::proportional(BASE_UI_FONT_SIZE);
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
                    let badge_font = egui::FontId::proportional(BASE_UI_FONT_SIZE);
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
            match source {
                FsSource::Project => self.fs_selected = Some(key.to_string()),
                FsSource::Browser => self.browser_selected = Some(key.to_string()),
            }
        }

        let mut action: Option<(&'static str, PathBuf)> = None;
        let mut open_folder_remove = false;
        response.context_menu(|ui| {
            if !is_dir {
                if ui.button("Delete").clicked() {
                    action = Some(("delete", path.to_path_buf()));
                    ui.close_menu();
                }
                if ui.button("Duplicate").clicked() {
                    action = Some(("duplicate", path.to_path_buf()));
                    ui.close_menu();
                }
            }
            if ui.button("Show in Explorer").clicked() {
                action = Some(("show", path.to_path_buf()));
                ui.close_menu();
            }
            if source == FsSource::Browser && depth == 0 && is_dir {
                if ui.button("Remove Folder").clicked() {
                    open_folder_remove = true;
                    ui.close_menu();
                }
            }
        });
        if let Some((kind, path)) = action {
            match kind {
                "delete" => {
                    if let Err(err) = self.delete_fs_path(&path) {
                        self.status = format!("Delete failed: {err}");
                    }
                }
                "duplicate" => {
                    match self.duplicate_fs_path(&path) {
                        Ok(new_path) => {
                            self.status = format!("Duplicated: {}", new_path.to_string_lossy());
                        }
                        Err(err) => {
                            self.status = format!("Duplicate failed: {err}");
                        }
                    }
                }
                "show" => self.show_in_explorer(&path),
                _ => {}
            }
        }
        if open_folder_remove {
            self.remove_browser_folder(path);
        }

        if !is_dir {
            if response.drag_started() {
                if let Some(kind) = Self::fs_drag_kind_for_path(path) {
                    self.fs_drag = Some(FsDragState {
                        path: path.to_path_buf(),
                        kind,
                    });
                }
            }
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
            style.text_styles.insert(
                egui::TextStyle::Heading,
                egui::FontId::proportional(BASE_UI_FONT_SIZE),
            );
            style.text_styles.insert(
                egui::TextStyle::Body,
                egui::FontId::proportional(BASE_UI_FONT_SIZE),
            );
            style.text_styles.insert(
                egui::TextStyle::Button,
                egui::FontId::proportional(BASE_UI_FONT_SIZE),
            );
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
                                    egui::FontId::proportional(BASE_UI_FONT_SIZE),
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
                                    egui::FontId::proportional(BASE_UI_FONT_SIZE),
                                    egui::Color32::from_gray(220),
                                );
                                Self::outlined_text(
                                    ui.painter(),
                                    solo_rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    "S",
                                    egui::FontId::proportional(BASE_UI_FONT_SIZE),
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

                ui.separator();
                let master_color = egui::Color32::from_rgb(200, 210, 230);
                let master_fill = egui::Color32::from_rgba_premultiplied(40, 50, 70, 80);
                egui::Frame::none()
                    .fill(master_fill)
                    .rounding(egui::Rounding::same(0.0))
                    .inner_margin(egui::Margin::symmetric(6.0, 0.0))
                    .show(ui, |ui| {
                        let (label_rect, _) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), 18.0),
                            egui::Sense::hover(),
                        );
                        ui.painter().rect_filled(label_rect, 0.0, master_fill);
                        Self::outlined_text(
                            ui.painter(),
                            egui::pos2(label_rect.left() + 6.0, label_rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            "Master",
                            egui::FontId::proportional(BASE_UI_FONT_SIZE),
                            master_color,
                        );

                        if let Ok(mut master) = self.master_settings.lock() {
                            let level_response = ui.add_sized(
                                [ui.available_width(), 12.0],
                                egui::Slider::new(&mut master.level, 0.0..=1.5).text("Level"),
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
                            let peak = self.master_peak_display.clamp(0.0, 1.0);
                            let fill_w = meter_rect.width() * peak;
                            if fill_w > 0.0 {
                                let color = if peak > 0.9 {
                                    egui::Color32::from_rgb(255, 90, 64)
                                } else if peak > 0.7 {
                                    egui::Color32::from_rgb(250, 200, 80)
                                } else {
                                    egui::Color32::from_rgb(90, 210, 120)
                                };
                                let fill_rect = egui::Rect::from_min_size(
                                    meter_rect.min,
                                    egui::vec2(fill_w, meter_rect.height()),
                                );
                                ui.painter().rect_filled(fill_rect, 2.0, color);
                            }
                            ui.separator();
                            ui.checkbox(&mut master.enabled, "Compressor");
                            ui.horizontal(|ui| {
                                ui.label("Thresh (dB)");
                                ui.add(
                                    egui::DragValue::new(&mut master.threshold_db)
                                        .speed(0.5)
                                        .clamp_range(-60.0..=0.0),
                                );
                            });
                            ui.horizontal(|ui| {
                                ui.label("Ratio");
                                ui.add(
                                    egui::DragValue::new(&mut master.ratio)
                                        .speed(0.1)
                                        .clamp_range(1.0..=20.0),
                                );
                            });
                            ui.horizontal(|ui| {
                                ui.label("Attack (ms)");
                                ui.add(
                                    egui::DragValue::new(&mut master.attack_ms)
                                        .speed(0.5)
                                        .clamp_range(1.0..=200.0),
                                );
                            });
                            ui.horizontal(|ui| {
                                ui.label("Release (ms)");
                                ui.add(
                                    egui::DragValue::new(&mut master.release_ms)
                                        .speed(1.0)
                                        .clamp_range(10.0..=1000.0),
                                );
                            });
                            ui.horizontal(|ui| {
                                ui.label("Makeup (dB)");
                                ui.add(
                                    egui::DragValue::new(&mut master.makeup_db)
                                        .speed(0.5)
                                        .clamp_range(-12.0..=12.0),
                                );
                            });
                        }
                    });
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
                                host.prepare_for_drop();
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
                            if fx_index < track.effect_clap_ids.len() {
                                track.effect_clap_ids.remove(fx_index);
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
                                host.prepare_for_drop();
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
                                if target_index < track.effect_clap_ids.len() {
                                    track.effect_clap_ids.swap(fx_index, target_index);
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
                ui.label("Grid Snap");
                let snap_label = if self.arranger_snap_beats < 0.0 {
                    "Cell"
                } else if self.arranger_snap_beats <= 0.0 {
                    "None"
                } else if (self.arranger_snap_beats - 4.0).abs() <= f32::EPSILON {
                    "Bar"
                } else {
                    "Beat"
                };
                egui::ComboBox::from_id_source("arranger_grid_snap")
                    .selected_text(snap_label)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(self.arranger_snap_beats.abs() <= f32::EPSILON, "None")
                            .clicked()
                        {
                            self.arranger_snap_beats = 0.0;
                        }
                        if ui.selectable_label(self.arranger_snap_beats < 0.0, "Cell").clicked() {
                            self.arranger_snap_beats = -1.0;
                        }
                        if ui
                            .selectable_label((self.arranger_snap_beats - 1.0).abs() <= f32::EPSILON, "Beat")
                            .clicked()
                        {
                            self.arranger_snap_beats = 1.0;
                        }
                        if ui
                            .selectable_label((self.arranger_snap_beats - 4.0).abs() <= f32::EPSILON, "Bar")
                            .clicked()
                        {
                            self.arranger_snap_beats = 4.0;
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.label("Tools");
                let tool_size = egui::vec2(110.0, 22.0);
                let icon_size = egui::vec2(14.0, 14.0);
                let button_bg = egui::Color32::from_rgba_premultiplied(18, 20, 24, 220);
                let button_on = egui::Color32::from_rgba_premultiplied(46, 94, 130, 220);
                let icon_tint = egui::Color32::from_gray(220);
                let mut tool_button = |tool: ArrangerTool, icon: egui::ImageSource<'static>, label: &str| {
                    let selected = self.arranger_tool == tool;
                    let button = egui::Button::image_and_text(
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
            let box_select_active = ctx.input(|i| i.key_down(egui::Key::B) || i.modifiers.ctrl);
            if over_arranger && !self.piano_roll_hovered {
                let input = ctx.input(|i| i.clone());
                if input.modifiers.ctrl {
                    let zoom = input.zoom_delta();
                    if (zoom - 1.0).abs() > f32::EPSILON {
                        self.arranger_zoom = (self.arranger_zoom * zoom).clamp(0.05, 4.0);
                    } else {
                        let mut delta = input.smooth_scroll_delta;
                        if delta == egui::Vec2::ZERO {
                            delta = input.raw_scroll_delta;
                        }
                        let zoom_delta = (delta.x + delta.y) * 0.01;
                        self.arranger_zoom = (self.arranger_zoom + zoom_delta).clamp(0.05, 4.0);
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
            self.arranger_zoom = self.arranger_zoom.clamp(0.05, 4.0);
            let beat_width = 22.0 * self.arranger_zoom;
            let beats_per_view = view_width / beat_width.max(0.001);
            let draw_step = if beats_per_view >= 64.0 {
                16.0
            } else if beats_per_view >= 32.0 {
                4.0
            } else if beats_per_view >= 20.0 {
                2.0
            } else if beats_per_view >= 12.0 {
                1.0
            } else if beats_per_view >= 8.0 {
                0.5
            } else {
                0.25
            };
            let major_step = 4.0f32;
            let band_step = if draw_step >= major_step {
                draw_step
            } else {
                major_step
            };
            let arranger_snap = if self.arranger_snap_beats.abs() <= f32::EPSILON {
                0.0
            } else {
                draw_step
            };
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
            let arranger_bg = egui::Color32::from_rgb(8, 9, 11);
            let playlist_bg = egui::Color32::from_rgb(18, 20, 24);
            painter.rect_filled(rect, 0.0, arranger_bg);
            let header_rect = egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.top()),
                egui::pos2(rect.right(), rect.top() + header_height),
            );
            painter.rect_filled(header_rect, 0.0, playlist_bg);
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
            let major_div = if draw_step >= major_step {
                1
            } else {
                (major_step / draw_step).round() as i32
            };
            let mut minor_index = 0;
            let mut x = row_left;
            let step_px = beat_width * draw_step;
            while x <= grid_right {
                let major = major_div > 0 && minor_index % major_div == 0;
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
                        egui::pos2(x + beat_width * band_step, grid_bottom),
                    );
                    let band_index = if major_div > 0 { minor_index / major_div } else { 0 };
                    let band_color = if band_index % 2 == 0 {
                        egui::Color32::from_rgba_premultiplied(0, 0, 0, 0)
                    } else {
                        egui::Color32::from_rgba_premultiplied(4, 6, 8, 120)
                    };
                    grid_painter.rect_filled(band_rect, 0.0, band_color);
                }
                minor_index += 1;
                x += step_px;
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
            shelf_painter.rect_filled(shelf_rect, 0.0, playlist_bg);
            let timeline_clip = egui::Rect::from_min_max(
                egui::pos2(row_left, header_rect.top()),
                egui::pos2(header_rect.right(), header_rect.bottom()),
            );
            let _header_painter = painter.with_clip_rect(timeline_clip);

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
                let mut midi_started = false;
                for (index, file) in dropped_files.iter().enumerate() {
                    let Some(path) = file.path.as_ref() else {
                        continue;
                    };
                    let offset = index as f32 * 0.5;
                    match Self::fs_drag_kind_for_path(path) {
                        Some(FsDragKind::Midi) => {
                            if !midi_started {
                                let _ = self.begin_midi_import_with_mode(
                                    path.to_string_lossy().to_string(),
                                    MidiImportMode::AppendTracks {
                                        start_beats: start_beats + offset,
                                    },
                                );
                                midi_started = true;
                            }
                        }
                        Some(FsDragKind::Audio) => match self.add_audio_clip_from_path(
                            target_track,
                            start_beats + offset,
                            path,
                        ) {
                            Ok(()) => {
                                self.status = format!("Added clip: {}", path.to_string_lossy());
                            }
                            Err(err) => {
                                self.status = format!("Drop import failed: {err}");
                            }
                        },
                        None => {}
                    }
                }
            }

            if ctx.input(|i| i.pointer.any_released()) {
                if let Some(fs_drag) = self.fs_drag.take() {
                    if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) {
                        if rect.contains(pos) {
                            let mut target_track = self
                                .selected_track
                                .unwrap_or(0)
                                .min(self.tracks.len().saturating_sub(1));
                            if let Some(track_index) = track_for_pos(pos) {
                                target_track = track_index;
                            }
                            let start_beats = ((pos.x - row_left) / beat_width).max(0.0);
                            match fs_drag.kind {
                                FsDragKind::Audio => {
                                    self.push_undo_state();
                                    match self.add_audio_clip_from_path(
                                        target_track,
                                        start_beats,
                                        &fs_drag.path,
                                    ) {
                                        Ok(()) => {
                                            self.status = format!(
                                                "Added clip: {}",
                                                fs_drag.path.to_string_lossy()
                                            );
                                        }
                                        Err(err) => {
                                            self.status = format!("Drop import failed: {err}");
                                        }
                                    }
                                }
                                FsDragKind::Midi => {
                                    let _ = self.begin_midi_import_with_mode(
                                        fs_drag.path.to_string_lossy().to_string(),
                                        MidiImportMode::AppendTracks { start_beats },
                                    );
                                }
                            }
                        }
                    }
                }
            }

            let mut pending_select: Option<(usize, usize, bool)> = None;
            let mut pending_multi_select: Option<Vec<(usize, usize)>> = None;
            let mut pending_delete: Option<usize> = None;
            let mut pending_drag_start: Option<ClipDragState> = None;
            let mut pending_track_select: Option<usize> = None;
            let mut pending_track_move: Option<(usize, usize)> = None;
            let mut pending_stamp_copy: Option<(Clip, usize, usize, f32, f32, f32)> = None;
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
                let label_rect = label_rect.intersect(shelf_clip);
                let row_rect = row_rect.intersect(grid_clip);
                let row_click_rect = row_click_rect.intersect(grid_clip);
                if row_click_rect.height() <= 0.0 || row_rect.height() <= 0.0 {
                    continue;
                }
                match *row {
                    ArrangerRow::Track { track_index } => {
                        let track_clips = match self.tracks.get(track_index) {
                            Some(track) => track.clips.clone(),
                            None => continue,
                        };
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
                        for clip in &track_clips {
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
                            let selected = self.selected_clips.contains(&clip.id);
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
                            if let Some(loop_len) = self.clip_loop_len_beats(clip) {
                                if loop_len > 0.0 && clip.length_beats > loop_len + 0.0001 {
                                    let mut marker = clip.start_beats + loop_len;
                                    while marker < clip.start_beats + clip.length_beats - 0.0001 {
                                        let x = row_left + marker * beat_width;
                                        if x >= clip_rect.left() && x <= clip_rect.right() {
                                            clip_painter.line_segment(
                                                [
                                                    egui::pos2(x, clip_rect.top()),
                                                    egui::pos2(x, clip_rect.bottom()),
                                                ],
                                                egui::Stroke::new(
                                                    1.0,
                                                    egui::Color32::from_rgba_premultiplied(220, 230, 255, 120),
                                                ),
                                            );
                                        }
                                        marker += loop_len;
                                    }
                                }
                            }
                            let name = if clip.name.trim().is_empty() {
                                if clip.is_midi { "MIDI" } else { "Audio" }
                            } else {
                                clip.name.as_str()
                            };
                            let header_text = ui.fonts(|f| {
                                let font = egui::FontId::proportional(BASE_UI_FONT_SIZE);
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
                                egui::FontId::proportional(BASE_UI_FONT_SIZE),
                                egui::Color32::WHITE,
                            );
                            if clip.is_midi {
                                let preview_rect = body_rect.shrink2(egui::vec2(6.0, 6.0));
                                self.draw_midi_preview(
                                    &clip_painter,
                                    preview_rect,
                                    clip,
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

                            let handle_w = 12.0;
                            let trim_h = 10.0;
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
                            let mut clip_response =
                                ui.interact(clip_interact_rect, clip_id, egui::Sense::click_and_drag());
                            if clip_response.hovered() {
                                if let Some(pos) = clip_response.interact_pointer_pos() {
                                    let edge_pad = 10.0;
                                    let near_left = (pos.x - clip_rect.left()).abs() <= edge_pad;
                                    let near_right = (clip_rect.right() - pos.x).abs() <= edge_pad;
                                    let icon = if near_left || near_right {
                                        egui::CursorIcon::ResizeHorizontal
                                    } else {
                                        egui::CursorIcon::Move
                                    };
                                    clip_response = clip_response.on_hover_cursor(icon);
                                }
                            }
                            if clip_response.hovered() {
                                over_clip = true;
                            }
                            if clip_response.double_clicked() {
                                pending_select = Some((clip.id, track_index, false));
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
                                let add = ctx.input(|i| i.modifiers.shift || i.modifiers.ctrl);
                                pending_select = Some((clip.id, track_index, add));
                                if self.arranger_tool == ArrangerTool::Select {
                                    switch_to_move = true;
                                }
                            }

                            let can_grab = pending_drag_start.is_none();
                            let mut start_drag =
                                |this: &mut DawApp, kind: ClipDragKind, pos: Option<egui::Pos2>| {
                                if let Some(pos) = pos {
                                    let offset_beats = (pos.x - row_left) / beat_width - clip.start_beats;
                                    let shift_copy = ui.input(|i| i.modifiers.shift);
                                    let mut clip_id = clip.id;
                                    let mut copy_mode = false;
                                    let mut undo_pushed = false;
                                    let mut group: Option<Vec<ClipDragGroupItem>> = None;
                                    let multi_selected = this.selected_clips.len() > 1
                                        && this.selected_clips.contains(&clip.id);
                                    if shift_copy && kind == ClipDragKind::Move && multi_selected {
                                        let mut new_ids = Vec::new();
                                        let mut group_items = Vec::new();
                                        let mut primary_new_id = None;
                                        this.push_undo_state();
                                        undo_pushed = true;
                                        let selected_ids: Vec<usize> =
                                            this.selected_clips.iter().copied().collect();
                                        for selected_id in selected_ids {
                                            let mut found = None;
                                            for (ti, track) in this.tracks.iter().enumerate() {
                                                if let Some(found_clip) =
                                                    track.clips.iter().find(|c| c.id == selected_id)
                                                {
                                                    found = Some((ti, found_clip.clone()));
                                                    break;
                                                }
                                            }
                                            let Some((ti, mut copy)) = found else {
                                                continue;
                                            };
                                            let new_id = this.next_clip_id();
                                            copy.id = new_id;
                                            copy.track = ti;
                                            copy.link_id = this.ensure_clip_link_id(ti, selected_id);
                                            if let Some(track) = this.tracks.get_mut(ti) {
                                                track.clips.push(copy.clone());
                                            }
                                            if copy.is_midi {
                                                this.sync_track_audio_notes(ti);
                                            }
                                            group_items.push(ClipDragGroupItem {
                                                clip_id: new_id,
                                                source_track: ti,
                                                start_beats: copy.start_beats,
                                                length_beats: copy.length_beats,
                                                is_midi: copy.is_midi,
                                            });
                                            if selected_id == clip.id {
                                                primary_new_id = Some(new_id);
                                            }
                                            new_ids.push(new_id);
                                        }
                                        if let Some(primary_id) = primary_new_id {
                                            clip_id = primary_id;
                                        }
                                        this.selected_clips.clear();
                                        for id in &new_ids {
                                            this.selected_clips.insert(*id);
                                        }
                                        this.selected_clip = Some(clip_id);
                                        group = Some(group_items);
                                        copy_mode = true;
                                    }
                                    if shift_copy && kind == ClipDragKind::Move {
                                        if group.is_none() {
                                            let new_id = this.next_clip_id();
                                            let link_id = this.ensure_clip_link_id(track_index, clip.id);
                                            if let Some(track) = this.tracks.get_mut(track_index) {
                                                let mut copy = clip.clone();
                                                copy.id = new_id;
                                                copy.link_id = link_id;
                                                track.clips.push(copy);
                                                if clip.is_midi {
                                                    this.sync_track_audio_notes(track_index);
                                                }
                                                clip_id = new_id;
                                                copy_mode = true;
                                                undo_pushed = true;
                                                this.push_undo_state();
                                            }
                                        }
                                    }
                                    pending_drag_start = Some(ClipDragState {
                                        clip_id,
                                        source_track: track_index,
                                        origin_track: track_index,
                                        offset_beats,
                                        start_beats: clip.start_beats,
                                        length_beats: clip.length_beats,
                                        origin_start_beats: clip.start_beats,
                                        origin_length_beats: clip.length_beats,
                                        audio_offset_beats: clip.audio_offset_beats,
                                        audio_source_beats: clip.audio_source_beats,
                                        kind,
                                        undo_pushed,
                                        grabbed: false,
                                        copy_mode,
                                        group,
                                    });
                                }
                            };

                            if clip_response.hovered()
                                && can_grab
                                && ctx.input(|i| i.key_pressed(egui::Key::G))
                            {
                                pending_select = Some((clip.id, track_index, false));
                                let pos = clip_response
                                    .interact_pointer_pos()
                                    .or_else(|| ctx.input(|i| i.pointer.interact_pos()));
                                start_drag(self, ClipDragKind::Move, pos);
                            }

                            if let Some(resp) = header_left_resp.as_ref() {
                                if resp.drag_started() {
                                    pending_select = Some((clip.id, track_index, false));
                                    start_drag(self, ClipDragKind::ResizeStart, resp.interact_pointer_pos());
                                }
                            }
                            if let Some(resp) = header_right_resp.as_ref() {
                                if resp.drag_started() {
                                    pending_select = Some((clip.id, track_index, false));
                                    start_drag(self, ClipDragKind::ResizeEnd, resp.interact_pointer_pos());
                                }
                            }
                            if let Some(resp) = trim_left_resp.as_ref() {
                                if resp.drag_started() {
                                    pending_select = Some((clip.id, track_index, false));
                                    start_drag(self, ClipDragKind::TrimStart, resp.interact_pointer_pos());
                                }
                            }
                            if let Some(resp) = trim_right_resp.as_ref() {
                                if resp.drag_started() {
                                    pending_select = Some((clip.id, track_index, false));
                                    start_drag(self, ClipDragKind::TrimEnd, resp.interact_pointer_pos());
                                }
                            }
                            if clip_response.drag_started() {
                                pending_select = Some((clip.id, track_index, false));
                                let pos = clip_response.interact_pointer_pos();
                                let edge_pad = 10.0;
                                let kind = if let Some(pos) = pos {
                                    let near_left = (pos.x - clip_rect.left()).abs() <= edge_pad;
                                    let near_right = (clip_rect.right() - pos.x).abs() <= edge_pad;
                                    if near_left {
                                        ClipDragKind::ResizeStart
                                    } else if near_right {
                                        ClipDragKind::ResizeEnd
                                    } else {
                                        ClipDragKind::Move
                                    }
                                } else {
                                    ClipDragKind::Move
                                };
                                start_drag(self, kind, pos);
                            }

                            clip_response.context_menu(|ui| {
                                let clone_label = if self.selected_clips.len() > 1
                                    && self.selected_clips.contains(&clip.id)
                                {
                                    "Clone Selected Clips"
                                } else {
                                    "Clone Clip"
                                };
                                if ui
                                    .add(egui::Button::image_and_text(
                                        egui::Image::new(egui::include_image!("../../icons/copy.svg"))
                                            .fit_to_exact_size(egui::vec2(12.0, 12.0))
                                            .tint(base),
                                        egui::RichText::new(clone_label).color(base),
                                    ))
                                    .clicked()
                                {
                                    let clone_ids: Vec<usize> = if self.selected_clips.contains(&clip.id) {
                                        self.selected_clips.iter().copied().collect()
                                    } else {
                                        vec![clip.id]
                                    };
                                    self.clone_clips_by_ids(&clone_ids);
                                    ui.close_menu();
                                }
                                if clip.link_id.is_some() {
                                    if ui
                                        .add(egui::Button::image_and_text(
                                            egui::Image::new(egui::include_image!("../../icons/link-2.svg"))
                                                .fit_to_exact_size(egui::vec2(12.0, 12.0))
                                                .tint(base),
                                            egui::RichText::new("Make Unique").color(base),
                                        ))
                                        .clicked()
                                    {
                                        self.make_clip_unique(track_index, clip.id);
                                        ui.close_menu();
                                    }
                                }
                                let can_merge = self.can_merge_selected_clips()
                                    && self.selected_clips.contains(&clip.id);
                                if ui
                                    .add_enabled(
                                        can_merge,
                                        egui::Button::image_and_text(
                                            egui::Image::new(egui::include_image!("../../icons/git-merge.svg"))
                                                .fit_to_exact_size(egui::vec2(12.0, 12.0))
                                                .tint(base),
                                            egui::RichText::new("Merge Clips").color(base),
                                        ),
                                    )
                                    .clicked()
                                {
                                    self.merge_selected_clips();
                                    ui.close_menu();
                                }
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
                        let target_key = match lane.target {
                            AutomationTarget::Instrument => "inst".to_string(),
                            AutomationTarget::Effect(fx_index) => format!("fx_{fx_index}"),
                        };
                        let lane_id = egui::Id::new(format!(
                            "automation_lane_row_{}_{}_{}",
                            track_index, target_key, lane.param_id
                        ));
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
                            egui::FontId::proportional(BASE_UI_FONT_SIZE),
                            egui::Color32::from_rgb(140, 160, 200),
                        );
                    }
                }
            }

            let mut marquee_rect: Option<egui::Rect> = None;
            let mut draw_rect: Option<egui::Rect> = None;
            if let Some(pos) = response.interact_pointer_pos() {
                let in_grid = grid_clip.contains(pos);
                let select_mode = self.arranger_tool == ArrangerTool::Select || box_select_active;
                if select_mode && in_grid && !over_clip {
                    if response.drag_started() {
                        self.arranger_select_start = Some(pos);
                        self.arranger_select_add = ctx.input(|i| i.modifiers.shift || i.modifiers.ctrl);
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

            if response.clicked()
                && ctx.input(|i| i.modifiers.shift)
                && self.arranger_tool != ArrangerTool::Draw
                && !over_clip
            {
                if let (Some(clip_id), Some(pos)) = (self.selected_clip, response.interact_pointer_pos()) {
                    if let Some(target_track) = track_for_pos(pos) {
                        let mut source_clip = None;
                        let mut source_track = None;
                        for (track_index, track) in self.tracks.iter().enumerate() {
                            if let Some(clip) = track.clips.iter().find(|c| c.id == clip_id) {
                                source_clip = Some(clip.clone());
                                source_track = Some(track_index);
                                break;
                            }
                        }
                        if let (Some(clip), Some(source_track)) = (source_clip, source_track) {
                            let source_start = clip.start_beats;
                            let source_length = clip.length_beats;
                            let start_beats = ((pos.x - row_left) / beat_width).max(0.0);
                            let snap = arranger_snap;
                            let snapped_start = if snap > 0.0 {
                                let snap = snap.max(0.25);
                                (start_beats / snap).round() * snap
                            } else {
                                start_beats
                            };
                            let delta = snapped_start - source_start;
                            pending_stamp_copy = Some((
                                clip,
                                source_track,
                                target_track,
                                delta,
                                source_start,
                                source_length,
                            ));
                        }
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
                        let mut hits: Vec<(usize, usize)> = Vec::new();
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
                                    hits.push((clip.id, track_index));
                                }
                            }
                        }
                        if !hits.is_empty() {
                            pending_multi_select = Some(hits);
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
                        let snap = arranger_snap;
                        let (snapped_start, snapped_end) = if snap > 0.0 {
                            let snap = snap.max(0.25);
                            (
                                (start_beats / snap).round() * snap,
                                (end_beats / snap).round() * snap,
                            )
                        } else {
                            (start_beats, end_beats)
                        };
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
                        let snap = arranger_snap;
                        let min_len = 0.25;
                        let (mut start, mut end) = if snap > 0.0 {
                            let snap = snap.max(0.25);
                            (
                                (draw.start_beats / snap).round() * snap,
                                (end_beats / snap).round() * snap,
                            )
                        } else {
                            (draw.start_beats, end_beats)
                        };
                        if (end - start).abs() < min_len {
                            end = start + min_len;
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
                                length_beats: (end - start).max(min_len),
                                is_midi: true,
                                midi_notes: Vec::new(),
                                midi_source_beats: Some((end - start).max(min_len)),
                                link_id: None,
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
            // Overlay timeline bar and grid/loop/playhead lines above clips.
            let timeline_overlay_rect = header_rect;
            painter.rect_filled(timeline_overlay_rect, 0.0, egui::Color32::from_rgb(0, 0, 0));
            let overlay_painter = painter.with_clip_rect(timeline_clip);
            let mut overlay_x = row_left;
            let mut overlay_step_index = 0i32;
            let overlay_major_div = if draw_step >= major_step {
                1
            } else {
                (major_step / draw_step).round() as i32
            };
            while overlay_x <= rect.right() - 8.0 {
                let major = overlay_major_div > 0 && overlay_step_index % overlay_major_div == 0;
                let line_x = overlay_x.round() + 0.5;
                let line_width = if major { 2.0 } else { 1.0 };
                let color = if major {
                    egui::Color32::from_rgba_premultiplied(48, 52, 60, 170)
                } else {
                    egui::Color32::from_rgba_premultiplied(32, 36, 44, 120)
                };
                if major {
                    let band_rect = egui::Rect::from_min_max(
                        egui::pos2(overlay_x, timeline_overlay_rect.top()),
                        egui::pos2(
                            (overlay_x + beat_width * band_step).min(timeline_overlay_rect.right()),
                            timeline_overlay_rect.bottom(),
                        ),
                    );
                    let band_index = if overlay_major_div > 0 {
                        overlay_step_index / overlay_major_div
                    } else {
                        0
                    };
                    let shade = if band_index % 2 == 0 {
                        egui::Color32::from_rgb(8, 8, 8)
                    } else {
                        egui::Color32::from_rgb(0, 0, 0)
                    };
                    overlay_painter.rect_filled(band_rect, 0.0, shade);
                }
                grid_painter.line_segment(
                    [egui::pos2(line_x, grid_top), egui::pos2(line_x, grid_bottom)],
                    egui::Stroke::new(line_width, color),
                );
                if major {
                    let bar = ((overlay_step_index as f32 * draw_step) / 4.0).floor() as i32 + 1;
                    Self::outlined_text(
                        &overlay_painter,
                        egui::pos2(overlay_x + 4.0, timeline_overlay_rect.top() + 2.0),
                        egui::Align2::LEFT_TOP,
                        &format!("{bar}"),
                        egui::FontId::proportional(BASE_UI_FONT_SIZE),
                        egui::Color32::from_gray(200),
                    );
                }
                overlay_step_index += 1;
                overlay_x += beat_width * draw_step;
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
                            egui::Color32::from_rgba_premultiplied(0, 0, 0, 90),
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
                    let drag_id = egui::Id::new(format!("arranger_track_drag_{}", track_index));
                    let drag_response =
                        ui.interact(label_click_rect, drag_id, egui::Sense::click_and_drag());
                    if drag_response.drag_started() {
                        self.track_drag = Some(TrackDragState { source_index: track_index });
                    }
                    if drag_response.drag_stopped() {
                        if let Some(drag) = self.track_drag.take() {
                            let pos = drag_response
                                .interact_pointer_pos()
                                .or_else(|| response.interact_pointer_pos())
                                .or_else(|| ctx.input(|i| i.pointer.interact_pos()));
                            if let Some(pos) = pos {
                                if let Some(target_track) = track_for_pos(pos) {
                                    pending_track_move = Some((drag.source_index, target_track));
                                }
                            }
                        }
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
                    egui::FontId::proportional(BASE_UI_FONT_SIZE),
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

            if let Some((mut copy, source_track, target_track, delta, _source_start, _source_len)) =
                pending_stamp_copy
            {
                let source_clip_id = copy.id;
                let link_id = self.ensure_clip_link_id(source_track, source_clip_id);
                let new_id = self.next_clip_id();
                copy.id = new_id;
                copy.track = target_track;
                let base_start = copy.start_beats;
                copy.start_beats = (base_start + delta).max(0.0);
                copy.link_id = link_id;
                self.push_undo_state();
                if let Some(track) = self.tracks.get_mut(target_track) {
                    track.clips.push(copy.clone());
                }
                if copy.is_midi {
                    self.shift_clip_notes_by_delta(new_id, delta);
                }
                if copy.is_midi {
                    self.sync_track_audio_notes(target_track);
                }
                pending_select = Some((new_id, target_track, false));
                switch_to_move = true;
            }

            let has_pending_drag = pending_drag_start.is_some();
            let mut selection_changed = false;
            if let Some(hits) = pending_multi_select {
                if !self.arranger_select_add {
                    self.selected_clips.clear();
                }
                let mut last_clip = None;
                let mut last_track = None;
                for (clip_id, track_index) in hits {
                    self.selected_clips.insert(clip_id);
                    last_clip = Some(clip_id);
                    last_track = Some(track_index);
                }
                self.selected_clip = last_clip;
                self.selected_track = last_track;
                selection_changed = true;
            }
            if let Some((clip_id, track_index, add)) = pending_select {
                if !add {
                    self.selected_clips.clear();
                }
                self.selected_clips.insert(clip_id);
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
                self.remove_clip_and_notes_by_id(clip_id);
                self.selected_clips.remove(&clip_id);
                if self.selected_clip == Some(clip_id) {
                    self.selected_clip = None;
                }
            }

            if let Some((from, to)) = pending_track_move {
                self.move_track_order(from, to);
            }

            if let Some(mut drag) = self.clip_drag.take() {
                let (pointer_down, pointer_pos, pointer_released) = ctx.input(|i| {
                    (i.pointer.primary_down(), i.pointer.interact_pos(), i.pointer.any_released())
                });
                if pointer_down {
                    if let Some(pos) = pointer_pos {
                        if !drag.undo_pushed {
                            self.push_undo_state();
                            drag.undo_pushed = true;
                        }
                        let min_len = 0.25;
                        let target_track = track_for_pos(pos)
                            .unwrap_or(0)
                            .min(self.tracks.len().saturating_sub(1));
                        let cursor_beats = (pos.x - row_left) / beat_width;
                        let snap = arranger_snap.max(0.0);
                        let snap_value = |value: f32| {
                            if snap > 0.0 {
                                let snap = snap.max(0.25);
                                (value / snap).round() * snap
                            } else {
                                value
                            }
                        };

                        match drag.kind {
                            ClipDragKind::Move => {
                                let raw_start = (cursor_beats - drag.offset_beats).max(0.0);
                                let new_start = snap_value(raw_start).max(0.0);
                                let delta = new_start - drag.start_beats;
                                if let Some(group) = drag.group.as_mut() {
                                    for item in group.iter_mut() {
                                        let old_track = item.source_track;
                                        let new_track = item.source_track;
                                        let new_item_start = (item.start_beats + delta).max(0.0);
                                        if item.is_midi
                                            && (delta.abs() > f32::EPSILON
                                                || new_track != item.source_track)
                                        {
                                            self.shift_clip_notes_by_delta(item.clip_id, delta);
                                        }
                                        self.move_clip_by_id(item.clip_id, new_track, new_item_start);
                                        if item.is_midi {
                                            self.sync_track_audio_notes(old_track);
                                            if new_track != old_track {
                                                self.sync_track_audio_notes(new_track);
                                            }
                                        }
                                        item.start_beats = new_item_start;
                                        item.source_track = new_track;
                                    }
                                    drag.source_track = target_track;
                                    drag.start_beats = new_start;
                                } else {
                                    let old_track = drag.source_track;
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
                                        self.shift_clip_notes_by_delta(drag.clip_id, delta);
                                    }
                                    self.move_clip_by_id(drag.clip_id, target_track, new_start);
                                    if is_midi {
                                        self.sync_track_audio_notes(old_track);
                                        if target_track != old_track {
                                            self.sync_track_audio_notes(target_track);
                                        }
                                    }
                                    drag.source_track = target_track;
                                    drag.start_beats = new_start;
                                }
                            }
                            ClipDragKind::ResizeStart => {
                                let end = drag.start_beats + drag.length_beats;
                                let raw_start = cursor_beats.min(end - min_len).max(0.0);
                                let new_start = snap_value(raw_start)
                                    .min(end - min_len)
                                    .max(0.0);
                                let new_len = (end - new_start).max(min_len);
                                self.update_clip_by_id(drag.clip_id, |clip| {
                                    clip.start_beats = new_start;
                                    clip.length_beats = new_len;
                                });
                            }
                            ClipDragKind::ResizeEnd => {
                                let raw_end = cursor_beats.max(drag.start_beats + min_len);
                                let snapped_end = snap_value(raw_end).max(drag.start_beats + min_len);
                                let new_len = (snapped_end - drag.start_beats).max(min_len);
                                self.update_clip_by_id(drag.clip_id, |clip| {
                                    if clip.is_midi && clip.midi_source_beats.is_none() {
                                        clip.midi_source_beats = Some(clip.length_beats.max(min_len));
                                    }
                                    clip.length_beats = new_len;
                                });
                            }
                            ClipDragKind::TrimStart => {
                                let end = drag.start_beats + drag.length_beats;
                                let raw_start = cursor_beats.min(end - min_len);
                                let new_start = snap_value(raw_start).min(end - min_len);
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
                                let raw_end = cursor_beats.max(drag.start_beats + min_len);
                                let snapped_end = snap_value(raw_end).max(drag.start_beats + min_len);
                                let new_len = (snapped_end - drag.start_beats).max(min_len);
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
                if pointer_released || !pointer_down {
                    if matches!(
                        drag.kind,
                        ClipDragKind::ResizeStart
                            | ClipDragKind::ResizeEnd
                            | ClipDragKind::TrimStart
                            | ClipDragKind::TrimEnd
                    ) {
                        if let Some(track) = self.tracks.get(drag.source_track) {
                            if let Some(clip) = track.clips.iter().find(|c| c.id == drag.clip_id) {
                                if clip.is_midi {
                                    self.crop_clip_notes_to_clip_range(
                                        clip.id,
                                        clip.start_beats,
                                        clip.length_beats,
                                    );
                                }
                            }
                        }
                    }
                    if drag.copy_mode {
                        let is_midi = self
                            .tracks
                            .get(drag.source_track)
                            .and_then(|track| track.clips.iter().find(|c| c.id == drag.clip_id))
                            .map(|clip| clip.is_midi)
                            .unwrap_or(false);
                        let _ = is_midi;
                    }
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
                        let button = egui::Button::image_and_text(
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
                            let clip_path = self
                                .tracks
                                .get(ti)
                                .and_then(|t| t.clips.get(ci))
                                .and_then(|clip| self.resolve_clip_audio_path(clip))
                                .map(|path| path.to_path_buf());
                            if let Some(clip) =
                                self.tracks.get_mut(ti).and_then(|t| t.clips.get_mut(ci))
                            {
                                ui.label("Clip Properties");
                                ui.add_space(6.0);
                                ui.horizontal(|ui| {
                                    ui.label("Gain");
                                    Self::colored_slider(ui, &mut clip.audio_gain, 0.0..=2.0, track_color);
                                });
                                if ui
                                    .add(egui::Button::new("Normalize"))
                                    .on_hover_text("Normalize clip gain to -1 dB peak")
                                    .clicked()
                                {
                                    match clip_path.as_ref() {
                                        Some(path) => {
                                            if let Err(err) = Self::normalize_audio_clip_with_path(clip, path) {
                                                self.status = format!("Normalize failed: {err}");
                                            }
                                        }
                                        None => {
                                            self.status = "Normalize failed: Clip has no audio file".to_string();
                                        }
                                    }
                                }
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
                            self.draw_effect_params_panel(
                                ui,
                                ti,
                                track_color,
                                &mut pending_automation_record,
                            );
                        }
                    } else {
                        let track = selected_track_index.and_then(|i| self.tracks.get(i));
                        let name = track.map(|t| t.name.as_str()).unwrap_or("None");
                        ui.label(format!("Track: {name}"));
                        if let Some(track) = track {
                            ui.label(format!("FX slots: {}", track.effect_paths.len()));
                        }
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
                                            host.prepare_for_drop();
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
                            let host_change = if let Some(PluginHostHandle::Vst3(host)) = self.selected_track_host() {
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
                            let project_root = self.presets_root_project();
                            let global_root = self.presets_root_global();
                            let can_project = project_root.is_some();
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
                            }
                            if let Some(track_index) = selected_track_index {
                                self.draw_effect_params_panel(
                                    ui,
                                    track_index,
                                    track_color,
                                    &mut pending_automation_record,
                                );
                            }
                            ui.separator();
                            ui.label("Presets");
                            ui.horizontal(|ui| {
                                ui.label("Name");
                                ui.text_edit_singleline(&mut self.preset_name_buffer);
                            });
                            let preset_name = self.preset_name_buffer.trim().to_string();
                            ui.horizontal(|ui| {
                                if ui.button("Generate GM Presets").clicked() {
                                    self.ensure_builtin_gm_presets();
                                    self.status = "GM presets generated".to_string();
                                }
                                if ui.button("Save Global").clicked() {
                                    if let Some(index) = selected_track_index {
                                        match self.save_preset_for_track(
                                            index,
                                            global_root.clone(),
                                            &preset_name,
                                        ) {
                                            Ok(path) => self.status = format!("Preset saved: {path}"),
                                            Err(err) => self.status = format!("Preset save failed: {err}"),
                                        }
                                    }
                                }
                                ui.add_enabled_ui(can_project, |ui| {
                                    if ui.button("Save Project").clicked() {
                                        if let (Some(index), Some(root)) =
                                            (selected_track_index, project_root.clone())
                                        {
                                            match self.save_preset_for_track(
                                                index,
                                                root,
                                                &preset_name,
                                            ) {
                                                Ok(path) => {
                                                    self.status = format!("Preset saved: {path}");
                                                }
                                                Err(err) => {
                                                    self.status = format!("Preset save failed: {err}");
                                                }
                                            }
                                        }
                                    }
                                });
                            });
                            ui.horizontal(|ui| {
                                if ui.button("Load Global").clicked() {
                                    if let Some(index) = selected_track_index {
                                        let file = rfd::FileDialog::new()
                                            .set_directory(&global_root)
                                            .add_filter("Preset", &["json"])
                                            .pick_file();
                                        if let Some(file) = file {
                                            if let Err(err) = self.load_preset_from_path(index, &file) {
                                                self.status = format!("Preset load failed: {err}");
                                            } else {
                                                self.status = "Preset loaded".to_string();
                                            }
                                        }
                                    }
                                }
                                ui.add_enabled_ui(can_project, |ui| {
                                    if ui.button("Load Project").clicked() {
                                        if let (Some(index), Some(root)) =
                                            (selected_track_index, project_root.clone())
                                        {
                                            let file = rfd::FileDialog::new()
                                                .set_directory(&root)
                                                .add_filter("Preset", &["json"])
                                                .pick_file();
                                            if let Some(file) = file {
                                                if let Err(err) =
                                                    self.load_preset_from_path(index, &file)
                                                {
                                                    self.status =
                                                        format!("Preset load failed: {err}");
                                                } else {
                                                    self.status = "Preset loaded".to_string();
                                                }
                                            }
                                        }
                                    }
                                });
                            });
                            if let Some(track) = selected_track_index.and_then(|i| self.tracks.get_mut(i)) {
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
                                                if let Some(PluginHostHandle::Vst3(host)) =
                                                    state.host.as_ref()
                                                {
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
            let note_button_size = egui::vec2(22.0, 22.0);
            let note_icon_size = egui::vec2(18.0, 18.0);
            let note_icon_tint = egui::Color32::from_gray(230);
            let note_button_bg = egui::Color32::from_rgba_premultiplied(18, 20, 24, 200);
            let note_button_on = egui::Color32::from_rgba_premultiplied(46, 94, 130, 230);
            ui.horizontal(|ui| {
                ui.label("Note Length");
                ui.add_space(4.0);
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
                        .fit_to_exact_size(note_icon_size)
                        .tint(note_icon_tint);
                    let button = egui::Button::image(icon)
                        .min_size(note_button_size)
                        .fill(if selected { note_button_on } else { note_button_bg });
                    let response = ui
                        .add_sized(note_button_size, button)
                        .on_hover_text(label);
                    if response.clicked() {
                        self.piano_note_len = value;
                    }
                    if selected {
                        ui.painter().rect_stroke(
                            response.rect,
                            4.0,
                            egui::Stroke::new(1.0, egui::Color32::from_rgb(140, 190, 255)),
                        );
                    }
                }
            });
            ui.horizontal(|ui| {
                let grid_icon = egui::Image::new(egui::include_image!("../../icons/grid.svg"))
                    .fit_to_exact_size(note_icon_size);
                ui.add(grid_icon);
                ui.label("Snap");
                ui.add_space(4.0);
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
                        .fit_to_exact_size(note_icon_size)
                        .tint(note_icon_tint);
                    let button = egui::Button::image(icon)
                        .min_size(note_button_size)
                        .fill(if selected { note_button_on } else { note_button_bg });
                    let response = ui
                        .add_sized(note_button_size, button)
                        .on_hover_text(label);
                    if response.clicked() {
                        self.piano_snap = value;
                    }
                    if selected {
                        ui.painter().rect_stroke(
                            response.rect,
                            4.0,
                            egui::Stroke::new(1.0, egui::Color32::from_rgb(140, 190, 255)),
                        );
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
                                                host.prepare_for_drop();
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
                                let host_change = if let Some(PluginHostHandle::Vst3(host)) = self.selected_track_host() {
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
                                                    if let Some(PluginHostHandle::Vst3(host)) =
                                                        state.host.as_ref()
                                                    {
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
                                }
                                if let Some(track) = selected_track_index.and_then(|i| self.tracks.get_mut(i)) {
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

                                    if !track.automation_lanes.is_empty() {
                                        ui.separator();
                                        ui.label("Automation Lanes");
                                        for (lane_index, lane) in track.automation_lanes.iter().enumerate() {
                                            ui.push_id(lane_index, |ui| {
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
                                            });
                                        }
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
                    let pointer_interact = ctx.input(|i| i.pointer.interact_pos());
                    let pointer_down = ctx.input(|i| i.pointer.primary_down());
                    let pointer_clicked = ctx.input(|i| i.pointer.primary_clicked());
                    let pointer_released = ctx.input(|i| i.pointer.any_released());
                    let ctrl_down = ctx.input(|i| i.modifiers.ctrl);
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
                    let clip_offset = selected_clip_info
                        .and_then(|(ti, ci)| self.tracks.get(ti).and_then(|t| t.clips.get(ci)))
                        .filter(|clip| clip.is_midi)
                        .map(|clip| clip.start_beats)
                        .unwrap_or(0.0);
                    let pos_to_local = |x: f32, pan: f32| (x - roll_rect.left() - pan) / beat_width;
                    let pos_to_abs = |x: f32, pan: f32| pos_to_local(x, pan) + clip_offset;
                    let header_id = egui::Id::new("piano_roll_timeline");
                    let header_response = ui.interact(header_rect, header_id, egui::Sense::click());
                    if header_response.clicked() {
                        if let Some(pos) = header_response.interact_pointer_pos() {
                            let local = self.beats_from_pos(
                                pos.x,
                                roll_rect.left() + self.piano_pan.x,
                                beat_width,
                            );
                            self.seek_playhead(local + clip_offset);
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
                    let mut hovered_key: Option<u8> = None;
                    let mut hovered_key_vel: Option<u8> = None;
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
                        if let Some(pos) = pointer_interact {
                            if key_rect.contains(pos) {
                                hovered_key = Some(note);
                                let t = ((pos.x - keyboard_rect.left()) / keyboard_rect.width())
                                    .clamp(0.0, 1.0);
                                let vel = (t * 127.0).round().clamp(1.0, 127.0) as u8;
                                hovered_key_vel = Some(vel);
                            }
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
                    if let Some(note) = hovered_key {
                        if ctrl_down && pointer_clicked {
                            if let Some(clip_id) = self.selected_clip {
                                if let Some((track_index, clip_index)) =
                                    self.find_clip_indices_by_id(clip_id)
                                {
                                    if let Some(clip) =
                                        self.tracks.get(track_index).and_then(|t| t.clips.get(clip_index))
                                    {
                                        self.piano_selected.clear();
                                        for (index, data) in clip.midi_notes.iter().enumerate() {
                                            if data.midi_note == note {
                                                self.piano_selected.insert(index);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if !ctrl_down {
                        if pointer_down {
                            match hovered_key {
                                Some(note) => {
                                    if self.piano_key_down != Some(note) {
                                        if let Some(prev) = self.piano_key_down {
                                            self.piano_preview_note_off(prev);
                                        }
                                        let vel = hovered_key_vel.unwrap_or(100);
                                        self.piano_preview_note_on(note, vel);
                                        self.piano_key_down = Some(note);
                                    }
                                }
                                None => {
                                    if let Some(prev) = self.piano_key_down {
                                        self.piano_preview_note_off(prev);
                                        self.piano_key_down = None;
                                    }
                                }
                            }
                        }
                        if pointer_released {
                            if let Some(prev) = self.piano_key_down {
                                self.piano_preview_note_off(prev);
                                self.piano_key_down = None;
                            }
                        }
                    }
                    let pointer_pos = roll_response
                        .interact_pointer_pos()
                        .or_else(|| roll_response.hover_pos());
                    let mut hovered_note: Option<(usize, egui::Rect)> = None;
                    if let Some(clip_id) = self.selected_clip {
                        if let Some((track_index, clip_index)) = self.find_clip_indices_by_id(clip_id) {
                            if let Some(clip) =
                                self.tracks.get(track_index).and_then(|t| t.clips.get(clip_index))
                            {
                                if !clip.midi_notes.is_empty() {
                                    for (index, note) in clip.midi_notes.iter().enumerate() {
                                        let local_start = note.start_beats - clip_offset;
                                        let x = roll_rect.left() + self.piano_pan.x + local_start * beat_width;
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
                                        if let Some(pos) = pointer_pos {
                                            if pos.x >= roll_rect.left() && note_rect.contains(pos) {
                                                hovered_note = Some((index, note_rect));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if let Some((_, note_rect)) = hovered_note {
                        if let Some(pos) = pointer_pos {
                            if pos.x >= roll_rect.left() {
                                let right_edge = note_rect.right();
                                let edge_pad = 8.0;
                                let icon = if (right_edge - pos.x).abs() <= edge_pad {
                                    egui::CursorIcon::ResizeHorizontal
                                } else {
                                    egui::CursorIcon::Grab
                                };
                                roll_response.clone().on_hover_cursor(icon);
                            }
                        }
                    }

                    let needs_clip_hint = match self.selected_clip {
                        None => true,
                        Some(clip_id) => self
                            .find_clip_indices_by_id(clip_id)
                            .and_then(|(track_index, clip_index)| {
                                self.tracks
                                    .get(track_index)
                                    .and_then(|t| t.clips.get(clip_index))
                            })
                            .map(|clip| !clip.is_midi)
                            .unwrap_or(true),
                    };
                    if needs_clip_hint {
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
                    let box_select_active = input.key_down(egui::Key::B);
                    let shift = input.modifiers.shift;
                    let alt = input.modifiers.alt;
                    let mut marquee_rect: Option<egui::Rect> = None;
                    let mut scale_handle_active = self.piano_scale_drag.is_some();
                    let mut scale_handle_stopped = false;
                    if shift && !self.piano_selected.is_empty() {
                        if let Some(clip_id) = self.selected_clip {
                            if let Some((track_index, clip_index)) =
                                self.find_clip_indices_by_id(clip_id)
                            {
                                if let Some(clip) = self
                                    .tracks
                                    .get(track_index)
                                    .and_then(|t| t.clips.get(clip_index))
                                {
                                    let mut min_start = f32::MAX;
                                    let mut max_end = 0.0f32;
                                    let mut min_y = f32::MAX;
                                    let mut max_y = 0.0f32;
                                    let mut selected_notes = Vec::new();
                                    for index in self.piano_selected.iter().copied() {
                                        if let Some(note) = clip.midi_notes.get(index) {
                                            min_start = min_start.min(note.start_beats);
                                            max_end = max_end.max(note.start_beats + note.length_beats);
                                            let y = roll_rect.bottom() + self.piano_pan.y
                                                - (note.midi_note as f32 - 40.0) * note_height;
                                            let y_min = y - note_height;
                                            min_y = min_y.min(y_min);
                                            max_y = max_y.max(y);
                                            selected_notes.push((
                                                index,
                                                note.start_beats,
                                                note.midi_note,
                                                note.length_beats,
                                            ));
                                        }
                                    }
                                    if min_start.is_finite() && max_end > min_start {
                                        let handle_x = roll_rect.left()
                                            + self.piano_pan.x
                                            + (max_end - clip_offset) * beat_width;
                                        let handle_y = (min_y + max_y) * 0.5;
                                        let handle_rect = egui::Rect::from_center_size(
                                            egui::pos2(handle_x, handle_y),
                                            egui::vec2(12.0, 12.0),
                                        );
                                        painter.circle_filled(
                                            handle_rect.center(),
                                            5.0,
                                            egui::Color32::from_rgb(210, 230, 255),
                                        );
                                        painter.circle_stroke(
                                            handle_rect.center(),
                                            5.0,
                                            egui::Stroke::new(1.0, egui::Color32::from_rgb(40, 60, 90)),
                                        );
                                        let handle_hover = pointer_pos
                                            .map(|pos| handle_rect.contains(pos))
                                            .unwrap_or(false);
                                        if handle_hover {
                                            roll_response
                                                .clone()
                                                .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
                                        }
                                        if handle_hover && roll_response.drag_started() {
                                            self.push_undo_state();
                                            self.piano_scale_drag = Some(PianoScaleDragState {
                                                track_index,
                                                anchor_start: min_start,
                                                anchor_end: max_end,
                                                selected_notes,
                                            });
                                            scale_handle_active = true;
                                        }
                                        if handle_hover && roll_response.dragged() {
                                            scale_handle_active = true;
                                        }
                                        if handle_hover && roll_response.drag_stopped() {
                                            scale_handle_stopped = true;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if (ctrl || box_select_active) && roll_response.drag_started() {
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
                                if let Some(clip_id) = self.selected_clip {
                                    if let Some((track_index, clip_index)) =
                                        self.find_clip_indices_by_id(clip_id)
                                    {
                                        if let Some(clip) = self
                                            .tracks
                                            .get(track_index)
                                            .and_then(|t| t.clips.get(clip_index))
                                        {
                                            let mut hits: Vec<usize> = Vec::new();
                                            for (index, note) in clip.midi_notes.iter().enumerate() {
                                                let x = roll_rect.left()
                                                    + self.piano_pan.x
                                                    + (note.start_beats - clip_offset) * beat_width;
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
                    } else if !box_select_active && roll_response.clicked_by(egui::PointerButton::Primary) {
                        if let Some(pos) = roll_response.interact_pointer_pos() {
                            if pos.x < roll_rect.left() {
                                return;
                            }
                            if let Some(clip_id) = self.selected_clip {
                                if let Some((track_index, clip_index)) =
                                    self.find_clip_indices_by_id(clip_id)
                                {
                                    if let Some(clip) = self
                                        .tracks
                                        .get_mut(track_index)
                                        .and_then(|t| t.clips.get_mut(clip_index))
                                    {
                                        if clip.is_midi && hovered_note.is_none() {
                                            let local = pos_to_local(pos.x, self.piano_pan.x);
                                            let snapped_local = if alt {
                                                local
                                            } else {
                                                (local / quantize).round() * quantize
                                            };
                                            let snapped = (snapped_local + clip_offset).max(0.0);
                                            let pitch_f =
                                                (roll_rect.bottom() + self.piano_pan.y - pos.y) / note_height;
                                            let pitch = (40.0 + pitch_f).floor() as i32;
                                            let pitch = pitch.clamp(0, 127) as u8;
                                            if shift {
                                                clip.midi_notes.retain(|note| {
                                                    note.midi_note != pitch
                                                        || note.start_beats + note.length_beats <= snapped
                                                        || note.start_beats >= snapped + self.piano_note_len
                                                });
                                            }
                                            clip.midi_notes.push(PianoRollNote::new(
                                                snapped,
                                                self.piano_note_len,
                                                pitch,
                                                100,
                                            ));
                                            if let Some(index) = clip.midi_notes.len().checked_sub(1) {
                                                self.piano_selected.clear();
                                                self.piano_selected.insert(index);
                                            }
                                            self.sync_track_audio_notes(track_index);
                                            self.sync_linked_notes_after_edit(track_index);
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if roll_response.clicked_by(egui::PointerButton::Secondary) {
                        if let Some((note_index, _)) = hovered_note {
                            if let Some(clip_id) = self.selected_clip {
                                if let Some((track_index, clip_index)) =
                                    self.find_clip_indices_by_id(clip_id)
                                {
                                    if let Some(clip) = self
                                        .tracks
                                        .get_mut(track_index)
                                        .and_then(|t| t.clips.get_mut(clip_index))
                                    {
                                        if note_index < clip.midi_notes.len() {
                                            clip.midi_notes.remove(note_index);
                                            self.piano_selected.remove(&note_index);
                                            let shifted: HashSet<usize> = self
                                                .piano_selected
                                                .iter()
                                                .map(|idx| if *idx > note_index { idx - 1 } else { *idx })
                                                .collect();
                                            self.piano_selected = shifted;
                                            self.sync_track_audio_notes(track_index);
                                            self.sync_linked_notes_after_edit(track_index);
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if roll_response.drag_started() && !ctrl && !scale_handle_active {
                        if let Some((note_index, note_rect)) = hovered_note {
                            if let Some(pos) = roll_response.interact_pointer_pos() {
                                if pos.x < roll_rect.left() {
                                    return;
                                }
                                if !self.piano_selected.contains(&note_index) {
                                    self.piano_selected.clear();
                                    self.piano_selected.insert(note_index);
                                }
                                let right_edge = note_rect.right();
                                let edge_pad = 8.0;
                                let kind = if shift {
                                    PianoDragKind::Move
                                } else if (right_edge - pos.x).abs() <= edge_pad {
                                    PianoDragKind::Resize
                                } else {
                                    PianoDragKind::Move
                                };
                                let offset_beats = pos_to_abs(pos.x, self.piano_pan.x);
                                let shift_copy = shift;
                                if shift_copy {
                                    self.push_undo_state();
                                }
                                if let Some(clip_id) = self.selected_clip {
                                    if let Some((track_index, clip_index)) =
                                        self.find_clip_indices_by_id(clip_id)
                                    {
                                        if let Some(clip) = self
                                            .tracks
                                            .get_mut(track_index)
                                            .and_then(|t| t.clips.get_mut(clip_index))
                                        {
                                            if shift_copy {
                                                let mut selection: Vec<usize> =
                                                    self.piano_selected.iter().copied().collect();
                                                selection.sort_unstable();
                                                if selection.is_empty() {
                                                    selection.push(note_index);
                                                }
                                                let base_len = clip.midi_notes.len();
                                                let mut new_indices = Vec::new();
                                                for idx in selection.iter().copied() {
                                                    if let Some(note) = clip.midi_notes.get(idx).cloned() {
                                                        clip.midi_notes.push(note);
                                                        new_indices.push(base_len + new_indices.len());
                                                    }
                                                }
                                                self.piano_selected.clear();
                                                for idx in &new_indices {
                                                    self.piano_selected.insert(*idx);
                                                }
                                            }

                                            let (start_beats, start_length, start_pitch) = clip
                                                .midi_notes
                                                .get(note_index)
                                                .map(|note| (note.start_beats, note.length_beats, note.midi_note))
                                                .unwrap_or((0.0, self.piano_note_len.max(0.03125), 60));
                                            let mut selected_notes = Vec::new();
                                            for index in self.piano_selected.iter().copied() {
                                                if let Some(note) = clip.midi_notes.get(index) {
                                                    selected_notes.push((
                                                        index,
                                                        note.start_beats,
                                                        note.midi_note,
                                                        note.length_beats,
                                                    ));
                                                }
                                            }
                                            if selected_notes.is_empty() {
                                                if let Some(note) = clip.midi_notes.get(note_index) {
                                                    selected_notes.push((
                                                        note_index,
                                                        note.start_beats,
                                                        note.midi_note,
                                                        note.length_beats,
                                                    ));
                                                }
                                            }
                                            let primary_index =
                                                selected_notes.first().map(|v| v.0).unwrap_or(note_index);
                                            let primary = clip
                                                .midi_notes
                                                .get(primary_index)
                                                .map(|note| (note.start_beats, note.length_beats, note.midi_note))
                                                .unwrap_or((start_beats, start_length, start_pitch));
                                            self.piano_drag = Some(PianoDragState {
                                                track_index,
                                                note_index: primary_index,
                                                kind,
                                                offset_beats,
                                                start_beats: primary.0,
                                                start_length: primary.1,
                                                start_pitch: primary.2,
                                                start_pos_y: pos.y,
                                                selected_notes,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if (roll_response.dragged() || scale_handle_active) && !ctrl {
                        if let Some(scale_drag) = &self.piano_scale_drag {
                            if let Some(pos) = roll_response.interact_pointer_pos() {
                                if pos.x < roll_rect.left() {
                                    return;
                                }
                                if let Some(clip_id) = self.selected_clip {
                                    let clip_index = self
                                        .find_clip_indices_by_id(clip_id)
                                        .map(|(_, ci)| ci)
                                        .unwrap_or(usize::MAX);
                                    let Some(clip) = self
                                        .tracks
                                        .get_mut(scale_drag.track_index)
                                        .and_then(|t| t.clips.get_mut(clip_index))
                                    else {
                                        return;
                                    };
                                    let beat = pos_to_abs(pos.x, self.piano_pan.x);
                                    let snapped = (beat / quantize).round() * quantize;
                                    let new_end = snapped.max(scale_drag.anchor_start + quantize);
                                    let denom = (scale_drag.anchor_end - scale_drag.anchor_start)
                                        .max(quantize);
                                    let scale = (new_end - scale_drag.anchor_start) / denom;
                                    for (index, start, _pitch, len) in &scale_drag.selected_notes {
                                        if let Some(note) = clip.midi_notes.get_mut(*index) {
                                            note.start_beats =
                                                (scale_drag.anchor_start + (start - scale_drag.anchor_start) * scale)
                                                    .max(0.0);
                                            note.length_beats = (len * scale).max(quantize);
                                        }
                                    }
                                }
                            }
                        } else if let Some(drag) = &self.piano_drag {
                            if let Some(pos) = roll_response.interact_pointer_pos() {
                                if pos.x < roll_rect.left() {
                                    return;
                                }
                                if let Some(clip_id) = self.selected_clip {
                                    let clip_index = self
                                        .find_clip_indices_by_id(clip_id)
                                        .map(|(_, ci)| ci)
                                        .unwrap_or(usize::MAX);
                                    let Some(clip) = self
                                        .tracks
                                        .get_mut(drag.track_index)
                                        .and_then(|t| t.clips.get_mut(clip_index))
                                    else {
                                        return;
                                    };
                                    let beat = pos_to_abs(pos.x, self.piano_pan.x);
                                    match drag.kind {
                                        PianoDragKind::Move => {
                                            let raw_delta = beat - drag.offset_beats;
                                            let delta = if alt {
                                                raw_delta
                                            } else {
                                                let snapped = ((drag.start_beats + raw_delta) / quantize)
                                                    .round()
                                                    * quantize;
                                                snapped - drag.start_beats
                                            };
                                            let delta_pitch =
                                                ((drag.start_pos_y - pos.y) / note_height).round() as i32;
                                            if !drag.selected_notes.is_empty() {
                                                for (index, start, pitch, _) in &drag.selected_notes {
                                                    if let Some(note) = clip.midi_notes.get_mut(*index) {
                                                        note.start_beats = (start + delta).max(0.0);
                                                        let next_pitch = (*pitch as i32 + delta_pitch)
                                                            .clamp(0, 127) as u8;
                                                        note.midi_note = next_pitch;
                                                    }
                                                }
                                            } else if let Some(note) = clip.midi_notes.get_mut(drag.note_index) {
                                                note.start_beats = (drag.start_beats + delta).max(0.0);
                                                let next_pitch = (drag.start_pitch as i32 + delta_pitch)
                                                    .clamp(0, 127) as u8;
                                                note.midi_note = next_pitch;
                                            }
                                        }
                                        PianoDragKind::Resize => {
                                            if alt {
                                                let mut min_start = f32::MAX;
                                                let mut max_end = 0.0f32;
                                                for (_, start, _, len) in &drag.selected_notes {
                                                    min_start = min_start.min(*start);
                                                    max_end = max_end.max(start + len);
                                                }
                                                let anchor = min_start;
                                                let raw_end = beat.max(anchor + quantize);
                                                let snapped_end = (raw_end / quantize).round() * quantize;
                                                let new_end = snapped_end.max(anchor + quantize);
                                                let scale = if max_end > anchor {
                                                    (new_end - anchor) / (max_end - anchor)
                                                } else {
                                                    1.0
                                                };
                                                for (index, start, _pitch, len) in &drag.selected_notes {
                                                    if let Some(note) = clip.midi_notes.get_mut(*index) {
                                                        note.start_beats = (anchor + (start - anchor) * scale).max(0.0);
                                                        note.length_beats = (len * scale).max(quantize);
                                                    }
                                                }
                                            } else {
                                                let length = beat - drag.start_beats;
                                                let snapped = if alt {
                                                    length
                                                } else {
                                                    (length / quantize).round() * quantize
                                                };
                                                let delta_len = snapped - drag.start_length;
                                                if !drag.selected_notes.is_empty() {
                                                    for (index, _, _, start_len) in &drag.selected_notes {
                                                        if let Some(note) = clip.midi_notes.get_mut(*index) {
                                                            note.length_beats = (start_len + delta_len).max(quantize);
                                                        }
                                                    }
                                                } else if let Some(note) = clip.midi_notes.get_mut(drag.note_index) {
                                                    note.length_beats = snapped.max(quantize);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if roll_response.drag_stopped() || scale_handle_stopped {
                        if let Some(drag) = self.piano_scale_drag.take() {
                            self.sync_track_audio_notes(drag.track_index);
                            self.sync_linked_notes_after_edit(drag.track_index);
                        }
                        if let Some(drag) = self.piano_drag.take() {
                            self.sync_track_audio_notes(drag.track_index);
                            self.sync_linked_notes_after_edit(drag.track_index);
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

                    let playhead_x = roll_rect.left()
                        + self.piano_pan.x
                        + (self.playhead_beats - clip_offset) * beat_width;
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

                        if let Some(clip_id) = self.selected_clip {
                            if let Some((track_index, clip_index)) = self.find_clip_indices_by_id(clip_id) {
                                if let Some(track) = self.tracks.get(track_index) {
                                    let clip = track.clips.get(clip_index);
                                    match self.piano_lane_mode {
                                        PianoLaneMode::Velocity => {
                                            if let Some(clip) = clip {
                                                for note in &clip.midi_notes {
                                                    let value =
                                                        (note.velocity as f32 / 127.0).clamp(0.0, 1.0);
                                                    let h = lane_rect.height() * value;
                                                    let x = roll_rect.left()
                                                        + self.piano_pan.x
                                                        + (note.start_beats - clip_offset) * beat_width;
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
                                            if let Some(clip) = clip {
                                                for note in &clip.midi_notes {
                                                    let pan = note.pan.clamp(-1.0, 1.0);
                                                    let h = lane_rect.height() * 0.5 * pan.abs();
                                                    let x = roll_rect.left()
                                                        + self.piano_pan.x
                                                        + (note.start_beats - clip_offset) * beat_width;
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
                                        }
                                        PianoLaneMode::Cutoff => {
                                            if let Some(clip) = clip {
                                                for note in &clip.midi_notes {
                                                    let value = note.cutoff.clamp(0.0, 1.0);
                                                    let h = lane_rect.height() * value;
                                                    let x = roll_rect.left()
                                                        + self.piano_pan.x
                                                        + (note.start_beats - clip_offset) * beat_width;
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
                                        }
                                        PianoLaneMode::Resonance => {
                                            if let Some(clip) = clip {
                                                for note in &clip.midi_notes {
                                                    let value = note.resonance.clamp(0.0, 1.0);
                                                    let h = lane_rect.height() * value;
                                                    let x = roll_rect.left()
                                                        + self.piano_pan.x
                                                        + (note.start_beats - clip_offset) * beat_width;
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
                                        }
                                        PianoLaneMode::MidiCc => {
                                            if let Some(lane) = track
                                                .midi_cc_lanes
                                                .iter()
                                                .find(|lane| lane.cc == self.piano_cc)
                                            {
                                                let mut points = lane.points.clone();
                                                points.sort_by(|a, b| {
                                                    a.beat
                                                        .partial_cmp(&b.beat)
                                                        .unwrap_or(std::cmp::Ordering::Equal)
                                                });
                                                for window in points.windows(2) {
                                                    let a = &window[0];
                                                    let b = &window[1];
                                                    let x1 = roll_rect.left()
                                                        + self.piano_pan.x
                                                        + (a.beat - clip_offset) * beat_width;
                                                    let x2 = roll_rect.left()
                                                        + self.piano_pan.x
                                                        + (b.beat - clip_offset) * beat_width;
                                                    let y1 = lane_rect.bottom()
                                                        - a.value.clamp(0.0, 1.0) * lane_rect.height();
                                                    let y2 = lane_rect.bottom()
                                                        - b.value.clamp(0.0, 1.0) * lane_rect.height();
                                                    lane_painter.line_segment(
                                                        [egui::pos2(x1, y1), egui::pos2(x2, y2)],
                                                        egui::Stroke::new(
                                                            1.2,
                                                            egui::Color32::from_rgb(150, 180, 230),
                                                        ),
                                                    );
                                                }
                                                for point in &points {
                                                    let x = roll_rect.left()
                                                        + self.piano_pan.x
                                                        + (point.beat - clip_offset) * beat_width;
                                                    let y = lane_rect.bottom()
                                                        - point.value.clamp(0.0, 1.0) * lane_rect.height();
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
                                                        / beat_width
                                                        + clip_offset;
                                                    let value = (lane_rect.bottom() - pos.y)
                                                        / lane_rect.height();
                                                    let value = value.clamp(0.0, 1.0);

                                                    if lane_response.drag_started() || lane_response.clicked() {
                                                        let mut closest: Option<(usize, f32)> = None;
                                                        for (idx, point) in lane.points.iter().enumerate() {
                                                            let px = roll_rect.left()
                                                                + self.piano_pan.x
                                                                + (point.beat - clip_offset) * beat_width;
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
                                                if let Some(clip_id) = self.selected_clip {
                                                    if let Some((track_index, clip_index)) =
                                                        self.find_clip_indices_by_id(clip_id)
                                                    {
                                                        let beat = pos_to_abs(pos.x, self.piano_pan.x);
                                                        if beat >= clip_offset {
                                                            if let Some(clip) = self
                                                                .tracks
                                                                .get_mut(track_index)
                                                                .and_then(|t| t.clips.get_mut(clip_index))
                                                            {
                                                                if let Some(note_index) = clip
                                                                    .midi_notes
                                                                    .iter()
                                                                    .position(|note| {
                                                                        beat >= note.start_beats
                                                                            && beat <= note.start_beats + note.length_beats
                                                                    })
                                                                {
                                                                    let value = (lane_rect.bottom() - pos.y)
                                                                        / lane_rect.height();
                                                                    let value = value.clamp(0.0, 1.0);
                                                                    if let Some(note) = clip.midi_notes.get_mut(note_index) {
                                                                        match self.piano_lane_mode {
                                                                            PianoLaneMode::Velocity => {
                                                                                note.velocity =
                                                                                    (value * 127.0).round() as u8;
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
                            ui.horizontal(|ui| {
                                ui.label("Autosave (minutes)");
                                ui.add(
                                    egui::DragValue::new(&mut self.settings.autosave_minutes)
                                        .clamp_range(0..=120),
                                );
                            });
                            ui.label("Set to 0 to disable autosave.");
                            ui.checkbox(
                                &mut self.settings.load_last_project,
                                "Load last project at startup",
                            );
                            ui.checkbox(
                                &mut self.settings.play_startup_sound,
                                "Play startup sound",
                            );
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
            let mut chosen: Option<PluginCandidate> = None;
            let mut refresh = false;
            egui::Window::new("Plugin Picker")
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.label("Scanning VST3 + CLAP folders");
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
                        for candidate in &self.plugin_candidates {
                            let display = &candidate.display;
                            if !search.is_empty()
                                && !candidate.path.to_ascii_lowercase().contains(&search)
                                && !display.to_ascii_lowercase().contains(&search)
                            {
                                continue;
                            }
                            if ui.selectable_label(false, display).clicked() {
                                chosen = Some(candidate.clone());
                            }
                        }
                    });
                });

            if refresh {
                self.plugin_candidates = self.scan_plugins();
            }

            if let Some(candidate) = chosen {
                if let Some(target) = self.plugin_target {
                    match target {
                        PluginTarget::Instrument(index) => {
                            self.replace_instrument(index, candidate.path, candidate.clap_id);
                        }
                        PluginTarget::Effect(index) => {
                            let was_running = self.audio_running;
                            if was_running {
                                self.stop_audio_and_midi();
                            }
                            if let Some(track) = self.tracks.get_mut(index) {
                                track.effect_paths.push(candidate.path);
                                track.effect_clap_ids.push(candidate.clap_id);
                                track.effect_bypass.push(false);
                                track.effect_params.push(Vec::new());
                                track.effect_param_ids.push(Vec::new());
                                track.effect_param_values.push(Vec::new());
                            }
                            if let Some(state) = self.track_audio.get_mut(index) {
                                for host in state.effect_hosts.drain(..) {
                                    host.prepare_for_drop();
                                    self.orphaned_hosts.push(host);
                                }
                            }
                            if was_running {
                                if let Err(err) = self.start_audio_and_midi() {
                                    self.status = format!("Audio restart failed: {err}");
                                } else {
                                    self.status = "Audio restarted for new plugin".to_string();
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
            instrument_clap_id: None,
            effect_paths: Vec::new(),
            effect_clap_ids: Vec::new(),
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
        if let Ok(mut master) = self.master_settings.lock() {
            *master = MasterCompSettings::default();
        }
        if let Ok(mut state) = self.master_comp_state.lock() {
            *state = MasterCompState::default();
        }
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
            if let PluginUiEditor::Vst3(editor) = &ui_host.editor {
                editor.set_focus(false);
            }
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
        let mut hosts: Vec<PluginHostHandle> = Vec::new();
        for state in self.track_audio.iter_mut() {
            if let Some(host) = state.host.take() {
                host.prepare_for_drop();
                hosts.push(host);
            }
            for host in state.effect_hosts.drain(..) {
                host.prepare_for_drop();
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
            instrument_clap_id: None,
            effect_paths: Vec::new(),
            effect_clap_ids: Vec::new(),
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
                        host.prepare_for_drop();
                        self.orphaned_hosts.push(host);
                    }
                    for host in state.effect_hosts.drain(..) {
                        host.prepare_for_drop();
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
                let new_index = index + 1;
                dup.name = format!("{} Copy", track.name);
                for clip in &mut dup.clips {
                    clip.id = self.next_clip_id();
                    clip.track = new_index;
                }
                self.tracks.insert(new_index, dup);
                let state = TrackAudioState::from_track(&track);
                self.track_audio.insert(new_index, state);
                self.selected_track = Some(new_index);
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
        let previous_folder = self.project_path.trim().to_string();
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
            master_settings: self.master_settings_snapshot(),
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
        if !previous_folder.is_empty() {
            let previous = PathBuf::from(previous_folder);
            self.copy_project_assets_if_needed(&previous, &folder)?;
        }

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
                for note in &clip.midi_notes {
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
                    format!("{:02}_{}_{}_clip{}.mid", index + 1, safe_track, safe_clip, clip.id)
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
        self.register_recent_project_path(&folder);
        self.clear_dirty();
        self.status = format!("Saved {}", self.project_path);
        Ok(())
    }

    fn copy_project_assets_if_needed(
        &self,
        source_folder: &Path,
        target_folder: &Path,
    ) -> Result<(), String> {
        if !source_folder.exists() {
            return Ok(());
        }
        if Self::paths_equal(source_folder, target_folder) {
            return Ok(());
        }
        for name in ["audio", "samples"] {
            let source = source_folder.join(name);
            if !source.exists() {
                continue;
            }
            let target = target_folder.join(name);
            fs::create_dir_all(&target).map_err(|e| e.to_string())?;
            Self::copy_dir_recursive(&source, &target)?;
        }
        Ok(())
    }

    fn copy_dir_recursive(source: &Path, target: &Path) -> Result<(), String> {
        let entries = fs::read_dir(source).map_err(|e| e.to_string())?;
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let dest = target.join(name);
            if path.is_dir() {
                fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
                Self::copy_dir_recursive(&path, &dest)?;
            } else if !dest.exists() {
                let _ = fs::copy(&path, &dest);
            }
        }
        Ok(())
    }

    fn paths_equal(a: &Path, b: &Path) -> bool {
        #[cfg(windows)]
        {
            let left = Self::normalize_windows_path(a)
                .to_string_lossy()
                .to_ascii_lowercase();
            let right = Self::normalize_windows_path(b)
                .to_string_lossy()
                .to_ascii_lowercase();
            return left == right;
        }
        #[cfg(not(windows))]
        {
            return a == b;
        }
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
        if let Ok(mut master) = self.master_settings.lock() {
            *master = state.master_settings.clone();
        }
        if let Ok(mut comp_state) = self.master_comp_state.lock() {
            *comp_state = MasterCompState::default();
        }
        self.selected_clip = None;
        self.selected_clips.clear();
        self.piano_selected.clear();
        self.piano_drag = None;
        self.clip_drag = None;
        self.arranger_draw = None;
        self.arranger_select_start = None;
        self.project_path = folder.to_string_lossy().to_string();
        self.load_midi_from_folder(folder)?;
        self.migrate_track_notes_to_clips();
        self.sync_track_audio_states();
        self.selected_track = if self.tracks.is_empty() { None } else { Some(0) };
        if self.project_name.trim().is_empty() {
            if let Some(name) = self.project_name_from_path() {
                self.project_name = name;
            }
        }
        self.register_recent_project_path(folder);
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

    fn save_project_new_version(&mut self) -> Result<(), String> {
        let current = self.project_path.trim();
        if current.is_empty() {
            return Err("No project path to version".to_string());
        }
        let current_path = Path::new(current);
        let parent = current_path
            .parent()
            .ok_or_else(|| "Project folder has no parent".to_string())?;
        let name = current_path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| "Project folder name unavailable".to_string())?;
        let (base, version) = Self::split_version_suffix(name);
        let base = base.trim_end();
        if base.is_empty() {
            return Err("Project name unavailable".to_string());
        }
        let mut next = version.unwrap_or(1).saturating_add(1);
        loop {
            let candidate_name = format!("{} v{}", base, next);
            let candidate = parent.join(&candidate_name);
            if !candidate.exists() {
                self.save_project_to_folder(&candidate)?;
                self.status = format!("Saved new version {}", self.project_path);
                return Ok(());
            }
            next = next.saturating_add(1);
            if next > 9999 {
                return Err("No available version number".to_string());
            }
        }
    }

    fn open_project_from_path(&mut self, path: &str) -> Result<(), String> {
        let folder = Path::new(path);
        if !folder.exists() {
            self.settings.recent_projects.retain(|p| !p.eq_ignore_ascii_case(path));
            let _ = self.save_settings();
            return Err("Project folder not found".to_string());
        }
        self.load_project_from_folder(folder)
    }

    fn load_template_from_path(&mut self, path: &str) -> Result<(), String> {
        let path = Path::new(path);
        let folder = if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent()
                .ok_or_else(|| "Template folder unavailable".to_string())?
                .to_path_buf()
        };
        let manifest_path = folder.join("project.json");
        if !manifest_path.exists() {
            return Err("Template project.json missing".to_string());
        }
        self.prepare_for_project_change();
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
        if let Ok(mut master) = self.master_settings.lock() {
            *master = state.master_settings.clone();
        }
        if let Ok(mut comp_state) = self.master_comp_state.lock() {
            *comp_state = MasterCompState::default();
        }
        self.selected_clip = None;
        self.selected_clips.clear();
        self.piano_selected.clear();
        self.piano_drag = None;
        self.clip_drag = None;
        self.arranger_draw = None;
        self.arranger_select_start = None;
        self.project_path.clear();
        self.load_midi_from_folder(&folder)?;
        self.migrate_track_notes_to_clips();
        self.sync_track_audio_states();
        self.selected_track = if self.tracks.is_empty() { None } else { Some(0) };
        if self.project_name.trim().is_empty() {
            if let Some(name) = folder.file_name().and_then(|s| s.to_str()) {
                self.project_name = name.replace('_', " ");
            }
        }
        self.clear_dirty();
        self.status = "Template loaded".to_string();
        Ok(())
    }

    fn list_templates(&self) -> Vec<(String, String)> {
        let Some(root) = self.templates_dir() else {
            return Vec::new();
        };
        let mut templates = Vec::new();
        if let Ok(entries) = fs::read_dir(&root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                if !path.join("project.json").exists() {
                    continue;
                }
                let name = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Template")
                    .replace('_', " ");
                let normalized = Self::normalize_windows_path(&path);
                templates.push((name, normalized.to_string_lossy().to_string()));
            }
        }
        templates.sort_by(|a, b| a.0.cmp(&b.0));
        templates
    }

    fn templates_dir(&self) -> Option<PathBuf> {
        if let Ok(dir) = std::env::current_dir() {
            let candidate = dir.join("templates");
            if candidate.exists() {
                return Some(candidate);
            }
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                let candidate = parent.join("templates");
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
        None
    }

    fn register_recent_project_path(&mut self, folder: &Path) {
        let normalized = Self::normalize_windows_path(folder);
        let path = normalized.to_string_lossy().to_string();
        if path.trim().is_empty() {
            return;
        }
        self.settings
            .recent_projects
            .retain(|p| !p.eq_ignore_ascii_case(&path));
        self.settings.recent_projects.insert(0, path);
        if self.settings.recent_projects.len() > 10 {
            self.settings.recent_projects.truncate(10);
        }
        let _ = self.save_settings();
    }

    fn split_version_suffix(name: &str) -> (String, Option<u32>) {
        if let Some((base, suffix)) = name.rsplit_once(" v") {
            if !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit()) {
                if let Ok(num) = suffix.parse::<u32>() {
                    return (base.to_string(), Some(num));
                }
            }
        }
        (name.to_string(), None)
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

        let source_beats = Self::audio_length_beats(&target, self.tempo_bpm);
        let clip_len = source_beats.unwrap_or(4.0).max(0.25);

        let clip_id = self.next_clip_id();
        if let Some(track) = self.tracks.get_mut(track_index) {
            track.clips.push(Clip {
                id: clip_id,
                track: track_index,
                start_beats: start_beats.max(0.0),
                length_beats: clip_len,
                is_midi: false,
                midi_notes: Vec::new(),
                midi_source_beats: None,
                link_id: None,
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

    fn normalize_audio_clip_with_path(clip: &mut Clip, path: &Path) -> Result<(), String> {
        let (samples, _channels, _sample_rate) =
            Self::decode_audio_samples(path).ok_or_else(|| "Unsupported audio format".to_string())?;
        let mut peak = 0.0f32;
        for sample in samples {
            peak = peak.max(sample.abs());
        }
        if peak <= 0.0 {
            return Err("Clip is silent".to_string());
        }
        let target = db_to_gain(-1.0);
        clip.audio_gain = (target / peak).clamp(0.0, 2.0);
        Ok(())
    }

    fn audio_length_beats(path: &Path, tempo_bpm: f32) -> Option<f32> {
        let seconds = Self::audio_length_seconds(path)?;
        let beats = seconds * tempo_bpm.max(1.0) / 60.0;
        Some(beats.max(0.0))
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

    fn audio_length_seconds(path: &Path) -> Option<f32> {
        if path
            .extension()
            .and_then(|s| s.to_str())
            .map(|e| e.eq_ignore_ascii_case("wav"))
            .unwrap_or(false)
        {
            return Self::wav_length_seconds(path);
        }
        let (samples, channels, sample_rate) = Self::decode_audio_samples(path)?;
        if sample_rate == 0 || channels == 0 {
            return None;
        }
        let frames = samples.len() as f32 / channels as f32;
        Some((frames / sample_rate as f32).max(0.0))
    }

    fn decode_audio_samples(path: &Path) -> Option<(Vec<f32>, usize, u32)> {
        let is_wav = path
            .extension()
            .and_then(|s| s.to_str())
            .map(|e| e.eq_ignore_ascii_case("wav"))
            .unwrap_or(false);
        if is_wav {
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
            return Some((samples, channels, spec.sample_rate));
        }

        let file = std::fs::File::open(path).ok()?;
        let reader = BufReader::new(file);
        let decoder = Decoder::new(reader).ok()?;
        let channels = decoder.channels().max(1) as usize;
        let sample_rate = decoder.sample_rate();
        let samples: Vec<f32> = decoder.convert_samples::<f32>().collect();
        Some((samples, channels, sample_rate))
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

        let mut clip_notes_by_id: HashMap<(usize, usize), Vec<PianoRollNote>> = HashMap::new();
        let mut clip_notes_by_name: HashMap<(usize, String), Vec<PianoRollNote>> = HashMap::new();
        let mut track_notes: HashMap<usize, Vec<PianoRollNote>> = HashMap::new();

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
            if let Some(clip_id) = Self::clip_id_from_filename(file_name) {
                clip_notes_by_id.insert((track_index, clip_id), notes);
            } else {
                let clip_name = Path::new(file_name)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("MIDI")
                    .replace('_', " ");
                clip_notes_by_name.insert((track_index, clip_name.clone()), notes.clone());
                track_notes.entry(track_index).or_insert(notes);
            }
        }

        let mut next_clip_id = self.next_clip_id();
        let mut rebuild_indices = Vec::new();
        for (track_index, track) in self.tracks.iter_mut().enumerate() {
            for clip in track.clips.iter_mut() {
                clip.midi_notes.clear();
            }
            let mut any_clip_notes = false;
            if track.clips.is_empty() {
                if let Some(notes) = track_notes.remove(&track_index) {
                    let max_end: f32 = notes
                        .iter()
                        .map(|n| n.start_beats + n.length_beats)
                        .fold(1.0, |a, b| a.max(b));
                    track.clips.push(Clip {
                        id: next_clip_id,
                        track: track_index,
                        start_beats: 0.0,
                        length_beats: max_end.max(1.0),
                        is_midi: true,
                        midi_notes: notes.clone(),
                        midi_source_beats: Some(max_end.max(1.0)),
                        link_id: None,
                        name: "MIDI".to_string(),
                        audio_path: None,
                        audio_source_beats: None,
                        audio_offset_beats: 0.0,
                        audio_gain: 1.0,
                        audio_pitch_semitones: 0.0,
                        audio_time_mul: 1.0,
                    });
                    next_clip_id = next_clip_id.saturating_add(1);
                    any_clip_notes = true;
                }
            }
            for clip in track.clips.iter_mut() {
                if !clip.is_midi {
                    continue;
                }
                let mut clip_notes = clip_notes_by_id
                    .remove(&(track_index, clip.id))
                    .or_else(|| {
                        if clip.name.trim().is_empty() {
                            None
                        } else {
                            clip_notes_by_name.get(&(track_index, clip.name.clone())).cloned()
                        }
                    });
                if let Some(mut notes) = clip_notes.take() {
                    for note in &mut notes {
                        note.start_beats = (note.start_beats + clip.start_beats).max(0.0);
                    }
                    clip.midi_notes = notes;
                    if clip.midi_source_beats.is_none() {
                        clip.midi_source_beats = Some(clip.length_beats.max(0.25));
                    }
                    any_clip_notes = true;
                }
            }
            if !any_clip_notes {
                if let Some(notes) = track_notes.remove(&track_index) {
                    if track.clips.iter().any(|c| c.is_midi) {
                        for clip in track.clips.iter_mut() {
                            if !clip.is_midi {
                                continue;
                            }
                            let mut shifted_notes = Vec::new();
                            for note in &notes {
                                let mut shifted = note.clone();
                                shifted.start_beats = (shifted.start_beats + clip.start_beats).max(0.0);
                                shifted_notes.push(shifted);
                            }
                            clip.midi_notes = shifted_notes;
                            if clip.midi_source_beats.is_none() {
                                clip.midi_source_beats = Some(clip.length_beats.max(0.25));
                            }
                        }
                    } else {
                        let max_end: f32 = notes
                            .iter()
                            .map(|n| n.start_beats + n.length_beats)
                            .fold(1.0, |a, b| a.max(b));
                        track.clips.push(Clip {
                            id: next_clip_id,
                            track: track_index,
                            start_beats: 0.0,
                            length_beats: max_end.max(1.0),
                            is_midi: true,
                            midi_notes: notes.clone(),
                            midi_source_beats: Some(max_end.max(1.0)),
                            link_id: None,
                            name: "MIDI".to_string(),
                            audio_path: None,
                            audio_source_beats: None,
                            audio_offset_beats: 0.0,
                            audio_gain: 1.0,
                            audio_pitch_semitones: 0.0,
                            audio_time_mul: 1.0,
                        });
                        next_clip_id = next_clip_id.saturating_add(1);
                    }
                }
            }
            rebuild_indices.push(track_index);
        }

        for track_index in rebuild_indices {
            self.rebuild_track_midi_notes(track_index);
        }

        Ok(())
    }

    fn migrate_track_notes_to_clips(&mut self) {
        let mut next_clip_id = self.next_clip_id();
        let mut rebuild_indices = Vec::new();
        for (track_index, track) in self.tracks.iter_mut().enumerate() {
            if track.midi_notes.is_empty() {
                continue;
            }
            let has_clip_notes = track
                .clips
                .iter()
                .any(|clip| clip.is_midi && !clip.midi_notes.is_empty());
            if has_clip_notes {
                continue;
            }
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
                    midi_notes: track.midi_notes.clone(),
                    midi_source_beats: Some(max_end.max(1.0)),
                    link_id: None,
                    name: "MIDI".to_string(),
                    audio_path: None,
                    audio_source_beats: None,
                    audio_offset_beats: 0.0,
                    audio_gain: 1.0,
                    audio_pitch_semitones: 0.0,
                    audio_time_mul: 1.0,
                });
                next_clip_id = next_clip_id.saturating_add(1);
            } else {
                for clip in track.clips.iter_mut().filter(|c| c.is_midi) {
                    let clip_start = clip.start_beats;
                    let clip_end = clip.start_beats + clip.length_beats;
                    let mut notes = Vec::new();
                    for note in &track.midi_notes {
                        let note_end = note.start_beats + note.length_beats;
                        if note.start_beats < clip_end && note_end > clip_start {
                            notes.push(note.clone());
                        }
                    }
                    if !notes.is_empty() {
                        clip.midi_notes = notes;
                        if clip.midi_source_beats.is_none() {
                            clip.midi_source_beats = Some(clip.length_beats.max(0.25));
                        }
                    }
                }
            }
            rebuild_indices.push(track_index);
        }
        for track_index in rebuild_indices {
            self.rebuild_track_midi_notes(track_index);
        }
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

    fn clip_id_from_filename(file_name: &str) -> Option<usize> {
        let stem = Path::new(file_name).file_stem()?.to_str()?;
        let marker = "_clip";
        let pos = stem.rfind(marker)?;
        let digits = &stem[pos + marker.len()..];
        if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        digits.parse().ok()
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
        self.begin_midi_import_with_mode(path_str, MidiImportMode::ReplaceProject)
    }

    fn begin_midi_import_with_mode(
        &mut self,
        path_str: String,
        mode: MidiImportMode,
    ) -> Result<(), String> {
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
            mode,
        });
        self.show_midi_import = true;
        Ok(())
    }

    fn apply_midi_import(&mut self) -> Result<(), String> {
        let Some(state) = self.midi_import_state.take() else {
            return Ok(());
        };
        let was_running = self.audio_running;
        let append_mode = matches!(state.mode, MidiImportMode::AppendTracks { .. });
        if append_mode {
            if was_running {
                self.stop_audio_and_midi();
            }
        } else {
            self.prepare_for_project_change();
        }

        let mut next_id = self.next_clip_id();
        let mut tracks = Vec::new();
        let mut missing_plugins: HashSet<String> = HashSet::new();
        let insert_start = match state.mode {
            MidiImportMode::AppendTracks { start_beats } => start_beats.max(0.0),
            MidiImportMode::ReplaceProject => 0.0,
        };
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
            let mut notes = track_data.notes.clone();
            if insert_start > 0.0 {
                for note in &mut notes {
                    note.start_beats += insert_start;
                }
            }
            let clip = Clip {
                id: next_id,
                track: if append_mode {
                    self.tracks.len() + tracks.len()
                } else {
                    tracks.len()
                },
                start_beats: insert_start,
                length_beats: max_end.max(1.0),
                is_midi: true,
                midi_notes: notes,
                midi_source_beats: Some(max_end.max(1.0)),
                link_id: None,
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
                midi_notes: Vec::new(),
                instrument_path,
                instrument_clap_id: None,
                effect_paths: Vec::new(),
                effect_clap_ids: Vec::new(),
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
            let start_index = if append_mode {
                let base = self.tracks.len();
                self.tracks.extend(tracks);
                base
            } else {
                self.tracks = tracks;
                0
            };
            self.selected_track = Some(start_index);
            self.selected_clip = self
                .tracks
                .get(start_index)
                .and_then(|t| t.clips.first())
                .map(|c| c.id);
            self.sync_track_audio_states();
            self.ensure_builtin_gm_presets();
            for index in 0..self.tracks.len() {
                let mut should_refresh = false;
                if let Some(track) = self.tracks.get(index) {
                    if track.instrument_path.is_some() && track.midi_program.is_some() {
                        should_refresh = true;
                    }
                }
                if should_refresh {
                    self.refresh_track_params(index);
                    self.apply_micesynth_program_from_midi(index);
                    if let Some(program) = self.tracks.get(index).and_then(|t| t.midi_program) {
                        let _ = self.load_preset_for_program(index, program);
                    }
                }
            }
            if missing_plugins.is_empty() {
                self.status = if append_mode {
                    "MIDI imported (appended)".to_string()
                } else {
                    "MIDI imported".to_string()
                };
            } else {
                let mut missing: Vec<String> = missing_plugins.into_iter().collect();
                missing.sort();
                let suffix = if append_mode { " (appended)" } else { "" };
                self.status = format!("MIDI imported{suffix} (missing: {})", missing.join(", "));
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
        self.ensure_synth_soundfont();
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
        if self.render_tail_mode == RenderTailMode::Release {
            let tail_beats = (self.render_release_seconds.max(0.0) * self.tempo_bpm.max(1.0) / 60.0)
                .max(0.0);
            range_end = range_end.max(range_start + 0.25) + tail_beats;
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
                instrument_clap_id: track.instrument_clap_id.clone(),
                param_ids: track.param_ids.clone(),
                param_values: track.param_values.clone(),
                plugin_state_component: track.plugin_state_component.clone(),
                plugin_state_controller: track.plugin_state_controller.clone(),
                effect_paths: track.effect_paths.clone(),
                effect_clap_ids: track.effect_clap_ids.clone(),
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
            wav_bit_depth: self.render_wav_bit_depth,
            render_tail_mode: self.render_tail_mode,
            render_release_seconds: self.render_release_seconds,
            tracks,
            notes: Vec::new(),
            instrument_path: None,
            param_ids: Vec::new(),
            param_values: Vec::new(),
            plugin_state_component: None,
            plugin_state_controller: None,
            audio_clips,
            audio_cache,
            master_settings: self.master_settings_snapshot(),
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
        let (notes, instrument_path, instrument_clap_id, param_ids, param_values, component, controller, automation_lanes) = self
            .tracks
            .get(index)
            .map(|track| {
                (
                    track.midi_notes.clone(),
                    track.instrument_path.clone(),
                    track.instrument_clap_id.clone(),
                    track.param_ids.clone(),
                    track.param_values.clone(),
                    track.plugin_state_component.clone(),
                    track.plugin_state_controller.clone(),
                    track.automation_lanes.clone(),
                )
            })
            .unwrap_or_else(|| (Vec::new(), None, None, Vec::new(), Vec::new(), None, None, Vec::new()));
        let (effect_paths, effect_bypass, effect_clap_ids) = self
            .tracks
            .get(index)
            .map(|track| (track.effect_paths.clone(), track.effect_bypass.clone(), track.effect_clap_ids.clone()))
            .unwrap_or_else(|| (Vec::new(), Vec::new(), Vec::new()));
        let (audio_clips, audio_cache) =
            self.build_audio_clip_render_data(sample_rate, Some(index));
        let track = RenderTrack {
            notes,
            instrument_path,
            instrument_clap_id,
            param_ids,
            param_values,
            plugin_state_component: component,
            plugin_state_controller: controller,
            effect_paths,
            effect_clap_ids,
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
            wav_bit_depth: self.render_wav_bit_depth,
            render_tail_mode: self.render_tail_mode,
            render_release_seconds: self.render_release_seconds,
            tracks: vec![track],
            notes: Vec::new(),
            instrument_path: None,
            param_ids: Vec::new(),
            param_values: Vec::new(),
            plugin_state_component: None,
            plugin_state_controller: None,
            audio_clips,
            audio_cache,
            master_settings: self.master_settings_snapshot(),
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

    fn ensure_synth_soundfont(&self) {
        let cwd = match std::env::current_dir() {
            Ok(dir) => dir,
            Err(_) => return,
        };
        let synths_root = cwd.join("synths");
        if !synths_root.exists() {
            return;
        }
        let candidates = [
            synths_root.join("FishSynth").join("Ling.sf2"),
            synths_root
                .join("FishSynth")
                .join("FishSynth.vst3")
                .join("Ling.sf2"),
            synths_root
                .join("FishSynth")
                .join("FishSynth.vst3")
                .join("Contents")
                .join("x86_64-win")
                .join("SF")
                .join("Ling.sf2"),
            synths_root
                .join("FishSynth")
                .join("FishSynth.vst3")
                .join("Contents")
                .join("x86_64-win")
                .join("Resources")
                .join("SF")
                .join("Ling.sf2"),
        ];
        let source = candidates.iter().find(|path| path.exists()).cloned();
        let Some(source) = source else {
            return;
        };
        let targets = [
            cwd.join("Ling.sf2"),
            cwd.join("SF").join("Ling.sf2"),
            cwd.join("Resources").join("SF").join("Ling.sf2"),
            synths_root.join("Ling.sf2"),
            synths_root.join("MiceSynth").join("Ling.sf2"),
            synths_root
                .join("MiceSynth")
                .join("MiceSynth.vst3")
                .join("Contents")
                .join("x86_64-win")
                .join("SF")
                .join("Ling.sf2"),
            synths_root
                .join("MiceSynth")
                .join("MiceSynth.vst3")
                .join("Contents")
                .join("x86_64-win")
                .join("Resources")
                .join("SF")
                .join("Ling.sf2"),
        ];
        for target in targets {
            if target.exists() {
                continue;
            }
            if let Some(parent) = target.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::copy(&source, &target);
        }
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
        self.ensure_synth_soundfont();
        let channels = 2u16;
        let tempo = self.tempo_bpm.max(1.0);
        let beats = self.project_end_beats().max(1.0);
        let samples_per_beat = sample_rate as f64 * 60.0 / tempo as f64;
        let total_samples = (beats as f64 * samples_per_beat).ceil() as usize;
        let block_size = self.settings.buffer_size.max(64) as usize;
        let total_samples_u64 = total_samples as u64;
        self.render_progress = Some((0, total_samples_u64));

        let spec = wav_spec_for_depth(sample_rate, channels, self.render_wav_bit_depth);
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
        let master_settings = self.master_settings_snapshot();
        let mut master_state = MasterCompState::default();
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
            apply_master_processing(
                &mut output,
                channels as usize,
                sample_rate as f32,
                &master_settings,
                &mut master_state,
            );
            write_wav_samples(&mut writer, self.render_wav_bit_depth, &output)?;
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
                midi_notes: Vec::new(),
                midi_source_beats: None,
                link_id: None,
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
            track.clips.push(Clip {
                id: clip_id,
                track: track_index,
                start_beats,
                length_beats: (max_end - start_beats).max(0.25),
                is_midi: true,
                midi_notes: notes,
                midi_source_beats: Some((max_end - start_beats).max(0.25)),
                link_id: None,
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
        let master_settings = self.master_settings.clone();
        let master_comp_state = self.master_comp_state.clone();
        self.adaptive_buffer_size
            .store(effective_buffer, Ordering::Relaxed);
        self.last_overrun.store(false, Ordering::Relaxed);
        if reset_transport {
            self.transport_samples.store(0, Ordering::Relaxed);
            self.playback_panic.store(true, Ordering::Relaxed);
            self.playback_fade_in.store(true, Ordering::Relaxed);
        }
        self.ensure_synth_soundfont();
        self.tempo_bits.store(self.tempo_bpm.to_bits(), Ordering::Relaxed);
        self.sync_track_audio_states();
        let timeline = self.build_audio_clip_timeline(self.settings.sample_rate);
        if let Ok(mut guard) = self.audio_clip_timeline.lock() {
            *guard = timeline;
        }
        self.preload_audio_clips(&self.audio_clip_cache);
        let mut micesynth_program_sync: Vec<usize> = Vec::new();
        for index in 0..self.tracks.len() {
            let path = self.tracks[index].instrument_path.clone();
            let effect_paths = self.tracks[index].effect_paths.clone();
            let sync_micesynth_program = self
                .tracks
                .get(index)
                .and_then(|track| track.instrument_path.as_deref())
                .map(Self::is_micesynth_path)
                .unwrap_or(false)
                && self.tracks.get(index).and_then(|track| track.midi_program).is_some();
            let state = match self.track_audio.get_mut(index) {
                Some(state) => state,
                None => continue,
            };
            if let Some(path) = path {
                if state.host.is_none() {
                    let kind = Self::plugin_kind_from_path(&path);
                    let host = match kind {
                        PluginKind::Vst3 => vst3::Vst3Host::load(
                            &path,
                            self.settings.sample_rate as f64,
                            buffer_size_usize,
                            channels,
                        )
                        .ok()
                        .map(|host| PluginHostHandle::Vst3(Arc::new(Mutex::new(host)))),
                        PluginKind::Clap => {
                            let clap_id = self
                                .tracks
                                .get(index)
                                .and_then(|track| track.instrument_clap_id.clone())
                                .or_else(|| clap_host::default_plugin_id(&path).ok());
                            clap_id.and_then(|clap_id| {
                                if let Some(track) = self.tracks.get_mut(index) {
                                    track.instrument_clap_id = Some(clap_id.clone());
                                }
                                clap_host::ClapHost::load(
                                    &path,
                                    &clap_id,
                                    self.settings.sample_rate as f64,
                                    buffer_size_usize as u32,
                                    channels,
                                    channels,
                                )
                                .ok()
                                .map(|host| PluginHostHandle::Clap(Arc::new(Mutex::new(host))))
                            })
                        }
                    };
                    if let Some(host) = host {
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
                        state.host = Some(host.clone());
                        if sync_micesynth_program {
                            micesynth_program_sync.push(index);
                        }
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
                    } else {
                        self.status = "Plugin host error: unable to load".to_string();
                    }
                }
            } else {
                state.host = None;
            }
            if state.effect_hosts.len() != effect_paths.len() {
                for host in state.effect_hosts.drain(..) {
                    host.prepare_for_drop();
                    self.orphaned_hosts.push(host);
                }
                for (slot, fx_path) in effect_paths.iter().enumerate() {
                    let kind = Self::plugin_kind_from_path(fx_path);
                    let host = match kind {
                        PluginKind::Vst3 => vst3::Vst3Host::load_with_input(
                            fx_path,
                            self.settings.sample_rate as f64,
                            buffer_size_usize,
                            channels,
                            channels,
                        )
                        .ok()
                        .map(|host| PluginHostHandle::Vst3(Arc::new(Mutex::new(host)))),
                        PluginKind::Clap => {
                            let clap_id = self
                                .tracks
                                .get(index)
                                .and_then(|track| track.effect_clap_ids.get(slot).and_then(|id| id.clone()))
                                .or_else(|| clap_host::default_plugin_id(fx_path).ok());
                            clap_id.and_then(|clap_id| {
                                if let Some(track) = self.tracks.get_mut(index) {
                                    if track.effect_clap_ids.len() < effect_paths.len() {
                                        track.effect_clap_ids.resize(effect_paths.len(), None);
                                    }
                                    track.effect_clap_ids[slot] = Some(clap_id.clone());
                                }
                                clap_host::ClapHost::load(
                                    fx_path,
                                    &clap_id,
                                    self.settings.sample_rate as f64,
                                    buffer_size_usize as u32,
                                    channels,
                                    channels,
                                )
                                .ok()
                                .map(|host| PluginHostHandle::Clap(Arc::new(Mutex::new(host))))
                            })
                        }
                    };
                    if let Some(host) = host {
                        state.effect_hosts.push(host);
                    } else {
                        self.status = "FX host error: unable to load".to_string();
                    }
                }
            }
            if let Some(track) = self.tracks.get_mut(index) {
                if track.effect_bypass.len() != effect_paths.len() {
                    track.effect_bypass.resize(effect_paths.len(), false);
                }
                if track.effect_clap_ids.len() != effect_paths.len() {
                    track.effect_clap_ids.resize(effect_paths.len(), None);
                }
                if track.effect_params.len() != effect_paths.len() {
                    track.effect_params.resize(effect_paths.len(), Vec::new());
                    track.effect_param_ids.resize(effect_paths.len(), Vec::new());
                    track.effect_param_values.resize(effect_paths.len(), Vec::new());
                }
                for (fx_index, fx_host) in state.effect_hosts.iter().enumerate() {
                    let params = fx_host.enumerate_params();
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
                state.sync_effect_bypass(track);
            }
        }
        for index in micesynth_program_sync {
            self.apply_micesynth_program_from_midi(index);
        }
        self.send_midi_stop_to_hosts();
        self.warmup_hosts(channels, buffer_size_usize, 2);
        let track_audio = self.track_audio.clone();
        let track_mix = self.track_mix.clone();
        let tempo_bits = self.tempo_bits.clone();
        let transport_samples = self.transport_samples.clone();
        let loop_start_samples = self.loop_start_samples.clone();
        let loop_end_samples = self.loop_end_samples.clone();
        let playback_panic = self.playback_panic.clone();
        let playback_fade_in = self.playback_fade_in.clone();
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
                            &playback_panic,
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
                        let settings = master_settings.lock().map(|s| s.clone()).unwrap_or_default();
                        if let Ok(mut state) = master_comp_state.lock() {
                            apply_master_processing(
                                data,
                                channels,
                                sample_rate,
                                &settings,
                                &mut state,
                            );
                        }
                        apply_fade_in_if_needed(data, channels, &playback_fade_in);
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
                            &playback_panic,
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
                        let settings = master_settings.lock().map(|s| s.clone()).unwrap_or_default();
                        if let Ok(mut state) = master_comp_state.lock() {
                            apply_master_processing(
                                &mut temp,
                                channels,
                                sample_rate,
                                &settings,
                                &mut state,
                            );
                        }
                        apply_fade_in_if_needed(&mut temp, channels, &playback_fade_in);
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
                            &playback_panic,
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
                        let settings = master_settings.lock().map(|s| s.clone()).unwrap_or_default();
                        if let Ok(mut state) = master_comp_state.lock() {
                            apply_master_processing(
                                &mut temp,
                                channels,
                                sample_rate,
                                &settings,
                                &mut state,
                            );
                        }
                        apply_fade_in_if_needed(&mut temp, channels, &playback_fade_in);
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

    fn pause_audio_and_midi(&mut self) {
        if !self.audio_running {
            return;
        }
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
        self.send_midi_stop_to_hosts();
        self.midi_gate.store(false, Ordering::Relaxed);
        for state in &self.track_audio {
            if let Ok(mut events) = state.midi_events.lock() {
                events.clear();
            }
        }
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
            let _ = host.process_f32(&mut buffer, channels, &events);
        }
    }

    fn warmup_hosts(&mut self, channels: usize, block_size: usize, blocks: usize) {
        if channels == 0 || block_size == 0 || blocks == 0 {
            return;
        }
        let frames = block_size.max(1);
        let mut silence = vec![0.0f32; frames * channels];
        let mut scratch = vec![0.0f32; frames * channels];
        let events: [vst3::MidiEvent; 0] = [];
        for _ in 0..blocks {
            for state in &self.track_audio {
                if let Some(host) = state.host.as_ref() {
                    silence.fill(0.0);
                    let _ = host.process_f32(&mut silence, channels, &events);
                }
                for fx in &state.effect_hosts {
                    silence.fill(0.0);
                    scratch.fill(0.0);
                    let _ = fx.process_f32_with_input(&silence, &mut scratch, channels, &events);
                }
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
            .add_filter("Plugins", &["vst3", "clap"])
            .pick_file();
        path.map(|p| p.to_string_lossy().to_string())
    }

    fn open_plugin_picker(&mut self, target: PluginTarget) {
        self.plugin_target = Some(target);
        if self.plugin_candidates.is_empty() {
            self.plugin_candidates = self.scan_plugins();
        }
        self.show_plugin_picker = true;
    }

    fn scan_plugins(&self) -> Vec<PluginCandidate> {
        let mut candidates = Vec::new();
        let mut vst3_paths = Vec::new();
        for root in self.vst3_search_roots() {
            self.scan_dir_for_exts(&root, &mut vst3_paths, &["vst3"]);
        }
        vst3_paths.sort();
        for path in vst3_paths {
            let display = Self::plugin_display_name(&path);
            candidates.push(PluginCandidate {
                path,
                kind: PluginKind::Vst3,
                clap_id: None,
                display,
            });
        }

        let mut clap_paths = Vec::new();
        for root in self.clap_search_roots() {
            self.scan_dir_for_exts(&root, &mut clap_paths, &["clap"]);
        }
        clap_paths.sort();
        for path in clap_paths {
            match clap_host::enumerate_plugins(&path) {
                Ok(descriptors) if !descriptors.is_empty() => {
                    for desc in descriptors {
                        let display = format!("{} (CLAP)", desc.name);
                        candidates.push(PluginCandidate {
                            path: path.clone(),
                            kind: PluginKind::Clap,
                            clap_id: Some(desc.id),
                            display,
                        });
                    }
                }
                _ => {
                    let display = format!("{} (CLAP)", Self::plugin_display_name(&path));
                    candidates.push(PluginCandidate {
                        path: path.clone(),
                        kind: PluginKind::Clap,
                        clap_id: None,
                        display,
                    });
                }
            }
        }

        candidates.sort_by(|a, b| a.display.to_ascii_lowercase().cmp(&b.display.to_ascii_lowercase()));
        candidates
    }

    fn plugin_kind_from_path(path: &str) -> PluginKind {
        let ext = Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ext == "clap" {
            PluginKind::Clap
        } else {
            PluginKind::Vst3
        }
    }

    fn enumerate_plugin_params_for_track(
        &mut self,
        index: usize,
        path: &str,
    ) -> Result<Vec<vst3::ParamInfo>, String> {
        match Self::plugin_kind_from_path(path) {
            PluginKind::Vst3 => vst3::enumerate_params(path),
            PluginKind::Clap => {
                let clap_id = self
                    .tracks
                    .get(index)
                    .and_then(|t| t.instrument_clap_id.clone())
                    .or_else(|| clap_host::default_plugin_id(path).ok())
                    .ok_or_else(|| "CLAP plugin id not found".to_string())?;
                if let Some(track) = self.tracks.get_mut(index) {
                    track.instrument_clap_id = Some(clap_id.clone());
                }
                let mut host = clap_host::ClapHost::load(
                    path,
                    &clap_id,
                    self.settings.sample_rate as f64,
                    self.settings.buffer_size as u32,
                    2,
                    2,
                )
                .map_err(|e| e.to_string())?;
                let params = host
                    .enumerate_params()
                    .into_iter()
                    .map(|param| vst3::ParamInfo {
                        id: param.id,
                        name: param.name,
                        default_value: param.default_value,
                    })
                    .collect();
                Ok(params)
            }
        }
    }

    fn vst3_search_roots(&self) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        #[cfg(windows)]
        {
            roots.push(PathBuf::from("C:\\Program Files\\Common Files\\VST3"));
        }
        #[cfg(target_os = "macos")]
        {
            roots.push(PathBuf::from("/Library/Audio/Plug-Ins/VST3"));
            if let Some(home) = Self::home_dir() {
                roots.push(home.join("Library/Audio/Plug-Ins/VST3"));
            }
        }
        #[cfg(target_os = "linux")]
        {
            roots.push(PathBuf::from("/usr/lib/vst3"));
            roots.push(PathBuf::from("/usr/local/lib/vst3"));
            if let Some(home) = Self::home_dir() {
                roots.push(home.join(".vst3"));
            }
        }
        roots.push(PathBuf::from("synths"));
        roots
    }

    fn clap_search_roots(&self) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        #[cfg(windows)]
        {
            roots.push(PathBuf::from("C:\\Program Files\\Common Files\\CLAP"));
        }
        #[cfg(target_os = "macos")]
        {
            roots.push(PathBuf::from("/Library/Audio/Plug-Ins/CLAP"));
            if let Some(home) = Self::home_dir() {
                roots.push(home.join("Library/Audio/Plug-Ins/CLAP"));
            }
        }
        #[cfg(target_os = "linux")]
        {
            roots.push(PathBuf::from("/usr/lib/clap"));
            roots.push(PathBuf::from("/usr/local/lib/clap"));
            if let Some(home) = Self::home_dir() {
                roots.push(home.join(".clap"));
            }
        }
        roots.push(PathBuf::from("synths"));
        roots
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
            self.plugin_candidates = self.scan_plugins();
        }
        let needle = name.to_ascii_lowercase();
        self.plugin_candidates
            .iter()
            .filter(|candidate| candidate.kind == PluginKind::Vst3)
            .find(|candidate| {
                let display = candidate.display.to_ascii_lowercase();
                display == needle || display.contains(&needle)
            })
            .map(|candidate| candidate.path.clone())
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

    fn presets_root_global(&self) -> PathBuf {
        let base = Path::new(self.settings_path())
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("presets")
    }

    fn presets_root_project(&self) -> Option<PathBuf> {
        let trimmed = self.project_path.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(Self::normalize_windows_path(&PathBuf::from(trimmed)).join("presets"))
    }

    fn preset_plugin_dir(&self, root: &Path, plugin_path: &str) -> PathBuf {
        let name = Self::plugin_display_name(plugin_path);
        let safe = Self::sanitize_folder_name(&name);
        root.join(safe)
    }

    fn preset_file_path(
        &self,
        root: &Path,
        plugin_path: &str,
        preset_name: &str,
    ) -> Result<PathBuf, String> {
        let safe = Self::sanitize_folder_name(preset_name);
        if safe.trim().is_empty() {
            return Err("Preset name required".to_string());
        }
        let file_name = format!("{}.lingpreset.json", safe);
        Ok(self.preset_plugin_dir(root, plugin_path).join(file_name))
    }

    fn preset_name_for_program(&self, program: u8) -> String {
        format!("{:03} {}", program + 1, gm_program_name(program))
    }

    fn preset_path_for_program(&self, root: &Path, plugin_path: &str, program: u8) -> PathBuf {
        let name = self.preset_name_for_program(program);
        self.preset_plugin_dir(root, plugin_path)
            .join(format!("{}.lingpreset.json", Self::sanitize_folder_name(&name)))
    }

    fn load_preset_for_program(&mut self, index: usize, program: u8) -> Result<(), String> {
        let plugin_path = self
            .tracks
            .get(index)
            .and_then(|t| t.instrument_path.as_deref())
            .ok_or_else(|| "No instrument loaded".to_string())?
            .to_string();

        if let Some(project_root) = self.presets_root_project() {
            let project_path = self.preset_path_for_program(&project_root, &plugin_path, program);
            if project_path.exists() {
                return self.load_preset_from_path(index, &project_path);
            }
        }

        let global_root = self.presets_root_global();
        let global_path = self.preset_path_for_program(&global_root, &plugin_path, program);
        if global_path.exists() {
            return self.load_preset_from_path(index, &global_path);
        }

        self.ensure_gm_presets_for_plugin(&plugin_path, program)?;

        if let Some(project_root) = self.presets_root_project() {
            let project_path = self.preset_path_for_program(&project_root, &plugin_path, program);
            if project_path.exists() {
                return self.load_preset_from_path(index, &project_path);
            }
        }
        let global_root = self.presets_root_global();
        let global_path = self.preset_path_for_program(&global_root, &plugin_path, program);
        if global_path.exists() {
            return self.load_preset_from_path(index, &global_path);
        }

        Err("Preset file not found".to_string())
    }

    fn save_preset_for_track(
        &mut self,
        index: usize,
        root: PathBuf,
        preset_name: &str,
    ) -> Result<String, String> {
        let track = self
            .tracks
            .get(index)
            .ok_or_else(|| "Track not found".to_string())?;
        let plugin_path = track
            .instrument_path
            .as_deref()
            .ok_or_else(|| "No instrument loaded".to_string())?;
        let preset_path = self.preset_file_path(&root, plugin_path, preset_name)?;
        if let Some(parent) = preset_path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        let (mut component_bytes, mut controller_bytes) = (Vec::new(), Vec::new());
        if let Some(host) = self.track_audio.get(index).and_then(|state| state.host.as_ref()) {
            let (component, controller) = host.get_state_bytes();
            component_bytes = component;
            controller_bytes = controller;
        }
        if component_bytes.is_empty() {
            if let Some(bytes) = track.plugin_state_component.as_ref() {
                component_bytes = bytes.clone();
            }
        }
        if controller_bytes.is_empty() {
            if let Some(bytes) = track.plugin_state_controller.as_ref() {
                controller_bytes = bytes.clone();
            }
        }

        let preset = Vst3PresetFile {
            version: 1,
            name: preset_name.to_string(),
            plugin: Self::plugin_display_name(plugin_path),
            param_names: track.params.clone(),
            param_ids: track.param_ids.clone(),
            param_values: track.param_values.clone(),
            component_state: BASE64.encode(&component_bytes),
            controller_state: BASE64.encode(&controller_bytes),
        };

        let json = serde_json::to_string_pretty(&preset).map_err(|e| e.to_string())?;
        fs::write(&preset_path, json).map_err(|e| e.to_string())?;
        Ok(preset_path.to_string_lossy().to_string())
    }

    fn load_preset_from_path(&mut self, index: usize, path: &Path) -> Result<(), String> {
        let track = self
            .tracks
            .get(index)
            .ok_or_else(|| "Track not found".to_string())?;
        let plugin_path = track
            .instrument_path
            .as_deref()
            .ok_or_else(|| "No instrument loaded".to_string())?;
        let data = fs::read_to_string(path).map_err(|e| e.to_string())?;
        let preset: Vst3PresetFile = serde_json::from_str(&data).map_err(|e| e.to_string())?;
        let expected = Self::plugin_display_name(plugin_path).to_ascii_lowercase();
        let actual = preset.plugin.to_ascii_lowercase();
        if expected != actual {
            return Err("Preset plugin does not match current instrument".to_string());
        }

        let component_bytes = if preset.component_state.trim().is_empty() {
            Vec::new()
        } else {
            BASE64
                .decode(preset.component_state.as_bytes())
                .map_err(|e| e.to_string())?
        };
        let controller_bytes = if preset.controller_state.trim().is_empty() {
            Vec::new()
        } else {
            BASE64
                .decode(preset.controller_state.as_bytes())
                .map_err(|e| e.to_string())?
        };

        if let Some(track) = self.tracks.get_mut(index) {
            if !component_bytes.is_empty() {
                track.plugin_state_component = Some(component_bytes.clone());
            }
            if !controller_bytes.is_empty() {
                track.plugin_state_controller = Some(controller_bytes.clone());
            }

            if !preset.param_ids.is_empty() && !preset.param_values.is_empty() {
                if track.param_ids.is_empty() || track.param_ids.len() != track.param_values.len() {
                    track.param_ids = preset.param_ids.clone();
                    track.param_values = preset.param_values.clone();
                } else {
                    let mut map = HashMap::new();
                    for (id, value) in preset.param_ids.iter().zip(preset.param_values.iter()) {
                        map.insert(*id, *value);
                    }
                    if track.param_values.len() != track.param_ids.len() {
                        track.param_values.resize(track.param_ids.len(), 0.0);
                    }
                    for (slot, param_id) in track.param_ids.iter().enumerate() {
                        if let Some(value) = map.get(param_id).copied() {
                            if let Some(target) = track.param_values.get_mut(slot) {
                                *target = value;
                            }
                        }
                    }
                }
            } else if !preset.param_names.is_empty() && !preset.param_values.is_empty() {
                if track.param_values.len() != track.params.len() {
                    track.param_values.resize(track.params.len(), 0.0);
                }
                let mut map = HashMap::new();
                for (name, value) in preset.param_names.iter().zip(preset.param_values.iter()) {
                    map.insert(Self::normalize_param_name(name), *value);
                }
                for (slot, name) in track.params.iter().enumerate() {
                    let key = Self::normalize_param_name(name);
                    if let Some(value) = map.get(&key).copied() {
                        if let Some(target) = track.param_values.get_mut(slot) {
                            *target = value;
                        }
                    }
                }
            }
        }

        if let Some(host) = self.track_audio.get(index).and_then(|state| state.host.as_ref()) {
            if !component_bytes.is_empty() || !controller_bytes.is_empty() {
                let _ = host.set_state_bytes(
                    if component_bytes.is_empty() {
                        None
                    } else {
                        Some(component_bytes.as_slice())
                    },
                    if controller_bytes.is_empty() {
                        None
                    } else {
                        Some(controller_bytes.as_slice())
                    },
                );
            } else if let Some(track) = self.tracks.get(index) {
                for (param_id, value) in track.param_ids.iter().zip(track.param_values.iter()) {
                    host.push_param_change(*param_id, *value as f64);
                }
            }

            if let Some(track) = self.tracks.get_mut(index) {
                for (slot, param_id) in track.param_ids.iter().enumerate() {
                    if let Some(value) = host.get_param_normalized(*param_id) {
                        if let Some(target) = track.param_values.get_mut(slot) {
                            *target = value as f32;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn normalize_param_name(name: &str) -> String {
        name.to_ascii_lowercase()
            .replace(' ', "")
            .replace('_', "")
            .replace('-', "")
    }

    fn ensure_gm_presets_for_plugin(
        &mut self,
        plugin_path: &str,
        requested_program: u8,
    ) -> Result<(), String> {
        let params = match Self::plugin_kind_from_path(plugin_path) {
            PluginKind::Vst3 => vst3::enumerate_params(plugin_path)?,
            PluginKind::Clap => {
                let clap_id = clap_host::default_plugin_id(plugin_path)?;
                let mut host = clap_host::ClapHost::load(
                    plugin_path,
                    &clap_id,
                    self.settings.sample_rate as f64,
                    self.settings.buffer_size as u32,
                    2,
                    2,
                )?;
                host.enumerate_params()
                    .into_iter()
                    .map(|param| vst3::ParamInfo {
                        id: param.id,
                        name: param.name,
                        default_value: param.default_value,
                    })
                    .collect()
            }
        };
        if params.is_empty() {
            return Err("Preset generation failed: no parameters".to_string());
        }

        let mut roots = Vec::new();
        roots.push(self.presets_root_global());
        if let Some(project_root) = self.presets_root_project() {
            roots.push(project_root);
        }

        for root in &roots {
            let preset_path = self.preset_path_for_program(root, plugin_path, requested_program);
            if preset_path.exists() {
                continue;
            }
            if let Some(parent) = preset_path.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
        }

        let targets: Vec<u8> = (0u8..=127).collect();
        for root in &roots {
            for program in &targets {
                let preset_path = self.preset_path_for_program(root, plugin_path, *program);
                if preset_path.exists() {
                    continue;
                }
                let preset = self.build_gm_preset_file(plugin_path, &params, *program);
                let json = serde_json::to_string_pretty(&preset).map_err(|e| e.to_string())?;
                fs::write(&preset_path, json).map_err(|e| e.to_string())?;
            }
        }

        Ok(())
    }

    fn ensure_builtin_gm_presets(&mut self) {
        if self.gm_presets_generated {
            return;
        }
        let synths = [
            "FishSynth",
            "CatSynth",
            "SannySynth",
            "DogSynth",
            "LingSynth",
            "MiceSynth",
        ];
        for name in synths {
            if let Some(path) = self.find_vst3_plugin_by_name(name) {
                let _ = self.ensure_gm_presets_for_plugin(&path, 0);
            }
        }
        self.gm_presets_generated = true;
    }

    fn build_gm_preset_file(
        &self,
        plugin_path: &str,
        params: &[vst3::ParamInfo],
        program: u8,
    ) -> Vst3PresetFile {
        let mut values: Vec<f32> = Vec::with_capacity(params.len());
        for param in params {
            let value = self.gm_param_value(&param.name, param.default_value as f32, program);
            values.push(value);
        }
        Vst3PresetFile {
            version: 1,
            name: self.preset_name_for_program(program),
            plugin: Self::plugin_display_name(plugin_path),
            param_names: params.iter().map(|p| p.name.clone()).collect(),
            param_ids: params.iter().map(|p| p.id).collect(),
            param_values: values,
            component_state: String::new(),
            controller_state: String::new(),
        }
    }

    fn gm_param_value(&self, name: &str, default_value: f32, program: u8) -> f32 {
        let category = GmCategory::from_program(program);
        let values = GmParamValues::from_category(category);
        let key = Self::normalize_param_name(name);

        if key.contains("preset") || key.contains("program") || key.contains("patch") {
            return (program as f32 / 127.0).clamp(0.0, 1.0);
        }
        if key.contains("gain") || key.contains("volume") || key.contains("master") {
            return values.gain;
        }
        if key.contains("attack") || key.ends_with("atk") || key.contains("_atk") {
            return values.attack;
        }
        if key.contains("decay") || key.ends_with("dec") || key.contains("_dec") {
            return values.decay;
        }
        if key.contains("sustain") || key.ends_with("sus") || key.contains("_sus") {
            return values.sustain;
        }
        if key.contains("release") || key.ends_with("rel") || key.contains("_rel") {
            return values.release;
        }
        if key.contains("cutoff") || key.contains("filtercut") || key.contains("filter_cut") || key.contains("filtercutoff") || key.contains("cut") {
            return values.cutoff;
        }
        if key.contains("resonance") || key.contains("filterres") || key.contains("filter_res") || key.contains("res") {
            return values.resonance;
        }
        if key.contains("vibrato") && key.contains("rate") {
            return values.vibrato_rate;
        }
        if key.contains("vibrato") && (key.contains("int") || key.contains("amount")) {
            return values.vibrato_intensity;
        }
        if key.contains("tremolo") && key.contains("rate") {
            return values.tremolo_rate;
        }
        if key.contains("tremolo") && (key.contains("int") || key.contains("amount")) {
            return values.tremolo_intensity;
        }

        default_value.clamp(0.0, 1.0)
    }

    fn is_micesynth_path(path: &str) -> bool {
        path.to_ascii_lowercase().contains("micesynth")
    }

    fn apply_micesynth_program_from_midi(&mut self, index: usize) {
        let (program, path, params, param_ids, mut param_values, has_state) =
            match self.tracks.get(index) {
                Some(track) => (
                    track.midi_program,
                    track.instrument_path.clone(),
                    track.params.clone(),
                    track.param_ids.clone(),
                    track.param_values.clone(),
                    track
                        .plugin_state_component
                        .as_ref()
                        .map(|v| !v.is_empty())
                        .unwrap_or(false)
                        || track
                            .plugin_state_controller
                            .as_ref()
                            .map(|v| !v.is_empty())
                            .unwrap_or(false),
                ),
                None => return,
            };
        let Some(program) = program else {
            return;
        };
        let Some(path) = path else {
            return;
        };
        if has_state {
            return;
        }
        if !Self::is_micesynth_path(&path) {
            return;
        }
        let program_index = params.iter().position(|name| {
            let name = name.to_ascii_lowercase();
            name.contains("program") || name.contains("patch") || name.contains("preset")
        });
        let Some(program_index) = program_index else {
            return;
        };
        let Some(program_param_id) = param_ids.get(program_index).copied() else {
            return;
        };
        if param_values.len() != param_ids.len() {
            param_values.resize(param_ids.len(), 0.0);
        }
        let normalized = (program as f64 / 127.0).clamp(0.0, 1.0);
        if let Some(value) = param_values.get_mut(program_index) {
            *value = normalized as f32;
        }

        if let Some(host) = self.track_audio.get(index).and_then(|state| state.host.as_ref()) {
            host.push_param_change(program_param_id, normalized);
            for (slot, param_id) in param_ids.iter().enumerate() {
                if let Some(value) = host.get_param_normalized(*param_id) {
                    if let Some(target) = param_values.get_mut(slot) {
                        *target = value as f32;
                    }
                }
            }
        }

        if let Some(track) = self.tracks.get_mut(index) {
            track.param_values = param_values;
        }
    }

    fn scan_dir(&self, dir: &Path, out: &mut Vec<String>) {
        self.scan_dir_for_exts(dir, out, &["vst3"]);
    }

    fn scan_dir_for_exts(&self, dir: &Path, out: &mut Vec<String>, exts: &[&str]) {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let matches_ext = exts.iter().any(|e| *e == ext);
            if path.is_dir() {
                if matches_ext {
                    out.push(path.to_string_lossy().to_string());
                    continue;
                }
                self.scan_dir_for_exts(&path, out, exts);
            } else if matches_ext {
                out.push(path.to_string_lossy().to_string());
            }
        }
    }

    fn home_dir() -> Option<PathBuf> {
        if cfg!(windows) {
            std::env::var("USERPROFILE").ok().map(PathBuf::from)
        } else {
            std::env::var("HOME").ok().map(PathBuf::from)
        }
    }

    fn refresh_track_params(&mut self, index: usize) {
        let path = self
            .tracks
            .get(index)
            .and_then(|track| track.instrument_path.clone());
        let Some(path) = path else {
            if let Some(track) = self.tracks.get_mut(index) {
                track.params = default_midi_params();
                track.param_ids.clear();
                track.param_values.clear();
            }
            return;
        };
        let params_result = self
            .track_audio
            .get(index)
            .and_then(|state| state.host.as_ref())
            .map(|host| Ok(host.enumerate_params()))
            .unwrap_or_else(|| self.enumerate_plugin_params_for_track(index, &path));
        let Some(track) = self.tracks.get_mut(index) else {
            return;
        };
        match params_result {
            Ok(params) if !params.is_empty() => {
                let next_ids: Vec<u32> = params.iter().map(|p| p.id).collect();
                let reuse_values = track.param_ids == next_ids && track.param_values.len() == params.len();
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
                self.status = format!("Plugin params unavailable: {err}");
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

    fn piano_preview_note_on(&mut self, note: u8, velocity: u8) {
        let freq = 440.0f32 * 2.0f32.powf((note as f32 - 69.0) / 12.0);
        self.midi_freq_bits.store(freq.to_bits(), Ordering::Relaxed);
        self.midi_gate.store(true, Ordering::Relaxed);
        let Some(index) = self.selected_track else {
            return;
        };
        if let Some(state) = self.track_audio.get(index) {
            if let Ok(mut events) = state.midi_events.lock() {
                events.push(vst3::MidiEvent::note_on(0, note, velocity));
            }
        }
    }

    fn piano_preview_note_off(&mut self, note: u8) {
        self.midi_gate.store(false, Ordering::Relaxed);
        let Some(index) = self.selected_track else {
            return;
        };
        if let Some(state) = self.track_audio.get(index) {
            if let Ok(mut events) = state.midi_events.lock() {
                events.push(vst3::MidiEvent::note_off(0, note, 0));
            }
        }
    }

    fn replace_instrument(&mut self, index: usize, path: String, clap_id: Option<String>) {
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
            track.instrument_clap_id = clap_id;
            track.params = default_instrument_params();
            track.param_ids.clear();
            track.param_values.clear();
        }
        if let Some(state) = self.track_audio.get_mut(index) {
            if let Some(host) = state.host.take() {
                host.prepare_for_drop();
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

fn wav_spec_for_depth(
    sample_rate: u32,
    channels: u16,
    bit_depth: RenderWavBitDepth,
) -> hound::WavSpec {
    hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: bit_depth.bits_per_sample(),
        sample_format: bit_depth.sample_format(),
    }
}

fn sample_to_int(sample: f32, bits: u16) -> i32 {
    let max = (1i64 << (bits.saturating_sub(1))) - 1;
    let min = -(1i64 << (bits.saturating_sub(1)));
    let scaled = (sample.clamp(-1.0, 1.0) * max as f32).round() as i64;
    scaled.clamp(min, max) as i32
}

fn write_wav_samples<W: std::io::Write + std::io::Seek>(
    writer: &mut hound::WavWriter<W>,
    bit_depth: RenderWavBitDepth,
    samples: &[f32],
) -> Result<(), String> {
    match bit_depth {
        RenderWavBitDepth::Float32 => {
            for sample in samples {
                writer.write_sample(*sample).map_err(|e| e.to_string())?;
            }
        }
        RenderWavBitDepth::Int16 => {
            for sample in samples {
                let value = sample_to_int(*sample, 16) as i16;
                writer.write_sample(value).map_err(|e| e.to_string())?;
            }
        }
        RenderWavBitDepth::Int24 => {
            for sample in samples {
                let value = sample_to_int(*sample, 24);
                writer.write_sample(value).map_err(|e| e.to_string())?;
            }
        }
        RenderWavBitDepth::Int32 => {
            for sample in samples {
                let value = sample_to_int(*sample, 32);
                writer.write_sample(value).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
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

    let spec = wav_spec_for_depth(plan.sample_rate, channels, plan.wav_bit_depth);
    if let Some(parent) = Path::new(&plan.path).parent() {
        if let Err(err) = fs::create_dir_all(parent) {
            return Err(format!("Render folder create failed: {err}"));
        }
    }
    let file = std::fs::File::create(&plan.path).map_err(|e| e.to_string())?;
    let mut writer = hound::WavWriter::new(file, spec).map_err(|e| e.to_string())?;

    let mut track_hosts: Vec<(RenderTrack, Option<RenderHost>, Vec<RenderHost>)> = Vec::new();
    if !plan.tracks.is_empty() {
        for track in plan.tracks {
            let host = if let Some(path) = track.instrument_path.as_ref() {
                load_render_host(
                    path,
                    track.instrument_clap_id.as_deref(),
                    plan.sample_rate as f64,
                    plan.block_size,
                    channels as usize,
                    false,
                )
            } else {
                None
            };
            let mut fx_hosts = Vec::new();
            for (fx_index, fx_path) in track.effect_paths.iter().enumerate() {
                let fx = load_render_host(
                    fx_path,
                    track.effect_clap_ids.get(fx_index).and_then(|id| id.as_deref()),
                    plan.sample_rate as f64,
                    plan.block_size,
                    channels as usize,
                    true,
                );
                if let Some(fx) = fx {
                    fx_hosts.push(fx);
                }
            }
            track_hosts.push((track, host, fx_hosts));
        }
    } else {
        let host = if let Some(path) = plan.instrument_path.as_ref() {
            load_render_host(
                path,
                None,
                plan.sample_rate as f64,
                plan.block_size,
                channels as usize,
                false,
            )
        } else {
            None
        };
        let single = RenderTrack {
            notes: plan.notes.clone(),
            instrument_path: plan.instrument_path.clone(),
            instrument_clap_id: None,
            param_ids: plan.param_ids.clone(),
            param_values: plan.param_values.clone(),
            plugin_state_component: plan.plugin_state_component.clone(),
            plugin_state_controller: plan.plugin_state_controller.clone(),
            effect_paths: Vec::new(),
            effect_clap_ids: Vec::new(),
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
            let _ = host.apply_state_for_render(
                track.plugin_state_component.as_deref(),
                track.plugin_state_controller.as_deref(),
            );
        }
        if !track.param_ids.is_empty() {
            for (param_id, value) in track.param_ids.iter().zip(track.param_values.iter()) {
                host.push_param_change(*param_id, *value as f64);
            }
        }
    }

    render_send_midi_stop(&mut track_hosts, channels as usize, plan.block_size);
    render_warmup_hosts(&mut track_hosts, channels as usize, plan.block_size, 2);

    let mut master_state = MasterCompState::default();
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
            let mut events = if plan.render_tail_mode == RenderTailMode::Release
                && start_samples > 0
                && block_start == start_samples
            {
                (0u8..=127)
                    .map(|note| vst3::MidiEvent::note_off_at(0, note, 0, 0))
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            events.extend(collect_block_events(
                &track.notes,
                block_start,
                block_end,
                samples_per_beat,
            ));
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
                    let src_pos = if len > 0.0 {
                        if plan.render_tail_mode == RenderTailMode::Wrap {
                            pos % len
                        } else if pos >= len {
                            continue;
                        } else {
                            pos
                        }
                    } else {
                        pos
                    };
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
        apply_master_processing(
            &mut output,
            channels as usize,
            plan.sample_rate as f32,
            &plan.master_settings,
            &mut master_state,
        );
        write_wav_samples(&mut writer, plan.wav_bit_depth, &output)?;
        cursor += frames;
        done.store(cursor as u64, Ordering::Relaxed);
    }

    writer.finalize().map_err(|e| e.to_string())?;
    done.store(total_samples_u64, Ordering::Relaxed);
    Ok(())
}

fn load_render_host(
    path: &str,
    clap_id: Option<&str>,
    sample_rate: f64,
    block_size: usize,
    channels: usize,
    with_input: bool,
) -> Option<RenderHost> {
    match DawApp::plugin_kind_from_path(path) {
        PluginKind::Vst3 => {
            if with_input {
                vst3::Vst3Host::load_with_input(path, sample_rate, block_size, channels, channels)
                    .ok()
                    .map(RenderHost::Vst3)
            } else {
                vst3::Vst3Host::load(path, sample_rate, block_size, channels)
                    .ok()
                    .map(RenderHost::Vst3)
            }
        }
        PluginKind::Clap => {
            let clap_id = clap_id
                .map(|id| id.to_string())
                .or_else(|| clap_host::default_plugin_id(path).ok())?;
            clap_host::ClapHost::load(
                path,
                &clap_id,
                sample_rate,
                block_size as u32,
                channels,
                channels,
            )
            .ok()
            .map(RenderHost::Clap)
        }
    }
}

fn render_send_midi_stop(
    track_hosts: &mut [(RenderTrack, Option<RenderHost>, Vec<RenderHost>)],
    channels: usize,
    block_size: usize,
) {
    if channels == 0 {
        return;
    }
    let frames = block_size.max(1);
    let mut buffer = vec![0.0f32; frames * channels];
    let mut input = vec![0.0f32; frames * channels];
    let mut events = Vec::with_capacity(16 * 128);
    for channel in 0u8..16 {
        events.push(vst3::MidiEvent::control_change(channel, 120, 0));
        events.push(vst3::MidiEvent::control_change(channel, 123, 0));
        for note in 0u8..=127 {
            events.push(vst3::MidiEvent::note_off_at(channel, note, 0, 0));
        }
    }
    for (_, host, fx_hosts) in track_hosts.iter_mut() {
        if let Some(host) = host.as_mut() {
            buffer.fill(0.0);
            let _ = host.process_f32(&mut buffer, channels, &events);
        }
        for fx in fx_hosts.iter_mut() {
            input.fill(0.0);
            buffer.fill(0.0);
            let _ = fx.process_f32_with_input(&input, &mut buffer, channels, &events);
        }
    }
}

fn render_warmup_hosts(
    track_hosts: &mut [(RenderTrack, Option<RenderHost>, Vec<RenderHost>)],
    channels: usize,
    block_size: usize,
    blocks: usize,
) {
    if channels == 0 || block_size == 0 || blocks == 0 {
        return;
    }
    let frames = block_size.max(1);
    let mut buffer = vec![0.0f32; frames * channels];
    let mut input = vec![0.0f32; frames * channels];
    let events: [vst3::MidiEvent; 0] = [];
    for _ in 0..blocks {
        for (_, host, fx_hosts) in track_hosts.iter_mut() {
            if let Some(host) = host.as_mut() {
                buffer.fill(0.0);
                let _ = host.process_f32(&mut buffer, channels, &events);
            }
            for fx in fx_hosts.iter_mut() {
                input.fill(0.0);
                buffer.fill(0.0);
                let _ = fx.process_f32_with_input(&input, &mut buffer, channels, &events);
            }
        }
    }
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

    let mut track_hosts: Vec<(RenderTrack, Option<RenderHost>, Vec<RenderHost>)> = Vec::new();
    if !plan.tracks.is_empty() {
        for track in plan.tracks.iter().cloned() {
            let host = if let Some(path) = track.instrument_path.as_ref() {
                load_render_host(
                    path,
                    track.instrument_clap_id.as_deref(),
                    plan.sample_rate as f64,
                    plan.block_size,
                    channels as usize,
                    false,
                )
            } else {
                None
            };
            let mut fx_hosts = Vec::new();
            for (fx_index, fx_path) in track.effect_paths.iter().enumerate() {
                let fx = load_render_host(
                    fx_path,
                    track.effect_clap_ids.get(fx_index).and_then(|id| id.as_deref()),
                    plan.sample_rate as f64,
                    plan.block_size,
                    channels as usize,
                    true,
                );
                if let Some(fx) = fx {
                    fx_hosts.push(fx);
                }
            }
            track_hosts.push((track, host, fx_hosts));
        }
    } else {
        let host = if let Some(path) = plan.instrument_path.as_ref() {
            load_render_host(
                path,
                None,
                plan.sample_rate as f64,
                plan.block_size,
                channels as usize,
                false,
            )
        } else {
            None
        };
        let single = RenderTrack {
            notes: plan.notes.clone(),
            instrument_path: plan.instrument_path.clone(),
            instrument_clap_id: None,
            param_ids: plan.param_ids.clone(),
            param_values: plan.param_values.clone(),
            plugin_state_component: plan.plugin_state_component.clone(),
            plugin_state_controller: plan.plugin_state_controller.clone(),
            effect_paths: Vec::new(),
            effect_clap_ids: Vec::new(),
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

    render_send_midi_stop(&mut track_hosts, channels as usize, plan.block_size);
    render_warmup_hosts(&mut track_hosts, channels as usize, plan.block_size, 2);

    let mut master_state = MasterCompState::default();
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
            let mut events = if plan.render_tail_mode == RenderTailMode::Release
                && start_samples > 0
                && block_start == start_samples
            {
                (0u8..=127)
                    .map(|note| vst3::MidiEvent::note_off_at(0, note, 0, 0))
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            events.extend(collect_block_events(
                &track.notes,
                block_start,
                block_end,
                samples_per_beat,
            ));
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
                    let src_pos = if len > 0.0 {
                        if plan.render_tail_mode == RenderTailMode::Wrap {
                            pos % len
                        } else if pos >= len {
                            continue;
                        } else {
                            pos
                        }
                    } else {
                        pos
                    };
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
        apply_master_processing(
            &mut output,
            channels as usize,
            plan.sample_rate as f32,
            &plan.master_settings,
            &mut master_state,
        );
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

    let mut track_hosts: Vec<(RenderTrack, Option<RenderHost>, Vec<RenderHost>)> = Vec::new();
    if !plan.tracks.is_empty() {
        for track in plan.tracks {
            let host = if let Some(path) = track.instrument_path.as_ref() {
                load_render_host(
                    path,
                    track.instrument_clap_id.as_deref(),
                    plan.sample_rate as f64,
                    plan.block_size,
                    channels as usize,
                    false,
                )
            } else {
                None
            };
            let mut fx_hosts = Vec::new();
            for (fx_index, fx_path) in track.effect_paths.iter().enumerate() {
                let fx = load_render_host(
                    fx_path,
                    track.effect_clap_ids.get(fx_index).and_then(|id| id.as_deref()),
                    plan.sample_rate as f64,
                    plan.block_size,
                    channels as usize,
                    true,
                );
                if let Some(fx) = fx {
                    fx_hosts.push(fx);
                }
            }
            track_hosts.push((track, host, fx_hosts));
        }
    } else {
        let host = if let Some(path) = plan.instrument_path.as_ref() {
            load_render_host(
                path,
                None,
                plan.sample_rate as f64,
                plan.block_size,
                channels as usize,
                false,
            )
        } else {
            None
        };
        let single = RenderTrack {
            notes: plan.notes.clone(),
            instrument_path: plan.instrument_path.clone(),
            instrument_clap_id: None,
            param_ids: plan.param_ids.clone(),
            param_values: plan.param_values.clone(),
            plugin_state_component: plan.plugin_state_component.clone(),
            plugin_state_controller: plan.plugin_state_controller.clone(),
            effect_paths: Vec::new(),
            effect_clap_ids: Vec::new(),
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

    render_send_midi_stop(&mut track_hosts, channels as usize, plan.block_size);
    render_warmup_hosts(&mut track_hosts, channels as usize, plan.block_size, 2);

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
                    let src_pos = if len > 0.0 {
                        if plan.render_tail_mode == RenderTailMode::Wrap {
                            pos % len
                        } else if pos >= len {
                            continue;
                        } else {
                            pos
                        }
                    } else {
                        pos
                    };
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
    playback_panic: &AtomicBool,
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
    let panic_notes = playback_panic.swap(false, Ordering::Relaxed);
    let mut loop_wrapped = false;
    if loop_end > loop_start && block_start < loop_end && block_end > loop_end {
        block_start = loop_start;
        block_end = block_start + frames as u64;
        transport_samples.store(block_end, Ordering::Relaxed);
        loop_wrapped = true;
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

    // Process tracks on the calling thread to avoid VST3 thread-affinity issues.
    per_track_buffers
        .iter_mut()
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
            let mut remaining_params: Vec<PendingParamChange> = Vec::new();
            let mut filtered_events: Vec<vst3::MidiEvent> = Vec::new();
            if let Some(host) = state.host.as_ref() {
                let mut events =
                    collect_block_events(&notes, block_start, block_end, samples_per_beat);
                if panic_notes {
                    for channel in 0u8..16 {
                        events.push(vst3::MidiEvent::control_change(channel, 120, 0));
                        events.push(vst3::MidiEvent::control_change(channel, 123, 0));
                    }
                    for channel in 0u8..16 {
                        events.extend(
                            (0u8..=127)
                                .map(|note| vst3::MidiEvent::note_off_at(channel, note, 0, 0)),
                        );
                    }
                    if frames > 1 {
                        for event in events.iter_mut() {
                            if let vst3::MidiEvent::NoteOn { sample_offset, .. } = event {
                                if *sample_offset == 0 {
                                    *sample_offset = 1;
                                }
                            }
                        }
                    }
                }
                if loop_wrapped {
                    events.extend((0u8..=127).map(|note| vst3::MidiEvent::note_off(0, note, 0)));
                }
                if let Ok(mut queued) = state.midi_events.lock() {
                    events.extend(queued.drain(..));
                }
                let has_note_on = events
                    .iter()
                    .any(|event| matches!(event, vst3::MidiEvent::NoteOn { .. }));
                let pending_params = state
                    .pending_param_changes
                    .lock()
                    .ok()
                    .map(|mut pending| pending.drain(..).collect::<Vec<_>>())
                    .unwrap_or_default();
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
                    if let Some(value) = DawApp::automation_value_at(&lane.points, block_beat) {
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
                            if controller >= 120 {
                                filtered.push(event);
                                continue;
                            }
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
                filtered_events = filtered;
                if host.process_f32(temp, channels, &filtered_events).is_ok() {
                    track_processed = true;
                }
                if panic_notes && !has_note_on {
                    temp.fill(0.0);
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

            if !state.effect_hosts.is_empty() {
                let temp_len = temp.len();
                let mut scratch: Vec<f32> = vec![0.0; temp_len];
                let mut use_temp = true;
                let mut current: &mut [f32] = temp;
                let mut scratch_slice: &mut [f32] = &mut scratch;
                let skip_fx = smart_disable_plugins && !has_notes && !has_audio;
                for (fx_index, fx_host) in state.effect_hosts.iter().enumerate() {
                    if skip_fx || bypass.get(fx_index).copied().unwrap_or(false) {
                        continue;
                    }
                    scratch_slice.fill(0.0);
                    let mut still_pending: Vec<PendingParamChange> = Vec::new();
                    for pending in remaining_params.drain(..) {
                        match pending.target {
                            PendingParamTarget::Effect(target_index) if target_index == fx_index => {
                                fx_host.push_param_change(pending.param_id, pending.value);
                            }
                            _ => still_pending.push(pending),
                        }
                    }
                    remaining_params = still_pending;
                    for lane in &automation {
                        if let Some(value) = DawApp::automation_value_at(&lane.points, block_beat) {
                            if lane.target == AutomationTarget::Effect(fx_index) {
                                fx_host.push_param_change(lane.param_id, value as f64);
                            }
                        }
                    }
                    if fx_host
                        .process_f32_with_input(current, scratch_slice, channels, &filtered_events)
                        .is_ok()
                    {
                        std::mem::swap(&mut current, &mut scratch_slice);
                        use_temp = !use_temp;
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

fn apply_fade_in_if_needed(samples: &mut [f32], channels: usize, flag: &AtomicBool) {
    if !flag.swap(false, Ordering::Relaxed) {
        return;
    }
    let frames = samples.len() / channels.max(1);
    if frames == 0 {
        return;
    }
    for frame in 0..frames {
        let gain = (frame as f32 + 1.0) / frames as f32;
        let base = frame * channels;
        for ch in 0..channels {
            if let Some(sample) = samples.get_mut(base + ch) {
                *sample *= gain;
            }
        }
    }
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

fn db_to_gain(db: f32) -> f32 {
    10.0f32.powf(db / 20.0)
}

fn apply_master_processing(
    samples: &mut [f32],
    channels: usize,
    sample_rate: f32,
    settings: &MasterCompSettings,
    state: &mut MasterCompState,
) {
    if samples.is_empty() {
        return;
    }
    let mut gain = settings.level.clamp(0.0, 2.0);
    if settings.enabled {
        let threshold = db_to_gain(settings.threshold_db);
        let ratio = settings.ratio.max(1.0);
        let attack = (settings.attack_ms.max(0.1) / 1000.0).max(0.0001);
        let release = (settings.release_ms.max(0.1) / 1000.0).max(0.0001);
        let attack_coeff = (-1.0 / (attack * sample_rate.max(1.0))).exp();
        let release_coeff = (-1.0 / (release * sample_rate.max(1.0))).exp();
        let makeup = db_to_gain(settings.makeup_db);
        gain *= makeup;

        for frame in samples.chunks_mut(channels.max(1)) {
            let mut level = 0.0f32;
            for sample in frame.iter() {
                level = level.max(sample.abs());
            }
            let target_gain = if level > threshold {
                let over = (level / threshold).max(1.0);
                let compressed = over.powf(1.0 / ratio);
                (compressed / over).clamp(0.0, 1.0)
            } else {
                1.0
            };
            if target_gain < state.gain {
                state.gain = attack_coeff * (state.gain - target_gain) + target_gain;
            } else {
                state.gain = release_coeff * (state.gain - target_gain) + target_gain;
            }
            let frame_gain = state.gain * gain;
            for sample in frame.iter_mut() {
                *sample *= frame_gain;
            }
        }
    } else if gain != 1.0 {
        for sample in samples.iter_mut() {
            *sample *= gain;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RenderFormat {
    Wav,
    Ogg,
    Flac,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RenderWavBitDepth {
    Int16,
    Int24,
    Int32,
    Float32,
}

impl RenderWavBitDepth {
    fn all() -> [Self; 4] {
        [Self::Int16, Self::Int24, Self::Int32, Self::Float32]
    }

    fn label(self) -> &'static str {
        match self {
            Self::Int16 => "16-bit",
            Self::Int24 => "24-bit",
            Self::Int32 => "32-bit int",
            Self::Float32 => "32f",
        }
    }

    fn bits_per_sample(self) -> u16 {
        match self {
            Self::Int16 => 16,
            Self::Int24 => 24,
            Self::Int32 => 32,
            Self::Float32 => 32,
        }
    }

    fn sample_format(self) -> hound::SampleFormat {
        match self {
            Self::Float32 => hound::SampleFormat::Float,
            _ => hound::SampleFormat::Int,
        }
    }
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
