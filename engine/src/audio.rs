use crate::error::Result as LingResult;
use crate::hosts::vst3::{self};
use crate::models::*;
use crate::node_editor::TrackNodeActivity;
use crate::performance::PerformanceRuntimeClip;
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

pub const MAX_PLUGIN_OUTPUT_CHANNELS: usize = 32;
pub static PLUGIN_PROCESS_FAILURES: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone)]
pub struct AudioClipData {
    pub samples: Vec<f32>,
    pub channels: usize,
    pub sample_rate: u32,
}

pub struct AudioClipCache {
    pub entries: HashMap<Arc<str>, Arc<AudioClipData>>,
    pub order: VecDeque<Arc<str>>,
    pub bytes: usize,
    pub max_bytes: usize,
}

impl Default for AudioClipCache {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioClipCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            max_bytes: 1024 * 1024 * 1024, // 1GB default
        }
    }

    pub fn get(&mut self, path: &str) -> Option<Arc<AudioClipData>> {
        if let Some(data) = self.entries.get(path) {
            return Some(data.clone());
        }
        None
    }

    pub fn insert(&mut self, path: Arc<str>, data: Arc<AudioClipData>) {
        self.entries.insert(path, data);
    }

    pub fn remove(&mut self, path: &str) {
        self.entries.remove(path);
        self.order.retain(|k| k.as_ref() != path);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.bytes = 0;
    }
}

pub struct AudioEngine {
    pub sample_rate: f32,
    pub block_size: usize,
    pub track_audio: Vec<TrackAudioState>,
    pub track_mix: Arc<Mutex<Vec<TrackMixState>>>,
    pub node_activity: Arc<Mutex<Vec<TrackNodeActivity>>>,
    pub node_routes: Arc<Mutex<Vec<NodeRouteLink>>>,
    pub performance_runtime: Arc<Mutex<Vec<Option<PerformanceRuntimeClip>>>>,
    pub audio_clips: Arc<Mutex<Vec<AudioClipRender>>>,
    pub audio_cache: Arc<Mutex<AudioClipCache>>,
    pub tempo_bpm: Arc<AtomicU32>,
    pub transport_samples: Arc<AtomicU64>,
    pub loop_start_samples: Arc<AtomicU64>,
    pub loop_end_samples: Arc<AtomicU64>,
    pub playback_panic: Arc<AtomicBool>,
    pub arrangement_playback_enabled: Arc<AtomicBool>,
    pub master_comp: Arc<Mutex<MasterCompSettings>>,
    pub smart_suspend: Arc<AtomicBool>,
    pub stats: Arc<AudioStats>,
    pub master_peak_bits: Arc<AtomicU32>,
    pub selected_track_index: Arc<AtomicUsize>,
    pub midi_freq_bits: Arc<AtomicU32>,
    pub midi_gate: Arc<AtomicBool>,
    pub playback_fade_in: Arc<AtomicBool>,
    pub master_comp_state: Arc<Mutex<MasterCompState>>,
    pub audio_callback_active: Arc<AtomicUsize>,
    pub recording: Arc<Mutex<RecordingBuffers>>,
}

impl AudioEngine {
    pub fn new(sample_rate: f32, block_size: usize) -> Self {
        Self {
            sample_rate,
            block_size,
            track_audio: Vec::new(),
            track_mix: Arc::new(Mutex::new(Vec::new())),
            node_activity: Arc::new(Mutex::new(Vec::new())),
            node_routes: Arc::new(Mutex::new(Vec::new())),
            performance_runtime: Arc::new(Mutex::new(Vec::new())),
            audio_clips: Arc::new(Mutex::new(Vec::new())),
            audio_cache: Arc::new(Mutex::new(AudioClipCache::new())),
            tempo_bpm: Arc::new(AtomicU32::new(120.0f32.to_bits())),
            transport_samples: Arc::new(AtomicU64::new(0)),
            loop_start_samples: Arc::new(AtomicU64::new(0)),
            loop_end_samples: Arc::new(AtomicU64::new(0)),
            playback_panic: Arc::new(AtomicBool::new(false)),
            arrangement_playback_enabled: Arc::new(AtomicBool::new(true)),
            master_comp: Arc::new(Mutex::new(MasterCompSettings::default())),
            smart_suspend: Arc::new(AtomicBool::new(true)),
            stats: Arc::new(AudioStats::new()),
            master_peak_bits: Arc::new(AtomicU32::new(0)),
            selected_track_index: Arc::new(AtomicUsize::new(0)),
            midi_freq_bits: Arc::new(AtomicU32::new(0)),
            midi_gate: Arc::new(AtomicBool::new(false)),
            playback_fade_in: Arc::new(AtomicBool::new(false)),
            master_comp_state: Arc::new(Mutex::new(MasterCompState::default())),
            audio_callback_active: Arc::new(AtomicUsize::new(0)),
            recording: Arc::new(Mutex::new(RecordingBuffers::new())),
        }
    }

