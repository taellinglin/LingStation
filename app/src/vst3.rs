use com_scrape_types::{Class, ComPtr, ComWrapper};
use libloading::Library;
use std::collections::HashMap;
use std::ffi::CStr;
use std::path::{Path, PathBuf};
use std::sync::{atomic::{AtomicUsize, Ordering}, Arc, Mutex};
use vst3::Steinberg::FIDString;
use vst3::Steinberg::{
    int32, int64, kNotImplemented, kResultFalse, kResultOk, kResultTrue, tresult, FUnknown,
    IBStream, IBStreamTrait, IPlugFrame, IPlugFrameTrait, IPlugView, IPlugViewTrait, IPluginBase,
    IPluginFactory, IPluginFactory3, IPluginFactory3Trait, IPlugFrame_iid, PClassInfo, TUID, ViewRect,
};
use vst3::Steinberg::Vst::{
    AudioBusBuffers, AudioBusBuffers__type0, BusDirections_, Event, Event__type0,
    Event_::EventTypes_, IAttributeList, IAttributeListTrait, IAttributeList_::AttrID,
    IAudioProcessor, IAudioProcessorTrait, IComponent, IComponentTrait, IComponentHandler,
    IComponentHandlerTrait, IComponent_iid, IConnectionPoint, IConnectionPointTrait, IEditController,
    IEditControllerTrait, IEditController_iid, IEventList, IEventListTrait, IHostApplication,
    IHostApplicationTrait, IMessage, IMessageTrait, IMidiMapping, IMidiMappingTrait, IoModes_,
    IPlugInterfaceSupport, IPlugInterfaceSupportTrait,
    IParamValueQueue, IParamValueQueueTrait, IParameterChanges, IParameterChangesTrait, MediaTypes_,
    NoteOffEvent, NoteOnEvent, ParamID, ParamValue, ParameterInfo, ProcessContext, ProcessData,
    ProcessModes_, ProcessSetup, SpeakerArr, String128, SymbolicSampleSizes_, TChar, ViewType,
    IAttributeList_iid, IMessage_iid, IConnectionPoint_iid, IComponentHandler_iid,
    IPlugInterfaceSupport_iid,
};
use vst3::Steinberg::IPluginFactory3_iid;
use vst3::Steinberg::IBStream_::IStreamSeekMode_;

type GetPluginFactoryFn = unsafe extern "system" fn() -> *mut IPluginFactory;

#[cfg(windows)]
pub fn init_windows_com_for_thread() {
    use std::cell::Cell;
    use windows_sys::Win32::System::Com::{
        CoInitializeEx, COINIT_APARTMENTTHREADED,
    };
    use windows_sys::Win32::System::Ole::OleInitialize;
    thread_local! {
        static COM_INIT: Cell<bool> = Cell::new(false);
    }
    let already = COM_INIT.with(|flag| {
        if flag.get() {
            true
        } else {
            flag.set(true);
            false
        }
    });
    if already {
        return;
    }
    unsafe {
        let sta = CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED as u32);
        if sta == 0 {
            let _ = OleInitialize(std::ptr::null_mut());
        }
    }
}

#[cfg(not(windows))]
pub fn init_windows_com_for_thread() {}

#[derive(Clone, Debug)]
pub struct ParamInfo {
    pub id: u32,
    pub name: String,
    pub default_value: f64,
}

struct ParamValueQueue {
    id: ParamID,
    points: Vec<(i32, ParamValue)>,
}

impl ParamValueQueue {
    fn new(id: ParamID) -> Self {
        Self {
            id,
            points: Vec::new(),
        }
    }
}

impl IParamValueQueueTrait for ParamValueQueue {
    unsafe fn getParameterId(&self) -> ParamID {
        self.id
    }

    unsafe fn getPointCount(&self) -> i32 {
        self.points.len() as i32
    }

    unsafe fn getPoint(&self, index: i32, sample_offset: *mut i32, value: *mut ParamValue) -> tresult {
        if index < 0 || index as usize >= self.points.len() || sample_offset.is_null() || value.is_null() {
            return kResultFalse;
        }
        let (offset, val) = self.points[index as usize];
        *sample_offset = offset;
        *value = val;
        kResultOk
    }

    unsafe fn addPoint(&self, _sample_offset: i32, _value: ParamValue, _index: *mut i32) -> tresult {
        kResultFalse
    }
}

impl Class for ParamValueQueue {
    type Interfaces = (IParamValueQueue,);
}

struct ParameterChanges {
    queues: Mutex<Vec<ComWrapper<ParamValueQueue>>>,
}

impl ParameterChanges {
    fn from_changes(changes: Vec<(ParamID, ParamValue)>) -> Self {
        let mut queues = Vec::new();
        for (id, value) in changes {
            let mut queue = ParamValueQueue::new(id);
            queue.points.push((0, value));
            queues.push(ComWrapper::new(queue));
        }
        Self {
            queues: Mutex::new(queues),
        }
    }
}

impl IParameterChangesTrait for ParameterChanges {
    unsafe fn getParameterCount(&self) -> i32 {
        self.queues.lock().map(|q| q.len() as i32).unwrap_or(0)
    }

    unsafe fn getParameterData(&self, index: i32) -> *mut IParamValueQueue {
        let guard = match self.queues.lock() {
            Ok(guard) => guard,
            Err(_) => return std::ptr::null_mut(),
        };
        if index < 0 || index as usize >= guard.len() {
            return std::ptr::null_mut();
        }
        guard[index as usize]
            .to_com_ptr::<IParamValueQueue>()
            .map(|ptr| ptr.into_raw())
            .unwrap_or(std::ptr::null_mut())
    }

    unsafe fn addParameterData(&self, id: *const ParamID, index: *mut i32) -> *mut IParamValueQueue {
        if id.is_null() {
            return std::ptr::null_mut();
        }
        let id_value = *id;
        let mut guard = match self.queues.lock() {
            Ok(guard) => guard,
            Err(_) => return std::ptr::null_mut(),
        };
        let position = guard.len() as i32;
        let queue = ComWrapper::new(ParamValueQueue::new(id_value));
        let ptr = queue
            .to_com_ptr::<IParamValueQueue>()
            .map(|ptr| ptr.into_raw())
            .unwrap_or(std::ptr::null_mut());
        guard.push(queue);
        if !index.is_null() {
            *index = position;
        }
        ptr
    }
}

impl Class for ParameterChanges {
    type Interfaces = (IParameterChanges,);
}

struct ComponentHandler {
    last_param_change: Arc<Mutex<Option<(ParamID, ParamValue)>>>,
    pending_param_changes: Arc<Mutex<Vec<(ParamID, ParamValue)>>>,
}

struct HostConnectionPoint {
    peer: Mutex<Option<ComPtr<IConnectionPoint>>>,
}

impl HostConnectionPoint {
    fn new() -> Self {
        Self {
            peer: Mutex::new(None),
        }
    }
}

impl IConnectionPointTrait for HostConnectionPoint {
    unsafe fn connect(&self, other: *mut IConnectionPoint) -> tresult {
        if other.is_null() {
            return kResultFalse;
        }
        let peer = match ComPtr::from_raw(other) {
            Some(ptr) => ptr,
            None => return kResultFalse,
        };
        if let Ok(mut slot) = self.peer.lock() {
            *slot = Some(peer);
            kResultOk
        } else {
            kResultFalse
        }
    }

    unsafe fn disconnect(&self, _other: *mut IConnectionPoint) -> tresult {
        if let Ok(mut slot) = self.peer.lock() {
            *slot = None;
            kResultOk
        } else {
            kResultFalse
        }
    }

    unsafe fn notify(&self, message: *mut IMessage) -> tresult {
        let peer = match self.peer.lock() {
            Ok(slot) => slot.clone(),
            Err(_) => None,
        };
        if let Some(peer) = peer {
            return peer.notify(message);
        }
        kResultOk
    }
}

impl Class for HostConnectionPoint {
    type Interfaces = (IConnectionPoint,);
}

struct AttributeList {
    ints: Mutex<HashMap<String, int64>>,
    floats: Mutex<HashMap<String, f64>>,
    strings: Mutex<HashMap<String, Vec<TChar>>>,
    binary: Mutex<HashMap<String, Vec<u8>>>,
}

impl AttributeList {
    fn new() -> Self {
        Self {
            ints: Mutex::new(HashMap::new()),
            floats: Mutex::new(HashMap::new()),
            strings: Mutex::new(HashMap::new()),
            binary: Mutex::new(HashMap::new()),
        }
    }
}

