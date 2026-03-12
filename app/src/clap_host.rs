use clack_common::events::event_types::{MidiEvent as ClapMidiEvent, NoteOffEvent, NoteOnEvent, ParamValueEvent};
use clack_common::events::io::EventBuffer;
use clack_common::utils::{ClapId, Cookie};
use clack_extensions::gui::{GuiApiType, GuiConfiguration, GuiSize, HostGui, HostGuiImpl, PluginGui, Window};
use clack_extensions::latency::{HostLatency, HostLatencyImpl, PluginLatency};
use clack_extensions::log::{HostLog, HostLogImpl, LogSeverity};
use clack_extensions::params::{HostParams, HostParamsImplMainThread, HostParamsImplShared, ParamClearFlags, ParamInfoBuffer, ParamRescanFlags, PluginParams};
use clack_extensions::state::{HostState, HostStateImpl, PluginState};
use clack_host::prelude::*;
use clack_host::process::audio_buffers::{AudioPortBuffer, AudioPortBufferType, AudioPorts, InputAudioBuffers};
use clap_sys::entry::clap_plugin_entry;
use clap_sys::factory::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use clap_sys::plugin::clap_plugin_descriptor;
use libloading::Library;
use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::vst3::MidiEvent as VstMidiEvent;

#[derive(Clone, Debug)]
pub struct ParamInfo {
    pub id: u32,
    pub name: String,
    pub default_value: f64,
}

#[derive(Clone, Debug)]
pub struct ClapPluginDescriptor {
    pub id: String,
    pub name: String,
}

#[derive(Default)]
struct ClapHostShared {
    params_ext: OnceLock<Option<PluginParams>>,
    state_ext: OnceLock<Option<PluginState>>,
    gui_ext: OnceLock<Option<PluginGui>>,
    latency_ext: OnceLock<Option<PluginLatency>>,
    restart_requested: AtomicBool,
    process_requested: AtomicBool,
    callback_requested: AtomicBool,
    flush_requested: AtomicBool,
    gui_resize: AtomicU64,
    gui_request_show: AtomicBool,
    gui_request_hide: AtomicBool,
    gui_closed: AtomicBool,
}

impl<'a> SharedHandler<'a> for ClapHostShared {
    fn initializing(&self, instance: InitializingPluginHandle<'a>) {
        let _ = self.params_ext.set(instance.get_extension());
        let _ = self.state_ext.set(instance.get_extension());
        let _ = self.gui_ext.set(instance.get_extension());
        let _ = self.latency_ext.set(instance.get_extension());
    }

    fn request_restart(&self) {
        self.restart_requested.store(true, Ordering::SeqCst);
    }

    fn request_process(&self) {
        self.process_requested.store(true, Ordering::SeqCst);
    }

    fn request_callback(&self) {
        self.callback_requested.store(true, Ordering::SeqCst);
    }
}

impl HostParamsImplShared for ClapHostShared {
    fn request_flush(&self) {
        self.flush_requested.store(true, Ordering::SeqCst);
    }
}

impl HostGuiImpl for ClapHostShared {
    fn resize_hints_changed(&self) {}

    fn request_resize(&self, new_size: GuiSize) -> Result<(), HostError> {
        self.gui_resize.store(new_size.pack_to_u64(), Ordering::SeqCst);
        Ok(())
    }

    fn request_show(&self) -> Result<(), HostError> {
        self.gui_request_show.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn request_hide(&self) -> Result<(), HostError> {
        self.gui_request_hide.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn closed(&self, _was_destroyed: bool) {
        self.gui_closed.store(true, Ordering::SeqCst);
    }
}

impl HostLogImpl for ClapHostShared {
    fn log(&self, severity: LogSeverity, message: &str) {
        eprintln!("[CLAP {severity:?}] {message}");
    }
}

struct ClapHostMainThread<'a> {
    shared: &'a ClapHostShared,
    instance: Option<InitializedPluginHandle<'a>>,
    rescan_flags: Mutex<Vec<ParamRescanFlags>>,
    state_dirty: bool,
    latency_changed: bool,
}

impl<'a> MainThreadHandler<'a> for ClapHostMainThread<'a> {
    fn initialized(&mut self, instance: InitializedPluginHandle<'a>) {
        self.instance = Some(instance);
    }
}