    pub fn master_comp_snapshot(&self) -> MasterCompSettings {
        self.master_comp.lock().clone()
    }
}

pub struct AudioStats {
    pub blocks: AtomicU64,
    pub overruns: AtomicU64,
    pub last_block_ms_bits: AtomicU32,
    pub max_block_ms_bits: AtomicU32,
}

impl Default for AudioStats {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioStats {
    pub fn new() -> Self {
        Self {
            blocks: AtomicU64::new(0),
            overruns: AtomicU64::new(0),
            last_block_ms_bits: AtomicU32::new(0.0f32.to_bits()),
            max_block_ms_bits: AtomicU32::new(0.0f32.to_bits()),
        }
    }

    pub fn snapshot(&self) -> (u64, u64, f32, f32) {
        (
            self.blocks.load(Ordering::Relaxed),
            self.overruns.load(Ordering::Relaxed),
            f32::from_bits(self.last_block_ms_bits.load(Ordering::Relaxed)),
            f32::from_bits(self.max_block_ms_bits.load(Ordering::Relaxed)),
        )
    }

    pub fn record_block(&self, last_ms: f32, overrun: bool) {
        self.blocks.fetch_add(1, Ordering::Relaxed);
        if overrun {
            self.overruns.fetch_add(1, Ordering::Relaxed);
        }
        self.last_block_ms_bits
            .store(last_ms.to_bits(), Ordering::Relaxed);
        let mut max_bits = self.max_block_ms_bits.load(Ordering::Relaxed);
        loop {
            let cur = f32::from_bits(max_bits);
            if last_ms <= cur {
                break;
            }
            match self.max_block_ms_bits.compare_exchange_weak(
                max_bits,
                last_ms.to_bits(),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(v) => max_bits = v,
            }
        }
    }
}

#[derive(Clone, Copy)]
pub struct TreeSynthVoice {
    pub sample_index: usize,
    pub sample_pos: f64,
    pub sample_end: f64,
    pub step: f64,
    pub note: u8,
    pub start_sample: u64,
    pub release_sample: Option<u64>,
    pub release_level: f32,
    pub gain: f32,
    pub pan: f32,
    pub rate: f64,
    pub rate_step: f64,
    pub glide_remaining: u64,
}

#[derive(Clone)]
pub struct TreeSynthRuntime {
    pub voices: Vec<TreeSynthVoice>,
    pub sequence_index: usize,
    pub rng_state: u64,
    pub last_note: Option<u8>,
}

#[derive(Clone, Copy, Default)]
pub struct DrumMachineFilterState {
    pub lp: f32,
    pub bp: f32,
}

#[derive(Clone)]
pub struct DrumMachineVoice {
    pub pad_index: usize,
    pub sample_pos: f64,
    pub sample_end: f64,
    pub step: f64,
    pub start_sample: u64,
    pub gain: f32,
    pub pan: f32,
    pub cutoff: f32,
    pub resonance: f32,
    pub output_pair: usize,
    pub note: u8,
    pub filter: DrumMachineFilterState,
}

#[derive(Clone)]
pub struct DrumMachineRuntime {
    pub voices: Vec<DrumMachineVoice>,
}

impl Default for DrumMachineRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl DrumMachineRuntime {
    pub fn new() -> Self {
        Self {
            voices: Vec::with_capacity(64),
        }
    }
}

impl Default for TreeSynthRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl TreeSynthRuntime {
    pub fn new() -> Self {
        Self {
            voices: Vec::with_capacity(32),
            sequence_index: 0,
            rng_state: 0x9E3779B97F4A7C15,
            last_note: None,
        }
    }

    pub fn next_rand(&mut self) -> u32 {
        self.rng_state = self
            .rng_state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1);
        (self.rng_state >> 32) as u32
    }
}