impl IAttributeListTrait for AttributeList {
    unsafe fn setInt(&self, id: AttrID, value: int64) -> tresult {
        let Some(key) = attr_id_to_string(id) else {
            return kResultFalse;
        };
        if let Ok(mut map) = self.ints.lock() {
            map.insert(key, value);
            kResultOk
        } else {
            kResultFalse
        }
    }

    unsafe fn getInt(&self, id: AttrID, value: *mut int64) -> tresult {
        if value.is_null() {
            return kResultFalse;
        }
        let Some(key) = attr_id_to_string(id) else {
            return kResultFalse;
        };
        if let Ok(map) = self.ints.lock() {
            if let Some(found) = map.get(&key) {
                *value = *found;
                return kResultOk;
            }
        }
        kResultFalse
    }

    unsafe fn setFloat(&self, id: AttrID, value: f64) -> tresult {
        let Some(key) = attr_id_to_string(id) else {
            return kResultFalse;
        };
        if let Ok(mut map) = self.floats.lock() {
            map.insert(key, value);
            kResultOk
        } else {
            kResultFalse
        }
    }

    unsafe fn getFloat(&self, id: AttrID, value: *mut f64) -> tresult {
        if value.is_null() {
            return kResultFalse;
        }
        let Some(key) = attr_id_to_string(id) else {
            return kResultFalse;
        };
        if let Ok(map) = self.floats.lock() {
            if let Some(found) = map.get(&key) {
                *value = *found;
                return kResultOk;
            }
        }
        kResultFalse
    }

    unsafe fn setString(&self, id: AttrID, string: *const TChar) -> tresult {
        let Some(key) = attr_id_to_string(id) else {
            return kResultFalse;
        };
        if string.is_null() {
            return kResultFalse;
        }
        let mut data = Vec::new();
        let mut ptr = string;
        loop {
            let ch = *ptr;
            data.push(ch);
            if ch == 0 {
                break;
            }
            ptr = ptr.add(1);
        }
        if let Ok(mut map) = self.strings.lock() {
            map.insert(key, data);
            kResultOk
        } else {
            kResultFalse
        }
    }

    unsafe fn getString(
        &self,
        id: AttrID,
        string: *mut TChar,
        sizeInBytes: u32,
    ) -> tresult {
        if string.is_null() || sizeInBytes == 0 {
            return kResultFalse;
        }
        let Some(key) = attr_id_to_string(id) else {
            return kResultFalse;
        };
        if let Ok(map) = self.strings.lock() {
            if let Some(found) = map.get(&key) {
                let max_chars = (sizeInBytes as usize / std::mem::size_of::<TChar>()).max(1);
                let mut count = found.len().min(max_chars);
                if count == max_chars {
                    count -= 1;
                }
                std::ptr::copy_nonoverlapping(found.as_ptr(), string, count);
                *string.add(count) = 0;
                return kResultOk;
            }
        }
        kResultFalse
    }

    unsafe fn setBinary(&self, id: AttrID, data: *const std::ffi::c_void, sizeInBytes: u32) -> tresult {
        let Some(key) = attr_id_to_string(id) else {
            return kResultFalse;
        };
        if data.is_null() || sizeInBytes == 0 {
            return kResultFalse;
        }
        let slice = std::slice::from_raw_parts(data as *const u8, sizeInBytes as usize);
        if let Ok(mut map) = self.binary.lock() {
            map.insert(key, slice.to_vec());
            kResultOk
        } else {
            kResultFalse
        }
    }

    unsafe fn getBinary(
        &self,
        id: AttrID,
        data: *mut *const std::ffi::c_void,
        sizeInBytes: *mut u32,
    ) -> tresult {
        if data.is_null() || sizeInBytes.is_null() {
            return kResultFalse;
        }
        let Some(key) = attr_id_to_string(id) else {
            return kResultFalse;
        };
        if let Ok(map) = self.binary.lock() {
            if let Some(found) = map.get(&key) {
                *data = found.as_ptr() as *const std::ffi::c_void;
                *sizeInBytes = found.len() as u32;
                return kResultOk;
            }
        }
        kResultFalse
    }
}

impl Class for AttributeList {
    type Interfaces = (IAttributeList,);
}

struct HostMessage {
    message_id: Mutex<String>,
    message_id_c: Mutex<std::ffi::CString>,
    attributes: ComWrapper<AttributeList>,
}

impl HostMessage {
    fn new() -> Self {
        Self {
            message_id: Mutex::new(String::new()),
            message_id_c: Mutex::new(std::ffi::CString::new("").unwrap()),
            attributes: ComWrapper::new(AttributeList::new()),
        }
    }
}

impl IMessageTrait for HostMessage {
    unsafe fn getMessageID(&self) -> FIDString {
        self.message_id_c
            .lock()
            .ok()
            .map(|id| id.as_ptr())
            .unwrap_or(std::ptr::null())
    }

    unsafe fn setMessageID(&self, id: FIDString) {
        if id.is_null() {
            return;
        }
        let name = CStr::from_ptr(id).to_string_lossy().into_owned();
        if let Ok(mut msg) = self.message_id.lock() {
            *msg = name.clone();
        }
        if let Ok(mut msg) = self.message_id_c.lock() {
            if let Ok(cstr) = std::ffi::CString::new(name) {
                *msg = cstr;
            }
        }
    }

    unsafe fn getAttributes(&self) -> *mut IAttributeList {
        let ptr = self
            .attributes
            .to_com_ptr::<IAttributeList>()
            .map(|ptr| ptr.into_raw())
            .unwrap_or(std::ptr::null_mut());
        ptr
    }
}

impl Class for HostMessage {
    type Interfaces = (IMessage,);
}

impl IComponentHandlerTrait for ComponentHandler {
    unsafe fn beginEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }

    unsafe fn performEdit(&self, id: ParamID, value_normalized: ParamValue) -> tresult {
        if let Ok(mut last) = self.last_param_change.lock() {
            *last = Some((id, value_normalized));
        }
        if let Ok(mut pending) = self.pending_param_changes.lock() {
            pending.push((id, value_normalized));
        }
        kResultOk
    }

    unsafe fn endEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }

    unsafe fn restartComponent(&self, _flags: int32) -> tresult {
        kResultOk
    }
}

impl Class for ComponentHandler {
    type Interfaces = (IComponentHandler,);
}

struct MemoryStream {
    data: Vec<u8>,
    cursor: i64,
}

impl MemoryStream {
    fn new() -> Self {
        Self {
            data: Vec::new(),
            cursor: 0,
        }
    }

    fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            data: bytes.to_vec(),
            cursor: 0,
        }
    }

    fn bytes(&self) -> Vec<u8> {
        self.data.clone()
    }
}

impl IBStreamTrait for MemoryStream {
    unsafe fn read(
        &self,
        buffer: *mut std::ffi::c_void,
        num_bytes: int32,
        num_bytes_read: *mut int32,
    ) -> tresult {
        if buffer.is_null() {
            return kResultFalse;
        }
        let mut bytes_read = 0i32;
        if num_bytes > 0 && self.cursor >= 0 {
            let available = self.data.len() as i64 - self.cursor;
            if available > 0 {
                let count = (num_bytes as i64).min(available) as usize;
                let src = self.data.as_ptr().add(self.cursor as usize);
                std::ptr::copy_nonoverlapping(src, buffer as *mut u8, count);
                bytes_read = count as i32;
                let data_ptr = self as *const _ as *mut MemoryStream;
                (*data_ptr).cursor = (self.cursor + count as i64).max(0);
            }
        }
        if !num_bytes_read.is_null() {
            *num_bytes_read = bytes_read;
        }
        kResultOk
    }