impl HostParamsImplMainThread for ClapHostMainThread<'_> {
    fn rescan(&mut self, flags: ParamRescanFlags) {
        if let Ok(mut pending) = self.rescan_flags.lock() {
            pending.push(flags);
        }
    }

    fn clear(&mut self, _param_id: ClapId, _flags: ParamClearFlags) {}
}

impl HostStateImpl for ClapHostMainThread<'_> {
    fn mark_dirty(&mut self) {
        self.state_dirty = true;
    }
}

impl HostLatencyImpl for ClapHostMainThread<'_> {
    fn changed(&mut self) {
        self.latency_changed = true;
    }
}

struct ClapHostHandlers;

impl HostHandlers for ClapHostHandlers {
    type Shared<'a> = ClapHostShared;
    type MainThread<'a> = ClapHostMainThread<'a>;
    type AudioProcessor<'a> = ();

    fn declare_extensions(builder: &mut HostExtensions<Self>, _shared: &Self::Shared<'_>) {
        builder
            .register::<HostParams>()
            .register::<HostState>()
            .register::<HostGui>()
            .register::<HostLatency>()
            .register::<HostLog>();
    }
}

pub struct ClapHost {
    entry: PluginEntry,
    instance: PluginInstance<ClapHostHandlers>,
    audio_processor: PluginAudioProcessor<ClapHostHandlers>,
    params_ext: Option<PluginParams>,
    state_ext: Option<PluginState>,
    gui_ext: Option<PluginGui>,
    latency_ext: Option<PluginLatency>,
    input_channels: usize,
    output_channels: usize,
    input_buffers: Vec<Vec<f32>>,
    output_buffers: Vec<Vec<f32>>,
    input_ports: AudioPorts,
    output_ports: AudioPorts,
    input_events: EventBuffer,
    output_events: EventBuffer,
    pending_params: Vec<(u32, f64)>,
    gui_parent: Option<Window<'static>>,
    gui_open: bool,
    gui_size: Option<GuiSize>,
    gui_created: bool,
}

// Safety: ClapHost is only accessed behind a Mutex and never concurrently without locking.
unsafe impl Send for ClapHost {}
unsafe impl Sync for ClapHost {}

impl ClapHost {
    pub fn load(
        path: &str,
        plugin_id: &str,
        sample_rate: f64,
        block_size: u32,
        input_channels: usize,
        output_channels: usize,
    ) -> Result<Self, String> {
        let host_info = HostInfo::new("LingStation", "LingStation", "", "0.1")
            .map_err(|e| e.to_string())?;
        let module_path = resolve_clap_binary(path)?;
        let entry = unsafe { PluginEntry::load(&module_path) }.map_err(|e| e.to_string())?;
        let plugin_id = CString::new(plugin_id).map_err(|e| e.to_string())?;

        let mut instance = PluginInstance::<ClapHostHandlers>::new(
            |_| ClapHostShared::default(),
            |shared| ClapHostMainThread {
                shared,
                instance: None,
                rescan_flags: Mutex::new(Vec::new()),
                state_dirty: false,
                latency_changed: false,
            },
            &entry,
            plugin_id.as_c_str(),
            &host_info,
        )
        .map_err(|e| format!("CLAP instance failed: {e}"))?;

        let audio_config = PluginAudioConfiguration {
            sample_rate,
            min_frames_count: block_size.max(1),
            max_frames_count: block_size.max(1),
        };
        let processor = instance
            .activate(|_, _| (), audio_config)
            .map_err(|e| format!("CLAP activate failed: {e}"))?;

        let params_ext = instance.access_shared_handler(|h| h.params_ext.get().copied().flatten());
        let state_ext = instance.access_shared_handler(|h| h.state_ext.get().copied().flatten());
        let gui_ext = instance.access_shared_handler(|h| h.gui_ext.get().copied().flatten());
        let latency_ext = instance.access_shared_handler(|h| h.latency_ext.get().copied().flatten());

        Ok(Self {
            entry,
            instance,
            audio_processor: PluginAudioProcessor::from(processor),
            params_ext,
            state_ext,
            gui_ext,
            latency_ext,
            input_channels,
            output_channels,
            input_buffers: Vec::new(),
            output_buffers: Vec::new(),
            input_ports: AudioPorts::with_capacity(input_channels.max(1), 1),
            output_ports: AudioPorts::with_capacity(output_channels.max(1), 1),
            input_events: EventBuffer::with_capacity(64),
            output_events: EventBuffer::with_capacity(64),
            pending_params: Vec::new(),
            gui_parent: None,
            gui_open: false,
            gui_size: None,
            gui_created: false,
        })
    }

