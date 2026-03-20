#[allow(dead_code)]
impl DawApp {
    pub(crate) fn render_with_options(&mut self, folder: &Path) -> Result<(), String> {
            // レンダー直前にtrack_audioを最新化
            self.sync_track_audio_states();
        if self.audio_running {
            self.stop_audio_and_midi_internal(false);
            self.status = "Playback paused for render stability".to_string();
        }
        self.preload_audio_clips(&self.audio_clip_cache);
        if self.render_job.is_some() {
            return Ok(());
        }
        self.ensure_synth_soundfont();
        self.capture_plugin_states();
        // Ensure TreeSynth instrument_path is set for all TreeSynth tracks before rendering
        for track in &mut self.tracks {
            if track.treesynth.is_some() {
                track.instrument_path = Some("native:treesynth".to_string());
            }
        }
        // TreeSynth enable/sync for all tracks
        for (i, track) in self.tracks.iter().enumerate() {
            if let Some(audio_state) = self.track_audio.get(i) {
                let enabled = track.treesynth.is_some();
                audio_state.sync_treesynth(track, enabled, &self.audio_clip_cache);
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

        let master_name = match format {
            RenderFormat::Wav => format!("{base_name}.wav"),
            RenderFormat::Ogg => format!("{base_name}.ogg"),
            RenderFormat::Flac => format!("{base_name}.flac"),
        };
        let master_path = Self::safe_join_within_base(&folder, &master_name)?;
        let master_plan = self.build_master_render_plan(
            &master_path,
            sample_rate,
            range_start,
            range_end,
            license_comment.clone(),
        );
        let mut plans = vec![master_plan];
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
                plans.push(self.build_render_plan_for_track(
                    index,
                    &path,
                    sample_rate,
                    range_start,
                    range_end,
                    license_comment.clone(),
                ));
            }
        }

        let done = Arc::new(AtomicU64::new(0));
        let total = Arc::new(AtomicU64::new(1));
        let finished = Arc::new(AtomicBool::new(false));
        let result = Arc::new(Mutex::new(None));
        self.render_progress = Some((0, 1));
        self.render_job = Some(RenderJob {
            done: done.clone(),
            total: total.clone(),
            finished: finished.clone(),
            result: result.clone(),
        });

        let track_audio = self.track_audio.clone();
        let audio_clip_cache = self.audio_clip_cache.clone();
        let format = self.render_format;
        let plans = plans.clone();
        let done = done.clone();
        let total = total.clone();
        let finished = finished.clone();
        let result = result.clone();
        std::thread::spawn(move || {
            let mut final_status = Ok("Render complete".to_string());
            for plan in plans {
                done.store(0, Ordering::Relaxed);
                total.store(1, Ordering::Relaxed);
                let res = match format {
                    RenderFormat::Wav => render_plan_to_wav(plan, &done, &total, &track_audio, &audio_clip_cache),
                    RenderFormat::Ogg => render_plan_to_ogg(plan, &done, &total, &track_audio, &audio_clip_cache),
                    RenderFormat::Flac => render_plan_to_flac(plan, &done, &total, &track_audio, &audio_clip_cache),
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

    pub(crate) fn build_master_render_plan(
        &self,
        path: &Path,
        sample_rate: u32,
        start_beats: f32,
        end_beats: f32,
        license_comment: Option<String>,
    ) -> RenderPlan {
        let block_size = self.settings.buffer_size.max(64) as usize;
        let has_solo = self.tracks.iter().any(|t| t.solo);
        let (audio_clips, audio_cache) = self.build_audio_clip_render_data(sample_rate, None);
        let tracks = self
            .tracks
            .iter()
            .enumerate()
            .map(|(track_index, track)| {
                let mut instrument_path = track.instrument_path.clone();
                if track.treesynth.is_some() {
                    instrument_path = Some("native:treesynth".to_string());
                }
                RenderTrack {
                    source_track_index: Some(track_index),
                    notes: track.midi_notes.clone(),
                    instrument_path,
                    instrument_clap_id: track.instrument_clap_id.clone(),
                    param_ids: track.param_ids.clone(),
                    param_values: track.param_values.clone(),
                    plugin_state_component: track.plugin_state_component.clone(),
                    plugin_state_controller: track.plugin_state_controller.clone(),
                    effect_paths: track.effect_paths.clone(),
                    effect_clap_ids: track.effect_clap_ids.clone(),
                    effect_bypass: track.effect_bypass.clone(),
                    automation_lanes: track.automation_lanes.clone(),
                    level: track.level,
                    active: !track.muted && (!has_solo || track.solo),
                }
            })
            .collect::<Vec<_>>();
        RenderPlan {
            path: path.to_string_lossy().to_string().into(),
            sample_rate,
            block_size,
            tempo_bpm: self.tempo_bpm.max(1.0),
            start_beats: start_beats.max(0.0),
            end_beats: end_beats.max(start_beats + 0.25),
            bitrate_kbps: self.render_bitrate,
            wav_bit_depth: self.render_wav_bit_depth,
            render_tail_mode: self.render_tail_mode,
            render_release_seconds: self.render_release_seconds,
            tracks,
            node_routes: self.node_routes.clone(),
            notes: Vec::new(),
            instrument_path: None,
            param_ids: Vec::new(),
            param_values: Vec::new(),
            plugin_state_component: None,
            plugin_state_controller: None,
            audio_clips,
            audio_cache,
            master_settings: self.master_settings_snapshot(),
            license_comment,
        }
    }

    pub(crate) fn build_render_plan_for_track(
        &self,
        index: usize,
        path: &Path,
        sample_rate: u32,
        start_beats: f32,
        end_beats: f32,
        license_comment: Option<String>,
    ) -> RenderPlan {
        let block_size = self.settings.buffer_size.max(64) as usize;
        let (notes, instrument_path, instrument_clap_id, param_ids, param_values, component, controller, automation_lanes) = self
            .tracks
            .get(index)
            .map(|track| {
                let mut instrument_path = track.instrument_path.clone();
                if track.treesynth.is_some() {
                    instrument_path = Some("native:treesynth".to_string());
                }
                (
                    track.midi_notes.clone(),
                    instrument_path,
                    track.instrument_clap_id.clone(),
                    track.param_ids.clone(),
                    track.param_values.clone(),
                    track.plugin_state_component.clone(),
                    track.plugin_state_controller.clone(),
                    track.automation_lanes.clone(),
                )
            })
            .unwrap_or_else(|| (Vec::new(), None, None, Vec::new(), Vec::new(), None, None, Vec::new()));
        let (effect_paths, effect_bypass, effect_clap_ids) = self
            .tracks
            .get(index)
            .map(|track| (track.effect_paths.clone(), track.effect_bypass.clone(), track.effect_clap_ids.clone()))
            .unwrap_or_else(|| (Vec::new(), Vec::new(), Vec::new()));
        let (audio_clips, audio_cache) =
            self.build_audio_clip_render_data(sample_rate, Some(index));
        let track = RenderTrack {
            source_track_index: Some(index),
            notes,
            instrument_path,
            instrument_clap_id,
            param_ids,
            param_values,
            plugin_state_component: component,
            plugin_state_controller: controller,
            effect_paths,
            effect_clap_ids,
            effect_bypass,
            automation_lanes,
            level: 1.0,
            active: true,
        };
        RenderPlan {
            path: path.to_string_lossy().to_string().into(),
            sample_rate,
            block_size,
            tempo_bpm: self.tempo_bpm.max(1.0),
            start_beats: start_beats.max(0.0),
            end_beats: end_beats.max(start_beats + 0.25),
            bitrate_kbps: self.render_bitrate,
            wav_bit_depth: self.render_wav_bit_depth,
            render_tail_mode: self.render_tail_mode,
            render_release_seconds: self.render_release_seconds,
            tracks: vec![track],
            node_routes: Vec::new(),
            notes: Vec::new(),
            instrument_path: None,
            param_ids: Vec::new(),
            param_values: Vec::new(),
            plugin_state_component: None,
            plugin_state_controller: None,
            audio_clips,
            audio_cache,
            master_settings: self.master_settings_snapshot(),
            license_comment,
        }
    }
}
