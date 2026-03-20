use super::*;

#[derive(Clone)]
pub(crate) struct TreeSynthVoice {
    pub(crate) sample_index:  usize,
    pub(crate) sample_pos:  f64,
    pub(crate) sample_end:  f64,
    pub(crate) step:  f64,
    pub(crate) note:  u8,
    pub(crate) start_sample:  u64,
    pub(crate) release_sample:  Option<u64>,
    pub(crate) release_level:  f32,
    pub(crate) gain:  f32,
    pub(crate) pan:  f32,
    pub(crate) rate:  f64,
    pub(crate) rate_step:  f64,
    pub(crate) glide_remaining:  u64,
}

#[derive(Clone)]
pub(crate) struct TreeSynthRuntime {
    pub(crate) voices:  Vec<TreeSynthVoice>,
    pub(crate) sequence_index:  usize,
    pub(crate) rng_state:  u64,
    pub(crate) last_note:  Option<u8>,
}

impl TreeSynthRuntime {
    pub(crate) fn new() -> Self {
        Self {
            voices: Vec::with_capacity(32),
            sequence_index: 0,
            rng_state: 0x9E3779B97F4A7C15,
            last_note: None,
        }
    }

    pub(crate) fn next_rand(&mut self) -> u32 {
        self.rng_state = self
            .rng_state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1);
        (self.rng_state >> 32) as u32
    }
}

#[derive(Clone)]
pub(crate) struct TrackAudioState {
    pub(crate) host:  Option<PluginHostHandle>,
    pub(crate) effect_hosts:  Vec<PluginHostHandle>,
    pub(crate) effect_bypass:  Arc<Mutex<Vec<bool>>>,
    pub(crate) midi_events:  Arc<Mutex<Vec<vst3::MidiEvent>>>,
    pub(crate) clip_notes:  Arc<Mutex<Vec<PianoRollNote>>>,
    pub(crate) learned_cc:  Arc<Mutex<std::collections::HashMap<(u8, u8), u32>>>,
    pub(crate) peak_bits:  Arc<AtomicU32>,
    pub(crate) peak_l_bits:  Arc<AtomicU32>,
    pub(crate) peak_r_bits:  Arc<AtomicU32>,
    pub(crate) automation_lanes:  Arc<Mutex<Vec<AutomationLane>>>,
    pub(crate) pending_param_changes:  Arc<Mutex<Vec<PendingParamChange>>>,
    pub(crate) silent_blocks:  Arc<AtomicU32>,
    pub(crate) treesynth_state:  Option<Arc<Mutex<TreeSynthState>>>,
    pub(crate) treesynth_runtime:  Arc<Mutex<TreeSynthRuntime>>,
    pub(crate) treesynth_enabled:  Arc<AtomicBool>,
    pub(crate) track_buffer:  Arc<Mutex<Vec<f32>>>,
    pub(crate) fx_buffer:  Arc<Mutex<Vec<f32>>>,
    pub(crate) midi_in_peak:  Arc<AtomicU32>,
    pub(crate) midi_out_peak:  Arc<AtomicU32>,
    pub(crate) fx_in_peaks:  Arc<Mutex<Vec<f32>>>,
    pub(crate) fx_out_peaks:  Arc<Mutex<Vec<f32>>>,
}

#[derive(Clone)]
pub(crate) enum PluginHostHandle {
    Vst3(Arc<Mutex<vst3::Vst3Host>>),
    Clap(Arc<Mutex<clap_host::ClapHost>>),
}