    pub fn enumerate_params(&mut self) -> Vec<ParamInfo> {
        let Some(params) = self.params_ext else {
            return Vec::new();
        };
        let mut handle = self.instance.plugin_handle();
        let count = params.count(&mut handle);
        let mut buffer = ParamInfoBuffer::new();
        let mut out = Vec::with_capacity(count as usize);
        for index in 0..count {
            if let Some(info) = params.get_info(&mut handle, index, &mut buffer) {
                let name = String::from_utf8_lossy(info.name).to_string();
                out.push(ParamInfo {
                    id: info.id.get(),
                    name,
                    default_value: info.default_value,
                });
            }
        }
        out
    }

    pub fn push_param_change(&mut self, param_id: u32, value: f64) {
        self.pending_params.push((param_id, value));
    }

    pub fn process_f32(
        &mut self,
        output: &mut [f32],
        channels: usize,
        midi_events: &[VstMidiEvent],
    ) -> Result<(), String> {
        self.process_internal(None, output, channels, midi_events)
    }

    pub fn process_f32_with_input(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        channels: usize,
        midi_events: &[VstMidiEvent],
    ) -> Result<(), String> {
        self.process_internal(Some(input), output, channels, midi_events)
    }

    pub fn get_state_bytes(&mut self) -> Vec<u8> {
        let Some(state) = self.state_ext else {
            return Vec::new();
        };
        let mut handle = self.instance.plugin_handle();
        let mut buffer = Vec::new();
        if state.save(&mut handle, &mut buffer).is_ok() {
            buffer
        } else {
            Vec::new()
        }
    }

    pub fn set_state_bytes(&mut self, bytes: &[u8]) -> Result<(), String> {
        let Some(state) = self.state_ext else {
            return Ok(());
        };
        let mut handle = self.instance.plugin_handle();
        let mut cursor = std::io::Cursor::new(bytes);
        state.load(&mut handle, &mut cursor).map_err(|e| e.to_string())
    }

    pub fn io_channels(&self) -> (usize, usize) {
        (self.input_channels, self.output_channels)
    }

    pub fn latency_samples(&mut self) -> u32 {
        let Some(latency) = self.latency_ext else {
            return 0;
        };
        let mut handle = self.instance.plugin_handle();
        latency.get(&mut handle)
    }

