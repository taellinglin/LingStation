impl DawApp {
    fn render_track_model(
        track_index: usize,
        track: &Track,
        has_solo: bool,
        force_active: Option<bool>,
    ) -> RenderTrack {
        let mut active = !track.muted && (!has_solo || track.solo);
        if let Some(f) = force_active {
            active = f;
        }
        RenderTrack {
            index: track_index,
            mix: TrackMixState {
                muted: track.muted,
                solo: track.solo,
                level: track.level,
                active,
            },
            treesynth_enabled: track.treesynth.is_some(),
            treesynth_state: track.treesynth.clone(),
            host_id: track
                .instrument_clap_id
                .clone()
                .or_else(|| track.instrument_path.clone()),
            host_state: track.plugin_state_component.clone(),
            clips: track.clips.clone(),
            automation: track.automation_lanes.clone(),
            output_pair: None,
        }
    }

    pub(crate) fn render_with_options(&mut self, folder: &Path) -> Result<(), String> {
        self.rebuild_all_track_midi_notes();
        self.sync_track_audio_states();
        if self.audio_running {
            self.stop_audio_and_midi_internal(false);
            self.status = "Playback paused for render stability".to_string();
        }
        self.preload_audio_clips(&self.engine.audio_cache);
        if self.render_job.is_some() {
            return Ok(());
        }
        self.ensure_synth_soundfont();
        self.capture_plugin_states();
        for track in &mut self.tracks {
            if track.treesynth.is_some() {
                track.instrument_path = Some("native:treesynth".to_string());
            }
            if track.drum_machine.is_some() {
                track.instrument_path = Some("native:drummachine".to_string());
            }
        }
        for (i, track) in self.tracks.iter().enumerate() {
            if let Some(state) = self.engine.track_audio.get_mut(i) {
                let enabled = track.treesynth.is_some();
                state.sync_treesynth(track, enabled, &self.engine.audio_cache);
                let drum_enabled = track.drum_machine.is_some();
                state.sync_drum_machine(track, drum_enabled);
            }
        }
        let license_comment = self.render_license_comment();
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

        let has_solo = self.tracks.iter().any(|t| t.solo);
        let mut plan_paths: Vec<(PathBuf, RenderPlan)> = Vec::new();

        let master_name = match format {
            RenderFormat::Wav => format!("{base_name}.wav"),
            RenderFormat::Ogg => format!("{base_name}.ogg"),
            RenderFormat::Flac => format!("{base_name}.flac"),
        };
        let master_path = Self::safe_join_within_base(&folder, &master_name)?;
        plan_paths.push((
            master_path,
            self.build_master_render_plan(sample_rate, range_start, range_end, has_solo, license_comment.clone()),
        ));

        if self.render_split_tracks {
            for (index, track) in self.tracks.iter().enumerate() {
                if !self.track_has_audio_in_range(track, range_start, range_end) {
                    continue;
                }
                let safe_name = Self::sanitize_folder_name(&track.name);
                let ext = match format {
                    RenderFormat::Wav => "wav",
                    RenderFormat::Ogg => "ogg",
                    RenderFormat::Flac => "flac",
                };
                let runtime_pairs = self
                    .engine
                    .track_audio
                    .get(index)
                    .map(|state| state.native_output_channels.load(Ordering::Relaxed))
                    .unwrap_or(2)
                    .max(2) as usize
                    / 2;
                let pair_count = track.output_pair_mix.len().max(runtime_pairs).max(1).min(8);
                if pair_count > 1 {
                    for pair in 0..pair_count {
                        let file_name = format!(
                            "{} - {:02}_{}_Out{}-{}.{}",
                            base_name,
                            index + 1,
                            safe_name,
                            pair * 2 + 1,
                            pair * 2 + 2,
                            ext
                        );
                        let path = Self::safe_join_within_base(&folder, &file_name)?;
                        let mut plan = self.build_stem_render_plan(
                            index,
                            sample_rate,
                            range_start,
                            range_end,
                            has_solo,
                            license_comment.clone(),
                        );
                        if let Some(track_plan) = plan.tracks.get_mut(index) {
                            track_plan.output_pair = Some(pair);
                        }
                        plan_paths.push((path, plan));
                    }
                } else {
                    let file_name =
                        format!("{} - {:02}_{}.{}", base_name, index + 1, safe_name, ext);
                    let path = Self::safe_join_within_base(&folder, &file_name)?;
                    plan_paths.push((
                        path,
                        self.build_stem_render_plan(
                            index,
                            sample_rate,
                            range_start,
                            range_end,
                            has_solo,
                            license_comment.clone(),
                        ),
                    ));
                }
            }
        }

        let done = Arc::new(AtomicU64::new(0));
        let total = Arc::new(AtomicU64::new(1));
        let finished = Arc::new(AtomicBool::new(false));
        let result = Arc::new(std::sync::Mutex::new(None));
        self.render_progress = Some((0, 1));
        self.render_job = Some(RenderJob {
            done: done.clone(),
            total: total.clone(),
            finished: finished.clone(),
            result: result.clone(),
        });

        let track_audio = self.engine.track_audio.clone();
        let audio_clip_cache = self.engine.audio_cache.clone();
        let format = self.render_format;
        let ogg_quality = self.render_ogg_quality.clamp(0.0, 1.0);
        let flac_bit_depth = self.render_flac_bit_depth;
        std::thread::spawn(move || {
            let mut final_status = Ok("Render complete".to_string());
            for (path, plan) in plan_paths {
                done.store(0, Ordering::Relaxed);
                total.store(1, Ordering::Relaxed);
                let res = match format {
                    RenderFormat::Wav => {
                        Self::offline_render_plan_to_wav_thread(&plan, &path, &done, &total, track_audio.clone(), &audio_clip_cache)
                    }
                    RenderFormat::Ogg => {
                        Self::offline_render_plan_to_ogg_thread(
                            &plan,
                            &path,
                            &done,
                            &total,
                            track_audio.clone(),
                            &audio_clip_cache,
                            ogg_quality,
                        )
                    }
                    RenderFormat::Flac => {
                        Self::offline_render_plan_to_flac_thread(
                            &plan,
                            &path,
                            &done,
                            &total,
                            track_audio.clone(),
                            &audio_clip_cache,
                            flac_bit_depth,
                        )
                    }
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

    fn track_has_audio_in_range(&self, track: &Track, range_start: f32, range_end: f32) -> bool {
        let end = range_end.max(range_start + 0.001);
        if !track.automation_lanes.is_empty()
            || track.instrument_path.is_some()
            || !track.effect_paths.is_empty()
            || track.treesynth.is_some()
            || track.drum_machine.is_some()
        {
            return true;
        }
        if track.midi_notes.iter().any(|note| {
            let note_end = note.start_beats + note.length_beats;
            note.start_beats < end && note_end > range_start
        }) {
            return true;
        }
        track.clips.iter().any(|clip| {
            if clip.is_midi {
                return false;
            }
            let clip_end = clip.start_beats + clip.length_beats;
            clip.start_beats < end && clip_end > range_start
        })
    }

    fn offline_render_plan_to_wav_thread(
        plan: &RenderPlan,
        out_path: &Path,
        progress_done: &AtomicU64,
        progress_total: &AtomicU64,
        track_audio: Vec<TrackAudioState>,
        audio_clip_cache: &Arc<ParkingMutex<AudioClipCache>>,
    ) -> Result<(), String> {
        use engine::render::{append_wav_comment, render_plan_for_each_block, wav_spec_for_depth, write_wav_samples};
        let cancel = AtomicBool::new(false);
        let sample_rate = plan.sample_rate;
        let _channels = plan.channels as usize;
        let bit_depth = plan.bit_depth;
        let spec = wav_spec_for_depth(plan.sample_rate, plan.channels, bit_depth);
        let file = std::fs::File::create(out_path).map_err(|e| e.to_string())?;
        let mut writer = hound::WavWriter::new(file, spec).map_err(|e| e.to_string())?;

        let bpm = plan.bpm;
        let samples_per_beat = (sample_rate as f64 * 60.0) / bpm as f64;
        let start_samples = (plan.start_beats as f64 * samples_per_beat).round() as u64;
        let end_samples = (plan.end_beats as f64 * samples_per_beat).round() as u64;
        let expected_total = end_samples.saturating_sub(start_samples);
        progress_total.store(expected_total.max(1), Ordering::Relaxed);
        progress_done.store(0, Ordering::Relaxed);

        let mut rendered_block_samples = 0u64;
        render_plan_for_each_block(
            plan,
            &cancel,
            expected_total,
            track_audio,
            audio_clip_cache,
            |block, frames| {
                write_wav_samples(&mut writer, bit_depth, block)?;
                rendered_block_samples = rendered_block_samples.saturating_add(frames as u64);
                progress_done.store(
                    rendered_block_samples.min(expected_total),
                    Ordering::Relaxed,
                );
                Ok(())
            },
        )?;
        writer.finalize().map_err(|e| e.to_string())?;
        if let Some(comment) = plan.license_comment.as_ref() {
            if !comment.is_empty() {
                let _ = append_wav_comment(&out_path.to_string_lossy(), comment);
            }
        }
        Ok(())
    }

    fn offline_render_plan_to_ogg_thread(
        plan: &RenderPlan,
        out_path: &Path,
        progress_done: &AtomicU64,
        progress_total: &AtomicU64,
        track_audio: Vec<TrackAudioState>,
        audio_clip_cache: &Arc<ParkingMutex<AudioClipCache>>,
        quality: f32,
    ) -> Result<(), String> {
        use engine::render::render_plan_for_each_block;
        use std::io::Write;
        let cancel = AtomicBool::new(false);
        let sample_rate = plan.sample_rate as u64;
        let channels = plan.channels.max(1) as usize;
        let mut encoder = vorbis_encoder::Encoder::new(
            channels as u32,
            sample_rate,
            quality.clamp(0.0, 1.0),
        )
            .map_err(|e| format!("Vorbis encoder init failed: {e}"))?;
        let mut file = std::fs::File::create(out_path).map_err(|e| e.to_string())?;

        let bpm = plan.bpm;
        let samples_per_beat = (plan.sample_rate as f64 * 60.0) / bpm as f64;
        let start_samples = (plan.start_beats as f64 * samples_per_beat).round() as u64;
        let end_samples = (plan.end_beats as f64 * samples_per_beat).round() as u64;
        let expected_total = end_samples.saturating_sub(start_samples);
        progress_total.store(expected_total.max(1), Ordering::Relaxed);
        progress_done.store(0, Ordering::Relaxed);

        let mut rendered_block_samples = 0u64;
        render_plan_for_each_block(
            plan,
            &cancel,
            expected_total,
            track_audio,
            audio_clip_cache,
            |block, frames| {
                let mut pcm = Vec::with_capacity(frames * channels);
                for sample in block.iter().take(frames * channels) {
                    let clamped = sample.clamp(-1.0, 1.0);
                    let value = (clamped * i16::MAX as f32).round() as i16;
                    pcm.push(value);
                }
                let encoded = encoder
                    .encode(&pcm)
                    .map_err(|e| format!("Vorbis encode failed: {e}"))?;
                if !encoded.is_empty() {
                    file.write_all(&encoded).map_err(|e| e.to_string())?;
                }
                rendered_block_samples = rendered_block_samples.saturating_add(frames as u64);
                progress_done.store(
                    rendered_block_samples.min(expected_total),
                    Ordering::Relaxed,
                );
                Ok(())
            },
        )?;
        let encoded = encoder
            .flush()
            .map_err(|e| format!("Vorbis flush failed: {e}"))?;
        if !encoded.is_empty() {
            file.write_all(&encoded).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn offline_render_plan_to_flac_thread(
        plan: &RenderPlan,
        out_path: &Path,
        progress_done: &AtomicU64,
        progress_total: &AtomicU64,
        track_audio: Vec<TrackAudioState>,
        audio_clip_cache: &Arc<ParkingMutex<AudioClipCache>>,
        bit_depth: RenderWavBitDepth,
    ) -> Result<(), String> {
        use engine::render::{render_plan_for_each_block, sample_to_int};
        use flacenc::component::BitRepr;
        use flacenc::error::Verify;

        let cancel = AtomicBool::new(false);
        let channels = plan.channels.max(1) as usize;
        let bits_per_sample = match bit_depth {
            RenderWavBitDepth::Int16 => 16,
            _ => 24,
        };

        let bpm = plan.bpm;
        let samples_per_beat = (plan.sample_rate as f64 * 60.0) / bpm as f64;
        let start_samples = (plan.start_beats as f64 * samples_per_beat).round() as u64;
        let end_samples = (plan.end_beats as f64 * samples_per_beat).round() as u64;
        let expected_total = end_samples.saturating_sub(start_samples);
        progress_total.store(expected_total.max(1), Ordering::Relaxed);
        progress_done.store(0, Ordering::Relaxed);

        let mut samples: Vec<i32> = Vec::new();
        let mut rendered_block_samples = 0u64;
        render_plan_for_each_block(
            plan,
            &cancel,
            expected_total,
            track_audio,
            audio_clip_cache,
            |block, frames| {
                let total = frames * channels;
                samples.reserve(total);
                for sample in block.iter().take(total) {
                    let value = sample_to_int(*sample, bits_per_sample) as i32;
                    samples.push(value);
                }
                rendered_block_samples = rendered_block_samples.saturating_add(frames as u64);
                progress_done.store(
                    rendered_block_samples.min(expected_total),
                    Ordering::Relaxed,
                );
                Ok(())
            },
        )?;

        let config = flacenc::config::Encoder::default()
            .into_verified()
            .map_err(|e| format!("FLAC config error: {e:?}"))?;
        let source = flacenc::source::MemSource::from_samples(
            &samples,
            channels,
            bits_per_sample as usize,
            plan.sample_rate as usize,
        );
        let stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
            .map_err(|e| format!("FLAC encode failed: {e}"))?;
        let mut sink = flacenc::bitsink::ByteSink::new();
        stream.write(&mut sink);
        std::fs::write(out_path, sink.as_slice()).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub(crate) fn build_master_render_plan(
        &self,
        sample_rate: u32,
        start_beats: f32,
        end_beats: f32,
        has_solo: bool,
        license_comment: Option<String>,
    ) -> RenderPlan {
        let tracks = self
            .tracks
            .iter()
            .enumerate()
            .map(|(track_index, track)| Self::render_track_model(track_index, track, has_solo, None))
            .collect::<Vec<_>>();
        RenderPlan {
            start_beats: start_beats.max(0.0),
            end_beats: end_beats.max(start_beats + 0.25),
            sample_rate,
            channels: 2,
            bit_depth: self.render_wav_bit_depth,
            bpm: self.tempo_bpm.max(1.0),
            tracks,
            master_comp: self.engine.master_comp_snapshot(),
            license_comment,
        }
    }

    pub(crate) fn build_stem_render_plan(
        &self,
        stem_index: usize,
        sample_rate: u32,
        start_beats: f32,
        end_beats: f32,
        has_solo: bool,
        license_comment: Option<String>,
    ) -> RenderPlan {
        let tracks = self
            .tracks
            .iter()
            .enumerate()
            .map(|(track_index, track)| {
                let force = Some(track_index == stem_index && !track.muted);
                Self::render_track_model(track_index, track, has_solo, force)
            })
            .collect::<Vec<_>>();
        RenderPlan {
            start_beats: start_beats.max(0.0),
            end_beats: end_beats.max(start_beats + 0.25),
            sample_rate,
            channels: 2,
            bit_depth: self.render_wav_bit_depth,
            bpm: self.tempo_bpm.max(1.0),
            tracks,
            master_comp: self.engine.master_comp_snapshot(),
            license_comment,
        }
    }
}
