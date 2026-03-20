use super::*;

pub(crate) fn wav_spec_for_depth(
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

pub(crate) fn sample_to_int(sample: f32, bits: u16) -> i32 {
    let max = (1i64 << (bits.saturating_sub(1))) - 1;
    let min = -(1i64 << (bits.saturating_sub(1)));
    let scaled = (sample.clamp(-1.0, 1.0) * max as f32).round() as i64;
    scaled.clamp(min, max) as i32
}

pub(crate) fn write_wav_samples<W: std::io::Write + std::io::Seek>(
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

pub(crate) fn render_plan_to_wav(
    plan: RenderPlan,
    done: &AtomicU64,
    total: &AtomicU64,
    track_audio: &[TrackAudioState],
    audio_clip_cache: &Arc<Mutex<AudioClipCache>>,
) -> Result<(), String> {
    reset_treesynth_runtime_for_plan(&plan, track_audio);
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
    let tmp_path = format!("{}.tmp", plan.path);
    let file = std::fs::File::create(&tmp_path).map_err(|e| e.to_string())?;
    let mut writer = hound::WavWriter::new(file, spec).map_err(|e| e.to_string())?;

    let mut track_hosts: Vec<(RenderTrack, Option<RenderHost>, Vec<RenderHost>)> = Vec::new();
    if !plan.tracks.is_empty() {
        for track in plan.tracks {
            if !track.active {
                track_hosts.push((track, None, Vec::new()));
                continue;
            }
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
            source_track_index: None,
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
        } else if !track.param_ids.is_empty() {
            for (param_id, value) in track.param_ids.iter().zip(track.param_values.iter()) {
                host.push_param_change(*param_id, *value as f64);
            }
        }
    }

    render_send_midi_stop(&mut track_hosts, channels as usize, plan.block_size);
    render_warmup_hosts(&mut track_hosts, channels as usize, plan.block_size, 2);

    let mut per_track_clips: Vec<Vec<(AudioClipRender, Arc<AudioClipData>)>> =
        vec![Vec::new(); track_hosts.len()];
    for clip in plan.audio_clips.iter() {
        if clip.track_index >= per_track_clips.len() {
            continue;
        }
        let Some(data) = plan.audio_cache.get(&clip.path) else {
            continue;
        };
        per_track_clips[clip.track_index].push((clip.clone(), data.clone()));
    }

    let mut master_state = MasterCompState::default();
    let routes_snapshot = plan.node_routes.clone();
    let mut sidechain_states = vec![1.0f32; routes_snapshot.len()];
    let mut cursor = 0usize;

    // Reuse per-block buffers to avoid allocator churn during offline rendering.
    let mut output: Vec<f32> = Vec::new();
    let mut temp: Vec<f32> = Vec::new();
    let mut fx_temp: Vec<f32> = Vec::new();
    let mut track_buffers: Vec<Vec<f32>> =
        (0..track_hosts.len()).map(|_| Vec::<f32>::new()).collect();
    let mut track_host_outputs: Vec<Option<(Vec<f32>, usize)>> =
        vec![None; track_hosts.len()];

    while cursor < total_samples {
        let frames = (total_samples - cursor).min(plan.block_size);
        let block_start = start_samples + cursor as u64;
        let block_end = start_samples + (cursor + frames) as u64;

        let block_samples = frames * channels as usize;
        output.resize(block_samples, 0.0);
        output.fill(0.0);
        temp.resize(block_samples, 0.0);
        temp.fill(0.0);
        fx_temp.resize(block_samples, 0.0);
        fx_temp.fill(0.0);

        for buf in track_buffers.iter_mut() {
            buf.resize(block_samples, 0.0);
            buf.fill(0.0);
        }

        for slot in track_host_outputs.iter_mut() {
            *slot = None;
        }

        for (track_index, (track, host, fx_hosts)) in track_hosts.iter_mut().enumerate() {
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
            let is_treesynth = track.instrument_path.as_deref().map(|p| p.eq_ignore_ascii_case("native:treesynth")).unwrap_or(false);
            if is_treesynth {
                let source_index = track.source_track_index.unwrap_or(track_index);
                if let Some(track_audio) = track_audio.get(source_index) {
                    let (_processed, treesynth_events) = mix_treesynth_block(
                        &mut temp,
                        channels as usize,
                        plan.sample_rate as f32,
                        block_start,
                        block_end,
                        samples_per_beat,
                        false,
                        false,
                        &track.notes,
                        &[],
                        track_audio,
                        audio_clip_cache,
                    );
                    if !treesynth_events.is_empty() {
                        events = treesynth_events;
                    }
                }
            } else {
                if let Some(host) = host.as_mut() {
                    let host_out_channels = host
                        .io_channels()
                        .1
                        .max(channels as usize)
                        .clamp(1, MAX_PLUGIN_OUTPUT_CHANNELS);
                    let mut host_output = vec![0.0f32; frames * host_out_channels];
                    if host
                        .process_f32(&mut host_output, host_out_channels, &events)
                        .is_ok()
                    {
                        for frame in 0..frames {
                            let src_base = frame * host_out_channels;
                            let dst_base = frame * channels as usize;
                            for ch in 0..channels as usize {
                                let src_ch = if host_out_channels == 1 {
                                    0
                                } else {
                                    ch.min(host_out_channels - 1)
                                };
                                temp[dst_base + ch] += host_output[src_base + src_ch];
                            }
                        }
                        track_host_outputs[track_index] = Some((host_output, host_out_channels));
                    }
                }
            }
            if let Some(clips) = per_track_clips.get(track_index) {
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
                    let rate_ratio = data.sample_rate as f64 / plan.sample_rate as f64;
                    let time_mul = clip.time_mul.max(0.01) as f64;
                    let start_in_block = block_start.max(clip.start_samples) - block_start;
                    let end_in_block = block_end.min(clip_end) - block_start;
                    for i in start_in_block..end_in_block {
                        let clip_pos = i + block_start - clip.start_samples;
                        let pos =
                            ((clip_pos as f64 + clip.offset_samples as f64) * rate_ratio / time_mul)
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
                            if out_index < temp.len() {
                                temp[out_index] += sample * clip.gain;
                            }
                        }
                    }
                }
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
            if let Some(track_out) = track_buffers.get_mut(track_index) {
                for (out, sample) in track_out.iter_mut().zip(current.iter()) {
                    *out += *sample * level;
                }
            }
        }
        apply_render_sidechain_routes(
            &mut track_buffers,
            &track_host_outputs,
            &routes_snapshot,
            frames,
            channels as usize,
            plan.sample_rate as f32,
            &mut sidechain_states,
        );
        output.fill(0.0);
        for track_out in &track_buffers {
            for (out, sample) in output.iter_mut().zip(track_out.iter()) {
                *out += *sample;
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
    if let Some(comment) = plan.license_comment.as_deref() {
        let _ = append_wav_comment(&tmp_path, comment);
    }
    std::fs::rename(&tmp_path, &plan.path).map_err(|e| e.to_string())?;
    done.store(total_samples_u64, Ordering::Relaxed);
    Ok(())
}

pub(crate) fn load_render_host(
    path: &str,
    clap_id: Option<&str>,
    sample_rate: f64,
    block_size: usize,
    channels: usize,
    with_input: bool,
) -> Option<RenderHost> {
    match DawApp::plugin_kind_from_path(path) {
        PluginKind::Native => None,
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
                channels.min(MAX_CLAP_OUTPUT_CHANNELS),
            )
            .ok()
            .map(RenderHost::Clap)
        }
    }
}

pub(crate) fn render_send_midi_stop(
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

pub(crate) fn render_warmup_hosts(
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

pub(crate) fn render_plan_for_each_block<F>(
    plan: &RenderPlan,
    done: &AtomicU64,
    progress_offset: u64,
    track_audio: &[TrackAudioState],
    audio_clip_cache: &Arc<Mutex<AudioClipCache>>,
    mut on_block: F,
) -> Result<usize, String>
where
    F: FnMut(&[f32], usize) -> Result<(), String>,
{
    reset_treesynth_runtime_for_plan(plan, track_audio);
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
            if !track.active {
                track_hosts.push((track, None, Vec::new()));
                continue;
            }
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
            source_track_index: None,
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
        } else if !track.param_ids.is_empty() {
            for (param_id, value) in track.param_ids.iter().zip(track.param_values.iter()) {
                host.push_param_change(*param_id, *value as f64);
            }
        }
    }

    render_send_midi_stop(&mut track_hosts, channels as usize, plan.block_size);
    render_warmup_hosts(&mut track_hosts, channels as usize, plan.block_size, 2);

    let mut per_track_clips: Vec<Vec<(AudioClipRender, Arc<AudioClipData>)>> =
        vec![Vec::new(); track_hosts.len()];
    for clip in plan.audio_clips.iter() {
        if clip.track_index >= per_track_clips.len() {
            continue;
        }
        let Some(data) = plan.audio_cache.get(&clip.path) else {
            continue;
        };
        per_track_clips[clip.track_index].push((clip.clone(), data.clone()));
    }

    let mut master_state = MasterCompState::default();
    let routes_snapshot = plan.node_routes.clone();
    let mut sidechain_states = vec![1.0f32; routes_snapshot.len()];
    let mut cursor = 0usize;

    // Reuse per-block buffers to avoid allocator churn during offline rendering.
    let mut output: Vec<f32> = Vec::new();
    let mut temp: Vec<f32> = Vec::new();
    let mut fx_temp: Vec<f32> = Vec::new();
    let mut track_buffers: Vec<Vec<f32>> =
        (0..track_hosts.len()).map(|_| Vec::<f32>::new()).collect();
    let mut track_host_outputs: Vec<Option<(Vec<f32>, usize)>> =
        vec![None; track_hosts.len()];

    while cursor < total_samples {
        let frames = (total_samples - cursor).min(plan.block_size);
        let block_start = start_samples + cursor as u64;
        let block_end = start_samples + (cursor + frames) as u64;
        let block_samples = frames * channels as usize;

        output.resize(block_samples, 0.0);
        output.fill(0.0);
        temp.resize(block_samples, 0.0);
        temp.fill(0.0);
        fx_temp.resize(block_samples, 0.0);
        fx_temp.fill(0.0);

        for buf in track_buffers.iter_mut() {
            buf.resize(block_samples, 0.0);
            buf.fill(0.0);
        }

        for slot in track_host_outputs.iter_mut() {
            *slot = None;
        }
        for (track_index, (track, host, fx_hosts)) in track_hosts.iter_mut().enumerate() {
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
            let is_treesynth = track
                .instrument_path
                .as_deref()
                .map(|p| p.eq_ignore_ascii_case("native:treesynth"))
                .unwrap_or(false);
            if is_treesynth {
                let source_index = track.source_track_index.unwrap_or(track_index);
                if let Some(track_audio) = track_audio.get(source_index) {
                    let (_processed, treesynth_events) = mix_treesynth_block(
                        &mut temp,
                        channels as usize,
                        plan.sample_rate as f32,
                        block_start,
                        block_end,
                        samples_per_beat,
                        false,
                        false,
                        &track.notes,
                        &[],
                        track_audio,
                        audio_clip_cache,
                    );
                    if !treesynth_events.is_empty() {
                        events = treesynth_events;
                    }
                }
            } else if let Some(host) = host.as_mut() {
                let host_out_channels = host
                    .io_channels()
                    .1
                    .max(channels as usize)
                    .clamp(1, MAX_PLUGIN_OUTPUT_CHANNELS);
                let mut host_output = vec![0.0f32; frames * host_out_channels];
                if host
                    .process_f32(&mut host_output, host_out_channels, &events)
                    .is_ok()
                {
                    for frame in 0..frames {
                        let src_base = frame * host_out_channels;
                        let dst_base = frame * channels as usize;
                        for ch in 0..channels as usize {
                            let src_ch = if host_out_channels == 1 {
                                0
                            } else {
                                ch.min(host_out_channels - 1)
                            };
                            temp[dst_base + ch] += host_output[src_base + src_ch];
                        }
                    }
                    track_host_outputs[track_index] = Some((host_output, host_out_channels));
                }
            }
            if let Some(clips) = per_track_clips.get(track_index) {
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
                    let rate_ratio = data.sample_rate as f64 / plan.sample_rate as f64;
                    let time_mul = clip.time_mul.max(0.01) as f64;
                    let start_in_block = block_start.max(clip.start_samples) - block_start;
                    let end_in_block = block_end.min(clip_end) - block_start;
                    for i in start_in_block..end_in_block {
                        let clip_pos = i + block_start - clip.start_samples;
                        let pos =
                            ((clip_pos as f64 + clip.offset_samples as f64) * rate_ratio / time_mul)
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
                            if out_index < temp.len() {
                                temp[out_index] += sample * clip.gain;
                            }
                        }
                    }
                }
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
            if let Some(track_out) = track_buffers.get_mut(track_index) {
                for (out, sample) in track_out.iter_mut().zip(current.iter()) {
                    *out += *sample * level;
                }
            }
        }
        apply_render_sidechain_routes(
            &mut track_buffers,
            &track_host_outputs,
            &routes_snapshot,
            frames,
            channels as usize,
            plan.sample_rate as f32,
            &mut sidechain_states,
        );
        output.fill(0.0);
        for track_out in &track_buffers {
            for (out, sample) in output.iter_mut().zip(track_out.iter()) {
                *out += *sample;
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

pub(crate) fn render_plan_to_f32(
    plan: RenderPlan,
    done: &AtomicU64,
    total: &AtomicU64,
    track_audio: &[TrackAudioState],
    audio_clip_cache: &Arc<Mutex<AudioClipCache>>,
) -> Result<Vec<f32>, String> {
    reset_treesynth_runtime_for_plan(&plan, track_audio);
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
            if !track.active {
                track_hosts.push((track, None, Vec::new()));
                continue;
            }
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
            source_track_index: None,
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
        } else if !track.param_ids.is_empty() {
            for (param_id, value) in track.param_ids.iter().zip(track.param_values.iter()) {
                host.push_param_change(*param_id, *value as f64);
            }
        }
    }

    render_send_midi_stop(&mut track_hosts, channels as usize, plan.block_size);
    render_warmup_hosts(&mut track_hosts, channels as usize, plan.block_size, 2);

    let mut per_track_clips: Vec<Vec<(AudioClipRender, Arc<AudioClipData>)>> =
        vec![Vec::new(); track_hosts.len()];
    for clip in plan.audio_clips.iter() {
        if clip.track_index >= per_track_clips.len() {
            continue;
        }
        let Some(data) = plan.audio_cache.get(&clip.path) else {
            continue;
        };
        per_track_clips[clip.track_index].push((clip.clone(), data.clone()));
    }

    let mut output_all = Vec::with_capacity(total_samples * channels as usize);
    let mut master_state = MasterCompState::default();
    let routes_snapshot = plan.node_routes.clone();
    let mut sidechain_states = vec![1.0f32; routes_snapshot.len()];
    let mut cursor = 0usize;

    // Reuse per-block buffers to avoid allocator churn during offline rendering.
    let mut output: Vec<f32> = Vec::new();
    let mut temp: Vec<f32> = Vec::new();
    let mut fx_temp: Vec<f32> = Vec::new();
    let mut track_buffers: Vec<Vec<f32>> =
        (0..track_hosts.len()).map(|_| Vec::<f32>::new()).collect();
    let mut track_host_outputs: Vec<Option<(Vec<f32>, usize)>> =
        vec![None; track_hosts.len()];

    while cursor < total_samples {
        let frames = (total_samples - cursor).min(plan.block_size);
        let block_start = start_samples + cursor as u64;
        let block_end = start_samples + (cursor + frames) as u64;
        let block_samples = frames * channels as usize;

        output.resize(block_samples, 0.0);
        output.fill(0.0);
        temp.resize(block_samples, 0.0);
        temp.fill(0.0);
        fx_temp.resize(block_samples, 0.0);
        fx_temp.fill(0.0);

        for buf in track_buffers.iter_mut() {
            buf.resize(block_samples, 0.0);
            buf.fill(0.0);
        }

        for slot in track_host_outputs.iter_mut() {
            *slot = None;
        }
        for (track_index, (track, host, fx_hosts)) in track_hosts.iter_mut().enumerate() {
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
            let mut events = collect_block_events(&track.notes, block_start, block_end, samples_per_beat);
            let is_treesynth = track
                .instrument_path
                .as_deref()
                .map(|p| p.eq_ignore_ascii_case("native:treesynth"))
                .unwrap_or(false);
            if is_treesynth {
                let source_index = track.source_track_index.unwrap_or(track_index);
                if let Some(track_audio) = track_audio.get(source_index) {
                    let (_processed, treesynth_events) = mix_treesynth_block(
                        &mut temp,
                        channels as usize,
                        plan.sample_rate as f32,
                        block_start,
                        block_end,
                        samples_per_beat,
                        false,
                        false,
                        &track.notes,
                        &[],
                        track_audio,
                        audio_clip_cache,
                    );
                    if !treesynth_events.is_empty() {
                        events = treesynth_events;
                    }
                }
            } else if let Some(host) = host.as_mut() {
                let host_out_channels = host
                    .io_channels()
                    .1
                    .max(channels as usize)
                    .clamp(1, MAX_PLUGIN_OUTPUT_CHANNELS);
                let mut host_output = vec![0.0f32; frames * host_out_channels];
                if host
                    .process_f32(&mut host_output, host_out_channels, &events)
                    .is_ok()
                {
                    for frame in 0..frames {
                        let src_base = frame * host_out_channels;
                        let dst_base = frame * channels as usize;
                        for ch in 0..channels as usize {
                            let src_ch = if host_out_channels == 1 {
                                0
                            } else {
                                ch.min(host_out_channels - 1)
                            };
                            temp[dst_base + ch] += host_output[src_base + src_ch];
                        }
                    }
                    track_host_outputs[track_index] = Some((host_output, host_out_channels));
                }
            }
            if let Some(clips) = per_track_clips.get(track_index) {
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
                    let rate_ratio = data.sample_rate as f64 / plan.sample_rate as f64;
                    let time_mul = clip.time_mul.max(0.01) as f64;
                    let start_in_block = block_start.max(clip.start_samples) - block_start;
                    let end_in_block = block_end.min(clip_end) - block_start;
                    for i in start_in_block..end_in_block {
                        let clip_pos = i + block_start - clip.start_samples;
                        let pos =
                            ((clip_pos as f64 + clip.offset_samples as f64) * rate_ratio / time_mul)
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
                            if out_index < temp.len() {
                                temp[out_index] += sample * clip.gain;
                            }
                        }
                    }
                }
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
            if let Some(track_out) = track_buffers.get_mut(track_index) {
                for (out, sample) in track_out.iter_mut().zip(current.iter()) {
                    *out += *sample * level;
                }
            }
        }
        apply_render_sidechain_routes(
            &mut track_buffers,
            &track_host_outputs,
            &routes_snapshot,
            frames,
            channels as usize,
            plan.sample_rate as f32,
            &mut sidechain_states,
        );
        output.fill(0.0);
        for track_out in &track_buffers {
            for (out, sample) in output.iter_mut().zip(track_out.iter()) {
                *out += *sample;
            }
        }
        apply_master_processing(
            &mut output,
            channels as usize,
            plan.sample_rate as f32,
            &plan.master_settings,
            &mut master_state,
        );
        output_all.extend_from_slice(&output);
        cursor += frames;
        done.store(cursor as u64, Ordering::Relaxed);
    }

    done.store(total_samples_u64, Ordering::Relaxed);
    Ok(output_all)
}

pub(crate) fn render_plan_to_ogg(
    plan: RenderPlan,
    done: &AtomicU64,
    total: &AtomicU64,
    track_audio: &[TrackAudioState],
    audio_clip_cache: &Arc<Mutex<AudioClipCache>>,
) -> Result<(), String> {
    let path = plan.path.clone();
    let sample_rate = plan.sample_rate;
    let bitrate = plan.bitrate_kbps;
    let mut samples = render_plan_to_f32(plan, done, total, track_audio, audio_clip_cache)?;
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
    // OGG path encodes 16-bit PCM, so add headroom to avoid hard clipping crackle
    // when the offline mix/master briefly exceeds 0 dBFS.
    let peak = samples
        .iter()
        .fold(0.0f32, |acc, s| acc.max(s.abs()));
    let target_peak = 0.95f32;
    let gain = if peak > target_peak {
        target_peak / peak
    } else {
        1.0
    };
    if gain < 1.0 {
        eprintln!(
            "OGG pre-gain applied: peak={:.4} gain={:.4}",
            peak,
            gain
        );
    }

    let mut pcm_i16 = Vec::with_capacity(samples.len());
    for sample in samples.drain(..) {
        let scaled = (sample * gain).clamp(-1.0, 1.0);
        let value = (scaled * i16::MAX as f32).round() as i16;
        pcm_i16.push(value);
    }
    let data = encoder
        .encode(&pcm_i16)
        .map_err(|e| format!("Vorbis encode failed: {e}"))?;
    let tail = encoder
        .flush()
        .map_err(|e| format!("Vorbis flush failed: {e}"))?;
    let tmp_path = format!("{}.tmp", path);
    let mut file = std::fs::File::create(&tmp_path).map_err(|e| e.to_string())?;
    use std::io::Write;
    file.write_all(&data).map_err(|e| e.to_string())?;
    file.write_all(&tail).map_err(|e| e.to_string())?;
    drop(file);
    std::fs::rename(&tmp_path, &path).map_err(|e| e.to_string())?;
    Ok(())
}

pub(crate) fn apply_render_sidechain_routes(
    track_buffers: &mut [Vec<f32>],
    track_host_outputs: &[Option<(Vec<f32>, usize)>],
    routes: &[NodeRouteLink],
    frames: usize,
    channels: usize,
    sample_rate: f32,
    sidechain_states: &mut Vec<f32>,
) {
    if channels == 0 || frames == 0 || track_buffers.is_empty() || routes.is_empty() {
        return;
    }
    if sidechain_states.len() < routes.len() {
        sidechain_states.resize(routes.len(), 1.0);
    } else if sidechain_states.len() > routes.len() {
        sidechain_states.truncate(routes.len());
    }
    for (route_index, route) in routes.iter().enumerate() {
        if !route.enabled || route.kind != NodeRouteKind::AudioSidechain {
            continue;
        }
        if route.from_track >= track_buffers.len()
            || route.to_track >= track_buffers.len()
            || route.from_track == route.to_track
        {
            continue;
        }
        let threshold = db_to_gain(route.sidechain_threshold_db.clamp(-60.0, 0.0));
        let amount = route.sidechain_amount.clamp(0.0, 1.0);
        let attack = (route.sidechain_attack_ms.max(0.1) / 1000.0).max(0.0001);
        let release = (route.sidechain_release_ms.max(0.1) / 1000.0).max(0.0001);
        let attack_coeff = (-1.0 / (attack * sample_rate.max(1.0))).exp();
        let release_coeff = (-1.0 / (release * sample_rate.max(1.0))).exp();

        let source_pair = route.source_output_pair;
        let (source, target): (&[f32], &mut [f32]) = if route.from_track < route.to_track {
            let (left, right) = track_buffers.split_at_mut(route.to_track);
            (&left[route.from_track], &mut right[0])
        } else {
            let (left, right) = track_buffers.split_at_mut(route.from_track);
            (&right[0], &mut left[route.to_track])
        };

        let mut gain = sidechain_states.get(route_index).copied().unwrap_or(1.0);
        for frame in 0..frames {
            let base = frame * channels;
            let detector = if let Some((host_buf, host_channels)) =
                track_host_outputs.get(route.from_track).and_then(|entry| entry.as_ref())
            {
                let src_base = frame * *host_channels;
                let ch0 = (source_pair * 2).min(host_channels.saturating_sub(1));
                let ch1 = (ch0 + 1).min(host_channels.saturating_sub(1));
                host_buf
                    .get(src_base + ch0)
                    .copied()
                    .unwrap_or(0.0)
                    .abs()
                    .max(host_buf.get(src_base + ch1).copied().unwrap_or(0.0).abs())
            } else {
                let mut v = 0.0f32;
                for ch in 0..channels {
                    v = v.max(source.get(base + ch).copied().unwrap_or(0.0).abs());
                }
                v
            };
            let target_gain = if detector > threshold {
                let over = ((detector - threshold) / (1.0 - threshold).max(1e-6)).clamp(0.0, 1.0);
                (1.0 - amount * over).clamp(0.05, 1.0)
            } else {
                1.0
            };
            if target_gain < gain {
                gain = attack_coeff * (gain - target_gain) + target_gain;
            } else {
                gain = release_coeff * (gain - target_gain) + target_gain;
            }
            for ch in 0..channels {
                if let Some(sample) = target.get_mut(base + ch) {
                    *sample *= gain;
                }
            }
        }
        if let Some(state) = sidechain_states.get_mut(route_index) {
            *state = gain;
        }
    }
}

pub(crate) fn reset_treesynth_runtime_for_plan(plan: &RenderPlan, track_audio: &[TrackAudioState]) {
    let mut reset_indices = HashSet::new();
    for (track_index, track) in plan.tracks.iter().enumerate() {
        let is_treesynth = track
            .instrument_path
            .as_deref()
            .map(|p| p.eq_ignore_ascii_case("native:treesynth"))
            .unwrap_or(false);
        if !is_treesynth {
            continue;
        }
        let source_index = track.source_track_index.unwrap_or(track_index);
        reset_indices.insert(source_index);
    }
    for index in reset_indices {
        if let Some(state) = track_audio.get(index) {
            if let Ok(mut runtime) = state.treesynth_runtime.lock() {
                runtime.voices.clear();
                runtime.sequence_index = 0;
                runtime.last_note = None;
                runtime.rng_state = 0x9E3779B97F4A7C15;
            }
        }
    }
}

pub(crate) fn render_plan_to_flac(
    mut plan: RenderPlan,
    done: &AtomicU64,
    total: &AtomicU64,
    track_audio: &[TrackAudioState],
    audio_clip_cache: &Arc<Mutex<AudioClipCache>>,
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
        track_audio,
        audio_clip_cache,
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

    let tmp_path = format!("{}.tmp", path);
    let mut file = std::fs::File::create(&tmp_path).map_err(|e| e.to_string())?;
    use std::io::Write;
    if let Some(comment) = plan.license_comment.as_deref() {
        let mut header = sink.as_slice().to_vec();
        if let Some(first) = header.get_mut(0) {
            *first &= 0x7f;
        }
        file.write_all(&header).map_err(|e| e.to_string())?;
        let block = build_flac_comment_block(comment);
        file.write_all(&block).map_err(|e| e.to_string())?;
    } else {
        file.write_all(sink.as_slice()).map_err(|e| e.to_string())?;
    }

    let mut framebuf = FrameBuf::with_size(channels, block_size)
        .map_err(|e| format!("FLAC frame buffer error: {e:?}"))?;
    let mut frame_number = 0usize;
    render_plan_for_each_block(
        &plan,
        done,
        total_samples as u64,
        track_audio,
        audio_clip_cache,
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
    drop(file);
    std::fs::rename(&tmp_path, &path).map_err(|e| e.to_string())?;
    Ok(())
}

pub(crate) fn build_wav_info_chunk(comment: &str) -> Vec<u8> {
    let mut info = Vec::new();
    info.extend_from_slice(b"INFO");
    info.extend_from_slice(b"ICMT");
    let mut text = comment.as_bytes().to_vec();
    text.push(0);
    let text_len = text.len() as u32;
    info.extend_from_slice(&text_len.to_le_bytes());
    info.extend_from_slice(&text);
    if text_len % 2 == 1 {
        info.push(0);
    }
    let list_size = info.len() as u32;
    let mut chunk = Vec::new();
    chunk.extend_from_slice(b"LIST");
    chunk.extend_from_slice(&list_size.to_le_bytes());
    chunk.extend_from_slice(&info);
    chunk
}

pub(crate) fn append_wav_comment(path: &str, comment: &str) -> Result<(), String> {
    use std::fs::OpenOptions;
    use std::io::{Seek, SeekFrom, Write};

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    let file_len = file.metadata().map_err(|e| e.to_string())?.len();
    if file_len < 12 {
        return Err("Invalid WAV file".to_string());
    }
    let chunk = build_wav_info_chunk(comment);
    let new_len = file_len.saturating_add(chunk.len() as u64);
    if new_len > u32::MAX as u64 + 8 {
        return Err("WAV too large for metadata".to_string());
    }
    let riff_size = (new_len - 8) as u32;
    file.seek(SeekFrom::Start(4))
        .map_err(|e| e.to_string())?;
    file.write_all(&riff_size.to_le_bytes())
        .map_err(|e| e.to_string())?;
    file.seek(SeekFrom::End(0))
        .map_err(|e| e.to_string())?;
    file.write_all(&chunk).map_err(|e| e.to_string())?;
    Ok(())
}

pub(crate) fn build_flac_comment_block(comment: &str) -> Vec<u8> {
    let vendor = "LingStation";
    let mut payload = Vec::new();
    payload.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    payload.extend_from_slice(vendor.as_bytes());
    let comment_entry = format!("COMMENT={}", comment);
    payload.extend_from_slice(&1u32.to_le_bytes());
    payload.extend_from_slice(&(comment_entry.len() as u32).to_le_bytes());
    payload.extend_from_slice(comment_entry.as_bytes());

    let len = payload.len();
    let mut block = Vec::with_capacity(4 + len);
    block.push(0x84);
    block.push(((len >> 16) & 0xff) as u8);
    block.push(((len >> 8) & 0xff) as u8);
    block.push((len & 0xff) as u8);
    block.extend_from_slice(&payload);
    block
}

thread_local! {
    static MIX_TEMP: RefCell<Vec<f32>> = RefCell::new(Vec::new());
    static FX_TEMP: RefCell<Vec<f32>> = RefCell::new(Vec::new());
    static HOST_OUTPUT_TEMP: RefCell<Vec<f32>> = RefCell::new(Vec::new());
    static EVENTS_TMP: RefCell<Vec<vst3::MidiEvent>> = RefCell::new(Vec::new());
    static FILTERED_EVENTS_TMP: RefCell<Vec<vst3::MidiEvent>> = RefCell::new(Vec::new());
    static PERF_EVENTS_TMP: RefCell<Vec<vst3::MidiEvent>> = RefCell::new(Vec::new());
    static REMAINING_PARAMS_TMP: RefCell<Vec<PendingParamChange>> = RefCell::new(Vec::new());
    static CIN_PEAKS_TMP: RefCell<Vec<f32>> = RefCell::new(Vec::new());
    static COUT_PEAKS_TMP: RefCell<Vec<f32>> = RefCell::new(Vec::new());
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
    let velocity_gain = (velocity as f32 / 127.0).clamp(0.0, 1.0);
    let gain = state.gain * sample.gain * velocity_gain;
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
                            let sample = treesynth_state.samples[idx].clone();
                            treesynth_spawn_voice(&mut runtime, &treesynth_state, idx, &sample, &data, *note, *velocity, start_sample, sample_rate);
                        }
                    }
                    TreeSynthMode::Sequential => {
                        let idx = runtime.sequence_index % sample_count;
                        runtime.sequence_index = runtime.sequence_index.wrapping_add(1);
                        if let Some(data) = sample_data.get(idx).and_then(|d| d.as_ref()) {
                            let sample = treesynth_state.samples[idx].clone();
                            treesynth_spawn_voice(&mut runtime, &treesynth_state, idx, &sample, &data, *note, *velocity, start_sample, sample_rate);
                        }
                    }
                    TreeSynthMode::Reorder => {
                        let pos = ((f32::from(*note) / 127.0) + treesynth_state.reorder).fract();
                        let idx = (pos * sample_count as f32).floor() as usize;
                        let idx = idx.min(sample_count.saturating_sub(1));
                        if let Some(data) = sample_data.get(idx).and_then(|d| d.as_ref()) {
                            let sample = treesynth_state.samples[idx].clone();
                            treesynth_spawn_voice(&mut runtime, &treesynth_state, idx, &sample, &data, *note, *velocity, start_sample, sample_rate);
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
                            treesynth_spawn_voice(&mut runtime, &treesynth_state, idx0, &sample, &data, *note, *velocity, start_sample, sample_rate);
                        }
                        if idx1 != idx0 {
                            if let Some(data) = sample_data.get(idx1).and_then(|d| d.as_ref()) {
                                let mut sample = treesynth_state.samples[idx1].clone();
                                sample.gain *= weight1;
                                treesynth_spawn_voice(&mut runtime, &treesynth_state, idx1, &sample, &data, *note, *velocity, start_sample, sample_rate);
                            }
                        }
                    }
                    TreeSynthMode::Layer => {
                        for idx in 0..sample_count {
                            if let Some(data) = sample_data.get(idx).and_then(|d| d.as_ref()) {
                                let sample = treesynth_state.samples[idx].clone();
                                treesynth_spawn_voice(&mut runtime, &treesynth_state, idx, &sample, &data, *note, *velocity, start_sample, sample_rate);
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
    smart_disable_plugins: bool,
    smart_suspend_tracks: bool,
    sidechain_states: &mut Vec<f32>,
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

    let mix_snapshot = track_mix.try_lock().ok().map(|m| m.clone()).unwrap_or_default();
    let any_solo = mix_snapshot.iter().any(|m| m.solo);
    let performance_snapshot = performance_runtime
        .try_lock()
        .ok()
        .map(|runtime| runtime.clone())
        .unwrap_or_default();
    let track_count = track_audio.len();
    let mut track_has_audio = vec![false; track_count];
    let mut per_track_clips: Vec<Vec<(AudioClipRender, Arc<AudioClipData>)>> =
        vec![Vec::new(); track_count];
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
                track_has_audio[clip.track_index] = true;
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
                per_track_clips[clip.track_index].push((clip.clone(), data));
            }
        }
    }

    let routes_snapshot = node_routes.try_lock().ok().map(|r| r.clone()).unwrap_or_default();
    if sidechain_states.len() < routes_snapshot.len() {
        sidechain_states.resize(routes_snapshot.len(), 1.0);
    } else if sidechain_states.len() > routes_snapshot.len() {
        sidechain_states.truncate(routes_snapshot.len());
    }

    let processed_any_atomic = AtomicBool::new(false);

    // Realtime-safe: keep track processing deterministic (no Rayon scheduling variance).
    let track_host_outputs: Vec<Option<(Vec<f32>, usize)>> = (0..track_count).into_iter().map(|index| {
        let mix = mix_snapshot.get(index).copied().unwrap_or(TrackMixState {
            muted: false,
            solo: false,
            level: 1.0,
        });
        let state = match track_audio.get(index) {
            Some(state) => state,
            None => return None,
        };

        if mix.muted || (any_solo && !mix.solo) {
            state.peak_bits.store(0.0f32.to_bits(), Ordering::Relaxed);
            state.peak_l_bits.store(0.0f32.to_bits(), Ordering::Relaxed);
            state.peak_r_bits.store(0.0f32.to_bits(), Ordering::Relaxed);
            state.midi_in_peak.store(0.0f32.to_bits(), Ordering::Relaxed);
            state.midi_out_peak.store(0.0f32.to_bits(), Ordering::Relaxed);
            return None;
        }

        let notes = if arrangement_playing {
            match state.clip_notes.try_lock() {
                Ok(guard) => guard.clone(),
                Err(_) => Vec::new(),
            }
        } else {
            Vec::new()
        };
        let active_performance = performance_snapshot.get(index).and_then(|slot| slot.clone());
        let has_notes = !notes.is_empty()
            || active_performance
                .as_ref()
                .map(|runtime| runtime.clip.is_midi)
                .unwrap_or(false);
        let has_audio = track_has_audio.get(index).copied().unwrap_or(false)
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
                return None;
            }
        } else {
            state.silent_blocks.store(0, Ordering::Relaxed);
        }

        let mut track_processed = false;
        let mut host_out_result = None;

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
                                        let out =
                                            std::mem::take(&mut *host_output_temp);
                                        host_out_result = Some((out, host_out_channels));
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

                if let Some(clips) = per_track_clips.get(index) {
                    for (clip, data) in clips {
                        mix_clip_resample(&mut temp, channels, clip, data, block_start, block_end, sample_rate);
                        track_processed = true;
                    }
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
                    track_out_mutex.resize(temp.len(), 0.0);
                    for (out, sample) in track_out_mutex.iter_mut().zip(temp.iter()) {
                        *out = *sample * mix.level;
                    }
                    }
                })
            })
        });
        host_out_result
    }).collect();

    let processed_any = processed_any_atomic.load(Ordering::Relaxed);

    for (route_index, route) in routes_snapshot.iter().enumerate() {
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
        let attack_coeff = (-1.0 / (attack * sample_rate.max(1.0))).exp();
        let release_coeff = (-1.0 / (release * sample_rate.max(1.0))).exp();

        let source_pair = route.source_output_pair;

        let from_state = &track_audio[route.from_track];
        let to_state = &track_audio[route.to_track];
        
        // Locked access to buffers for sidechain processing
        let from_buf_guard = from_state.track_buffer.try_lock();
        let mut to_buf_guard = to_state.track_buffer.try_lock();
        
        if let (Ok(source), Ok(mut target)) = (from_buf_guard, to_buf_guard) {
            let mut gain = sidechain_states.get(route_index).copied().unwrap_or(1.0);
            for frame in 0..frames {
                let base = frame * channels;
                let detector = if let Some((host_buf, host_channels)) =
                    track_host_outputs.get(route.from_track).and_then(|entry| entry.as_ref())
                {
                    let src_base = frame * *host_channels;
                    let ch0 = (source_pair * 2).min(host_channels.saturating_sub(1));
                    let ch1 = (ch0 + 1).min(host_channels.saturating_sub(1));
                    host_buf.get(src_base + ch0).copied().unwrap_or(0.0).abs().max(
                        host_buf.get(src_base + ch1).copied().unwrap_or(0.0).abs(),
                    )
                } else {
                    let mut v = 0.0f32;
                    for ch in 0..channels {
                        v = v.max(source.get(base + ch).copied().unwrap_or(0.0).abs());
                    }
                    v
                };
                let target_gain = if detector > threshold {
                    let over = ((detector - threshold) / (1.0 - threshold).max(1e-6)).clamp(0.0, 1.0);
                    (1.0 - amount * over).clamp(0.05, 1.0)
                } else {
                    1.0
                };
                if target_gain < gain {
                    gain = attack_coeff * (gain - target_gain) + target_gain;
                } else {
                    gain = release_coeff * (gain - target_gain) + target_gain;
                }
                for ch in 0..channels {
                    if let Some(sample) = target.get_mut(base + ch) {
                        *sample *= gain;
                    }
                }
            }
            if let Some(state) = sidechain_states.get_mut(route_index) {
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

    if let Ok(mut activity) = node_activity.lock() {
        if activity.len() < track_count {
            activity.resize(track_count, TrackNodeActivity::default());
        }
        for (index, state) in track_audio.iter().enumerate() {
            let mut pair_peaks = [0.0f32; 8];
            if let Some((host_buf, host_channels)) = track_host_outputs.get(index).and_then(|v| v.as_ref()) {
                let hc = (*host_channels).max(1);
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

            if let Some(slot) = activity.get_mut(index) {
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
            }
        }
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

pub(crate) fn collect_block_events(
    notes: &[PianoRollNote],
    block_start: u64,
    block_end: u64,
    samples_per_beat: f64,
) -> Vec<vst3::MidiEvent> {
    let mut events = Vec::new();
    for note in notes {
        let start_sample = (note.start_beats as f64 * samples_per_beat).round() as u64;
        let mut end_sample = ((note.start_beats + note.length_beats) as f64 * samples_per_beat)
            .round() as u64;
        if end_sample <= start_sample {
            end_sample = start_sample.saturating_add(1);
        }
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

pub(crate) fn collect_block_events_into(
    notes: &[PianoRollNote],
    block_start: u64,
    block_end: u64,
    samples_per_beat: f64,
    out: &mut Vec<vst3::MidiEvent>,
) {
    out.clear();
    for note in notes {
        let start_sample = (note.start_beats as f64 * samples_per_beat).round() as u64;
        let mut end_sample =
            ((note.start_beats + note.length_beats) as f64 * samples_per_beat).round() as u64;
        if end_sample <= start_sample {
            end_sample = start_sample.saturating_add(1);
        }
        if start_sample >= block_start && start_sample < block_end {
            let offset = (start_sample - block_start) as i32;
            out.push(vst3::MidiEvent::note_on_at(0, note.midi_note, note.velocity, offset));
        }
        if end_sample >= block_start && end_sample < block_end {
            let offset = (end_sample - block_start) as i32;
            out.push(vst3::MidiEvent::note_off_at(0, note.midi_note, 0, offset));
        }
    }
}

pub(crate) fn db_to_gain(db: f32) -> f32 {
    10.0f32.powf(db / 20.0)
}

pub(crate) fn apply_master_processing(
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
pub(crate) enum RenderFormat {
    Wav,
    Ogg,
    Flac,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RenderWavBitDepth {
    Int16,
    Int24,
    Int32,
    Float32,
}

impl RenderWavBitDepth {
    pub(crate) fn all() -> [Self; 4] {
        [Self::Int16, Self::Int24, Self::Int32, Self::Float32]
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Int16 => "16-bit",
            Self::Int24 => "24-bit",
            Self::Int32 => "32-bit int",
            Self::Float32 => "32f",
        }
    }

    pub(crate) fn bits_per_sample(self) -> u16 {
        match self {
            Self::Int16 => 16,
            Self::Int24 => 24,
            Self::Int32 => 32,
            Self::Float32 => 32,
        }
    }

    pub(crate) fn sample_format(self) -> hound::SampleFormat {
        match self {
            Self::Float32 => hound::SampleFormat::Float,
            _ => hound::SampleFormat::Int,
        }
    }
}

pub(crate) fn default_midi_params() -> Vec<String> {
    vec![
        "CC1 Modwheel".to_string(),
        "CC7 Volume".to_string(),
        "CC10 Pan".to_string(),
        "CC11 Expression".to_string(),
        "CC64 Sustain".to_string(),
    ]
}

pub(crate) fn gm_program_name(program: u8) -> &'static str {
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

pub(crate) fn gm_drum_kit_name(program: u8) -> Option<&'static str> {
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

pub(crate) fn default_instrument_params() -> Vec<String> {
    vec![
        "Gain".to_string(),
        "Cutoff".to_string(),
        "Resonance".to_string(),
        "Attack".to_string(),
        "Release".to_string(),
    ]
}