    pub fn open_gui(&mut self, parent_hwnd: isize) -> Result<(), String> {
        let Some(gui) = self.gui_ext else {
            return Err("CLAP GUI not supported".to_string());
        };
        let mut handle = self.instance.plugin_handle();
        let api = GuiApiType::WIN32;
        let embedded = GuiConfiguration {
            api_type: api,
            is_floating: false,
        };
        let floating = GuiConfiguration {
            api_type: api,
            is_floating: true,
        };
        let can_embed = parent_hwnd != 0 && gui.is_api_supported(&mut handle, embedded);
        if can_embed {
            if gui.create(&mut handle, embedded).is_ok() {
                self.gui_created = true;
                let window = Window::from_win32_hwnd(parent_hwnd as *mut _);
                if unsafe { gui.set_parent(&mut handle, window) }.is_ok() {
                    if let Some(size) = gui.get_size(&mut handle) {
                        self.gui_size = Some(size);
                    }
                    gui.show(&mut handle).map_err(|e| e.to_string())?;
                    self.gui_parent = Some(window);
                    self.gui_open = true;
                    return Ok(());
                }
                gui.destroy(&mut handle);
                self.gui_created = false;
            }
        }

        if !gui.is_api_supported(&mut handle, floating) {
            return Err("CLAP GUI does not support Win32".to_string());
        }
        gui.create(&mut handle, floating).map_err(|e| e.to_string())?;
        self.gui_created = true;
        if let Some(size) = gui.get_size(&mut handle) {
            self.gui_size = Some(size);
        }
        gui.show(&mut handle).map_err(|e| e.to_string())?;
        self.gui_parent = None;
        self.gui_open = true;
        Ok(())
    }

    pub fn supports_embedded_gui(&mut self) -> bool {
        let Some(gui) = self.gui_ext else {
            return false;
        };
        let mut handle = self.instance.plugin_handle();
        let config = GuiConfiguration {
            api_type: GuiApiType::WIN32,
            is_floating: false,
        };
        gui.is_api_supported(&mut handle, config)
    }

    pub fn hide_gui(&mut self) {
        let Some(gui) = self.gui_ext else {
            return;
        };
        if !self.gui_created {
            return;
        }
        let mut handle = self.instance.plugin_handle();
        let _ = gui.hide(&mut handle);
        self.gui_open = false;
    }

    pub fn show_gui(&mut self) {
        let Some(gui) = self.gui_ext else {
            return;
        };
        if !self.gui_created {
            return;
        }
        let mut handle = self.instance.plugin_handle();
        let _ = gui.show(&mut handle);
        self.gui_open = true;
    }

    pub fn destroy_gui(&mut self) {
        let Some(gui) = self.gui_ext else {
            return;
        };
        if !self.gui_created {
            return;
        }
        let mut handle = self.instance.plugin_handle();
        gui.destroy(&mut handle);
        self.gui_created = false;
        self.gui_open = false;
        self.gui_parent = None;
        self.gui_size = None;
    }

    pub fn gui_size(&self) -> Option<(i32, i32)> {
        self.gui_size
            .map(|size| (size.width as i32, size.height as i32))
    }

    pub fn gui_embedded(&self) -> bool {
        self.gui_parent.is_some()
    }

    pub fn take_gui_resize(&mut self) -> Option<(i32, i32)> {
        let packed = self
            .instance
            .access_shared_handler(|h| h.gui_resize.swap(0, Ordering::SeqCst));
        if packed == 0 {
            return None;
        }
        let size = GuiSize::unpack_from_u64(packed);
        self.gui_size = Some(size);
        Some((size.width as i32, size.height as i32))
    }

    pub fn take_gui_closed(&mut self) -> bool {
        self.instance.access_shared_handler(|h| h.gui_closed.swap(false, Ordering::SeqCst))
    }

    pub fn prepare_for_drop(&mut self) {
        let _ = self.audio_processor.ensure_processing_stopped();
        let _ = self.instance.try_deactivate();
        self.destroy_gui();
    }

