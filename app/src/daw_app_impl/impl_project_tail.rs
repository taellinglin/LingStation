#[allow(dead_code)]
impl DawApp {
    pub(crate) fn new_project(&mut self) {
        self.prepare_for_project_change();

        self.project_name = "Untitled Project".to_string();
        self.project_path = String::new();
        self.metadata_artist.clear();
        self.metadata_title.clear();
        self.metadata_album.clear();
        self.metadata_genre.clear();
        self.metadata_year.clear();
        self.metadata_comment.clear();
        self.tracks = vec![Track {
            name: "Track 1".to_string(),
            clips: Vec::new(),
            level: 0.8,
            muted: false,
            solo: false,
            midi_notes: Vec::new(),
            instrument_path: None,
            instrument_clap_id: None,
            effect_paths: Vec::new(),
            effect_clap_ids: Vec::new(),
            effect_bypass: Vec::new(),
            effect_params: Vec::new(),
            effect_param_ids: Vec::new(),
            effect_param_values: Vec::new(),
            params: default_midi_params(),
            param_ids: Vec::new(),
            param_values: Vec::new(),
            plugin_state_component: None,
            plugin_state_controller: None,
            automation_lanes: Vec::new(),
            automation_channels: Vec::new(),
            midi_cc_lanes: Vec::new(),
            midi_program: None,
            treesynth: None,
        }];
        self.selected_clip = None;
        self.selected_track = Some(0);
        self.playhead_beats = 0.0;
        if let Ok(mut master) = self.master_settings.lock() {
            *master = MasterCompSettings::default();
        }
        if let Ok(mut state) = self.master_comp_state.lock() {
            *state = MasterCompState::default();
        }
        self.performance_clip_settings.clear();
        self.performance_selected_clip = None;
        self.node_routes = Self::default_node_routes(&self.tracks);
        self.sync_node_routes();
        self.sync_track_audio_states();
        self.clear_dirty();
        self.status = "New project".to_string();
    }

    pub(crate) fn prepare_for_project_change(&mut self) {
        if self.is_recording {
            let _ = self.end_recording();
        }
        self.stop_audio_preview();
        self.plugin_ui_resume_at = None;
        self.show_plugin_ui = false;
        let plugin_hwnd = self.plugin_ui.as_ref().map(|ui_host| ui_host.hwnd);
        if let Some(ui_host) = self.plugin_ui.as_ref() {
            if let PluginUiEditor::Vst3(editor) = &ui_host.editor {
                editor.set_focus(false);
            }
            hide_plugin_window(ui_host.hwnd);
            release_mouse_capture();
        }
        self.destroy_plugin_ui();
        if let Some(hwnd) = plugin_hwnd {
            pump_plugin_messages(hwnd);
        }
        self.plugin_ui_hidden = false;
        self.pending_viewport_focus = true;
        self.pending_repaint_frames = 12;
        if self.audio_running {
            self.stop_audio_and_midi();
        }
        let mut hosts: Vec<PluginHostHandle> = Vec::new();
        for state in self.track_audio.iter_mut() {
            if let Some(host) = state.host.take() {
                host.prepare_for_drop();
                hosts.push(host);
            }
            for host in state.effect_hosts.drain(..) {
                host.prepare_for_drop();
                hosts.push(host);
            }
        }
        self.orphaned_hosts.extend(hosts);
        self.track_audio.clear();
        self.waveform_cache.borrow_mut().clear();
        self.waveform_color_cache.borrow_mut().clear();
        self.waveform_len_seconds_cache.borrow_mut().clear();
        self.waveform_cache_order.borrow_mut().clear();
        self.waveform_color_cache_order.borrow_mut().clear();
        self.waveform_len_seconds_cache_order.borrow_mut().clear();
        if let Ok(mut cache) = self.audio_clip_cache.lock() {
            cache.clear();
        }
        if let Ok(mut timeline) = self.audio_clip_timeline.lock() {
            timeline.clear();
        }
    }

    pub(crate) fn add_track(&mut self) {
        let index = self.tracks.len() + 1;
        self.tracks.push(Track {
            name: format!("Track {}", index),
            clips: Vec::new(),
            level: 0.8,
            muted: false,
            solo: false,
            midi_notes: Vec::new(),
            instrument_path: None,
            instrument_clap_id: None,
            effect_paths: Vec::new(),
            effect_clap_ids: Vec::new(),
            effect_bypass: Vec::new(),
            effect_params: Vec::new(),
            effect_param_ids: Vec::new(),
            effect_param_values: Vec::new(),
            params: default_midi_params(),
            param_ids: Vec::new(),
            param_values: Vec::new(),
            plugin_state_component: None,
            plugin_state_controller: None,
            automation_lanes: Vec::new(),
            automation_channels: Vec::new(),
            midi_cc_lanes: Vec::new(),
            midi_program: None,
            treesynth: None,
        });
        self.selected_track = Some(self.tracks.len().saturating_sub(1));
        self.refresh_params_for_selected_track(true);
        if let Some(track) = self.tracks.last() {
            self.track_audio.push(TrackAudioState::from_track(track));
        }
        self.sync_track_mix();
        self.mark_dirty();
        self.status = "Track added".to_string();
    }

    pub(crate) fn remove_selected_track(&mut self) {
        if let Some(index) = self.selected_track {
            if self.tracks.len() > 1 {
                if self
                    .plugin_ui
                    .as_ref()
                    .map(|ui| matches!(ui.target, PluginUiTarget::Instrument(ti) | PluginUiTarget::Effect(ti, _) if ti == index))
                    .unwrap_or(false)
                {
                    self.show_plugin_ui = false;
                    self.destroy_plugin_ui();
                }
                self.tracks.remove(index);
                if index < self.track_audio.len() {
                    let mut state = self.track_audio.remove(index);
                    if let Some(host) = state.host.take() {
                        host.prepare_for_drop();
                        self.orphaned_hosts.push(host);
                    }
                    for host in state.effect_hosts.drain(..) {
                        host.prepare_for_drop();
                        self.orphaned_hosts.push(host);
                    }
                }
                let next = index.saturating_sub(1).min(self.tracks.len().saturating_sub(1));
                self.selected_track = Some(next);
                self.sync_track_mix();
                self.mark_dirty();
                self.status = "Track removed".to_string();
            } else {
                self.status = "At least one track required".to_string();
            }
        }
    }

    pub(crate) fn duplicate_selected_track(&mut self) {
        if let Some(index) = self.selected_track {
            if let Some(track) = self.tracks.get(index).cloned() {
                let mut dup = track.clone();
                let new_index = index + 1;
                dup.name = format!("{} Copy", track.name);
                for clip in &mut dup.clips {
                    clip.id = self.next_clip_id();
                    clip.track = new_index;
                }
                self.tracks.insert(new_index, dup);
                let state = TrackAudioState::from_track(&track);
                self.track_audio.insert(new_index, state);
                self.selected_track = Some(new_index);
                self.sync_track_mix();
                self.mark_dirty();
                self.status = "Track duplicated".to_string();
            }
        }
    }

    pub(crate) fn clone_selected_track(&mut self) {
        if let Some(index) = self.selected_track {
            if let Some(track) = self.tracks.get(index).cloned() {
                let mut clone = track.clone();
                clone.clips.clear();
                clone.name = format!("{} Clone", clone.name);
                self.tracks.insert(index + 1, clone);
                let state = TrackAudioState::from_track(&track);
                self.track_audio.insert(index + 1, state);
                self.selected_track = Some(index + 1);
                self.sync_track_mix();
                self.mark_dirty();
                self.status = "Track cloned".to_string();
            }
        }
    }

    pub(crate) fn begin_rename_selected_track(&mut self) {
        if let Some(index) = self.selected_track {
            if let Some(track) = self.tracks.get(index) {
                self.rename_buffer = track.name.clone();
                self.show_rename_track = true;
            }
        }
    }

    pub(crate) fn begin_rename_clip(&mut self, track_index: usize, clip_id: usize) {
        let name = self
            .tracks
            .get(track_index)
            .and_then(|track| track.clips.iter().find(|c| c.id == clip_id))
            .map(|clip| clip.name.clone())
            .unwrap_or_else(|| "Clip".to_string());
        self.rename_clip_buffer = name;
        self.rename_clip_target = Some((track_index, clip_id));
        self.show_rename_clip = true;
    }

    pub(crate) fn apply_rename(&mut self) {
        if let Some(index) = self.selected_track {
            if let Some(track) = self.tracks.get_mut(index) {
                let name = self.rename_buffer.trim();
                if !name.is_empty() {
                    track.name = name.to_string();
                    self.mark_dirty();
                    self.status = "Track renamed".to_string();
                }
            }
        }
    }

    pub(crate) fn apply_rename_clip(&mut self) {
        let Some((track_index, clip_id)) = self.rename_clip_target else {
            return;
        };
        let name = self.rename_clip_buffer.trim();
        if name.is_empty() {
            return;
        }
        if let Some(track) = self.tracks.get_mut(track_index) {
            if let Some(clip) = track.clips.iter_mut().find(|c| c.id == clip_id) {
                clip.name = name.to_string();
                self.mark_dirty();
                self.status = "Clip renamed".to_string();
            }
        }
    }

    pub(crate) fn capture_plugin_states(&mut self) {
        for (index, track) in self.tracks.iter_mut().enumerate() {
            let Some(state) = self.track_audio.get(index) else {
                continue;
            };
            let Some(host) = state.host.as_ref() else {
                continue;
            };
            let (component, controller) = host.get_state_bytes();
            track.plugin_state_component = if component.is_empty() { None } else { Some(component) };
            track.plugin_state_controller = if controller.is_empty() { None } else { Some(controller) };
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
            if !has_state
                && !track.param_ids.is_empty()
                && (track.param_values.is_empty() || track.param_values.len() != track.param_ids.len())
            {
                track.param_values.resize(track.param_ids.len(), 0.0);
                for (slot, param_id) in track.param_ids.iter().enumerate() {
                    if let Some(value) = host.get_param_normalized(*param_id) {
                        if let Some(target) = track.param_values.get_mut(slot) {
                            *target = value as f32;
                        }
                    }
                }
            }
            Self::log_fm_ratio_param_from(index, "capture", &track.params, &track.param_ids, &track.param_values);
        }
    }

    pub(crate) fn save_project(&mut self) -> Result<(), String> {
        if self.project_path.trim().is_empty() {
            if let Some(folder) = self.default_project_dir() {
                return self.save_project_to_folder(&folder);
            }
            return Err("Default project folder unavailable".to_string());
        }
        let path = self.project_path.clone();
        self.save_project_to_folder(Path::new(&path))
    }

    pub(crate) fn save_project_to_folder(&mut self, folder: &Path) -> Result<(), String> {
        let previous_folder = self.project_path.trim().to_string();
        self.capture_plugin_states();
        // Ensure TreeSynth instrument_path is set
        let mut tracks = self.tracks.clone();
        for track in &mut tracks {
            if track.treesynth.is_some() {
                track.instrument_path = Some("native:treesynth".to_string());
            }
        }
        let state = ProjectState {
            name: self.project_name.clone(),
            artist: self.metadata_artist.clone(),
            title: self.metadata_title.clone(),
            album: self.metadata_album.clone(),
            genre: self.metadata_genre.clone(),
            year: self.metadata_year.clone(),
            comment: self.metadata_comment.clone(),
            project_key: self.project_key,
            project_key_minor: self.project_key_minor,
            tempo_bpm: self.tempo_bpm,
            tracks,
            node_routes: self.node_routes.clone(),
            performance_clip_settings: self.performance_clip_settings.clone(),
            performance_launch_quantize_beats: self.performance_launch_quantize_beats.max(0.0),
            master_settings: self.master_settings_snapshot(),
        };
        let folder = Self::normalize_windows_path(folder);
        fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        let midi_dir = folder.join("midi");
        let samples_dir = folder.join("assets").join("samples");
        let audio_dir = folder.join("audio");
        let renders_dir = folder.join("renders");
        fs::create_dir_all(&midi_dir).map_err(|e| e.to_string())?;
        fs::create_dir_all(&samples_dir).map_err(|e| e.to_string())?;
        fs::create_dir_all(&audio_dir).map_err(|e| e.to_string())?;
        fs::create_dir_all(&renders_dir).map_err(|e| e.to_string())?;
        if !previous_folder.is_empty() {
            let previous = PathBuf::from(previous_folder);
            self.copy_project_assets_if_needed(&previous, &folder)?;
        }

        let json = serde_json::to_string_pretty(&state).map_err(|e| e.to_string())?;
        let manifest_path = folder.join("project.json");
        fs::write(&manifest_path, json).map_err(|e| e.to_string())?;

        for (index, track) in self.tracks.iter().enumerate() {
            let safe_track = Self::sanitize_folder_name(&track.name);
            let mut wrote_clip = false;
            for clip in &track.clips {
                if !clip.is_midi {
                    continue;
                }
                let clip_start = clip.start_beats;
                let clip_end = clip.start_beats + clip.length_beats;
                let mut notes = Vec::new();
                for note in &clip.midi_notes {
                    let note_end = note.start_beats + note.length_beats;
                    if note_end < clip_start || note.start_beats > clip_end {
                        continue;
                    }
                    let mut adjusted = note.clone();
                    adjusted.start_beats = (adjusted.start_beats - clip_start).max(0.0);
                    notes.push(adjusted);
                }
                if notes.is_empty() {
                    continue;
                }
                let safe_clip = Self::sanitize_folder_name(&clip.name);
                let file_name = if safe_clip.is_empty() {
                    format!("{:02}_{}_clip{}.mid", index + 1, safe_track, clip.id)
                } else {
                    format!("{:02}_{}_{}_clip{}.mid", index + 1, safe_track, safe_clip, clip.id)
                };
                let midi_path = midi_dir.join(file_name);
                export_midi(midi_path.to_string_lossy().as_ref(), &notes, 480)?;
                wrote_clip = true;
            }

            if !wrote_clip && !track.midi_notes.is_empty() {
                let file_name = format!("{:02}_{}.mid", index + 1, safe_track);
                let midi_path = midi_dir.join(file_name);
                export_midi(midi_path.to_string_lossy().as_ref(), &track.midi_notes, 480)?;
            }
        }

        self.project_path = folder.to_string_lossy().to_string();
        if self.project_name.trim().is_empty() {
            if let Some(name) = self.project_name_from_path() {
                self.project_name = name;
            }
        }
        self.register_recent_project_path(&folder);
        self.clear_dirty();
        self.status = format!("Saved {}", self.project_path);
        Ok(())
    }

    pub(crate) fn copy_project_assets_if_needed(
        &self,
        source_folder: &Path,
        target_folder: &Path,
    ) -> Result<(), String> {
        if !source_folder.exists() {
            return Ok(());
        }
        if Self::paths_equal(source_folder, target_folder) {
            return Ok(());
        }
        for name in ["audio", "samples"] {
            let source = source_folder.join(name);
            if !source.exists() {
                continue;
            }
            let target = target_folder.join(name);
            fs::create_dir_all(&target).map_err(|e| e.to_string())?;
            Self::copy_dir_recursive(&source, &target)?;
        }
        Ok(())
    }

