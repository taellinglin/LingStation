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
    treesynth_spawn_voice_with_gain(runtime, state, sample_index, sample, 1.0, data, note, velocity, start_sample, sample_rate);
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
        if let Some(pos) = runtime.voices.iter().position(|v| v.release_sample.is_some()) {
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
    let treesynth_state =
        match state.treesynth_state.as_ref().and_then(|arc| arc.try_lock().ok()) {
        Some(guard) => guard.clone(),
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
            events.extend(
                (0u8..=127).map(|note| vst3::MidiEvent::note_off_at(channel, note, 0, 0)),
            );
        }
    }
    if loop_wrapped {
        events.extend((0u8..=127).map(|note| vst3::MidiEvent::note_off(0, note, 0)));
    }
    if let Ok(mut queued) = state.midi_events.try_lock() {
        events.extend(queued.drain(..));
    }

    let mut runtime = match state.treesynth_runtime.try_lock() {
        Ok(guard) => guard,
        Err(_) => return (false, events),
    };

    let sample_count = treesynth_state.samples.len();
    let sample_data: Vec<Option<Arc<AudioClipData>>> = if let Ok(mut cache) = audio_cache.try_lock() {
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
                            treesynth_spawn_voice(&mut runtime, &treesynth_state, idx, sample, &data, *note, *velocity, start_sample, sample_rate);
                        }
                    }
                    TreeSynthMode::Sequential => {
                        let idx = runtime.sequence_index % sample_count;
                        runtime.sequence_index = runtime.sequence_index.wrapping_add(1);
                        if let Some(data) = sample_data.get(idx).and_then(|d| d.as_ref()) {
                            let sample = &treesynth_state.samples[idx];
                            treesynth_spawn_voice(&mut runtime, &treesynth_state, idx, sample, &data, *note, *velocity, start_sample, sample_rate);
                        }
                    }
                    TreeSynthMode::Reorder => {
                        let pos = ((f32::from(*note) / 127.0) + treesynth_state.reorder).fract();
                        let idx = (pos * sample_count as f32).floor() as usize;
                        let idx = idx.min(sample_count.saturating_sub(1));
                        if let Some(data) = sample_data.get(idx).and_then(|d| d.as_ref()) {
                            let sample = &treesynth_state.samples[idx];
                            treesynth_spawn_voice(&mut runtime, &treesynth_state, idx, sample, &data, *note, *velocity, start_sample, sample_rate);
                        }
                    }
                    TreeSynthMode::Morph => {
                        let morph = treesynth_state.morph.clamp(0.0, 1.0) * (sample_count.saturating_sub(1) as f32);
                        let idx0 = morph.floor() as usize;
                        let idx1 = (idx0 + 1).min(sample_count.saturating_sub(1));
                        let frac = morph - idx0 as f32;
                        let weight0 = (1.0f32 - frac).clamp(0.0, 1.0);
                        let weight1 = frac.clamp(0.0, 1.0);
                        if let Some(data) = sample_data.get(idx0).and_then(|d| d.as_ref()) {
                            let mut sample = treesynth_state.samples[idx0].clone();
                            sample.gain *= weight0;
                            // Still cloning here because we modify the gain
                            // However, we only do this in Morph mode.
                            // Better way: pass an explicit gain override to spawn_voice
                            treesynth_spawn_voice_with_gain(&mut runtime, &treesynth_state, idx0, &treesynth_state.samples[idx0], weight0, &data, *note, *velocity, start_sample, sample_rate);
                        }
                        if idx1 != idx0 {
                            if let Some(data) = sample_data.get(idx1).and_then(|d| d.as_ref()) {
                                treesynth_spawn_voice_with_gain(&mut runtime, &treesynth_state, idx1, &treesynth_state.samples[idx1], weight1, &data, *note, *velocity, start_sample, sample_rate);
                            }
                        }
                    }
                    TreeSynthMode::Layer => {
                        for idx in 0..sample_count {
                            if let Some(data) = sample_data.get(idx).and_then(|d| d.as_ref()) {
                                let sample = &treesynth_state.samples[idx];
                                treesynth_spawn_voice(&mut runtime, &treesynth_state, idx, sample, &data, *note, *velocity, start_sample, sample_rate);
                            }
                        }
                    }
                }
                runtime.last_note = Some(*note);
            }
            vst3::MidiEvent::NoteOff { note, sample_offset, .. } => {
                let offset = (*sample_offset).max(0) as u64;
                note_offs.push((*note, block_start + offset));
            }
            _ => {}
        }
    }

    for (note, release_sample) in note_offs {
        for voice in runtime.voices.iter_mut().filter(|v| v.note == note && v.release_sample.is_none()) {
            let elapsed = (release_sample.saturating_sub(voice.start_sample)) as f32 / sample_rate.max(1.0);
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
            let mut level = if let Some(release_sample) = voice.release_sample {
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
            level *= trem;

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
                let src_ch = if src_channels == 1 { 0 } else { ch.min(src_channels - 1) };
                let idx0 = base * src_channels + src_ch;
                let idx1 = next * src_channels + src_ch;
                let s0 = data.samples.get(idx0).copied().unwrap_or(0.0);
                let s1 = data.samples.get(idx1).copied().unwrap_or(0.0);
                let mut sample_value = s0 + (s1 - s0) * frac;
                let pan_gain = if ch == 0 { left_gain } else { right_gain };
                sample_value *= voice.gain * level * pan_gain;
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

pub(crate) fn mix_track_hosts(
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
    let mut block_start = transport_samples.fetch_add(frames as u64, Ordering::Relaxed);
    let mut block_end = block_start + frames as u64;
    let loop_start = loop_start_samples.load(Ordering::Relaxed);
    let loop_end = loop_end_samples.load(Ordering::Relaxed);
    let panic_notes = playback_panic.swap(false, Ordering::Relaxed);
    let arrangement_playing = arrangement_playback_enabled.load(Ordering::Relaxed);
    let mut loop_wrapped = false;
    if arrangement_playing && loop_end > loop_start && block_start < loop_end && block_end > loop_end {
        block_start = loop_start;
        block_end = block_start + frames as u64;
        transport_samples.store(block_end, Ordering::Relaxed);
        loop_wrapped = true;
    }
    let block_beat = (block_start as f64 / samples_per_beat) as f32;

    if let Ok(m) = track_mix.try_lock() {
        if runtime_buffers.track_mix_snapshot.len() != m.len() {
            runtime_buffers.track_mix_snapshot.resize(m.len(), TrackMixState::default());
        }
        runtime_buffers.track_mix_snapshot.copy_from_slice(&m);
    }
    let any_solo = runtime_buffers.track_mix_snapshot.iter().any(|m| m.solo);

    if let Ok(runtime) = performance_runtime.try_lock() {
        if runtime_buffers.performance_snapshot.len() != runtime.len() {
            runtime_buffers.performance_snapshot.resize(runtime.len(), None);
        }
        for (i, slot) in runtime.iter().enumerate() {
            runtime_buffers.performance_snapshot[i] = slot.clone();
        }
    }
    let track_count = track_audio.len();
    
    runtime_buffers.resize(track_count, frames);
    runtime_buffers.track_has_audio.fill(false);
    for clips in runtime_buffers.per_track_clips.iter_mut() {
        clips.clear();
    }
    
    if arrangement_playing {
        if let Ok(clips) = audio_clips.try_lock() {
            for clip in clips.iter() {
                if clip.track_index >= track_count {
                    continue;
                }
                let clip_end = clip.start_samples + clip.length_samples;
                if block_end <= clip.start_samples || block_start >= clip_end {
                    continue;
                }
                runtime_buffers.track_has_audio[clip.track_index] = true;
                let data = {
                    let mut cache = match audio_cache.try_lock() {
                        Ok(cache) => cache,
                        Err(_) => continue,
                    };
                    cache.get(&clip.path)
                };
                let Some(data) = data else {
                    continue;
                };
                runtime_buffers.per_track_clips[clip.track_index].push((clip.clone(), data));
            }
        }
    }

    if let Ok(r) = node_routes.try_lock() {
        if runtime_buffers.routes_snapshot.len() != r.len() {
            runtime_buffers.routes_snapshot.resize(r.len(), NodeRouteLink::default());
        }
        for (i, route) in r.iter().enumerate() {
            runtime_buffers.routes_snapshot[i] = route.clone();
        }
    }
    if runtime_buffers.sidechain_states.len() < runtime_buffers.routes_snapshot.len() {
        runtime_buffers.sidechain_states.resize(runtime_buffers.routes_snapshot.len(), 1.0);
    } else if runtime_buffers.sidechain_states.len() > runtime_buffers.routes_snapshot.len() {
        runtime_buffers.sidechain_states.truncate(runtime_buffers.routes_snapshot.len());
    }

    let processed_any_atomic = AtomicBool::new(false);
    
    let h_audio_slice = &mut runtime_buffers.track_has_audio[..track_count];
    let p_clips_slice = &mut runtime_buffers.per_track_clips[..track_count];
    let h_outputs_slice = &mut runtime_buffers.track_host_outputs[..track_count];
    let h_chans_slice = &mut runtime_buffers.track_host_output_channels[..track_count];
    let h_active_slice = &mut runtime_buffers.track_host_output_active[..track_count];
    let mix_snap_slice = &runtime_buffers.track_mix_snapshot[..track_count];
    let perf_snap_slice = &runtime_buffers.performance_snapshot[..track_count];

    h_audio_slice.par_iter_mut()
        .zip(p_clips_slice.par_iter_mut())
        .zip(h_outputs_slice.par_iter_mut())
        .zip(h_chans_slice.par_iter_mut())
        .zip(h_active_slice.par_iter_mut())
        .zip(mix_snap_slice.par_iter())
        .zip(perf_snap_slice.par_iter())
        .zip(track_audio.par_iter())
        .for_each(|(((((((has_audio_from_clips, clips), _host_out_buf), _host_out_ch), host_out_active), mix), active_performance), state)| {
            let mix = *mix;
            let active_performance = active_performance.clone();
            let has_audio_from_clips = *has_audio_from_clips;

        if mix.muted || (any_solo && !mix.solo) {
            state.peak_bits.store(0.0f32.to_bits(), Ordering::Relaxed);
            state.peak_l_bits.store(0.0f32.to_bits(), Ordering::Relaxed);
            state.peak_r_bits.store(0.0f32.to_bits(), Ordering::Relaxed);
            state.midi_in_peak.store(0.0f32.to_bits(), Ordering::Relaxed);
            state.midi_out_peak.store(0.0f32.to_bits(), Ordering::Relaxed);
            return;
        }

        let notes = if arrangement_playing {
            match state.clip_notes.try_lock() {
                Ok(guard) => guard.clone(),
                Err(_) => Vec::new(),
            }
        } else {
            Vec::new()
        };
        let has_notes = !notes.is_empty()
            || active_performance
                .as_ref()
                .map(|runtime| runtime.clip.is_midi)
                .unwrap_or(false);
        let has_audio = has_audio_from_clips
            || active_performance
                .as_ref()
                .map(|runtime| !runtime.clip.is_midi)
                .unwrap_or(false);
        let automation = state
            .automation_lanes
            .try_lock()
            .ok()
            .map(|lanes| lanes.clone())
            .unwrap_or_default();
        let queued_len = state
            .midi_events
            .try_lock()
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
                state.midi_in_peak.store(0.0f32.to_bits(), Ordering::Relaxed);
                state.midi_out_peak.store(0.0f32.to_bits(), Ordering::Relaxed);
                return;
            }
        } else {
            state.silent_blocks.store(0, Ordering::Relaxed);
        }

        let mut track_processed = false;

        MIX_TEMP.with(|mix_cell| {
            FX_TEMP.with(|fx_cell| {
                HOST_OUTPUT_TEMP.with(|host_out_cell| {
                    let mut temp = mix_cell.borrow_mut();
                    if temp.len() != output.len() {
                        temp.resize(output.len(), 0.0);
                    }
                    temp.fill(0.0);

                    let learned_map = state.learned_cc.try_lock().ok();

                    REMAINING_PARAMS_TMP.with(|cell| cell.borrow_mut().clear());
                    FILTERED_EVENTS_TMP.with(|cell| cell.borrow_mut().clear());
                    let mut has_note_on = false;

                    PERF_EVENTS_TMP.with(|perf_cell| {
                        let mut performance_events = perf_cell.borrow_mut();
                        performance_events.clear();
                        if let Some(runtime) = active_performance.as_ref() {
                            collect_performance_block_events_into(
                                runtime,
                                block_start,
                                block_end,
                                samples_per_beat,
                                &mut performance_events,
                            );
                        }

                        if state.treesynth_enabled.load(Ordering::Relaxed) {
                            let (processed, events) = mix_treesynth_block(
                                &mut temp,
                                channels,
                                sample_rate,
                                block_start,
                                block_end,
                                samples_per_beat,
                                panic_notes,
                                loop_wrapped,
                                &notes,
                                &performance_events,
                                state,
                                audio_cache,
                            );
                            if processed {
                                track_processed = true;
                            }
                            FILTERED_EVENTS_TMP.with(|cell| cell.borrow_mut().extend(events));
                        }

                        if let Some(host) = state.host.as_ref() {
                            EVENTS_TMP.with(|events_cell| {
                                let mut events = events_cell.borrow_mut();
                                collect_block_events_into(
                                    &notes,
                                    block_start,
                                    block_end,
                                    samples_per_beat,
                                    &mut events,
                                );
                                events.extend(performance_events.iter().copied());

                                if panic_notes {
                                    for channel in 0u8..16 {
                                        events.push(vst3::MidiEvent::control_change(channel, 120, 0));
                                        events.push(vst3::MidiEvent::control_change(channel, 123, 0));
                                    }
                                    for channel in 0u8..16 {
                                        events.extend(
                                            (0u8..=127).map(|note| {
                                                vst3::MidiEvent::note_off_at(channel, note, 0, 0)
                                            }),
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
                                    events.extend(
                                        (0u8..=127).map(|note| vst3::MidiEvent::note_off(0, note, 0)),
                                    );
                                }
                                if let Ok(mut queued) = state.midi_events.try_lock() {
                                    events.extend(queued.drain(..));
                                }

                                has_note_on = events
                                    .iter()
                                    .any(|event| matches!(event, vst3::MidiEvent::NoteOn { .. }));

                                if let Ok(mut pending) = state.pending_param_changes.try_lock() {
                                    REMAINING_PARAMS_TMP.with(|remaining_cell| {
                                        let mut remaining_params = remaining_cell.borrow_mut();
                                        for pending in pending.drain(..) {
                                            match pending.target {
                                                PendingParamTarget::Instrument => {
                                                    host.push_param_change(pending.param_id, pending.value);
                                                }
                                                PendingParamTarget::Effect(_) => {
                                                    remaining_params.push(pending);
                                                }
                                            }
                                        }
                                    });
                                }

                                for lane in &automation {
                                    if let Some(value) =
                                        DawApp::automation_value_at(&lane.points, block_beat)
                                    {
                                        if lane.target == AutomationTarget::Instrument {
                                            host.push_param_change(lane.param_id, value as f64);
                                        }
                                    }
                                }

                                FILTERED_EVENTS_TMP.with(|filtered_cell| {
                                    let mut filtered_events = filtered_cell.borrow_mut();
                                    filtered_events.clear();

                                    for event in events.drain(..) {
                                        match event {
                                            vst3::MidiEvent::ControlChange {
                                                channel,
                                                controller,
                                                value,
                                            } => {
                                                if controller >= 120 {
                                                    filtered_events.push(event);
                                                    continue;
                                                }
                                                if let Some(learned_map) = learned_map.as_ref() {
                                                    if let Some(param_id) =
                                                        learned_map.get(&(channel, controller))
                                                    {
                                                        let norm =
                                                            (value as f64 / 127.0).clamp(0.0, 1.0);
                                                        host.push_param_change(*param_id, norm);
                                                    } else {
                                                        filtered_events.push(event);
                                                    }
                                                } else {
                                                    filtered_events.push(event);
                                                }
                                            }
                                            _ => filtered_events.push(event),
                                        }
                                    }

                                    let host_out_channels = host
                                        .io_channels()
                                        .1
                                        .max(channels)
                                        .clamp(1, MAX_PLUGIN_OUTPUT_CHANNELS);

                                    let mut host_output_temp = host_out_cell.borrow_mut();
                                    let total_samples = frames * host_out_channels;
                                    if host_output_temp.len() != total_samples {
                                        host_output_temp.resize(total_samples, 0.0);
                                    }

                                    let process_res = host
                                        .process_f32(
                                            &mut host_output_temp,
                                            host_out_channels,
                                            &filtered_events,
                                        );
                                    if process_res.is_ok() {
                                        for frame in 0..frames {
                                            let src_base = frame * host_out_channels;
                                            let dst_base = frame * channels;
                                            for ch in 0..channels {
                                                let src_ch = if host_out_channels == 1 {
                                                    0
                                                } else {
                                                    ch.min(host_out_channels - 1)
                                                };
                                                temp[dst_base + ch] +=
                                                    host_output_temp[src_base + src_ch];
                                            }
                                        }
                                        
                                        _host_out_buf[..total_samples].copy_from_slice(&host_output_temp[..total_samples]);
                                        *_host_out_ch = host_out_channels;
                                        *host_out_active = true;
                                        track_processed = true;
                                    } else {
                                        PLUGIN_PROCESS_FAILURES.fetch_add(
                                            1,
                                            Ordering::Relaxed,
                                        );
                                    }

                                    if panic_notes && !has_note_on {
                                        temp.fill(0.0);
                                    }
                                });
                            });
                        }
                    });

                    if panic_notes && !has_note_on {
                        temp.fill(0.0);
                    }

                FILTERED_EVENTS_TMP.with(|cell| {
                    let filtered_events = cell.borrow();
                    if !filtered_events.is_empty() {
                        let midi_level =
                            (filtered_events.len() as f32 / 16.0).clamp(0.0, 1.0);
                        state
                            .midi_in_peak
                            .store(midi_level.to_bits(), Ordering::Relaxed);
                        state
                            .midi_out_peak
                            .store(midi_level.to_bits(), Ordering::Relaxed);
                    } else {
                        state
                            .midi_in_peak
                            .store(0.0f32.to_bits(), Ordering::Relaxed);
                        state
                            .midi_out_peak
                            .store(0.0f32.to_bits(), Ordering::Relaxed);
                    }
                });

                for (clip, data) in clips {
                    mix_clip_resample(&mut temp, channels, clip, data, block_start, block_end, sample_rate);
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
                        mix_clip_resample(&mut temp, channels, &clip, &data, block_start, block_end, sample_rate);
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

                    let bypass_guard = state.effect_bypass.try_lock();
                    let bypass = bypass_guard.as_ref().map(|g| g.as_slice()).unwrap_or(&[]);
                    REMAINING_PARAMS_TMP.with(|remaining_cell| {
                        let remaining_params = remaining_cell.borrow();
                        CIN_PEAKS_TMP.with(|cell| {
                            let mut cin_peaks = cell.borrow_mut();
                            cin_peaks.clear();
                            COUT_PEAKS_TMP.with(|cell| {
                                let mut cout_peaks = cell.borrow_mut();
                                cout_peaks.clear();

                                for (fx_index, fx) in state.effect_hosts.iter().enumerate() {
                                    let is_bypassed =
                                        bypass.get(fx_index).copied().unwrap_or(false);
                                    if is_bypassed {
                                        cin_peaks.push(0.0);
                                        cout_peaks.push(0.0);
                                        continue;
                                    }

                                    let mut in_peak = 0.0f32;
                                    for sample in current.iter() {
                                        in_peak = in_peak.max(sample.abs());
                                    }
                                    cin_peaks.push(in_peak);

                                    for pending in remaining_params.iter() {
                                        if let PendingParamTarget::Effect(target_fx) = pending.target
                                        {
                                            if target_fx == fx_index {
                                                fx.push_param_change(pending.param_id, pending.value);
                                            }
                                        }
                                    }
                                    for lane in &automation {
                                        if let Some(value) = DawApp::automation_value_at(
                                            &lane.points,
                                            block_beat,
                                        ) {
                                            if let AutomationTarget::Effect(target_fx) = lane.target {
                                                if target_fx == fx_index {
                                                    fx.push_param_change(lane.param_id, value as f64);
                                                }
                                            }
                                        }
                                    }

                                    scratch.fill(0.0);
                                    if fx
                                        .process_f32_with_input(current, scratch, channels, &[])
                                        .is_ok()
                                    {
                                        std::mem::swap(&mut current, &mut scratch);
                                        uses_scratch = !uses_scratch;
                                        track_processed = true;
                                    }

                                    let mut out_peak = 0.0f32;
                                    for sample in current.iter() {
                                        out_peak = out_peak.max(sample.abs());
                                    }
                                    cout_peaks.push(out_peak);
                                }

                                if let Ok(mut cp) = state.fx_in_peaks.try_lock() {
                                    if cp.len() != cin_peaks.len() {
                                        cp.resize(cin_peaks.len(), 0.0);
                                    }
                                    cp.copy_from_slice(&cin_peaks);
                                }
                                if let Ok(mut cp) = state.fx_out_peaks.try_lock() {
                                    if cp.len() != cout_peaks.len() {
                                        cp.resize(cout_peaks.len(), 0.0);
                                    }
                                    cp.copy_from_slice(&cout_peaks);
                                }
                            });
                        });
                    });
                }

                if uses_scratch {
                    temp.copy_from_slice(&fx_temp_buf);
                }

                let (mut peak_l, mut peak_r) = (0.0f32, 0.0f32);
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
                state.peak_bits.store(peak_l.max(peak_r).to_bits(), Ordering::Relaxed);

                if track_processed {
                    processed_any_atomic.store(true, Ordering::Relaxed);
                }

                if let Ok(mut track_out_mutex) = state.track_buffer.try_lock() {
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
});

    let processed_any = processed_any_atomic.load(Ordering::Relaxed);

    for (route_index, route) in runtime_buffers.routes_snapshot.iter().enumerate() {
        if !route.enabled || route.kind != NodeRouteKind::AudioSidechain {
            continue;
        }
        if route.from_track >= track_count || route.to_track >= track_count || route.from_track == route.to_track {
            continue;
        }
        let threshold = db_to_gain(route.sidechain_threshold_db.clamp(-60.0, 0.0));
        let amount = route.sidechain_amount.clamp(0.0, 1.0);
        let attack = (route.sidechain_attack_ms.max(0.1) / 1000.0).max(0.0001);
        let release = (route.sidechain_release_ms.max(0.1) / 1000.0).max(0.0001);
        let attack_coeff = (-1.0f32 / (attack * sample_rate.max(1.0))).exp();
        let release_coeff = (-1.0f32 / (release * sample_rate.max(1.0))).exp();

        let source_pair = route.source_output_pair;

        let from_state = &track_audio[route.from_track];
        let to_state = &track_audio[route.to_track];
        
        // Locked access to buffers for sidechain processing
        let from_buf_guard = from_state.track_buffer.try_lock();
        let to_buf_guard = to_state.track_buffer.try_lock();
        
        if let (Ok(source), Ok(mut target)) = (from_buf_guard, to_buf_guard) {
            let mut gain = runtime_buffers.sidechain_states.get(route_index).copied().unwrap_or(1.0);
            for frame in 0..frames {
                let base = frame * channels;
                let detector = if runtime_buffers.track_host_output_active[route.from_track] {
                    let host_buf = &runtime_buffers.track_host_outputs[route.from_track];
                    let host_channels = runtime_buffers.track_host_output_channels[route.from_track];
                    let src_base = frame * host_channels;
                    let ch0 = (source_pair * 2).min(host_channels.saturating_sub(1));
                    let ch1 = (ch0 + 1).min(host_channels.saturating_sub(1));
                    host_buf.get(src_base + ch0).copied().unwrap_or(0.0).abs().max(
                        host_buf.get(src_base + ch1).copied().unwrap_or(0.0).abs(),
                    )
                } else {
                    let mut v = 0.0f32;
                    for ch in 0..channels {
                        v = v.max(source.as_slice().get(base + ch).copied().unwrap_or(0.0).abs());
                    }
                    v
                };
                let target_gain = if detector > threshold {
                    let over = ((detector - threshold) / (1.0 - threshold).max(1e-6)).clamp(0.0, 1.0);
                    (1.0 - amount * over).clamp(0.05f32, 1.0f32)
                } else {
                    1.0
                };
                if target_gain < gain {
                    gain = attack_coeff * (gain - target_gain) + target_gain;
                } else {
                    gain = release_coeff * (gain - target_gain) + target_gain;
                }
                for ch in 0..channels {
                    if let Some(sample) = target.as_mut_slice().get_mut(base + ch) {
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
        if let Ok(buf) = state.track_buffer.try_lock() {
            for (out, sample) in output.iter_mut().zip(buf.iter()) {
                *out += *sample;
            }
        }
    }

    if let Ok(mut activity) = node_activity.try_lock() {
        if activity.len() < track_count {
            activity.resize(track_count, TrackNodeActivity::default());
        }
        let activity_slice = &mut activity[..track_count];
        let h_active_slice = &runtime_buffers.track_host_output_active[..track_count];
        let h_outputs_slice = &runtime_buffers.track_host_outputs[..track_count];
        let h_chans_slice = &runtime_buffers.track_host_output_channels[..track_count];

        activity_slice.par_iter_mut()
            .zip(track_audio.par_iter())
            .zip(h_active_slice.par_iter())
            .zip(h_outputs_slice.par_iter())
            .zip(h_chans_slice.par_iter())
            .for_each(|((((slot, state), host_active), host_buf), hc)| {
                let host_active = *host_active;
                let hc = (*hc).max(1);
                let mut pair_peaks = [0.0f32; 8];

                if host_active {
                    for frame in 0..frames {
                        let base = frame * hc;
                        for pair in 0..8 {
                            let ch0 = pair * 2;
                            if ch0 < hc {
                                pair_peaks[pair] = pair_peaks[pair].max(host_buf.get(base + ch0).copied().unwrap_or(0.0).abs());
                            }
                        }
                    }
                } else if let Ok(track_buf) = state.track_buffer.try_lock() {
                    for frame in 0..frames {
                        let base = frame * channels;
                        let mut detector = 0.0f32;
                        for ch in 0..channels {
                            detector = detector.max(track_buf.get(base + ch).copied().unwrap_or(0.0).abs());
                        }
                        pair_peaks[0] = pair_peaks[0].max(detector);
                    }
                }

                for pair in 0..8 {
                    slot.output_pair_peaks[pair] = (slot.output_pair_peaks[pair] * 0.82).max(pair_peaks[pair]);
                }
                
                if let Ok(cin) = state.fx_in_peaks.try_lock() {
                    if slot.fx_input_peaks.len() != cin.len() { slot.fx_input_peaks.resize(cin.len(), 0.0); }
                    for (i, v) in cin.iter().enumerate() {
                        slot.fx_input_peaks[i] = (slot.fx_input_peaks[i] * 0.82).max(*v);
                    }
                }
                if let Ok(cout) = state.fx_out_peaks.try_lock() {
                    if slot.fx_output_peaks.len() != cout.len() { slot.fx_output_peaks.resize(cout.len(), 0.0); }
                    for (i, v) in cout.iter().enumerate() {
                        slot.fx_output_peaks[i] = (slot.fx_output_peaks[i] * 0.82).max(*v);
                    }
                }
                
                slot.midi_in = (slot.midi_in * 0.78).max(f32::from_bits(state.midi_in_peak.load(Ordering::Relaxed)));
                slot.midi_out = (slot.midi_out * 0.78).max(f32::from_bits(state.midi_out_peak.load(Ordering::Relaxed)));
            });
    }
    processed_any
}

pub(crate) fn mix_clip_resample(
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
}

#[cfg(all(windows, has_rubberband))]
pub(crate) fn mix_clip_stretch(
    temp: &mut [f32],
    channels: usize,
    clip: &AudioClipRender,
    data: &AudioClipData,
    block_start: u64,
    block_end: u64,
    sample_rate: f32,
    stretcher: &Arc<Mutex<RubberBandClipState>>,
) {
    let clip_end = clip.start_samples + clip.length_samples;
    if block_end <= clip.start_samples || block_start >= clip_end {
        return;
    }
    let start_in_block = block_start.max(clip.start_samples) - block_start;
    let end_in_block = block_end.min(clip_end) - block_start;
    let frames_needed = (end_in_block - start_in_block) as usize;
    if frames_needed == 0 {
        return;
    }
    let src_channels = data.channels.max(1);
    let src_frames = data.samples.len() / src_channels;
    if src_frames == 0 {
        return;
    }
    let rate_ratio = data.sample_rate as f64 / sample_rate.max(1.0) as f64;
    let time_mul = clip.time_mul.max(0.01) as f64;
    let mut state = match stretcher.lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };
    if state.state.is_null() {
        drop(state);
        mix_clip_resample(
            temp,
            channels,
            clip,
            data,
            block_start,
            block_end,
            sample_rate,
        );
        return;
    }
    if state.needs_reposition {
        state.reset_stream();
        state.needs_reposition = false;
    }
    let block_size = state.block_size.max(64);
    for ch in 0..channels {
        state.input_buffers[ch].resize(block_size, 0.0);
        state.output_buffers[ch].resize(block_size, 0.0);
    }

    let mut frames_done = 0usize;
    while frames_done < frames_needed {
        let chunk = (frames_needed - frames_done).min(block_size);
        for ch in 0..channels {
            for i in 0..block_size {
                state.input_buffers[ch][i] = 0.0;
            }
        }
        for i in 0..chunk {
            let clip_pos = start_in_block + frames_done as u64 + i as u64 + block_start - clip.start_samples;
            let pos = ((clip_pos as f64 + clip.offset_samples as f64) * rate_ratio / time_mul).max(0.0);
            let src_pos = if src_frames > 0 {
                pos.rem_euclid(src_frames as f64)
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
                state.input_buffers[ch][i] = s0 + (s1 - s0) * frac;
            }
        }
        let input_ptrs: Vec<*const f32> = state.input_buffers.iter().map(|b| b.as_ptr()).collect();
        let mut output_ptrs: Vec<*mut f32> =
            state.output_buffers.iter_mut().map(|b| b.as_mut_ptr()).collect();
        unsafe {
            rubberband::rubberband_live_shift(state.state, input_ptrs.as_ptr(), output_ptrs.as_mut_ptr());
        }
        for i in 0..chunk {
            let out_index = (start_in_block as usize + frames_done + i) * channels;
            for ch in 0..channels {
                if out_index + ch < temp.len() {
                    temp[out_index + ch] += state.output_buffers[ch][i] * clip.gain;
                }
            }
        }
        frames_done += chunk;
    }
}

pub(crate) fn apply_fade_in_if_needed(samples: &mut [f32], channels: usize, flag: &AtomicBool) {
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

pub(crate) fn update_master_peak_f32(output: &[f32], peak_bits: &AtomicU32) {
    let mut peak = 0.0f32;
    for sample in output {
        let value = sample.abs();
        if value > peak {
            peak = value;
        }
    }
    peak_bits.store(peak.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
}

pub(crate) fn update_master_peak_i16(output: &[i16], peak_bits: &AtomicU32) {
    let mut peak = 0.0f32;
    for sample in output {
        let value = (*sample as f32 / i16::MAX as f32).abs();
        if value > peak {
            peak = value;
        }
    }
    peak_bits.store(peak.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
}

pub(crate) fn update_master_peak_u16(output: &[u16], peak_bits: &AtomicU32) {
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



pub(crate) fn render_sine<T: cpal::Sample + cpal::FromSample<f32>>(
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