    fn process_internal(
        &mut self,
        input: Option<&[f32]>,
        output: &mut [f32],
        channels: usize,
        midi_events: &[VstMidiEvent],
    ) -> Result<(), String> {
        if channels == 0 {
            return Ok(());
        }
        let frames = output.len() / channels;
        if frames == 0 {
            return Ok(());
        }
        if let Some(input) = input {
            if input.len() < frames * channels {
                return Err("CLAP input buffer too small".to_string());
            }
        }

        self.ensure_buffers(frames, channels, input.is_some());
        self.input_events.clear();

        for (param_id, value) in self.pending_params.drain(..) {
            let event = ParamValueEvent::new(
                0,
                ClapId::new(param_id),
                clack_common::events::Pckn::new(0u8, 0u8, 0u8, 0u8),
                value,
                Cookie::empty(),
            );
            self.input_events.push(&event);
        }

        for event in midi_events {
            match *event {
                VstMidiEvent::NoteOn {
                    channel,
                    note,
                    velocity,
                    sample_offset,
                } => {
                    let pckn =
                        clack_common::events::Pckn::new(0u8, channel, note, 0u8);
                    let event = NoteOnEvent::new(
                        sample_offset.max(0) as u32,
                        pckn,
                        (velocity as f64 / 127.0).clamp(0.0, 1.0),
                    );
                    self.input_events.push(&event);
                }
                VstMidiEvent::NoteOff {
                    channel,
                    note,
                    velocity,
                    sample_offset,
                } => {
                    let pckn =
                        clack_common::events::Pckn::new(0u8, channel, note, 0u8);
                    let event = NoteOffEvent::new(
                        sample_offset.max(0) as u32,
                        pckn,
                        (velocity as f64 / 127.0).clamp(0.0, 1.0),
                    );
                    self.input_events.push(&event);
                }
                VstMidiEvent::ControlChange {
                    channel,
                    controller,
                    value,
                } => {
                    let status = 0xB0 | (channel & 0x0F);
                    let event = ClapMidiEvent::new(0, 0, [status, controller, value]);
                    self.input_events.push(&event);
                }
            }
        }

        self.input_events.sort();
        self.output_events.clear();

        let input_ports = if let Some(input) = input {
            self.fill_input_buffers(input, channels, frames);
            self.input_ports.with_input_buffers([AudioPortBuffer {
                latency: 0,
                channels: AudioPortBufferType::f32_input_only(
                    self.input_buffers
                        .iter_mut()
                        .map(|b| clack_host::process::audio_buffers::InputChannel::variable(b)),
                ),
            }])
        } else {
            InputAudioBuffers::empty()
        };

        let input_events = self.input_events.as_input();

        let mut output_events = self.output_events.as_output();

        let mut output_ports = self.output_ports.with_output_buffers([AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_output_only(
                self.output_buffers.iter_mut().map(|b| b.as_mut_slice()),
            ),
        }]);

        let started = self
            .audio_processor
            .ensure_processing_started()
            .map_err(|e| format!("CLAP start failed: {e}"))?;
        let _ = started
            .process(
                &input_ports,
                &mut output_ports,
                &input_events,
                &mut output_events,
                None,
                None,
            )
            .map_err(|e| format!("CLAP process failed: {e}"))?;

        self.mix_output(output, channels, frames);
        Ok(())
    }

    fn ensure_buffers(&mut self, frames: usize, channels: usize, has_input: bool) {
        if self.output_buffers.len() != channels {
            self.output_buffers = vec![vec![0.0; frames]; channels];
        }
        for buffer in &mut self.output_buffers {
            if buffer.len() != frames {
                buffer.resize(frames, 0.0);
            }
            buffer.fill(0.0);
        }

        if has_input {
            if self.input_buffers.len() != channels {
                self.input_buffers = vec![vec![0.0; frames]; channels];
            }
            for buffer in &mut self.input_buffers {
                if buffer.len() != frames {
                    buffer.resize(frames, 0.0);
                }
            }
        }
    }

    fn fill_input_buffers(&mut self, input: &[f32], channels: usize, frames: usize) {
        if self.input_buffers.len() != channels {
            return;
        }
        for frame in 0..frames {
            let base = frame * channels;
            for ch in 0..channels {
                if let Some(buf) = self.input_buffers.get_mut(ch) {
                    buf[frame] = input[base + ch];
                }
            }
        }
    }

    fn mix_output(&mut self, output: &mut [f32], channels: usize, frames: usize) {
        for frame in 0..frames {
            let base = frame * channels;
            for ch in 0..channels {
                let sample = self
                    .output_buffers
                    .get(ch)
                    .and_then(|b| b.get(frame))
                    .copied()
                    .unwrap_or(0.0);
                output[base + ch] = sample;
            }
        }
    }
}

