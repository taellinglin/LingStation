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
        }
    }

    pub(crate) fn render_with_options(&mut self, folder: &Path) -> Result<(), String> {
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
                let safe_name = Self::sanitize_folder_name(&track.name);
                let ext = match format {
                    RenderFormat::Wav => "wav",
                    RenderFormat::Ogg => "ogg",
                    RenderFormat::Flac => "flac",
                };
                let file_name = format!("{} - {:02}_{}.{}", base_name, index + 1, safe_name, ext);
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
        let ogg_quality = Self::vorbis_quality_for_ogg(self.render_bitrate);
        std::thread::spawn(move || {
            let mut final_status = Ok("Render complete".to_string());
            for (path, plan) in plan_paths {
                done.store(0, Ordering::Relaxed);
                total.store(1, Ordering::Relaxed);
                let res = match format {
                    RenderFormat::Wav => Self::offline_render_plan_to_wav_thread(
                        &plan,
                        &path,
                        &done,
                        &total,
                        track_audio.clone(),
                        &audio_clip_cache,
                    ),
                    RenderFormat::Flac => Self::offline_render_plan_to_flac_thread(
                        &plan,
                        &path,
                        &done,
                        &total,
                        track_audio.clone(),
                        &audio_clip_cache,
                    ),
                    RenderFormat::Ogg => Self::offline_render_plan_to_ogg_thread(
                        &plan,
                        &path,
                        &done,
                        &total,
                        track_audio.clone(),
                        &audio_clip_cache,
                        ogg_quality,
                    ),
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

    fn vorbis_quality_for_ogg(bitrate_kbps: u32) -> f32 {
        let br = bitrate_kbps.max(48) as f32;
        (0.11 + (br / 320.0).min(1.0) * 0.74).clamp(0.1, 0.95)
    }

    pub(crate) fn poll_render_job_completion(&mut self) {
        let finished = self
            .render_job
            .as_ref()
            .is_some_and(|j| j.finished.load(Ordering::Relaxed));
        if let Some(job) = self.render_job.as_ref() {
            let done = job.done.load(Ordering::Relaxed);
            let total = job.total.load(Ordering::Relaxed);
            if total > 0 {
                self.render_progress = Some((done, total));
            }
        }
        if !finished {
            return;
        }
        let job = match self.render_job.take() {
            Some(j) => j,
            None => return,
        };
        if let Ok(mut guard) = job.result.lock() {
            if let Some(result) = guard.take() {
                match result {
                    Ok(msg) => {
                        self.status = msg;
                        self.show_render_dialog = false;
                    }
                    Err(err) => {
                        self.status = format!("Render failed: {err}");
                    }
                }
            }
        }
        self.render_progress = None;
    }

    fn offline_render_plan_to_flac_thread(
        plan: &RenderPlan,
        out_path: &Path,
        progress_done: &AtomicU64,
        progress_total: &AtomicU64,
        track_audio: Vec<TrackAudioState>,
        audio_clip_cache: &Arc<ParkingMutex<AudioClipCache>>,
    ) -> Result<(), String> {
        engine::render::offline_render_plan_to_flac(
            plan,
            out_path,
            progress_done,
            progress_total,
            track_audio,
            audio_clip_cache,
        )
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
        engine::render::offline_render_plan_to_ogg_vorbis(
            plan,
            out_path,
            progress_done,
            progress_total,
            track_audio,
            audio_clip_cache,
            quality,
        )
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