    unsafe fn write(
        &self,
        buffer: *mut std::ffi::c_void,
        num_bytes: int32,
        num_bytes_written: *mut int32,
    ) -> tresult {
        if buffer.is_null() {
            return kResultFalse;
        }
        if num_bytes <= 0 {
            if !num_bytes_written.is_null() {
                *num_bytes_written = 0;
            }
            return kResultOk;
        }

        let mut cursor = self.cursor.max(0) as usize;
        let count = num_bytes as usize;
        let required = cursor.saturating_add(count);
        if required > self.data.len() {
            let mut data = self.data.clone();
            data.resize(required, 0);
            let src = buffer as *const u8;
            std::ptr::copy_nonoverlapping(src, data.as_mut_ptr().add(cursor), count);
            let data_ptr = self as *const _ as *mut MemoryStream;
            (*data_ptr).data = data;
        } else {
            let src = buffer as *const u8;
            let data_ptr = self as *const _ as *mut MemoryStream;
            std::ptr::copy_nonoverlapping(
                src,
                (*data_ptr).data.as_mut_ptr().add(cursor),
                count,
            );
        }
        cursor += count;
        let data_ptr = self as *const _ as *mut MemoryStream;
        (*data_ptr).cursor = cursor as i64;

        if !num_bytes_written.is_null() {
            *num_bytes_written = count as i32;
        }
        kResultOk
    }

    unsafe fn seek(&self, pos: int64, mode: int32, result: *mut int64) -> tresult {
        let len = self.data.len() as i64;
        let current = self.cursor;
        let next = match mode {
            mode if mode == IStreamSeekMode_::kIBSeekSet as i32 => pos,
            mode if mode == IStreamSeekMode_::kIBSeekCur as i32 => current.saturating_add(pos),
            mode if mode == IStreamSeekMode_::kIBSeekEnd as i32 => len.saturating_add(pos),
            _ => return kResultFalse,
        };
        let next = next.clamp(0, len);
        let data_ptr = self as *const _ as *mut MemoryStream;
        (*data_ptr).cursor = next;
        if !result.is_null() {
            *result = next;
        }
        kResultOk
    }

    unsafe fn tell(&self, pos: *mut int64) -> tresult {
        if pos.is_null() {
            return kResultFalse;
        }
        *pos = self.cursor;
        kResultOk
    }
}

impl Class for MemoryStream {
    type Interfaces = (IBStream,);
}

#[derive(Clone, Copy)]
pub enum MidiEvent {
    NoteOn {
        channel: u8,
        note: u8,
        velocity: u8,
        sample_offset: i32,
    },
    NoteOff {
        channel: u8,
        note: u8,
        velocity: u8,
        sample_offset: i32,
    },
    ControlChange {
        channel: u8,
        controller: u8,
        value: u8,
    },
}

impl MidiEvent {
    pub fn note_on(channel: u8, note: u8, velocity: u8) -> Self {
        Self::NoteOn {
            channel,
            note,
            velocity,
            sample_offset: 0,
        }
    }

    pub fn note_on_at(channel: u8, note: u8, velocity: u8, sample_offset: i32) -> Self {
        Self::NoteOn {
            channel,
            note,
            velocity,
            sample_offset,
        }
    }

    pub fn note_off(channel: u8, note: u8, velocity: u8) -> Self {
        Self::NoteOff {
            channel,
            note,
            velocity,
            sample_offset: 0,
        }
    }

    pub fn note_off_at(channel: u8, note: u8, velocity: u8, sample_offset: i32) -> Self {
        Self::NoteOff {
            channel,
            note,
            velocity,
            sample_offset,
        }
    }

    pub fn control_change(channel: u8, controller: u8, value: u8) -> Self {
        Self::ControlChange {
            channel,
            controller,
            value,
        }
    }
}

struct EventList {
    events: Mutex<Vec<Event>>,
}

impl EventList {
    fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
        }
    }

    fn set_events(&self, events: Vec<Event>) {
        if let Ok(mut guard) = self.events.lock() {
            *guard = events;
        }
    }
}

impl IEventListTrait for EventList {
    unsafe fn getEventCount(&self) -> i32 {
        self.events.lock().map(|g| g.len() as i32).unwrap_or(0)
    }

    unsafe fn getEvent(&self, index: i32, e: *mut Event) -> tresult {
        if e.is_null() {
            return kResultFalse;
        }
        let guard = match self.events.lock() {
            Ok(guard) => guard,
            Err(_) => return kResultFalse,
        };
        if let Some(event) = guard.get(index as usize) {
            *e = *event;
            kResultOk
        } else {
            kResultFalse
        }
    }

    unsafe fn addEvent(&self, e: *mut Event) -> tresult {
        if e.is_null() {
            return kResultFalse;
        }
        let mut guard = match self.events.lock() {
            Ok(guard) => guard,
            Err(_) => return kResultFalse,
        };
        guard.push(*e);
        kResultOk
    }
}

impl Class for EventList {
    type Interfaces = (IEventList,);
}

struct HostApplication {
    name: String,
}

impl HostApplication {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
        }
    }
}

impl IHostApplicationTrait for HostApplication {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        if name.is_null() {
            return kResultFalse;
        }
        let target = &mut *name;
        for slot in target.iter_mut() {
            *slot = 0;
        }
        for (index, ch) in self.name.encode_utf16().take(127).enumerate() {
            target[index] = ch;
        }
        kResultOk
    }

    unsafe fn createInstance(
        &self,
        _cid: *mut TUID,
        iid: *mut TUID,
        obj: *mut *mut std::ffi::c_void,
    ) -> tresult {
        if !obj.is_null() {
            *obj = std::ptr::null_mut();
        }
        if iid.is_null() || obj.is_null() {
            return kResultFalse;
        }
        let cid_matches_message = !_cid.is_null() && *_cid == IMessage_iid;
        let iid_matches_message = *iid == IMessage_iid;
        if cid_matches_message || iid_matches_message {
            let wrapper = Box::new(ComWrapper::new(HostMessage::new()));
            let ptr = wrapper
                .to_com_ptr::<IMessage>()
                .map(|ptr| ptr.into_raw())
                .unwrap_or(std::ptr::null_mut());
            if !ptr.is_null() {
                *obj = ptr as *mut std::ffi::c_void;
                std::mem::forget(wrapper);
                return kResultOk;
            }
        }
        let cid_matches_attr = !_cid.is_null() && *_cid == IAttributeList_iid;
        let iid_matches_attr = *iid == IAttributeList_iid;
        if cid_matches_attr || iid_matches_attr {
            let wrapper = Box::new(ComWrapper::new(AttributeList::new()));
            let ptr = wrapper
                .to_com_ptr::<IAttributeList>()
                .map(|ptr| ptr.into_raw())
                .unwrap_or(std::ptr::null_mut());
            if !ptr.is_null() {
                *obj = ptr as *mut std::ffi::c_void;
                std::mem::forget(wrapper);
                return kResultOk;
            }
        }
        let cid_matches_cp = !_cid.is_null() && *_cid == IConnectionPoint_iid;
        let iid_matches_cp = *iid == IConnectionPoint_iid;
        if cid_matches_cp || iid_matches_cp {
            let wrapper = Box::new(ComWrapper::new(HostConnectionPoint::new()));
            let ptr = wrapper
                .to_com_ptr::<IConnectionPoint>()
                .map(|ptr| ptr.into_raw())
                .unwrap_or(std::ptr::null_mut());
            if !ptr.is_null() {
                *obj = ptr as *mut std::ffi::c_void;
                std::mem::forget(wrapper);
                return kResultOk;
            }
        }
        kNotImplemented
    }
}

impl IPlugInterfaceSupportTrait for HostApplication {
    unsafe fn isPlugInterfaceSupported(&self, iid: *const TUID) -> tresult {
        if iid.is_null() {
            return kResultFalse;
        }
        let id = *iid;
        if id == IComponentHandler_iid
            || id == IPlugInterfaceSupport_iid
            || id == IMessage_iid
            || id == IAttributeList_iid
            || id == IConnectionPoint_iid
            || id == IPlugFrame_iid
        {
            return kResultTrue;
        }
        kResultFalse
    }
}

impl Class for HostApplication {
    type Interfaces = (IHostApplication, IPlugInterfaceSupport);
}

pub struct Vst3Host {
    _lib: Library,
    _deinit_module: Option<unsafe extern "C" fn() -> bool>,
    _host_app: ComWrapper<HostApplication>,
    pub plugin_path: String,
    component: ComPtr<IComponent>,
    processor: ComPtr<IAudioProcessor>,
    controller: Option<ComPtr<IEditController>>,
    midi_mapping: Option<ComPtr<IMidiMapping>>,
    param_cc_map: HashMap<ParamID, (u8, u8)>,
    _component_handler: Option<ComWrapper<ComponentHandler>>,
    _host_cp_to_controller: Option<ComWrapper<HostConnectionPoint>>,
    _host_cp_to_component: Option<ComWrapper<HostConnectionPoint>>,
    event_list: ComWrapper<EventList>,
    event_list_ptr: ComPtr<IEventList>,
    pending_param_changes: Arc<Mutex<Vec<(ParamID, ParamValue)>>>,
    last_param_change: Arc<Mutex<Option<(ParamID, ParamValue)>>>,
    last_process_param_count: AtomicUsize,
    process_context: ProcessContext,
    input_buffers: Vec<Vec<f32>>,
    input_ptrs: Vec<*mut f32>,
    input_channels: usize,
    output_buffers: Vec<Vec<f32>>,
    output_ptrs: Vec<*mut f32>,
    output_channels: usize,
}

