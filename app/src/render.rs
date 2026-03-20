use rayon::prelude::*;
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
    if let Some(parent) = Path::new(&*plan.path).parent() {
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
    std::fs::rename(&tmp_path, &*plan.path).map_err(|e| e.to_string())?;
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
        log::info!(
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
    std::fs::rename(&tmp_path, &*path).map_err(|e| e.to_string())?;
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
    std::fs::rename(&tmp_path, &*path).map_err(|e| e.to_string())?;
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
    static MIX_TEMP: RefCell<Vec<f32>> = RefCell::new(Vec::with_capacity(2048));
    static FX_TEMP: RefCell<Vec<f32>> = RefCell::new(Vec::with_capacity(2048));
    static HOST_OUTPUT_TEMP: RefCell<Vec<f32>> = RefCell::new(Vec::with_capacity(2048));
    static EVENTS_TMP: RefCell<Vec<vst3::MidiEvent>> = RefCell::new(Vec::with_capacity(512));
    static FILTERED_EVENTS_TMP: RefCell<Vec<vst3::MidiEvent>> = RefCell::new(Vec::with_capacity(512));
    static PERF_EVENTS_TMP: RefCell<Vec<vst3::MidiEvent>> = RefCell::new(Vec::with_capacity(512));
    static REMAINING_PARAMS_TMP: RefCell<Vec<PendingParamChange>> = RefCell::new(Vec::with_capacity(128));
    static CIN_PEAKS_TMP: RefCell<Vec<f32>> = RefCell::new(Vec::with_capacity(64));
    static COUT_PEAKS_TMP: RefCell<Vec<f32>> = RefCell::new(Vec::with_capacity(64));
}


include!("render_mix.rs");
include!("render_util.rs");