impl PluginHostHandle {
    pub(crate) fn enumerate_params(&self) -> Vec<vst3::ParamInfo> {
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

    pub(crate) fn push_param_change(&self, param_id: u32, value: f64) {
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

    pub(crate) fn get_param_normalized(&self, param_id: u32) -> Option<f64> {
        match self {
            PluginHostHandle::Vst3(host) => host.lock().ok().and_then(|host| host.get_param_normalized(param_id)),
            PluginHostHandle::Clap(_) => None,
        }
    }

    pub(crate) fn get_state_bytes(&self) -> (Vec<u8>, Vec<u8>) {
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

    pub(crate) fn clap_blocks_params(&self) -> bool {
        match self {
            PluginHostHandle::Clap(host) => host
                .lock()
                .ok()
                .map(|host| host.param_changes_blocked())
                .unwrap_or(false),
            _ => false,
        }
    }

    pub(crate) fn set_state_bytes(
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

    pub(crate) fn prepare_for_drop(&self) {
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

    pub(crate) fn io_channels(&self) -> (usize, usize) {
        match self {
            PluginHostHandle::Vst3(host) => host
                .lock()
                .ok()
                .map(|host| host.io_channels())
                .unwrap_or((0, 0)),
            PluginHostHandle::Clap(host) => host
                .lock()
                .ok()
                .map(|host| host.io_channels())
                .unwrap_or((0, 0)),
        }
    }

    pub(crate) fn process_f32(
        &self,
        output: &mut [f32],
        channels: usize,
        midi_events: &[vst3::MidiEvent],
    ) -> Result<(), String> {
        match self {
            PluginHostHandle::Vst3(host) => host
                .try_lock()
                .map_err(|_| "Plugin lock failed".to_string())?
                .process_f32(output, channels, midi_events),
            PluginHostHandle::Clap(host) => host
                .try_lock()
                .map_err(|_| "Plugin lock failed".to_string())?
                .process_f32(output, channels, midi_events),
        }
    }

    pub(crate) fn process_f32_with_input(
        &self,
        input: &[f32],
        output: &mut [f32],
        channels: usize,
        midi_events: &[vst3::MidiEvent],
    ) -> Result<(), String> {
        match self {
            PluginHostHandle::Vst3(host) => host
                .try_lock()
                .map_err(|_| "Plugin lock failed".to_string())?
                .process_f32_with_input(input, output, channels, midi_events),
            PluginHostHandle::Clap(host) => host
                .try_lock()
                .map_err(|_| "Plugin lock failed".to_string())?
                .process_f32_with_input(input, output, channels, midi_events),
        }
    }
}

impl TrackAudioState {
    pub(crate) fn from_track(track: &Track) -> Self {
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
            treesynth_state: Some(Arc::new(Mutex::new(
                match &track.treesynth {
                    Some(ts) if !ts.samples.is_empty() => ts.clone(),
                    _ => TreeSynthState::default(),
                }
            ))),
            treesynth_runtime: Arc::new(Mutex::new(TreeSynthRuntime::new())),
            treesynth_enabled: Arc::new(AtomicBool::new(track.treesynth.is_some())),
            track_buffer: Arc::new(Mutex::new(Vec::with_capacity(8192))),
            fx_buffer: Arc::new(Mutex::new(Vec::with_capacity(8192))),
            midi_in_peak: Arc::new(AtomicU32::new(0.0f32.to_bits())),
            midi_out_peak: Arc::new(AtomicU32::new(0.0f32.to_bits())),
            fx_in_peaks: Arc::new(Mutex::new(Vec::with_capacity(16))),
            fx_out_peaks: Arc::new(Mutex::new(Vec::with_capacity(16))),
        }
    }

    pub(crate) fn sync_notes(&self, track: &Track) {
        if let Ok(mut notes) = self.clip_notes.lock() {
            *notes = track.midi_notes.clone();
        }
    }

    pub(crate) fn sync_automation(&self, track: &Track) {
        if let Ok(mut lanes) = self.automation_lanes.lock() {
            *lanes = track.automation_lanes.clone();
        }
    }

    pub(crate) fn sync_effect_bypass(&self, track: &Track) {
        if let Ok(mut bypass) = self.effect_bypass.lock() {
            *bypass = track.effect_bypass.clone();
        }
    }

    pub(crate) fn sync_treesynth(&self, track: &Track, enabled: bool, audio_cache: &Arc<Mutex<AudioClipCache>>) {
        if let (Some(state), Some(treesynth_arc)) = (track.treesynth.clone(), self.treesynth_state.as_ref()) {
            if !state.samples.is_empty() {
                if let Ok(mut treesynth) = treesynth_arc.lock() {
                    *treesynth = state.clone();
                }
                // Pre-load samples into cache from UI thread
                if let Ok(mut cache) = audio_cache.lock() {
                    for sample in &state.samples {
                        if cache.get(&sample.path).is_none() {
                            if let Some(data) = DawApp::load_audio_clip_data(Path::new(&sample.path)).map(Arc::new) {
                                cache.insert(sample.path.clone(), data);
                            }
                        }
                    }
                }
            }
        }
        self.treesynth_enabled.store(enabled, Ordering::Relaxed);
        if !enabled {
            if let Ok(mut runtime) = self.treesynth_runtime.lock() {
                runtime.voices.clear();
                runtime.sequence_index = 0;
            }
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct TrackMixState {
    pub(crate) muted:  bool,
    pub(crate) solo:  bool,
    pub(crate) level:  f32,
}

pub(crate) struct AudioClipData {
    pub(crate) samples:  Vec<f32>,
    pub(crate) channels:  usize,
    pub(crate) sample_rate:  u32,
}

pub(crate) struct AudioAnalysis {
    pub(crate) bpm:  Option<f32>,
    pub(crate) key:  Option<(u8, bool)>,
    pub(crate) fine_pitch_cents:  Option<f32>,
}

pub(crate) struct AudioAnalysisRequest {
    pub(crate) clip_id:  usize,
    pub(crate) path:  PathBuf,
}

pub(crate) struct AudioAnalysisResult {
    pub(crate) clip_id:  usize,
    pub(crate) path:  PathBuf,
    pub(crate) analysis:  Option<AudioAnalysis>,
}

#[cfg(all(windows, has_rubberband))]
#[allow(non_upper_case_globals)]
mod rubberband {
    use std::os::raw::{c_double, c_float, c_int, c_uint};

    #[repr(C)]
    pub struct RubberBandState_ {
        _private: [u8; 0],
    }

    pub type RubberBandState = *mut RubberBandState_;
    pub type RubberBandLiveState = *mut std::ffi::c_void;
    pub type RubberBandOptions = c_int;

    pub const RubberBandOptionProcessRealTime: RubberBandOptions = 0x00000001;
    pub const RubberBandOptionFormantShifted: RubberBandOptions = 0x00000000;
    pub const RubberBandOptionFormantPreserved: RubberBandOptions = 0x01000000;
    pub const RubberBandOptionPitchHighQuality: RubberBandOptions = 0x02000000;

    #[link(name = "rubberband-library", kind = "static")]
    extern "C" {
        pub fn rubberband_new(
            sample_rate: c_uint,
            channels: c_uint,
            options: RubberBandOptions,
            initial_time_ratio: c_double,
            initial_pitch_scale: c_double,
        ) -> RubberBandState;
        pub fn rubberband_delete(state: RubberBandState);
        pub fn rubberband_reset(state: RubberBandState);
        pub fn rubberband_set_time_ratio(state: RubberBandState, ratio: c_double);
        pub fn rubberband_set_pitch_scale(state: RubberBandState, scale: c_double);
        pub fn rubberband_set_formant_option(state: RubberBandState, options: RubberBandOptions);
        pub fn rubberband_set_formant_scale(state: RubberBandState, scale: c_double);
        pub fn rubberband_get_start_delay(state: RubberBandState) -> c_uint;
        pub fn rubberband_get_samples_required(state: RubberBandState) -> c_uint;
        pub fn rubberband_process(
            state: RubberBandState,
            input: *const *const c_float,
            samples: c_uint,
            final_block: c_int,
        );
        pub fn rubberband_available(state: RubberBandState) -> c_int;
        pub fn rubberband_retrieve(
            state: RubberBandState,
            output: *const *mut c_float,
            samples: c_uint,
        ) -> c_uint;

        pub fn rubberband_live_new(
            sample_rate: c_uint,
            channels: c_uint,
            options: RubberBandOptions,
        ) -> RubberBandLiveState;
        pub fn rubberband_live_delete(state: RubberBandLiveState);
        pub fn rubberband_live_reset(state: RubberBandLiveState);
        pub fn rubberband_live_set_pitch_scale(state: RubberBandLiveState, scale: c_double);
        pub fn rubberband_live_set_formant_scale(state: RubberBandLiveState, scale: c_double);
        pub fn rubberband_live_set_formant_option(state: RubberBandLiveState, options: RubberBandOptions);
        pub fn rubberband_live_get_block_size(state: RubberBandLiveState) -> c_uint;
        pub fn rubberband_live_shift(
            state: RubberBandLiveState,
            input: *const *const c_float,
            output: *const *mut c_float,
        );
    }
}

#[cfg(any(not(windows), not(has_rubberband)))]
#[allow(non_upper_case_globals)]
mod rubberband {
    pub type RubberBandState = *mut std::ffi::c_void;
    pub type RubberBandLiveState = *mut std::ffi::c_void;
    pub type RubberBandOptions = i32;
    pub const RubberBandOptionProcessRealTime: RubberBandOptions = 0;
    pub const RubberBandOptionFormantShifted: RubberBandOptions = 0;
    pub const RubberBandOptionFormantPreserved: RubberBandOptions = 0;
    pub const RubberBandOptionPitchHighQuality: RubberBandOptions = 0;
}

pub(crate) struct RubberBandClipState {
    #[cfg(all(windows, has_rubberband))]
    pub(crate) state:  rubberband::RubberBandLiveState,
    pub(crate) channels:  usize,
    pub(crate) sample_rate:  u32,
    pub(crate) pitch_scale:  f64,
    pub(crate) formant_preserve:  bool,
    pub(crate) formant_scale:  f64,
    pub(crate) input_buffers:  Vec<Vec<f32>>,
    pub(crate) output_buffers:  Vec<Vec<f32>>,
    pub(crate) needs_reposition:  bool,
    #[cfg(all(windows, has_rubberband))]
    pub(crate) block_size:  usize,
}

impl RubberBandClipState {
    #[cfg(all(windows, has_rubberband))]
    pub(crate) fn new(
        sample_rate: u32,
        channels: usize,
        pitch_scale: f64,
        formant_preserve: bool,
        formant_scale: f64,
    ) -> Self {
        let options = rubberband::RubberBandOptionPitchHighQuality;
        let state = unsafe { rubberband::rubberband_live_new(sample_rate, channels as u32, options) };
        let block_size = if state.is_null() {
            256
        } else {
            unsafe { rubberband::rubberband_live_get_block_size(state) as usize }
        }
        .max(64);
        if !state.is_null() {
            let formant_option = if formant_preserve {
                rubberband::RubberBandOptionFormantPreserved
            } else {
                rubberband::RubberBandOptionFormantShifted
            };
            unsafe {
                rubberband::rubberband_live_set_pitch_scale(state, pitch_scale);
                rubberband::rubberband_live_set_formant_option(state, formant_option);
                rubberband::rubberband_live_set_formant_scale(state, formant_scale);
            }
        }
        Self {
            state,
            channels,
            sample_rate,
            pitch_scale,
            formant_preserve,
            formant_scale,
            input_buffers: vec![Vec::new(); channels.max(1)],
            output_buffers: vec![Vec::new(); channels.max(1)],
            needs_reposition: false,
            block_size,
        }
    }

    #[cfg(any(not(windows), not(has_rubberband)))]
    pub(crate) fn new(
        sample_rate: u32,
        channels: usize,
        pitch_scale: f64,
        formant_preserve: bool,
        formant_scale: f64,
    ) -> Self {
        Self {
            channels,
            sample_rate,
            pitch_scale,
            formant_preserve,
            formant_scale,
            input_buffers: vec![Vec::new(); channels.max(1)],
            output_buffers: vec![Vec::new(); channels.max(1)],
            needs_reposition: false,
        }
    }

    #[cfg(all(windows, has_rubberband))]
    pub(crate) fn reset_stream(&mut self) {
        if !self.state.is_null() {
            unsafe {
                rubberband::rubberband_live_reset(self.state);
            }
        }
    }

    #[cfg(any(not(windows), not(has_rubberband)))]
    pub(crate) fn reset_stream(&mut self) {
    }

    #[cfg(all(windows, has_rubberband))]
    pub(crate) fn ensure_config(
        &mut self,
        sample_rate: u32,
        channels: usize,
        pitch_scale: f64,
        formant_preserve: bool,
        formant_scale: f64,
    ) {
        if self.sample_rate != sample_rate || self.channels != channels || self.state.is_null() {
            if !self.state.is_null() {
                unsafe { rubberband::rubberband_live_delete(self.state) };
            }
            *self = Self::new(sample_rate, channels, pitch_scale, formant_preserve, formant_scale);
            return;
        }
        if (self.pitch_scale - pitch_scale).abs() > f64::EPSILON {
            self.pitch_scale = pitch_scale;
            unsafe { rubberband::rubberband_live_set_pitch_scale(self.state, pitch_scale) };
        }
        if self.formant_preserve != formant_preserve {
            self.formant_preserve = formant_preserve;
            let option = if formant_preserve {
                rubberband::RubberBandOptionFormantPreserved
            } else {
                rubberband::RubberBandOptionFormantShifted
            };
            unsafe { rubberband::rubberband_live_set_formant_option(self.state, option) };
        }
        if (self.formant_scale - formant_scale).abs() > f64::EPSILON {
            self.formant_scale = formant_scale;
            unsafe { rubberband::rubberband_live_set_formant_scale(self.state, formant_scale) };
        }
    }

    #[cfg(any(not(windows), not(has_rubberband)))]
    pub(crate) fn ensure_config(
        &mut self,
        sample_rate: u32,
        channels: usize,
        pitch_scale: f64,
        formant_preserve: bool,
        formant_scale: f64,
    ) {
        self.sample_rate = sample_rate;
        self.channels = channels;
        self.pitch_scale = pitch_scale;
        self.formant_preserve = formant_preserve;
        self.formant_scale = formant_scale;
        self.needs_reposition = true;
    }
}

#[cfg(all(windows, has_rubberband))]
unsafe impl Send for RubberBandClipState {}

#[cfg(all(windows, has_rubberband))]
unsafe impl Sync for RubberBandClipState {}

#[cfg(all(windows, has_rubberband))]
impl Drop for RubberBandClipState {
    fn drop(&mut self) {
        if !self.state.is_null() {
            unsafe { rubberband::rubberband_live_delete(self.state) };
        }
    }
}

pub(crate) struct AudioClipCache {
    pub(crate) entries:  HashMap<String, Arc<AudioClipData>>,
    pub(crate) order:  VecDeque<String>,
    pub(crate) bytes:  usize,
    pub(crate) max_bytes:  usize,
    pub(crate) max_entries:  usize,
    pub(crate) stretchers:  HashMap<usize, Arc<Mutex<RubberBandClipState>>>,
}

impl AudioClipCache {
    pub(crate) fn new(max_bytes: usize, max_entries: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            max_bytes,
            max_entries,
            stretchers: HashMap::new(),
        }
    }

    pub(crate) fn get(&mut self, key: &str) -> Option<Arc<AudioClipData>> {
        if let Some(data) = self.entries.get(key).cloned() {
            self.touch(key);
            Some(data)
        } else {
            None
        }
    }

    pub(crate) fn insert(&mut self, key: String, data: Arc<AudioClipData>) {
        let new_size = Self::clip_bytes(&data);
        if let Some(existing) = self.entries.get(&key) {
            let old_size = Self::clip_bytes(existing);
            self.bytes = self.bytes.saturating_sub(old_size);
        }
        self.entries.insert(key.clone(), data);
        self.bytes = self.bytes.saturating_add(new_size);
        self.touch(&key);
        self.trim_to_limits();
    }

    pub(crate) fn remove(&mut self, key: &str) {
        if let Some(data) = self.entries.remove(key) {
            let size = Self::clip_bytes(&data);
            self.bytes = self.bytes.saturating_sub(size);
        }
        self.order.retain(|entry| entry != key);
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.bytes = 0;
        self.stretchers.clear();
    }

    pub(crate) fn get_or_create_stretcher(
        &mut self,
        clip_id: usize,
        sample_rate: u32,
        channels: usize,
        pitch_scale: f64,
        formant_preserve: bool,
        formant_scale: f64,
    ) -> Arc<Mutex<RubberBandClipState>> {
        let entry = self.stretchers.entry(clip_id).or_insert_with(|| {
            Arc::new(Mutex::new(RubberBandClipState::new(
                sample_rate,
                channels,
                pitch_scale,
                formant_preserve,
                formant_scale,
            )))
        });
        if let Ok(mut guard) = entry.lock() {
            guard.ensure_config(
                sample_rate,
                channels,
                pitch_scale,
                formant_preserve,
                formant_scale,
            );
        }
        entry.clone()
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn size_bytes(&self) -> usize {
        self.bytes
    }

    pub(crate) fn touch(&mut self, key: &str) {
        self.order.retain(|entry| entry != key);
        self.order.push_back(key.to_string());
    }

    pub(crate) fn trim_to_limits(&mut self) {
        while (self.max_entries > 0 && self.entries.len() > self.max_entries)
            || (self.max_bytes > 0 && self.bytes > self.max_bytes)
        {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(data) = self.entries.remove(&oldest) {
                let size = Self::clip_bytes(&data);
                self.bytes = self.bytes.saturating_sub(size);
            }
        }
    }

    pub(crate) fn clip_bytes(data: &AudioClipData) -> usize {
        data.samples.len().saturating_mul(std::mem::size_of::<f32>())
    }
}

pub(crate) struct AudioRuntimeStats {
    pub(crate) blocks:  AtomicU64,
    pub(crate) overruns:  AtomicU64,
    pub(crate) last_block_ms_bits:  AtomicU32,
    pub(crate) max_block_ms_bits:  AtomicU32,
}

impl AudioRuntimeStats {
    pub(crate) fn new() -> Self {
        Self {
            blocks: AtomicU64::new(0),
            overruns: AtomicU64::new(0),
            last_block_ms_bits: AtomicU32::new(0.0f32.to_bits()),
            max_block_ms_bits: AtomicU32::new(0.0f32.to_bits()),
        }
    }

    pub(crate) fn record_block(&self, elapsed_ms: f32, overrun: bool) {
        self.blocks.fetch_add(1, Ordering::Relaxed);
        if overrun {
            self.overruns.fetch_add(1, Ordering::Relaxed);
        }
        self.last_block_ms_bits.store(elapsed_ms.to_bits(), Ordering::Relaxed);
        let mut current_max = f32::from_bits(self.max_block_ms_bits.load(Ordering::Relaxed));
        if elapsed_ms > current_max {
            current_max = elapsed_ms;
            self.max_block_ms_bits
                .store(current_max.to_bits(), Ordering::Relaxed);
        }
    }

    pub(crate) fn snapshot(&self) -> (u64, u64, f32, f32) {
        (
            self.blocks.load(Ordering::Relaxed),
            self.overruns.load(Ordering::Relaxed),
            f32::from_bits(self.last_block_ms_bits.load(Ordering::Relaxed)),
            f32::from_bits(self.max_block_ms_bits.load(Ordering::Relaxed)),
        )
    }
}

#[derive(Clone)]
pub(crate) struct AudioClipRender {
    pub(crate) clip_id:  usize,
    pub(crate) path:  String,
    pub(crate) track_index:  usize,
    pub(crate) start_samples:  u64,
    pub(crate) length_samples:  u64,
    pub(crate) offset_samples:  u64,
    pub(crate) gain:  f32,
    pub(crate) time_mul:  f32,
    pub(crate) pitch_semitones:  f32,
    pub(crate) stretch_mode:  AudioStretchMode,
    pub(crate) formant_scale:  f32,
}

#[derive(Clone)]
pub(crate) struct RenderPlan {
    pub(crate) path:  String,
    pub(crate) sample_rate:  u32,
    pub(crate) block_size:  usize,
    pub(crate) tempo_bpm:  f32,
    pub(crate) start_beats:  f32,
    pub(crate) end_beats:  f32,
    pub(crate) bitrate_kbps:  u32,
    pub(crate) wav_bit_depth:  RenderWavBitDepth,
    pub(crate) render_tail_mode:  RenderTailMode,
    pub(crate) render_release_seconds:  f32,
    pub(crate) tracks:  Vec<RenderTrack>,
    pub(crate) node_routes:  Vec<NodeRouteLink>,
    pub(crate) notes:  Vec<PianoRollNote>,
    pub(crate) instrument_path:  Option<String>,
    pub(crate) param_ids:  Vec<u32>,
    pub(crate) param_values:  Vec<f32>,
    pub(crate) plugin_state_component:  Option<Vec<u8>>,
    pub(crate) plugin_state_controller:  Option<Vec<u8>>,
    pub(crate) audio_clips:  Vec<AudioClipRender>,
    pub(crate) audio_cache:  HashMap<String, Arc<AudioClipData>>,
    pub(crate) master_settings:  MasterCompSettings,
    pub(crate) license_comment:  Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RenderTailMode {
    Wrap,
    Release,
    Cut,
}

#[derive(Clone)]
pub(crate) struct RenderTrack {
    pub(crate) source_track_index:  Option<usize>,
    pub(crate) notes:  Vec<PianoRollNote>,
    pub(crate) instrument_path:  Option<String>,
    pub(crate) instrument_clap_id:  Option<String>,
    pub(crate) param_ids:  Vec<u32>,
    pub(crate) param_values:  Vec<f32>,
    pub(crate) plugin_state_component:  Option<Vec<u8>>,
    pub(crate) plugin_state_controller:  Option<Vec<u8>>,
    pub(crate) effect_paths:  Vec<String>,
    pub(crate) effect_clap_ids:  Vec<Option<String>>,
    pub(crate) effect_bypass:  Vec<bool>,
    pub(crate) automation_lanes:  Vec<AutomationLane>,
    pub(crate) level:  f32,
    pub(crate) active:  bool,
}

pub(crate) enum RenderHost {
    Vst3(vst3::Vst3Host),
    Clap(clap_host::ClapHost),
}

impl RenderHost {
    pub(crate) fn push_param_change(&mut self, param_id: u32, value: f64) {
        match self {
            RenderHost::Vst3(host) => host.push_param_change(param_id, value),
            RenderHost::Clap(host) => host.push_param_change(param_id, value),
        }
    }

    pub(crate) fn set_state_bytes(
        &mut self,
        component_state: Option<&[u8]>,
        controller_state: Option<&[u8]>,
    ) -> Result<(), String> {
        match self {
            RenderHost::Vst3(host) => host.set_state_bytes(component_state, controller_state),
            RenderHost::Clap(host) => host.set_state_bytes(component_state.unwrap_or(&[])),
        }
    }

    pub(crate) fn apply_state_for_render(
        &mut self,
        component_state: Option<&[u8]>,
        controller_state: Option<&[u8]>,
    ) -> Result<(), String> {
        match self {
            RenderHost::Vst3(host) => host.apply_state_for_render(component_state, controller_state),
            RenderHost::Clap(host) => host.set_state_bytes(component_state.unwrap_or(&[])),
        }
    }

    pub(crate) fn process_f32(
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

    pub(crate) fn process_f32_with_input(
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

    pub(crate) fn io_channels(&self) -> (usize, usize) {
        match self {
            RenderHost::Vst3(host) => host.io_channels(),
            RenderHost::Clap(host) => host.io_channels(),
        }
    }
}

pub(crate) struct RenderJob {
    pub(crate) done:  Arc<AtomicU64>,
    pub(crate) total:  Arc<AtomicU64>,
    pub(crate) finished:  Arc<AtomicBool>,
    pub(crate) result:  Arc<Mutex<Option<Result<String, String>>>>,
}

pub(crate) struct FsEntry {
    pub(crate) name:  String,
    pub(crate) path:  PathBuf,
    pub(crate) is_dir:  bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct AutomationPoint {
    pub(crate) beat:  f32,
    pub(crate) value:  f32,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct AutomationLane {
    pub(crate) name:  String,
    pub(crate) param_id:  u32,
    #[serde(default)]
    pub(crate) target:  AutomationTarget,
    pub(crate) points:  Vec<AutomationPoint>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct MidiCcLane {
    pub(crate) cc:  u8,
    pub(crate) points:  Vec<AutomationPoint>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum AutomationTarget {
    Instrument,
    Effect(usize),
}

impl Default for AutomationTarget {
    fn default() -> Self {
        AutomationTarget::Instrument
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PendingParamTarget {
    Instrument,
    Effect(usize),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PendingParamChange {
    pub(crate) target:  PendingParamTarget,
    pub(crate) param_id:  u32,
    pub(crate) value:  f64,
}

#[derive(Clone)]
pub(crate) struct RecordedAutomationPoint {
    pub(crate) param_id:  u32,
    pub(crate) target:  AutomationTarget,
    pub(crate) beat:  f32,
    pub(crate) value:  f32,
}

#[derive(Clone, Debug)]
pub(crate) struct ActivePerformanceTake {
    pub(crate) source_clip_id:  usize,
    pub(crate) start_beat:  f32,
    pub(crate) trigger_mode:  PerformanceTriggerMode,
    pub(crate) loop_enabled:  bool,
}

#[derive(Clone, Debug)]
pub(crate) struct RecordedPerformanceTake {
    pub(crate) track_index:  usize,
    pub(crate) source_clip_id:  usize,
    pub(crate) start_beat:  f32,
    pub(crate) end_beat:  f32,
    pub(crate) loop_enabled:  bool,
}

pub(crate) struct RecordingBuffers {
    pub(crate) active:  bool,
    pub(crate) track_index:  usize,
    pub(crate) start_samples:  u64,
    pub(crate) start_beats:  f32,
    pub(crate) record_audio:  bool,
    pub(crate) record_midi:  bool,
    pub(crate) record_automation:  bool,
    pub(crate) record_performance:  bool,
    pub(crate) audio_samples:  Vec<f32>,
    pub(crate) audio_channels:  usize,
    pub(crate) audio_sample_rate:  u32,
    pub(crate) midi_active:  HashMap<u8, (f32, u8)>,
    pub(crate) midi_notes:  Vec<PianoRollNote>,
    pub(crate) automation_points:  Vec<RecordedAutomationPoint>,
    pub(crate) performance_active:  HashMap<usize, ActivePerformanceTake>,
    pub(crate) performance_takes:  Vec<RecordedPerformanceTake>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct MasterCompSettings {
    pub(crate) enabled:  bool,
    pub(crate) threshold_db:  f32,
    pub(crate) ratio:  f32,
    pub(crate) attack_ms:  f32,
    pub(crate) release_ms:  f32,
    pub(crate) makeup_db:  f32,
    pub(crate) level:  f32,
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
pub(crate) struct MasterCompState {
    pub(crate) gain:  f32,
}

impl Default for MasterCompState {
    fn default() -> Self {
        Self { gain: 1.0 }
    }
}

pub(crate) struct LicenseJob {
    pub(crate) finished:  Arc<AtomicBool>,
    pub(crate) result:  Arc<Mutex<Option<LicenseJobResult>>>,
}

pub(crate) struct LicenseJobResult {
    pub(crate) status:  Result<String, String>,
    pub(crate) token:  Option<String>,
    pub(crate) license_file:  Option<String>,
    pub(crate) registered_to:  Option<String>,
    pub(crate) remaining_activations:  Option<u64>,
}

pub(crate) struct LicensePayloadInfo {
    pub(crate) registered_to:  Option<String>,
    pub(crate) license_type:  Option<String>,
    pub(crate) max_activations:  Option<u64>,
    pub(crate) monthly_activations:  Option<u64>,
}

pub(crate) enum PluginUiEditor {
    Vst3(vst3::Vst3Editor),
    Clap,
}

pub(crate) struct PluginUiHost {
    pub(crate) hwnd:  isize,
    pub(crate) child_hwnd:  isize,
    pub(crate) editor:  PluginUiEditor,
    pub(crate) host:  PluginHostHandle,
    pub(crate) target:  PluginUiTarget,
    pub(crate) close_requested:  Arc<AtomicBool>,
    pub(crate) floating:  bool,
}

pub(crate) struct CallbackGuard {
    pub(crate) counter:  Arc<AtomicUsize>,
}

impl CallbackGuard {
    pub(crate) fn new(counter: Arc<AtomicUsize>) -> Self {
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
pub(crate) struct UndoState {
    pub(crate) project_name:  String,
    pub(crate) tempo_bpm:  f32,
    pub(crate) tracks:  Vec<Track>,
    pub(crate) selected_clip:  Option<usize>,
    pub(crate) selected_track:  Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClipDragKind {
    Move,
    ResizeStart,
    ResizeEnd,
    TrimStart,
    TrimEnd,
}