pub struct Vst3Editor {
    view: ComPtr<IPlugView>,
    attached: bool,
    frame: Option<ComWrapper<PlugFrame>>,
}

struct PlugFrame;

impl IPlugFrameTrait for PlugFrame {
    unsafe fn resizeView(&self, view: *mut IPlugView, new_size: *mut ViewRect) -> tresult {
        if view.is_null() || new_size.is_null() {
            return kResultFalse;
        }
        let _ = ((*(*view).vtbl).onSize)(view, new_size);
        kResultOk
    }
}

impl Class for PlugFrame {
    type Interfaces = (IPlugFrame,);
}

// Safety: access is serialized through the audio-thread mutex.
unsafe impl Send for Vst3Host {}
unsafe impl Sync for Vst3Host {}

impl Vst3Host {
    pub fn io_channels(&self) -> (usize, usize) {
        (self.input_channels, self.output_channels)
    }

    pub fn latency_samples(&self) -> u32 {
        unsafe { self.processor.getLatencySamples() }
    }
    pub fn load(
        plugin_path: &str,
        sample_rate: f64,
        max_block_size: usize,
        channels: usize,
    ) -> Result<Self, String> {
        Self::load_with_input(plugin_path, sample_rate, max_block_size, channels, 0)
    }

    pub fn load_with_input(
        plugin_path: &str,
        sample_rate: f64,
        max_block_size: usize,
        channels: usize,
        input_channels: usize,
    ) -> Result<Self, String> {
        init_windows_com_for_thread();
        let module_path = resolve_vst3_binary(plugin_path)?;
        eprintln!("VST3 load: {plugin_path}");
        unsafe {
            let lib = Library::new(&module_path).map_err(|e| e.to_string())?;
            let init_module: Option<unsafe extern "C" fn() -> bool> = lib
                .get(b"InitModule")
                .ok()
                .map(|s: libloading::Symbol<unsafe extern "C" fn() -> bool>| *s);
            let deinit_module: Option<unsafe extern "C" fn() -> bool> = lib
                .get(b"DeinitModule")
                .ok()
                .map(|s: libloading::Symbol<unsafe extern "C" fn() -> bool>| *s);
            if let Some(init) = init_module {
                let ok = init();
                eprintln!("VST3 InitModule -> {ok}");
            }
            let get_factory: libloading::Symbol<GetPluginFactoryFn> = lib
                .get(b"GetPluginFactory")
                .map_err(|e| e.to_string())?;
            let factory = get_factory();
            if factory.is_null() {
                return Err("GetPluginFactory returned null".to_string());
            }
            eprintln!("VST3 factory ok");

            let host_app = ComWrapper::new(HostApplication::new("LingStation"));
            let host_ptr = host_app
                .to_com_ptr::<IHostApplication>()
                .ok_or_else(|| "Host application unavailable".to_string())?;

            let mut factory3_ptr: *mut IPluginFactory3 = std::ptr::null_mut();
            let qi_result = ((*(*factory).vtbl).base.queryInterface)(
                factory as *mut FUnknown,
                &IPluginFactory3_iid as *const TUID,
                &mut factory3_ptr as *mut _ as *mut *mut std::ffi::c_void,
            );
            if qi_result == kResultOk && !factory3_ptr.is_null() {
                if let Some(factory3) = ComPtr::from_raw(factory3_ptr) {
                    let result = factory3.setHostContext(host_ptr.as_ptr() as *mut FUnknown);
                    eprintln!("VST3 setHostContext -> {result}");
                }
            }

            let mut component_ptr: *mut IComponent = std::ptr::null_mut();
            let count = ((*(*factory).vtbl).countClasses)(factory);
            for index in 0..count {
                let mut class_info: PClassInfo = std::mem::zeroed();
                let result = ((*(*factory).vtbl).getClassInfo)(factory, index, &mut class_info);
                if result != kResultOk {
                    continue;
                }
                let category = cstr_to_string(class_info.category.as_ptr());
                let name = cstr_to_string(class_info.name.as_ptr());
                if category != "Audio Module Class" {
                    continue;
                }
                eprintln!("VST3 create component class index {index} name={name}");
                let result = ((*(*factory).vtbl).createInstance)(
                    factory,
                    class_info.cid.as_ptr(),
                    IComponent_iid.as_ptr(),
                    &mut component_ptr as *mut _ as *mut *mut std::ffi::c_void,
                );
                if result == kResultOk && !component_ptr.is_null() {
                    break;
                }
            }
            let component = ComPtr::from_raw(component_ptr)
                .ok_or_else(|| "No VST3 component created".to_string())?;
            eprintln!("VST3 component created");

            let plugin_base = component.as_ptr() as *mut IPluginBase;
            let init_result = ((*(*plugin_base).vtbl).initialize)(
                plugin_base,
                host_ptr.as_ptr() as *mut FUnknown,
            );
            if init_result != kResultOk {
                return Err("VST3 initialize failed".to_string());
            }
            eprintln!("VST3 component initialize ok");

            let _ = component.setIoMode(IoModes_::kAdvanced as i32);

            let mut controller = None;
            let mut controller_cid: TUID = [0; 16];
            let controller_result = component.getControllerClassId(&mut controller_cid);
            if controller_result == kResultOk {
                let mut controller_ptr: *mut IEditController = std::ptr::null_mut();
                let result = ((*(*factory).vtbl).createInstance)(
                    factory,
                    controller_cid.as_ptr(),
                    IEditController_iid.as_ptr(),
                    &mut controller_ptr as *mut _ as *mut *mut std::ffi::c_void,
                );
                if result == kResultOk {
                    if let Some(controller_created) = ComPtr::from_raw(controller_ptr) {
                        let controller_base = controller_created.as_ptr() as *mut IPluginBase;
                        let _ = ((*(*controller_base).vtbl).initialize)(
                            controller_base,
                            host_ptr.as_ptr() as *mut FUnknown,
                        );
                        let count_params =
                            ((*(*controller_created.as_ptr()).vtbl).getParameterCount)(
                                controller_created.as_ptr(),
                            );
                        if count_params > 0 {
                            controller = Some(controller_created);
                        }
                    }
                }
            }
            if controller.is_none() {
                controller = component.cast::<IEditController>();
            }
            eprintln!("VST3 controller ok: {}", controller.is_some());

            let mut host_cp_to_controller: Option<ComWrapper<HostConnectionPoint>> = None;
            let mut host_cp_to_component: Option<ComWrapper<HostConnectionPoint>> = None;
            if let Some(controller) = controller.as_ref() {
                let component_cp = component.cast::<IConnectionPoint>();
                let controller_cp = controller.cast::<IConnectionPoint>();
                if let (Some(component_cp), Some(controller_cp)) = (component_cp, controller_cp) {
                    let host_to_controller = ComWrapper::new(HostConnectionPoint::new());
                    let host_to_component = ComWrapper::new(HostConnectionPoint::new());
                    if let Some(host_to_controller_ptr) = host_to_controller.to_com_ptr::<IConnectionPoint>() {
                        let _ = component_cp.connect(host_to_controller_ptr.as_ptr());
                        let _ = host_to_controller_ptr.connect(controller_cp.as_ptr());
                    }
                    if let Some(host_to_component_ptr) = host_to_component.to_com_ptr::<IConnectionPoint>() {
                        let _ = controller_cp.connect(host_to_component_ptr.as_ptr());
                        let _ = host_to_component_ptr.connect(component_cp.as_ptr());
                    }
                    host_cp_to_controller = Some(host_to_controller);
                    host_cp_to_component = Some(host_to_component);
                    eprintln!("VST3 connection point bridge ok");
                }

                let stream = ComWrapper::new(MemoryStream::new());
                if let Some(stream_ptr) = stream.to_com_ptr::<IBStream>() {
                    let get_state_result = component.getState(stream_ptr.as_ptr());
                    eprintln!("VST3 component getState -> {get_state_result}");
                    if get_state_result == kResultOk {
                        let bytes = stream.bytes();
                        if !bytes.is_empty() {
                            let _ = stream_ptr.seek(
                                0,
                                IStreamSeekMode_::kIBSeekSet as i32,
                                std::ptr::null_mut(),
                            );
                            let _ = controller.setComponentState(stream_ptr.as_ptr());
                            eprintln!("VST3 controller setComponentState ok");
                        }
                    }
                }
            }

            let midi_mapping = controller
                .as_ref()
                .and_then(|controller| controller.cast::<IMidiMapping>());
            let param_cc_map = if let Some(mapping) = midi_mapping.as_ref() {
                build_param_cc_map(mapping)
            } else {
                HashMap::new()
            };

            let last_param_change = Arc::new(Mutex::new(None));
            let pending_param_changes = Arc::new(Mutex::new(Vec::new()));
            let component_handler = controller.as_ref().and_then(|controller| {
                let handler = ComWrapper::new(ComponentHandler {
                    last_param_change: last_param_change.clone(),
                    pending_param_changes: pending_param_changes.clone(),
                });
                let handler_ptr = handler
                    .to_com_ptr::<IComponentHandler>()
                    .map(|ptr| ptr.as_ptr())
                    .unwrap_or(std::ptr::null_mut());
                if handler_ptr.is_null() {
                    return None;
                }
                let result = controller.setComponentHandler(handler_ptr);
                if result == kResultOk {
                    Some(handler)
                } else {
                    None
                }
            });

            let processor = component
                .cast::<IAudioProcessor>()
                .ok_or_else(|| "VST3 has no audio processor".to_string())?;
            eprintln!("VST3 audio processor ok");

            let _ = component.setIoMode(IoModes_::kAdvanced as i32);

            let audio_in_count = component.getBusCount(
                MediaTypes_::kAudio as i32,
                BusDirections_::kInput as i32,
            );
            let audio_out_count = component.getBusCount(
                MediaTypes_::kAudio as i32,
                BusDirections_::kOutput as i32,
            );
            if audio_out_count <= 0 {
                return Err("VST3 has no audio output buses".to_string());
            }
            if audio_out_count > 1 {
                return Err("VST3 multiple output buses not supported".to_string());
            }
            if audio_in_count > 1 {
                return Err("VST3 multiple input buses not supported".to_string());
            }
            let mut output_channels = if channels <= 1 { 1 } else { 2 };
            let mut input_channels = if input_channels == 0 {
                0
            } else if input_channels <= 1 {
                1
            } else {
                2
            };
            if audio_in_count == 0 {
                input_channels = 0;
            }
            let mut output_arrangement = if output_channels == 1 {
                SpeakerArr::kMono
            } else {
                SpeakerArr::kStereo
            };
            let mut input_arrangement = if input_channels == 1 {
                SpeakerArr::kMono
            } else {
                SpeakerArr::kStereo
            };
            let mut bus_result = if input_channels == 0 {
                processor.setBusArrangements(
                    std::ptr::null_mut(),
                    0,
                    &mut output_arrangement as *mut _,
                    1,
                )
            } else {
                processor.setBusArrangements(
                    &mut input_arrangement as *mut _,
                    1,
                    &mut output_arrangement as *mut _,
                    1,
                )
            };
            if bus_result != kResultOk {
                output_arrangement = if output_channels == 1 {
                    SpeakerArr::kStereo
                } else {
                    SpeakerArr::kMono
                };
                input_arrangement = if input_channels == 1 {
                    SpeakerArr::kStereo
                } else {
                    SpeakerArr::kMono
                };
                bus_result = if input_channels == 0 {
                    processor.setBusArrangements(
                        std::ptr::null_mut(),
                        0,
                        &mut output_arrangement as *mut _,
                        1,
                    )
                } else {
                    processor.setBusArrangements(
                        &mut input_arrangement as *mut _,
                        1,
                        &mut output_arrangement as *mut _,
                        1,
                    )
                };
            }
            if bus_result != kResultOk {
                eprintln!("VST3 setBusArrangements failed: {bus_result}");
            } else {
                eprintln!("VST3 bus arrangement result: {bus_result}");
            }
            if audio_out_count > 0 {
                let _ = component.activateBus(
                    MediaTypes_::kAudio as i32,
                    BusDirections_::kOutput as i32,
                    0,
                    1,
                );
            }
            if input_channels > 0 && audio_in_count > 0 {
                let _ = component.activateBus(
                    MediaTypes_::kAudio as i32,
                    BusDirections_::kInput as i32,
                    0,
                    1,
                );
            }
            let event_in_count = component.getBusCount(
                MediaTypes_::kEvent as i32,
                BusDirections_::kInput as i32,
            );
            if event_in_count > 0 {
                let _ = component.activateBus(
                    MediaTypes_::kEvent as i32,
                    BusDirections_::kInput as i32,
                    0,
                    1,
                );
            }

            let mut bus_info: vst3::Steinberg::Vst::BusInfo = std::mem::zeroed();
            let bus_info_result = component.getBusInfo(
                MediaTypes_::kAudio as i32,
                BusDirections_::kOutput as i32,
                0,
                &mut bus_info as *mut _,
            );
            if bus_info_result == kResultOk {
                let count = bus_info.channelCount as usize;
                if count == 1 || count == 2 {
                    output_channels = count;
                }
            }
            let in_bus_info_result = component.getBusInfo(
                MediaTypes_::kAudio as i32,
                BusDirections_::kInput as i32,
                0,
                &mut bus_info as *mut _,
            );
            if in_bus_info_result == kResultOk {
                let count = bus_info.channelCount as usize;
                if count == 1 || count == 2 {
                    input_channels = count;
                }
            } else if audio_in_count == 0 {
                input_channels = 0;
            }

            let active_result = component.setActive(1);
            eprintln!("VST3 setActive -> {active_result}");

            let mut setup = ProcessSetup {
                processMode: ProcessModes_::kRealtime as i32,
                symbolicSampleSize: SymbolicSampleSizes_::kSample32 as i32,
                maxSamplesPerBlock: max_block_size as i32,
                sampleRate: sample_rate,
            };
            let setup_result = processor.setupProcessing(&mut setup as *mut _);
            if setup_result != kResultOk {
                return Err("VST3 setupProcessing failed".to_string());
            }
            eprintln!("VST3 setupProcessing ok");
            if active_result != kResultOk {
                let _ = component.setActive(1);
            }
            let processing_result = processor.setProcessing(1);
            if processing_result != kResultOk {
                return Err("VST3 setProcessing failed".to_string());
            }
            eprintln!("VST3 setProcessing ok");

            let event_list = ComWrapper::new(EventList::new());
            let event_list_ptr = event_list
                .to_com_ptr::<IEventList>()
                .ok_or_else(|| "Event list creation failed".to_string())?;

            Ok(Self {
                _lib: lib,
                _deinit_module: deinit_module,
                _host_app: host_app,
                plugin_path: plugin_path.to_string(),
                component,
                processor,
                controller,
                midi_mapping,
                param_cc_map,
                _component_handler: component_handler,
                _host_cp_to_controller: host_cp_to_controller,
                _host_cp_to_component: host_cp_to_component,
                event_list,
                event_list_ptr,
                pending_param_changes,
                last_param_change,
                last_process_param_count: AtomicUsize::new(0),
                process_context: std::mem::zeroed(),
                input_buffers: Vec::new(),
                input_ptrs: Vec::new(),
                input_channels,
                output_buffers: Vec::new(),
                output_ptrs: Vec::new(),
                output_channels,
            })
        }
    }