#[derive(Clone)]
pub struct TrackAudioState {
    pub host: Option<PluginHostHandle>,
    pub effect_hosts: Vec<PluginHostHandle>,
    pub clip_notes: Arc<Mutex<Vec<PianoRollNote>>>,
    pub treesynth_enabled: Arc<AtomicBool>,
    pub treesynth_state: Option<Arc<Mutex<TreeSynthState>>>,
    pub treesynth_runtime: Arc<Mutex<TreeSynthRuntime>>,
    pub drum_machine_state: Option<Arc<Mutex<DrumMachineState>>>,
    pub drum_machine_runtime: Arc<Mutex<DrumMachineRuntime>>,
    pub drum_machine_enabled: Arc<AtomicBool>,
    pub automation_lanes: Arc<Mutex<Vec<AutomationLane>>>,
    pub effect_bypass: Arc<Mutex<Vec<bool>>>,
    pub midi_events: Arc<Mutex<Vec<vst3::MidiEvent>>>,
    pub pending_param_changes: Arc<Mutex<Vec<PendingParamChange>>>,
    pub learned_cc: Arc<Mutex<HashMap<(u8, u8), u32>>>,
    pub peak_bits: Arc<AtomicU32>,
    pub peak_l_bits: Arc<AtomicU32>,
    pub peak_r_bits: Arc<AtomicU32>,
    pub midi_in_peak: Arc<AtomicU32>,
    pub midi_out_peak: Arc<AtomicU32>,
    pub fx_in_peaks: Arc<Mutex<Vec<f32>>>,
    pub fx_out_peaks: Arc<Mutex<Vec<f32>>>,
    pub track_buffer: Arc<Mutex<Vec<f32>>>,
    pub output_pair_mix: Arc<Mutex<Vec<TrackMixState>>>,
    pub output_pair_buffers: Arc<Mutex<Vec<Vec<f32>>>>,
    pub output_pair_override: Arc<AtomicI32>,
    pub native_output_channels: Arc<AtomicU32>,
    pub silent_blocks: Arc<AtomicU64>,
}

impl PluginHostHandle {
    pub fn prepare_for_drop(&mut self) {
        match self {
            PluginHostHandle::Vst3(h) => {
                if let Some(mut g) = h.try_lock() {
                    g.prepare_for_drop();
                }
            }
            PluginHostHandle::Clap(h) => {
                if let Some(mut g) = h.try_lock() {
                    g.prepare_for_drop();
                }
            }
        }
    }

    pub fn push_param_change(&self, param_id: u32, value: f64) {
        match self {
            PluginHostHandle::Vst3(h) => {
                if let Some(mut h) = h.try_lock() {
                    h.push_param_change(param_id, value);
                }
            }
            PluginHostHandle::Clap(h) => {
                if let Some(mut h) = h.try_lock() {
                    h.push_param_change(param_id, value);
                }
            }
        }
    }

    pub fn io_channels(&self) -> (usize, usize) {
        match self {
            PluginHostHandle::Vst3(h) => h.lock().io_channels(),
            PluginHostHandle::Clap(h) => h.lock().io_channels(),
        }
    }

    pub fn process_f32(
        &self,
        output: &mut [f32],
        channels: usize,
        midi: &[vst3::MidiEvent],
    ) -> LingResult<()> {
        match self {
            PluginHostHandle::Vst3(h) => h.lock().process_f32(output, channels, midi),
            PluginHostHandle::Clap(h) => h.lock().process_f32(output, channels, midi),
        }
    }

    pub fn process_f32_with_input(
        &self,
        input: &[f32],
        output: &mut [f32],
        channels: usize,
        midi: &[vst3::MidiEvent],
    ) -> LingResult<()> {
        match self {
            PluginHostHandle::Vst3(h) => h
                .lock()
                .process_f32_with_input(input, output, channels, midi),
            PluginHostHandle::Clap(h) => h
                .lock()
                .process_f32_with_input(input, output, channels, midi),
        }
    }

    pub fn enumerate_params(&self) -> Vec<vst3::ParamInfo> {
        match self {
            PluginHostHandle::Vst3(h) => h.lock().enumerate_params(),
            PluginHostHandle::Clap(h) => h
                .lock()
                .enumerate_params()
                .into_iter()
                .map(|p| vst3::ParamInfo {
                    id: p.id,
                    name: p.name,
                    default_value: p.default_value,
                })
                .collect(),
        }
    }

    pub fn clap_blocks_params(&self) -> bool {
        match self {
            PluginHostHandle::Vst3(_) => false,
            PluginHostHandle::Clap(h) => h.lock().param_changes_blocked(),
        }
    }

    pub fn get_state_bytes(&self) -> (Vec<u8>, Vec<u8>) {
        match self {
            PluginHostHandle::Vst3(h) => h.lock().get_state_bytes(),
            PluginHostHandle::Clap(h) => {
                let mut g = h.lock();
                let b = g.get_state_bytes();
                (b, Vec::new())
            }
        }
    }

    pub fn set_state_bytes(
        &mut self,
        component: Option<&[u8]>,
        controller: Option<&[u8]>,
    ) -> LingResult<()> {
        match self {
            PluginHostHandle::Vst3(h) => h.lock().set_state_bytes(component, controller),
            PluginHostHandle::Clap(h) => {
                let mut g = h.lock();
                if let Some(bytes) = component.or(controller) {
                    if !bytes.is_empty() {
                        g.set_state_bytes(bytes)?;
                    }
                }
                Ok(())
            }
        }
    }

