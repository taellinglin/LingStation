impl DawApp {
    const UNDO_LIMIT: usize = 4096;

    pub(crate) fn leak_hosts_on_exit(&mut self) {
        let mut hosts: Vec<PluginHostHandle> = Vec::new();
        for state in self.engine.track_audio.iter_mut() {
            if let Some(mut host) = state.host.take() {
                host.prepare_for_drop();
                hosts.push(host);
            }
            for mut host in state.effect_hosts.drain(..) {
                host.prepare_for_drop();
                hosts.push(host);
            }
        }
        hosts.append(&mut self.orphaned_hosts);
        for host in hosts {
            std::mem::forget(host);
        }
    }

    pub(crate) fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() {
            return;
        }
        let input = ctx.input(|i| i.clone());
        if input.modifiers.ctrl && input.modifiers.shift && input.key_pressed(egui::Key::A) {
            self.piano_selected.clear();
            self.selected_clips.clear();
            self.selected_clip = None;
            return;
        }
        if input.modifiers.ctrl && input.key_pressed(egui::Key::A) {
            if self.piano_roll_hovered {
                if let Some(clip_id) = self.selected_clip {
                    if let Some((track_index, clip_index)) = self.find_clip_indices_by_id(clip_id) {
                        self.piano_selected.clear();
                        if let Some(clip) = self
                            .tracks
                            .get(track_index)
                            .and_then(|t| t.clips.get(clip_index))
                        {
                            for index in 0..clip.midi_notes.len() {
                                self.piano_selected.insert(index);
                            }
                        }
                    }
                }
            } else {
                self.selected_clips.clear();
                let mut last_clip = None;
                let mut last_track = None;
                for track in &self.tracks {
                    for clip in &track.clips {
                        self.selected_clips.insert(clip.id);
                        last_clip = Some(clip.id);
                        last_track = Some(clip.track);
                    }
                }
                self.selected_clip = last_clip;
                if let Some(track_index) = last_track {
                    self.selected_track = Some(track_index);
                }
            }
            return;
        }
        let has_piano_selection = !self.piano_selected.is_empty();
        if has_piano_selection {
            if input.key_pressed(egui::Key::Delete) || input.key_pressed(egui::Key::Backspace) {
                if let Some(clip_id) = self.selected_clip {
                    if let Some((track_index, clip_index)) = self.find_clip_indices_by_id(clip_id) {
                        let mut indices: Vec<usize> = self.piano_selected.iter().copied().collect();
                        indices.sort_unstable_by(|a, b| b.cmp(a));
                        self.push_undo_state();
                        if let Some(clip) = self
                            .tracks
                            .get_mut(track_index)
                            .and_then(|t| t.clips.get_mut(clip_index))
                        {
                            for index in indices {
                                if index < clip.midi_notes.len() {
                                    clip.midi_notes.remove(index);
                                }
                            }
                        }
                        self.piano_selected.clear();
                        self.sync_track_audio_notes(track_index);
                    }
                }
                return;
            }
            let nudge_beats = 1.0;
            let nudge_pitch = if input.modifiers.shift { 12 } else { 1 };
            let mut beat_delta = 0.0f32;
            let mut pitch_delta = 0i32;
            if input.key_pressed(egui::Key::ArrowLeft) {
                beat_delta = -nudge_beats;
            } else if input.key_pressed(egui::Key::ArrowRight) {
                beat_delta = nudge_beats;
            } else if input.key_pressed(egui::Key::ArrowUp) {
                pitch_delta = nudge_pitch;
            } else if input.key_pressed(egui::Key::ArrowDown) {
                pitch_delta = -nudge_pitch;
            }
            if (beat_delta.abs() > f32::EPSILON || pitch_delta != 0)
                && self.selected_track.is_some()
            {
                if let Some(clip_id) = self.selected_clip {
                    if let Some((track_index, clip_index)) = self.find_clip_indices_by_id(clip_id) {
                        let mut indices: Vec<usize> = self.piano_selected.iter().copied().collect();
                        indices.sort_unstable();
                        self.push_undo_state();
                        if let Some(clip) = self
                            .tracks
                            .get_mut(track_index)
                            .and_then(|t| t.clips.get_mut(clip_index))
                        {
                            for index in indices {
                                if let Some(note) = clip.midi_notes.get_mut(index) {
                                    if beat_delta.abs() > f32::EPSILON {
                                        note.start_beats = (note.start_beats + beat_delta).max(0.0);
                                    }
                                    if pitch_delta != 0 {
                                        let next_pitch = (note.midi_note as i32 + pitch_delta)
                                            .clamp(0, 127) as u8;
                                        note.midi_note = next_pitch;
                                    }
                                }
                            }
                        }
                        self.sync_track_audio_notes(track_index);
                    }
                }
                return;
            }
        }
        if self.selected_clip.is_some() {
            let nudge_beats = if input.modifiers.shift {
                4.0
            } else {
                self.piano_snap.max(0.25)
            };
            let mut beat_delta = 0.0f32;
            let mut track_delta: i32 = 0;
            if input.key_pressed(egui::Key::ArrowLeft) {
                beat_delta = -nudge_beats;
            } else if input.key_pressed(egui::Key::ArrowRight) {
                beat_delta = nudge_beats;
            } else if input.key_pressed(egui::Key::ArrowUp) {
                track_delta = -1;
            } else if input.key_pressed(egui::Key::ArrowDown) {
                track_delta = 1;
            }
            if beat_delta.abs() > f32::EPSILON || track_delta != 0 {
                if let Some(clip_id) = self.selected_clip {
                    let mut clip_info = None;
                    for (track_index, track) in self.tracks.iter().enumerate() {
                        if let Some(clip) = track.clips.iter().find(|c| c.id == clip_id) {
                            clip_info = Some((
                                track_index,
                                clip.start_beats,
                                clip.length_beats,
                                clip.is_midi,
                            ));
                            break;
                        }
                    }
                    if let Some((track_index, start_beats, _length_beats, is_midi)) = clip_info {
                        let target_track = if track_delta != 0 {
                            let next = track_index as i32 + track_delta;
                            if next >= 0 && next < self.tracks.len() as i32 {
                                next as usize
                            } else {
                                track_index
                            }
                        } else {
                            track_index
                        };
                        let new_start = (start_beats + beat_delta).max(0.0);
                        if target_track != track_index || (new_start - start_beats).abs() > f32::EPSILON {
                            self.push_undo_state();
                            if is_midi
                                && (beat_delta.abs() > f32::EPSILON
                                    || target_track != track_index)
                            {
                                self.shift_clip_notes_by_delta(clip_id, new_start - start_beats);
                            }
                            self.move_clip_by_id(clip_id, target_track, new_start);
                            if is_midi {
                                self.sync_track_audio_notes(track_index);
                                if target_track != track_index {
                                    self.sync_track_audio_notes(target_track);
                                }
                            }
                            self.selected_track = Some(target_track);
                        }
                    }
                }
                return;
            }
        }
        if input.modifiers.ctrl && input.key_pressed(egui::Key::Z) {
            self.undo();
        }
        if input.modifiers.ctrl && input.key_pressed(egui::Key::Y) {
            self.redo();
        }
        if input.modifiers.ctrl && input.key_pressed(egui::Key::S) {
            let _ = self.save_project_or_prompt();
        }
        if input.modifiers.ctrl && input.modifiers.shift && input.key_pressed(egui::Key::S) {
            let _ = self.save_project_dialog();
        }
        if input.modifiers.ctrl && input.key_pressed(egui::Key::O) {
            self.request_project_action(ProjectAction::OpenProject);
        }
        if input.modifiers.ctrl && input.key_pressed(egui::Key::N) {
            self.request_project_action(ProjectAction::NewProject);
        }
        if input.modifiers.ctrl && input.key_pressed(egui::Key::Comma) {
            self.show_settings = true;
        }
        if input.key_pressed(egui::Key::Space) {
            if self.audio_running {
                if self.is_recording {
                    let _ = self.end_recording();
                } else {
                    self.pause_audio_and_midi();
                    self.status = "Paused".to_string();
                }
            } else {
                self.seek_playhead(self.playhead_beats);
                if let Err(err) = self.start_audio_and_midi_internal(false) {
                    self.status = format!("Play failed: {err}");
                }
            }
        }
        if input.key_pressed(egui::Key::R) {
            self.toggle_recording();
        }
        if input.key_pressed(egui::Key::Delete) || input.key_pressed(egui::Key::Backspace) {
            if !self.selected_clips.is_empty() {
                let mut ids: Vec<usize> = self.selected_clips.iter().copied().collect();
                ids.sort_unstable();
                self.push_undo_state();
                for clip_id in ids {
                    self.remove_clip_and_notes_by_id(clip_id);
                }
                self.selected_clips.clear();
                self.selected_clip = None;
            } else if let Some(clip_id) = self.selected_clip {
                self.push_undo_state();
                self.remove_clip_and_notes_by_id(clip_id);
                self.selected_clip = None;
            }
        }
        if input.modifiers.ctrl && input.key_pressed(egui::Key::D) {
            self.duplicate_selected_track();
        }
        if input.modifiers.ctrl && input.key_pressed(egui::Key::E) {
            self.show_render_dialog = true;
        }
    }

    pub(crate) fn sync_selected_track_index(&self) {
        let index = self.selected_track.unwrap_or(usize::MAX);
        self.engine
            .selected_track_index
            .store(index, Ordering::Relaxed);
    }

    pub(crate) fn mark_dirty(&mut self) {
        self.project_dirty = true;
        if self.last_autosave_at.is_none() {
            self.last_autosave_at = Some(std::time::Instant::now());
        }
    }

    pub(crate) fn clear_dirty(&mut self) {
        self.project_dirty = false;
        self.last_autosave_at = None;
    }

    pub(crate) fn update_autosave(&mut self) {
        let minutes = self.settings.autosave_minutes;
        if minutes == 0 || !self.project_dirty {
            return;
        }
        let interval = std::time::Duration::from_secs(minutes.saturating_mul(60) as u64);
        let now = std::time::Instant::now();
        let last = self.last_autosave_at.unwrap_or(now);
        if now.duration_since(last) >= interval {
            let result = self.save_project();
            if let Err(err) = result {
                self.status = format!("Autosave failed: {err}");
            } else {
                self.status = format!("Autosaved {}", self.project_path);
            }
            self.last_autosave_at = Some(now);
        } else if self.last_autosave_at.is_none() {
            self.last_autosave_at = Some(now);
        }
    }

    pub(crate) fn request_project_action(&mut self, action: ProjectAction) {
        if self.project_dirty {
            self.pending_project_action = Some(action);
            self.show_close_confirm = true;
        } else {
            self.pending_project_action = Some(action);
        }
    }

    pub(crate) fn perform_project_action(&mut self, action: ProjectAction) {
        match action {
            ProjectAction::NewProject => {
                self.new_project();
            }
            ProjectAction::OpenProject => {
                if let Err(err) = self.open_project_dialog() {
                    self.status = format!("Open failed: {err}");
                }
            }
            ProjectAction::OpenProjectPath(path) => {
                if let Err(err) = self.open_project_from_path(&path) {
                    self.status = format!("Open failed: {err}");
                }
            }
            ProjectAction::ImportMidi => {
                if let Err(err) = self.import_midi_dialog() {
                    self.status = format!("Import failed: {err}");
                }
            }
            ProjectAction::NewFromTemplate(path) => {
                if let Err(err) = self.load_template_from_path(&path) {
                    self.status = format!("Template failed: {err}");
                }
            }
        }
    }

    pub(crate) fn sync_track_audio_states(&mut self) {
        self.rebuild_all_track_midi_notes();
        for track in &mut self.tracks {
            if track
                .instrument_path
                .as_deref()
                .map(Self::is_treesynth_path)
                .unwrap_or(false)
                && track.treesynth.is_none()
            {
                track.treesynth = Some(TreeSynthState::default());
            }
        }
        if self.engine.track_audio.len() != self.tracks.len() {
            self.engine.track_audio = self
                .tracks
                .iter()
                .map(TrackAudioState::from_track)
                .collect();
        } else {
            for (index, track) in self.tracks.iter().enumerate() {
                if let Some(state) = self.engine.track_audio.get_mut(index) {
                    state.sync_notes(track);
                    state.sync_automation(track);
                    state.sync_effect_bypass(track);
                    let enabled = track
                        .instrument_path
                        .as_deref()
                        .map(Self::is_treesynth_path)
                        .unwrap_or(false);
                    state.sync_treesynth(track, enabled, &self.engine.audio_cache);
                }
            }
        }
        self.sync_node_activity();
        self.sync_track_mix();
    }

    pub(crate) fn plugin_path_exists(path: &str) -> bool {
        if Self::is_native_plugin_path(path) {
            return true;
        }
        let input = PathBuf::from(path);
        if input.exists() {
            return true;
        }
        if input.is_absolute() {
            return false;
        }
        if let Ok(current_dir) = std::env::current_dir() {
            if current_dir.join(&input).exists() {
                return true;
            }
        }
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                if exe_dir.join(&input).exists() {
                    return true;
                }
            }
        }
        false
    }

    pub(crate) fn clear_missing_plugin_references(&mut self) -> (usize, usize) {
        let mut missing_instruments = 0usize;
        let mut missing_effects = 0usize;

        for track in &mut self.tracks {
            let missing_instrument = track
                .instrument_path
                .as_deref()
                .map(|path| !Self::plugin_path_exists(path) && !Self::is_treesynth_path(path))
                .unwrap_or(false);
            if missing_instrument {
                track.instrument_path = None;
                track.instrument_clap_id = None;
                track.plugin_state_component = None;
                track.plugin_state_controller = None;
                track.params = default_midi_params();
                track.param_ids.clear();
                track.param_values.clear();
                missing_instruments = missing_instruments.saturating_add(1);
            }

            if track.effect_paths.is_empty() {
                continue;
            }

            let effect_paths = std::mem::take(&mut track.effect_paths);
            let effect_clap_ids = std::mem::take(&mut track.effect_clap_ids);
            let effect_bypass = std::mem::take(&mut track.effect_bypass);
            let effect_params = std::mem::take(&mut track.effect_params);
            let effect_param_ids = std::mem::take(&mut track.effect_param_ids);
            let effect_param_values = std::mem::take(&mut track.effect_param_values);

            for (fx_index, path) in effect_paths.into_iter().enumerate() {
                if Self::plugin_path_exists(&path) {
                    track.effect_paths.push(path);
                    track.effect_clap_ids.push(effect_clap_ids.get(fx_index).cloned().unwrap_or(None));
                    track.effect_bypass.push(effect_bypass.get(fx_index).copied().unwrap_or(false));
                    track.effect_params.push(effect_params.get(fx_index).cloned().unwrap_or_default());
                    track.effect_param_ids.push(effect_param_ids.get(fx_index).cloned().unwrap_or_default());
                    track.effect_param_values.push(effect_param_values.get(fx_index).cloned().unwrap_or_default());
                } else {
                    missing_effects = missing_effects.saturating_add(1);
                }
            }
        }

        (missing_instruments, missing_effects)
    }

    pub(crate) fn sync_node_activity(&mut self) {
        let mut activity = self.engine.node_activity.lock();
        if activity.len() < self.tracks.len() {
            activity.resize(self.tracks.len(), TrackNodeActivity::default());
        } else if activity.len() > self.tracks.len() {
            activity.truncate(self.tracks.len());
        }
        for (idx, track) in self.tracks.iter().enumerate() {
            if let Some(slot) = activity.get_mut(idx) {
                let fx_count = track.effect_paths.len();
                if slot.fx_input_peaks.len() != fx_count {
                    slot.fx_input_peaks.resize(fx_count, 0.0);
                }
                if slot.fx_output_peaks.len() != fx_count {
                    slot.fx_output_peaks.resize(fx_count, 0.0);
                }
            }
        }
    }

    pub(crate) fn sync_track_mix(&mut self) {
        {
            let mut mix = self.engine.track_mix.lock();
            mix.clear();
            for track in &self.tracks {
                mix.push(TrackMixState {
                    muted: track.muted,
                    solo: track.solo,
                    level: track.level,
                    active: true,
                });
            }
        }
        self.sync_node_routes();
    }

    pub(crate) fn sync_node_routes(&mut self) {
        self.node_routes = Self::sanitize_node_routes(self.node_routes.clone(), &self.tracks);
        self.performance_clip_settings = Self::sanitize_performance_clip_settings(
            self.performance_clip_settings.clone(),
            &self.tracks,
        );
        self.sync_performance_runtime();
        if let Some(clip_id) = self.performance_selected_clip {
            if self.find_clip_indices_by_id(clip_id).is_none() {
                self.performance_selected_clip = None;
            }
        }
        *self.engine.node_routes.lock() = self.node_routes.clone();
    }

    pub(crate) fn sync_performance_runtime(&mut self) {
        let track_count = self.tracks.len();
        let mut runtime = self.engine.performance_runtime.lock();
        if runtime.len() < track_count {
            runtime.resize(track_count, None);
        } else if runtime.len() > track_count {
            runtime.truncate(track_count);
        }
        for (track_index, slot) in runtime.iter_mut().enumerate() {
            let Some(active) = slot.as_ref() else {
                continue;
            };
            let still_exists = self
                .find_clip_indices_by_id(active.clip.id)
                .map(|(ti, _)| ti == track_index)
                .unwrap_or(false);
            if !still_exists {
                *slot = None;
            }
        }
    }

    pub(crate) fn rebuild_all_track_midi_notes(&mut self) {
        for index in 0..self.tracks.len() {
            self.rebuild_track_midi_notes(index);
        }
    }

    pub(crate) fn midi_loop_len_for_clip(clip: &Clip) -> Option<f32> {
        if !clip.is_midi {
            return None;
        }
        let loop_len = clip.midi_source_beats.unwrap_or(clip.length_beats);
        if loop_len <= 0.0 {
            return None;
        }
        let clip_start = clip.start_beats;
        let loop_end = clip_start + loop_len;
        let has_outside = clip
            .midi_notes
            .iter()
            .any(|note| note.start_beats < clip_start || note.start_beats >= loop_end);
        if has_outside {
            return None;
        }
        Some(loop_len)
    }

    pub(crate) fn rebuild_track_midi_notes(&mut self, index: usize) {
        let Some(track) = self.tracks.get_mut(index) else {
            return;
        };
        track.midi_notes.clear();
        for clip in &track.clips {
            if !clip.is_midi || clip.midi_notes.is_empty() {
                continue;
            }
            let loop_len = Self::midi_loop_len_for_clip(clip);
            if let Some(loop_len) = loop_len {
                let clip_start = clip.start_beats;
                let clip_end = clip.start_beats + clip.length_beats;
                if clip.length_beats > loop_len + 0.0001 {
                    for note in &clip.midi_notes {
                        let rel = note.start_beats - clip_start;
                        if rel < 0.0 || rel >= loop_len {
                            continue;
                        }
                        let mut t = clip_start + rel;
                        while t < clip_end {
                            let mut cloned = note.clone();
                            cloned.start_beats = t;
                            track.midi_notes.push(cloned);
                            t += loop_len;
                        }
                    }
                    continue;
                }
            }
            track.midi_notes.extend(clip.midi_notes.iter().cloned());
        }
        if !track.midi_notes.is_empty() {
            track.midi_notes.sort_by(|a, b| {
                a.start_beats
                    .partial_cmp(&b.start_beats)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
    }

    pub(crate) fn sync_track_audio_notes(&mut self, index: usize) {
        self.rebuild_track_midi_notes(index);
        if let Some(track) = self.tracks.get(index) {
            if let Some(state) = self.engine.track_audio.get(index) {
                state.sync_notes(track);
            }
        }
    }

    pub(crate) fn selected_track_host(&self) -> Option<PluginHostHandle> {
        let index = self.selected_track?;
        self.engine.track_audio.get(index).and_then(|state| state.host.clone())
    }

    pub(crate) fn ensure_track_host(&mut self, index: usize, channels: usize) -> Option<PluginHostHandle> {
        let path = self.tracks.get(index).and_then(|t| t.instrument_path.clone())?;
        if !Self::plugin_path_exists(&path) && !Self::is_treesynth_path(&path) {
            if let Some(track) = self.tracks.get_mut(index) {
                track.instrument_path = None;
                track.instrument_clap_id = None;
                track.plugin_state_component = None;
                track.plugin_state_controller = None;
                track.params = default_midi_params();
                track.param_ids.clear();
                track.param_values.clear();
            }
            self.status = format!("Missing instrument cleared: {}", Self::plugin_display_name(&path));
            return None;
        }
        let state = self.engine.track_audio.get_mut(index)?;
        if let Some(host) = state.host.as_ref() {
            return Some(host.clone());
        }
        let kind = Self::plugin_kind_from_path(&path);
        if kind == PluginKind::Native {
            return None;
        }
        let host = match kind {
            PluginKind::Native => None,
            PluginKind::Vst3 => {
                let host = vst3::Vst3Host::load(
                    &path,
                    self.settings.sample_rate as f64,
                    self.settings.buffer_size as usize,
                    channels.max(1),
                )
                .ok()?;
                Some(PluginHostHandle::Vst3(Arc::new(ParkingMutex::new(host))))
            }
            PluginKind::Clap => {
                let clap_id = self
                    .tracks
                    .get(index)
                    .and_then(|t| t.instrument_clap_id.clone())
                    .or_else(|| clap_host::default_plugin_id(&path).ok())?;
                if let Some(track) = self.tracks.get_mut(index) {
                    track.instrument_clap_id = Some(clap_id.clone());
                }
                let host = clap_host::ClapHost::load(
                    &path,
                    &clap_id,
                    self.settings.sample_rate as f64,
                    self.settings.buffer_size,
                    0,
                    channels.clamp(1, MAX_CLAP_OUTPUT_CHANNELS),
                )
                .ok()?;
                Some(PluginHostHandle::Clap(Arc::new(ParkingMutex::new(host))))
            }
        };
        let host = host?;
        state.host = Some(host.clone());
        Some(host)
    }

    pub(crate) fn ensure_effect_host(
        &mut self,
        track_index: usize,
        effect_index: usize,
        channels: usize,
    ) -> Option<PluginHostHandle> {
        let missing_effect_path = self
            .tracks
            .get(track_index)
            .and_then(|track| track.effect_paths.get(effect_index))
            .cloned()
            .filter(|path| !Self::plugin_path_exists(path));
        if let Some(path) = missing_effect_path {
            if let Some(track) = self.tracks.get_mut(track_index) {
                if effect_index < track.effect_paths.len() {
                    track.effect_paths.remove(effect_index);
                }
                if effect_index < track.effect_clap_ids.len() {
                    track.effect_clap_ids.remove(effect_index);
                }
                if effect_index < track.effect_bypass.len() {
                    track.effect_bypass.remove(effect_index);
                }
                if effect_index < track.effect_params.len() {
                    track.effect_params.remove(effect_index);
                }
                if effect_index < track.effect_param_ids.len() {
                    track.effect_param_ids.remove(effect_index);
                }
                if effect_index < track.effect_param_values.len() {
                    track.effect_param_values.remove(effect_index);
                }
            }
            if let Some(state) = self.engine.track_audio.get_mut(track_index) {
                if effect_index < state.effect_hosts.len() {
                    let mut host = state.effect_hosts.remove(effect_index);
                    host.prepare_for_drop();
                    self.orphaned_hosts.push(host);
                }
            }
            self.status = format!("Missing effect cleared: {}", Self::plugin_display_name(&path));
            return None;
        }
        let state = self.engine.track_audio.get_mut(track_index)?;
        let (paths, clap_ids) = {
            let track = self.tracks.get(track_index)?;
            (track.effect_paths.clone(), track.effect_clap_ids.clone())
        };
        if state.effect_hosts.len() != paths.len() {
            for mut host in state.effect_hosts.drain(..) {
                host.prepare_for_drop();
                self.orphaned_hosts.push(host);
            }
            for (slot, path) in paths.iter().enumerate() {
                let kind = Self::plugin_kind_from_path(path);
                let host = match kind {
                    PluginKind::Native => None,
                    PluginKind::Vst3 => vst3::Vst3Host::load_with_input(
                        path,
                        self.settings.sample_rate as f64,
                        self.settings.buffer_size as usize,
                        channels,
                        channels,
                    )
                    .ok()
                    .map(|host| PluginHostHandle::Vst3(Arc::new(ParkingMutex::new(host)))),
                    PluginKind::Clap => {
                        let clap_id = clap_ids
                            .get(slot)
                            .and_then(|id| id.clone())
                            .or_else(|| clap_host::default_plugin_id(path).ok());
                        clap_id.and_then(|clap_id| {
                            if let Some(track) = self.tracks.get_mut(track_index) {
                                if track.effect_clap_ids.len() < paths.len() {
                                    track.effect_clap_ids.resize(paths.len(), None);
                                }
                                track.effect_clap_ids[slot] = Some(clap_id.clone());
                            }
                            clap_host::ClapHost::load(
                                path,
                                &clap_id,
                                self.settings.sample_rate as f64,
                                self.settings.buffer_size,
                                channels,
                                channels.min(MAX_CLAP_OUTPUT_CHANNELS),
                            )
                            .ok()
                            .map(|host| PluginHostHandle::Clap(Arc::new(ParkingMutex::new(host))))
                        })
                    }
                };
                if let Some(host) = host {
                    state.effect_hosts.push(host);
                }
            }
        }
        state.effect_hosts.get(effect_index).cloned()
    }

    pub(crate) fn draw_effect_params_panel(
        &mut self,
        ui: &mut egui::Ui,
        track_index: usize,
        track_color: Option<egui::Color32>,
        pending_automation_record: &mut Vec<(usize, RecordedAutomationPoint)>,
    ) {
        let (effect_paths, needs_params) = if let Some(track) = self.tracks.get(track_index) {
            let paths = track.effect_paths.clone();
            let needs = paths
                .iter()
                .enumerate()
                .map(|(fx_index, _)| {
                    track
                        .effect_params
                        .get(fx_index)
                        .map(|p| p.is_empty())
                        .unwrap_or(true)
                })
                .collect::<Vec<_>>();
            (paths, needs)
        } else {
            return;
        };
        type FxParamSnapshot = (usize, Vec<String>, Vec<u32>, Vec<f32>);
        let mut fx_updates: Vec<FxParamSnapshot> = Vec::new();
        for (fx_index, _) in effect_paths.iter().enumerate() {
            if !needs_params.get(fx_index).copied().unwrap_or(true) {
                continue;
            }
            if !self.audio_running {
                continue;
            }
            if let Some(host) = self.ensure_effect_host(track_index, fx_index, 2) {
                let params = host.enumerate_params();
                if !params.is_empty() {
                    let names = params.iter().map(|p| p.name.clone()).collect();
                    let ids = params.iter().map(|p| p.id).collect();
                    let values = params.iter().map(|p| p.default_value as f32).collect();
                    fx_updates.push((fx_index, names, ids, values));
                }
            }
        }
        if let Some(track) = self.tracks.get_mut(track_index) {
            for (fx_index, names, ids, values) in fx_updates {
                if track.effect_params.len() <= fx_index {
                    track.effect_params.resize(fx_index + 1, Vec::new());
                    track.effect_param_ids.resize(fx_index + 1, Vec::new());
                    track.effect_param_values.resize(fx_index + 1, Vec::new());
                }
                track.effect_params[fx_index] = names;
                track.effect_param_ids[fx_index] = ids;
                if track.effect_param_values[fx_index].is_empty() {
                    track.effect_param_values[fx_index] = values;
                }
            }
            ui.separator();
            ui.label("Effects Params");
            if track.effect_paths.is_empty() {
                ui.label("(no effects on this track)");
                return;
            }
            let menu_color = track_color.unwrap_or(egui::Color32::from_rgb(120, 160, 220));
            for (fx_index, fx_path) in track.effect_paths.iter().enumerate() {
                let title = format!(
                    "FX {}: {}",
                    fx_index + 1,
                    Self::plugin_display_name(fx_path)
                );
                egui::CollapsingHeader::new(title)
                    .default_open(true)
                    .show(ui, |ui| {
                    if ui
                        .add(egui::Button::image(
                            egui::Image::new(egui::include_image!("../../../assets/icons/eye.svg"))
                                .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                        ))
                        .on_hover_text("Open UI")
                        .clicked()
                    {
                        self.plugin_ui_target = Some(PluginUiTarget::Effect(track_index, fx_index));
                        self.show_plugin_ui = true;
                    }
                    let params = track
                        .effect_params
                        .get(fx_index)
                        .cloned()
                        .unwrap_or_default();
                    if params.is_empty() {
                        ui.label("(no parameters)");
                        return;
                    }
                    if let Some(values) = track.effect_param_values.get_mut(fx_index) {
                        if values.len() != params.len() {
                            values.resize(params.len(), 0.0);
                        }
                    }
                    let ids = track
                        .effect_param_ids
                        .get(fx_index)
                        .cloned()
                        .unwrap_or_default();
                    for (param_index, label) in params.iter().enumerate() {
                        let label = label.clone();
                        let value = track
                            .effect_param_values
                            .get_mut(fx_index)
                            .and_then(|vals| vals.get_mut(param_index));
                        let Some(value) = value else {
                            continue;
                        };
                        let slider = ui.push_id(
                            format!("fx{}_param_{}", fx_index, label),
                            |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(&label);
                                    Self::colored_slider(ui, value, 0.0..=1.0, track_color)
                                })
                                .inner
                            },
                        );
                        let response = slider.response;
                        let slider_response = slider.inner;
                        let changed = slider_response.changed()
                            || slider_response.dragged()
                            || response.dragged();
                        if changed {
                            if let Some(param_id) = ids.get(param_index).copied() {
                                if let Some(state) = self.engine.track_audio.get(track_index) {
                                    if state.effect_hosts.get(fx_index).is_some() {
                                        {
                                            let mut pending = state.pending_param_changes.lock();
                                            pending.push(PendingParamChange {
                                                target: PendingParamTarget::Effect(fx_index),
                                                param_id,
                                                value: *value as f64,
                                            });
                                        }
                                    }
                                }
                                if self.is_recording && self.record_automation {
                                    pending_automation_record.push((
                                        track_index,
                                        RecordedAutomationPoint {
                                            param_id,
                                            target: AutomationTarget::Effect(fx_index),
                                            beat: self.playhead_beats,
                                            value: *value,
                                        },
                                    ));
                                }
                            }
                        }
                        response.context_menu(|ui| {
                            if ui
                                .add(egui::Button::image_and_text(
                                    egui::Image::new(egui::include_image!(
                                        "../../../assets/icons/target.svg"
                                    ))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0))
                                    .tint(menu_color),
                                    egui::RichText::new("MIDI Learn").color(menu_color),
                                ))
                                .clicked()
                            {
                                if let Some(param_id) = ids.get(param_index).copied() {
                                    if let Ok(mut learn) = self.midi_learn.lock() {
                                        *learn = Some((track_index, param_id));
                                    }
                                    self.status = format!("MIDI Learn armed for {}", label);
                                }
                                ui.close_menu();
                            }
                            if ui
                                .add(egui::Button::image_and_text(
                                    egui::Image::new(egui::include_image!(
                                        "../../../assets/icons/activity.svg"
                                    ))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0))
                                    .tint(menu_color),
                                    egui::RichText::new("Create Automation Lane")
                                        .color(menu_color),
                                ))
                                .clicked()
                            {
                                if let Some(param_id) = ids.get(param_index).copied() {
                                    if !track.automation_lanes.iter().any(|l| {
                                        l.param_id == param_id
                                            && l.target == AutomationTarget::Effect(fx_index)
                                    }) {
                                        track.automation_lanes.push(AutomationLane {
                                            name: format!(
                                                "{}: {}",
                                                Self::plugin_display_name(fx_path),
                                                label
                                            ),
                                            param_id,
                                            target: AutomationTarget::Effect(fx_index),
                                            points: Vec::new(),
                                        });
                                    }
                                }
                                ui.close_menu();
                            }
                        });
                    }
                });
            }
        }
    }

    pub(crate) fn plugin_ui_matches(&self, target: PluginUiTarget) -> bool {
        self.plugin_ui
            .as_ref()
            .map(|ui| ui.target == target)
            .unwrap_or(false)
    }

    pub(crate) fn update_playhead(&mut self, ctx: &egui::Context) {
        self.engine.tempo_bpm.store(self.tempo_bpm.to_bits(), Ordering::Relaxed);
        let now = ctx.input(|i| i.time);
        if self.audio_running {
            let samples = self.engine.transport_samples.load(Ordering::Relaxed) as f32;
            let sample_rate = self.settings.sample_rate.max(1) as f32;
            let seconds = samples / sample_rate;
            if self.arrangement_playback_enabled() {
                self.playhead_beats = seconds * (self.tempo_bpm / 60.0);
            }
            self.last_frame_time = Some(now);
        } else {
            self.last_frame_time = None;
        }
        if self.arrangement_playback_enabled() {
            if let (Some(start), Some(end)) = (self.loop_start_beats, self.loop_end_beats) {
                if end > start && self.playhead_beats >= end {
                    self.seek_playhead(start);
                }
            }
        }
        self.update_loop_samples();
    }

    pub(crate) fn update_loop_samples(&mut self) {
        if let (Some(start), Some(end)) = (self.loop_start_beats, self.loop_end_beats) {
            if end > start {
                let start_samples = self.beats_to_samples(start, self.settings.sample_rate);
                let end_samples = self.beats_to_samples(end, self.settings.sample_rate);
                self.engine.loop_start_samples.store(start_samples, Ordering::Relaxed);
                self.engine.loop_end_samples.store(end_samples.max(start_samples + 1), Ordering::Relaxed);
                return;
            }
        }
        self.engine.loop_start_samples.store(0, Ordering::Relaxed);
        self.engine.loop_end_samples.store(0, Ordering::Relaxed);
    }

    pub(crate) fn seek_playhead(&mut self, beats: f32) {
        let beats = beats.max(0.0);
        self.playhead_beats = beats;
        let tempo = self.tempo_bpm.max(1.0);
        let seconds = beats * 60.0 / tempo;
        let samples = (seconds * self.settings.sample_rate as f32).max(0.0) as u64;
        self.engine.transport_samples.store(samples, Ordering::Relaxed);
        self.last_frame_time = None;
    }

    pub(crate) fn beats_from_pos(&self, pos_x: f32, row_left: f32, beat_width: f32) -> f32 {
        ((pos_x - row_left) / beat_width).max(0.0)
    }

    pub(crate) fn play_startup_sound(&mut self) -> Result<(), String> {
        let bytes = include_bytes!("../../../assets/startup.wav");
        let reader = BufReader::new(std::io::Cursor::new(bytes));
        let (stream, handle) = OutputStream::try_default().map_err(|e| e.to_string())?;
        let sink = Sink::try_new(&handle).map_err(|e| e.to_string())?;
        let source = Decoder::new(reader).map_err(|e| e.to_string())?;
        sink.append(source);
        self.startup_stream = Some(stream);
        self.startup_sink = Some(sink);
        Ok(())
    }

    pub(crate) fn snapshot_state(&self) -> UndoState {
        UndoState {
            project_name: self.project_name.clone(),
            tempo_bpm: self.tempo_bpm,
            tracks: self.tracks.clone(),
            selected_clip: self.selected_clip,
            selected_track: self.selected_track,
        }
    }

    pub(crate) fn restore_state(&mut self, state: UndoState) {
        self.project_name = state.project_name;
        self.tempo_bpm = state.tempo_bpm;
        self.tracks = state.tracks;
        self.selected_clip = state.selected_clip;
        self.selected_track = state.selected_track;
    }

    pub(crate) fn push_undo_state(&mut self) {
        if self.undo_stack.len() >= Self::UNDO_LIMIT {
            self.undo_stack.remove(0);
        }
        self.undo_stack.push(self.snapshot_state());
        self.redo_stack.clear();
        self.mark_dirty();
    }

    pub(crate) fn undo(&mut self) {
        if let Some(state) = self.undo_stack.pop() {
            let current = self.snapshot_state();
            self.redo_stack.push(current);
            self.restore_state(state);
            self.status = "Undo".to_string();
        }
    }

    pub(crate) fn redo(&mut self) {
        if let Some(state) = self.redo_stack.pop() {
            let current = self.snapshot_state();
            self.undo_stack.push(current);
            self.restore_state(state);
            self.status = "Redo".to_string();
        }
    }
}