    pub fn push_param_change(&mut self, param_id: u32, value: f64) {
        if let Ok(mut last) = self.last_param_change.lock() {
            *last = Some((param_id, value));
        }
        if let Some(controller) = self.controller.as_ref() {
            let _ = unsafe { controller.setParamNormalized(param_id, value) };
        }
        if let Ok(mut changes) = self.pending_param_changes.lock() {
            changes.push((param_id, value));
        }
    }

    pub fn take_last_param_change(&mut self) -> Option<(ParamID, ParamValue)> {
        if let Ok(mut last) = self.last_param_change.lock() {
            last.take()
        } else {
            None
        }
    }

    pub fn prepare_for_drop(&mut self) {
        unsafe {
            let _ = self.processor.setProcessing(0);
            let _ = self.component.setActive(0);
        }
    }

    pub fn enumerate_params(&self) -> Vec<ParamInfo> {
        let Some(controller) = self.controller.as_ref() else {
            return Vec::new();
        };
        let count_params = unsafe { controller.getParameterCount() };
        if count_params <= 0 {
            return Vec::new();
        }
        let mut params = Vec::with_capacity(count_params as usize);
        for i in 0..count_params {
            let mut info: ParameterInfo = unsafe { std::mem::zeroed() };
            let res = unsafe { controller.getParameterInfo(i, &mut info) };
            if res != kResultOk {
                continue;
            }
            let name = string128_to_string(&info.title);
            if !name.trim().is_empty() {
                params.push(ParamInfo {
                    id: info.id,
                    name,
                    default_value: info.defaultNormalizedValue,
                });
            }
        }
        params
    }