    pub(crate) fn copy_dir_recursive(source: &Path, target: &Path) -> Result<(), String> {
        let entries = fs::read_dir(source).map_err(|e| e.to_string())?;
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let dest = target.join(name);
            if path.is_dir() {
                fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
                Self::copy_dir_recursive(&path, &dest)?;
            } else if !dest.exists() {
                let _ = fs::copy(&path, &dest);
            }
        }
        Ok(())
    }

    pub(crate) fn paths_equal(a: &Path, b: &Path) -> bool {
        #[cfg(windows)]
        {
            let left = Self::normalize_windows_path(a)
                .to_string_lossy()
                .to_ascii_lowercase();
            let right = Self::normalize_windows_path(b)
                .to_string_lossy()
                .to_ascii_lowercase();
            return left == right;
        }
        #[cfg(not(windows))]
        {
            return a == b;
        }
    }

    pub(crate) fn load_project(&mut self) -> Result<(), String> {
        let path = self.project_path.clone();
        self.load_project_from_folder(Path::new(&path))
    }

    pub(crate) fn load_project_from_folder(&mut self, folder: &Path) -> Result<(), String> {
        self.prepare_for_project_change();
        let manifest_path = folder.join("project.json");
        let data = fs::read_to_string(&manifest_path).map_err(|e| e.to_string())?;
        let state: ProjectState = serde_json::from_str(&data).map_err(|e| e.to_string())?;
        let project_root = Self::normalize_windows_path(folder);
        self.project_name = state.name;
        self.metadata_artist = state.artist;
        self.metadata_title = state.title;
        self.metadata_album = state.album;
        self.metadata_genre = state.genre;
        self.metadata_year = state.year;
        self.metadata_comment = state.comment;
        self.project_key = state.project_key;
        self.project_key_minor = state.project_key_minor;
        self.tempo_bpm = state.tempo_bpm;
        self.tracks = state.tracks;
        self.node_routes = Self::sanitize_node_routes(state.node_routes, &self.tracks);
        self.performance_clip_settings =
            Self::sanitize_performance_clip_settings(state.performance_clip_settings, &self.tracks);
        self.performance_launch_quantize_beats = state.performance_launch_quantize_beats.max(0.0);
        self.performance_selected_clip = None;
        if self.node_routes.is_empty() {
            self.node_routes = Self::default_node_routes(&self.tracks);
        }
        self.sync_node_routes();
        for track in &mut self.tracks {
            if let Some(treesynth) = track.treesynth.as_mut() {
                if track
                    .instrument_path
                    .as_deref()
                    .map(Self::is_treesynth_path)
                    .unwrap_or(false)
                    == false
                {
                    track.instrument_path = Some("native:treesynth".to_string());
                }
                Self::resolve_treesynth_paths_from_project_root(treesynth, &project_root);
            }
        }
        if let Ok(mut master) = self.master_settings.lock() {
            *master = state.master_settings.clone();
        }
        if let Ok(mut comp_state) = self.master_comp_state.lock() {
            *comp_state = MasterCompState::default();
        }
        self.selected_clip = None;
        self.selected_clips.clear();
        self.piano_selected.clear();
        self.piano_drag = None;
        self.clip_drag = None;
        self.arranger_draw = None;
        self.arranger_select_start = None;
        self.project_path = folder.to_string_lossy().to_string();
        self.load_midi_from_folder(folder)?;
        self.migrate_track_notes_to_clips();
        let (missing_instruments, missing_effects) = self.clear_missing_plugin_references();
        self.sync_track_audio_states();
        self.log_all_fm_ratio_params("after_load");
        self.selected_track = if self.tracks.is_empty() { None } else { Some(0) };
        if self.project_name.trim().is_empty() {
            if let Some(name) = self.project_name_from_path() {
                self.project_name = name;
            }
        }
        self.register_recent_project_path(folder);
        if missing_instruments == 0 && missing_effects == 0 {
            self.clear_dirty();
            self.status = format!("Loaded {}", self.project_path);
        } else {
            self.mark_dirty();
            self.status = format!(
                "Loaded {} | cleared {} missing instrument(s), {} missing effect(s)",
                self.project_path,
                missing_instruments,
                missing_effects
            );
        }
        Ok(())
    }

    pub(crate) fn resolve_treesynth_paths_from_project_root(state: &mut TreeSynthState, project_root: &Path) {
        if let Some(folder) = state.folder.as_deref() {
            let path = Path::new(folder);
            if path.is_relative() {
                state.folder = Some(
                    project_root
                        .join(path)
                        .to_string_lossy()
                        .to_string(),
                );
            }
        }
        for sample in &mut state.samples {
            let path = Path::new(&sample.path);
            if path.is_relative() {
                sample.path = project_root.join(path).to_string_lossy().to_string();
            }
        }
    }

    pub(crate) fn sanitize_node_routes(routes: Vec<NodeRouteLink>, tracks: &[Track]) -> Vec<NodeRouteLink> {
        if tracks.is_empty() {
            return Vec::new();
        }
        routes
            .into_iter()
            .filter_map(|mut route| {
                if route.from_track >= tracks.len() || route.to_track >= tracks.len() {
                    return None;
                }
                route.source_output_pair = route.source_output_pair.min(7);
                if let Some(fx_index) = route.to_fx {
                    if fx_index >= tracks[route.to_track].effect_paths.len() {
                        route.to_fx = None;
                    }
                }
                Some(route)
            })
            .collect()
    }

    pub(crate) fn sanitize_performance_clip_settings(
        settings: HashMap<usize, PerformanceClipSettings>,
        tracks: &[Track],
    ) -> HashMap<usize, PerformanceClipSettings> {
        let valid_ids: HashSet<usize> = tracks
            .iter()
            .flat_map(|t| t.clips.iter().map(|c| c.id))
            .collect();
        settings
            .into_iter()
            .filter(|(clip_id, _)| valid_ids.contains(clip_id))
            .collect()
    }

    pub(crate) fn default_node_routes(tracks: &[Track]) -> Vec<NodeRouteLink> {
        let mut routes = Vec::new();
        for (track_index, track) in tracks.iter().enumerate() {
            if track.effect_paths.is_empty() {
                routes.push(NodeRouteLink {
                    from_track: track_index,
                    source_output_pair: 0,
                    to_track: track_index,
                    to_fx: None,
                    kind: NodeRouteKind::AudioSend,
                    enabled: true,
                    sidechain_amount: default_sidechain_amount(),
                    sidechain_attack_ms: default_sidechain_attack_ms(),
                    sidechain_release_ms: default_sidechain_release_ms(),
                    sidechain_threshold_db: default_sidechain_threshold_db(),
                });
            } else {
                for fx_index in 0..track.effect_paths.len() {
                    routes.push(NodeRouteLink {
                        from_track: track_index,
                        source_output_pair: 0,
                        to_track: track_index,
                        to_fx: Some(fx_index),
                        kind: NodeRouteKind::AudioSend,
                        enabled: true,
                        sidechain_amount: default_sidechain_amount(),
                        sidechain_attack_ms: default_sidechain_attack_ms(),
                        sidechain_release_ms: default_sidechain_release_ms(),
                        sidechain_threshold_db: default_sidechain_threshold_db(),
                    });
                }
            }
        }
        routes
    }

    pub(crate) fn open_project_dialog(&mut self) -> Result<(), String> {
        let folder = rfd::FileDialog::new().pick_folder();
        if let Some(folder) = folder {
            return self.load_project_from_folder(&folder);
        }
        Ok(())
    }

    pub(crate) fn save_project_dialog(&mut self) -> Result<(), String> {
        let folder = rfd::FileDialog::new().pick_folder();
        if let Some(folder) = folder {
            return self.save_project_to_folder(&folder);
        }
        Ok(())
    }

    pub(crate) fn save_project_new_version(&mut self) -> Result<(), String> {
        let current = self.project_path.trim();
        if current.is_empty() {
            return Err("No project path to version".to_string());
        }
        let current_path = Path::new(current);
        let parent = current_path
            .parent()
            .ok_or_else(|| "Project folder has no parent".to_string())?;
        let name = current_path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| "Project folder name unavailable".to_string())?;
        let (base, version) = Self::split_version_suffix(name);
        let base = base.trim_end();
        if base.is_empty() {
            return Err("Project name unavailable".to_string());
        }
        let mut next = version.unwrap_or(1).saturating_add(1);
        loop {
            let candidate_name = format!("{} v{}", base, next);
            let candidate = parent.join(&candidate_name);
            if !candidate.exists() {
                self.save_project_to_folder(&candidate)?;
                self.status = format!("Saved new version {}", self.project_path);
                return Ok(());
            }
            next = next.saturating_add(1);
            if next > 9999 {
                return Err("No available version number".to_string());
            }
        }
    }

    pub(crate) fn open_project_from_path(&mut self, path: &str) -> Result<(), String> {
        let folder = Path::new(path);
        if !folder.exists() {
            self.settings.recent_projects.retain(|p| !p.eq_ignore_ascii_case(path));
            let _ = self.save_settings();
            return Err("Project folder not found".to_string());
        }
        self.load_project_from_folder(folder)
    }

    pub(crate) fn load_template_from_path(&mut self, path: &str) -> Result<(), String> {
        let path = Path::new(path);
        let folder = if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent()
                .ok_or_else(|| "Template folder unavailable".to_string())?
                .to_path_buf()
        };
        let manifest_path = folder.join("project.json");
        if !manifest_path.exists() {
            return Err("Template project.json missing".to_string());
        }
        self.prepare_for_project_change();
        let data = fs::read_to_string(&manifest_path).map_err(|e| e.to_string())?;
        let state: ProjectState = serde_json::from_str(&data).map_err(|e| e.to_string())?;
        self.project_name = state.name;
        self.metadata_artist = state.artist;
        self.metadata_title = state.title;
        self.metadata_album = state.album;
        self.metadata_genre = state.genre;
        self.metadata_year = state.year;
        self.metadata_comment = state.comment;
        self.project_key = state.project_key;
        self.project_key_minor = state.project_key_minor;
        self.tempo_bpm = state.tempo_bpm;
        self.tracks = state.tracks;
        self.node_routes = Self::sanitize_node_routes(state.node_routes, &self.tracks);
        self.performance_clip_settings =
            Self::sanitize_performance_clip_settings(state.performance_clip_settings, &self.tracks);
        self.performance_launch_quantize_beats = state.performance_launch_quantize_beats.max(0.0);
        self.performance_selected_clip = None;
        if self.node_routes.is_empty() {
            self.node_routes = Self::default_node_routes(&self.tracks);
        }
        self.sync_node_routes();
        if let Ok(mut master) = self.master_settings.lock() {
            *master = state.master_settings.clone();
        }
        if let Ok(mut comp_state) = self.master_comp_state.lock() {
            *comp_state = MasterCompState::default();
        }
        self.selected_clip = None;
        self.selected_clips.clear();
        self.piano_selected.clear();
        self.piano_drag = None;
        self.clip_drag = None;
        self.arranger_draw = None;
        self.arranger_select_start = None;
        self.project_path.clear();
        self.load_midi_from_folder(&folder)?;
        self.migrate_track_notes_to_clips();
        self.sync_track_audio_states();
        self.selected_track = if self.tracks.is_empty() { None } else { Some(0) };
        if self.project_name.trim().is_empty() {
            if let Some(name) = folder.file_name().and_then(|s| s.to_str()) {
                self.project_name = name.replace('_', " ");
            }
        }
        self.clear_dirty();
        self.status = "Template loaded".to_string();
        Ok(())
    }

    pub(crate) fn list_templates(&self) -> Vec<(String, String)> {
        let Some(root) = self.templates_dir() else {
            return Vec::new();
        };
        let mut templates = Vec::new();
        if let Ok(entries) = fs::read_dir(&root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                if !path.join("project.json").exists() {
                    continue;
                }
                let name = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Template")
                    .replace('_', " ");
                let normalized = Self::normalize_windows_path(&path);
                templates.push((name, normalized.to_string_lossy().to_string()));
            }
        }
        templates.sort_by(|a, b| a.0.cmp(&b.0));
        templates
    }

    pub(crate) fn templates_dir(&self) -> Option<PathBuf> {
        if let Ok(dir) = std::env::current_dir() {
            let candidate = dir.join("assets").join("templates");
            if candidate.exists() {
                return Some(candidate);
            }
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                let candidate = parent.join("assets").join("templates");
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
        None
    }

    pub(crate) fn register_recent_project_path(&mut self, folder: &Path) {
        let normalized = Self::normalize_windows_path(folder);
        let path = normalized.to_string_lossy().to_string();
        if path.trim().is_empty() {
            return;
        }
        self.settings
            .recent_projects
            .retain(|p| !p.eq_ignore_ascii_case(&path));
        self.settings.recent_projects.insert(0, path);
        if self.settings.recent_projects.len() > 10 {
            self.settings.recent_projects.truncate(10);
        }
        let _ = self.save_settings();
    }

    pub(crate) fn split_version_suffix(name: &str) -> (String, Option<u32>) {
        if let Some((base, suffix)) = name.rsplit_once(" v") {
            if !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit()) {
                if let Ok(num) = suffix.parse::<u32>() {
                    return (base.to_string(), Some(num));
                }
            }
        }
        (name.to_string(), None)
    }

    pub(crate) fn begin_rename_project(&mut self) {
        self.project_name_buffer = self.project_name.clone();
        self.show_rename_project = true;
    }

    pub(crate) fn apply_rename_project(&mut self) {
        let name = self.project_name_buffer.trim();
        if !name.is_empty() {
            self.project_name = name.to_string();
            self.mark_dirty();
            self.status = "Project renamed".to_string();
        }
    }

    pub(crate) fn project_name_from_path(&self) -> Option<String> {
        let path = Path::new(&self.project_path);
        path.file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.replace('_', " "))
    }

    pub(crate) fn save_project_or_prompt(&mut self) -> Result<(), String> {
        if self.project_path.trim().is_empty() {
            if let Some(folder) = self.default_project_dir() {
                return self.save_project_to_folder(&folder);
            }
            return Err("Default project folder unavailable".to_string());
        }
        self.save_project()
    }

    pub(crate) fn sanitize_folder_name(name: &str) -> String {
        let mut cleaned = String::new();
        for ch in name.chars() {
            let safe = match ch {
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
                _ => ch,
            };
            cleaned.push(safe);
        }
        let trimmed = cleaned.trim();
        if trimmed.is_empty() {
            "LingStationProject".to_string()
        } else {
            trimmed.to_string()
        }
    }

    pub(crate) fn safe_join_within_base(base: &Path, child: &str) -> Result<PathBuf, String> {
        // Deterministic path containment check (no filesystem access): prevents `..` traversal
        // from sneaking into output paths.
        let candidate = base.join(child);

        let mut base_it = base.components();
        let base_prefix = base_it
            .next()
            .ok_or_else(|| "Base path is empty".to_string())?;

        let mut base_stack: Vec<std::ffi::OsString> = Vec::new();
        for c in base_it {
            match c {
                std::path::Component::Normal(s) => base_stack.push(s.to_os_string()),
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    // Normalizing the base path; if we would pop past root, it's unsafe.
                    if base_stack.pop().is_none() {
                        return Err("Output path escapes render base directory".to_string());
                    }
                }
                _ => {}
            }
        }

        let mut cand_it = candidate.components();
        let cand_prefix = cand_it
            .next()
            .ok_or_else(|| "Candidate path is empty".to_string())?;
        if cand_prefix != base_prefix {
            return Err("Output path escapes render base directory".to_string());
        }

        let mut cand_stack: Vec<std::ffi::OsString> = Vec::new();
        for c in cand_it {
            match c {
                std::path::Component::Normal(s) => cand_stack.push(s.to_os_string()),
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    if cand_stack.pop().is_none() {
                        return Err("Output path escapes render base directory".to_string());
                    }
                }
                _ => {}
            }
        }

        if cand_stack.len() < base_stack.len() || cand_stack[..base_stack.len()] != base_stack[..] {
            return Err("Output path escapes render base directory".to_string());
        }
        Ok(candidate)
    }

    pub(crate) fn render_base_name(&self) -> String {
        let artist = self.metadata_artist.trim();
        let title = self.metadata_title.trim();
        let base = if !artist.is_empty() && !title.is_empty() {
            format!("{} - {}", artist, title)
        } else if !title.is_empty() {
            title.to_string()
        } else if !artist.is_empty() {
            artist.to_string()
        } else if !self.project_name.trim().is_empty() {
            self.project_name.clone()
        } else {
            "render".to_string()
        };
        Self::sanitize_folder_name(&base)
    }

    pub(crate) fn render_license_comment(&self) -> Option<String> {
        let status = self.license_status.trim();
        let registered = status.starts_with("Registered")
            || !self.settings.registered_to.trim().is_empty();
        if registered {
            let lower = status.to_ascii_lowercase();
            if lower.contains("edu") || lower.contains("education") {
                Some("Made with an Educational License".to_string())
            } else {
                None
            }
        } else {
            Some("Made with LingStation[Unlicensed]".to_string())
        }
    }

    pub(crate) fn note_icon_source(value: f32) -> egui::ImageSource<'static> {
        if (value - 1.0 / 32.0).abs() < f32::EPSILON {
            egui::include_image!("../../../assets/icons/note-thirtysecond.svg")
        } else if (value - 1.0 / 16.0).abs() < f32::EPSILON {
            egui::include_image!("../../../assets/icons/note-sixteenth.svg")
        } else if (value - 1.0 / 8.0).abs() < f32::EPSILON {
            egui::include_image!("../../../assets/icons/note-eighth.svg")
        } else if (value - 1.0 / 4.0).abs() < f32::EPSILON {
            egui::include_image!("../../../assets/icons/note-quarter.svg")
        } else if (value - 1.0 / 2.0).abs() < f32::EPSILON {
            egui::include_image!("../../../assets/icons/note-half.svg")
        } else {
            egui::include_image!("../../../assets/icons/note-whole.svg")
        }
    }

    pub(crate) fn ensure_project_folder(&mut self) -> Result<PathBuf, String> {
        if self.project_path.trim().is_empty() {
            let folder = self
                .default_project_dir()
                .ok_or_else(|| "Default project folder unavailable".to_string())?;
            fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
            self.project_path = folder.to_string_lossy().to_string();
            return Ok(folder);
        }
        let folder = PathBuf::from(self.project_path.trim());
        fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        Ok(folder)
    }

    pub(crate) fn add_audio_clip_from_path(
        &mut self,
        track_index: usize,
        start_beats: f32,
        source: &Path,
    ) -> Result<(), String> {
        if track_index >= self.tracks.len() {
            return Err("Invalid track for dropped clip".to_string());
        }
        let project_folder = self.ensure_project_folder()?;
        let audio_dir = project_folder.join("audio");
        fs::create_dir_all(&audio_dir).map_err(|e| e.to_string())?;

        let _file_name = source
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| "Invalid file name".to_string())?;
        let stem = source
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Audio");
        let ext = source.extension().and_then(|s| s.to_str()).unwrap_or("wav");
        let safe_stem = Self::sanitize_folder_name(stem);
        let mut target = audio_dir.join(format!("{}.{}", safe_stem, ext));
        let mut counter = 1;
        while target.exists() {
            target = audio_dir.join(format!("{}_{}.{}", safe_stem, counter, ext));
            counter += 1;
        }
        fs::copy(source, &target).map_err(|e| e.to_string())?;

        let source_beats = Self::audio_length_beats(&target, self.tempo_bpm);
        let clip_len = source_beats.unwrap_or(4.0).max(0.25);

        let clip_id = self.next_clip_id();
        if let Some(track) = self.tracks.get_mut(track_index) {
            let clip = Clip {
                id: clip_id,
                track: track_index,
                start_beats: start_beats.max(0.0),
                length_beats: clip_len,
                is_midi: false,
                midi_notes: Vec::new(),
                midi_source_beats: None,
                link_id: None,
                name: safe_stem.clone(),
                audio_path: Some(format!("audio/{}", target.file_name().unwrap().to_string_lossy())),
                audio_source_beats: source_beats,
                audio_offset_beats: 0.0,
                audio_gain: 1.0,
                audio_pitch_semitones: 0.0,
                audio_stretch_mode: AudioStretchMode::Stretch,
                audio_time_mul: 1.0,
                audio_key: None,
                audio_key_minor: false,
                audio_key_source: None,
                audio_bpm: None,
                audio_fine_pitch_cents: 0.0,
                audio_formant_scale: 1.0,
            };
            track.clips.push(clip);
        }
        self.enqueue_audio_analysis(clip_id, target);
        self.selected_track = Some(track_index);
        self.selected_clip = Some(clip_id);
        Ok(())
    }

    pub(crate) fn normalize_audio_clip_with_path(clip: &mut Clip, path: &Path) -> Result<(), String> {
        let (samples, _channels, _sample_rate) =
            Self::decode_audio_samples(path).ok_or_else(|| "Unsupported audio format".to_string())?;
        let mut peak = 0.0f32;
        for sample in samples {
            peak = peak.max(sample.abs());
        }
        if peak <= 0.0 {
            return Err("Clip is silent".to_string());
        }
        let target = db_to_gain(-1.0);
        clip.audio_gain = (target / peak).clamp(0.0, 2.0);
        Ok(())
    }

    pub(crate) fn audio_length_beats(path: &Path, tempo_bpm: f32) -> Option<f32> {
        let seconds = Self::audio_length_seconds(path)?;
        let beats = seconds * tempo_bpm.max(1.0) / 60.0;
        Some(beats.max(0.0))
    }

    pub(crate) fn wav_length_beats(path: &Path, tempo_bpm: f32) -> Option<f32> {
        let reader = hound::WavReader::open(path).ok()?;
        let spec = reader.spec();
        let samples = reader.duration() as f32;
        let channels = spec.channels.max(1) as f32;
        if spec.sample_rate == 0 {
            return None;
        }
        let frames = samples / channels;
        let seconds = frames / spec.sample_rate as f32;
        let beats = seconds * tempo_bpm.max(1.0) / 60.0;
        Some(beats.max(0.0))
    }

    pub(crate) fn audio_length_seconds(path: &Path) -> Option<f32> {
        if path
            .extension()
            .and_then(|s| s.to_str())
            .map(|e| e.eq_ignore_ascii_case("wav"))
            .unwrap_or(false)
        {
            return Self::wav_length_seconds(path);
        }
        let (samples, channels, sample_rate) = Self::decode_audio_samples(path)?;
        if sample_rate == 0 || channels == 0 {
            return None;
        }
        let frames = samples.len() as f32 / channels as f32;
        Some((frames / sample_rate as f32).max(0.0))
    }

    pub(crate) fn decode_audio_samples(path: &Path) -> Option<(Vec<f32>, usize, u32)> {
        let is_wav = path
            .extension()
            .and_then(|s| s.to_str())
            .map(|e| e.eq_ignore_ascii_case("wav"))
            .unwrap_or(false);
        if is_wav {
            let mut reader = hound::WavReader::open(path).ok()?;
            let spec = reader.spec();
            let channels = spec.channels.max(1) as usize;
            let mut samples = Vec::new();
            match spec.sample_format {
                hound::SampleFormat::Float => {
                    for sample in reader.samples::<f32>() {
                        samples.push(sample.ok()?);
                    }
                }
                hound::SampleFormat::Int => {
                    if spec.bits_per_sample <= 16 {
                        let max = i16::MAX as f32;
                        for sample in reader.samples::<i16>() {
                            samples.push(sample.ok()? as f32 / max);
                        }
                    } else {
                        let max = i32::MAX as f32;
                        for sample in reader.samples::<i32>() {
                            samples.push(sample.ok()? as f32 / max);
                        }
                    }
                }
            }
            return Some((samples, channels, spec.sample_rate));
        }

        let file = std::fs::File::open(path).ok()?;
        let reader = BufReader::new(file);
        let decoder = Decoder::new(reader).ok()?;
        let channels = decoder.channels().max(1) as usize;
        let sample_rate = decoder.sample_rate();
        let samples: Vec<f32> = decoder.convert_samples::<f32>().collect();
        Some((samples, channels, sample_rate))
    }

    pub(crate) fn wav_length_seconds(path: &Path) -> Option<f32> {
        if path
            .extension()
            .and_then(|s| s.to_str())
            .map(|e| !e.eq_ignore_ascii_case("wav"))
            .unwrap_or(true)
        {
            return None;
        }
        let reader = hound::WavReader::open(path).ok()?;
        let spec = reader.spec();
        let samples = reader.duration() as f32;
        let channels = spec.channels.max(1) as f32;
        if spec.sample_rate == 0 {
            return None;
        }
        let frames = samples / channels;
        let seconds = frames / spec.sample_rate as f32;
        Some(seconds.max(0.0))
    }

    pub(crate) fn load_midi_from_folder(&mut self, folder: &Path) -> Result<(), String> {
        let midi_dir = folder.join("midi");
        if !midi_dir.exists() {
            return Ok(());
        }
        let mut entries: Vec<PathBuf> = fs::read_dir(&midi_dir)
            .map_err(|e| e.to_string())?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .and_then(|s| s.to_str())
                    .map(|ext| ext.eq_ignore_ascii_case("mid") || ext.eq_ignore_ascii_case("midi"))
                    .unwrap_or(false)
            })
            .collect();
        entries.sort();

        let mut clip_notes_by_id: HashMap<(usize, usize), Vec<PianoRollNote>> = HashMap::new();
        let mut clip_notes_by_name: HashMap<(usize, String), Vec<PianoRollNote>> = HashMap::new();
        let mut track_notes: HashMap<usize, Vec<PianoRollNote>> = HashMap::new();

        for path in entries {
            let file_name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            let Some(track_index) = Self::track_index_from_filename(file_name) else {
                continue;
            };
            if track_index >= self.tracks.len() {
                continue;
            }
            let path_str = path.to_string_lossy().to_string();
            let channels = import_midi_channels(&path_str)?;
            let notes = channels
                .into_iter()
                .find(|c| !c.notes.is_empty())
                .map(|c| c.notes)
                .unwrap_or_default();
            if notes.is_empty() {
                continue;
            }
            if let Some(clip_id) = Self::clip_id_from_filename(file_name) {
                clip_notes_by_id.insert((track_index, clip_id), notes);
            } else {
                let clip_name = Path::new(file_name)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("MIDI")
                    .replace('_', " ");
                clip_notes_by_name.insert((track_index, clip_name.clone()), notes.clone());
                track_notes.entry(track_index).or_insert(notes);
            }
        }

        let mut next_clip_id = self.next_clip_id();
        let mut rebuild_indices = Vec::new();
        for (track_index, track) in self.tracks.iter_mut().enumerate() {
            for clip in track.clips.iter_mut() {
                clip.midi_notes.clear();
            }
            let mut any_clip_notes = false;
            if track.clips.is_empty() {
                if let Some(notes) = track_notes.remove(&track_index) {
                    let max_end: f32 = notes
                        .iter()
                        .map(|n| n.start_beats + n.length_beats)
                        .fold(1.0, |a, b| a.max(b));
                    track.clips.push(Clip {
                        id: next_clip_id,
                        track: track_index,
                        start_beats: 0.0,
                        length_beats: max_end.max(1.0),
                        is_midi: true,
                        midi_notes: notes.clone(),
                        midi_source_beats: Some(max_end.max(1.0)),
                        link_id: None,
                        name: "MIDI".to_string(),
                        audio_path: None,
                        audio_source_beats: None,
                        audio_offset_beats: 0.0,
                        audio_gain: 1.0,
                        audio_pitch_semitones: 0.0,
                        audio_stretch_mode: AudioStretchMode::Stretch,
                        audio_time_mul: 1.0,
                        audio_key: None,
                        audio_key_minor: false,
                        audio_key_source: None,
                        audio_bpm: None,
                        audio_fine_pitch_cents: 0.0,
                        audio_formant_scale: 1.0,
                    });
                    next_clip_id = next_clip_id.saturating_add(1);
                    any_clip_notes = true;
                }
            }
            for clip in track.clips.iter_mut() {
                if !clip.is_midi {
                    continue;
                }
                let mut clip_notes = clip_notes_by_id
                    .remove(&(track_index, clip.id))
                    .or_else(|| {
                        if clip.name.trim().is_empty() {
                            None
                        } else {
                            clip_notes_by_name.get(&(track_index, clip.name.clone())).cloned()
                        }
                    });
                if let Some(mut notes) = clip_notes.take() {
                    for note in &mut notes {
                        note.start_beats = (note.start_beats + clip.start_beats).max(0.0);
                    }
                    clip.midi_notes = notes;
                    if clip.midi_source_beats.is_none() {
                        clip.midi_source_beats = Some(clip.length_beats.max(0.25));
                    }
                    any_clip_notes = true;
                }
            }
            if !any_clip_notes {
                if let Some(notes) = track_notes.remove(&track_index) {
                    if track.clips.iter().any(|c| c.is_midi) {
                        for clip in track.clips.iter_mut() {
                            if !clip.is_midi {
                                continue;
                            }
                            let mut shifted_notes = Vec::new();
                            for note in &notes {
                                let mut shifted = note.clone();
                                shifted.start_beats = (shifted.start_beats + clip.start_beats).max(0.0);
                                shifted_notes.push(shifted);
                            }
                            clip.midi_notes = shifted_notes;
                            if clip.midi_source_beats.is_none() {
                                clip.midi_source_beats = Some(clip.length_beats.max(0.25));
                            }
                        }
                    } else {
                        let max_end: f32 = notes
                            .iter()
                            .map(|n| n.start_beats + n.length_beats)
                            .fold(1.0, |a, b| a.max(b));
                        track.clips.push(Clip {
                            id: next_clip_id,
                            track: track_index,
                            start_beats: 0.0,
                            length_beats: max_end.max(1.0),
                            is_midi: true,
                            midi_notes: notes.clone(),
                            midi_source_beats: Some(max_end.max(1.0)),
                            link_id: None,
                            name: "MIDI".to_string(),
                            audio_path: None,
                            audio_source_beats: None,
                            audio_offset_beats: 0.0,
                            audio_gain: 1.0,
                            audio_pitch_semitones: 0.0,
                            audio_stretch_mode: AudioStretchMode::Stretch,
                            audio_time_mul: 1.0,
                            audio_key: None,
                            audio_key_minor: false,
                            audio_key_source: None,
                            audio_bpm: None,
                            audio_fine_pitch_cents: 0.0,
                            audio_formant_scale: 1.0,
                        });
                        next_clip_id = next_clip_id.saturating_add(1);
                    }
                }
            }
            rebuild_indices.push(track_index);
        }

        for track_index in rebuild_indices {
            self.rebuild_track_midi_notes(track_index);
        }

        Ok(())
    }

    pub(crate) fn migrate_track_notes_to_clips(&mut self) {
        let mut next_clip_id = self.next_clip_id();
        let mut rebuild_indices = Vec::new();
        for (track_index, track) in self.tracks.iter_mut().enumerate() {
            if track.midi_notes.is_empty() {
                continue;
            }
            let has_clip_notes = track
                .clips
                .iter()
                .any(|clip| clip.is_midi && !clip.midi_notes.is_empty());
            if has_clip_notes {
                continue;
            }
            if track.clips.is_empty() {
                let max_end: f32 = track
                    .midi_notes
                    .iter()
                    .map(|n| n.start_beats + n.length_beats)
                    .fold(1.0, |a, b| a.max(b));
                track.clips.push(Clip {
                    id: next_clip_id,
                    track: track_index,
                    start_beats: 0.0,
                    length_beats: max_end.max(1.0),
                    is_midi: true,
                    midi_notes: track.midi_notes.clone(),
                    midi_source_beats: Some(max_end.max(1.0)),
                    link_id: None,
                    name: "MIDI".to_string(),
                    audio_path: None,
                    audio_source_beats: None,
                    audio_offset_beats: 0.0,
                    audio_gain: 1.0,
                    audio_pitch_semitones: 0.0,
                    audio_stretch_mode: AudioStretchMode::Stretch,
                    audio_time_mul: 1.0,
                    audio_key: None,
                    audio_key_minor: false,
                    audio_key_source: None,
                    audio_bpm: None,
                    audio_fine_pitch_cents: 0.0,
                    audio_formant_scale: 1.0,
                });
                next_clip_id = next_clip_id.saturating_add(1);
            } else {
                for clip in track.clips.iter_mut().filter(|c| c.is_midi) {
                    let clip_start = clip.start_beats;
                    let clip_end = clip.start_beats + clip.length_beats;
                    let mut notes = Vec::new();
                    for note in &track.midi_notes {
                        let note_end = note.start_beats + note.length_beats;
                        if note.start_beats < clip_end && note_end > clip_start {
                            notes.push(note.clone());
                        }
                    }
                    if !notes.is_empty() {
                        clip.midi_notes = notes;
                        if clip.midi_source_beats.is_none() {
                            clip.midi_source_beats = Some(clip.length_beats.max(0.25));
                        }
                    }
                }
            }
            rebuild_indices.push(track_index);
        }
        for track_index in rebuild_indices {
            self.rebuild_track_midi_notes(track_index);
        }
    }

    pub(crate) fn track_index_from_filename(file_name: &str) -> Option<usize> {
        let mut digits = String::new();
        for ch in file_name.chars() {
            if ch.is_ascii_digit() {
                digits.push(ch);
            } else {
                break;
            }
        }
        if digits.is_empty() {
            return None;
        }
        let index: usize = digits.parse().ok()?;
        index.checked_sub(1)
    }

    pub(crate) fn clip_id_from_filename(file_name: &str) -> Option<usize> {
        let stem = Path::new(file_name).file_stem()?.to_str()?;
        let marker = "_clip";
        let pos = stem.rfind(marker)?;
        let digits = &stem[pos + marker.len()..];
        if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        digits.parse().ok()
    }

    pub(crate) fn import_midi_dialog(&mut self) -> Result<(), String> {
        let path = rfd::FileDialog::new()
            .add_filter("MIDI", &["mid", "midi"])
            .pick_file();
        if let Some(path) = path {
            let path_str = path.to_string_lossy().to_string();
            self.begin_midi_import(path_str)?;
        }
        Ok(())
    }

    pub(crate) fn begin_midi_import(&mut self, path_str: String) -> Result<(), String> {
        self.begin_midi_import_with_mode(path_str, MidiImportMode::ReplaceProject)
    }

    pub(crate) fn begin_midi_import_with_mode(
        &mut self,
        path_str: String,
        mode: MidiImportMode,
    ) -> Result<(), String> {
        let tracks = import_midi_tracks(&path_str)?;
        if tracks.is_empty() {
            self.status = "No MIDI tracks found".to_string();
            return Ok(());
        }
        let enabled = vec![true; tracks.len()];
        let apply_program = tracks.iter().map(|t| t.program.is_some()).collect();
        self.midi_import_state = Some(MidiImportState {
            path: path_str,
            tracks,
            enabled,
            apply_program,
            instrument_plugin: "FishSynth".to_string(),
            percussion_plugin: "Catsynth".to_string(),
            import_portamento: true,
            mode,
        });
        self.show_midi_import = true;
        Ok(())
    }

    pub(crate) fn apply_midi_import(&mut self) -> Result<(), String> {
        let Some(state) = self.midi_import_state.take() else {
            return Ok(());
        };
        let was_running = self.audio_running;
        let append_mode = matches!(state.mode, MidiImportMode::AppendTracks { .. });
        if append_mode {
            if was_running {
                self.stop_audio_and_midi();
            }
        } else {
            self.prepare_for_project_change();
        }

        let mut next_id = self.next_clip_id();
        let mut tracks = Vec::new();
        let mut missing_plugins: HashSet<String> = HashSet::new();
        let insert_start = match state.mode {
            MidiImportMode::AppendTracks { start_beats } => start_beats.max(0.0),
            MidiImportMode::ReplaceProject => 0.0,
        };
        for (index, track_data) in state.tracks.iter().enumerate() {
            if !state.enabled.get(index).copied().unwrap_or(true) {
                continue;
            }
            if track_data.notes.is_empty() {
                continue;
            }
            let is_drums = track_data.has_drums;
            let plugin_name = if is_drums {
                state.percussion_plugin.as_str()
            } else {
                state.instrument_plugin.as_str()
            };
            let instrument_path = if plugin_name == "None" {
                None
            } else {
                let path = self.find_vst3_plugin_by_name(plugin_name);
                if path.is_none() {
                    missing_plugins.insert(plugin_name.to_string());
                }
                path
            };
            let params = if instrument_path.is_some() {
                default_instrument_params()
            } else {
                default_midi_params()
            };
            let max_end: f32 = track_data
                .notes
                .iter()
                .map(|n| n.start_beats + n.length_beats)
                .fold(1.0f32, |a, b| a.max(b));
            let mut notes = track_data.notes.clone();
            if insert_start > 0.0 {
                for note in &mut notes {
                    note.start_beats += insert_start;
                }
            }
            let clip = Clip {
                id: next_id,
                track: if append_mode {
                    self.tracks.len() + tracks.len()
                } else {
                    tracks.len()
                },
                start_beats: insert_start,
                length_beats: max_end.max(1.0),
                is_midi: true,
                midi_notes: notes,
                midi_source_beats: Some(max_end.max(1.0)),
                link_id: None,
                name: format!("Track {}", track_data.track_index + 1),
                audio_path: None,
                audio_source_beats: None,
                audio_offset_beats: 0.0,
                audio_gain: 1.0,
                audio_pitch_semitones: 0.0,
                audio_stretch_mode: AudioStretchMode::Stretch,
                audio_time_mul: 1.0,
                audio_key: None,
                audio_key_minor: false,
                audio_key_source: None,
                audio_bpm: None,
                audio_fine_pitch_cents: 0.0,
                audio_formant_scale: 1.0,
            };
            next_id += 1;
            let track_name = match track_data.program {
                Some(program) if is_drums => gm_drum_kit_name(program)
                    .unwrap_or("Drum Kit")
                    .to_string(),
                Some(program) => gm_program_name(program).to_string(),
                None => format!("Track {}", track_data.track_index + 1),
            };
            let mut midi_cc_lanes = Vec::new();
            if state.import_portamento {
                let mut points = Vec::new();
                for event in &track_data.cc_events {
                    if event.cc == 65 {
                        points.push(AutomationPoint {
                            beat: event.beat,
                            value: event.value,
                        });
                    }
                }
                if !points.is_empty() {
                    points.sort_by(|a, b| a.beat.partial_cmp(&b.beat).unwrap_or(std::cmp::Ordering::Equal));
                    midi_cc_lanes.push(MidiCcLane { cc: 65, points });
                }
            }
            let midi_program = if state.apply_program.get(index).copied().unwrap_or(true) {
                track_data.program
            } else {
                None
            };
            tracks.push(Track {
                name: track_name,
                clips: vec![clip],
                level: 0.8,
                muted: false,
                solo: false,
                midi_notes: Vec::new(),
                instrument_path,
                instrument_clap_id: None,
                effect_paths: Vec::new(),
                effect_clap_ids: Vec::new(),
                effect_bypass: Vec::new(),
                effect_params: Vec::new(),
                effect_param_ids: Vec::new(),
                effect_param_values: Vec::new(),
                params,
                param_ids: Vec::new(),
                param_values: Vec::new(),
                plugin_state_component: None,
                plugin_state_controller: None,
                automation_lanes: Vec::new(),
                automation_channels: Vec::new(),
                midi_cc_lanes,
                midi_program,
                treesynth: None,
            });
        }

        if tracks.is_empty() {
            self.status = "No MIDI tracks imported".to_string();
        } else {
            let start_index = if append_mode {
                let base = self.tracks.len();
                self.tracks.extend(tracks);
                base
            } else {
                self.tracks = tracks;
                0
            };
            self.selected_track = Some(start_index);
            self.selected_clip = self
                .tracks
                .get(start_index)
                .and_then(|t| t.clips.first())
                .map(|c| c.id);
            self.sync_track_audio_states();
            self.ensure_builtin_gm_presets();
            for index in 0..self.tracks.len() {
                let mut should_refresh = false;
                if let Some(track) = self.tracks.get(index) {
                    if track.instrument_path.is_some() && track.midi_program.is_some() {
                        should_refresh = true;
                    }
                }
                if should_refresh {
                    self.refresh_track_params(index);
                    self.apply_micesynth_program_from_midi(index);
                    if let Some(program) = self.tracks.get(index).and_then(|t| t.midi_program) {
                        let _ = self.load_preset_for_program(index, program);
                    }
                }
            }
            if missing_plugins.is_empty() {
                self.status = if append_mode {
                    "MIDI imported (appended)".to_string()
                } else {
                    "MIDI imported".to_string()
                };
            } else {
                let mut missing: Vec<String> = missing_plugins.into_iter().collect();
                missing.sort();
                let suffix = if append_mode { " (appended)" } else { "" };
                self.status = format!("MIDI imported{suffix} (missing: {})", missing.join(", "));
            }
        }
        self.import_path = state.path;
        self.show_midi_import = false;
        self.mark_dirty();
            self.pending_viewport_focus = true;
            self.pending_repaint_frames = 12;
        if was_running {
            if let Err(err) = self.start_audio_and_midi() {
                self.status = format!("Audio restart failed: {err}");
            }
        }
        Ok(())
    }

    pub(crate) fn export_midi_dialog(&mut self) -> Result<(), String> {
        let path = rfd::FileDialog::new()
            .add_filter("MIDI", &["mid", "midi"])
            .set_file_name("export.mid")
            .save_file();
        if let Some(path) = path {
            let path_str = path.to_string_lossy().to_string();
            let notes = self
                .selected_track
                .and_then(|index| self.tracks.get(index))
                .map(|track| track.midi_notes.as_slice())
                .unwrap_or(&[]);
            export_midi(&path_str, notes, 480)?;
            self.export_path = path_str;
            self.status = "MIDI exported".to_string();
        }
        Ok(())
    }



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
            path: path.to_string_lossy().to_string(),
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
            path: path.to_string_lossy().to_string(),
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

    pub(crate) fn default_render_dir(&self) -> Option<PathBuf> {
        if !self.project_path.trim().is_empty() {
            let path = PathBuf::from(self.project_path.trim());
            if path.exists() {
                return Some(path);
            }
        }
        if let Some(default_dir) = self.default_project_dir() {
            let _ = fs::create_dir_all(&default_dir);
            if default_dir.exists() {
                return Some(default_dir);
            }
        }
        if let Ok(home) = std::env::var("USERPROFILE") {
            let home_path = PathBuf::from(home);
            let music_path = home_path.join("Music");
            if !music_path.exists() {
                let _ = fs::create_dir_all(&music_path);
            }
            if music_path.exists() {
                return Some(music_path);
            }
            if home_path.exists() {
                return Some(home_path);
            }
        }
        None
    }

    pub(crate) fn default_project_dir(&self) -> Option<PathBuf> {
        let base = std::env::current_exe().ok().and_then(|p| {
            let dir = p.parent()?.to_path_buf();
            Some(dir)
        })?;
        let name = if self.project_name.trim().is_empty() {
            "LingStationProject"
        } else {
            self.project_name.trim()
        };
        let folder = Self::sanitize_folder_name(name);
        Some(base.join(folder))
    }

    pub(crate) fn default_settings_path() -> String {
        #[cfg(windows)]
        {
            if let Ok(appdata) = std::env::var("APPDATA") {
                let dir = PathBuf::from(appdata).join("LingStation");
                let _ = fs::create_dir_all(&dir);
                return dir.join("settings.ling.json").to_string_lossy().to_string();
            }
        }
        #[cfg(not(windows))]
        {
            if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
                let dir = PathBuf::from(xdg).join("LingStation");
                let _ = fs::create_dir_all(&dir);
                return dir.join("settings.ling.json").to_string_lossy().to_string();
            }
            if let Ok(home) = std::env::var("HOME") {
                let dir = PathBuf::from(home).join(".config").join("LingStation");
                let _ = fs::create_dir_all(&dir);
                return dir.join("settings.ling.json").to_string_lossy().to_string();
            }
        }
        "settings.ling.json".to_string()
    }

    pub(crate) fn ensure_synth_soundfont(&self) {
        let cwd = match std::env::current_dir() {
            Ok(dir) => dir,
            Err(_) => return,
        };
        let synths_root = cwd.join("synths");
        if !synths_root.exists() {
            return;
        }
        let candidates = [
            synths_root.join("FishSynth").join("Ling.sf2"),
            synths_root
                .join("FishSynth")
                .join("FishSynth.vst3")
                .join("Ling.sf2"),
            synths_root
                .join("FishSynth")
                .join("FishSynth.vst3")
                .join("Contents")
                .join("x86_64-win")
                .join("SF")
                .join("Ling.sf2"),
            synths_root
                .join("FishSynth")
                .join("FishSynth.vst3")
                .join("Contents")
                .join("x86_64-win")
                .join("Resources")
                .join("SF")
                .join("Ling.sf2"),
        ];
        let source = candidates.iter().find(|path| path.exists()).cloned();
        let Some(source) = source else {
            return;
        };
        let targets = [
            cwd.join("assets").join("Ling.sf2"),
            synths_root.join("Ling.sf2"),
            synths_root.join("MiceSynth").join("Ling.sf2"),
            synths_root
                .join("MiceSynth")
                .join("MiceSynth.vst3")
                .join("Contents")
                .join("x86_64-win")
                .join("SF")
                .join("Ling.sf2"),
            synths_root
                .join("MiceSynth")
                .join("MiceSynth.vst3")
                .join("Contents")
                .join("x86_64-win")
                .join("Resources")
                .join("SF")
                .join("Ling.sf2"),
        ];
        for target in targets {
            if target.exists() {
                continue;
            }
            if let Some(parent) = target.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::copy(&source, &target);
        }
    }

    pub(crate) fn normalize_windows_path(path: &Path) -> PathBuf {
        #[cfg(windows)]
        {
            let raw = path.to_string_lossy();
            if let Some(stripped) = raw.strip_prefix(r"\\?\") {
                return PathBuf::from(stripped);
            }
        }
        path.to_path_buf()
    }



    pub(crate) fn project_end_beats(&self) -> f32 {
        let mut max_beat = 0.0f32;
        for track in &self.tracks {
            for clip in &track.clips {
                max_beat = max_beat.max(clip.start_beats + clip.length_beats);
            }
            for note in &track.midi_notes {
                max_beat = max_beat.max(note.start_beats + note.length_beats);
            }
        }
        max_beat
    }

    pub(crate) fn project_clip_range(&self) -> Option<(f32, f32)> {
        let mut min_start = f32::MAX;
        let mut max_end = 0.0f32;
        let mut found = false;
        for track in &self.tracks {
            for clip in &track.clips {
                min_start = min_start.min(clip.start_beats);
                max_end = max_end.max(clip.start_beats + clip.length_beats);
                found = true;
            }
        }
        if found {
            Some((min_start.max(0.0), max_end.max(min_start + 0.25)))
        } else {
            None
        }
    }

    pub(crate) fn active_instrument_path(&self) -> Option<String> {
        if let Some(index) = self.selected_track {
            self.tracks
                .get(index)
                .and_then(|track| track.instrument_path.clone())
        } else {
            self.tracks
                .first()
                .and_then(|track| track.instrument_path.clone())
        }
    }

    pub(crate) fn active_midi_notes(&self) -> Vec<PianoRollNote> {
        if let Some(index) = self.selected_track {
            self.tracks
                .get(index)
                .map(|track| track.midi_notes.clone())
                .unwrap_or_default()
        } else {
            self.tracks
                .first()
                .map(|track| track.midi_notes.clone())
                .unwrap_or_default()
        }
    }

    pub(crate) fn active_track_snapshot(
        &self,
    ) -> Option<(
        Vec<PianoRollNote>,
        Option<String>,
        Vec<u32>,
        Vec<f32>,
        Option<Vec<u8>>,
        Option<Vec<u8>>,
    )> {
        let index = self.selected_track.unwrap_or(0);
        let track = self.tracks.get(index)?;
        Some((
            track.midi_notes.clone(),
            track.instrument_path.clone(),
            track.param_ids.clone(),
            track.param_values.clone(),
            track.plugin_state_component.clone(),
            track.plugin_state_controller.clone(),
        ))
    }

    pub(crate) fn toggle_recording(&mut self) {
        if self.is_recording {
            if let Err(err) = self.end_recording() {
                self.status = format!("Stop recording failed: {err}");
            }
        } else if let Err(err) = self.begin_recording() {
            self.status = format!("Record failed: {err}");
        }
    }

    pub(crate) fn begin_recording(&mut self) -> Result<(), String> {
        if self.is_recording {
            return Ok(());
        }
        if self.render_job.is_some() {
            return Err("Cannot record while rendering".to_string());
        }
        let track_index = self.selected_track.unwrap_or(0).min(self.tracks.len().saturating_sub(1));
        let start_beats = self.playhead_beats.max(0.0);
        let start_samples = self.transport_samples.load(Ordering::Relaxed);
        if !self.audio_running {
            self.start_audio_and_midi()?;
            self.seek_playhead(start_beats);
            self.record_started_audio = true;
        } else {
            self.record_started_audio = false;
        }
        if let Ok(mut rec) = self.recording.lock() {
            rec.active = true;
            rec.track_index = track_index;
            rec.start_samples = start_samples;
            rec.start_beats = start_beats;
            rec.record_audio = self.record_audio;
            rec.record_midi = self.record_midi;
            rec.record_automation = self.record_automation;
            rec.record_performance = self.record_performance;
            rec.audio_samples.clear();
            rec.audio_channels = 0;
            rec.audio_sample_rate = self.settings.sample_rate.max(1);
            rec.midi_active.clear();
            rec.midi_notes.clear();
            rec.automation_points.clear();
            rec.performance_active.clear();
            rec.performance_takes.clear();
        }
        if self.record_audio {
            self.start_audio_input_stream()?;
        }
        self.is_recording = true;
        self.status = "Recording...".to_string();
        Ok(())
    }

    pub(crate) fn end_recording(&mut self) -> Result<(), String> {
        if !self.is_recording {
            return Ok(());
        }
        self.is_recording = false;
        let _stream = self.audio_input_stream.take();
        let mut rec = self.recording.lock().map_err(|_| "Recording lock failed".to_string())?;
        rec.active = false;
        let track_index = rec.track_index;
        let start_beats = rec.start_beats;
        let record_audio = rec.record_audio;
        let record_midi = rec.record_midi;
        let record_automation = rec.record_automation;
        let record_performance = rec.record_performance;
        let audio_samples = std::mem::take(&mut rec.audio_samples);
        let audio_channels = rec.audio_channels.max(1);
        let audio_sample_rate = rec.audio_sample_rate.max(1);
        let midi_notes = std::mem::take(&mut rec.midi_notes);
        let automation_points = std::mem::take(&mut rec.automation_points);
        let end_beat = self.current_transport_beat().max(start_beats);
        let remaining_tracks: Vec<usize> = rec.performance_active.keys().copied().collect();
        for track_index in remaining_tracks {
            Self::finalize_performance_take_locked(&mut rec, track_index, end_beat);
        }
        let performance_takes = std::mem::take(&mut rec.performance_takes);
        drop(rec);

        if record_audio && !audio_samples.is_empty() {
            self.finalize_audio_recording(track_index, start_beats, audio_channels, audio_sample_rate, audio_samples)?;
        }
        if record_midi && !midi_notes.is_empty() {
            self.finalize_midi_recording(track_index, start_beats, midi_notes);
        }
        if record_automation && !automation_points.is_empty() {
            self.apply_recorded_automation(track_index, automation_points);
        }
        if record_performance && !performance_takes.is_empty() {
            self.finalize_performance_recording(performance_takes);
        }

        if self.record_started_audio {
            self.stop_audio_and_midi();
            self.record_started_audio = false;
        }
        self.status = "Recording stopped".to_string();
        Ok(())
    }

    pub(crate) fn start_audio_input_stream(&mut self) -> Result<(), String> {
        let host = cpal::default_host();
        let device = if self.settings.input_device.trim().is_empty() {
            host.default_input_device()
        } else {
            host.input_devices()
                .ok()
                .and_then(|mut devices| {
                    devices.find(|d| d.name().ok().as_deref() == Some(self.settings.input_device.as_str()))
                })
                .or_else(|| host.default_input_device())
        }
        .ok_or("No input device")?;
        let config = device.default_input_config().map_err(|e| e.to_string())?;
        let channels = config.channels() as usize;
        let mut stream_config: cpal::StreamConfig = config.clone().into();
        stream_config.sample_rate = cpal::SampleRate(self.settings.sample_rate.max(1));
        stream_config.buffer_size = cpal::BufferSize::Fixed(self.effective_buffer_size());
        let recording = self.recording.clone();

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &stream_config,
                move |data: &[f32], _| {
                    if let Ok(mut rec) = recording.lock() {
                        if !rec.active || !rec.record_audio {
                            return;
                        }
                        rec.audio_channels = channels;
                        rec.audio_sample_rate = stream_config.sample_rate.0;
                        rec.audio_samples.extend_from_slice(data);
                    }
                },
                move |err| {
                    eprintln!("audio input error: {err}");
                },
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                &stream_config,
                move |data: &[i16], _| {
                    if let Ok(mut rec) = recording.lock() {
                        if !rec.active || !rec.record_audio {
                            return;
                        }
                        rec.audio_channels = channels;
                        rec.audio_sample_rate = stream_config.sample_rate.0;
                        rec.audio_samples.extend(data.iter().map(|s| *s as f32 / i16::MAX as f32));
                    }
                },
                move |err| {
                    eprintln!("audio input error: {err}");
                },
                None,
            ),
            cpal::SampleFormat::U16 => device.build_input_stream(
                &stream_config,
                move |data: &[u16], _| {
                    if let Ok(mut rec) = recording.lock() {
                        if !rec.active || !rec.record_audio {
                            return;
                        }
                        rec.audio_channels = channels;
                        rec.audio_sample_rate = stream_config.sample_rate.0;
                        let norm = u16::MAX as f32;
                        rec.audio_samples.extend(data.iter().map(|s| (*s as f32 / norm) * 2.0 - 1.0));
                    }
                },
                move |err| {
                    eprintln!("audio input error: {err}");
                },
                None,
            ),
            _ => return Err("Unsupported input sample format".to_string()),
        }
        .map_err(|e| e.to_string())?;

        stream.play().map_err(|e| e.to_string())?;
        self.audio_input_stream = Some(stream);
        Ok(())
    }

    pub(crate) fn finalize_audio_recording(
        &mut self,
        track_index: usize,
        start_beats: f32,
        channels: usize,
        sample_rate: u32,
        samples: Vec<f32>,
    ) -> Result<(), String> {
        let project_folder = self.ensure_project_folder()?;
        let audio_dir = project_folder.join("audio");
        fs::create_dir_all(&audio_dir).map_err(|e| e.to_string())?;
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let file_name = format!("recording_{timestamp}.wav");
        let path = audio_dir.join(&file_name);
        let spec = hound::WavSpec {
            channels: channels as u16,
            sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
        let mut writer = hound::WavWriter::new(file, spec).map_err(|e| e.to_string())?;
        for sample in samples.iter() {
            writer.write_sample(*sample).map_err(|e| e.to_string())?;
        }
        writer.finalize().map_err(|e| e.to_string())?;

        let frames = samples.len().saturating_sub(0) / channels.max(1);
        let seconds = frames as f32 / sample_rate.max(1) as f32;
        let beats = (seconds * self.tempo_bpm.max(1.0) / 60.0).max(0.25);
        let clip_id = self.next_clip_id();
        if let Some(track) = self.tracks.get_mut(track_index) {
            let clip = Clip {
                id: clip_id,
                track: track_index,
                start_beats,
                length_beats: beats,
                is_midi: false,
                midi_notes: Vec::new(),
                midi_source_beats: None,
                link_id: None,
                name: "Recording".to_string(),
                audio_path: Some(format!("audio/{file_name}")),
                audio_source_beats: Some(beats),
                audio_offset_beats: 0.0,
                audio_gain: 1.0,
                audio_pitch_semitones: 0.0,
                audio_stretch_mode: AudioStretchMode::Stretch,
                audio_time_mul: 1.0,
                audio_key: None,
                audio_key_minor: false,
                audio_key_source: None,
                audio_bpm: None,
                audio_fine_pitch_cents: 0.0,
                audio_formant_scale: 1.0,
            };
            track.clips.push(clip);
        }
        self.enqueue_audio_analysis(clip_id, path.clone());
        if self.audio_running {
            let timeline = self.build_audio_clip_timeline(self.settings.sample_rate);
            if let Ok(mut guard) = self.audio_clip_timeline.lock() {
                *guard = timeline;
            }
            self.preload_audio_clips(&self.audio_clip_cache);
        }
        self.selected_track = Some(track_index);
        self.selected_clip = Some(clip_id);
        Ok(())
    }

    pub(crate) fn finalize_midi_recording(
        &mut self,
        track_index: usize,
        start_beats: f32,
        notes: Vec<PianoRollNote>,
    ) {
        let clip_id = self.next_clip_id();
        if let Some(track) = self.tracks.get_mut(track_index) {
            let mut max_end = start_beats + 0.25;
            for note in &notes {
                max_end = max_end.max(note.start_beats + note.length_beats);
            }
            track.clips.push(Clip {
                id: clip_id,
                track: track_index,
                start_beats,
                length_beats: (max_end - start_beats).max(0.25),
                is_midi: true,
                midi_notes: notes,
                midi_source_beats: Some((max_end - start_beats).max(0.25)),
                link_id: None,
                name: "MIDI Rec".to_string(),
                audio_path: None,
                audio_source_beats: None,
                audio_offset_beats: 0.0,
                audio_gain: 1.0,
                audio_pitch_semitones: 0.0,
                audio_stretch_mode: AudioStretchMode::Stretch,
                audio_time_mul: 1.0,
                audio_key: None,
                audio_key_minor: false,
                audio_key_source: None,
                audio_bpm: None,
                audio_fine_pitch_cents: 0.0,
                audio_formant_scale: 1.0,
            });
            self.sync_track_audio_notes(track_index);
        }
    }

    pub(crate) fn apply_recorded_automation(&mut self, track_index: usize, points: Vec<RecordedAutomationPoint>) {
        if let Some(track) = self.tracks.get_mut(track_index) {
            let mut grouped: HashMap<(i32, u32), Vec<AutomationPoint>> = HashMap::new();
            for point in points {
                let key = match point.target {
                    AutomationTarget::Instrument => -1,
                    AutomationTarget::Effect(index) => index as i32,
                };
                grouped
                    .entry((key, point.param_id))
                    .or_default()
                    .push(AutomationPoint {
                        beat: point.beat,
                        value: point.value,
                    });
            }
            for ((target_key, param_id), mut new_points) in grouped {
                let target = if target_key < 0 {
                    AutomationTarget::Instrument
                } else {
                    AutomationTarget::Effect(target_key as usize)
                };
                Self::coalesce_automation_points(&mut new_points, 0.02);
                let lane_index = if let Some(index) = track
                    .automation_lanes
                    .iter()
                    .position(|lane| lane.param_id == param_id && lane.target == target)
                {
                    index
                } else {
                    let name = match target {
                        AutomationTarget::Instrument => track
                            .param_ids
                            .iter()
                            .position(|id| *id == param_id)
                            .and_then(|i| track.params.get(i).cloned())
                            .unwrap_or_else(|| format!("Param {}", param_id)),
                        AutomationTarget::Effect(fx_index) => {
                            let fx_name = track
                                .effect_paths
                                .get(fx_index)
                                .map(|p| Self::plugin_display_name(p))
                                .unwrap_or_else(|| format!("FX {}", fx_index + 1));
                            let param_name = track
                                .effect_param_ids
                                .get(fx_index)
                                .and_then(|ids| ids.iter().position(|id| *id == param_id))
                                .and_then(|i| track.effect_params.get(fx_index).and_then(|p| p.get(i)).cloned())
                                .unwrap_or_else(|| format!("Param {}", param_id));
                            format!("{}: {}", fx_name, param_name)
                        }
                    };
                    track.automation_lanes.push(AutomationLane {
                        name,
                        param_id,
                        target,
                        points: Vec::new(),
                    });
                    track.automation_lanes.len() - 1
                };
                if let Some(lane) = track.automation_lanes.get_mut(lane_index) {
                    if new_points.is_empty() {
                        continue;
                    }
                    let range_start = new_points.first().map(|p| p.beat).unwrap_or(0.0);
                    let range_end = new_points.last().map(|p| p.beat).unwrap_or(range_start);
                    let mut merged: Vec<AutomationPoint> = lane
                        .points
                        .iter()
                        .cloned()
                        .filter(|p| p.beat < range_start - 0.02 || p.beat > range_end + 0.02)
                        .collect();
                    merged.extend(new_points.into_iter());
                    Self::coalesce_automation_points(&mut merged, 0.02);
                    lane.points = merged;
                }
            }
            if let Some(state) = self.track_audio.get(track_index) {
                if let Ok(mut lanes) = state.automation_lanes.lock() {
                    *lanes = track.automation_lanes.clone();
                }
            }
        }
    }

    pub(crate) fn record_automation_point(
        &mut self,
        track_index: usize,
        target: AutomationTarget,
        param_id: u32,
        beat: f32,
        value: f32,
    ) {
        if let Some(track) = self.tracks.get_mut(track_index) {
            let lane_index = if let Some(index) = track
                .automation_lanes
                .iter()
                .position(|lane| lane.param_id == param_id && lane.target == target)
            {
                index
            } else {
                let name = match target {
                    AutomationTarget::Instrument => track
                        .param_ids
                        .iter()
                        .position(|id| *id == param_id)
                        .and_then(|i| track.params.get(i).cloned())
                        .unwrap_or_else(|| format!("Param {}", param_id)),
                    AutomationTarget::Effect(fx_index) => {
                        let fx_name = track
                            .effect_paths
                            .get(fx_index)
                            .map(|p| Self::plugin_display_name(p))
                            .unwrap_or_else(|| format!("FX {}", fx_index + 1));
                        let param_name = track
                            .effect_param_ids
                            .get(fx_index)
                            .and_then(|ids| ids.iter().position(|id| *id == param_id))
                            .and_then(|i| track.effect_params.get(fx_index).and_then(|p| p.get(i)).cloned())
                            .unwrap_or_else(|| format!("Param {}", param_id));
                        format!("{}: {}", fx_name, param_name)
                    }
                };
                track.automation_lanes.push(AutomationLane {
                    name,
                    param_id,
                    target,
                    points: Vec::new(),
                });
                track.automation_lanes.len() - 1
            };
            if let Some(lane) = track.automation_lanes.get_mut(lane_index) {
                let mut updated = false;
                for point in lane.points.iter_mut() {
                    if (point.beat - beat).abs() <= 0.02 {
                        point.beat = beat;
                        point.value = value;
                        updated = true;
                        break;
                    }
                }
                if !updated {
                    lane.points.push(AutomationPoint { beat, value });
                }
                Self::coalesce_automation_points(&mut lane.points, 0.02);
            }
            if let Some(state) = self.track_audio.get(track_index) {
                if let Ok(mut lanes) = state.automation_lanes.lock() {
                    *lanes = track.automation_lanes.clone();
                }
            }
        }
    }

    pub(crate) fn current_transport_beat(&self) -> f32 {
        if self.audio_running {
            let sample_rate = self.settings.sample_rate.max(1) as f32;
            let bpm = self.tempo_bpm.max(1.0);
            let samples = self.transport_samples.load(Ordering::Relaxed) as f32;
            (samples / sample_rate) * (bpm / 60.0)
        } else {
            self.playhead_beats.max(0.0)
        }
    }

    pub(crate) fn arrangement_playback_enabled(&self) -> bool {
        self.arrangement_playback_enabled.load(Ordering::Relaxed)
    }

    pub(crate) fn set_arrangement_playback_enabled(&self, enabled: bool) {
        self.arrangement_playback_enabled
            .store(enabled, Ordering::Relaxed);
    }

    pub(crate) fn samples_to_beats(&self, samples: u64) -> f32 {
        let sample_rate = self.settings.sample_rate.max(1) as f32;
        let bpm = self.tempo_bpm.max(1.0);
        (samples as f32 / sample_rate) * (bpm / 60.0)
    }

    pub(crate) fn performance_launch_samples(&self) -> u64 {
        let current_samples = if self.audio_running {
            self.transport_samples.load(Ordering::Relaxed)
        } else {
            self.beats_to_samples(self.playhead_beats.max(0.0), self.settings.sample_rate)
        };
        let quantize_beats = self.performance_launch_quantize_beats.max(0.0);
        if quantize_beats <= f32::EPSILON {
            return current_samples;
        }
        let bpm = self.tempo_bpm.max(1.0) as f64;
        let sample_rate = self.settings.sample_rate.max(1) as f64;
        let quantum_samples = (quantize_beats as f64 * sample_rate * 60.0 / bpm)
            .round()
            .max(1.0) as u64;
        if quantum_samples <= 1 || current_samples == 0 {
            return current_samples;
        }
        let remainder = current_samples % quantum_samples;
        if remainder == 0 {
            current_samples
        } else {
            current_samples + (quantum_samples - remainder)
        }
    }

    pub(crate) fn start_session_clock(&mut self) -> Result<(), String> {
        self.seek_playhead(self.playhead_beats);
        self.set_arrangement_playback_enabled(false);
        self.start_audio_and_midi_internal(false)
    }

    pub(crate) fn finalize_performance_take_locked(
        rec: &mut RecordingBuffers,
        track_index: usize,
        end_beat: f32,
    ) {
        let Some(active) = rec.performance_active.remove(&track_index) else {
            return;
        };
        let final_end = end_beat.max(active.start_beat + 0.05);
        match active.trigger_mode {
            PerformanceTriggerMode::OneShot => {}
            _ => {
                rec.performance_takes.push(RecordedPerformanceTake {
                    track_index,
                    source_clip_id: active.source_clip_id,
                    start_beat: active.start_beat,
                    end_beat: final_end,
                    loop_enabled: active.loop_enabled,
                });
            }
        }
    }

    pub(crate) fn record_performance_clip_trigger(
        &mut self,
        track_index: usize,
        clip_id: usize,
        launch_beat: f32,
        settings: PerformanceClipSettings,
    ) {
        if !self.is_recording {
            return;
        }
        let now = launch_beat.max(0.0);
        let source_length = self
            .find_clip_indices_by_id(clip_id)
            .and_then(|(ti, ci)| self.tracks.get(ti).and_then(|t| t.clips.get(ci)))
            .map(|clip| clip.length_beats.max(0.25))
            .unwrap_or(0.25);
        if let Ok(mut rec) = self.recording.lock() {
            if !rec.active || !rec.record_performance {
                return;
            }
            let same_active = rec
                .performance_active
                .get(&track_index)
                .map(|active| active.source_clip_id == clip_id)
                .unwrap_or(false);
            match settings.trigger_mode {
                PerformanceTriggerMode::OneShot => {
                    Self::finalize_performance_take_locked(&mut rec, track_index, now);
                    rec.performance_takes.push(RecordedPerformanceTake {
                        track_index,
                        source_clip_id: clip_id,
                        start_beat: now,
                        end_beat: now + source_length,
                        loop_enabled: false,
                    });
                }
                PerformanceTriggerMode::Toggle => {
                    if same_active {
                        Self::finalize_performance_take_locked(&mut rec, track_index, now);
                    } else {
                        Self::finalize_performance_take_locked(&mut rec, track_index, now);
                        rec.performance_active.insert(
                            track_index,
                            ActivePerformanceTake {
                                source_clip_id: clip_id,
                                start_beat: now,
                                trigger_mode: settings.trigger_mode,
                                loop_enabled: settings.loop_enabled,
                            },
                        );
                    }
                }
                PerformanceTriggerMode::Gate | PerformanceTriggerMode::Loop => {
                    Self::finalize_performance_take_locked(&mut rec, track_index, now);
                    rec.performance_active.insert(
                        track_index,
                        ActivePerformanceTake {
                            source_clip_id: clip_id,
                            start_beat: now,
                            trigger_mode: settings.trigger_mode,
                            loop_enabled: settings.loop_enabled
                                || settings.trigger_mode == PerformanceTriggerMode::Loop,
                        },
                    );
                }
            }
        }
    }

    pub(crate) fn preload_performance_audio_clip(&self, path_str: &str) {
        if let Ok(mut cache) = self.audio_clip_cache.lock() {
            if cache.get(path_str).is_none() {
                let path = PathBuf::from(path_str);
                if let Some(data) = Self::load_audio_clip_data(&path) {
                    cache.insert(path_str.to_string(), Arc::new(data));
                }
            }
        }
    }

    pub(crate) fn launch_performance_clip(
        &mut self,
        track_index: usize,
        clip_id: usize,
        settings: PerformanceClipSettings,
    ) -> Result<(), String> {
        self.launch_performance_clip_at(
            track_index,
            clip_id,
            settings,
            self.performance_launch_samples(),
        )
    }

    pub(crate) fn launch_performance_scene(&mut self, scene_beat: f32) -> Result<usize, String> {
        let launch_samples = self.performance_launch_samples();
        self.launch_performance_scene_at(scene_beat, launch_samples)
    }

    pub(crate) fn launch_performance_scene_at(
        &mut self,
        scene_beat: f32,
        launch_samples: u64,
    ) -> Result<usize, String> {
        let launches: Vec<(usize, usize, PerformanceClipSettings)> = self
            .tracks
            .iter()
            .enumerate()
            .filter_map(|(track_index, track)| {
                track.clips
                    .iter()
                    .find(|clip| performance_scene_matches(clip.start_beats, scene_beat))
                    .map(|clip| {
                        (
                            track_index,
                            clip.id,
                            self.performance_clip_settings
                                .get(&clip.id)
                                .cloned()
                                .unwrap_or_default(),
                        )
                    })
            })
            .collect();
        let launched = launches.len();
        for (track_index, clip_id, settings) in launches {
            self.launch_performance_clip_at(track_index, clip_id, settings, launch_samples)?;
        }
        Ok(launched)
    }

    pub(crate) fn stop_performance_track(&mut self, track_index: usize) {
        if let Ok(mut runtime) = self.performance_runtime.lock() {
            if let Some(slot) = runtime.get_mut(track_index) {
                *slot = None;
            }
        }
        self.send_all_notes_off(track_index);
    }

    pub(crate) fn record_performance_scene_trigger(&mut self, scene_beat: f32) -> usize {
        self.record_performance_scene_trigger_at(scene_beat, self.current_transport_beat())
    }

    pub(crate) fn record_performance_scene_trigger_at(&mut self, scene_beat: f32, launch_beat: f32) -> usize {
        let launches: Vec<(usize, usize, PerformanceClipSettings)> = self
            .tracks
            .iter()
            .enumerate()
            .filter_map(|(track_index, track)| {
                track.clips
                    .iter()
                    .find(|clip| performance_scene_matches(clip.start_beats, scene_beat))
                    .map(|clip| {
                        (
                            track_index,
                            clip.id,
                            self.performance_clip_settings
                                .get(&clip.id)
                                .cloned()
                                .unwrap_or_default(),
                        )
                    })
            })
            .collect();
        let launched = launches.len();
        for (track_index, clip_id, settings) in launches {
            self.record_performance_clip_trigger(track_index, clip_id, launch_beat, settings);
        }
        launched
    }

    pub(crate) fn record_performance_track_stop(&mut self, track_index: usize) {
        if !self.is_recording {
            return;
        }
        let now = self.current_transport_beat().max(0.0);
        if let Ok(mut rec) = self.recording.lock() {
            if !rec.active || !rec.record_performance {
                return;
            }
            Self::finalize_performance_take_locked(&mut rec, track_index, now);
        }
    }

    pub(crate) fn make_recorded_performance_clip(
        &self,
        source: &Clip,
        track_index: usize,
        start_beat: f32,
        end_beat: f32,
        loop_enabled: bool,
        clip_id: usize,
    ) -> Option<Clip> {
        let requested_len = (end_beat - start_beat).max(0.05);
        let source_len = source.length_beats.max(0.25);
        let target_len = if loop_enabled {
            requested_len.max(0.25)
        } else {
            requested_len.min(source_len).max(0.25)
        };
        let mut clip = source.clone();
        clip.id = clip_id;
        clip.track = track_index;
        clip.start_beats = start_beat.max(0.0);
        clip.length_beats = target_len;
        clip.link_id = None;
        clip.name = if source.name.trim().is_empty() {
            "Performance".to_string()
        } else {
            source.name.clone()
        };
        if clip.is_midi {
            let delta = clip.start_beats - source.start_beats;
            for note in &mut clip.midi_notes {
                note.start_beats = (note.start_beats + delta).max(0.0);
            }
            let clip_end = clip.start_beats + clip.length_beats;
            clip.midi_notes.retain_mut(|note| {
                let note_start = note.start_beats.max(clip.start_beats);
                let note_end = (note.start_beats + note.length_beats).min(clip_end);
                let next_len = note_end - note_start;
                if next_len <= 0.0 {
                    return false;
                }
                note.start_beats = note_start;
                note.length_beats = next_len;
                true
            });
            clip.midi_source_beats = if loop_enabled {
                Some(source.midi_source_beats.unwrap_or(source_len))
            } else {
                None
            };
        } else {
            clip.audio_source_beats = if loop_enabled {
                Some(source.audio_source_beats.unwrap_or(source_len))
            } else {
                None
            };
        }
        Some(clip)
    }

    pub(crate) fn finalize_performance_recording(&mut self, takes: Vec<RecordedPerformanceTake>) {
        let mut next_clip_id = self.next_clip_id();
        let mut changed_tracks = HashSet::new();
        let mut added_audio_clip = false;
        let mut last_clip_id = None;
        let mut added_count = 0usize;

        for take in takes {
            let Some((source_track_index, source_clip_index)) = self.find_clip_indices_by_id(take.source_clip_id) else {
                continue;
            };
            let Some(source_clip) = self
                .tracks
                .get(source_track_index)
                .and_then(|track| track.clips.get(source_clip_index))
                .cloned()
            else {
                continue;
            };
            let Some(new_clip) = self.make_recorded_performance_clip(
                &source_clip,
                take.track_index,
                take.start_beat,
                take.end_beat,
                take.loop_enabled,
                next_clip_id,
            ) else {
                continue;
            };
            if let Some(track) = self.tracks.get_mut(take.track_index) {
                if !new_clip.is_midi {
                    added_audio_clip = true;
                }
                track.clips.push(new_clip);
                changed_tracks.insert(take.track_index);
                last_clip_id = Some(next_clip_id);
                next_clip_id = next_clip_id.saturating_add(1);
                added_count = added_count.saturating_add(1);
            }
        }

        if changed_tracks.is_empty() {
            return;
        }

        for track_index in changed_tracks.iter().copied() {
            self.sync_track_audio_notes(track_index);
        }
        if added_audio_clip && self.audio_running {
            let timeline = self.build_audio_clip_timeline(self.settings.sample_rate);
            if let Ok(mut guard) = self.audio_clip_timeline.lock() {
                *guard = timeline;
            }
            self.preload_audio_clips(&self.audio_clip_cache);
        }
        if let Some(clip_id) = last_clip_id {
            self.selected_clip = Some(clip_id);
            self.performance_selected_clip = Some(clip_id);
        }
        self.mark_dirty();
        self.status = format!("Performance recorded: {} clips", added_count);
    }

    pub(crate) fn coalesce_automation_points(points: &mut Vec<AutomationPoint>, epsilon: f32) {
        points.sort_by(|a, b| a.beat.partial_cmp(&b.beat).unwrap_or(std::cmp::Ordering::Equal));
        let mut merged: Vec<AutomationPoint> = Vec::with_capacity(points.len());
        for point in points.drain(..) {
            if let Some(last) = merged.last_mut() {
                if (last.beat - point.beat).abs() <= epsilon {
                    *last = point;
                    continue;
                }
            }
            merged.push(point);
        }
        *points = merged;
    }

    pub(crate) fn base_buffer_size(&self) -> u32 {
        self.buffer_override
            .unwrap_or(self.settings.buffer_size)
            .max(1)
    }

    pub(crate) fn effective_buffer_size(&self) -> u32 {
        let base = self.base_buffer_size();
        if self.settings.triple_buffer {
            base.saturating_mul(3).max(1)
        } else {
            base
        }
    }


    pub(crate) fn start_audio_and_midi(&mut self) -> Result<(), String> {
        self.set_arrangement_playback_enabled(true);
        self.start_audio_and_midi_internal(true)
    }

    pub(crate) fn start_audio_and_midi_internal(&mut self, reset_transport: bool) -> Result<(), String> {
        if self.render_job.is_some() {
            return Err("Cannot start playback while rendering".to_string());
        }
        if self.audio_running {
            return Ok(());
        }
        self.audio_stats = Arc::new(AudioRuntimeStats::new());
        self.audio_stop.store(false, Ordering::Relaxed);
        let host = cpal::default_host();
        let device = if self.settings.output_device.trim().is_empty() {
            host.default_output_device()
        } else {
            host.output_devices()
                .ok()
                .and_then(|mut devices| {
                    devices.find(|d| d.name().ok().as_deref() == Some(self.settings.output_device.as_str()))
                })
                .or_else(|| host.default_output_device())
        }
        .ok_or("No output device")?;
        let config = device.default_output_config().map_err(|e| e.to_string())?;
        let sample_rate = self.settings.sample_rate.max(1) as f32;
        let channels = config.channels() as usize;
        self.last_output_channels = channels.max(1);
        let effective_buffer = self.effective_buffer_size();
        let buffer_size_usize = effective_buffer as usize;
        let freq_bits = self.midi_freq_bits.clone();
        let gate = self.midi_gate.clone();
        let master_peak_bits = self.master_peak_bits.clone();
        let master_settings = self.master_settings.clone();
        let master_comp_state = self.master_comp_state.clone();
        self.adaptive_buffer_size
            .store(effective_buffer, Ordering::Relaxed);
        self.last_overrun.store(false, Ordering::Relaxed);
        if reset_transport {
            self.transport_samples.store(0, Ordering::Relaxed);
            self.playback_panic.store(true, Ordering::Relaxed);
            self.playback_fade_in.store(true, Ordering::Relaxed);
        }
        self.ensure_synth_soundfont();
        self.tempo_bits.store(self.tempo_bpm.to_bits(), Ordering::Relaxed);
        self.sync_track_audio_states();
        let timeline = self.build_audio_clip_timeline(self.settings.sample_rate);
        if let Ok(mut guard) = self.audio_clip_timeline.lock() {
            *guard = timeline;
        }
        self.preload_audio_clips(&self.audio_clip_cache);
        let mut micesynth_program_sync: Vec<usize> = Vec::new();
        for index in 0..self.tracks.len() {
            let path = self.tracks[index].instrument_path.clone();
            let effect_paths = self.tracks[index].effect_paths.clone();
            let sync_micesynth_program = self
                .tracks
                .get(index)
                .and_then(|track| track.instrument_path.as_deref())
                .map(Self::is_micesynth_path)
                .unwrap_or(false)
                && self.tracks.get(index).and_then(|track| track.midi_program).is_some();
            let state = match self.track_audio.get_mut(index) {
                Some(state) => state,
                None => continue,
            };
            if let Some(path) = path {
                if state.host.is_none() {
                    let kind = Self::plugin_kind_from_path(&path);
                    let host = match kind {
                        PluginKind::Native => None,
                        PluginKind::Vst3 => vst3::Vst3Host::load(
                            &path,
                            self.settings.sample_rate as f64,
                            buffer_size_usize,
                            channels,
                        )
                        .ok()
                        .map(|host| PluginHostHandle::Vst3(Arc::new(Mutex::new(host)))),
                        PluginKind::Clap => {
                            let clap_id = self
                                .tracks
                                .get(index)
                                .and_then(|track| track.instrument_clap_id.clone())
                                .or_else(|| clap_host::default_plugin_id(&path).ok());
                            clap_id.and_then(|clap_id| {
                                if let Some(track) = self.tracks.get_mut(index) {
                                    track.instrument_clap_id = Some(clap_id.clone());
                                }
                                clap_host::ClapHost::load(
                                    &path,
                                    &clap_id,
                                    self.settings.sample_rate as f64,
                                    buffer_size_usize as u32,
                                    channels,
                                    channels.min(MAX_CLAP_OUTPUT_CHANNELS),
                                )
                                .ok()
                                .map(|host| PluginHostHandle::Clap(Arc::new(Mutex::new(host))))
                            })
                        }
                    };
                    if let Some(host) = host {
                        let params = host.enumerate_params();
                        if let Some(track) = self.tracks.get_mut(index) {
                            if !params.is_empty() {
                                let next_values = Self::remap_param_values_by_id_or_name(
                                    &track.param_ids,
                                    &track.params,
                                    &track.param_values,
                                    &params,
                                );
                                track.params = params.iter().map(|p| p.name.clone()).collect();
                                track.param_ids = params.iter().map(|p| p.id).collect();
                                track.param_values = next_values;
                                Self::apply_program_param(track);
                            }
                        }
                        state.host = Some(host.clone());
                        if sync_micesynth_program {
                            micesynth_program_sync.push(index);
                        }
                        if let Some(track) = self.tracks.get(index) {
                            let component = track.plugin_state_component.clone();
                            let controller = track.plugin_state_controller.clone();
                            let has_state = component
                                .as_ref()
                                .map(|v| !v.is_empty())
                                .unwrap_or(false)
                                || controller
                                    .as_ref()
                                    .map(|v| !v.is_empty())
                                    .unwrap_or(false);
                            let allow_state_restore = if kind == PluginKind::Clap {
                                // Disabled for CLAP runtime init path to avoid plugin crashes
                                // from stale/incompatible serialized state.
                                false
                            } else {
                                true
                            };
                            if has_state && allow_state_restore {
                                let _ = host.set_state_bytes(
                                    component.as_deref(),
                                    controller.as_deref(),
                                );
                                if let Some(track) = self.tracks.get_mut(index) {
                                    if track.param_values.len() != track.param_ids.len() {
                                        track.param_values.resize(track.param_ids.len(), 0.0);
                                    }
                                    for (slot, param_id) in track.param_ids.iter().enumerate() {
                                        if let Some(value) = host.get_param_normalized(*param_id) {
                                            if let Some(target) = track.param_values.get_mut(slot) {
                                                *target = value as f32;
                                            }
                                        }
                                    }
                                }
                            } else if has_state && kind == PluginKind::Clap {
                                eprintln!(
                                    "CLAP state restore skipped for track {} during runtime init",
                                    index
                                );
                            } else if !track.param_ids.is_empty() {
                                for (param_id, value) in
                                    track.param_ids.iter().zip(track.param_values.iter())
                                {
                                    host.push_param_change(*param_id, *value as f64);
                                }
                            }
                        }
                    } else if kind != PluginKind::Native {
                        self.status = "Plugin host error: unable to load".to_string();
                    }
                }
            }
            if state.effect_hosts.len() != effect_paths.len() {
                for host in state.effect_hosts.drain(..) {
                    host.prepare_for_drop();
                    self.orphaned_hosts.push(host);
                }
                for (slot, fx_path) in effect_paths.iter().enumerate() {
                    let kind = Self::plugin_kind_from_path(fx_path);
                    let host = match kind {
                        PluginKind::Native => None,
                        PluginKind::Vst3 => vst3::Vst3Host::load_with_input(
                            fx_path,
                            self.settings.sample_rate as f64,
                            buffer_size_usize,
                            channels,
                            channels,
                        )
                        .ok()
                        .map(|host| PluginHostHandle::Vst3(Arc::new(Mutex::new(host)))),
                        PluginKind::Clap => {
                            let clap_id = self
                                .tracks
                                .get(index)
                                .and_then(|track| track.effect_clap_ids.get(slot).and_then(|id| id.clone()))
                                .or_else(|| clap_host::default_plugin_id(fx_path).ok());
                            clap_id.and_then(|clap_id| {
                                if let Some(track) = self.tracks.get_mut(index) {
                                    if track.effect_clap_ids.len() < effect_paths.len() {
                                        track.effect_clap_ids.resize(effect_paths.len(), None);
                                    }
                                    track.effect_clap_ids[slot] = Some(clap_id.clone());
                                }
                                clap_host::ClapHost::load(
                                    fx_path,
                                    &clap_id,
                                    self.settings.sample_rate as f64,
                                    buffer_size_usize as u32,
                                    channels,
                                    channels.min(MAX_CLAP_OUTPUT_CHANNELS),
                                )
                                .ok()
                                .map(|host| PluginHostHandle::Clap(Arc::new(Mutex::new(host))))
                            })
                        }
                    };
                    if let Some(host) = host {
                        state.effect_hosts.push(host);
                    } else if kind != PluginKind::Native {
                        self.status = "FX host error: unable to load".to_string();
                    }
                }
            }
            if let Some(track) = self.tracks.get_mut(index) {
                if track.effect_bypass.len() != effect_paths.len() {
                    track.effect_bypass.resize(effect_paths.len(), false);
                }
                if track.effect_clap_ids.len() != effect_paths.len() {
                    track.effect_clap_ids.resize(effect_paths.len(), None);
                }
                if track.effect_params.len() != effect_paths.len() {
                    track.effect_params.resize(effect_paths.len(), Vec::new());
                    track.effect_param_ids.resize(effect_paths.len(), Vec::new());
                    track.effect_param_values.resize(effect_paths.len(), Vec::new());
                }
                for (fx_index, fx_host) in state.effect_hosts.iter().enumerate() {
                    let params = fx_host.enumerate_params();
                    if !params.is_empty() {
                        let next_values = if let (Some(old_ids), Some(old_names), Some(old_values)) = (
                            track.effect_param_ids.get(fx_index),
                            track.effect_params.get(fx_index),
                            track.effect_param_values.get(fx_index),
                        ) {
                            Self::remap_param_values_by_id_or_name(
                                old_ids,
                                old_names,
                                old_values,
                                &params,
                            )
                        } else {
                            params.iter().map(|p| p.default_value as f32).collect()
                        };
                        if let Some(slot) = track.effect_params.get_mut(fx_index) {
                            *slot = params.iter().map(|p| p.name.clone()).collect();
                        }
                        if let Some(slot) = track.effect_param_ids.get_mut(fx_index) {
                            *slot = params.iter().map(|p| p.id).collect();
                        }
                        if let Some(slot) = track.effect_param_values.get_mut(fx_index) {
                            *slot = next_values;
                        }
                    }
                }
                state.sync_effect_bypass(track);
            }
        }
        for index in micesynth_program_sync {
            self.apply_micesynth_program_from_midi(index);
        }
        self.send_midi_stop_to_hosts();
        self.warmup_hosts(channels, buffer_size_usize, 2);
        self.sync_node_routes();
        let track_audio = self.track_audio.clone();
        let track_mix = self.track_mix.clone();
        let node_activity_rt = self.node_activity_rt.clone();
        let node_routes_rt = self.node_routes_rt.clone();
        let performance_runtime = self.performance_runtime.clone();
        let arrangement_playback_enabled = self.arrangement_playback_enabled.clone();
        let tempo_bits = self.tempo_bits.clone();
        let transport_samples = self.transport_samples.clone();
        let loop_start_samples = self.loop_start_samples.clone();
        let loop_end_samples = self.loop_end_samples.clone();
        let playback_panic = self.playback_panic.clone();
        let playback_fade_in = self.playback_fade_in.clone();
        let audio_stop = self.audio_stop.clone();
        let audio_callback_active = self.audio_callback_active.clone();
        let audio_clip_cache = self.audio_clip_cache.clone();
        let audio_clip_timeline = self.audio_clip_timeline.clone();
        let adaptive_enabled = self.settings.adaptive_buffer;
        let safe_underruns = self.settings.safe_underruns;
        let smart_disable_plugins = self.settings.smart_disable_plugins;
        let smart_suspend_tracks = self.settings.smart_suspend_tracks;
        let adaptive_restart_requested = self.adaptive_restart_requested.clone();
        let adaptive_buffer_size = self.adaptive_buffer_size.clone();
        let last_overrun = self.last_overrun.clone();
        let audio_stats = self.audio_stats.clone();

        let mut stream_config: cpal::StreamConfig = config.clone().into();
        let target_rate = self.settings.sample_rate;
        let mut actual_rate = target_rate;
        if let Ok(supported) = device.supported_output_configs() {
            let mut found = false;
            let mut closest_rate = 44100;
            let mut min_diff = u32::MAX;
            for conf in supported {
                let min = conf.min_sample_rate().0;
                let max = conf.max_sample_rate().0;
                if target_rate >= min && target_rate <= max {
                    found = true;
                    break;
                }
                let diff_min = target_rate.abs_diff(min);
                let diff_max = target_rate.abs_diff(max);
                if diff_min < min_diff { min_diff = diff_min; closest_rate = min; }
                if diff_max < min_diff { min_diff = diff_max; closest_rate = max; }
            }
            if !found && min_diff != u32::MAX {
                actual_rate = closest_rate;
            }
        }
        stream_config.sample_rate = cpal::SampleRate(actual_rate);
        stream_config.buffer_size = cpal::BufferSize::Fixed(effective_buffer);

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                let track_audio = track_audio.clone();
                let track_mix = track_mix.clone();
                let node_activity_rt = node_activity_rt.clone();
                let node_routes_rt = node_routes_rt.clone();
                let tempo_bits = tempo_bits.clone();
                let transport_samples = transport_samples.clone();
                let audio_stats = audio_stats.clone();
                let mut runtime_buffers = AudioRuntimeBuffers::new(track_audio.len(), effective_buffer as usize);
                device.build_output_stream(
                    &stream_config,
                    move |data: &mut [f32], _| {
                        let _guard = CallbackGuard::new(audio_callback_active.clone());
                        if audio_stop.load(Ordering::Relaxed) {
                            data.fill(0.0);
                            update_master_peak_f32(data, &master_peak_bits);
                            return;
                        }
                        if safe_underruns && last_overrun.swap(false, Ordering::Relaxed) {
                            data.fill(0.0);
                            update_master_peak_f32(data, &master_peak_bits);
                            return;
                        }
                        let started = std::time::Instant::now();
                        data.fill(0.0);
                        let processed = mix_track_hosts(
                            data,
                            channels,
                            sample_rate,
                            &tempo_bits,
                            &transport_samples,
                            &loop_start_samples,
                            &loop_end_samples,
                            &playback_panic,
                            &arrangement_playback_enabled,
                            &track_audio,
                            &track_mix,
                            &node_activity_rt,
                            &node_routes_rt,
                            &performance_runtime,
                            &audio_clip_timeline,
                            &audio_clip_cache,
                            smart_disable_plugins,
                            smart_suspend_tracks,
                            &mut runtime_buffers,
                        );
                        if !processed {
                            render_sine(data, channels, sample_rate, &freq_bits, &gate);
                        }
                        let settings = master_settings.try_lock().map(|s| s.clone()).unwrap_or_default();
                        if let Ok(mut state) = master_comp_state.try_lock() {
                            apply_master_processing(
                                data,
                                channels,
                                sample_rate,
                                &settings,
                                &mut state,
                            );
                        }
                        apply_fade_in_if_needed(data, channels, &playback_fade_in);
                        for sample in data.iter_mut() {
                            *sample = sample.clamp(-1.0, 1.0);
                        }
                        update_master_peak_f32(data, &master_peak_bits);
                        let elapsed = started.elapsed().as_secs_f32();
                        let buffer_secs = (data.len() / channels) as f32 / sample_rate.max(1.0);
                        let overrun = elapsed > buffer_secs;
                        audio_stats.record_block(elapsed * 1000.0, overrun);
                        if overrun {
                            if safe_underruns {
                                last_overrun.store(true, Ordering::Relaxed);
                            }
                            if adaptive_enabled {
                                let current = adaptive_buffer_size.load(Ordering::Relaxed);
                                let next = (current.saturating_mul(2)).min(8192).max(current);
                                if next > current {
                                    adaptive_buffer_size.store(next, Ordering::Relaxed);
                                    adaptive_restart_requested.store(true, Ordering::Relaxed);
                                }
                            }
                        }
                    },
                    move |err| {
                        eprintln!("audio error: {err}");
                    },
                    None,
                )
            }
            cpal::SampleFormat::I16 => {
                let track_audio = track_audio.clone();
                let track_mix = track_mix.clone();
                let node_activity_rt = node_activity_rt.clone();
                let node_routes_rt = node_routes_rt.clone();
                let tempo_bits = tempo_bits.clone();
                let transport_samples = transport_samples.clone();
                let audio_stop = audio_stop.clone();
                let audio_callback_active = audio_callback_active.clone();
                let audio_stats = audio_stats.clone();
                let mut temp = Vec::<f32>::new();
                let mut runtime_buffers = AudioRuntimeBuffers::new(track_audio.len(), effective_buffer as usize);
                device.build_output_stream(
                    &stream_config,
                    move |data: &mut [i16], _| {
                        let _guard = CallbackGuard::new(audio_callback_active.clone());
                        if audio_stop.load(Ordering::Relaxed) {
                            data.fill(0);
                            update_master_peak_i16(data, &master_peak_bits);
                            return;
                        }
                        if safe_underruns && last_overrun.swap(false, Ordering::Relaxed) {
                            data.fill(0);
                            update_master_peak_i16(data, &master_peak_bits);
                            return;
                        }
                        let started = std::time::Instant::now();
                        if temp.len() != data.len() { temp.resize(data.len(), 0.0); }
                        temp.fill(0.0);
                        let processed = mix_track_hosts(
                            &mut temp,
                            channels,
                            sample_rate,
                            &tempo_bits,
                            &transport_samples,
                            &loop_start_samples,
                            &loop_end_samples,
                            &playback_panic,
                            &arrangement_playback_enabled,
                            &track_audio,
                            &track_mix,
                            &node_activity_rt,
                            &node_routes_rt,
                            &performance_runtime,
                            &audio_clip_timeline,
                            &audio_clip_cache,
                            smart_disable_plugins,
                            smart_suspend_tracks,
                            &mut runtime_buffers,
                        );
                        if !processed {
                            render_sine(&mut temp, channels, sample_rate, &freq_bits, &gate);
                        }
                        let settings = master_settings.try_lock().map(|s| s.clone()).unwrap_or_default();
                        if let Ok(mut state) = master_comp_state.try_lock() {
                            apply_master_processing(
                                &mut temp,
                                channels,
                                sample_rate,
                                &settings,
                                &mut state,
                            );
                        }
                        apply_fade_in_if_needed(&mut temp, channels, &playback_fade_in);
                        for sample in temp.iter_mut() {
                            *sample = sample.clamp(-1.0, 1.0);
                        }
                        for (out, sample) in data.iter_mut().zip(temp.iter()) {
                            *out = cpal::Sample::from_sample(*sample);
                        }
                        update_master_peak_f32(&temp, &master_peak_bits);
                        let elapsed = started.elapsed().as_secs_f32();
                        let buffer_secs = (data.len() / channels) as f32 / sample_rate.max(1.0);
                        let overrun = elapsed > buffer_secs;
                        audio_stats.record_block(elapsed * 1000.0, overrun);
                        if overrun {
                            if safe_underruns {
                                last_overrun.store(true, Ordering::Relaxed);
                            }
                            if adaptive_enabled {
                                let current = adaptive_buffer_size.load(Ordering::Relaxed);
                                let next = (current.saturating_mul(2)).min(8192).max(current);
                                if next > current {
                                    adaptive_buffer_size.store(next, Ordering::Relaxed);
                                    adaptive_restart_requested.store(true, Ordering::Relaxed);
                                }
                            }
                        }
                    },
                    move |err| {
                        eprintln!("audio error: {err}");
                    },
                    None,
                )
            }
            cpal::SampleFormat::U16 => {
                let track_audio = track_audio.clone();
                let track_mix = track_mix.clone();
                let node_activity_rt = node_activity_rt.clone();
                let node_routes_rt = node_routes_rt.clone();
                let tempo_bits = tempo_bits.clone();
                let transport_samples = transport_samples.clone();
                let audio_stop = audio_stop.clone();
                let audio_callback_active = audio_callback_active.clone();
                let audio_stats = audio_stats.clone();
                let mut temp = Vec::<f32>::new();
                let mut runtime_buffers = AudioRuntimeBuffers::new(track_audio.len(), effective_buffer as usize);
                device.build_output_stream(
                    &stream_config,
                    move |data: &mut [u16], _| {
                        let _guard = CallbackGuard::new(audio_callback_active.clone());
                        if audio_stop.load(Ordering::Relaxed) {
                            let silence = u16::MAX / 2;
                            data.fill(silence);
                            update_master_peak_u16(data, &master_peak_bits);
                            return;
                        }
                        if safe_underruns && last_overrun.swap(false, Ordering::Relaxed) {
                            let silence = u16::MAX / 2;
                            data.fill(silence);
                            update_master_peak_u16(data, &master_peak_bits);
                            return;
                        }
                        let started = std::time::Instant::now();
                        if temp.len() != data.len() { temp.resize(data.len(), 0.0); }
                        temp.fill(0.0);
                        let processed = mix_track_hosts(
                            &mut temp,
                            channels,
                            sample_rate,
                            &tempo_bits,
                            &transport_samples,
                            &loop_start_samples,
                            &loop_end_samples,
                            &playback_panic,
                            &arrangement_playback_enabled,
                            &track_audio,
                            &track_mix,
                            &node_activity_rt,
                            &node_routes_rt,
                            &performance_runtime,
                            &audio_clip_timeline,
                            &audio_clip_cache,
                            smart_disable_plugins,
                            smart_suspend_tracks,
                            &mut runtime_buffers,
                        );
                        if !processed {
                            render_sine(&mut temp, channels, sample_rate, &freq_bits, &gate);
                        }
                        let settings = master_settings.try_lock().map(|s| s.clone()).unwrap_or_default();
                        if let Ok(mut state) = master_comp_state.try_lock() {
                            apply_master_processing(
                                &mut temp,
                                channels,
                                sample_rate,
                                &settings,
                                &mut state,
                            );
                        }
                        apply_fade_in_if_needed(&mut temp, channels, &playback_fade_in);
                        for sample in temp.iter_mut() {
                            *sample = sample.clamp(-1.0, 1.0);
                        }
                        for (out, sample) in data.iter_mut().zip(temp.iter()) {
                            *out = cpal::Sample::from_sample(*sample);
                        }
                        update_master_peak_f32(&temp, &master_peak_bits);
                        let elapsed = started.elapsed().as_secs_f32();
                        let buffer_secs = (data.len() / channels) as f32 / sample_rate.max(1.0);
                        let overrun = elapsed > buffer_secs;
                        audio_stats.record_block(elapsed * 1000.0, overrun);
                        if overrun {
                            if safe_underruns {
                                last_overrun.store(true, Ordering::Relaxed);
                            }
                            if adaptive_enabled {
                                let current = adaptive_buffer_size.load(Ordering::Relaxed);
                                let next = (current.saturating_mul(2)).min(8192).max(current);
                                if next > current {
                                    adaptive_buffer_size.store(next, Ordering::Relaxed);
                                    adaptive_restart_requested.store(true, Ordering::Relaxed);
                                }
                            }
                        }
                    },
                    move |err| {
                        eprintln!("audio error: {err}");
                    },
                    None,
                )
            }
            _ => return Err("Unsupported sample format".to_string()),
        }
        .map_err(|e| e.to_string())?;

        stream.play().map_err(|e| e.to_string())?;
        self.audio_stream = Some(stream);

        if let Err(err) = self.reconnect_midi_inputs() {
            self.status = err;
        }

        self.audio_running = true;
        if self.adaptive_restart_pending {
            self.adaptive_restart_pending = false;
            self.status = format!("Audio buffer applied");
        }
        Ok(())
    }

    pub(crate) fn reinit_audio_if_running(&mut self) {
        if !self.audio_running {
            return;
        }
        self.stop_audio_and_midi();
        if let Err(err) = self.start_audio_and_midi() {
            self.status = format!("Audio restart failed: {err}");
        } else {
            self.status = "Audio restarted for new VST3".to_string();
        }
    }

    pub(crate) fn stop_audio_and_midi(&mut self) {
        self.stop_audio_and_midi_internal(true);
    }

    pub(crate) fn pause_audio_and_midi(&mut self) {
        if !self.audio_running {
            return;
        }
        self.audio_stop.store(true, Ordering::Relaxed);
        self.audio_running = false;
        self.set_arrangement_playback_enabled(false);
        if let Ok(mut runtime) = self.performance_runtime.lock() {
            runtime.iter_mut().for_each(|slot| *slot = None);
        }
        self.midi_conns.clear();
        let _stream = self.audio_stream.take();
        let _input = self.audio_input_stream.take();
        let start = std::time::Instant::now();
        while self.audio_callback_active.load(Ordering::Relaxed) > 0 {
            if start.elapsed() > std::time::Duration::from_millis(1000) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        self.send_midi_stop_to_hosts();
        self.midi_gate.store(false, Ordering::Relaxed);
        for state in &self.track_audio {
            if let Ok(mut events) = state.midi_events.lock() {
                events.clear();
            }
        }
    }

    pub(crate) fn stop_audio_and_midi_internal(&mut self, reset_transport: bool) {
        self.audio_stop.store(true, Ordering::Relaxed);
        self.audio_running = false;
        self.set_arrangement_playback_enabled(false);
        if let Ok(mut runtime) = self.performance_runtime.lock() {
            runtime.iter_mut().for_each(|slot| *slot = None);
        }
        self.midi_conns.clear();
        let _stream = self.audio_stream.take();
        let _input = self.audio_input_stream.take();
        let start = std::time::Instant::now();
        while self.audio_callback_active.load(Ordering::Relaxed) > 0 {
            if start.elapsed() > std::time::Duration::from_millis(1000) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        if reset_transport {
            self.send_midi_stop_to_hosts();
        }
        // Keep the host alive on Stop; dropping here can crash some plugins.
        self.midi_gate.store(false, Ordering::Relaxed);
        if reset_transport {
            self.transport_samples.store(0, Ordering::Relaxed);
        }
        for state in &self.track_audio {
            if let Ok(mut events) = state.midi_events.lock() {
                events.clear();
            }
        }
        if reset_transport {
            self.playhead_beats = 0.0;
            self.last_frame_time = None;
        }
    }

    pub(crate) fn send_midi_stop_to_hosts(&mut self) {
        let channels = self.last_output_channels.max(1);
        let mut buffer = vec![0.0f32; channels];
        let mut events = Vec::with_capacity(16 * 128);
        for channel in 0u8..16 {
            for note in 0u8..=127 {
                events.push(vst3::MidiEvent::note_off_at(channel, note, 0, 0));
            }
        }
        for state in &self.track_audio {
            let Some(host) = state.host.as_ref() else {
                continue;
            };
            let _ = host.process_f32(&mut buffer, channels, &events);
        }
    }

    pub(crate) fn warmup_hosts(&mut self, channels: usize, block_size: usize, blocks: usize) {
        if channels == 0 || block_size == 0 || blocks == 0 {
            return;
        }
        let frames = block_size.max(1);
        let mut silence = vec![0.0f32; frames * channels];
        let mut scratch = vec![0.0f32; frames * channels];
        let events: [vst3::MidiEvent; 0] = [];
        for _ in 0..blocks {
            for state in &self.track_audio {
                if let Some(host) = state.host.as_ref() {
                    silence.fill(0.0);
                    let _ = host.process_f32(&mut silence, channels, &events);
                }
                for fx in &state.effect_hosts {
                    silence.fill(0.0);
                    scratch.fill(0.0);
                    let _ = fx.process_f32_with_input(&silence, &mut scratch, channels, &events);
                }
            }
        }
    }

    pub(crate) fn settings_path(&self) -> &str {
        &self.settings_path
    }

    pub(crate) fn save_settings(&mut self) -> Result<(), String> {
        let json = serde_json::to_string_pretty(&self.settings).map_err(|e| e.to_string())?;
        fs::write(self.settings_path(), json).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub(crate) fn load_settings_or_default(&mut self) {
        let path = self.settings_path.to_string();
        let data = fs::read_to_string(&path).ok();
        if let Some(data) = data {
            if let Ok(settings) = serde_json::from_str::<SettingsState>(&data) {
                self.settings = settings;
                self.migrate_legacy_midi_settings();
                return;
            }
        }
        self.settings = SettingsState::default();
        self.migrate_legacy_midi_settings();
    }

    pub(crate) fn migrate_legacy_midi_settings(&mut self) {
        if self.settings.midi_devices.is_empty() && !self.settings.midi_input.trim().is_empty() {
            self.settings.midi_devices.push(MidiDeviceConfig {
                name: "Primary Keyboard".to_string(),
                input_port: self.settings.midi_input.clone(),
                ..MidiDeviceConfig::default()
            });
        }
        if self.settings.midi_input.trim().is_empty() {
            if let Some(device) = self
                .settings
                .midi_devices
                .iter()
                .find(|device| device.enabled && !device.input_port.trim().is_empty())
            {
                self.settings.midi_input = device.input_port.clone();
            }
        }
    }

    pub(crate) fn ensure_device_id(&mut self) {
        if !self.settings.device_id.is_empty() {
            return;
        }
        if self.settings.device_salt.is_empty() {
            self.settings.device_salt = Self::generate_device_salt();
        }
        let fingerprint = Self::device_fingerprint();
        self.settings.device_id = Self::hash_device_id(&fingerprint, &self.settings.device_salt);
        let _ = self.save_settings();
    }

    pub(crate) fn is_registered_user(&self) -> bool {
        self.license_status.starts_with("Registered")
            || !self.settings.registered_to.trim().is_empty()
    }

    pub(crate) fn wallpaper_enabled(&self) -> bool {
        self.is_registered_user() && !self.settings.wallpaper_path.trim().is_empty()
    }

    pub(crate) fn invalidate_wallpaper_texture(&mut self) {
        self.wallpaper_texture = None;
        self.wallpaper_texture_path.clear();
    }

    pub(crate) fn ensure_wallpaper_texture(&mut self, ctx: &egui::Context) -> Result<(), String> {
        if !self.wallpaper_enabled() {
            self.invalidate_wallpaper_texture();
            return Ok(());
        }
        if self.wallpaper_texture.is_some()
            && self.wallpaper_texture_path == self.settings.wallpaper_path
        {
            return Ok(());
        }

        let bytes = fs::read(&self.settings.wallpaper_path)
            .map_err(|e| format!("Wallpaper read failed: {e}"))?;
        let image = image::load_from_memory(&bytes)
            .map_err(|e| format!("Wallpaper decode failed: {e}"))?;
        let rgba = image.to_rgba8();
        let (width, height) = image.dimensions();
        let color_image = egui::ColorImage::from_rgba_unmultiplied(
            [width as usize, height as usize],
            rgba.as_raw(),
        );
        self.wallpaper_texture = Some(ctx.load_texture(
            "custom_wallpaper",
            color_image,
            egui::TextureOptions::LINEAR,
        ));
        self.wallpaper_texture_path = self.settings.wallpaper_path.clone();
        Ok(())
    }

    pub(crate) fn paint_wallpaper(&mut self, ctx: &egui::Context) {
        if !self.wallpaper_enabled() {
            self.invalidate_wallpaper_texture();
            return;
        }
        if self.ensure_wallpaper_texture(ctx).is_err() {
            return;
        }
        let Some(texture) = self.wallpaper_texture.as_ref() else {
            return;
        };
        let opacity = (self.settings.wallpaper_opacity.clamp(0.05, 1.0) * 255.0) as u8;
        let rect = ctx.screen_rect();
        let painter = ctx.layer_painter(egui::LayerId::background());
        painter.image(
            texture.id(),
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::from_white_alpha(opacity),
        );
    }

    pub(crate) fn generate_device_salt() -> String {
        let mut bytes = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        BASE64_URL_SAFE.encode(bytes)
    }

    pub(crate) fn device_fingerprint() -> String {
        let host = std::env::var("COMPUTERNAME").unwrap_or_default();
        let user = std::env::var("USERNAME").unwrap_or_default();
        let cpu = std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_default();
        format!("{host}|{user}|{cpu}")
    }

    pub(crate) fn hash_device_id(fingerprint: &str, salt: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(fingerprint.as_bytes());
        hasher.update(b"|");
        hasher.update(salt.as_bytes());
        let digest = hasher.finalize();
        digest.iter().map(|b| format!("{:02x}", b)).collect()
    }

    pub(crate) fn poll_license_job(&mut self) {
        let Some(job) = self.license_job.as_ref() else {
            return;
        };
        if !job.finished.load(Ordering::Relaxed) {
            return;
        }
        let mut result_opt = None;
        if let Ok(mut guard) = job.result.lock() {
            result_opt = guard.take();
        }
        if let Some(result) = result_opt {
            let mut status_ok = false;
            match result.status {
                Ok(message) => {
                    self.status = message;
                    status_ok = true;
                }
                Err(message) => self.status = message,
            }
            if let Some(token) = result.token {
                self.settings.auth_token = token;
                let _ = self.save_settings();
            }
            if let Some(license_file) = result.license_file {
                self.settings.license_file = license_file;
                let _ = self.save_settings();
                self.refresh_license_status();
            }
            if let Some(registered_to) = result.registered_to {
                self.settings.registered_to = registered_to;
                let _ = self.save_settings();
            }
            if let Some(remaining) = result.remaining_activations {
                self.settings.license_remaining_activations = Some(remaining);
                let _ = self.save_settings();
                if status_ok {
                    self.status = format!(
                        "{} You have {remaining} activations remaining this month.",
                        self.status
                    );
                }
            }
        }
        self.license_job = None;
    }

    pub(crate) fn refresh_license_status(&mut self) {
        if self.settings.license_file.trim().is_empty() {
            self.license_status = "Unregistered".to_string();
            self.settings.license_monthly_activations = None;
            self.settings.license_remaining_activations = None;
            let _ = self.save_settings();
            return;
        }
        match Self::verify_license_file(&self.settings.license_file) {
            Ok(info) => {
                let mut status = "Registered".to_string();
                if let Some(license_type) = info.license_type {
                    status = format!("Registered ({license_type})");
                }
                self.license_status = status;
                self.settings.license_monthly_activations = info.monthly_activations;
                let _ = self.save_settings();
                if let Some(name) = info.registered_to {
                    self.settings.registered_to = name;
                    let _ = self.save_settings();
                }
            }
            Err(err) => {
                self.license_status = format!("License error: {err}");
            }
        }
    }

    pub(crate) fn start_license_job<F>(&mut self, job: F)
    where
        F: FnOnce() -> LicenseJobResult + Send + 'static,
    {
        if self.license_job.is_some() {
            self.status = "License request already running".to_string();
            return;
        }
        let finished = Arc::new(AtomicBool::new(false));
        let result = Arc::new(Mutex::new(None));
        let finished_clone = finished.clone();
        let result_clone = result.clone();
        std::thread::spawn(move || {
            let outcome = job();
            if let Ok(mut guard) = result_clone.lock() {
                *guard = Some(outcome);
            }
            finished_clone.store(true, Ordering::Relaxed);
        });
        self.license_job = Some(LicenseJob { finished, result });
    }

    pub(crate) fn verify_license_file(file_data: &str) -> Result<LicensePayloadInfo, String> {
        let value: serde_json::Value =
            serde_json::from_str(file_data).map_err(|e| format!("License JSON error: {e}"))?;
        let payload = value
            .get("payload")
            .ok_or_else(|| "License missing payload".to_string())?;
        let signature = value
            .get("signature")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "License missing signature".to_string())?;
        let canonical = Self::canonical_json(payload);
        let sig_bytes = BASE64_URL_SAFE
            .decode(signature.as_bytes())
            .map_err(|e| format!("Signature decode error: {e}"))?;
        let key_bytes = Self::decode_public_key(LICENSE_PUBLIC_KEY_B64)?;
        let verify_key = VerifyingKey::from_bytes(&key_bytes)
            .map_err(|e| format!("Public key error: {e}"))?;
        let signature = Signature::from_slice(&sig_bytes)
            .map_err(|e| format!("Signature error: {e}"))?;
        verify_key
            .verify(canonical.as_bytes(), &signature)
            .map_err(|_| "License signature invalid".to_string())?;

        let status = Self::get_value_string(payload, &["status"])
            .ok_or_else(|| "License missing status".to_string())?;
        if status.to_ascii_lowercase() != "active" {
            return Err(format!("License inactive ({status})"));
        }
        let product = Self::get_value_string(payload, &["product", "code"])
            .or_else(|| Self::get_value_string(payload, &["product_code"]));
        if let Some(product) = product.as_ref() {
            if product != LICENSE_PRODUCT_CODE {
                return Err(format!("Product mismatch ({product})"));
            }
        }

        Ok(LicensePayloadInfo {
            registered_to: Self::get_value_string(payload, &["account", "username"])
                .or_else(|| Self::get_value_string(payload, &["account", "email"]))
                .or_else(|| Self::get_value_string(payload, &["user", "username"]))
                .or_else(|| Self::get_value_string(payload, &["user", "email"]))
                .or_else(|| Self::get_value_string(payload, &["registered_to"])),
            license_type: Self::get_value_string(payload, &["license_type"]),
            max_activations: Self::get_value_u64(payload, &["limits", "max_activations"]),
            monthly_activations: Self::get_value_u64(payload, &["limits", "monthly_activations"]),
        })
    }

    pub(crate) fn decode_public_key(key_text: &str) -> Result<[u8; 32], String> {
        let cleaned = key_text
            .replace("-----BEGIN PUBLIC KEY-----", "")
            .replace("-----END PUBLIC KEY-----", "")
            .replace('\n', "")
            .replace('\r', "")
            .trim()
            .to_string();
        let raw = BASE64_URL_SAFE
            .decode(cleaned.as_bytes())
            .map_err(|e| format!("Public key decode error: {e}"))?;
        if raw.len() == 32 {
            let mut bytes = [0u8; 32];
            bytes.copy_from_slice(&raw);
            return Ok(bytes);
        }
        // Accept Ed25519 SubjectPublicKeyInfo DER (44 bytes).
        let spki_prefix: [u8; 12] = [
            0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
        ];
        if raw.len() == 44 && raw.starts_with(&spki_prefix) {
            let mut bytes = [0u8; 32];
            bytes.copy_from_slice(&raw[12..]);
            return Ok(bytes);
        }
        // Accept Ed25519 PKCS#8 private key DER (48 bytes) and derive the public key.
        let pkcs8_prefix: [u8; 16] = [
            0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06,
            0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
        ];
        if raw.len() == 48 && raw.starts_with(&pkcs8_prefix) {
            let mut seed = [0u8; 32];
            seed.copy_from_slice(&raw[16..]);
            let signing_key = SigningKey::from_bytes(&seed);
            return Ok(signing_key.verifying_key().to_bytes());
        }
        Err("Public key must be 32 bytes".to_string())
    }

    pub(crate) fn canonical_json(value: &serde_json::Value) -> String {
        match value {
            serde_json::Value::Null => "null".to_string(),
            serde_json::Value::Bool(v) => v.to_string(),
            serde_json::Value::Number(v) => v.to_string(),
            serde_json::Value::String(v) => serde_json::to_string(v).unwrap_or_default(),
            serde_json::Value::Array(items) => {
                let parts: Vec<String> = items.iter().map(Self::canonical_json).collect();
                format!("[{}]", parts.join(","))
            }
            serde_json::Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                let mut parts = Vec::with_capacity(keys.len());
                for key in keys {
                    let key_json = serde_json::to_string(key).unwrap_or_default();
                    let value_json = Self::canonical_json(&map[key]);
                    parts.push(format!("{key_json}:{value_json}"));
                }
                format!("{{{}}}", parts.join(","))
            }
        }
    }

    pub(crate) fn get_value_string(value: &serde_json::Value, path: &[&str]) -> Option<String> {
        let mut current = value;
        for key in path {
            current = current.get(*key)?;
        }
        current.as_str().map(|s| s.to_string())
    }

    pub(crate) fn get_value_u64(value: &serde_json::Value, path: &[&str]) -> Option<u64> {
        let mut current = value;
        for key in path {
            current = current.get(*key)?;
        }
        current.as_u64()
    }

    pub(crate) fn license_api_url(path: &str) -> String {
        format!("{LICENSE_API_BASE}{path}")
    }

    pub(crate) fn build_license_client() -> Result<Client, String> {
        Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| format!("HTTP client error: {e}"))
    }

    pub(crate) fn start_license_login(&mut self) {
        let identifier = self.license_identifier.trim().to_string();
        let password = self.license_password.clone();
        let label = self.license_device_label.trim().to_string();
        if identifier.is_empty() || password.is_empty() {
            self.status = "Enter identifier + password".to_string();
            return;
        }
        self.start_license_job(move || {
            let client = match Self::build_license_client() {
                Ok(client) => client,
                Err(err) => {
                    return LicenseJobResult {
                        status: Err(err),
                        token: None,
                        license_file: None,
                        registered_to: None,
                        remaining_activations: None,
                    };
                }
            };
            let url = Self::license_api_url("/api/auth/token");
            let response = client
                .post(url)
                .json(&serde_json::json!({
                    "identifier": identifier,
                    "password": password,
                    "label": label,
                }))
                .send();
            let response = match response {
                Ok(response) => response,
                Err(err) => {
                    return LicenseJobResult {
                        status: Err(format!("Login failed: {err}")),
                        token: None,
                        license_file: None,
                        registered_to: None,
                        remaining_activations: None,
                    };
                }
            };
            let status_code = response.status();
            let text = response.text().unwrap_or_default();
            if !status_code.is_success() {
                return LicenseJobResult {
                    status: Err(format!("Login failed ({status_code}): {text}")),
                    token: None,
                    license_file: None,
                    registered_to: None,
                    remaining_activations: None,
                };
            }
            let token = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| v.get("access_token").and_then(|t| t.as_str()).map(|t| t.to_string()));
            if let Some(token) = token {
                LicenseJobResult {
                    status: Ok("Login OK".to_string()),
                    token: Some(token),
                    license_file: None,
                    registered_to: None,
                    remaining_activations: None,
                }
            } else {
                LicenseJobResult {
                    status: Err("Login response missing access_token".to_string()),
                    token: None,
                    license_file: None,
                    registered_to: None,
                    remaining_activations: None,
                }
            }
        });
    }

    pub(crate) fn start_license_claim(&mut self) {
        let serial = self.license_serial.trim().to_string();
        let token = self.settings.auth_token.clone();
        if serial.is_empty() {
            self.status = "Enter serial".to_string();
            return;
        }
        self.start_license_job(move || {
            let client = match Self::build_license_client() {
                Ok(client) => client,
                Err(err) => {
                    return LicenseJobResult {
                        status: Err(err),
                        token: None,
                        license_file: None,
                        registered_to: None,
                        remaining_activations: None,
                    };
                }
            };
            let url = Self::license_api_url("/api/licenses/claim");
            let mut request = client.post(url).json(&serde_json::json!({ "serial": serial }));
            if !token.is_empty() {
                request = request.bearer_auth(token);
            }
            let response = match request.send() {
                Ok(response) => response,
                Err(err) => {
                    return LicenseJobResult {
                        status: Err(format!("Claim failed: {err}")),
                        token: None,
                        license_file: None,
                        registered_to: None,
                        remaining_activations: None,
                    };
                }
            };
            let status_code = response.status();
            let text = response.text().unwrap_or_default();
            if !status_code.is_success() {
                return LicenseJobResult {
                    status: Err(format!("Claim failed ({status_code}): {text}")),
                    token: None,
                    license_file: None,
                    registered_to: None,
                    remaining_activations: None,
                };
            }
            let license_file = Self::license_file_from_response(&text);
            let registered_to = license_file
                .as_deref()
                .and_then(|data| Self::verify_license_file(data).ok())
                .and_then(|info| info.registered_to);
            LicenseJobResult {
                status: Ok("License claimed".to_string()),
                token: None,
                license_file,
                registered_to,
                remaining_activations: None,
            }
        });
    }

    pub(crate) fn start_license_activate(&mut self) {
        let serial = self.license_serial.trim().to_string();
        let device_id = self.settings.device_id.clone();
        let device_label = self.license_device_label.trim().to_string();
        let token = self.settings.auth_token.clone();
        if serial.is_empty() {
            self.status = "Enter serial".to_string();
            return;
        }
        if device_id.is_empty() {
            self.status = "Device id missing".to_string();
            return;
        }
        self.start_license_job(move || {
            let client = match Self::build_license_client() {
                Ok(client) => client,
                Err(err) => {
                    return LicenseJobResult {
                        status: Err(err),
                        token: None,
                        license_file: None,
                        registered_to: None,
                        remaining_activations: None,
                    };
                }
            };
            let url = Self::license_api_url("/api/licenses/activate");
            let mut request = client.post(url).json(&serde_json::json!({
                "serial": serial,
                "device_id": device_id,
                "device_label": device_label,
            }));
            if !token.is_empty() {
                request = request.bearer_auth(token);
            }
            let response = match request.send() {
                Ok(response) => response,
                Err(err) => {
                    return LicenseJobResult {
                        status: Err(format!("Activate failed: {err}")),
                        token: None,
                        license_file: None,
                        registered_to: None,
                        remaining_activations: None,
                    };
                }
            };
            let status_code = response.status();
            let text = response.text().unwrap_or_default();
            if !status_code.is_success() {
                return LicenseJobResult {
                    status: Err(format!("Activate failed ({status_code}): {text}")),
                    token: None,
                    license_file: None,
                    registered_to: None,
                    remaining_activations: None,
                };
            }
            let license_file = Self::license_file_from_response(&text);
            let registered_to = license_file
                .as_deref()
                .and_then(|data| Self::verify_license_file(data).ok())
                .and_then(|info| info.registered_to);
            let remaining_activations = Self::license_remaining_from_response(&text);
            LicenseJobResult {
                status: Ok("Device activated".to_string()),
                token: None,
                license_file,
                registered_to,
                remaining_activations,
            }
        });
    }

    pub(crate) fn start_license_fetch_file(&mut self) {
        let serial = self.license_serial.trim().to_string();
        let token = self.settings.auth_token.clone();
        if serial.is_empty() {
            self.status = "Enter serial".to_string();
            return;
        }
        self.start_license_job(move || {
            let client = match Self::build_license_client() {
                Ok(client) => client,
                Err(err) => {
                    return LicenseJobResult {
                        status: Err(err),
                        token: None,
                        license_file: None,
                        registered_to: None,
                        remaining_activations: None,
                    };
                }
            };
            let url = Self::license_api_url(&format!("/api/licenses/{serial}/file"));
            let mut request = client.get(url);
            if !token.is_empty() {
                request = request.bearer_auth(token);
            }
            let response = match request.send() {
                Ok(response) => response,
                Err(err) => {
                    return LicenseJobResult {
                        status: Err(format!("Download failed: {err}")),
                        token: None,
                        license_file: None,
                        registered_to: None,
                        remaining_activations: None,
                    };
                }
            };
            let status_code = response.status();
            let text = response.text().unwrap_or_default();
            if !status_code.is_success() {
                return LicenseJobResult {
                    status: Err(format!("Download failed ({status_code}): {text}")),
                    token: None,
                    license_file: None,
                    registered_to: None,
                    remaining_activations: None,
                };
            }
            let license_file = Self::license_file_from_response(&text);
            let registered_to = license_file
                .as_deref()
                .and_then(|data| Self::verify_license_file(data).ok())
                .and_then(|info| info.registered_to);
            LicenseJobResult {
                status: Ok("License file downloaded".to_string()),
                token: None,
                license_file,
                registered_to,
                remaining_activations: None,
            }
        });
    }

    pub(crate) fn license_file_from_response(body: &str) -> Option<String> {
        let value = serde_json::from_str::<serde_json::Value>(body).ok()?;
        if let Some(license) = value.get("license_file") {
            if let Some(text) = license.as_str() {
                return Some(text.to_string());
            }
            if license.get("payload").is_some() && license.get("signature").is_some() {
                return serde_json::to_string(license).ok();
            }
        }
        if value.get("payload").is_some() && value.get("signature").is_some() {
            return serde_json::to_string(&value).ok();
        }
        None
    }

    pub(crate) fn license_remaining_from_response(body: &str) -> Option<u64> {
        let value = serde_json::from_str::<serde_json::Value>(body).ok()?;
        Self::get_value_u64(&value, &["remaining_activations"])
            .or_else(|| Self::get_value_u64(&value, &["limits", "remaining_activations"]))
    }

    pub(crate) fn draw_animated_title(&self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let text = "Lingstation";
        let font = egui::FontId::proportional(28.0);
        let color = egui::Color32::from_rgb(220, 240, 255);
        let registered = self.license_status.starts_with("Registered")
            || !self.settings.registered_to.trim().is_empty();
        let rainbow = [
            egui::Color32::from_rgb(255, 48, 48),
            egui::Color32::from_rgb(255, 144, 48),
            egui::Color32::from_rgb(255, 224, 64),
            egui::Color32::from_rgb(72, 220, 120),
            egui::Color32::from_rgb(72, 200, 255),
            egui::Color32::from_rgb(88, 120, 255),
            egui::Color32::from_rgb(176, 96, 255),
        ];
        let time = ctx.input(|i| i.time) as f32;
        let color_offset = (time * 2.4) as usize;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 44.0), egui::Sense::hover());
        let painter = ui.painter();
        let mut cursor_x = rect.left();
        for (index, ch) in text.chars().enumerate() {
            let wobble = (time * 2.4 + index as f32 * 0.5).sin() * 2.0;
            let pos = egui::pos2(cursor_x, rect.center().y + wobble);
            let paint_color = if registered {
                let idx = (index + color_offset) % rainbow.len();
                rainbow[idx]
            } else {
                color
            };
            painter.text(pos, egui::Align2::LEFT_CENTER, ch, font.clone(), paint_color);
            let galley = ui.fonts(|f| f.layout_no_wrap(ch.to_string(), font.clone(), paint_color));
            cursor_x += galley.size().x + 1.0;
        }
    }

    pub(crate) fn shortcuts_by_category() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
        vec![
            (
                "Transport",
                vec![
                    ("Space", "Play/Pause"),
                    ("R", "Record"),
                ],
            ),
            (
                "File",
                vec![
                    ("Ctrl+S", "Save"),
                    ("Ctrl+Shift+S", "Save As"),
                    ("Ctrl+O", "Open"),
                    ("Ctrl+N", "New Project"),
                    ("Ctrl+E", "Render"),
                ],
            ),
            (
                "Edit",
                vec![
                    ("Ctrl+Z", "Undo"),
                    ("Ctrl+Y", "Redo"),
                    ("Delete/Backspace", "Delete selected clip"),
                ],
            ),
            (
                "View",
                vec![("Ctrl+,", "Settings")],
            ),
            (
                "Track",
                vec![("Ctrl+D", "Duplicate selected track")],
            ),
        ]
    }

    pub(crate) fn bundled_synths() -> Vec<(&'static str, &'static str)> {
        vec![
            ("CatSynth", "KVR info pending"),
            ("DogSynth", "KVR info pending"),
            ("FishSynth", "KVR info pending"),
            ("LingSynth", "KVR info pending"),
            ("MiceSynth", "KVR info pending"),
            ("PlantSynth", "KVR info pending"),
            ("SannySynth", "KVR info pending"),
        ]
    }

    pub(crate) fn list_output_devices(&self) -> Vec<String> {
        let host = cpal::default_host();
        let mut names = Vec::new();
        if let Ok(devices) = host.output_devices() {
            for dev in devices {
                if let Ok(name) = dev.name() {
                    names.push(name);
                }
            }
        }
        if names.is_empty() {
            names.push("Default".to_string());
        }
        names
    }

    pub(crate) fn list_input_devices(&self) -> Vec<String> {
        let host = cpal::default_host();
        let mut names = Vec::new();
        if let Ok(devices) = host.input_devices() {
            for dev in devices {
                if let Ok(name) = dev.name() {
                    names.push(name);
                }
            }
        }
        if names.is_empty() {
            names.push("Default".to_string());
        }
        names
    }

    pub(crate) fn list_midi_inputs(&self) -> Vec<String> {
        let mut names = Vec::new();
        if let Ok(midi_in) = MidiInput::new("LingStation") {
            for port in midi_in.ports() {
                if let Ok(name) = midi_in.port_name(&port) {
                    names.push(name);
                }
            }
        }
        if names.is_empty() {
            names.push("Default".to_string());
        }
        names
    }

    pub(crate) fn list_midi_outputs(&self) -> Vec<String> {
        let mut names = Vec::new();
        if let Ok(midi_out) = MidiOutput::new("LingStation") {
            for port in midi_out.ports() {
                if let Ok(name) = midi_out.port_name(&port) {
                    names.push(name);
                }
            }
        }
        if names.is_empty() {
            names.push("None".to_string());
        }
        names
    }

    pub(crate) fn active_midi_input_devices(&self) -> Vec<MidiDeviceConfig> {
        let devices: Vec<MidiDeviceConfig> = self
            .settings
            .midi_devices
            .iter()
            .filter(|device| device.enabled && !device.input_port.trim().is_empty())
            .cloned()
            .collect();
        if !devices.is_empty() {
            return devices;
        }
        if self.settings.midi_input.trim().is_empty() {
            Vec::new()
        } else {
            vec![MidiDeviceConfig {
                name: "Legacy MIDI Input".to_string(),
                input_port: self.settings.midi_input.clone(),
                ..MidiDeviceConfig::default()
            }]
        }
    }

    pub(crate) fn connect_midi_input_device(
        &self,
        device: &MidiDeviceConfig,
    ) -> Result<MidiInputConnection<()>, String> {
        let mut midi_in = MidiInput::new("LingStation").map_err(|e| e.to_string())?;
        midi_in.ignore(Ignore::None);
        let port = midi_in
            .ports()
            .into_iter()
            .find(|port| midi_in.port_name(port).ok().as_deref() == Some(device.input_port.as_str()))
            .ok_or_else(|| format!("port not found: {}", device.input_port))?;

        let freq_bits = self.midi_freq_bits.clone();
        let gate = self.midi_gate.clone();
        let track_audio = self.track_audio.clone();
        let selected_track_index = self.selected_track_index.clone();
        let midi_learn = self.midi_learn.clone();
        let recording = self.recording.clone();
        let tempo_bits = self.tempo_bits.clone();
        let transport_samples = self.transport_samples.clone();
        let record_sample_rate = self.settings.sample_rate.max(1) as f32;
        let channel_filter = device.midi_channel;

        midi_in
            .connect(
                &port,
                "lingstation-midi",
                move |_stamp, message, _| {
                    if message.len() < 3 {
                        return;
                    }
                    let status = message[0] & 0xF0;
                    let channel = message[0] & 0x0F;
                    if channel_filter > 0 && channel + 1 != channel_filter {
                        return;
                    }
                    let note = message[1];
                    let vel = message[2];
                    let index = selected_track_index.load(Ordering::Relaxed);
                    let state = if index == usize::MAX {
                        None
                    } else {
                        track_audio.get(index)
                    };
                    let bpm = f32::from_bits(tempo_bits.load(Ordering::Relaxed)).max(1.0);
                    let samples = transport_samples.load(Ordering::Relaxed) as f32;
                    let beat = (samples / record_sample_rate) * (bpm / 60.0);
                    if status == 0x90 && vel > 0 {
                        let freq = 440.0f32 * 2.0f32.powf((note as f32 - 69.0) / 12.0);
                        freq_bits.store(freq.to_bits(), Ordering::Relaxed);
                        gate.store(true, Ordering::Relaxed);
                        if let Some(state) = state {
                            if let Ok(mut events) = state.midi_events.lock() {
                                events.push(vst3::MidiEvent::note_on(channel, note, vel));
                            }
                        }
                        if let Ok(mut rec) = recording.lock() {
                            if rec.active && rec.record_midi {
                                rec.midi_active.insert(note, (beat, vel));
                            }
                        }
                    } else if status == 0x80 || (status == 0x90 && vel == 0) {
                        gate.store(false, Ordering::Relaxed);
                        if let Some(state) = state {
                            if let Ok(mut events) = state.midi_events.lock() {
                                events.push(vst3::MidiEvent::note_off(channel, note, vel));
                            }
                        }
                        if let Ok(mut rec) = recording.lock() {
                            if rec.active && rec.record_midi {
                                if let Some((start, start_vel)) = rec.midi_active.remove(&note) {
                                    let length = (beat - start).max(0.05);
                                    let velocity = if start_vel > 0 { start_vel } else { vel };
                                    rec.midi_notes.push(PianoRollNote::new(start, length, note, velocity));
                                }
                            }
                        }
                    } else if status == 0xB0 {
                        if let Ok(mut learn) = midi_learn.lock() {
                            if let Some((learn_index, param_id)) = *learn {
                                if learn_index == index {
                                    if let Some(state) = track_audio.get(learn_index) {
                                        if let Ok(mut map) = state.learned_cc.lock() {
                                            map.insert((channel, note), param_id);
                                        }
                                    }
                                    *learn = None;
                                    return;
                                }
                            }
                        }
                        if let Some(state) = state {
                            if let Ok(mut events) = state.midi_events.lock() {
                                events.push(vst3::MidiEvent::control_change(channel, note, vel));
                            }
                            if let Ok(map) = state.learned_cc.lock() {
                                if let Some(param_id) = map.get(&(channel, note)).copied() {
                                    if let Ok(mut rec) = recording.lock() {
                                        if rec.active && rec.record_automation {
                                            let value = (vel as f32 / 127.0).clamp(0.0, 1.0);
                                            rec.automation_points.push(RecordedAutomationPoint {
                                                param_id,
                                                target: AutomationTarget::Instrument,
                                                beat,
                                                value,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
                (),
            )
            .map_err(|e| e.to_string())
    }

    pub(crate) fn reconnect_midi_inputs(&mut self) -> Result<(), String> {
        self.midi_conns.clear();
        let devices = self.active_midi_input_devices();
        if devices.is_empty() {
            return Err("No MIDI input devices configured".to_string());
        }

        let mut connected = 0usize;
        let mut failures = Vec::new();
        for device in devices {
            match self.connect_midi_input_device(&device) {
                Ok(conn) => {
                    self.midi_conns.push(conn);
                    connected += 1;
                }
                Err(err) => failures.push(format!("{} ({err})", device.display_name())),
            }
        }

        if connected == 0 {
            Err(failures.join("; "))
        } else {
            if let Some(device) = self
                .settings
                .midi_devices
                .iter()
                .find(|device| device.enabled && !device.input_port.trim().is_empty())
            {
                self.settings.midi_input = device.input_port.clone();
            }
            if !failures.is_empty() {
                self.status = format!(
                    "Connected {connected} MIDI input(s); skipped {}",
                    failures.join(", ")
                );
            }
            Ok(())
        }
    }

    pub(crate) fn render_devices_settings(&mut self, ui: &mut egui::Ui) {
        ui.heading("Devices");
        ui.separator();
        ui.label("Assign controller profiles, input/output ports, and channel filters for Launchpad, APC, keyboards, and other MIDI hardware.");

        ui.horizontal(|ui| {
            if ui.button("Add Keyboard").clicked() {
                self.settings.midi_devices.push(MidiDeviceConfig {
                    name: format!("Keyboard {}", self.settings.midi_devices.len() + 1),
                    profile: MidiDeviceProfile::Keyboard,
                    ..MidiDeviceConfig::default()
                });
            }
            if ui.button("Add Launchpad").clicked() {
                self.settings.midi_devices.push(MidiDeviceConfig {
                    name: format!("Launchpad {}", self.settings.midi_devices.len() + 1),
                    profile: MidiDeviceProfile::Launchpad,
                    ..MidiDeviceConfig::default()
                });
            }
            if ui.button("Add APC").clicked() {
                self.settings.midi_devices.push(MidiDeviceConfig {
                    name: format!("APC {}", self.settings.midi_devices.len() + 1),
                    profile: MidiDeviceProfile::Apc,
                    ..MidiDeviceConfig::default()
                });
            }
            if ui.button("Add Generic").clicked() {
                self.settings.midi_devices.push(MidiDeviceConfig {
                    name: format!("Controller {}", self.settings.midi_devices.len() + 1),
                    profile: MidiDeviceProfile::Generic,
                    ..MidiDeviceConfig::default()
                });
            }
        });

        let midi_inputs = self.list_midi_inputs();
        let midi_outputs = self.list_midi_outputs();
        let mut remove_index = None;

        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
            if self.settings.midi_devices.is_empty() {
                ui.label("No controller devices configured yet.");
            }
            for (index, device) in self.settings.midi_devices.iter_mut().enumerate() {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.heading(device.display_name());
                        ui.checkbox(&mut device.enabled, "Enabled");
                        if ui.button("Remove").clicked() {
                            remove_index = Some(index);
                        }
                    });

                    ui.horizontal(|ui| {
                        ui.label("Name");
                        ui.text_edit_singleline(&mut device.name);
                    });

                    egui::ComboBox::from_id_source(("device_profile", index))
                        .selected_text(device.profile.label())
                        .show_ui(ui, |ui| {
                            for profile in [
                                MidiDeviceProfile::Keyboard,
                                MidiDeviceProfile::Launchpad,
                                MidiDeviceProfile::Apc,
                                MidiDeviceProfile::PadController,
                                MidiDeviceProfile::ControlSurface,
                                MidiDeviceProfile::Generic,
                            ] {
                                ui.selectable_value(&mut device.profile, profile, profile.label());
                            }
                        });

                    egui::ComboBox::from_id_source(("device_input", index))
                        .selected_text(if device.input_port.trim().is_empty() {
                            "None".to_string()
                        } else {
                            device.input_port.clone()
                        })
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(device.input_port.trim().is_empty(), "None")
                                .clicked()
                            {
                                device.input_port.clear();
                            }
                            for name in &midi_inputs {
                                if ui.selectable_label(device.input_port == *name, name).clicked() {
                                    device.input_port = name.clone();
                                }
                            }
                        });

                    egui::ComboBox::from_id_source(("device_output", index))
                        .selected_text(if device.output_port.trim().is_empty() {
                            "None".to_string()
                        } else {
                            device.output_port.clone()
                        })
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(device.output_port.trim().is_empty(), "None")
                                .clicked()
                            {
                                device.output_port.clear();
                            }
                            for name in &midi_outputs {
                                if ui.selectable_label(device.output_port == *name, name).clicked() {
                                    device.output_port = name.clone();
                                }
                            }
                        });

                    egui::ComboBox::from_id_source(("device_channel", index))
                        .selected_text(if device.midi_channel == 0 {
                            "Any MIDI Channel".to_string()
                        } else {
                            format!("Channel {}", device.midi_channel)
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut device.midi_channel, 0, "Any MIDI Channel");
                            for channel in 1..=16 {
                                ui.selectable_value(
                                    &mut device.midi_channel,
                                    channel,
                                    format!("Channel {}", channel),
                                );
                            }
                        });
                });
                ui.add_space(6.0);
            }
        });

        if let Some(index) = remove_index {
            self.settings.midi_devices.remove(index);
        }

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Reconnect MIDI Now").clicked() {
                match self.reconnect_midi_inputs() {
                    Ok(()) => {
                        if self.status.is_empty() {
                            self.status = "MIDI inputs reconnected".to_string();
                        }
                    }
                    Err(err) => self.status = format!("MIDI reconnect failed: {err}"),
                }
            }
            ui.label("Enabled input ports open together. Output ports are saved for controller feedback routing.");
        });
    }

    pub(crate) fn pick_vst_file(&self) -> Option<String> {
        let path = rfd::FileDialog::new()
            .add_filter("Plugins", &["vst3", "clap"])
            .pick_file();
        path.map(|p| p.to_string_lossy().to_string())
    }

    pub(crate) fn open_plugin_picker(&mut self, target: PluginTarget) {
        self.plugin_target = Some(target);
        if self.plugin_candidates.is_empty() {
            self.plugin_candidates = self.scan_plugins();
        }
        self.show_plugin_picker = true;
    }

    pub(crate) fn scan_plugins(&self) -> Vec<PluginCandidate> {
        let mut native = Vec::new();
        native.push(PluginCandidate {
            path: "native:treesynth".to_string(),
            kind: PluginKind::Native,
            clap_id: None,
            display: "TreeSynth (Sampler)".to_string(),
            category: PluginCategory::Native,
            instrument_only: true,
        });

        let mut bundled = Vec::new();
        let mut system = Vec::new();

        let mut vst3_paths = Vec::new();
        for root in self.vst3_search_roots() {
            self.scan_dir_for_exts(&root, &mut vst3_paths, &["vst3"]);
        }
        vst3_paths.sort();
        for path in vst3_paths {
            let display = Self::plugin_display_name(&path);
            let category = Self::categorize_plugin_path(&path);
            let target = if category == PluginCategory::Bundled {
                &mut bundled
            } else {
                &mut system
            };
            target.push(PluginCandidate {
                path,
                kind: PluginKind::Vst3,
                clap_id: None,
                display,
                category,
                instrument_only: false,
            });
        }

        let mut clap_paths = Vec::new();
        for root in self.clap_search_roots() {
            self.scan_dir_for_exts(&root, &mut clap_paths, &["clap"]);
        }
        clap_paths.sort();
        for path in clap_paths {
            let category = Self::categorize_plugin_path(&path);
            let target = if category == PluginCategory::Bundled {
                &mut bundled
            } else {
                &mut system
            };
            match clap_host::enumerate_plugins(&path) {
                Ok(descriptors) if !descriptors.is_empty() => {
                    for desc in descriptors {
                        let display = format!("{} (CLAP)", desc.name);
                        target.push(PluginCandidate {
                            path: path.clone(),
                            kind: PluginKind::Clap,
                            clap_id: Some(desc.id),
                            display,
                            category,
                            instrument_only: false,
                        });
                    }
                }
                _ => {
                    let display = format!("{} (CLAP)", Self::plugin_display_name(&path));
                    target.push(PluginCandidate {
                        path: path.clone(),
                        kind: PluginKind::Clap,
                        clap_id: None,
                        display,
                        category,
                        instrument_only: false,
                    });
                }
            }
        }

        let sort_by_name = |a: &PluginCandidate, b: &PluginCandidate| {
            a.display.to_ascii_lowercase().cmp(&b.display.to_ascii_lowercase())
        };
        bundled.sort_by(sort_by_name);
        system.sort_by(sort_by_name);
        native.sort_by(sort_by_name);

        let mut candidates = Vec::new();
        candidates.extend(native);
        candidates.extend(bundled);
        candidates.extend(system);
        candidates
    }

    pub(crate) fn is_native_plugin_path(path: &str) -> bool {
        path.starts_with("native:")
    }

    pub(crate) fn is_treesynth_path(path: &str) -> bool {
        path.eq_ignore_ascii_case("native:treesynth")
    }

    pub(crate) fn is_bundled_plugin_path(path: &str) -> bool {
        let lower = path.to_ascii_lowercase();
        if lower.starts_with("synths\\") || lower.starts_with("synths/") || lower == "synths" {
            return true;
        }
        lower.contains("\\synths\\") || lower.contains("/synths/")
    }

    pub(crate) fn categorize_plugin_path(path: &str) -> PluginCategory {
        if Self::is_native_plugin_path(path) {
            PluginCategory::Native
        } else if Self::is_bundled_plugin_path(path) {
            PluginCategory::Bundled
        } else {
            PluginCategory::System
        }
    }

    pub(crate) fn plugin_kind_from_path(path: &str) -> PluginKind {
        if Self::is_native_plugin_path(path) {
            return PluginKind::Native;
        }
        let ext = Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ext == "clap" {
            PluginKind::Clap
        } else {
            PluginKind::Vst3
        }
    }

    pub(crate) fn enumerate_plugin_params_for_track(
        &mut self,
        index: usize,
        path: &str,
    ) -> Result<Vec<vst3::ParamInfo>, String> {
        match Self::plugin_kind_from_path(path) {
            PluginKind::Native => Ok(Vec::new()),
            PluginKind::Vst3 => vst3::enumerate_params(path),
            PluginKind::Clap => {
                let clap_id = self
                    .tracks
                    .get(index)
                    .and_then(|t| t.instrument_clap_id.clone())
                    .or_else(|| clap_host::default_plugin_id(path).ok())
                    .ok_or_else(|| "CLAP plugin id not found".to_string())?;
                if let Some(track) = self.tracks.get_mut(index) {
                    track.instrument_clap_id = Some(clap_id.clone());
                }
                let mut host = clap_host::ClapHost::load(
                    path,
                    &clap_id,
                    self.settings.sample_rate as f64,
                    self.settings.buffer_size as u32,
                    0,
                    2,
                )
                .map_err(|e| e.to_string())?;
                let params = host
                    .enumerate_params()
                    .into_iter()
                    .map(|param| vst3::ParamInfo {
                        id: param.id,
                        name: param.name,
                        default_value: param.default_value,
                    })
                    .collect();
                Ok(params)
            }
        }
    }

    pub(crate) fn vst3_search_roots(&self) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        #[cfg(windows)]
        {
            roots.push(PathBuf::from("C:\\Program Files\\Common Files\\VST3"));
        }
        #[cfg(target_os = "macos")]
        {
            roots.push(PathBuf::from("/Library/Audio/Plug-Ins/VST3"));
            if let Some(home) = Self::home_dir() {
                roots.push(home.join("Library/Audio/Plug-Ins/VST3"));
            }
        }
        #[cfg(target_os = "linux")]
        {
            roots.push(PathBuf::from("/usr/lib/vst3"));
            roots.push(PathBuf::from("/usr/local/lib/vst3"));
            if let Some(home) = Self::home_dir() {
                roots.push(home.join(".vst3"));
            }
        }
        roots.push(PathBuf::from("synths"));
        roots
    }

    pub(crate) fn clap_search_roots(&self) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        #[cfg(windows)]
        {
            roots.push(PathBuf::from("C:\\Program Files\\Common Files\\CLAP"));
        }
        #[cfg(target_os = "macos")]
        {
            roots.push(PathBuf::from("/Library/Audio/Plug-Ins/CLAP"));
            if let Some(home) = Self::home_dir() {
                roots.push(home.join("Library/Audio/Plug-Ins/CLAP"));
            }
        }
        #[cfg(target_os = "linux")]
        {
            roots.push(PathBuf::from("/usr/lib/clap"));
            roots.push(PathBuf::from("/usr/local/lib/clap"));
            if let Some(home) = Self::home_dir() {
                roots.push(home.join(".clap"));
            }
        }
        roots.push(PathBuf::from("synths"));
        roots
    }

    pub(crate) fn plugin_display_name(path: &str) -> String {
        if Self::is_treesynth_path(path) {
            return "TreeSynth".to_string();
        }
        let candidate = Path::new(path)
            .file_stem()
            .or_else(|| Path::new(path).file_name())
            .and_then(|s| s.to_str())
            .unwrap_or(path);
        candidate.replace('_', " ")
    }

    pub(crate) fn find_vst3_plugin_by_name(&mut self, name: &str) -> Option<String> {
        if self.plugin_candidates.is_empty() {
            self.plugin_candidates = self.scan_plugins();
        }
        let needle = name.to_ascii_lowercase();
        self.plugin_candidates
            .iter()
            .filter(|candidate| candidate.kind == PluginKind::Vst3)
            .find(|candidate| {
                let display = candidate.display.to_ascii_lowercase();
                display == needle || display.contains(&needle)
            })
            .map(|candidate| candidate.path.clone())
    }

    pub(crate) fn apply_program_param(track: &mut Track) {
        let Some(program) = track.midi_program else {
            return;
        };
        let program_index = track.params.iter().position(|name| {
            let name = name.to_ascii_lowercase();
            name.contains("program") || name.contains("patch") || name.contains("preset")
        });
        if let Some(index) = program_index {
            if let Some(value) = track.param_values.get_mut(index) {
                *value = (program as f32 / 127.0).clamp(0.0, 1.0);
            }
        }
    }

    pub(crate) fn presets_root_global(&self) -> PathBuf {
        let base = Path::new(self.settings_path())
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("assets").join("presets")
    }

    pub(crate) fn presets_root_project(&self) -> Option<PathBuf> {
        let trimmed = self.project_path.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(Self::normalize_windows_path(&PathBuf::from(trimmed)).join("assets").join("presets"))
    }

    pub(crate) fn preset_plugin_dir(&self, root: &Path, plugin_path: &str) -> PathBuf {
        let name = Self::plugin_display_name(plugin_path);
        let safe = Self::sanitize_folder_name(&name);
        root.join(safe)
    }

    pub(crate) fn preset_file_path(
        &self,
        root: &Path,
        plugin_path: &str,
        preset_name: &str,
    ) -> Result<PathBuf, String> {
        let safe = Self::sanitize_folder_name(preset_name);
        if safe.trim().is_empty() {
            return Err("Preset name required".to_string());
        }
        let file_name = format!("{}.lingpreset.json", safe);
        Ok(self.preset_plugin_dir(root, plugin_path).join(file_name))
    }

    pub(crate) fn preset_name_for_program(&self, program: u8) -> String {
        format!("{:03} {}", program + 1, gm_program_name(program))
    }

    pub(crate) fn preset_path_for_program(&self, root: &Path, plugin_path: &str, program: u8) -> PathBuf {
        let name = self.preset_name_for_program(program);
        self.preset_plugin_dir(root, plugin_path)
            .join(format!("{}.lingpreset.json", Self::sanitize_folder_name(&name)))
    }

    pub(crate) fn load_preset_for_program(&mut self, index: usize, program: u8) -> Result<(), String> {
        let plugin_path = self
            .tracks
            .get(index)
            .and_then(|t| t.instrument_path.as_deref())
            .ok_or_else(|| "No instrument loaded".to_string())?
            .to_string();

        if let Some(project_root) = self.presets_root_project() {
            let project_path = self.preset_path_for_program(&project_root, &plugin_path, program);
            if project_path.exists() {
                return self.load_preset_from_path(index, &project_path);
            }
        }

        let global_root = self.presets_root_global();
        let global_path = self.preset_path_for_program(&global_root, &plugin_path, program);
        if global_path.exists() {
            return self.load_preset_from_path(index, &global_path);
        }

        self.ensure_gm_presets_for_plugin(&plugin_path, program)?;

        if let Some(project_root) = self.presets_root_project() {
            let project_path = self.preset_path_for_program(&project_root, &plugin_path, program);
            if project_path.exists() {
                return self.load_preset_from_path(index, &project_path);
            }
        }
        let global_root = self.presets_root_global();
        let global_path = self.preset_path_for_program(&global_root, &plugin_path, program);
        if global_path.exists() {
            return self.load_preset_from_path(index, &global_path);
        }

        Err("Preset file not found".to_string())
    }

    pub(crate) fn save_preset_for_track(
        &mut self,
        index: usize,
        root: PathBuf,
        preset_name: &str,
    ) -> Result<String, String> {
        let track = self
            .tracks
            .get(index)
            .ok_or_else(|| "Track not found".to_string())?;
        let plugin_path = track
            .instrument_path
            .as_deref()
            .ok_or_else(|| "No instrument loaded".to_string())?;
        let preset_path = self.preset_file_path(&root, plugin_path, preset_name)?;
        if let Some(parent) = preset_path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        if Self::is_treesynth_path(plugin_path) {
            return self.save_treesynth_preset_at_path(index, &preset_path, preset_name);
        }

        let (mut component_bytes, mut controller_bytes) = (Vec::new(), Vec::new());
        if let Some(host) = self.track_audio.get(index).and_then(|state| state.host.as_ref()) {
            let (component, controller) = host.get_state_bytes();
            component_bytes = component;
            controller_bytes = controller;
        }
        if component_bytes.is_empty() {
            if let Some(bytes) = track.plugin_state_component.as_ref() {
                component_bytes = bytes.clone();
            }
        }
        if controller_bytes.is_empty() {
            if let Some(bytes) = track.plugin_state_controller.as_ref() {
                controller_bytes = bytes.clone();
            }
        }

        let preset = Vst3PresetFile {
            version: 1,
            name: preset_name.to_string(),
            plugin: Self::plugin_display_name(plugin_path),
            param_names: track.params.clone(),
            param_ids: track.param_ids.clone(),
            param_values: track.param_values.clone(),
            component_state: BASE64.encode(&component_bytes),
            controller_state: BASE64.encode(&controller_bytes),
            treesynth: None,
        };

        let json = serde_json::to_string_pretty(&preset).map_err(|e| e.to_string())?;
        fs::write(&preset_path, json).map_err(|e| e.to_string())?;
        Ok(preset_path.to_string_lossy().to_string())
    }

    pub(crate) fn save_treesynth_preset_at_path(
        &mut self,
        index: usize,
        preset_path: &Path,
        preset_name: &str,
    ) -> Result<String, String> {
        let track = self
            .tracks
            .get(index)
            .ok_or_else(|| "Track not found".to_string())?;
        let plugin_path = track
            .instrument_path
            .as_deref()
            .ok_or_else(|| "No instrument loaded".to_string())?;
        if !Self::is_treesynth_path(plugin_path) {
            return Err("TreeSynth preset requires native TreeSynth".to_string());
        }
        if track.treesynth.is_none() {
            return Err("[TreeSynth] サンプル未ロード: プリセット保存不可".to_string());
        }
        if let Some(parent) = preset_path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut state = track.treesynth.clone().unwrap();
        let samples_dir_name = Self::treesynth_samples_dir_name(preset_name);
        let samples_dir = preset_path
            .parent()
            .ok_or_else(|| "Preset folder missing".to_string())?
            .join(&samples_dir_name);
        fs::create_dir_all(&samples_dir).map_err(|e| e.to_string())?;
        let mut used_names: HashMap<String, usize> = HashMap::new();
        for sample in state.samples.iter_mut() {
            let source = Path::new(&sample.path);
            let file_name = source
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("sample.wav");
            let target_name = Self::treesynth_unique_filename(&mut used_names, file_name);
            let target_path = samples_dir.join(&target_name);
            if !target_path.exists() {
                let _ = fs::copy(source, &target_path);
            }
            sample.path = format!("{}/{}", samples_dir_name, target_name);
            sample.name = target_name.clone();
        }
        state.folder = Some(samples_dir_name);
        let preset = Vst3PresetFile {
            version: 1,
            name: preset_name.to_string(),
            plugin: Self::plugin_display_name(plugin_path),
            param_names: Vec::new(),
            param_ids: Vec::new(),
            param_values: Vec::new(),
            component_state: String::new(),
            controller_state: String::new(),
            treesynth: Some(state),
        };
        let json = serde_json::to_string_pretty(&preset).map_err(|e| e.to_string())?;
        fs::write(preset_path, json).map_err(|e| e.to_string())?;
        Ok(preset_path.to_string_lossy().to_string())
    }

    pub(crate) fn load_preset_from_path(&mut self, index: usize, path: &Path) -> Result<(), String> {
        let plugin_path = self
            .tracks
            .get(index)
            .and_then(|track| track.instrument_path.as_deref())
            .ok_or_else(|| "No instrument loaded".to_string())?
            .to_string();
        let data = fs::read_to_string(path).map_err(|e| e.to_string())?;
        let preset: Vst3PresetFile = serde_json::from_str(&data).map_err(|e| e.to_string())?;
        let expected = Self::plugin_display_name(&plugin_path).to_ascii_lowercase();
        let actual = preset.plugin.to_ascii_lowercase();
        if expected != actual {
            return Err("Preset plugin does not match current instrument".to_string());
        }

        if Self::is_treesynth_path(&plugin_path) {
            let mut state = preset
                .treesynth
                .ok_or_else(|| "TreeSynth preset data missing".to_string())?;
            let preset_dir = path
                .parent()
                .ok_or_else(|| "Preset folder missing".to_string())?;
            if let Some(folder) = state.folder.clone() {
                if Path::new(&folder).is_relative() {
                    state.folder = Some(preset_dir.join(&folder).to_string_lossy().to_string());
                }
            }
            for sample in state.samples.iter_mut() {
                let sample_path = Path::new(&sample.path);
                if sample_path.is_relative() {
                    let resolved = preset_dir.join(sample_path);
                    sample.path = resolved.to_string_lossy().to_string();
                }
            }
            if let Some(track) = self.tracks.get_mut(index) {
                track.treesynth = Some(state);
            }
            if let Some(state) = self.track_audio.get(index) {
                let enabled = Self::is_treesynth_path(&plugin_path);
                if let Some(track) = self.tracks.get(index) {
                    state.sync_treesynth(track, enabled, &self.audio_clip_cache);
                }
                if let Ok(mut runtime) = state.treesynth_runtime.lock() {
                    runtime.voices.clear();
                    runtime.sequence_index = 0;
                }
            }
            self.preload_audio_clips(&self.audio_clip_cache);
            return Ok(());
        }

        let component_bytes = if preset.component_state.trim().is_empty() {
            Vec::new()
        } else {
            BASE64
                .decode(preset.component_state.as_bytes())
                .map_err(|e| e.to_string())?
        };
        let controller_bytes = if preset.controller_state.trim().is_empty() {
            Vec::new()
        } else {
            BASE64
                .decode(preset.controller_state.as_bytes())
                .map_err(|e| e.to_string())?
        };

        if let Some(track) = self.tracks.get_mut(index) {
            if !component_bytes.is_empty() {
                track.plugin_state_component = Some(component_bytes.clone());
            }
            if !controller_bytes.is_empty() {
                track.plugin_state_controller = Some(controller_bytes.clone());
            }

            if !preset.param_ids.is_empty() && !preset.param_values.is_empty() {
                if track.param_ids.is_empty() || track.param_ids.len() != track.param_values.len() {
                    track.param_ids = preset.param_ids.clone();
                    track.param_values = preset.param_values.clone();
                } else {
                    let mut map = HashMap::new();
                    for (id, value) in preset.param_ids.iter().zip(preset.param_values.iter()) {
                        map.insert(*id, *value);
                    }
                    if track.param_values.len() != track.param_ids.len() {
                        track.param_values.resize(track.param_ids.len(), 0.0);
                    }
                    for (slot, param_id) in track.param_ids.iter().enumerate() {
                        if let Some(value) = map.get(param_id).copied() {
                            if let Some(target) = track.param_values.get_mut(slot) {
                                *target = value;
                            }
                        }
                    }
                }
            } else if !preset.param_names.is_empty() && !preset.param_values.is_empty() {
                if track.param_values.len() != track.params.len() {
                    track.param_values.resize(track.params.len(), 0.0);
                }
                let mut map = HashMap::new();
                for (name, value) in preset.param_names.iter().zip(preset.param_values.iter()) {
                    map.insert(Self::normalize_param_name(name), *value);
                }
                for (slot, name) in track.params.iter().enumerate() {
                    let key = Self::normalize_param_name(name);
                    if let Some(value) = map.get(&key).copied() {
                        if let Some(target) = track.param_values.get_mut(slot) {
                            *target = value;
                        }
                    }
                }
            }
        }

        if let Some(host) = self.track_audio.get(index).and_then(|state| state.host.as_ref()) {
            if !component_bytes.is_empty() || !controller_bytes.is_empty() {
                let _ = host.set_state_bytes(
                    if component_bytes.is_empty() {
                        None
                    } else {
                        Some(component_bytes.as_slice())
                    },
                    if controller_bytes.is_empty() {
                        None
                    } else {
                        Some(controller_bytes.as_slice())
                    },
                );
            } else if let Some(track) = self.tracks.get(index) {
                for (param_id, value) in track.param_ids.iter().zip(track.param_values.iter()) {
                    host.push_param_change(*param_id, *value as f64);
                }
            }

            if let Some(track) = self.tracks.get_mut(index) {
                for (slot, param_id) in track.param_ids.iter().enumerate() {
                    if let Some(value) = host.get_param_normalized(*param_id) {
                        if let Some(target) = track.param_values.get_mut(slot) {
                            *target = value as f32;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    pub(crate) fn normalize_param_name(name: &str) -> String {
        name.to_ascii_lowercase()
            .replace(' ', "")
            .replace('_', "")
            .replace('-', "")
    }

    pub(crate) fn debug_param_enabled() -> bool {
        std::env::var("LING_DEBUG_PARAMS")
            .map(|value| {
                let value = value.to_ascii_lowercase();
                value == "1" || value == "true" || value == "yes"
            })
            .unwrap_or(false)
    }

    pub(crate) fn log_fm_ratio_param(&self, track_index: usize, stage: &str) {
        if !Self::debug_param_enabled() {
            return;
        }
        let Some(track) = self.tracks.get(track_index) else {
            return;
        };
        Self::log_fm_ratio_param_from(
            track_index,
            stage,
            &track.params,
            &track.param_ids,
            &track.param_values,
        );
    }

    pub(crate) fn log_fm_ratio_param_from(
        track_index: usize,
        stage: &str,
        params: &[String],
        param_ids: &[u32],
        param_values: &[f32],
    ) {
        if !Self::debug_param_enabled() {
            return;
        }
        let mut found = false;
        for (index, name) in params.iter().enumerate() {
            let key = Self::normalize_param_name(name);
            if key.contains("fmratio") || (key.contains("fm") && key.contains("ratio")) {
                let id = param_ids.get(index).copied().unwrap_or(0);
                let value = param_values.get(index).copied().unwrap_or(0.0);
                eprintln!(
                    "[param-debug] {stage} track={track_index} name={name} id={id} value={value}"
                );
                found = true;
            }
        }
        if !found {
            eprintln!("[param-debug] {stage} track={track_index} fm_ratio not found");
        }
    }

    pub(crate) fn log_all_fm_ratio_params(&self, stage: &str) {
        if !Self::debug_param_enabled() {
            return;
        }
        for index in 0..self.tracks.len() {
            self.log_fm_ratio_param(index, stage);
        }
    }

    pub(crate) fn remap_param_values_by_id_or_name(
        old_ids: &[u32],
        old_names: &[String],
        old_values: &[f32],
        new_params: &[vst3::ParamInfo],
    ) -> Vec<f32> {
        let mut id_map: HashMap<u32, f32> = HashMap::new();
        for (id, value) in old_ids.iter().zip(old_values.iter()) {
            id_map.insert(*id, *value);
        }
        let mut name_map: HashMap<String, f32> = HashMap::new();
        for (name, value) in old_names.iter().zip(old_values.iter()) {
            name_map.insert(Self::normalize_param_name(name), *value);
        }
        new_params
            .iter()
            .map(|p| {
                id_map
                    .get(&p.id)
                    .copied()
                    .or_else(|| name_map.get(&Self::normalize_param_name(&p.name)).copied())
                    .unwrap_or(p.default_value as f32)
            })
            .collect()
    }

    pub(crate) fn remap_param_values_by_id_or_name_clap(
        old_ids: &[u32],
        old_names: &[String],
        old_values: &[f32],
        new_params: &[clap_host::ParamInfo],
    ) -> Vec<f32> {
        let mut id_map: HashMap<u32, f32> = HashMap::new();
        for (id, value) in old_ids.iter().zip(old_values.iter()) {
            id_map.insert(*id, *value);
        }
        let mut name_map: HashMap<String, f32> = HashMap::new();
        for (name, value) in old_names.iter().zip(old_values.iter()) {
            name_map.insert(Self::normalize_param_name(name), *value);
        }
        new_params
            .iter()
            .map(|p| {
                id_map
                    .get(&p.id)
                    .copied()
                    .or_else(|| name_map.get(&Self::normalize_param_name(&p.name)).copied())
                    .unwrap_or(p.default_value as f32)
            })
            .collect()
    }

    pub(crate) fn ensure_gm_presets_for_plugin(
        &mut self,
        plugin_path: &str,
        requested_program: u8,
    ) -> Result<(), String> {
        let params = match Self::plugin_kind_from_path(plugin_path) {
            PluginKind::Native => {
                return Err("Native plugins do not support GM presets".to_string());
            }
            PluginKind::Vst3 => vst3::enumerate_params(plugin_path)?,
            PluginKind::Clap => {
                let clap_id = clap_host::default_plugin_id(plugin_path)?;
                let mut host = clap_host::ClapHost::load(
                    plugin_path,
                    &clap_id,
                    self.settings.sample_rate as f64,
                    self.settings.buffer_size as u32,
                    0,
                    2,
                )?;
                host.enumerate_params()
                    .into_iter()
                    .map(|param| vst3::ParamInfo {
                        id: param.id,
                        name: param.name,
                        default_value: param.default_value,
                    })
                    .collect()
            }
        };
        if params.is_empty() {
            return Err("Preset generation failed: no parameters".to_string());
        }

        let mut roots = Vec::new();
        roots.push(self.presets_root_global());
        if let Some(project_root) = self.presets_root_project() {
            roots.push(project_root);
        }

        for root in &roots {
            let preset_path = self.preset_path_for_program(root, plugin_path, requested_program);
            if preset_path.exists() {
                continue;
            }
            if let Some(parent) = preset_path.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
        }

        let targets: Vec<u8> = (0u8..=127).collect();
        for root in &roots {
            for program in &targets {
                let preset_path = self.preset_path_for_program(root, plugin_path, *program);
                if preset_path.exists() {
                    continue;
                }
                let preset = self.build_gm_preset_file(plugin_path, &params, *program);
                let json = serde_json::to_string_pretty(&preset).map_err(|e| e.to_string())?;
                fs::write(&preset_path, json).map_err(|e| e.to_string())?;
            }
        }

        Ok(())
    }

    pub(crate) fn ensure_builtin_gm_presets(&mut self) {
        if self.gm_presets_generated {
            return;
        }
        let synths = [
            "FishSynth",
            "CatSynth",
            "SannySynth",
            "DogSynth",
            "LingSynth",
            "MiceSynth",
        ];
        for name in synths {
            if let Some(path) = self.find_vst3_plugin_by_name(name) {
                let _ = self.ensure_gm_presets_for_plugin(&path, 0);
            }
        }
        self.gm_presets_generated = true;
    }

    pub(crate) fn build_gm_preset_file(
        &self,
        plugin_path: &str,
        params: &[vst3::ParamInfo],
        program: u8,
    ) -> Vst3PresetFile {
        let mut values: Vec<f32> = Vec::with_capacity(params.len());
        for param in params {
            let value = self.gm_param_value(&param.name, param.default_value as f32, program);
            values.push(value);
        }
        Vst3PresetFile {
            version: 1,
            name: self.preset_name_for_program(program),
            plugin: Self::plugin_display_name(plugin_path),
            param_names: params.iter().map(|p| p.name.clone()).collect(),
            param_ids: params.iter().map(|p| p.id).collect(),
            param_values: values,
            component_state: String::new(),
            controller_state: String::new(),
            treesynth: None,
        }
    }

    pub(crate) fn gm_param_value(&self, name: &str, default_value: f32, program: u8) -> f32 {
        let category = GmCategory::from_program(program);
        let values = GmParamValues::from_category(category);
        let key = Self::normalize_param_name(name);

        if key.contains("preset") || key.contains("program") || key.contains("patch") {
            return (program as f32 / 127.0).clamp(0.0, 1.0);
        }
        if key.contains("gain") || key.contains("volume") || key.contains("master") {
            return values.gain;
        }
        if key.contains("attack") || key.ends_with("atk") || key.contains("_atk") {
            return values.attack;
        }
        if key.contains("decay") || key.ends_with("dec") || key.contains("_dec") {
            return values.decay;
        }
        if key.contains("sustain") || key.ends_with("sus") || key.contains("_sus") {
            return values.sustain;
        }
        if key.contains("release") || key.ends_with("rel") || key.contains("_rel") {
            return values.release;
        }
        if key.contains("cutoff") || key.contains("filtercut") || key.contains("filter_cut") || key.contains("filtercutoff") || key.contains("cut") {
            return values.cutoff;
        }
        if key.contains("resonance") || key.contains("filterres") || key.contains("filter_res") || key.contains("res") {
            return values.resonance;
        }
        if key.contains("vibrato") && key.contains("rate") {
            return values.vibrato_rate;
        }
        if key.contains("vibrato") && (key.contains("int") || key.contains("amount")) {
            return values.vibrato_intensity;
        }
        if key.contains("tremolo") && key.contains("rate") {
            return values.tremolo_rate;
        }
        if key.contains("tremolo") && (key.contains("int") || key.contains("amount")) {
            return values.tremolo_intensity;
        }

        default_value.clamp(0.0, 1.0)
    }

    pub(crate) fn is_micesynth_path(path: &str) -> bool {
        path.to_ascii_lowercase().contains("micesynth")
    }

    pub(crate) fn apply_micesynth_program_from_midi(&mut self, index: usize) {
        let (program, path, params, param_ids, mut param_values, has_state) =
            match self.tracks.get(index) {
                Some(track) => (
                    track.midi_program,
                    track.instrument_path.clone(),
                    track.params.clone(),
                    track.param_ids.clone(),
                    track.param_values.clone(),
                    track
                        .plugin_state_component
                        .as_ref()
                        .map(|v| !v.is_empty())
                        .unwrap_or(false)
                        || track
                            .plugin_state_controller
                            .as_ref()
                            .map(|v| !v.is_empty())
                            .unwrap_or(false),
                ),
                None => return,
            };
        let Some(program) = program else {
            return;
        };
        let Some(path) = path else {
            return;
        };
        if has_state {
            return;
        }
        if !Self::is_micesynth_path(&path) {
            return;
        }
        let program_index = params.iter().position(|name| {
            let name = name.to_ascii_lowercase();
            name.contains("program") || name.contains("patch") || name.contains("preset")
        });
        let Some(program_index) = program_index else {
            return;
        };
        let Some(program_param_id) = param_ids.get(program_index).copied() else {
            return;
        };
        if param_values.len() != param_ids.len() {
            param_values.resize(param_ids.len(), 0.0);
        }
        let normalized = (program as f64 / 127.0).clamp(0.0, 1.0);
        if let Some(value) = param_values.get_mut(program_index) {
            *value = normalized as f32;
        }

        if let Some(host) = self.track_audio.get(index).and_then(|state| state.host.as_ref()) {
            host.push_param_change(program_param_id, normalized);
            for (slot, param_id) in param_ids.iter().enumerate() {
                if let Some(value) = host.get_param_normalized(*param_id) {
                    if let Some(target) = param_values.get_mut(slot) {
                        *target = value as f32;
                    }
                }
            }
        }

        if let Some(track) = self.tracks.get_mut(index) {
            track.param_values = param_values;
        }
    }

    pub(crate) fn scan_dir(&self, dir: &Path, out: &mut Vec<String>) {
        self.scan_dir_for_exts(dir, out, &["vst3"]);
    }

    pub(crate) fn scan_dir_for_exts(&self, dir: &Path, out: &mut Vec<String>, exts: &[&str]) {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let matches_ext = exts.iter().any(|e| *e == ext);
            if path.is_dir() {
                if matches_ext {
                    out.push(path.to_string_lossy().to_string());
                    continue;
                }
                self.scan_dir_for_exts(&path, out, exts);
            } else if matches_ext {
                out.push(path.to_string_lossy().to_string());
            }
        }
    }

    pub(crate) fn treesynth_color_from_name(name: &str) -> [u8; 3] {
        let mut hash = 2166136261u32;
        for byte in name.as_bytes() {
            hash ^= u32::from(*byte);
            hash = hash.wrapping_mul(16777619);
        }
        let r = ((hash >> 16) & 0xFF) as u8;
        let g = ((hash >> 8) & 0xFF) as u8;
        let b = (hash & 0xFF) as u8;
        [r, g, b]
    }

    pub(crate) fn treesynth_samples_dir_name(preset_name: &str) -> String {
        let safe = Self::sanitize_folder_name(preset_name);
        format!("{}_samples", safe)
    }

    pub(crate) fn treesynth_unique_filename(
        used: &mut HashMap<String, usize>,
        candidate: &str,
    ) -> String {
        if !used.contains_key(candidate) {
            used.insert(candidate.to_string(), 1);
            return candidate.to_string();
        }
        let count = used.entry(candidate.to_string()).or_insert(1);
        *count += 1;
        let stem = Path::new(candidate)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("sample");
        let ext = Path::new(candidate)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if ext.is_empty() {
            format!("{}_{}", stem, count)
        } else {
            format!("{}_{}.{}", stem, count, ext)
        }
    }

    pub(crate) fn scan_dir_for_exts_static(dir: &Path, out: &mut Vec<String>, exts: &[&str]) {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let matches_ext = exts.iter().any(|e| *e == ext);
            if path.is_dir() {
                if matches_ext {
                    out.push(path.to_string_lossy().to_string());
                    continue;
                }
                Self::scan_dir_for_exts_static(&path, out, exts);
            } else if matches_ext {
                out.push(path.to_string_lossy().to_string());
            }
        }
    }

    pub(crate) fn load_treesynth_folder(folder: &Path) -> Vec<TreeSynthSample> {
        let mut paths = Vec::new();
        Self::scan_dir_for_exts_static(
            folder,
            &mut paths,
            &["wav", "wave", "aif", "aiff", "flac", "ogg", "mp3"],
        );
        paths.sort();
        paths
            .into_iter()
            .map(|path| {
                let name = Path::new(&path)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("sample")
                    .to_string();
                TreeSynthSample {
                    path,
                    name: name.clone(),
                    root_note: 60,
                    gain: 1.0,
                    pan: 0.0,
                    start: 0.0,
                    end: 1.0,
                    color: Self::treesynth_color_from_name(&name),
                }
            })
            .collect()
    }

    pub(crate) fn draw_treesynth_panel(
        ui: &mut egui::Ui,
        state: &mut TreeSynthState,
        audio_clip_cache: &Arc<Mutex<AudioClipCache>>,
        project_root: Option<&Path>,
    ) -> bool {
        let mut changed = false;
        ui.heading("TreeSynth");
        ui.label("Native sampler");
        ui.add_space(4.0);

        let folder_label = state
            .folder
            .clone()
            .unwrap_or_else(|| "None".to_string());
        ui.horizontal(|ui| {
            ui.label("Folder");
            ui.label(&folder_label);
            if ui.button("Choose").clicked() {
                if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                    let folder = Self::normalize_windows_path(&folder);
                    let folder = if let Some(project_root) = project_root {
                        Self::import_treesynth_folder_to_project(&folder, project_root)
                            .unwrap_or(folder)
                    } else {
                        folder
                    };
                    state.folder = Some(folder.to_string_lossy().to_string());
                    state.samples = Self::load_treesynth_folder(&folder);
                    if let Ok(mut cache) = audio_clip_cache.lock() {
                        for sample in &state.samples {
                            if cache.get(&sample.path).is_some() {
                                continue;
                            }
                            if let Some(data) = Self::load_audio_clip_data(Path::new(&sample.path)) {
                                cache.insert(sample.path.clone(), Arc::new(data));
                            }
                        }
                    }
                    changed = true;
                }
            }
            let reload_enabled = state.folder.is_some();
            ui.add_enabled_ui(reload_enabled, |ui| {
                if ui.button("Reload").clicked() {
                    if let Some(folder) = state.folder.as_deref() {
                        let folder = PathBuf::from(folder);
                        state.samples = Self::load_treesynth_folder(&folder);
                        if let Ok(mut cache) = audio_clip_cache.lock() {
                            for sample in &state.samples {
                                if cache.get(&sample.path).is_some() {
                                    continue;
                                }
                                if let Some(data) = Self::load_audio_clip_data(Path::new(&sample.path)) {
                                    cache.insert(sample.path.clone(), Arc::new(data));
                                }
                            }
                        }
                        changed = true;
                    }
                }
            });
            ui.add_enabled_ui(reload_enabled, |ui| {
                if ui.button("Clear").clicked() {
                    state.folder = None;
                    state.samples.clear();
                    changed = true;
                }
            });
        });

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("Mode");
            egui::ComboBox::from_id_source("treesynth_mode")
                .selected_text(format!("{:?}", state.mode))
                .show_ui(ui, |ui| {
                    if ui.selectable_value(&mut state.mode, TreeSynthMode::Random, "Random").clicked() {
                        changed = true;
                    }
                    if ui.selectable_value(&mut state.mode, TreeSynthMode::Layer, "Layer").clicked() {
                        changed = true;
                    }
                    if ui.selectable_value(&mut state.mode, TreeSynthMode::Sequential, "Sequential").clicked() {
                        changed = true;
                    }
                    if ui.selectable_value(&mut state.mode, TreeSynthMode::Morph, "Morph").clicked() {
                        changed = true;
                    }
                    if ui.selectable_value(&mut state.mode, TreeSynthMode::Reorder, "Reorder").clicked() {
                        changed = true;
                    }
                });
        });
        if matches!(state.mode, TreeSynthMode::Morph) {
            changed |= ui
                .add(egui::Slider::new(&mut state.morph, 0.0..=1.0).text("Morph"))
                .changed();
        }
        if matches!(state.mode, TreeSynthMode::Reorder) {
            changed |= ui
                .add(egui::Slider::new(&mut state.reorder, 0.0..=1.0).text("Reorder"))
                .changed();
        }

        ui.add_space(6.0);
        ui.label("Amp");
        // パラメータごとに右クリックメニューを追加
        // TreeSynthパラメータに一意のparam_idを割り当て
        let treesynth_param_defs = [
            ("Gain", &mut state.gain, 0, 0.0, 2.0),
            ("Attack", &mut state.attack, 1, 0.0, 5.0),
            ("Decay", &mut state.decay, 2, 0.0, 5.0),
            ("Sustain", &mut state.sustain, 3, 0.0, 1.0),
            ("Release", &mut state.release, 4, 0.0, 8.0),
        ];
        // pending_midi_learn, pending_automationは親UIで管理する必要あり
        // ここではグローバル変数的に仮定（本来はAppState等で管理）
        thread_local! {
            static PENDING_MIDI_LEARN: std::cell::RefCell<Option<(usize, u32, String)>> = std::cell::RefCell::new(None);
            static PENDING_AUTOMATION: std::cell::RefCell<Option<(usize, u32, String)>> = std::cell::RefCell::new(None);
        }
        let track_index = 0; // TODO: 実際のトラックインデックスを渡す
        for (label, value, param_id, min, max) in treesynth_param_defs {
            let slider = egui::Slider::new(value, min..=max).text(label);
            let response = ui.add(slider);
            changed |= response.changed();
            response.context_menu(|ui| {
                if ui.button("Create Automation").clicked() {
                    PENDING_AUTOMATION.with(|pending| {
                        *pending.borrow_mut() = Some((track_index, param_id, label.to_string()));
                    });
                    ui.close_menu();
                }
                if ui.button("Midi Learn").clicked() {
                    PENDING_MIDI_LEARN.with(|pending| {
                        *pending.borrow_mut() = Some((track_index, param_id, label.to_string()));
                    });
                    ui.close_menu();
                }
            });
        }

        ui.add_space(6.0);
        ui.label("Modulation");
        changed |= ui
            .add(egui::Slider::new(&mut state.vibrato_rate, 0.1..=20.0).text("Vibrato Rate"))
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut state.vibrato_depth, 0.0..=1.0).text("Vibrato Depth"))
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut state.tremolo_rate, 0.1..=20.0).text("Tremolo Rate"))
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut state.tremolo_depth, 0.0..=1.0).text("Tremolo Depth"))
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut state.reverb_mix, 0.0..=1.0).text("Reverb Mix"))
            .changed();

        ui.add_space(6.0);
        ui.label("Performance");
        changed |= ui
            .add(egui::Slider::new(&mut state.pitch_bend_range, 1.0..=24.0).text("Pitch Bend"))
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut state.portamento_ms, 0.0..=500.0).text("Portamento ms"))
            .changed();
        changed |= ui.checkbox(&mut state.legato, "Legato").changed();

        ui.add_space(6.0);
        ui.label(format!("Samples: {}", state.samples.len()));
        egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
            for sample in state.samples.iter_mut() {
                ui.horizontal(|ui| {
                    ui.label(&sample.name);
                    changed |= ui
                        .add(egui::DragValue::new(&mut sample.root_note).clamp_range(0..=127))
                        .changed();
                    changed |= ui.add(egui::DragValue::new(&mut sample.gain).speed(0.01)).changed();
                    changed |= ui.add(egui::DragValue::new(&mut sample.pan).speed(0.01)).changed();
                });
            }
        });

        changed
    }

    pub(crate) fn import_treesynth_folder_to_project(source_folder: &Path, project_root: &Path) -> Result<PathBuf, String> {
        let source_folder = Self::normalize_windows_path(source_folder);
        if !source_folder.exists() {
            return Err("TreeSynth source folder not found".to_string());
        }
        let project_root = Self::normalize_windows_path(project_root);
        let samples_root = project_root.join("assets").join("samples").join("treesynth");
        fs::create_dir_all(&samples_root).map_err(|e| e.to_string())?;

        let base_name = source_folder
            .file_name()
            .and_then(|s| s.to_str())
            .map(Self::sanitize_folder_name)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "TreeSynthSamples".to_string());

        let mut dest = samples_root.join(&base_name);
        let mut suffix = 1usize;
        while dest.exists() && !Self::paths_equal(&dest, &source_folder) {
            dest = samples_root.join(format!("{}-{}", base_name, suffix));
            suffix += 1;
        }

        if !Self::paths_equal(&dest, &source_folder) {
            fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
            Self::copy_dir_recursive(&source_folder, &dest)?;
        }
        Ok(dest)
    }

    pub(crate) fn home_dir() -> Option<PathBuf> {
        if cfg!(windows) {
            std::env::var("USERPROFILE").ok().map(PathBuf::from)
        } else {
            std::env::var("HOME").ok().map(PathBuf::from)
        }
    }

    pub(crate) fn refresh_track_params(&mut self, index: usize) {
        let path = self
            .tracks
            .get(index)
            .and_then(|track| track.instrument_path.clone());
        let Some(path) = path else {
            if let Some(track) = self.tracks.get_mut(index) {
                track.params = default_midi_params();
                track.param_ids.clear();
                track.param_values.clear();
            }
            return;
        };
        let params_result = self
            .track_audio
            .get(index)
            .and_then(|state| state.host.as_ref())
            .map(|host| Ok(host.enumerate_params()))
            .unwrap_or_else(|| self.enumerate_plugin_params_for_track(index, &path));
        let Some(track) = self.tracks.get_mut(index) else {
            return;
        };
        match params_result {
            Ok(params) if !params.is_empty() => {
                let next_values = Self::remap_param_values_by_id_or_name(
                    &track.param_ids,
                    &track.params,
                    &track.param_values,
                    &params,
                );
                track.params = params.iter().map(|p| p.name.clone()).collect();
                track.param_ids = params.iter().map(|p| p.id).collect();
                track.param_values = next_values;
                Self::apply_program_param(track);
                Self::log_fm_ratio_param_from(index, "refresh_track", &track.params, &track.param_ids, &track.param_values);
                if track.automation_lanes.is_empty() && !track.automation_channels.is_empty() {
                    let mut lanes = Vec::new();
                    for name in &track.automation_channels {
                        if let Some((idx, param_id)) = track
                            .params
                            .iter()
                            .enumerate()
                            .find(|(_, n)| *n == name)
                            .and_then(|(i, _)| track.param_ids.get(i).copied().map(|id| (i, id)))
                        {
                            let _ = idx;
                            lanes.push(AutomationLane {
                                name: name.clone(),
                                param_id,
                                target: AutomationTarget::Instrument,
                                points: Vec::new(),
                            });
                        }
                    }
                    if !lanes.is_empty() {
                        track.automation_lanes = lanes;
                    }
                }
            }
            Ok(_) => {
                track.params = default_instrument_params();
                track.param_ids.clear();
                track.param_values.clear();
            }
            Err(err) => {
                track.params = default_instrument_params();
                track.param_ids.clear();
                track.param_values.clear();
                self.status = format!("Plugin params unavailable: {err}");
            }
        }
    }

    pub(crate) fn refresh_params_for_selected_track(&mut self, force: bool) {
        let Some(index) = self.selected_track else {
            return;
        };
        if self.last_params_track != Some(index) {
            self.reset_midi_for_selected_track();
        }
        if !force && self.last_params_track == Some(index) {
            return;
        }
        self.refresh_track_params(index);
        self.last_params_track = Some(index);
    }

    pub(crate) fn reset_midi_for_selected_track(&mut self) {
        self.midi_gate.store(false, Ordering::Relaxed);
        let Some(index) = self.selected_track else {
            return;
        };
        if let Some(state) = self.track_audio.get(index) {
            if let Ok(mut events) = state.midi_events.lock() {
                events.clear();
                for note in 0u8..=127 {
                    events.push(vst3::MidiEvent::note_off(0, note, 0));
                }
            }
        }
        self.sync_track_audio_notes(index);
    }

    pub(crate) fn piano_preview_note_on(&mut self, note: u8, velocity: u8) {
        let freq = 440.0f32 * 2.0f32.powf((note as f32 - 69.0) / 12.0);
        self.midi_freq_bits.store(freq.to_bits(), Ordering::Relaxed);
        self.midi_gate.store(true, Ordering::Relaxed);
        let Some(index) = self.selected_track else {
            return;
        };
        if let Some(state) = self.track_audio.get(index) {
            if let Ok(mut events) = state.midi_events.lock() {
                events.push(vst3::MidiEvent::note_on(0, note, velocity));
            }
        }
    }

    pub(crate) fn piano_preview_note_off(&mut self, note: u8) {
        self.midi_gate.store(false, Ordering::Relaxed);
        let Some(index) = self.selected_track else {
            return;
        };
        if let Some(state) = self.track_audio.get(index) {
            if let Ok(mut events) = state.midi_events.lock() {
                events.push(vst3::MidiEvent::note_off(0, note, 0));
            }
        }
    }

    pub(crate) fn replace_instrument(&mut self, index: usize, path: String, clap_id: Option<String>) {
        let mut reopen_ui = false;
        if self
            .plugin_ui
            .as_ref()
            .map_or(false, |ui| ui.target == PluginUiTarget::Instrument(index))
        {
            reopen_ui = self.show_plugin_ui;
            self.show_plugin_ui = false;
            self.destroy_plugin_ui();
        }
        let was_running = self.audio_running;
        if was_running {
            self.stop_audio_and_midi();
        }
        if let Some(track) = self.tracks.get_mut(index) {
            track.instrument_path = Some(path);
            track.instrument_clap_id = clap_id;
            track.params = default_instrument_params();
            track.param_ids.clear();
            track.param_values.clear();
            // Avoid restoring stale state blobs across plugin format switches (e.g. VST3 -> CLAP).
            track.plugin_state_component = None;
            track.plugin_state_controller = None;
            if Self::is_treesynth_path(track.instrument_path.as_deref().unwrap_or("")) {
                track.treesynth = Some(TreeSynthState::default());
            } else {
                track.treesynth = None;
            }
            if let Some(state) = self.track_audio.get(index) {
                let enabled = track
                    .instrument_path
                    .as_deref()
                    .map(Self::is_treesynth_path)
                    .unwrap_or(false);
                state.sync_treesynth(track, enabled, &self.audio_clip_cache);
            }
        }
        if let Some(state) = self.track_audio.get_mut(index) {
            if let Some(host) = state.host.take() {
                host.prepare_for_drop();
                self.orphaned_hosts.push(host);
            }
        }
        if was_running {
            self.last_params_track = None;
            self.status = "Instrument changed. Press Play to activate it safely.".to_string();
        } else if reopen_ui {
            self.plugin_ui_target = Some(PluginUiTarget::Instrument(index));
            self.plugin_ui_hidden = false;
            self.show_plugin_ui = true;
        }
        if !was_running {
            self.refresh_params_for_selected_track(true);
        } else {
            // Keep the editor closed and avoid immediate host enumeration while live transport
            // was active; some bundled synths are unstable during hot-load.
            self.show_plugin_ui = false;
        }
    }

    pub(crate) fn next_clip_id(&self) -> usize {
        self.tracks
            .iter()
            .flat_map(|track| track.clips.iter().map(|clip| clip.id))
            .max()
            .unwrap_or(0)
            + 1
    }
}
