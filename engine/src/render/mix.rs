use crate::audio::{
    AudioClipCache, AudioClipData, TrackAudioState, TreeSynthRuntime, TreeSynthVoice,
    PLUGIN_PROCESS_FAILURES,
};
use crate::error::LingError;
use crate::hosts::vst3;
use crate::models::PluginHostHandle;
use crate::models::*;
use crate::node_editor::TrackNodeActivity;
use crate::performance::{performance_audio_clip_for_block, PerformanceRuntimeClip};
use crate::render::{
    collect_block_events, db_to_gain, AudioRuntimeBuffers, EVENTS_TMP, FILTERED_EVENTS_TMP,
    FX_TEMP, HOST_OUTPUT_TEMP, MIX_TEMP,
};
use parking_lot::Mutex;
use rayon::prelude::*;
use std::f32::consts::TAU;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

pub const TREESYNTH_MAX_VOICES: usize = 32;

pub fn mix_clip_resample(
    temp: &mut [f32],
    channels: usize,
    clip: &AudioClipRender,
    data: &AudioClipData,
    block_start: u64,
    block_end: u64,
    sample_rate: f32,
) {
    let clip_end = clip.start_samples + clip.length_samples;
    if block_end <= clip.start_samples || block_start >= clip_end {
        return;
    }
    let src_channels = data.channels.max(1);
    let src_frames = data.samples.len() / src_channels;
    if src_frames == 0 {
        return;
    }
    let rate_ratio = data.sample_rate as f64 / sample_rate as f64;
    let time_mul = clip.time_mul.max(0.01) as f64;
    let start_in_block = block_start.max(clip.start_samples) - block_start;
    let end_in_block = block_end.min(clip_end) - block_start;
    for i in start_in_block..end_in_block {
        let clip_pos = i + block_start - clip.start_samples;
        let pos = ((clip_pos as f64 + clip.offset_samples as f64) * rate_ratio / time_mul).max(0.0);
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
            let src_ch = if src_channels == 1 {
                0
            } else {
                ch.min(src_channels - 1)
            };
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
}

pub(crate) fn treesynth_adsr_level(state: &TreeSynthState, elapsed: f32) -> f32 {
    let attack = state.attack.max(0.0001);
    let decay = state.decay.max(0.0001);
    let sustain = state.sustain.clamp(0.0, 1.0);
    if elapsed <= 0.0 {
        0.0
    } else if elapsed < attack {
        (elapsed / attack).clamp(0.0, 1.0)
    } else if elapsed < attack + decay {
        let t = (elapsed - attack) / decay;
        (1.0 - (1.0 - sustain) * t).clamp(0.0, 1.0)
    } else {
        sustain
    }
}

pub(crate) fn treesynth_spawn_voice(
    runtime: &mut TreeSynthRuntime,
    state: &TreeSynthState,
    sample_index: usize,
    sample: &TreeSynthSample,
    data: &AudioClipData,
    note: u8,
    velocity: u8,
    start_sample: u64,
    sample_rate: f32,
) {
    treesynth_spawn_voice_with_gain(
        runtime,
        state,
        sample_index,
        sample,
        1.0,
        data,
        note,
        velocity,
        start_sample,
        sample_rate,
    );
}

pub(crate) fn treesynth_spawn_voice_with_gain(
    runtime: &mut TreeSynthRuntime,
    state: &TreeSynthState,
    sample_index: usize,
    sample: &TreeSynthSample,
    gain_mul: f32,
    data: &AudioClipData,
    note: u8,
    velocity: u8,
    start_sample: u64,
    sample_rate: f32,
) {
    if runtime.voices.len() >= TREESYNTH_MAX_VOICES {
        if let Some(pos) = runtime
            .voices
            .iter()
            .position(|v| v.release_sample.is_some())
        {
            runtime.voices.remove(pos);
        } else if !runtime.voices.is_empty() {
            runtime.voices.remove(0);
        }
    }
    let src_frames = data.samples.len() / data.channels.max(1);
    if src_frames == 0 {
        return;
    }
    let start = (sample.start.clamp(0.0, 1.0) * src_frames as f32) as f64;
    let mut end = (sample.end.clamp(0.0, 1.0) * src_frames as f32) as f64;
    if end <= start {
        end = (start + 1.0).min(src_frames as f64);
    }
    let note_diff = note as f64 - sample.root_note as f64;
    let target_rate = 2.0f64.powf(note_diff / 12.0);
    let rate_ratio = data.sample_rate as f64 / sample_rate.max(1.0) as f64;
    let mut rate = target_rate;
    let mut rate_step = 0.0f64;
    let mut glide_remaining = 0u64;
    if state.portamento_ms > 0.0 {
        if let Some(last_note) = runtime.last_note {
            if !state.legato || runtime.voices.iter().any(|v| v.release_sample.is_none()) {
                let last_diff = last_note as f64 - sample.root_note as f64;
                let start_rate = 2.0f64.powf(last_diff / 12.0);
                let glide_samples = (state.portamento_ms / 1000.0 * sample_rate.max(1.0))
                    .round()
                    .max(1.0) as u64;
                rate = start_rate;
                rate_step = (target_rate - start_rate) / glide_samples as f64;
                glide_remaining = glide_samples;
            }
        }
    }
    let velocity_gain = (velocity as f32 / 127.0).powf(2.0).clamp(0.0, 1.0);
    let gain = state.gain * sample.gain * velocity_gain * gain_mul;
    if runtime.voices.len() >= 32 {
        runtime.voices.remove(0);
    }
    runtime.voices.push(TreeSynthVoice {
        sample_index,
        sample_pos: start,
        sample_end: end,
        step: rate_ratio * rate,
        note,
        start_sample,
        release_sample: None,
        release_level: 1.0,
        gain,
        pan: sample.pan,
        rate,
        rate_step,
        glide_remaining,
    });
}

pub(crate) fn mix_treesynth_block(
    temp: &mut [f32],
    channels: usize,
    sample_rate: f32,
    block_start: u64,
    block_end: u64,
    samples_per_beat: f64,
    panic_notes: bool,
    loop_wrapped: bool,
    notes: &[PianoRollNote],
    extra_events: &[vst3::MidiEvent],
    state: &TrackAudioState,
    audio_cache: &Arc<Mutex<AudioClipCache>>,
) -> (bool, Vec<vst3::MidiEvent>) {
    let treesynth_state = match state
        .treesynth_state
        .as_ref()
        .and_then(|arc| arc.try_lock())
    {
        Some(guard) => (*guard).clone(),
        None => return (false, Vec::new()),
    };
    if treesynth_state.samples.is_empty() {
        return (false, Vec::new());
    }

    let mut events = collect_block_events(notes, block_start, block_end, samples_per_beat);
    events.extend(extra_events.iter().cloned());
    if panic_notes {
        for channel in 0u8..16 {
            events.push(vst3::MidiEvent::control_change(channel, 120, 0));
            events.push(vst3::MidiEvent::control_change(channel, 123, 0));
        }
        for channel in 0u8..16 {
            events
                .extend((0u8..=127).map(|note| vst3::MidiEvent::note_off_at(channel, note, 0, 0)));
        }
    }
    if loop_wrapped {
        events.extend((0u8..=127).map(|note| vst3::MidiEvent::note_off(0, note, 0)));
    }
    if let Some(mut queued) = state.midi_events.try_lock() {
        events.extend(queued.drain(..));
    }

    let mut runtime = match state.treesynth_runtime.try_lock() {
        Some(guard) => guard,
        None => return (false, events),
    };

    let sample_count = treesynth_state.samples.len();
    let sample_data: Vec<Option<Arc<AudioClipData>>> =
        if let Some(mut cache) = audio_cache.try_lock() {
            treesynth_state
                .samples
                .iter()
                .map(|sample| cache.get(&sample.path))
                .collect()
        } else {
            vec![None; sample_count]
        };
    let mut note_offs: Vec<(u8, u64)> = Vec::new();
    for event in &events {
        match event {
            vst3::MidiEvent::NoteOn {
                note,
                velocity,
                sample_offset,
                ..
            } => {
                if *velocity == 0 {
                    let offset = (*sample_offset).max(0) as u64;
                    note_offs.push((*note, block_start + offset));
                    continue;
                }
                let offset = (*sample_offset).max(0) as u64;
                let start_sample = block_start + offset;
                match treesynth_state.mode {
                    TreeSynthMode::Random => {
                        let idx = (runtime.next_rand() as usize) % sample_count;
                        if let Some(data) = sample_data.get(idx).and_then(|d| d.as_ref()) {
                            let sample = &treesynth_state.samples[idx];
                            treesynth_spawn_voice(
                                &mut runtime,
                                &treesynth_state,
                                idx,
                                sample,
                                data,
                                *note,
                                *velocity,
                                start_sample,
                                sample_rate,
                            );
                        }
                    }
                    TreeSynthMode::Sequential => {
                        let idx = runtime.sequence_index % sample_count;
                        runtime.sequence_index = runtime.sequence_index.wrapping_add(1);
                        if let Some(data) = sample_data.get(idx).and_then(|d| d.as_ref()) {
                            let sample = &treesynth_state.samples[idx];
                            treesynth_spawn_voice(
                                &mut runtime,
                                &treesynth_state,
                                idx,
                                sample,
                                data,
                                *note,
                                *velocity,
                                start_sample,
                                sample_rate,
                            );
                        }
                    }
                    TreeSynthMode::Reorder => {
                        let pos = ((f32::from(*note) / 127.0) + treesynth_state.reorder).fract();
                        let idx = (pos * sample_count as f32).floor() as usize;
                        let idx = idx.min(sample_count.saturating_sub(1));
                        if let Some(data) = sample_data.get(idx).and_then(|d| d.as_ref()) {
                            let sample = &treesynth_state.samples[idx];
                            treesynth_spawn_voice(
                                &mut runtime,
                                &treesynth_state,
                                idx,
                                sample,
                                data,
                                *note,
                                *velocity,
                                start_sample,
                                sample_rate,
                            );
                        }
                    }
                    TreeSynthMode::Morph => {
                        let morph = treesynth_state.morph.clamp(0.0, 1.0)
                            * (sample_count.saturating_sub(1) as f32);
                        let idx0 = morph.floor() as usize;
                        let idx1 = (idx0 + 1).min(sample_count.saturating_sub(1));
                        let frac = morph - idx0 as f32;
                        let weight0 = (1.0f32 - frac).clamp(0.0, 1.0);
                        let weight1 = frac.clamp(0.0, 1.0);
                        if let Some(data) = sample_data.get(idx0).and_then(|d| d.as_ref()) {
                            treesynth_spawn_voice_with_gain(
                                &mut runtime,
                                &treesynth_state,
                                idx0,
                                &treesynth_state.samples[idx0],
                                weight0,
                                data,
                                *note,
                                *velocity,
                                start_sample,
                                sample_rate,
                            );
                        }
                        if idx1 != idx0 {
                            if let Some(data) = sample_data.get(idx1).and_then(|d| d.as_ref()) {
                                treesynth_spawn_voice_with_gain(
                                    &mut runtime,
                                    &treesynth_state,
                                    idx1,
                                    &treesynth_state.samples[idx1],
                                    weight1,
                                    data,
                                    *note,
                                    *velocity,
                                    start_sample,
                                    sample_rate,
                                );
                            }
                        }
                    }
                    TreeSynthMode::Layer => {
                        for idx in 0..sample_count {
                            if let Some(data) = sample_data.get(idx).and_then(|d| d.as_ref()) {
                                let sample = &treesynth_state.samples[idx];
                                treesynth_spawn_voice(
                                    &mut runtime,
                                    &treesynth_state,
                                    idx,
                                    sample,
                                    data,
                                    *note,
                                    *velocity,
                                    start_sample,
                                    sample_rate,
                                );
                            }
                        }
                    }
                }
                runtime.last_note = Some(*note);
            }
            vst3::MidiEvent::NoteOff {
                note,
                sample_offset,
                ..
            } => {
                let offset = (*sample_offset).max(0) as u64;
                note_offs.push((*note, block_start + offset));
            }
            _ => {}
        }
    }

    for (note, release_sample) in note_offs {
        for voice in runtime
            .voices
            .iter_mut()
            .filter(|v| v.note == note && v.release_sample.is_none())
        {
            let elapsed =
                (release_sample.saturating_sub(voice.start_sample)) as f32 / sample_rate.max(1.0);
            voice.release_level = treesynth_adsr_level(&treesynth_state, elapsed);
            voice.release_sample = Some(release_sample);
        }
    }

    let mut processed = false;
    let frame_count = (block_end - block_start) as usize;
    runtime.voices.retain_mut(|voice| {
        let sample_index = voice.sample_index;
        if treesynth_state.samples.get(sample_index).is_none() {
            return false;
        }
        let data = match sample_data.get(sample_index).and_then(|d| d.as_ref()) {
            Some(data) => data,
            None => return false,
        };
        if let Some(release_sample) = voice.release_sample {
            let release_frames: u64 =
                (treesynth_state.release.max(0.0001) * sample_rate.max(1.0)) as u64;
            if block_start >= release_sample.saturating_add(release_frames) {
                return false;
            }
        }
        let src_channels = data.channels.max(1);
        let src_frames = data.samples.len() / src_channels;
        if src_frames == 0 {
            return false;
        }
        let rate_ratio = data.sample_rate as f64 / sample_rate.max(1.0) as f64;
        let tremolo_rate = treesynth_state.tremolo_rate.max(0.1);
        let vibrato_rate = treesynth_state.vibrato_rate.max(0.1);
        let mut alive = false;
        for i in 0..frame_count {
            let current_sample = block_start + i as u64;
            if current_sample < voice.start_sample {
                continue;
            }
            let elapsed = (current_sample - voice.start_sample) as f32 / sample_rate.max(1.0);
            let level = if let Some(release_sample) = voice.release_sample {
                if current_sample >= release_sample {
                    let release = treesynth_state.release.max(0.0001);
                    let rel_t = (current_sample - release_sample) as f32 / sample_rate.max(1.0);
                    let remaining = (1.0 - rel_t / release).clamp(0.0, 1.0);
                    voice.release_level * remaining
                } else {
                    treesynth_adsr_level(&treesynth_state, elapsed)
                }
            } else {
                treesynth_adsr_level(&treesynth_state, elapsed)
            };
            if level <= 0.0001 {
                continue;
            }
            let trem = if treesynth_state.tremolo_depth > 0.0 {
                let t = (TAU * tremolo_rate * elapsed).sin();
                (1.0 - treesynth_state.tremolo_depth)
                    + (0.5 + 0.5 * t) * treesynth_state.tremolo_depth
            } else {
                1.0
            };
            let final_level = level * trem;

            if voice.glide_remaining > 0 {
                voice.rate += voice.rate_step;
                voice.glide_remaining -= 1;
            }
            let vibrato = if treesynth_state.vibrato_depth > 0.0 {
                let t = (TAU * vibrato_rate * elapsed).sin() as f64;
                2.0f64.powf((treesynth_state.vibrato_depth as f64 * t) / 12.0)
            } else {
                1.0
            };
            voice.step = rate_ratio * voice.rate * vibrato;
            let pos = voice.sample_pos;
            if pos >= voice.sample_end {
                continue;
            }
            let base = pos.floor() as usize;
            let next = (base + 1).min(src_frames.saturating_sub(1));
            let frac = (pos - base as f64) as f32;
            let left_gain = if channels >= 2 {
                (1.0 - voice.pan.clamp(-1.0, 1.0)) * 0.5
            } else {
                1.0
            };
            let right_gain = if channels >= 2 {
                (1.0 + voice.pan.clamp(-1.0, 1.0)) * 0.5
            } else {
                1.0
            };
            for ch in 0..channels {
                let src_ch = if src_channels == 1 {
                    0
                } else {
                    ch.min(src_channels - 1)
                };
                let idx0 = base * src_channels + src_ch;
                let idx1 = next * src_channels + src_ch;
                let s0 = data.samples.get(idx0).copied().unwrap_or(0.0);
                let s1 = data.samples.get(idx1).copied().unwrap_or(0.0);
                let mut sample_value = s0 + (s1 - s0) * frac;
                let pan_gain = if ch == 0 { left_gain } else { right_gain };
                sample_value *= voice.gain * final_level * pan_gain;
                let out_index = i * channels + ch;
                if out_index < temp.len() {
                    temp[out_index] += sample_value;
                }
            }
            voice.sample_pos += voice.step;
            alive = true;
        }
        if voice.sample_pos >= voice.sample_end {
            return false;
        }
        alive
    });

    if !runtime.voices.is_empty() {
        processed = true;
    }
    (processed, events)
}

pub fn mix_track_hosts(
    output: &mut [f32],
    channels: usize,
    sample_rate: f32,
    tempo_bits: &AtomicU32,
    transport_samples: &AtomicU64,
    loop_start_samples: &AtomicU64,
    loop_end_samples: &AtomicU64,
    playback_panic: &AtomicBool,
    arrangement_playback_enabled: &AtomicBool,
    track_audio: &[TrackAudioState],
    track_mix: &Arc<Mutex<Vec<TrackMixState>>>,
    node_activity: &Arc<Mutex<Vec<TrackNodeActivity>>>,
    node_routes: &Arc<Mutex<Vec<NodeRouteLink>>>,
    performance_runtime: &Arc<Mutex<Vec<Option<PerformanceRuntimeClip>>>>,
    audio_clips: &Arc<Mutex<Vec<AudioClipRender>>>,
    audio_cache: &Arc<Mutex<AudioClipCache>>,
    _smart_disable_plugins: bool,
    smart_suspend_tracks: bool,
    runtime_buffers: &mut AudioRuntimeBuffers,
) -> bool {
    let frames = output.len() / channels;
    if frames == 0 || channels == 0 {
        return false;
    }
    let bpm = f32::from_bits(tempo_bits.load(Ordering::Relaxed)).max(1.0);
    let samples_per_beat = sample_rate as f64 * 60.0 / bpm as f64;
    let transport_pos = transport_samples.load(Ordering::Relaxed);
    let playback_enabled = arrangement_playback_enabled.load(Ordering::Relaxed);
    let panic_all = playback_panic.load(Ordering::Relaxed);
    let loop_start = loop_start_samples.load(Ordering::Relaxed);
    let loop_end = loop_end_samples.load(Ordering::Relaxed);
    let is_looping = loop_end > loop_start;

    let track_count = track_audio.len();
    let processed_any_atomic = AtomicBool::new(false);

    let mix_guard = track_mix.lock();
    let routes_guard = node_routes.lock();
    runtime_buffers.routes_snapshot = (*routes_guard).clone();
    drop(routes_guard);

    track_audio
        .par_iter()
        .enumerate()
        .for_each(|(track_index, state)| {
            let mix = match mix_guard.get(track_index) {
                Some(m) => m,
                None => return,
            };
            if !mix.active
                && smart_suspend_tracks
                && state.silent_blocks.load(Ordering::Relaxed) > 100
            {
                return;
            }

            let mut track_processed = false;
            MIX_TEMP.with(|temp_cell| {
                let mut temp = temp_cell.borrow_mut();
                if temp.len() != output.len() {
                    temp.resize(output.len(), 0.0);
                }
                temp.fill(0.0);

                FX_TEMP.with(|fx_cell| {
                    let block_start = transport_pos;
                    let block_end = transport_pos + frames as u64;
                    let block_beat = block_start as f32 / samples_per_beat as f32;

                    let mut loop_wrapped = false;
                    if is_looping
                        && playback_enabled
                        && block_start < loop_end
                        && block_end >= loop_end
                    {
                        loop_wrapped = true;
                    }

                    let mut clips = Vec::new();
                    if let Some(guard) = audio_clips.try_lock() {
                        for clip in guard.iter() {
                            if clip.track_index == track_index {
                                if let Some(data) = audio_cache.lock().get(&clip.path) {
                                    clips.push((clip.clone(), data));
                                }
                            }
                        }
                    }

                    let automation = state.automation_lanes.lock().clone();
                    let panic_notes = panic_all;

                    let active_performance = {
                        let perf = performance_runtime.lock();
                        perf.get(track_index).and_then(|opt| opt.clone())
                    };

                    EVENTS_TMP.with(|events_cell| {
                        let mut events = events_cell.borrow_mut();
                        events.clear();

                        let (ts_processed, ts_events) = mix_treesynth_block(
                            &mut temp,
                            channels,
                            sample_rate,
                            block_start,
                            block_end,
                            samples_per_beat,
                            panic_notes,
                            loop_wrapped,
                            &state.clip_notes.lock(),
                            &[],
                            state,
                            audio_cache,
                        );
                        if ts_processed {
                            track_processed = true;
                        }
                        events.extend(ts_events);

                        if let Some(ref host) = state.host {
                            FILTERED_EVENTS_TMP.with(|filtered_cell| {
                                let mut filtered = filtered_cell.borrow_mut();
                                filtered.clear();
                                for ev in events.iter() {
                                    filtered.push(*ev);
                                }
                            });

                            HOST_OUTPUT_TEMP.with(|host_out_cell| {
                                let mut host_out = host_out_cell.borrow_mut();
                                if host_out.len() != output.len() {
                                    host_out.resize(output.len(), 0.0);
                                }
                                host_out.fill(0.0);

                                let mut remaining_params = Vec::new();
                                if let Some(mut pending) = state.pending_param_changes.try_lock() {
                                    remaining_params.extend(pending.drain(..));
                                }

                                for pending in remaining_params.iter() {
                                    if pending.target == PendingParamTarget::Instrument {
                                        match host {
                                            PluginHostHandle::Vst3(h) => {
                                                if let Some(mut g) = h.try_lock() {
                                                    g.push_param_change(
                                                        pending.param_id,
                                                        pending.value,
                                                    );
                                                }
                                            }
                                            PluginHostHandle::Clap(h) => {
                                                if let Some(mut g) = h.try_lock() {
                                                    g.push_param_change(
                                                        pending.param_id,
                                                        pending.value,
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }

                                for lane in &automation {
                                    if let Some(value) = lane.value_at(block_beat) {
                                        if lane.target == AutomationTarget::Instrument {
                                            match host {
                                                PluginHostHandle::Vst3(h) => {
                                                    if let Some(mut g) = h.try_lock() {
                                                        g.push_param_change(
                                                            lane.param_id,
                                                            value as f64,
                                                        );
                                                    }
                                                }
                                                PluginHostHandle::Clap(h) => {
                                                    if let Some(mut g) = h.try_lock() {
                                                        g.push_param_change(
                                                            lane.param_id,
                                                            value as f64,
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }

                                FILTERED_EVENTS_TMP.with(|filtered_cell| {
                                    let filtered = filtered_cell.borrow();
                                    let result = match host {
                                        PluginHostHandle::Vst3(h) => match h.try_lock() {
                                            Some(mut g) => {
                                                g.process_f32(&mut host_out, channels, &filtered)
                                            }
                                            None => {
                                                PLUGIN_PROCESS_FAILURES
                                                    .fetch_add(1, Ordering::Relaxed);
                                                Err(LingError::Plugin(
                                                    "instrument host lock unavailable".to_string(),
                                                ))
                                            }
                                        },
                                        PluginHostHandle::Clap(h) => match h.try_lock() {
                                            Some(mut g) => {
                                                g.process_f32(&mut host_out, channels, &filtered)
                                            }
                                            None => {
                                                PLUGIN_PROCESS_FAILURES
                                                    .fetch_add(1, Ordering::Relaxed);
                                                Err(LingError::Plugin(
                                                    "instrument host lock unavailable".to_string(),
                                                ))
                                            }
                                        },
                                    };
                                    if result.is_ok() {
                                        for (t, h) in temp.iter_mut().zip(host_out.iter()) {
                                            *t += *h;
                                        }
                                        track_processed = true;
                                    }
                                });
                            });
                        }

                        if panic_notes {
                            temp.fill(0.0);
                        }
                    });

                    for (clip, data) in clips {
                        mix_clip_resample(
                            &mut temp,
                            channels,
                            &clip,
                            &data,
                            block_start,
                            block_end,
                            sample_rate,
                        );
                        track_processed = true;
                    }

                    if let Some(runtime) = active_performance {
                        if let Some((clip, data)) = performance_audio_clip_for_block(
                            &runtime,
                            block_start,
                            sample_rate as u32,
                            samples_per_beat,
                            audio_cache,
                        ) {
                            mix_clip_resample(
                                &mut temp,
                                channels,
                                &clip,
                                &data,
                                block_start,
                                block_end,
                                sample_rate,
                            );
                            track_processed = true;
                        }
                    }

                    let mut fx_temp_buf = fx_cell.borrow_mut();
                    if fx_temp_buf.len() != output.len() {
                        fx_temp_buf.resize(output.len(), 0.0);
                    }
                    let mut uses_scratch = false;
                    {
                        let mut current = &mut *temp;
                        let mut scratch = &mut *fx_temp_buf;

                        let bypass_guard = state.effect_bypass.lock();
                        let bypass = (*bypass_guard).clone();
                        drop(bypass_guard);

                        for (fx_index, fx) in state.effect_hosts.iter().enumerate() {
                            let is_bypassed = bypass.get(fx_index).copied().unwrap_or(false);
                            if is_bypassed {
                                continue;
                            }

                            for lane in &automation {
                                if let Some(value) = lane.value_at(block_beat) {
                                    if let AutomationTarget::Effect(target_fx) = lane.target {
                                        if target_fx == fx_index {
                                            match fx {
                                                PluginHostHandle::Vst3(h) => {
                                                    if let Some(mut g) = h.try_lock() {
                                                        g.push_param_change(
                                                            lane.param_id,
                                                            value as f64,
                                                        );
                                                    }
                                                }
                                                PluginHostHandle::Clap(h) => {
                                                    if let Some(mut g) = h.try_lock() {
                                                        g.push_param_change(
                                                            lane.param_id,
                                                            value as f64,
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            scratch.fill(0.0);
                            let result = match fx {
                                PluginHostHandle::Vst3(h) => match h.try_lock() {
                                    Some(mut g) => {
                                        g.process_f32_with_input(current, scratch, channels, &[])
                                    }
                                    None => {
                                        PLUGIN_PROCESS_FAILURES.fetch_add(1, Ordering::Relaxed);
                                        Err(LingError::Plugin(
                                            "effect host lock unavailable".to_string(),
                                        ))
                                    }
                                },
                                PluginHostHandle::Clap(h) => match h.try_lock() {
                                    Some(mut g) => {
                                        g.process_f32_with_input(current, scratch, channels, &[])
                                    }
                                    None => {
                                        PLUGIN_PROCESS_FAILURES.fetch_add(1, Ordering::Relaxed);
                                        Err(LingError::Plugin(
                                            "effect host lock unavailable".to_string(),
                                        ))
                                    }
                                },
                            };
                            if result.is_ok() {
                                std::mem::swap(&mut current, &mut scratch);
                                uses_scratch = !uses_scratch;
                                track_processed = true;
                            }
                        }
                    }

                    if uses_scratch {
                        temp.copy_from_slice(&fx_temp_buf);
                    }

                    let mut peak_l = 0.0f32;
                    let mut peak_r = 0.0f32;
                    for frame in temp.chunks(channels.max(1)) {
                        if channels >= 2 {
                            peak_l = peak_l.max(frame[0].abs());
                            peak_r = peak_r.max(frame[1].abs());
                        } else if !frame.is_empty() {
                            peak_l = peak_l.max(frame[0].abs());
                            peak_r = peak_l;
                        }
                    }
                    state.peak_l_bits.store(peak_l.to_bits(), Ordering::Relaxed);
                    state.peak_r_bits.store(peak_r.to_bits(), Ordering::Relaxed);
                    state
                        .peak_bits
                        .store(peak_l.max(peak_r).to_bits(), Ordering::Relaxed);

                    if track_processed {
                        processed_any_atomic.store(true, Ordering::Relaxed);
                    }

                    if let Some(mut track_out_mutex) = state.track_buffer.try_lock() {
                        if track_out_mutex.len() != temp.len() {
                            track_out_mutex.resize(temp.len(), 0.0);
                        }
                        for (out, sample) in track_out_mutex.iter_mut().zip(temp.iter()) {
                            *out = *sample * mix.level;
                        }
                    }
                })
            })
        });

    let processed_any = processed_any_atomic.load(Ordering::Relaxed);

    for (route_index, route) in runtime_buffers.routes_snapshot.iter().enumerate() {
        if !route.enabled || route.kind != NodeRouteKind::AudioSidechain {
            continue;
        }
        if route.from_track >= track_count
            || route.to_track >= track_count
            || route.from_track == route.to_track
        {
            continue;
        }
        let threshold = db_to_gain(route.sidechain_threshold_db.clamp(-60.0, 0.0));
        let amount = route.sidechain_amount.clamp(0.0, 1.0);
        let attack = (route.sidechain_attack_ms.max(0.1) / 1000.0).max(0.0001);
        let release = (route.sidechain_release_ms.max(0.1) / 1000.0).max(0.0001);
        let attack_coeff = (-1.0f32 / (attack * sample_rate.max(1.0))).exp();
        let release_coeff = (-1.0f32 / (release * sample_rate.max(1.0))).exp();

        let from_state = &track_audio[route.from_track];
        let to_state = &track_audio[route.to_track];

        let from_buf_guard = from_state.track_buffer.try_lock();
        let to_buf_guard = to_state.track_buffer.try_lock();

        if let (Some(source), Some(mut target)) = (from_buf_guard, to_buf_guard) {
            let mut gain = runtime_buffers
                .sidechain_states
                .get(route_index)
                .copied()
                .unwrap_or(1.0);
            for frame in 0..frames {
                let base = frame * channels;
                let mut detector = 0.0f32;
                for ch in 0..channels {
                    detector = detector.max((*source).get(base + ch).copied().unwrap_or(0.0).abs());
                }
                let target_gain = if detector > threshold {
                    let over = ((detector - threshold) / (1.0f32 - threshold).max(1e-6))
                        .clamp(0.0f32, 1.0f32);
                    (1.0f32 - amount * over).clamp(0.05f32, 1.0f32)
                } else {
                    1.0f32
                };
                if target_gain < gain {
                    gain = attack_coeff * (gain - target_gain) + target_gain;
                } else {
                    gain = release_coeff * (gain - target_gain) + target_gain;
                }
                for ch in 0..channels {
                    if let Some(sample) = (*target).get_mut(base + ch) {
                        *sample *= gain;
                    }
                }
            }
            if let Some(state) = runtime_buffers.sidechain_states.get_mut(route_index) {
                *state = gain;
            }
        }
    }

    output.fill(0.0);
    for state in track_audio {
        if let Some(buf) = state.track_buffer.try_lock() {
            for (out, sample) in output.iter_mut().zip(buf.iter()) {
                *out += *sample;
            }
        }
    }

    if let Some(mut activity) = node_activity.try_lock() {
        if activity.len() < track_count {
            activity.resize(track_count, TrackNodeActivity::default());
        }
        let activity_slice = &mut activity[..track_count];

        activity_slice
            .par_iter_mut()
            .zip(track_audio.par_iter())
            .for_each(|(slot, state)| {
                let mut peak = 0.0f32;
                if let Some(track_buf) = state.track_buffer.try_lock() {
                    for frame in 0..frames {
                        let base = frame * channels;
                        let mut detector = 0.0f32;
                        for ch in 0..channels {
                            detector = detector
                                .max((*track_buf).get(base + ch).copied().unwrap_or(0.0).abs());
                        }
                        peak = peak.max(detector);
                    }
                }
                slot.output_pair_peaks[0] = (slot.output_pair_peaks[0] * 0.82).max(peak);

                slot.midi_in = (slot.midi_in * 0.78)
                    .max(f32::from_bits(state.midi_in_peak.load(Ordering::Relaxed)));
                slot.midi_out = (slot.midi_out * 0.78)
                    .max(f32::from_bits(state.midi_out_peak.load(Ordering::Relaxed)));
            });
    }
    processed_any
}

pub fn update_master_peak_f32(data: &[f32], peak_bits: &AtomicU32) {
    let mut peak = 0.0f32;
    for s in data {
        peak = peak.max(s.abs());
    }
    let prev = f32::from_bits(peak_bits.load(Ordering::Relaxed));
    peak_bits.store(prev.max(peak).to_bits(), Ordering::Relaxed);
}

pub fn update_master_peak_i16(data: &[i16], peak_bits: &AtomicU32) {
    let mut peak = 0.0f32;
    for s in data {
        let v = *s as f32 / i16::MAX as f32;
        peak = peak.max(v.abs());
    }
    let prev = f32::from_bits(peak_bits.load(Ordering::Relaxed));
    peak_bits.store(prev.max(peak).to_bits(), Ordering::Relaxed);
}

pub fn update_master_peak_u16(data: &[u16], peak_bits: &AtomicU32) {
    let mut peak = 0.0f32;
    let scale = u16::MAX as f32;
    for s in data {
        let v = (*s as f32 / scale) * 2.0 - 1.0;
        peak = peak.max(v.abs());
    }
    let prev = f32::from_bits(peak_bits.load(Ordering::Relaxed));
    peak_bits.store(prev.max(peak).to_bits(), Ordering::Relaxed);
}

pub fn render_sine(
    data: &mut [f32],
    channels: usize,
    sample_rate: f32,
    freq_bits: &AtomicU32,
    gate: &AtomicBool,
) {
    let ch = channels.max(1);
    if !gate.load(Ordering::Relaxed) {
        data.fill(0.0);
        return;
    }
    let freq = f32::from_bits(freq_bits.load(Ordering::Relaxed)).max(1.0);
    let sr = sample_rate.max(1.0);
    let step = TAU * freq / sr;
    let mut phase = 0.0f32;
    for frame in data.chunks_exact_mut(ch) {
        let s = phase.sin() * 0.2;
        for out in frame.iter_mut() {
            *out = s;
        }
        phase = (phase + step) % TAU;
    }
}

pub fn apply_fade_in_if_needed(data: &mut [f32], channels: usize, fade_gate: &AtomicBool) {
    if !fade_gate.load(Ordering::Relaxed) {
        return;
    }
    let ch = channels.max(1);
    let frames = data.len() / ch;
    if frames == 0 {
        return;
    }
    const FADE_FRAMES: usize = 2048;
    for (i, frame) in data.chunks_exact_mut(ch).enumerate().take(FADE_FRAMES) {
        let g = ((i + 1) as f32 / FADE_FRAMES as f32).min(1.0);
        for s in frame.iter_mut() {
            *s *= g;
        }
    }
    fade_gate.store(false, Ordering::Relaxed);
}