    pub fn get_state_bytes(&self) -> (Vec<u8>, Vec<u8>) {
        let mut component_bytes = Vec::new();
        let mut controller_bytes = Vec::new();
        let stream = ComWrapper::new(MemoryStream::new());
        if let Some(stream_ptr) = stream.to_com_ptr::<IBStream>() {
            let _ = unsafe { self.component.getState(stream_ptr.as_ptr()) };
            component_bytes = stream.bytes();
        }
        if let Some(controller) = self.controller.as_ref() {
            let stream = ComWrapper::new(MemoryStream::new());
            if let Some(stream_ptr) = stream.to_com_ptr::<IBStream>() {
                let _ = unsafe { controller.getState(stream_ptr.as_ptr()) };
                controller_bytes = stream.bytes();
            }
        }
        (component_bytes, controller_bytes)
    }

    pub fn set_state_bytes(
        &mut self,
        component_state: Option<&[u8]>,
        controller_state: Option<&[u8]>,
    ) -> Result<(), String> {
        if let Some(bytes) = component_state {
            if !bytes.is_empty() {
                let stream = ComWrapper::new(MemoryStream::from_bytes(bytes));
                if let Some(stream_ptr) = stream.to_com_ptr::<IBStream>() {
                    let _ = unsafe { self.component.setState(stream_ptr.as_ptr()) };
                    if let Some(controller) = self.controller.as_ref() {
                        let _ = unsafe {
                            stream_ptr.seek(
                                0,
                                IStreamSeekMode_::kIBSeekSet as i32,
                                std::ptr::null_mut(),
                            )
                        };
                        let _ = unsafe { controller.setComponentState(stream_ptr.as_ptr()) };
                    }
                }
            }
        }
        if let Some(bytes) = controller_state {
            if let Some(controller) = self.controller.as_ref() {
                if !bytes.is_empty() {
                    let stream = ComWrapper::new(MemoryStream::from_bytes(bytes));
                    if let Some(stream_ptr) = stream.to_com_ptr::<IBStream>() {
                        let _ = unsafe { controller.setState(stream_ptr.as_ptr()) };
                    }
                }
            }
        }
        Ok(())
    }

    pub fn apply_state_for_render(
        &mut self,
        component_state: Option<&[u8]>,
        controller_state: Option<&[u8]>,
    ) -> Result<(), String> {
        let _ = unsafe { self.processor.setProcessing(0) };
        let result = self.set_state_bytes(component_state, controller_state);
        let _ = unsafe { self.processor.setProcessing(1) };
        result
    }

    pub fn process_f32(
        &mut self,
        output: &mut [f32],
        channels: usize,
        midi_events: &[MidiEvent],
    ) -> Result<(), String> {
        if channels == 0 {
            return Ok(());
        }
        let frames = output.len() / channels;
        if frames == 0 {
            return Ok(());
        }
        self.ensure_buffers(frames);

        let mut events = Vec::with_capacity(midi_events.len());
        for event in midi_events {
            match *event {
                MidiEvent::ControlChange {
                    channel,
                    controller,
                    value,
                } => {
                    if let Some((param_id, param_value)) =
                        self.map_cc_to_param(channel, controller, value)
                    {
                        self.push_param_change(param_id, param_value);
                    }
                }
                _ => {
                    if let Some(vst_event) = midi_event_to_vst(*event) {
                        events.push(vst_event);
                    }
                }
            }
        }
        self.event_list.set_events(events);

        let mut output_bus = AudioBusBuffers {
            numChannels: self.output_channels as i32,
            silenceFlags: 0,
            __field0: AudioBusBuffers__type0 {
                channelBuffers32: self.output_ptrs.as_mut_ptr(),
            },
        };
        let mut _param_changes = None;
        let mut param_changes_ptr = std::ptr::null_mut();
        self.last_process_param_count.store(0, Ordering::Relaxed);
        if let Ok(mut pending) = self.pending_param_changes.lock() {
            if !pending.is_empty() {
                let changes = std::mem::take(&mut *pending);
                self.last_process_param_count
                    .store(changes.len(), Ordering::Relaxed);
                if let Some(controller) = self.controller.as_ref() {
                    for (param_id, value) in &changes {
                        let _ = unsafe { controller.setParamNormalized(*param_id, *value) };
                    }
                }
                let wrapper = ComWrapper::new(ParameterChanges::from_changes(changes));
                param_changes_ptr = wrapper
                    .to_com_ptr::<IParameterChanges>()
                    .map(|ptr| ptr.as_ptr())
                    .unwrap_or(std::ptr::null_mut());
                _param_changes = Some(wrapper);
            }
        }
            let mut process_data = ProcessData {
            processMode: ProcessModes_::kRealtime as i32,
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as i32,
            numSamples: frames as i32,
            numInputs: 0,
            numOutputs: 1,
            inputs: std::ptr::null_mut(),
            outputs: &mut output_bus as *mut _,
            inputParameterChanges: param_changes_ptr,
            outputParameterChanges: std::ptr::null_mut(),
            inputEvents: self.event_list_ptr.as_ptr(),
            outputEvents: std::ptr::null_mut(),
            processContext: std::ptr::null_mut(),
        };

        let result = unsafe { self.processor.process(&mut process_data as *mut _) };
        if result != kResultOk {
            return Err("VST3 process failed".to_string());
        }

        for frame in 0..frames {
            let base = frame * channels;
            if channels == 1 && self.output_channels >= 2 {
                let left = self.output_buffers[0][frame];
                let right = self.output_buffers[1][frame];
                output[base] = (left + right) * 0.5;
                continue;
            }
            for ch in 0..channels {
                let sample = if self.output_channels == 1 {
                    self.output_buffers[0][frame]
                } else if ch < self.output_channels {
                    self.output_buffers[ch][frame]
                } else {
                    0.0
                };
                output[base + ch] = sample;
            }
        }

        Ok(())
    }

