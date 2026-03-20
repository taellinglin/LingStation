use super::*;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(super) enum PerformanceTriggerMode {
    Gate,
    Toggle,
    OneShot,
    Loop,
}

impl Default for PerformanceTriggerMode {
    fn default() -> Self {
        Self::Gate
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub(super) struct PerformanceClipSettings {
    #[serde(default)]
    pub(super) trigger_mode: PerformanceTriggerMode,
    #[serde(default)]
    pub(super) loop_enabled: bool,
    #[serde(default)]
    pub(super) auto_follow: bool,
    #[serde(default)]
    pub(super) next_clip_id: Option<usize>,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(crate) struct PerformanceRuntimeClip {
    pub(super) track_index: usize,
    pub(super) launch_samples: u64,
    pub(super) clip: Clip,
    pub(super) loop_enabled: bool,
    pub(super) trigger_mode: PerformanceTriggerMode,
    pub(super) resolved_audio_path: Option<Arc<str>>,
}

pub(super) fn performance_clip_loop_beats(runtime: &PerformanceRuntimeClip) -> f32 {
    let clip = &runtime.clip;
    if clip.is_midi {
        clip.midi_source_beats.unwrap_or(clip.length_beats).max(0.25)
    } else {
        clip.audio_source_beats.unwrap_or(clip.length_beats).max(0.25)
    }
}

pub(super) fn performance_length_samples(
    runtime: &PerformanceRuntimeClip,
    samples_per_beat: f64,
) -> u64 {
    if runtime.loop_enabled {
        u64::MAX / 4
    } else {
        let beats = runtime.clip.length_beats.max(0.25) as f64;
        (beats * samples_per_beat).round().max(1.0) as u64
    }
}

#[allow(dead_code)]
pub(super) fn collect_performance_block_events(
    runtime: &PerformanceRuntimeClip,
    block_start: u64,
    block_end: u64,
    samples_per_beat: f64,
) -> Vec<vst3::MidiEvent> {
    let mut events = Vec::new();
    collect_performance_block_events_into(
        runtime,
        block_start,
        block_end,
        samples_per_beat,
        &mut events,
    );
    events
}

pub(super) fn collect_performance_block_events_into(
    runtime: &PerformanceRuntimeClip,
    block_start: u64,
    block_end: u64,
    samples_per_beat: f64,
    out: &mut Vec<vst3::MidiEvent>,
) {
    out.clear();
    if !runtime.clip.is_midi || runtime.clip.midi_notes.is_empty() {
        return;
    }
    let launch_sample = runtime.launch_samples;
    let block_len = block_end.saturating_sub(block_start);
    let recovering_launch_edge = block_start > launch_sample
        && block_start.saturating_sub(launch_sample) < block_len.max(1);
    let rel_block_start = block_start.saturating_sub(launch_sample) as f64 / samples_per_beat;
    let rel_block_end = block_end.saturating_sub(launch_sample) as f64 / samples_per_beat;
    let loop_beats = performance_clip_loop_beats(runtime) as f64;
    for note in &runtime.clip.midi_notes {
        let note_rel = (note.start_beats - runtime.clip.start_beats).max(0.0) as f64;
        let note_len = note.length_beats.max(0.01) as f64;
        let repeat_start = if runtime.loop_enabled {
            ((rel_block_start - note_rel) / loop_beats).floor().max(0.0) as i32
        } else {
            0
        };
        let repeat_end = if runtime.loop_enabled {
            ((rel_block_end - note_rel) / loop_beats).ceil().max(0.0) as i32
        } else {
            1
        };
        for repeat in repeat_start..repeat_end {
            let repeat_offset = if runtime.loop_enabled {
                repeat as f64 * loop_beats
            } else {
                0.0
            };
            let note_start_beats = note_rel + repeat_offset;
            let note_end_beats = note_start_beats + note_len;
            let start_sample = launch_sample + (note_start_beats * samples_per_beat).round() as u64;
            let mut end_sample = launch_sample + (note_end_beats * samples_per_beat).round() as u64;
            if end_sample <= start_sample {
                end_sample = start_sample.saturating_add(1);
            }
            if recovering_launch_edge
                && start_sample < block_start
                && end_sample > block_start
                && start_sample >= launch_sample
            {
                out.push(vst3::MidiEvent::note_on_at(
                    0,
                    note.midi_note,
                    note.velocity,
                    0,
                ));
            }
            if start_sample >= block_start && start_sample < block_end {
                out.push(vst3::MidiEvent::note_on_at(
                    0,
                    note.midi_note,
                    note.velocity,
                    (start_sample - block_start) as i32,
                ));
            }
            if end_sample >= block_start && end_sample < block_end {
                out.push(vst3::MidiEvent::note_off_at(
                    0,
                    note.midi_note,
                    0,
                    (end_sample - block_start) as i32,
                ));
            }
            if !runtime.loop_enabled {
                break;
            }
        }
    }
}

pub(super) fn performance_audio_clip_for_block(
    runtime: &PerformanceRuntimeClip,
    _block_start: u64,
    _sample_rate: u32,
    samples_per_beat: f64,
    audio_cache: &Arc<Mutex<AudioClipCache>>,
) -> Option<(AudioClipRender, Arc<AudioClipData>)> {
    if runtime.clip.is_midi {
        return None;
    }
    let path = runtime.resolved_audio_path.as_ref()?.clone();
    // Use try_lock in the realtime callback path to avoid blocking the audio thread.
    let data = audio_cache
        .try_lock()
        .ok()
        .and_then(|mut cache| cache.get(&path))?;
    let clip = &runtime.clip;
    let length_samples = if runtime.loop_enabled {
        u64::MAX / 4
    } else {
        (clip.length_beats.max(0.25) as f64 * samples_per_beat)
            .round()
            .max(1.0) as u64
    };
    let offset_samples = (clip.audio_offset_beats.max(0.0) as f64 * samples_per_beat)
        .round() as u64;
    Some((
        AudioClipRender {
            clip_id: clip.id,
            path,
            track_index: runtime.track_index,
            start_samples: runtime.launch_samples,
            length_samples,
            offset_samples,
            gain: clip.audio_gain,
            time_mul: clip.audio_time_mul.max(0.01),
            pitch_semitones: clip.audio_pitch_semitones,
            stretch_mode: clip.audio_stretch_mode,
            formant_scale: clip.audio_formant_scale,
        },
        data,
    ))
}