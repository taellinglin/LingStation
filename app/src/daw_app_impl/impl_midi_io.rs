impl DawApp {
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
                drum_machine: None,
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
}