    pub fn process_f32_with_input(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        channels: usize,
        midi_events: &[MidiEvent],
    ) -> Result<(), String> {
        if channels == 0 {
            return Ok(());
        }
        let frames = output.len() / channels;
        if frames == 0 {
            return Ok(());
        }
        if input.len() < frames * channels {
            return Err("VST3 input buffer too small".to_string());
        }
        if self.input_channels == 0 {
            output[..frames * channels].copy_from_slice(&input[..frames * channels]);
            return Ok(());
        }

        self.ensure_buffers(frames);
        self.ensure_input_buffers(frames);

        for frame in 0..frames {
            let base = frame * channels;
            let left = input[base];
            let right = if channels > 1 { input[base + 1] } else { left };
            if self.input_channels == 1 {
                self.input_buffers[0][frame] = (left + right) * 0.5;
            } else {
                self.input_buffers[0][frame] = left;
                if self.input_channels > 1 {
                    self.input_buffers[1][frame] = right;
                }
            }
        }

        let mut events = Vec::with_capacity(midi_events.len());
        for event in midi_events {
            match *event {
                MidiEvent::ControlChange {
                    channel,
                    controller,
                    value,
                } => {
                    if let Some((param_id, param_value)) =
                        self.map_cc_to_param(channel, controller, value)
                    {
                        self.push_param_change(param_id, param_value);
                    }
                }
                _ => {
                    if let Some(vst_event) = midi_event_to_vst(*event) {
                        events.push(vst_event);
                    }
                }
            }
        }
        self.event_list.set_events(events);

        let mut input_bus = AudioBusBuffers {
            numChannels: self.input_channels as i32,
            silenceFlags: 0,
            __field0: AudioBusBuffers__type0 {
                channelBuffers32: self.input_ptrs.as_mut_ptr(),
            },
        };
        let mut output_bus = AudioBusBuffers {
            numChannels: self.output_channels as i32,
            silenceFlags: 0,
            __field0: AudioBusBuffers__type0 {
                channelBuffers32: self.output_ptrs.as_mut_ptr(),
            },
        };
        let mut _param_changes = None;
        let mut param_changes_ptr = std::ptr::null_mut();
        self.last_process_param_count.store(0, Ordering::Relaxed);
        if let Ok(mut pending) = self.pending_param_changes.lock() {
            if !pending.is_empty() {
                let changes = std::mem::take(&mut *pending);
                self.last_process_param_count
                    .store(changes.len(), Ordering::Relaxed);
                if let Some(controller) = self.controller.as_ref() {
                    for (param_id, value) in &changes {
                        let _ = unsafe { controller.setParamNormalized(*param_id, *value) };
                    }
                }
                let wrapper = ComWrapper::new(ParameterChanges::from_changes(changes));
                param_changes_ptr = wrapper
                    .to_com_ptr::<IParameterChanges>()
                    .map(|ptr| ptr.as_ptr())
                    .unwrap_or(std::ptr::null_mut());
                _param_changes = Some(wrapper);
            }
        }
        let mut process_data = ProcessData {
            processMode: ProcessModes_::kRealtime as i32,
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as i32,
            numSamples: frames as i32,
            numInputs: 1,
            numOutputs: 1,
            inputs: &mut input_bus as *mut _,
            outputs: &mut output_bus as *mut _,
            inputParameterChanges: param_changes_ptr,
            outputParameterChanges: std::ptr::null_mut(),
            inputEvents: self.event_list_ptr.as_ptr(),
            outputEvents: std::ptr::null_mut(),
            processContext: std::ptr::null_mut(),
        };

        let result = unsafe { self.processor.process(&mut process_data as *mut _) };
        if result != kResultOk {
            return Err("VST3 process failed".to_string());
        }

        for frame in 0..frames {
            let base = frame * channels;
            if channels == 1 && self.output_channels >= 2 {
                let left = self.output_buffers[0][frame];
                let right = self.output_buffers[1][frame];
                output[base] = (left + right) * 0.5;
                continue;
            }
            for ch in 0..channels {
                let sample = if self.output_channels == 1 {
                    self.output_buffers[0][frame]
                } else if ch < self.output_channels {
                    self.output_buffers[ch][frame]
                } else {
                    0.0
                };
                output[base + ch] = sample;
            }
        }

        Ok(())
    }

    pub fn debug_last_param_change(&self) -> Option<(ParamID, ParamValue)> {
        self.last_param_change.lock().ok().and_then(|v| *v)
    }

    pub fn debug_last_process_param_count(&self) -> usize {
        self.last_process_param_count.load(Ordering::Relaxed)
    }

    pub fn create_editor(&self) -> Option<Vst3Editor> {
        let controller = self.controller.as_ref()?;
        let view = unsafe { controller.createView(ViewType::kEditor) };
        if view.is_null() {
            eprintln!("VST3 createView returned null");
            return None;
        }
        let view = unsafe { ComPtr::from_raw(view)? };
        Some(Vst3Editor {
            view,
            attached: false,
            frame: None,
        })
    }
    fn ensure_buffers(&mut self, frames: usize) {
        if self.output_buffers.len() != self.output_channels {
            self.output_buffers = vec![vec![0.0; frames]; self.output_channels];
            self.output_ptrs = vec![std::ptr::null_mut(); self.output_channels];
        }
        for (index, buffer) in self.output_buffers.iter_mut().enumerate() {
            if buffer.len() != frames {
                buffer.resize(frames, 0.0);
            }
            buffer.fill(0.0);
            self.output_ptrs[index] = buffer.as_mut_ptr();
        }
    }

    fn ensure_input_buffers(&mut self, frames: usize) {
        if self.input_channels == 0 {
            return;
        }
        if self.input_buffers.len() != self.input_channels {
            self.input_buffers = vec![vec![0.0; frames]; self.input_channels];
            self.input_ptrs = vec![std::ptr::null_mut(); self.input_channels];
        }
        for (index, buffer) in self.input_buffers.iter_mut().enumerate() {
            if buffer.len() != frames {
                buffer.resize(frames, 0.0);
            }
            self.input_ptrs[index] = buffer.as_mut_ptr();
        }
    }

    fn map_cc_to_param(
        &self,
        channel: u8,
        controller_number: u8,
        value: u8,
    ) -> Option<(ParamID, ParamValue)> {
        let mapping = self.midi_mapping.as_ref()?;
        let mut param_id: ParamID = 0;
        let result = unsafe {
            mapping.getMidiControllerAssignment(
                0,
                channel as i16,
                controller_number as i16,
                &mut param_id as *mut _,
            )
        };
        if result == kResultOk || result == kResultTrue {
            let value = (value as f64 / 127.0).clamp(0.0, 1.0);
            Some((param_id, value))
        } else {
            None
        }
    }

    pub fn param_to_cc(&self, param_id: ParamID) -> Option<(u8, u8)> {
        self.param_cc_map.get(&param_id).copied()
    }
}

impl Drop for Vst3Host {
    fn drop(&mut self) {
        unsafe {
            let _ = self.processor.setProcessing(0);
            let _ = self.component.setActive(0);
            let plugin_base = self.component.as_ptr() as *mut IPluginBase;
            let _ = ((*(*plugin_base).vtbl).terminate)(plugin_base);
            if let Some(deinit) = self._deinit_module {
                let _ = deinit();
            }
        }
    }
}

impl Vst3Editor {
    pub fn attach_hwnd(&mut self, hwnd: isize) -> Result<(), String> {
        if self.attached {
            return Ok(());
        }
        let type_str = b"HWND\0";
        let supported = unsafe { self.view.isPlatformTypeSupported(type_str.as_ptr() as *const i8) };
        eprintln!("VST3 isPlatformTypeSupported(HWND) -> {supported}");
        if supported != kResultOk {
            return Err("VST3 view does not support HWND".to_string());
        }
        if self.frame.is_none() {
            let frame = ComWrapper::new(PlugFrame);
            let frame_ptr = frame
                .to_com_ptr::<IPlugFrame>()
                .map(|ptr| ptr.as_ptr())
                .unwrap_or(std::ptr::null_mut());
            if !frame_ptr.is_null() {
                let _ = unsafe { self.view.setFrame(frame_ptr) };
                self.frame = Some(frame);
            }
        }
        let result = unsafe {
            self.view
                .attached(hwnd as *mut _, type_str.as_ptr() as *const i8)
        };
        eprintln!("VST3 view attached(HWND) -> {result}");
        if result != kResultOk {
            return Err("VST3 view attach failed".to_string());
        }
        self.attached = true;
        Ok(())
    }

    pub fn set_focus(&self, focus: bool) {
        let _ = unsafe { self.view.onFocus(if focus { 1 } else { 0 }) };
    }

    pub fn removed(&mut self) {
        if self.attached {
            self.set_focus(false);
            let _ = unsafe { self.view.removed() };
            self.attached = false;
        }
    }

    pub fn set_size(&self, width: i32, height: i32) {
        let mut rect = ViewRect {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };
        let _ = unsafe { self.view.onSize(&mut rect) };
    }

    pub fn get_size(&self) -> Option<(i32, i32)> {
        let mut rect = ViewRect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let result = unsafe { self.view.getSize(&mut rect) };
        if result != kResultOk {
            return None;
        }
        let width = (rect.right - rect.left).max(0);
        let height = (rect.bottom - rect.top).max(0);
        Some((width, height))
    }
}