pub fn default_plugin_id(path: &str) -> Result<String, String> {
    let plugins = enumerate_plugins(path)?;
    plugins
        .into_iter()
        .next()
        .map(|p| p.id)
        .ok_or_else(|| "No CLAP plugins found".to_string())
}

pub fn enumerate_plugins(path: &str) -> Result<Vec<ClapPluginDescriptor>, String> {
    let module_path = resolve_clap_binary(path)?;
    unsafe {
        let lib = Library::new(&module_path).map_err(|e| e.to_string())?;
        let entry_symbol: libloading::Symbol<*const clap_plugin_entry> =
            lib.get(b"clap_entry\0").map_err(|e| e.to_string())?;
        let entry_ptr = *entry_symbol;
        if entry_ptr.is_null() {
            return Err("CLAP entry is null".to_string());
        }
        let entry = &*entry_ptr;
        let c_path = CString::new(module_path.to_string_lossy().to_string())
            .map_err(|e| e.to_string())?;
        if let Some(init) = entry.init {
            if !init(c_path.as_ptr()) {
                return Err("CLAP init failed".to_string());
            }
        }
        let get_factory = entry
            .get_factory
            .ok_or_else(|| "CLAP get_factory missing".to_string())?;
        let factory_ptr = get_factory(CLAP_PLUGIN_FACTORY_ID.as_ptr());
        if factory_ptr.is_null() {
            return Err("CLAP factory not found".to_string());
        }
        let factory = factory_ptr as *const clap_plugin_factory;
        let get_count = (*factory)
            .get_plugin_count
            .ok_or_else(|| "CLAP get_plugin_count missing".to_string())?;
        let get_desc = (*factory)
            .get_plugin_descriptor
            .ok_or_else(|| "CLAP get_plugin_descriptor missing".to_string())?;
        let count = get_count(factory);
        let mut out = Vec::new();
        for index in 0..count {
            let desc_ptr = get_desc(factory, index);
            if desc_ptr.is_null() {
                continue;
            }
            let desc: &clap_plugin_descriptor = &*desc_ptr;
            let id = if desc.id.is_null() {
                "".to_string()
            } else {
                CStr::from_ptr(desc.id).to_string_lossy().to_string()
            };
            let name = if desc.name.is_null() {
                "CLAP Plugin".to_string()
            } else {
                CStr::from_ptr(desc.name).to_string_lossy().to_string()
            };
            out.push(ClapPluginDescriptor { id, name });
        }
        if let Some(deinit) = entry.deinit {
            deinit();
        }
        Ok(out)
    }
}

fn resolve_clap_binary(path: &str) -> Result<std::path::PathBuf, String> {
    let input = std::path::Path::new(path);
    if input.is_file() {
        return Ok(input.to_path_buf());
    }
    if !input.is_dir() {
        return Err("CLAP path not found".to_string());
    }

    #[cfg(target_os = "macos")]
    {
        let macos_dir = input.join("Contents").join("MacOS");
        if let Ok(entries) = std::fs::read_dir(&macos_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    return Ok(path);
                }
            }
        }
    }

    let mut stack = vec![input.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            #[cfg(windows)]
            let ok = ext == "clap" || ext == "dll";
            #[cfg(target_os = "linux")]
            let ok = ext == "clap" || ext == "so";
            #[cfg(target_os = "macos")]
            let ok = ext == "clap";
            if ok {
                return Ok(path);
            }
        }
    }
    Err("CLAP binary not found".to_string())
}