    pub fn get_param_normalized(&self, param_id: u32) -> Option<f64> {
        match self {
            PluginHostHandle::Vst3(h) => h.lock().get_param_normalized(param_id),
            PluginHostHandle::Clap(h) => h.lock().get_param_normalized(param_id),
        }
    }
}

impl TrackAudioState {
    pub fn from_track(track: &Track) -> Self {
        let drum_state = track.drum_machine.clone().unwrap_or_default();
        let native_output_channels = if track.drum_machine.is_some() {
            (DRUM_MACHINE_OUTPUT_PAIRS * 2) as u32
        } else {
            2
        };
        Self {
            host: None,
            effect_hosts: Vec::new(),
            clip_notes: Arc::new(Mutex::new(Vec::new())),
            treesynth_enabled: Arc::new(AtomicBool::new(false)),
            treesynth_state: track
                .treesynth
                .as_ref()
                .map(|t| Arc::new(Mutex::new(t.clone()))),
            treesynth_runtime: Arc::new(Mutex::new(TreeSynthRuntime::new())),
            drum_machine_state: Some(Arc::new(Mutex::new(drum_state))),
            drum_machine_runtime: Arc::new(Mutex::new(DrumMachineRuntime::new())),
            drum_machine_enabled: Arc::new(AtomicBool::new(track.drum_machine.is_some())),
            automation_lanes: Arc::new(Mutex::new(track.automation_lanes.clone())),
            effect_bypass: Arc::new(Mutex::new(track.effect_bypass.clone())),
            midi_events: Arc::new(Mutex::new(Vec::new())),
            pending_param_changes: Arc::new(Mutex::new(Vec::new())),
            learned_cc: Arc::new(Mutex::new(HashMap::new())),
            peak_bits: Arc::new(AtomicU32::new(0)),
            peak_l_bits: Arc::new(AtomicU32::new(0)),
            peak_r_bits: Arc::new(AtomicU32::new(0)),
            midi_in_peak: Arc::new(AtomicU32::new(0)),
            midi_out_peak: Arc::new(AtomicU32::new(0)),
            fx_in_peaks: Arc::new(Mutex::new(Vec::new())),
            fx_out_peaks: Arc::new(Mutex::new(Vec::new())),
            track_buffer: Arc::new(Mutex::new(Vec::new())),
            output_pair_mix: Arc::new(Mutex::new(track.output_pair_mix.clone())),
            output_pair_buffers: Arc::new(Mutex::new(Vec::new())),
            output_pair_override: Arc::new(AtomicI32::new(-1)),
            native_output_channels: Arc::new(AtomicU32::new(native_output_channels)),
            silent_blocks: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn sync_notes(&self, track: &Track) {
        *self.clip_notes.lock() = track.midi_notes.clone();
    }

    pub fn sync_automation(&self, track: &Track) {
        *self.automation_lanes.lock() = track.automation_lanes.clone();
    }

    pub fn sync_effect_bypass(&self, track: &Track) {
        *self.effect_bypass.lock() = track.effect_bypass.clone();
    }

    pub fn sync_output_pair_mix(&self, track: &Track) {
        *self.output_pair_mix.lock() = track.output_pair_mix.clone();
    }

    pub fn sync_treesynth(
        &mut self,
        track: &Track,
        enabled: bool,
        _audio_cache: &Arc<Mutex<AudioClipCache>>,
    ) {
        self.treesynth_enabled.store(enabled, Ordering::Relaxed);
        if enabled {
            self.treesynth_state = track
                .treesynth
                .as_ref()
                .map(|t| Arc::new(Mutex::new(t.clone())));
        } else {
            self.treesynth_state = None;
        }
    }

    pub fn sync_drum_machine(&mut self, track: &Track, enabled: bool) {
        if enabled {
            let next_state = track.drum_machine.clone().unwrap_or_default();
            if let Some(state) = self.drum_machine_state.as_ref() {
                *state.lock() = next_state;
            } else {
                self.drum_machine_state = Some(Arc::new(Mutex::new(next_state)));
            }
        }
        self.drum_machine_enabled.store(enabled, Ordering::Relaxed);
        let channels = if enabled {
            (DRUM_MACHINE_OUTPUT_PAIRS * 2) as u32
        } else {
            2
        };
        self.native_output_channels
            .store(channels, Ordering::Relaxed);
        if !enabled {
            self.drum_machine_state = None;
            self.drum_machine_runtime.lock().voices.clear();
        }
    }
}

pub struct AudioRuntimeBuffers {
    pub routes_snapshot: Vec<NodeRouteLink>,
    pub sidechain_states: Vec<f32>,
}

impl AudioRuntimeBuffers {
    pub fn new(_num_tracks: usize, _block_size: usize) -> Self {
        Self {
            routes_snapshot: Vec::new(),
            sidechain_states: vec![1.0; 128], // Default space for sidechain states
        }
    }
}