pub fn enumerate_params(plugin_path: &str) -> Result<Vec<ParamInfo>, String> {
    let module_path = resolve_vst3_binary(plugin_path)?;
    unsafe {
        let lib = Library::new(&module_path).map_err(|e| e.to_string())?;
        let get_factory: libloading::Symbol<GetPluginFactoryFn> = lib
            .get(b"GetPluginFactory")
            .map_err(|e| e.to_string())?;
        let factory = get_factory();
        if factory.is_null() {
            return Err("GetPluginFactory returned null".to_string());
        }

        let count = ((*(*factory).vtbl).countClasses)(factory);
        let mut params = Vec::new();
        for index in 0..count {
            let mut class_info: PClassInfo = std::mem::zeroed();
            let result = ((*(*factory).vtbl).getClassInfo)(factory, index, &mut class_info);
            if result != kResultOk {
                continue;
            }
            let category = cstr_to_string(class_info.category.as_ptr());
            if category != "Audio Module Class" {
                continue;
            }

            let mut component_ptr: *mut IComponent = std::ptr::null_mut();
            let result = ((*(*factory).vtbl).createInstance)(
                factory,
                class_info.cid.as_ptr(),
                IComponent_iid.as_ptr(),
                &mut component_ptr as *mut _ as *mut *mut std::ffi::c_void,
            );
            if result != kResultOk || component_ptr.is_null() {
                continue;
            }
            let component = match ComPtr::from_raw(component_ptr) {
                Some(component) => component,
                None => continue,
            };

            let host_app = ComWrapper::new(HostApplication::new("LingStation"));
            let host_ptr = match host_app.to_com_ptr::<IHostApplication>() {
                Some(ptr) => ptr,
                None => continue,
            };
            let component_base = component.as_ptr() as *mut IPluginBase;
            let _ = ((*(*component_base).vtbl).initialize)(
                component_base,
                host_ptr.as_ptr() as *mut FUnknown,
            );

            let mut controller_cid: TUID = [0; 16];
            let controller_result = component.getControllerClassId(&mut controller_cid);
            if controller_result == kResultOk {
                let mut controller_ptr: *mut IEditController = std::ptr::null_mut();
                let result = ((*(*factory).vtbl).createInstance)(
                    factory,
                    controller_cid.as_ptr(),
                    IEditController_iid.as_ptr(),
                    &mut controller_ptr as *mut _ as *mut *mut std::ffi::c_void,
                );
                if result == kResultOk && !controller_ptr.is_null() {
                    if let Some(controller) = ComPtr::from_raw(controller_ptr) {
                        let controller_base = controller.as_ptr() as *mut IPluginBase;
                        let _ = ((*(*controller_base).vtbl).initialize)(
                            controller_base,
                            host_ptr.as_ptr() as *mut FUnknown,
                        );
                        let count_params = ((*(*controller.as_ptr()).vtbl).getParameterCount)(controller.as_ptr());
                        for i in 0..count_params {
                            let mut info: ParameterInfo = std::mem::zeroed();
                            let res = ((*(*controller.as_ptr()).vtbl).getParameterInfo)(
                                controller.as_ptr(),
                                i,
                                &mut info,
                            );
                            if res != kResultOk {
                                continue;
                            }
                            let name = string128_to_string(&info.title);
                            if !name.trim().is_empty() {
                                params.push(ParamInfo {
                                    id: info.id,
                                    name,
                                    default_value: info.defaultNormalizedValue,
                                });
                            }
                        }
                        let _ = ((*(*controller_base).vtbl).terminate)(controller_base);
                    }
                }
            }

            if params.is_empty() {
                if let Some(controller) = component.cast::<IEditController>() {
                    let count_params = ((*(*controller.as_ptr()).vtbl).getParameterCount)(controller.as_ptr());
                    for i in 0..count_params {
                        let mut info: ParameterInfo = std::mem::zeroed();
                        let res = ((*(*controller.as_ptr()).vtbl).getParameterInfo)(
                            controller.as_ptr(),
                            i,
                            &mut info,
                        );
                        if res != kResultOk {
                            continue;
                        }
                        let name = string128_to_string(&info.title);
                        if !name.trim().is_empty() {
                            params.push(ParamInfo {
                                id: info.id,
                                name,
                                default_value: info.defaultNormalizedValue,
                            });
                        }
                    }
                }
            }

            let _ = ((*(*component_base).vtbl).terminate)(component_base);

            if params.is_empty() {
                let mut direct_controller: *mut IEditController = std::ptr::null_mut();
                let result = ((*(*factory).vtbl).createInstance)(
                    factory,
                    class_info.cid.as_ptr(),
                    IEditController_iid.as_ptr(),
                    &mut direct_controller as *mut _ as *mut *mut std::ffi::c_void,
                );
                if result == kResultOk && !direct_controller.is_null() {
                    let controller_base = direct_controller as *mut IPluginBase;
                    let _ = ((*(*controller_base).vtbl).initialize)(
                        controller_base,
                        host_ptr.as_ptr() as *mut FUnknown,
                    );
                    let count_params = ((*(*direct_controller).vtbl).getParameterCount)(direct_controller);
                    for i in 0..count_params {
                        let mut info: ParameterInfo = std::mem::zeroed();
                        let res = ((*(*direct_controller).vtbl).getParameterInfo)(
                            direct_controller,
                            i,
                            &mut info,
                        );
                        if res != kResultOk {
                            continue;
                        }
                        let name = string128_to_string(&info.title);
                        if !name.trim().is_empty() {
                            params.push(ParamInfo {
                                id: info.id,
                                name,
                                default_value: info.defaultNormalizedValue,
                            });
                        }
                    }
                    let _ = ((*(*controller_base).vtbl).terminate)(controller_base);
                }
            }

            if !params.is_empty() {
                break;
            }
        }

        if params.is_empty() {
            Err("No parameters found".to_string())
        } else {
            Ok(params)
        }
    }
}

fn cstr_to_string(ptr: *const i8) -> String {
    if ptr.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() }
}

fn midi_event_to_vst(event: MidiEvent) -> Option<Event> {
    match event {
        MidiEvent::NoteOn {
            channel,
            note,
            velocity,
            sample_offset,
        } => {
            let velocity = (velocity as f32 / 127.0).clamp(0.0, 1.0);
            Some(Event {
                busIndex: 0,
                sampleOffset: sample_offset,
                ppqPosition: 0.0,
                flags: 0,
                r#type: EventTypes_::kNoteOnEvent as u16,
                __field0: Event__type0 {
                    noteOn: NoteOnEvent {
                        channel: channel as i16,
                        pitch: note as i16,
                        tuning: 0.0,
                        velocity,
                        length: 0,
                        noteId: -1,
                    },
                },
            })
        }
        MidiEvent::NoteOff {
            channel,
            note,
            velocity,
            sample_offset,
        } => {
            let velocity = (velocity as f32 / 127.0).clamp(0.0, 1.0);
            Some(Event {
                busIndex: 0,
                sampleOffset: sample_offset,
                ppqPosition: 0.0,
                flags: 0,
                r#type: EventTypes_::kNoteOffEvent as u16,
                __field0: Event__type0 {
                    noteOff: NoteOffEvent {
                        channel: channel as i16,
                        pitch: note as i16,
                        velocity,
                        noteId: -1,
                        tuning: 0.0,
                    },
                },
            })
        }
        MidiEvent::ControlChange { .. } => None,
    }
}

fn build_param_cc_map(mapping: &ComPtr<IMidiMapping>) -> HashMap<ParamID, (u8, u8)> {
    let mut map = HashMap::new();
    for channel in 0..16 {
        for controller in 0..128 {
            let mut param_id: ParamID = 0;
            let result = unsafe {
                mapping.getMidiControllerAssignment(
                    0,
                    channel as i16,
                    controller as i16,
                    &mut param_id as *mut _,
                )
            };
            if result == kResultOk || result == kResultTrue {
                map.entry(param_id)
                    .or_insert((channel as u8, controller as u8));
            }
        }
    }
    map
}

fn resolve_vst3_binary(plugin_path: &str) -> Result<PathBuf, String> {
    let path = Path::new(plugin_path);
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    if path.is_dir() {
        let binary_root = path.join("Contents").join("x86_64-win");
        if binary_root.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&binary_root) {
                for entry in entries.flatten() {
                    let candidate = entry.path();
                    if candidate.extension().and_then(|e| e.to_str()) == Some("vst3") {
                        return Ok(candidate);
                    }
                }
            }
        }
    }
    Err(format!("VST3 binary not found at {plugin_path}"))
}

fn string128_to_string(value: &vst3::Steinberg::Vst::String128) -> String {
    let mut u16_buf = Vec::new();
    for ch in value.iter() {
        if *ch == 0 {
            break;
        }
        u16_buf.push(*ch as u16);
    }
    String::from_utf16(&u16_buf).unwrap_or_default()
}

fn attr_id_to_string(id: AttrID) -> Option<String> {
    if id.is_null() {
        return None;
    }
    unsafe { Some(CStr::from_ptr(id).to_string_lossy().into_owned()) }
}
