impl DawApp {
    pub(crate) fn render_params_roll_panel(
        &mut self,
        ctx: &egui::Context,
        ui: &mut egui::Ui,
        show_params: bool,
        show_roll: bool,
    ) {
        self.piano_roll_panel_height = ui.max_rect().height();
        self.piano_roll_hovered = false;
        let mut selected_clip_info = None;
        if let Some(clip_id) = self.selected_clip {
            for (track_index, track) in self.tracks.iter().enumerate() {
                if let Some(clip_index) = track.clips.iter().position(|c| c.id == clip_id) {
                    selected_clip_info = Some((track_index, clip_index));
                    break;
                }
            }
        }
        let is_audio_clip = selected_clip_info
            .and_then(|(ti, ci)| self.tracks.get(ti).and_then(|t| t.clips.get(ci)))
            .map(|c| !c.is_midi)
            .unwrap_or(false);

        if show_roll {
            egui::SidePanel::left("piano_tools")
                .default_width(220.0)
                .resizable(true)
                .show_inside(ui, |ui| {
                    ui.heading(if is_audio_clip { "Audio Clip" } else { "Piano Roll" });
                    if let Some(clip_id) = self.selected_clip {
                        ui.label(format!("Clip {}", clip_id));
                    } else {
                        ui.label("No clip selected");
                    }
                    if let Some((ti, ci)) = selected_clip_info {
                        if let Some(clip) =
                            self.tracks.get_mut(ti).and_then(|track| track.clips.get_mut(ci))
                        {
                            ui.add_space(4.0);
                            let mut name_changed = false;
                            ui.horizontal(|ui| {
                                ui.label("Name");
                                name_changed =
                                    ui.text_edit_singleline(&mut clip.name).changed();
                            });
                            if name_changed {
                                self.mark_dirty();
                            }
                        }
                    }
                    if !is_audio_clip {
                        ui.add_space(6.0);
                        let lengths = [
                            (1.0 / 32.0, "1/32"),
                            (1.0 / 16.0, "1/16"),
                            (1.0 / 8.0, "1/8"),
                            (1.0 / 4.0, "1/4"),
                            (1.0 / 2.0, "1/2"),
                            (1.0, "1"),
                        ];
                        let note_label = lengths
                            .iter()
                            .find(|(value, _)| (self.piano_note_len - *value).abs() < f32::EPSILON)
                            .map(|(_, label)| *label)
                            .unwrap_or("1/4");
                        let snap_label = lengths
                            .iter()
                            .find(|(value, _)| (self.piano_snap - *value).abs() < f32::EPSILON)
                            .map(|(_, label)| *label)
                            .unwrap_or("1/4");

                        ui.horizontal(|ui| {
                            ui.label("Tools:");
                        });
                        let tool_size = egui::vec2(86.0, 22.0);
                        let icon_size = egui::vec2(14.0, 14.0);
                        let button_bg = egui::Color32::from_rgba_premultiplied(18, 20, 24, 220);
                        let button_on = egui::Color32::from_rgba_premultiplied(46, 94, 130, 220);
                        let icon_tint = egui::Color32::from_gray(220);
                        ui.horizontal(|ui| {
                            let draw_selected = self.piano_tool == PianoTool::Pencil;
                            let draw_button = egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../../assets/icons/pen-tool.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(icon_tint),
                                "Draw",
                            )
                            .min_size(tool_size)
                            .fill(if draw_selected { button_on } else { button_bg });
                            if ui.add(draw_button).clicked() {
                                self.piano_tool = PianoTool::Pencil;
                            }

                            let select_selected = self.piano_tool == PianoTool::Select;
                            let select_button = egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../../assets/icons/mouse-pointer.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(icon_tint),
                                "Select",
                            )
                            .min_size(tool_size)
                            .fill(if select_selected { button_on } else { button_bg });
                            if ui.add(select_button).clicked() {
                                self.piano_tool = PianoTool::Select;
                            }
                        });
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.label("Note:");
                            egui::ComboBox::from_id_source("piano_note_len")
                                .selected_text(note_label)
                                .show_ui(ui, |ui| {
                                    for (value, label) in lengths {
                                        ui.selectable_value(&mut self.piano_note_len, value, label);
                                    }
                                });
                        });
                        ui.horizontal(|ui| {
                            ui.label("Snap:");
                            egui::ComboBox::from_id_source("piano_snap")
                                .selected_text(snap_label)
                                .show_ui(ui, |ui| {
                                    for (value, label) in lengths {
                                        ui.selectable_value(&mut self.piano_snap, value, label);
                                    }
                                });
                        });
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.label("Lane:");
                            egui::ComboBox::from_id_source("piano_lane_mode")
                                .selected_text(match self.piano_lane_mode {
                                    PianoLaneMode::Velocity => "Velocity",
                                    PianoLaneMode::Pan => "Pan",
                                    PianoLaneMode::Cutoff => "Cutoff",
                                    PianoLaneMode::Resonance => "Resonance",
                                    PianoLaneMode::MidiCc => "MIDI CC",
                                })
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut self.piano_lane_mode,
                                        PianoLaneMode::Velocity,
                                        "Velocity",
                                    );
                                    ui.selectable_value(&mut self.piano_lane_mode, PianoLaneMode::Pan, "Pan");
                                    ui.selectable_value(
                                        &mut self.piano_lane_mode,
                                        PianoLaneMode::Cutoff,
                                        "Cutoff",
                                    );
                                    ui.selectable_value(
                                        &mut self.piano_lane_mode,
                                        PianoLaneMode::Resonance,
                                        "Resonance",
                                    );
                                    ui.selectable_value(
                                        &mut self.piano_lane_mode,
                                        PianoLaneMode::MidiCc,
                                        "MIDI CC",
                                    );
                                });
                        });
                        if self.piano_lane_mode == PianoLaneMode::MidiCc {
                            ui.horizontal(|ui| {
                                ui.label("CC");
                                ui.add(
                                    egui::DragValue::new(&mut self.piano_cc)
                                        .clamp_range(0..=127)
                                        .speed(1.0),
                                );
                            });
                        }
                    }
                });
        }

        if show_params && !show_roll {
            let selected_track_index = self.selected_track;
            let track_color = selected_track_index.map(|i| self.track_color(i));
            let mut pending_automation_record: Vec<(usize, RecordedAutomationPoint)> = Vec::new();
            let mut pending_lane_delete: Option<(usize, usize)> = None;
            let mut pending_active_lane: Option<(usize, usize)> = None;
            let columns_height = ui.available_height();

            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(260.0);
                    ui.set_min_height(columns_height);
                    ui.heading("Parameters");
                    ui.separator();
                    if is_audio_clip {
                        let key_display_format = self.settings.key_display_format.clone();
                        let tempo_bpm = self.tempo_bpm;
                        let analyzing = selected_clip_info
                            .and_then(|(ti, ci)| {
                                self.tracks
                                    .get(ti)
                                    .and_then(|t| t.clips.get(ci))
                                    .map(|clip| self.analysis_pending.contains(&clip.id))
                            })
                            .unwrap_or(false);
                        let mut analyze_request: Option<(usize, Option<PathBuf>)> = None;
                        if let Some((ti, ci)) = selected_clip_info {
                            let clip_path = self
                                .tracks
                                .get(ti)
                                .and_then(|t| t.clips.get(ci))
                                .and_then(|clip| self.resolve_clip_audio_path(clip))
                                .map(|path| path.to_path_buf());
                            let mut audio_changed = false;
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                if let Some(clip) =
                                    self.tracks.get_mut(ti).and_then(|t| t.clips.get_mut(ci))
                                {
                                    ui.label("Clip Properties");
                                    ui.add_space(6.0);
                                    ui.horizontal(|ui| {
                                        ui.label("Gain");
                                        if Self::colored_slider(
                                            ui,
                                            &mut clip.audio_gain,
                                            0.0..=2.0,
                                            track_color,
                                        )
                                        .changed()
                                        {
                                            audio_changed = true;
                                        }
                                    });
                                    if ui
                                        .add(egui::Button::new("Normalize"))
                                        .on_hover_text("Normalize clip gain to -1 dB peak")
                                        .clicked()
                                    {
                                        match clip_path.as_ref() {
                                            Some(path) => {
                                                if let Err(err) =
                                                    Self::normalize_audio_clip_with_path(clip, path)
                                                {
                                                    self.status = format!("Normalize failed: {err}");
                                                } else {
                                                    audio_changed = true;
                                                }
                                            }
                                            None => {
                                                self.status =
                                                    "Normalize failed: Clip has no audio file".to_string();
                                            }
                                        }
                                    }
                                    ui.horizontal(|ui| {
                                        ui.label("Stretch");
                                        let prev = clip.audio_stretch_mode;
                                        let selected = match clip.audio_stretch_mode {
                                            AudioStretchMode::Stretch => "Stretch",
                                            AudioStretchMode::StretchFormant => "Stretch (Formant)",
                                            AudioStretchMode::StretchNeutral => "Stretch (Neutral)",
                                            AudioStretchMode::StretchVocal => "Vocal (Formant)",
                                            AudioStretchMode::Speed => "Speed (Resample)",
                                        };
                                        egui::ComboBox::from_id_source(
                                            ("audio_stretch_mode_params", clip.id),
                                        )
                                        .selected_text(selected)
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(
                                                &mut clip.audio_stretch_mode,
                                                AudioStretchMode::Stretch,
                                                "Stretch",
                                            );
                                            ui.selectable_value(
                                                &mut clip.audio_stretch_mode,
                                                AudioStretchMode::StretchFormant,
                                                "Stretch (Formant)",
                                            );
                                            ui.selectable_value(
                                                &mut clip.audio_stretch_mode,
                                                AudioStretchMode::StretchNeutral,
                                                "Stretch (Neutral)",
                                            );
                                            ui.selectable_value(
                                                &mut clip.audio_stretch_mode,
                                                AudioStretchMode::StretchVocal,
                                                "Vocal (Formant)",
                                            );
                                            ui.selectable_value(
                                                &mut clip.audio_stretch_mode,
                                                AudioStretchMode::Speed,
                                                "Speed (Resample)",
                                            );
                                        });
                                        if clip.audio_stretch_mode != prev {
                                            audio_changed = true;
                                        }
                                    });
                                    let rb_active = cfg!(has_rubberband)
                                        && clip.audio_stretch_mode != AudioStretchMode::Speed;
                                    let rb_label = if rb_active {
                                        "Rubber Band: Active"
                                    } else if cfg!(has_rubberband) {
                                        "Rubber Band: Off (Speed mode)"
                                    } else {
                                        "Rubber Band: Not linked"
                                    };
                                    ui.label(
                                        egui::RichText::new(rb_label).color(if rb_active {
                                            egui::Color32::from_rgb(120, 200, 140)
                                        } else {
                                            egui::Color32::from_gray(150)
                                        }),
                                    );
                                    ui.horizontal(|ui| {
                                        ui.label("Preserve Formant");
                                        let mut preserve = matches!(
                                            clip.audio_stretch_mode,
                                            AudioStretchMode::StretchFormant | AudioStretchMode::StretchVocal
                                        );
                                        let enabled = clip.audio_stretch_mode != AudioStretchMode::Speed;
                                        ui.add_enabled_ui(enabled, |ui| {
                                            if ui.checkbox(&mut preserve, "").changed() {
                                                clip.audio_stretch_mode = if preserve {
                                                    AudioStretchMode::StretchFormant
                                                } else {
                                                    AudioStretchMode::Stretch
                                                };
                                                audio_changed = true;
                                            }
                                        });
                                    });
                                    ui.horizontal(|ui| {
                                        ui.label("Pitch");
                                        if Self::colored_slider(
                                            ui,
                                            &mut clip.audio_pitch_semitones,
                                            -24.0..=24.0,
                                            track_color,
                                        )
                                        .changed()
                                        {
                                            audio_changed = true;
                                        }
                                    });
                                    ui.horizontal(|ui| {
                                        ui.label("Formant");
                                        let enabled = clip.audio_stretch_mode != AudioStretchMode::Speed;
                                        ui.add_enabled_ui(enabled, |ui| {
                                            if Self::colored_slider(
                                                ui,
                                                &mut clip.audio_formant_scale,
                                                0.5..=2.0,
                                                track_color,
                                            )
                                            .changed()
                                            {
                                                audio_changed = true;
                                            }
                                        });
                                    });
                                    let (analyze_clicked, analysis_changed) =
                                        Self::draw_audio_analysis_controls(
                                        ui,
                                        clip,
                                        clip_path.as_deref(),
                                        key_display_format.as_str(),
                                        tempo_bpm,
                                        analyzing,
                                    );
                                    if analysis_changed {
                                        audio_changed = true;
                                    }
                                    if analyze_clicked {
                                        analyze_request = Some((clip.id, clip_path.clone()));
                                    }
                                    ui.horizontal(|ui| {
                                        ui.label("Time Mul");
                                        let mut time_mul = clip.audio_time_mul;
                                        let slider_changed = Self::colored_slider(
                                            ui,
                                            &mut time_mul,
                                            0.25..=4.0,
                                            track_color,
                                        )
                                        .changed();
                                        let input_changed = ui
                                            .add(egui::DragValue::new(&mut time_mul).speed(0.01))
                                            .changed();
                                        if slider_changed || input_changed {
                                            clip.audio_time_mul = time_mul.clamp(0.01, 8.0);
                                            audio_changed = true;
                                        }
                                    });
                                    ui.horizontal(|ui| {
                                        ui.label("Offset");
                                        if ui
                                            .add(
                                                egui::DragValue::new(&mut clip.audio_offset_beats)
                                                    .speed(0.1),
                                            )
                                            .changed()
                                        {
                                            audio_changed = true;
                                        }
                                    });
                                    ui.add_space(6.0);
                                    if ui
                                        .add(egui::Button::image(
                                            egui::Image::new(egui::include_image!(
                                                "../../../assets/icons/refresh-cw.svg"
                                            ))
                                            .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                        ))
                                        .on_hover_text("Fit to project tempo")
                                        .clicked()
                                    {
                                        if let Some(source) = clip.audio_source_beats {
                                            if source > 0.0 && clip.length_beats > 0.0 {
                                                clip.audio_time_mul = source / clip.length_beats;
                                                audio_changed = true;
                                            }
                                        }
                                    }
                                    if ui
                                        .add(egui::Button::image(
                                            egui::Image::new(egui::include_image!(
                                                "../../../assets/icons/rotate-ccw.svg"
                                            ))
                                            .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                        ))
                                        .on_hover_text("Reset Audio Props")
                                        .clicked()
                                    {
                                        clip.audio_gain = 1.0;
                                        clip.audio_pitch_semitones = 0.0;
                                        clip.audio_stretch_mode = AudioStretchMode::Stretch;
                                        clip.audio_time_mul = 1.0;
                                        clip.audio_offset_beats = 0.0;
                                        clip.audio_key = None;
                                        clip.audio_key_minor = false;
                                        clip.audio_key_source = None;
                                        clip.audio_bpm = None;
                                        clip.audio_fine_pitch_cents = 0.0;
                                        clip.audio_formant_scale = 1.0;
                                        audio_changed = true;
                                    }
                                }
                                self.draw_effect_params_panel(
                                    ui,
                                    ti,
                                    track_color,
                                    &mut pending_automation_record,
                                );
                            });
                            if audio_changed {
                                self.refresh_audio_clip_timeline_if_running();
                            }
                            if let Some((clip_id, clip_path)) = analyze_request.take() {
                                if let Some(path) = clip_path {
                                    self.enqueue_audio_analysis(clip_id, path);
                                } else {
                                    self.status = "Analyze failed: clip has no audio file".to_string();
                                }
                            }
                        }
                    } else {
                        let track = selected_track_index.and_then(|i| self.tracks.get(i));
                        let name = track.map(|t| t.name.as_str()).unwrap_or("None");
                        ui.label(format!("Track: {name}"));
                        if let Some(track) = track {
                            ui.label(format!("FX slots: {}", track.effect_paths.len()));
                        }
                        let is_native = track
                            .and_then(|t| t.instrument_path.as_deref())
                            .map(Self::is_native_plugin_path)
                            .unwrap_or(false);
                        let plugin = track
                            .and_then(|t| t.instrument_path.as_deref())
                            .map(Self::plugin_display_name)
                            .unwrap_or_else(|| "No instrument".to_string());
                        ui.label(format!("Plugin: {plugin}"));
                        ui.add_space(6.0);
                        ui.label("Instrument");
                        ui.horizontal(|ui| {
                            let choose = ui
                                .add(egui::Button::image(
                                    egui::Image::new(egui::include_image!("../../../assets/icons/folder-plus.svg"))
                                        .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                ))
                                .on_hover_text("Choose");
                            let open = ui
                                .add(egui::Button::image(
                                    egui::Image::new(egui::include_image!("../../../assets/icons/external-link.svg"))
                                        .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                ))
                                .on_hover_text("Open UI");
                            let clear = ui
                                .add(egui::Button::image(
                                    egui::Image::new(egui::include_image!("../../../assets/icons/x.svg"))
                                        .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                ))
                                .on_hover_text("Clear");
                            if let Some(index) = selected_track_index {
                                if choose.clicked() {
                                    self.open_plugin_picker(PluginTarget::Instrument(index));
                                }
                                if !is_native && open.clicked() {
                                    self.plugin_ui_target = Some(PluginUiTarget::Instrument(index));
                                    self.show_plugin_ui = true;
                                }
                                if clear.clicked() {
                                    if self.plugin_ui_matches(PluginUiTarget::Instrument(index)) {
                                        self.show_plugin_ui = false;
                                        self.destroy_plugin_ui();
                                    }
                                    if let Some(track) = self.tracks.get_mut(index) {
                                        track.instrument_path = None;
                                        track.treesynth = None;
                                        track.params = default_midi_params();
                                        track.param_ids.clear();
                                        track.param_values.clear();
                                    }
                                    if let Some(state) = self.track_audio.get_mut(index) {
                                        if let Some(host) = state.host.take() {
                                            host.prepare_for_drop();
                                            self.orphaned_hosts.push(host);
                                        }
                                    }
                                    self.reinit_audio_if_running();
                                }
                            }
                        });
                        ui.add_space(6.0);
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            let is_treesynth = selected_track_index
                                .and_then(|i| self.tracks.get(i))
                                .and_then(|track| track.instrument_path.as_deref())
                                .map(Self::is_treesynth_path)
                                .unwrap_or(false);
                            if is_treesynth {
                                let audio_cache = self.audio_clip_cache.clone();
                                let mut changed = false;
                                if let Some(track_index) = selected_track_index {
                                    let project_root = if self.project_path.trim().is_empty() {
                                        None
                                    } else {
                                        Some(PathBuf::from(self.project_path.trim()))
                                    };
                                    if let Some(track) = self.tracks.get_mut(track_index) {
                                        if let Some(state) = track.treesynth.as_mut() {
                                            changed = Self::draw_treesynth_panel(
                                                ui,
                                                state,
                                                &audio_cache,
                                                project_root.as_deref(),
                                            );
                                        } else {
                                            ui.colored_label(egui::Color32::RED, "[TreeSynth] サンプル未ロード: プリセットまたはサンプルをロードしてください");
                                        }
                                    }
                                }
                                if let Some(track_index) = selected_track_index {
                                    if let Some(track) = self.tracks.get(track_index) {
                                        ui.add_space(6.0);
                                        if ui.button("Save As").clicked() {
                                            let plugin_path = track
                                                .instrument_path
                                                .as_deref()
                                                .unwrap_or("native:treesynth");
                                            let base_dir = self.presets_root_global();
                                            let preset_dir = self.preset_plugin_dir(&base_dir, plugin_path);
                                            let default_name = "TreeSynth".to_string();
                                            if track.treesynth.is_none() {
                                                self.status = "[TreeSynth] サンプル未ロード: 保存できません".to_string();
                                                return;
                                            }
                                            let file_name = format!(
                                                "{}.lingpreset.json",
                                                Self::sanitize_folder_name(&default_name)
                                            );
                                            let dialog = rfd::FileDialog::new()
                                                .set_directory(&preset_dir)
                                                .set_file_name(&file_name)
                                                .add_filter("Preset", &["json"]);
                                            if let Some(path) = dialog.save_file() {
                                                let preset_name = path
                                                    .file_stem()
                                                    .and_then(|s| s.to_str())
                                                    .unwrap_or("TreeSynth");
                                                match self.save_treesynth_preset_at_path(
                                                    track_index,
                                                    &path,
                                                    preset_name,
                                                ) {
                                                    Ok(saved) => {
                                                        self.status = format!("Preset saved: {saved}")
                                                    }
                                                    Err(err) => {
                                                        self.status = format!("Preset save failed: {err}")
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                if changed {
                                    self.mark_dirty();
                                    if let Some(track_index) = selected_track_index {
                                        if let Some(track) = self.tracks.get(track_index) {
                                            if let Some(state) = self.track_audio.get(track_index) {
                                                let enabled = track
                                                    .instrument_path
                                                    .as_deref()
                                                    .map(Self::is_treesynth_path)
                                                    .unwrap_or(false);
                                                state.sync_treesynth(track, enabled, &self.audio_clip_cache);
                                                if let Ok(mut runtime) =
                                                    state.treesynth_runtime.lock()
                                                {
                                                    runtime.voices.clear();
                                                    runtime.sequence_index = 0;
                                                }
                                            }
                                        }
                                    }
                                }
                                if let Some(track_index) = selected_track_index {
                                    self.draw_effect_params_panel(
                                        ui,
                                        track_index,
                                        track_color,
                                        &mut pending_automation_record,
                                    );
                                }
                                return;
                            }
                            self.refresh_clap_params_if_needed();
                            self.ensure_live_params();
                            let host_change = if let Some(PluginHostHandle::Vst3(host)) = self.selected_track_host() {
                                if let Ok(mut host) = host.try_lock() {
                                    host.take_last_param_change()
                                } else {
                                    None
                                }
                            } else {
                                None
                            };
                            let menu_color = selected_track_index
                                .map(|index| self.track_color(index))
                                .unwrap_or_else(|| egui::Color32::from_gray(200));
                            let mut pending_status: Option<String> = None;
                            let mut pending_midi_learn: Option<(usize, u32, String)> = None;
                            let mut pending_active_lane: Option<(usize, usize)> = None;
                            let project_root = self.presets_root_project();
                            let global_root = self.presets_root_global();
                            let can_project = project_root.is_some();
                            if let Some(track) = selected_track_index.and_then(|i| self.tracks.get_mut(i)) {
                                if let Some((param_id, value)) = host_change {
                                    if let Some(pos) = track.param_ids.iter().position(|id| *id == param_id) {
                                        track.param_values[pos] = value as f32;
                                        self.last_ui_param_change = Some((param_id, value as f32));
                                    }
                                }
                                if track.param_values.len() != track.params.len() {
                                    track.param_values.resize(track.params.len(), 0.0);
                                }
                                if let Some(program_index) = track
                                    .params
                                    .iter()
                                    .position(|name| {
                                        let name = name.to_ascii_lowercase();
                                        name.contains("program") || name.contains("preset")
                                    })
                                {
                                    let current = (track.param_values[program_index] * 127.0)
                                        .round()
                                        .clamp(0.0, 127.0) as u8;
                                    let mut selected = current;
                                    egui::ComboBox::from_label("Preset")
                                        .selected_text(format!(
                                            "{:03} {}",
                                            selected + 1,
                                            gm_program_name(selected)
                                        ))
                                        .show_ui(ui, |ui| {
                                            for program in 0u8..=127 {
                                                let label = format!(
                                                    "{:03} {}",
                                                    program + 1,
                                                    gm_program_name(program)
                                                );
                                                if ui
                                                    .selectable_label(program == selected, label)
                                                    .clicked()
                                                {
                                                    selected = program;
                                                }
                                            }
                                        });
                                    if selected != current {
                                        let value = (selected as f32 / 127.0).clamp(0.0, 1.0);
                                        track.param_values[program_index] = value;
                                        if let Some(param_id) = track.param_ids.get(program_index).copied() {
                                            if let Some(state) =
                                                selected_track_index.and_then(|i| self.track_audio.get(i))
                                            {
                                                if let Ok(mut pending) =
                                                    state.pending_param_changes.lock()
                                                {
                                                    pending.push(PendingParamChange {
                                                        target: PendingParamTarget::Instrument,
                                                        param_id,
                                                        value: value as f64,
                                                    });
                                                }
                                            }
                                            if self.is_recording && self.record_automation {
                                                if let Some(track_index) = selected_track_index {
                                                    pending_automation_record.push((
                                                        track_index,
                                                        RecordedAutomationPoint {
                                                            param_id,
                                                            target: AutomationTarget::Instrument,
                                                            beat: self.playhead_beats,
                                                            value,
                                                        },
                                                    ));
                                                }
                                            }
                                        }
                                    }
                                    ui.add_space(6.0);
                                }
                            }
                            if let Some(track_index) = selected_track_index {
                                self.draw_effect_params_panel(
                                    ui,
                                    track_index,
                                    track_color,
                                    &mut pending_automation_record,
                                );
                            }
                            ui.separator();
                            ui.label("Presets");
                            ui.horizontal(|ui| {
                                ui.label("Name");
                                ui.text_edit_singleline(&mut self.preset_name_buffer);
                            });
                            let preset_name = self.preset_name_buffer.trim().to_string();
                            ui.horizontal(|ui| {
                                if ui.button("Generate GM Presets").clicked() {
                                    self.ensure_builtin_gm_presets();
                                    self.status = "GM presets generated".to_string();
                                }
                                if ui.button("Save Global").clicked() {
                                    if let Some(index) = selected_track_index {
                                        match self.save_preset_for_track(
                                            index,
                                            global_root.clone(),
                                            &preset_name,
                                        ) {
                                            Ok(path) => self.status = format!("Preset saved: {path}"),
                                            Err(err) => self.status = format!("Preset save failed: {err}"),
                                        }
                                    }
                                }
                                ui.add_enabled_ui(can_project, |ui| {
                                    if ui.button("Save Project").clicked() {
                                        if let (Some(index), Some(root)) =
                                            (selected_track_index, project_root.clone())
                                        {
                                            match self.save_preset_for_track(
                                                index,
                                                root,
                                                &preset_name,
                                            ) {
                                                Ok(path) => {
                                                    self.status = format!("Preset saved: {path}");
                                                }
                                                Err(err) => {
                                                    self.status = format!("Preset save failed: {err}");
                                                }
                                            }
                                        }
                                    }
                                });
                            });
                            ui.horizontal(|ui| {
                                if ui.button("Load Global").clicked() {
                                    if let Some(index) = selected_track_index {
                                        let file = rfd::FileDialog::new()
                                            .set_directory(&global_root)
                                            .add_filter("Preset", &["json"])
                                            .pick_file();
                                        if let Some(file) = file {
                                            if let Err(err) = self.load_preset_from_path(index, &file) {
                                                self.status = format!("Preset load failed: {err}");
                                            } else {
                                                self.status = "Preset loaded".to_string();
                                            }
                                        }
                                    }
                                }
                                ui.add_enabled_ui(can_project, |ui| {
                                    if ui.button("Load Project").clicked() {
                                        if let (Some(index), Some(root)) =
                                            (selected_track_index, project_root.clone())
                                        {
                                            let file = rfd::FileDialog::new()
                                                .set_directory(&root)
                                                .add_filter("Preset", &["json"])
                                                .pick_file();
                                            if let Some(file) = file {
                                                if let Err(err) =
                                                    self.load_preset_from_path(index, &file)
                                                {
                                                    self.status =
                                                        format!("Preset load failed: {err}");
                                                } else {
                                                    self.status = "Preset loaded".to_string();
                                                }
                                            }
                                        }
                                    }
                                });
                            });
                            if let Some(track) = selected_track_index.and_then(|i| self.tracks.get_mut(i)) {
                                for index in 0..track.params.len() {
                                    let label = track.params[index].clone();
                                    let value = &mut track.param_values[index];
                                    let slider = ui.push_id(format!("param_{}", label), |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(&label);
                                                    Self::colored_slider(ui, value, 0.0..=1.0, track_color)
                                        })
                                        .inner
                                    });
                                    let response = slider.response;
                                    let slider_response = slider.inner;
                                    let changed = slider_response.changed()
                                        || slider_response.dragged()
                                        || response.dragged();
                                    if changed {
                                        let param_id = track.param_ids.get(index).copied();
                                        let debug_id = param_id.unwrap_or(u32::MAX);
                                        self.last_ui_param_change = Some((debug_id, *value));
                                        if let Some(param_id) = param_id {
                                            if let Some(state) =
                                                selected_track_index.and_then(|i| self.track_audio.get(i))
                                            {
                                                let blocked = state
                                                    .host
                                                    .as_ref()
                                                    .map(|host| host.clap_blocks_params())
                                                    .unwrap_or(false);
                                                if blocked {
                                                    self.status =
                                                        "Vital CLAP: DAW param changes disabled".to_string();
                                                    continue;
                                                }
                                                if let Some(PluginHostHandle::Vst3(host)) =
                                                    state.host.as_ref()
                                                {
                                                    if let Ok(host) = host.try_lock() {
                                                        if let Some((channel, controller)) =
                                                            host.param_to_cc(param_id)
                                                        {
                                                            if let Ok(mut events) = state.midi_events.lock() {
                                                                let cc_value = (*value * 127.0).round() as i32;
                                                                let cc_value = cc_value.clamp(0, 127) as u8;
                                                                events.push(vst3::MidiEvent::control_change(
                                                                    channel,
                                                                    controller,
                                                                    cc_value,
                                                                ));
                                                            }
                                                        }
                                                    }
                                                }
                                                if let Ok(mut pending) =
                                                    state.pending_param_changes.lock()
                                                {
                                                    pending.push(PendingParamChange {
                                                        target: PendingParamTarget::Instrument,
                                                        param_id,
                                                        value: *value as f64,
                                                    });
                                                }
                                            }
                                            if self.is_recording && self.record_automation {
                                                if let Some(track_index) = selected_track_index {
                                                    pending_automation_record.push((
                                                        track_index,
                                                        RecordedAutomationPoint {
                                                            param_id,
                                                            target: AutomationTarget::Instrument,
                                                            beat: self.playhead_beats,
                                                            value: *value,
                                                        },
                                                    ));
                                                }
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
                                            if let Some(param_id) = track.param_ids.get(index).copied() {
                                                if let Some(track_index) = selected_track_index {
                                                    pending_midi_learn =
                                                        Some((track_index, param_id, label.clone()));
                                                }
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
                                            if let Some(param_id) = track.param_ids.get(index).copied() {
                                                if !track
                                                    .automation_lanes
                                                    .iter()
                                                    .any(|l| l.param_id == param_id)
                                                {
                                                    track.automation_lanes.push(AutomationLane {
                                                        name: label.clone(),
                                                        param_id,
                                                        target: AutomationTarget::Instrument,
                                                        points: Vec::new(),
                                                    });
                                                }
                                                if let Some(pos) = track
                                                    .automation_lanes
                                                    .iter()
                                                    .position(|l| l.param_id == param_id)
                                                {
                                                    if let Some(track_index) = selected_track_index {
                                                        pending_active_lane = Some((track_index, pos));
                                                    }
                                                }
                                            }
                                            ui.close_menu();
                                        }
                                    });
                                }
                                if let Some((track_index, param_id, label)) = pending_midi_learn.take() {
                                    if let Ok(mut learn) = self.midi_learn.lock() {
                                        *learn = Some((track_index, param_id));
                                    }
                                    pending_status = Some(format!("MIDI Learn armed for {}", label));
                                }
                                if let Some(status) = pending_status.take() {
                                    self.status = status;
                                }
                                if let Some(active) = pending_active_lane.take() {
                                    self.automation_active = Some(active);
                                }

                                if ui
                                        .add(egui::Button::image(
                                            egui::Image::new(egui::include_image!("../../../assets/icons/shuffle.svg"))
                                                .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                        ))
                                        .on_hover_text("Randomize Params")
                                        .clicked()
                                    {
                                    let blocked = selected_track_index
                                        .and_then(|i| self.track_audio.get(i))
                                        .and_then(|state| state.host.as_ref())
                                        .map(|host| host.clap_blocks_params())
                                        .unwrap_or(false);
                                    if blocked {
                                        self.status =
                                            "Vital CLAP: DAW param changes disabled".to_string();
                                        return;
                                    }
                                    let seed = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_nanos() as u64)
                                        .unwrap_or(0x1234_5678);
                                    let mut rng = seed;
                                    for idx in 0..track.param_values.len() {
                                        rng ^= rng << 13;
                                        rng ^= rng >> 7;
                                        rng ^= rng << 17;
                                        let value = (rng as f64 / u64::MAX as f64) as f32;
                                        track.param_values[idx] = value;
                                        if let Some(param_id) = track.param_ids.get(idx).copied() {
                                            if let Some(state) = selected_track_index
                                                .and_then(|i| self.track_audio.get(i))
                                            {
                                                if let Ok(mut pending) =
                                                    state.pending_param_changes.lock()
                                                {
                                                    pending.push(PendingParamChange {
                                                        target: PendingParamTarget::Instrument,
                                                        param_id,
                                                        value: value as f64,
                                                    });
                                                }
                                            }
                                        }
                                    }
                                }

                            }
                        });
                    }
                });

                ui.separator();

                ui.vertical(|ui| {
                    ui.set_width(240.0);
                    ui.set_min_height(columns_height);
                    ui.heading("Automation");
                    ui.separator();
                    let Some(track_index) = selected_track_index else {
                        ui.label("No track selected");
                        return;
                    };
                    let Some(track) = self.tracks.get(track_index) else {
                        ui.label("No track selected");
                        return;
                    };
                    if track.automation_lanes.is_empty() {
                        ui.label("No automation lanes");
                    } else {
                        for (lane_index, lane) in track.automation_lanes.iter().enumerate() {
                            let selected = self
                                .automation_active
                                .map(|(ai, li)| ai == track_index && li == lane_index)
                                .unwrap_or(false);
                            ui.horizontal(|ui| {
                                let lane_response = ui.selectable_label(selected, &lane.name);
                                if lane_response.clicked() {
                                    pending_active_lane = Some((track_index, lane_index));
                                }
                                if ui
                                    .add(egui::Button::image(
                                        egui::Image::new(egui::include_image!("../../../assets/icons/trash-2.svg"))
                                            .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                    ))
                                    .on_hover_text("Delete")
                                    .clicked()
                                {
                                    pending_lane_delete = Some((track_index, lane_index));
                                }
                            });
                        }
                    }
                });

                ui.separator();

                ui.vertical(|ui| {
                    ui.set_width(240.0);
                    ui.set_min_height(columns_height);
                    ui.heading("Routing");
                    ui.separator();
                    ui.label("-Mappings");
                    ui.label("No mappings");
                    ui.add_space(8.0);
                    ui.label("-Macros");
                    ui.label("No macros");
                });
            });

            if let Some((track_index, lane_index)) = pending_active_lane {
                self.automation_active = Some((track_index, lane_index));
            }
            for (track_index, point) in pending_automation_record {
                self.record_automation_point(
                    track_index,
                    point.target,
                    point.param_id,
                    point.beat,
                    point.value,
                );
            }
            if let Some((track_index, lane_index)) = pending_lane_delete {
                if let Some(track) = self.tracks.get_mut(track_index) {
                    if lane_index < track.automation_lanes.len() {
                        track.automation_lanes.remove(lane_index);
                    }
                }
                if let Some(state) = self.track_audio.get(track_index) {
                    if let Ok(mut lanes) = state.automation_lanes.lock() {
                        *lanes = self
                            .tracks
                            .get(track_index)
                            .map(|t| t.automation_lanes.clone())
                            .unwrap_or_default();
                    }
                }
                if let Some((active_track, active_lane)) = self.automation_active {
                    if active_track == track_index {
                        if active_lane == lane_index {
                            self.automation_active = None;
                        } else if active_lane > lane_index {
                            self.automation_active = Some((track_index, active_lane - 1));
                        }
                    }
                }
            }
            return;
        }
        if show_params {
            egui::SidePanel::left("piano_params")
                .default_width(220.0)
                .resizable(true)
                .show_inside(ui, |ui| {
                        ui.heading(if is_audio_clip { "Audio" } else { "Parameters" });
                        ui.separator();
                        let track_color = self.selected_track.map(|i| self.track_color(i));
                        if is_audio_clip {
                            let key_display_format = self.settings.key_display_format.clone();
                            let tempo_bpm = self.tempo_bpm;
                            let analyzing = selected_clip_info
                                .and_then(|(ti, ci)| {
                                    self.tracks
                                        .get(ti)
                                        .and_then(|t| t.clips.get(ci))
                                        .map(|clip| self.analysis_pending.contains(&clip.id))
                                })
                                .unwrap_or(false);
                            let mut analyze_request: Option<(usize, Option<PathBuf>)> = None;
                            let clip_path = selected_clip_info
                                .and_then(|(ti, ci)| {
                                    self.tracks
                                        .get(ti)
                                        .and_then(|t| t.clips.get(ci))
                                        .and_then(|clip| self.resolve_clip_audio_path(clip))
                                })
                                .map(|path| path.to_path_buf());
                            if let Some((ti, ci)) = selected_clip_info {
                                let mut audio_changed = false;
                                if let Some(clip) = self.tracks.get_mut(ti).and_then(|t| t.clips.get_mut(ci)) {
                                    ui.label("Clip Properties");
                                    ui.add_space(6.0);
                                    ui.horizontal(|ui| {
                                        ui.label("Gain");
                                        if Self::colored_slider(ui, &mut clip.audio_gain, 0.0..=2.0, track_color)
                                            .changed()
                                        {
                                            audio_changed = true;
                                        }
                                    });
                                    ui.horizontal(|ui| {
                                        ui.label("Stretch");
                                        let prev = clip.audio_stretch_mode;
                                        let selected = match clip.audio_stretch_mode {
                                            AudioStretchMode::Stretch => "Stretch",
                                            AudioStretchMode::StretchFormant => "Stretch (Formant)",
                                            AudioStretchMode::StretchNeutral => "Stretch (Neutral)",
                                            AudioStretchMode::StretchVocal => "Vocal (Formant)",
                                            AudioStretchMode::Speed => "Speed (Resample)",
                                        };
                                        egui::ComboBox::from_id_source(("audio_stretch_mode_piano", clip.id))
                                            .selected_text(selected)
                                            .show_ui(ui, |ui| {
                                                ui.selectable_value(
                                                    &mut clip.audio_stretch_mode,
                                                    AudioStretchMode::Stretch,
                                                    "Stretch",
                                                );
                                                ui.selectable_value(
                                                    &mut clip.audio_stretch_mode,
                                                    AudioStretchMode::StretchFormant,
                                                    "Stretch (Formant)",
                                                );
                                                ui.selectable_value(
                                                    &mut clip.audio_stretch_mode,
                                                    AudioStretchMode::StretchNeutral,
                                                    "Stretch (Neutral)",
                                                );
                                                ui.selectable_value(
                                                    &mut clip.audio_stretch_mode,
                                                    AudioStretchMode::StretchVocal,
                                                    "Vocal (Formant)",
                                                );
                                                ui.selectable_value(
                                                    &mut clip.audio_stretch_mode,
                                                    AudioStretchMode::Speed,
                                                    "Speed (Resample)",
                                                );
                                            });
                                            if clip.audio_stretch_mode != prev {
                                                audio_changed = true;
                                            }
                                    });
                                    let rb_active = cfg!(has_rubberband)
                                        && clip.audio_stretch_mode != AudioStretchMode::Speed;
                                    let rb_label = if rb_active {
                                        "Rubber Band: Active"
                                    } else if cfg!(has_rubberband) {
                                        "Rubber Band: Off (Speed mode)"
                                    } else {
                                        "Rubber Band: Not linked"
                                    };
                                    ui.label(
                                        egui::RichText::new(rb_label).color(if rb_active {
                                            egui::Color32::from_rgb(120, 200, 140)
                                        } else {
                                            egui::Color32::from_gray(150)
                                        }),
                                    );
                                    ui.horizontal(|ui| {
                                        ui.label("Preserve Formant");
                                        let mut preserve = matches!(
                                            clip.audio_stretch_mode,
                                            AudioStretchMode::StretchFormant | AudioStretchMode::StretchVocal
                                        );
                                        let enabled = clip.audio_stretch_mode != AudioStretchMode::Speed;
                                        ui.add_enabled_ui(enabled, |ui| {
                                            if ui.checkbox(&mut preserve, "").changed() {
                                                clip.audio_stretch_mode = if preserve {
                                                    AudioStretchMode::StretchFormant
                                                } else {
                                                    AudioStretchMode::Stretch
                                                };
                                                    audio_changed = true;
                                            }
                                        });
                                    });
                                    ui.horizontal(|ui| {
                                        ui.label("Pitch");
                                        if Self::colored_slider(
                                            ui,
                                            &mut clip.audio_pitch_semitones,
                                            -24.0..=24.0,
                                            track_color,
                                        )
                                        .changed()
                                        {
                                            audio_changed = true;
                                        }
                                    });
                                    ui.horizontal(|ui| {
                                        ui.label("Formant");
                                        let enabled = clip.audio_stretch_mode != AudioStretchMode::Speed;
                                        ui.add_enabled_ui(enabled, |ui| {
                                            if Self::colored_slider(
                                                ui,
                                                &mut clip.audio_formant_scale,
                                                0.5..=2.0,
                                                track_color,
                                            )
                                            .changed()
                                            {
                                                audio_changed = true;
                                            }
                                        });
                                    });
                                        let (analyze_clicked, analysis_changed) =
                                            Self::draw_audio_analysis_controls(
                                        ui,
                                        clip,
                                        clip_path.as_deref(),
                                        key_display_format.as_str(),
                                        tempo_bpm,
                                        analyzing,
                                    );
                                        if analysis_changed {
                                            audio_changed = true;
                                        }
                                    if analyze_clicked {
                                        analyze_request = Some((clip.id, clip_path.clone()));
                                    }
                                    ui.horizontal(|ui| {
                                        ui.label("Time Mul");
                                        let mut time_mul = clip.audio_time_mul;
                                        let slider_changed = Self::colored_slider(
                                            ui,
                                            &mut time_mul,
                                            0.25..=4.0,
                                            track_color,
                                        )
                                        .changed();
                                        let input_changed = ui
                                            .add(egui::DragValue::new(&mut time_mul).speed(0.01))
                                            .changed();
                                        if slider_changed || input_changed {
                                            clip.audio_time_mul = time_mul.clamp(0.01, 8.0);
                                            audio_changed = true;
                                        }
                                    });
                                    ui.horizontal(|ui| {
                                        ui.label("Offset");
                                            if ui
                                                .add(
                                                    egui::DragValue::new(&mut clip.audio_offset_beats)
                                                        .speed(0.1),
                                                )
                                                .changed()
                                            {
                                                audio_changed = true;
                                            }
                                    });
                                    ui.add_space(6.0);
                                    if ui
                                        .add(egui::Button::image(
                                            egui::Image::new(egui::include_image!("../../../assets/icons/refresh-cw.svg"))
                                                .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                        ))
                                        .on_hover_text("Fit to project tempo")
                                        .clicked()
                                    {
                                        if let Some(source) = clip.audio_source_beats {
                                            if source > 0.0 && clip.length_beats > 0.0 {
                                                clip.audio_time_mul = source / clip.length_beats;
                                                    audio_changed = true;
                                            }
                                        }
                                    }
                                    if ui
                                        .add(egui::Button::image(
                                            egui::Image::new(egui::include_image!("../../../assets/icons/rotate-ccw.svg"))
                                                .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                        ))
                                        .on_hover_text("Reset Audio Props")
                                        .clicked()
                                    {
                                        clip.audio_gain = 1.0;
                                        clip.audio_pitch_semitones = 0.0;
                                        clip.audio_stretch_mode = AudioStretchMode::Stretch;
                                        clip.audio_time_mul = 1.0;
                                        clip.audio_offset_beats = 0.0;
                                        clip.audio_key = None;
                                        clip.audio_key_minor = false;
                                        clip.audio_key_source = None;
                                        clip.audio_bpm = None;
                                        clip.audio_fine_pitch_cents = 0.0;
                                        clip.audio_formant_scale = 1.0;
                                            audio_changed = true;
                                    }
                                }
                                    if audio_changed {
                                        self.refresh_audio_clip_timeline_if_running();
                                    }
                                if let Some((clip_id, clip_path)) = analyze_request.take() {
                                    if let Some(path) = clip_path {
                                        self.enqueue_audio_analysis(clip_id, path);
                                    } else {
                                        self.status = "Analyze failed: clip has no audio file".to_string();
                                    }
                                }
                            }
                        } else {
                            let track = self.selected_track.and_then(|i| self.tracks.get(i));
                            let name = track.map(|t| t.name.as_str()).unwrap_or("None");
                            ui.label(format!("Track: {name}"));
                            let plugin = track
                                .and_then(|t| t.instrument_path.as_deref())
                                .map(Self::plugin_display_name)
                                .unwrap_or_else(|| "No instrument".to_string());
                            ui.label(format!("Plugin: {plugin}"));
                            ui.add_space(6.0);
                            ui.label("Instrument");
                            ui.horizontal(|ui| {
                                let choose = ui
                                    .add(egui::Button::image(
                                        egui::Image::new(egui::include_image!("../../../assets/icons/folder-plus.svg"))
                                            .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                    ))
                                    .on_hover_text("Choose");
                                let open = ui
                                    .add(egui::Button::image(
                                        egui::Image::new(egui::include_image!("../../../assets/icons/external-link.svg"))
                                            .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                    ))
                                    .on_hover_text("Open UI");
                                let clear = ui
                                    .add(egui::Button::image(
                                        egui::Image::new(egui::include_image!("../../../assets/icons/x.svg"))
                                            .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                    ))
                                    .on_hover_text("Clear");
                                if let Some(index) = self.selected_track {
                                    if choose.clicked() {
                                        self.open_plugin_picker(PluginTarget::Instrument(index));
                                    }
                                    if open.clicked() {
                                        self.plugin_ui_target = Some(PluginUiTarget::Instrument(index));
                                        self.show_plugin_ui = true;
                                    }
                                    if clear.clicked() {
                                        if self.plugin_ui_matches(PluginUiTarget::Instrument(index)) {
                                            self.show_plugin_ui = false;
                                            self.destroy_plugin_ui();
                                        }
                                        if let Some(track) = self.tracks.get_mut(index) {
                                            track.instrument_path = None;
                                            track.params = default_midi_params();
                                            track.param_ids.clear();
                                            track.param_values.clear();
                                        }
                                        if let Some(state) = self.track_audio.get_mut(index) {
                                            if let Some(host) = state.host.take() {
                                                host.prepare_for_drop();
                                                self.orphaned_hosts.push(host);
                                            }
                                        }
                                        self.reinit_audio_if_running();
                                    }
                                }
                            });
                            ui.add_space(6.0);
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                self.refresh_clap_params_if_needed();
                                self.ensure_live_params();
                                let host_change = if let Some(PluginHostHandle::Vst3(host)) = self.selected_track_host() {
                                    if let Ok(mut host) = host.try_lock() {
                                        host.take_last_param_change()
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                };
                                let selected_track_index = self.selected_track;
                                let track_color = selected_track_index.map(|i| self.track_color(i));
                                let menu_color = selected_track_index
                                    .map(|index| self.track_color(index))
                                    .unwrap_or_else(|| egui::Color32::from_gray(200));
                                let mut pending_automation_record: Vec<(usize, RecordedAutomationPoint)> = Vec::new();
                                let mut pending_lane_delete: Option<(usize, usize)> = None;
                                let mut pending_status: Option<String> = None;
                                let mut pending_midi_learn: Option<(usize, u32, String)> = None;
                                let mut pending_active_lane: Option<(usize, usize)> = None;
                                if let Some(track) = selected_track_index.and_then(|i| self.tracks.get_mut(i)) {
                                    if let Some((param_id, value)) = host_change {
                                        if let Some(pos) = track.param_ids.iter().position(|id| *id == param_id) {
                                            track.param_values[pos] = value as f32;
                                            self.last_ui_param_change = Some((param_id, value as f32));
                                        }
                                    }
                                    if track.param_values.len() != track.params.len() {
                                        track.param_values.resize(track.params.len(), 0.0);
                                    }
                                    if let Some(program_index) = track
                                        .params
                                        .iter()
                                        .position(|name| {
                                            let name = name.to_ascii_lowercase();
                                            name.contains("program") || name.contains("preset")
                                        })
                                    {
                                        let current = (track.param_values[program_index] * 127.0)
                                            .round()
                                            .clamp(0.0, 127.0) as u8;
                                        let mut selected = current;
                                        egui::ComboBox::from_label("Preset")
                                            .selected_text(format!(
                                                "{:03} {}",
                                                selected + 1,
                                                gm_program_name(selected)
                                            ))
                                            .show_ui(ui, |ui| {
                                                for program in 0u8..=127 {
                                                    let label = format!(
                                                        "{:03} {}",
                                                        program + 1,
                                                        gm_program_name(program)
                                                    );
                                                    if ui
                                                        .selectable_label(program == selected, label)
                                                        .clicked()
                                                    {
                                                        selected = program;
                                                    }
                                                }
                                            });
                                        if selected != current {
                                            let value = (selected as f32 / 127.0).clamp(0.0, 1.0);
                                            track.param_values[program_index] = value;
                                            if let Some(param_id) = track.param_ids.get(program_index).copied() {
                                                if let Some(state) = selected_track_index
                                                    .and_then(|i| self.track_audio.get(i))
                                                {
                                                    if let Ok(mut pending) =
                                                        state.pending_param_changes.lock()
                                                    {
                                                        pending.push(PendingParamChange {
                                                            target: PendingParamTarget::Instrument,
                                                            param_id,
                                                            value: value as f64,
                                                        });
                                                    }
                                                }
                                                if self.is_recording && self.record_automation {
                                                    if let Some(track_index) = selected_track_index {
                                                        pending_automation_record.push((
                                                            track_index,
                                                            RecordedAutomationPoint {
                                                                param_id,
                                                                target: AutomationTarget::Instrument,
                                                                beat: self.playhead_beats,
                                                                value,
                                                            },
                                                        ));
                                                    }
                                                }
                                            }
                                        }
                                        ui.add_space(6.0);
                                    }
                                    for index in 0..track.params.len() {
                                        let label = track.params[index].clone();
                                        let value = &mut track.param_values[index];
                                        let slider = ui.push_id(format!("param_{}", label), |ui| {
                                            ui.horizontal(|ui| {
                                                ui.label(&label);
                                                Self::colored_slider(ui, value, 0.0..=1.0, track_color)
                                            })
                                            .inner
                                        });
                                        let response = slider.response;
                                        let slider_response = slider.inner;
                                        let changed = slider_response.changed()
                                            || slider_response.dragged()
                                            || response.dragged();
                                        if changed {
                                            let param_id = track.param_ids.get(index).copied();
                                            let debug_id = param_id.unwrap_or(u32::MAX);
                                            self.last_ui_param_change = Some((debug_id, *value));
                                            if let Some(param_id) = param_id {
                                                if let Some(state) = selected_track_index
                                                    .and_then(|i| self.track_audio.get(i))
                                                {
                                                    if let Some(PluginHostHandle::Vst3(host)) =
                                                        state.host.as_ref()
                                                    {
                                                        if let Ok(host) = host.try_lock() {
                                                            if let Some((channel, controller)) =
                                                                host.param_to_cc(param_id)
                                                            {
                                                                if let Ok(mut events) =
                                                                    state.midi_events.lock()
                                                                {
                                                                    let cc_value =
                                                                        (*value * 127.0).round() as i32;
                                                                    let cc_value =
                                                                        cc_value.clamp(0, 127) as u8;
                                                                    events.push(
                                                                        vst3::MidiEvent::control_change(
                                                                            channel,
                                                                            controller,
                                                                            cc_value,
                                                                        ),
                                                                    );
                                                                }
                                                            }
                                                        }
                                                    }
                                                    if let Ok(mut pending) =
                                                        state.pending_param_changes.lock()
                                                    {
                                                        pending.push(PendingParamChange {
                                                            target: PendingParamTarget::Instrument,
                                                            param_id,
                                                            value: *value as f64,
                                                        });
                                                    }
                                                }
                                                if self.is_recording && self.record_automation {
                                                    if let Some(track_index) = selected_track_index {
                                                        pending_automation_record.push((
                                                            track_index,
                                                            RecordedAutomationPoint {
                                                                param_id,
                                                                target: AutomationTarget::Instrument,
                                                                beat: self.playhead_beats,
                                                                value: *value,
                                                            },
                                                        ));
                                                    }
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
                                                    egui::RichText::new("MIDI Learn")
                                                        .color(menu_color),
                                                ))
                                                .clicked()
                                            {
                                                if let Some(param_id) = track.param_ids.get(index).copied() {
                                                    if let Some(track_index) = selected_track_index {
                                                        pending_midi_learn =
                                                            Some((track_index, param_id, label.clone()));
                                                    }
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
                                                if let Some(param_id) = track.param_ids.get(index).copied() {
                                                    if !track.automation_lanes.iter().any(|l| l.param_id == param_id) {
                                                        track.automation_lanes.push(AutomationLane {
                                                            name: label.clone(),
                                                            param_id,
                                                            target: AutomationTarget::Instrument,
                                                            points: Vec::new(),
                                                        });
                                                    }
                                                    if let Some(pos) = track
                                                        .automation_lanes
                                                        .iter()
                                                        .position(|l| l.param_id == param_id)
                                                    {
                                                        if let Some(track_index) = selected_track_index {
                                                            pending_active_lane = Some((track_index, pos));
                                                        }
                                                    }
                                                }
                                                ui.close_menu();
                                            }
                                        });
                                    }
                                }
                                if let Some(track) = selected_track_index.and_then(|i| self.tracks.get_mut(i)) {
                                    if ui
                                        .add(egui::Button::image(
                                            egui::Image::new(egui::include_image!("../../../assets/icons/shuffle.svg"))
                                                .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                        ))
                                        .on_hover_text("Randomize Params")
                                        .clicked()
                                    {
                                        let seed = std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .map(|d| d.as_nanos() as u64)
                                            .unwrap_or(0x1234_5678);
                                        let mut rng = seed;
                                        for idx in 0..track.param_values.len() {
                                            rng ^= rng << 13;
                                            rng ^= rng >> 7;
                                            rng ^= rng << 17;
                                            let value = (rng as f64 / u64::MAX as f64) as f32;
                                            track.param_values[idx] = value;
                                            if let Some(param_id) = track.param_ids.get(idx).copied() {
                                                if let Some(state) = selected_track_index
                                                    .and_then(|i| self.track_audio.get(i))
                                                {
                                                    if let Ok(mut pending) =
                                                        state.pending_param_changes.lock()
                                                    {
                                                        pending.push(PendingParamChange {
                                                            target: PendingParamTarget::Instrument,
                                                            param_id,
                                                            value: value as f64,
                                                        });
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    if !track.automation_lanes.is_empty() {
                                        ui.separator();
                                        ui.label("Automation Lanes");
                                        for (lane_index, lane) in track.automation_lanes.iter().enumerate() {
                                            ui.push_id(lane_index, |ui| {
                                            ui.horizontal(|ui| {
                                                let selected = selected_track_index
                                                    .and_then(|ti| self.automation_active.map(|(ai, li)| (ti, ai, li)))
                                                    .map(|(ti, ai, li)| ti == ai && li == lane_index)
                                                    .unwrap_or(false);
                                                let lane_response = ui.selectable_label(
                                                    selected,
                                                    format!("• {}", lane.name),
                                                );
                                                if lane_response.clicked() {
                                                    if let Some(track_index) = selected_track_index {
                                                        self.automation_active = Some((track_index, lane_index));
                                                    }
                                                }
                                                if ui
                                                    .add(egui::Button::image(
                                                        egui::Image::new(egui::include_image!("../../../assets/icons/trash-2.svg"))
                                                            .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                                    ))
                                                    .on_hover_text("Delete")
                                                    .clicked()
                                                {
                                                    if let Some(track_index) = selected_track_index {
                                                        pending_lane_delete = Some((track_index, lane_index));
                                                    }
                                                }
                                            });
                                            });
                                        }
                                    }
                                }
                                if let Some((track_index, param_id, label)) = pending_midi_learn.take() {
                                    if let Ok(mut learn) = self.midi_learn.lock() {
                                        *learn = Some((track_index, param_id));
                                    }
                                    pending_status =
                                        Some(format!("MIDI Learn armed for {}", label));
                                }
                                if let Some(status) = pending_status.take() {
                                    self.status = status;
                                }
                                if let Some(active) = pending_active_lane.take() {
                                    self.automation_active = Some(active);
                                }
                                for (track_index, point) in pending_automation_record {
                                    self.record_automation_point(
                                        track_index,
                                        point.target,
                                        point.param_id,
                                        point.beat,
                                        point.value,
                                    );
                                }
                                if let Some((track_index, lane_index)) = pending_lane_delete {
                                    if let Some(track) = self.tracks.get_mut(track_index) {
                                        if lane_index < track.automation_lanes.len() {
                                            track.automation_lanes.remove(lane_index);
                                        }
                                    }
                                    if let Some(state) = self.track_audio.get(track_index) {
                                        if let Ok(mut lanes) = state.automation_lanes.lock() {
                                            *lanes = self
                                                .tracks
                                                .get(track_index)
                                                .map(|t| t.automation_lanes.clone())
                                                .unwrap_or_default();
                                        }
                                    }
                                    if let Some((active_track, active_lane)) = self.automation_active {
                                        if active_track == track_index {
                                            if active_lane == lane_index {
                                                self.automation_active = None;
                                            } else if active_lane > lane_index {
                                                self.automation_active = Some((track_index, active_lane - 1));
                                            }
                                        }
                                    }
                                }
                            });
                        }
                    });
                }

                egui::CentralPanel::default().show_inside(ui, |ui| {
                    if !show_roll {
                        self.piano_roll_hovered = false;
                        self.piano_roll_rect = None;
                        ui.centered_and_justified(|ui| {
                            ui.label("Parameters");
                        });
                        return;
                    }
                    if is_audio_clip {
                        let (rect, response) =
                            ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
                        self.piano_roll_hovered = response.hovered();
                        self.piano_roll_rect = Some(rect);
                        let painter = ui.painter_at(rect);
                        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(12, 14, 16));
                        let preview_rect = rect.shrink2(egui::vec2(12.0, 28.0));
                        let selected_clip = selected_clip_info
                            .and_then(|(ti, ci)| self.tracks.get(ti).and_then(|t| t.clips.get(ci)));
                        let waveform = selected_clip.and_then(|clip| self.get_waveform_for_clip(clip));
                        let waveform_color =
                            selected_clip.and_then(|clip| self.get_waveform_color_for_clip(clip));
                        if let Some(clip) = selected_clip {
                            self.draw_audio_preview(
                                &painter,
                                preview_rect,
                                self.selected_clip.unwrap_or(0),
                                waveform.as_deref(),
                                waveform_color.as_deref(),
                                clip,
                                None,
                            );
                        }
                        let controls_rect = egui::Rect::from_min_max(
                            egui::pos2(rect.left() + 12.0, rect.bottom() - 24.0),
                            egui::pos2(rect.right() - 12.0, rect.bottom() - 6.0),
                        );
                        let mut x = controls_rect.left();
                        let button_w = 64.0;
                        let gap = 8.0;
                        let play_rect = egui::Rect::from_min_size(
                            egui::pos2(x, controls_rect.top()),
                            egui::vec2(button_w, controls_rect.height()),
                        );
                        x += button_w + gap;
                        let stop_rect = egui::Rect::from_min_size(
                            egui::pos2(x, controls_rect.top()),
                            egui::vec2(button_w, controls_rect.height()),
                        );
                        x += button_w + gap;
                        let loop_rect = egui::Rect::from_min_size(
                            egui::pos2(x, controls_rect.top()),
                            egui::vec2(button_w, controls_rect.height()),
                        );
                        if ui
                            .put(
                                play_rect,
                                egui::Button::image(
                                    egui::Image::new(egui::include_image!("../../../assets/icons/play.svg"))
                                        .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                ),
                            )
                            .on_hover_text("Play")
                            .clicked()
                        {
                            if let Some((ti, ci)) = selected_clip_info {
                                if let Some(clip) = self.tracks.get(ti).and_then(|t| t.clips.get(ci)).cloned() {
                                    if let Err(err) = self.start_audio_preview(&clip) {
                                        self.status = format!("Audio preview failed: {err}");
                                    } else {
                                        self.status = "Audio preview: play".to_string();
                                    }
                                }
                            }
                        }
                        if ui
                            .put(
                                stop_rect,
                                egui::Button::image(
                                    egui::Image::new(egui::include_image!("../../../assets/icons/stop-circle.svg"))
                                        .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                ),
                            )
                            .on_hover_text("Stop")
                            .clicked()
                        {
                            self.stop_audio_preview();
                            self.status = "Audio preview: stop".to_string();
                        }
                        let loop_label = if self.audio_preview_loop { "Loop On" } else { "Loop Off" };
                        if ui
                            .put(
                                loop_rect,
                                egui::Button::image(
                                    egui::Image::new(egui::include_image!("../../../assets/icons/repeat.svg"))
                                        .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                ),
                            )
                            .on_hover_text(loop_label)
                            .clicked()
                        {
                            self.audio_preview_loop = !self.audio_preview_loop;
                            if let Some((ti, ci)) = selected_clip_info {
                                if let Some(clip) = self.tracks.get(ti).and_then(|t| t.clips.get(ci)).cloned() {
                                    if self.audio_preview_sink.is_some() && self.audio_preview_clip_id == Some(clip.id) {
                                        let _ = self.start_audio_preview(&clip);
                                    }
                                }
                            }
                        }
                        return;
                    }
                    let total_size = ui.available_size();
                    let lane_height = 160.0;
                    let roll_height = (total_size.y - lane_height).max(80.0);
                    let lane_height = (total_size.y - roll_height).max(0.0);
                    let (roll_rect, roll_response) = ui.allocate_exact_size(
                        egui::vec2(total_size.x, roll_height),
                        egui::Sense::click_and_drag(),
                    );
                    let (lane_rect, lane_response) = ui.allocate_exact_size(
                        egui::vec2(total_size.x, lane_height),
                        egui::Sense::click_and_drag(),
                    );
                    self.piano_roll_hovered = roll_response.hovered();
                    let keyboard_w = 56.0;
                    let header_height = 20.0;
                    let header_rect = egui::Rect::from_min_max(
                        egui::pos2(roll_rect.left(), roll_rect.top()),
                        egui::pos2(roll_rect.right(), roll_rect.top() + header_height),
                    );
                    let keyboard_rect = egui::Rect::from_min_max(
                        egui::pos2(roll_rect.left(), header_rect.bottom()),
                        egui::pos2(roll_rect.left() + keyboard_w, roll_rect.bottom()),
                    );
                    let roll_rect = egui::Rect::from_min_max(
                        egui::pos2(keyboard_rect.right(), header_rect.bottom()),
                        roll_rect.max,
                    );
                    let pointer_pos = ctx.input(|i| i.pointer.hover_pos());
                    let pointer_interact = ctx.input(|i| i.pointer.interact_pos());
                    let pointer_down = ctx.input(|i| i.pointer.primary_down());
                    let pointer_clicked = ctx.input(|i| i.pointer.primary_clicked());
                    let pointer_released = ctx.input(|i| i.pointer.any_released());
                    let ctrl_down = ctx.input(|i| i.modifiers.ctrl);
                    let over_header = pointer_pos
                        .map(|pos| header_rect.contains(pos))
                        .unwrap_or(false);
                    let roll_or_header_hovered = roll_response.hovered()
                        || lane_response.hovered()
                        || over_header;
                    self.piano_roll_hovered = roll_or_header_hovered;
                    let piano_rect = egui::Rect::from_min_max(
                        egui::pos2(keyboard_rect.left(), header_rect.top()),
                        egui::pos2(roll_rect.right(), lane_rect.bottom().max(roll_rect.bottom())),
                    );
                    self.piano_roll_rect = Some(piano_rect);
                    let zoom_min_x = 0.4;
                    let zoom_max_x = 6.0;
                    let zoom_min_y = 0.4;
                    let zoom_max_y = 4.0;
                    if roll_or_header_hovered {
                        let input = ctx.input(|i| i.clone());
                        let mmb_down = input.pointer.button_down(egui::PointerButton::Middle);
                        if mmb_down {
                            self.piano_pan += input.pointer.delta();
                            roll_response.clone().on_hover_cursor(egui::CursorIcon::Move);
                        } else if input.modifiers.ctrl {
                            let pointer_x = pointer_pos
                                .or(pointer_interact)
                                .map(|pos| pos.x)
                                .unwrap_or(roll_rect.left());
                            let local_x = pointer_x - roll_rect.left();
                            let before_zoom = self.piano_zoom_x;
                            let zoom = input.zoom_delta();
                            if (zoom - 1.0).abs() > f32::EPSILON {
                                self.piano_zoom_x =
                                    (self.piano_zoom_x * zoom).clamp(zoom_min_x, zoom_max_x);
                            } else {
                                let mut delta = input.smooth_scroll_delta;
                                if delta == egui::Vec2::ZERO {
                                    delta = input.raw_scroll_delta;
                                }
                                let zoom_delta = (delta.x + delta.y) * 0.001;
                                self.piano_zoom_x =
                                    (self.piano_zoom_x + zoom_delta).clamp(zoom_min_x, zoom_max_x);
                            }
                            let scale = if before_zoom > 0.0 {
                                self.piano_zoom_x / before_zoom
                            } else {
                                1.0
                            };
                            self.piano_pan.x = (self.piano_pan.x - local_x) * scale + local_x;
                        } else if input.modifiers.shift {
                            let mut delta = input.smooth_scroll_delta;
                            if delta == egui::Vec2::ZERO {
                                delta = input.raw_scroll_delta;
                            }
                            let pan_delta = if delta.x.abs() > f32::EPSILON {
                                delta.x
                            } else if delta.y.abs() > f32::EPSILON {
                                delta.y
                            } else {
                                0.0
                            };
                            self.piano_pan.x += pan_delta;
                        } else {
                            let mut delta = input.smooth_scroll_delta;
                            if delta == egui::Vec2::ZERO {
                                delta = input.raw_scroll_delta;
                            }
                            if delta.x.abs() > f32::EPSILON {
                                self.piano_pan.x += delta.x;
                            }
                            if delta.y.abs() > f32::EPSILON {
                                self.piano_pan.y += delta.y;
                            }
                        }
                    }
                    let paint_rect = egui::Rect::from_min_max(
                        egui::pos2(keyboard_rect.left(), header_rect.top()),
                        egui::pos2(roll_rect.right(), roll_rect.bottom()),
                    );
                    let painter = ui.painter_at(paint_rect);
                    painter.rect_filled(roll_rect, 0.0, egui::Color32::from_rgb(12, 14, 16));
                    painter.rect_filled(keyboard_rect, 0.0, egui::Color32::from_rgb(10, 12, 14));
                    let beat_width = 24.0 * self.piano_zoom_x;
                    let note_height = 10.0 * self.piano_zoom_y;
                    if let Some(focus) = self.piano_focus_beats.take() {
                        let center_x = roll_rect.width() * 0.5;
                        self.piano_pan.x = center_x - focus * beat_width;
                    }
                    let handle_size = 14.0;
                    let handle_rect = egui::Rect::from_min_size(
                        egui::pos2(roll_rect.right() - handle_size - 4.0, roll_rect.top() + 4.0),
                        egui::vec2(handle_size, handle_size),
                    );
                    let handle_id = egui::Id::new("piano_zoom_handle");
                    let handle_resp =
                        ui.interact(handle_rect, handle_id, egui::Sense::click_and_drag());
                    painter.rect_filled(handle_rect, 2.0, egui::Color32::from_rgb(26, 30, 36));
                    painter.rect_stroke(
                        handle_rect,
                        2.0,
                        egui::Stroke::new(1.0, egui::Color32::from_rgb(70, 90, 120)),
                    );
                    if handle_resp.hovered() {
                        roll_response.clone().on_hover_cursor(egui::CursorIcon::ResizeRow);
                    }
                    if handle_resp.drag_started() {
                        if let Some(pos) = handle_resp.interact_pointer_pos().or(pointer_interact) {
                            self.piano_zoom_drag = Some(PianoZoomDragState {
                                start_pos: pos,
                                start_zoom_x: self.piano_zoom_x,
                                start_zoom_y: self.piano_zoom_y,
                            });
                        }
                    }
                    if handle_resp.dragged() {
                        if let Some(drag) = &self.piano_zoom_drag {
                            if let Some(pos) = handle_resp.interact_pointer_pos().or(pointer_interact) {
                                let delta = pos - drag.start_pos;
                                let scale_x = (1.0 + delta.x * 0.005).max(0.05);
                                let scale_y = (1.0 + delta.y * 0.005).max(0.05);
                                self.piano_zoom_x =
                                    (drag.start_zoom_x * scale_x).clamp(zoom_min_x, zoom_max_x);
                                self.piano_zoom_y =
                                    (drag.start_zoom_y * scale_y).clamp(zoom_min_y, zoom_max_y);
                            }
                        }
                    }
                    if handle_resp.drag_stopped() {
                        self.piano_zoom_drag = None;
                    }
                    let clip_offset = selected_clip_info
                        .and_then(|(ti, ci)| self.tracks.get(ti).and_then(|t| t.clips.get(ci)))
                        .filter(|clip| clip.is_midi)
                        .map(|clip| clip.start_beats)
                        .unwrap_or(0.0);
                    let pos_to_local = |x: f32, pan: f32| (x - roll_rect.left() - pan) / beat_width;
                    let pos_to_abs = |x: f32, pan: f32| pos_to_local(x, pan) + clip_offset;
                    let header_id = egui::Id::new("piano_roll_timeline");
                    let header_response = ui.interact(header_rect, header_id, egui::Sense::click());
                    if header_response.clicked() {
                        if let Some(pos) = header_response.interact_pointer_pos() {
                            let local = self.beats_from_pos(
                                pos.x,
                                roll_rect.left() + self.piano_pan.x,
                                beat_width,
                            );
                            self.seek_playhead(local + clip_offset);
                        }
                    }
                    let snap_grid = self.piano_snap.max(0.03125);
                    if snap_grid < 1.0 {
                        let minor_step = beat_width * snap_grid;
                        if minor_step >= 6.0 {
                            let mut minor_x = roll_rect.left() + self.piano_pan.x;
                            while minor_x <= roll_rect.right() {
                                painter.line_segment(
                                    [
                                        egui::pos2(minor_x, roll_rect.top()),
                                        egui::pos2(minor_x, roll_rect.bottom()),
                                    ],
                                    egui::Stroke::new(
                                        1.0,
                                        egui::Color32::from_rgba_premultiplied(14, 16, 20, 120),
                                    ),
                                );
                                minor_x += minor_step;
                            }
                        }
                    }
                    let mut x = roll_rect.left() + self.piano_pan.x;
                    let mut beat_idx = 0;
                    while x <= roll_rect.right() {
                        let major = beat_idx % 4 == 0;
                        let color = if major {
                            egui::Color32::from_rgba_premultiplied(26, 28, 32, 180)
                        } else {
                            egui::Color32::from_rgba_premultiplied(18, 20, 24, 160)
                        };
                        painter.line_segment(
                            [egui::pos2(x, roll_rect.top()), egui::pos2(x, roll_rect.bottom())],
                            egui::Stroke::new(1.0, color),
                        );
                        beat_idx += 1;
                        x += beat_width;
                    }
                    for note in 0u8..=127 {
                        let y = roll_rect.bottom() + self.piano_pan.y
                            - (note as f32 - 40.0) * note_height;
                        if y < roll_rect.top() || y > roll_rect.bottom() {
                            continue;
                        }
                        let is_c = note % 12 == 0;
                        let grid_color = if is_c {
                            egui::Color32::from_rgba_premultiplied(60, 64, 72, 220)
                        } else {
                            egui::Color32::from_rgba_premultiplied(20, 22, 26, 160)
                        };
                        let grid_width = if is_c { 1.6 } else { 1.0 };
                        painter.line_segment(
                            [egui::pos2(roll_rect.left(), y), egui::pos2(roll_rect.right(), y)],
                            egui::Stroke::new(grid_width, grid_color),
                        );
                    }
                    let mut hovered_key: Option<u8> = None;
                    let mut hovered_key_vel: Option<u8> = None;
                    for note in 0u8..=127 {
                        let y = roll_rect.bottom() + self.piano_pan.y
                            - (note as f32 - 40.0) * note_height;
                        let key_rect = egui::Rect::from_min_max(
                            egui::pos2(keyboard_rect.left(), y - note_height),
                            egui::pos2(keyboard_rect.right(), y),
                        );
                        if key_rect.bottom() < roll_rect.top() || key_rect.top() > roll_rect.bottom() {
                            continue;
                        }
                        if let Some(pos) = pointer_interact {
                            if key_rect.contains(pos) {
                                hovered_key = Some(note);
                                let t = ((pos.x - keyboard_rect.left()) / keyboard_rect.width())
                                    .clamp(0.0, 1.0);
                                let vel = (t * 127.0).round().clamp(1.0, 127.0) as u8;
                                hovered_key_vel = Some(vel);
                            }
                        }
                        let is_black = matches!(note % 12, 1 | 3 | 6 | 8 | 10);
                        let key_color = if is_black {
                            egui::Color32::from_rgb(24, 26, 30)
                        } else {
                            egui::Color32::from_rgb(200, 200, 200)
                        };
                        painter.rect_filled(key_rect, 0.0, key_color);
                        painter.rect_stroke(
                            key_rect,
                            0.0,
                            egui::Stroke::new(1.0, egui::Color32::from_rgb(8, 10, 12)),
                        );
                        let is_c = note % 12 == 0;
                        if is_c {
                            let octave = (note / 12) as i32 - 1;
                            Self::outlined_text(
                                &painter,
                                egui::pos2(keyboard_rect.left() + 4.0, y - note_height + 2.0),
                                egui::Align2::LEFT_TOP,
                                &format!("C{octave}"),
                                egui::FontId::proportional(9.0),
                                egui::Color32::from_gray(120),
                            );
                        }
                    }
                    if let Some(note) = hovered_key {
                        if ctrl_down && pointer_clicked {
                            if let Some(clip_id) = self.selected_clip {
                                if let Some((track_index, clip_index)) =
                                    self.find_clip_indices_by_id(clip_id)
                                {
                                    if let Some(clip) =
                                        self.tracks.get(track_index).and_then(|t| t.clips.get(clip_index))
                                    {
                                        self.piano_selected.clear();
                                        for (index, data) in clip.midi_notes.iter().enumerate() {
                                            if data.midi_note == note {
                                                self.piano_selected.insert(index);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if !ctrl_down {
                        if pointer_down {
                            match hovered_key {
                                Some(note) => {
                                    if self.piano_key_down != Some(note) {
                                        if let Some(prev) = self.piano_key_down {
                                            self.piano_preview_note_off(prev);
                                        }
                                        let vel = hovered_key_vel.unwrap_or(100);
                                        self.piano_preview_note_on(note, vel);
                                        self.piano_key_down = Some(note);
                                    }
                                }
                                None => {
                                    if let Some(prev) = self.piano_key_down {
                                        self.piano_preview_note_off(prev);
                                        self.piano_key_down = None;
                                    }
                                }
                            }
                        }
                        if pointer_released {
                            if let Some(prev) = self.piano_key_down {
                                self.piano_preview_note_off(prev);
                                self.piano_key_down = None;
                            }
                        }
                    }
                    let pointer_pos = roll_response
                        .interact_pointer_pos()
                        .or_else(|| roll_response.hover_pos());
                    let mut hovered_note: Option<(usize, egui::Rect)> = None;
                    let mut hovered_note_edge = false;
                    if let Some(clip_id) = self.selected_clip {
                        if let Some((track_index, clip_index)) = self.find_clip_indices_by_id(clip_id) {
                            if let Some(clip) =
                                self.tracks.get(track_index).and_then(|t| t.clips.get(clip_index))
                            {
                                if !clip.midi_notes.is_empty() {
                                    let visible_start_beats = clip_offset - (self.piano_pan.x / beat_width) - 1.0;
                                    let visible_end_beats = visible_start_beats + (roll_rect.width() / beat_width) + 2.0;

                                    for (index, note) in clip.midi_notes.iter().enumerate() {
                                        if note.start_beats + note.length_beats < visible_start_beats || note.start_beats > visible_end_beats {
                                            continue;
                                        }
                                        let local_start = note.start_beats - clip_offset;
                                        let x = roll_rect.left() + self.piano_pan.x + local_start * beat_width;
                                        let w = (note.length_beats * beat_width).max(12.0);
                                        if x + w < roll_rect.left() || x > roll_rect.right() {
                                            continue;
                                        }
                                        let y = roll_rect.bottom() + self.piano_pan.y
                                            - (note.midi_note as f32 - 40.0) * note_height;
                                        let note_rect = egui::Rect::from_min_size(
                                            egui::pos2(x, y - note_height),
                                            egui::vec2(w, note_height),
                                        );
                                        let roygbiv = [
                                            egui::Color32::from_rgb(230, 70, 60),
                                            egui::Color32::from_rgb(240, 140, 60),
                                            egui::Color32::from_rgb(235, 210, 80),
                                            egui::Color32::from_rgb(90, 210, 120),
                                            egui::Color32::from_rgb(80, 170, 220),
                                            egui::Color32::from_rgb(90, 120, 230),
                                            egui::Color32::from_rgb(160, 90, 220),
                                            egui::Color32::from_rgb(210, 70, 200),
                                        ];
                                        let durations = [
                                            1.0 / 32.0,
                                            1.0 / 24.0,
                                            1.0 / 16.0,
                                            3.0 / 32.0,
                                            1.0 / 12.0,
                                            1.0 / 8.0,
                                            3.0 / 16.0,
                                            1.0 / 6.0,
                                            1.0 / 4.0,
                                            3.0 / 8.0,
                                            1.0 / 3.0,
                                            1.0 / 2.0,
                                            3.0 / 4.0,
                                            2.0 / 3.0,
                                            1.0,
                                            1.5,
                                            4.0 / 3.0,
                                            2.0,
                                            3.0,
                                            8.0 / 3.0,
                                            4.0,
                                        ];
                                        let len = note.length_beats.max(1.0 / 32.0).min(4.0);
                                        let mut nearest = 0usize;
                                        let mut best = f32::MAX;
                                        for (idx, value) in durations.iter().enumerate() {
                                            let delta = (len - value).abs();
                                            if delta < best {
                                                best = delta;
                                                nearest = idx;
                                            }
                                        }
                                        let t = if durations.len() > 1 {
                                            nearest as f32 / (durations.len() - 1) as f32
                                        } else {
                                            0.0
                                        };
                                        let color_idx = (t * (roygbiv.len() - 1) as f32)
                                            .round()
                                            .clamp(0.0, (roygbiv.len() - 1) as f32) as usize;
                                        let base = roygbiv[color_idx];
                                        let vel = (note.velocity as f32 / 127.0).clamp(0.0, 1.0);
                                        let alpha = (vel * 200.0 + 30.0).clamp(40.0, 230.0) as u8;
                                        let pan = note.pan.clamp(-1.0, 1.0);
                                        let pan_red = (pan.max(0.0) * 80.0) as u8;
                                        let pan_blue = ((-pan).max(0.0) * 80.0) as u8;
                                        let cutoff_green = (note.cutoff.clamp(0.0, 1.0) * 80.0) as u8;
                                        let r = (base.r() as u16 + pan_red as u16).min(255) as u8;
                                        let g = (base.g() as u16 + cutoff_green as u16).min(255) as u8;
                                        let b = (base.b() as u16 + pan_blue as u16).min(255) as u8;
                                        let color = egui::Color32::from_rgba_premultiplied(r, g, b, alpha);
                                        painter.rect_filled(note_rect, 0.0, color);
                                        if self.show_hitboxes {
                                            let edge_pad = 8.0;
                                            let edge_rect = egui::Rect::from_min_max(
                                                egui::pos2(note_rect.right() - edge_pad, note_rect.top()),
                                                egui::pos2(note_rect.right() + edge_pad, note_rect.bottom()),
                                            );
                                            painter.rect_stroke(
                                                note_rect,
                                                0.0,
                                                egui::Stroke::new(
                                                    1.0,
                                                    egui::Color32::from_rgba_premultiplied(80, 200, 255, 180),
                                                ),
                                            );
                                            painter.rect_stroke(
                                                edge_rect,
                                                0.0,
                                                egui::Stroke::new(
                                                    1.0,
                                                    egui::Color32::from_rgba_premultiplied(255, 120, 80, 200),
                                                ),
                                            );
                                        }
                                        if self.piano_selected.contains(&index) {
                                            painter.rect_stroke(
                                                note_rect,
                                                0.0,
                                                egui::Stroke::new(1.4, egui::Color32::from_rgb(230, 240, 255)),
                                            );
                                        }
                                        if let Some(pos) = pointer_pos {
                                            if pos.x >= roll_rect.left() {
                                                let edge_pad = 8.0;
                                                let edge_rect = egui::Rect::from_min_max(
                                                    egui::pos2(note_rect.right() - edge_pad, note_rect.top()),
                                                    egui::pos2(note_rect.right() + edge_pad, note_rect.bottom()),
                                                );
                                                if edge_rect.contains(pos) {
                                                    hovered_note_edge = true;
                                                    hovered_note = Some((index, note_rect));
                                                } else if note_rect.contains(pos) {
                                                    hovered_note_edge = false;
                                                    hovered_note = Some((index, note_rect));
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if let Some((_, note_rect)) = hovered_note {
                        if let Some(pos) = pointer_pos {
                            if pos.x >= roll_rect.left() {
                                let right_edge = note_rect.right();
                                let edge_pad = 8.0;
                                let icon = if (right_edge - pos.x).abs() <= edge_pad {
                                    egui::CursorIcon::ResizeHorizontal
                                } else {
                                    egui::CursorIcon::Grab
                                };
                                roll_response.clone().on_hover_cursor(icon);
                            }
                        }
                    }

                    let needs_clip_hint = match self.selected_clip {
                        None => true,
                        Some(clip_id) => self
                            .find_clip_indices_by_id(clip_id)
                            .and_then(|(track_index, clip_index)| {
                                self.tracks
                                    .get(track_index)
                                    .and_then(|t| t.clips.get(clip_index))
                            })
                            .map(|clip| !clip.is_midi)
                            .unwrap_or(true),
                    };
                    if needs_clip_hint {
                        Self::outlined_text(
                            &painter,
                            roll_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "Select a MIDI clip to edit",
                            egui::FontId::proportional(14.0),
                            egui::Color32::from_gray(160),
                        );
                    }

                    let input = ctx.input(|i| i.clone());
                    let ctrl = input.modifiers.ctrl;
                    let box_select_active = input.key_down(egui::Key::B);
                    let shift = input.modifiers.shift;
                    let alt = input.modifiers.alt;
                    let mut marquee_rect: Option<egui::Rect> = None;
                    let mut scale_handle_active = self.piano_scale_drag.is_some();
                    let mut scale_handle_stopped = false;
                    let mut scale_handle_hot = false;
                    if alt && !self.piano_selected.is_empty() {
                        if let Some(clip_id) = self.selected_clip {
                            if let Some((track_index, clip_index)) =
                                self.find_clip_indices_by_id(clip_id)
                            {
                                if let Some(clip) = self
                                    .tracks
                                    .get(track_index)
                                    .and_then(|t| t.clips.get(clip_index))
                                {
                                    let mut min_start = f32::MAX;
                                    let mut max_end = 0.0f32;
                                    let mut min_y = f32::MAX;
                                    let mut max_y = 0.0f32;
                                    let mut selected_notes = Vec::new();
                                    for index in self.piano_selected.iter().copied() {
                                        if let Some(note) = clip.midi_notes.get(index) {
                                            min_start = min_start.min(note.start_beats);
                                            max_end = max_end.max(note.start_beats + note.length_beats);
                                            let y = roll_rect.bottom() + self.piano_pan.y
                                                - (note.midi_note as f32 - 40.0) * note_height;
                                            let y_min = y - note_height;
                                            min_y = min_y.min(y_min);
                                            max_y = max_y.max(y);
                                            selected_notes.push((
                                                index,
                                                note.start_beats,
                                                note.midi_note,
                                                note.length_beats,
                                            ));
                                        }
                                    }
                                    if min_start.is_finite() && max_end > min_start {
                                        let bounds_left = roll_rect.left()
                                            + self.piano_pan.x
                                            + (min_start - clip_offset) * beat_width;
                                        let bounds_right = roll_rect.left()
                                            + self.piano_pan.x
                                            + (max_end - clip_offset) * beat_width;
                                        let bounds_rect = egui::Rect::from_min_max(
                                            egui::pos2(bounds_left, min_y),
                                            egui::pos2(bounds_right, max_y),
                                        );
                                        painter.rect_stroke(
                                            bounds_rect,
                                            2.0,
                                            egui::Stroke::new(1.4, egui::Color32::from_rgb(235, 240, 245)),
                                        );
                                        let handle_x = roll_rect.left()
                                            + self.piano_pan.x
                                            + (max_end - clip_offset) * beat_width;
                                        let handle_y = (min_y + max_y) * 0.5;
                                        let handle_rect = egui::Rect::from_center_size(
                                            egui::pos2(handle_x, handle_y),
                                            egui::vec2(12.0, 12.0),
                                        );
                                        painter.circle_filled(
                                            handle_rect.center(),
                                            5.0,
                                            egui::Color32::from_rgb(210, 230, 255),
                                        );
                                        painter.circle_stroke(
                                            handle_rect.center(),
                                            5.0,
                                            egui::Stroke::new(1.0, egui::Color32::from_rgb(40, 60, 90)),
                                        );
                                        let handle_id = egui::Id::new("piano_scale_handle");
                                        let handle_resp =
                                            ui.interact(handle_rect, handle_id, egui::Sense::click_and_drag());
                                        let handle_hover = pointer_interact
                                            .or(pointer_pos)
                                            .map(|pos| handle_rect.contains(pos))
                                            .unwrap_or(false);
                                        scale_handle_hot = handle_hover || handle_resp.hovered();
                                        if scale_handle_hot {
                                            roll_response
                                                .clone()
                                                .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
                                        }
                                        if handle_resp.drag_started()
                                            || (handle_hover && roll_response.drag_started())
                                        {
                                            self.push_undo_state();
                                            self.piano_scale_drag = Some(PianoScaleDragState {
                                                track_index,
                                                anchor_start: min_start,
                                                anchor_end: max_end,
                                                selected_notes,
                                            });
                                            scale_handle_active = true;
                                        }
                                        if handle_resp.dragged() || (handle_hover && roll_response.dragged()) {
                                            scale_handle_active = true;
                                        }
                                        if handle_resp.drag_stopped() || (handle_hover && roll_response.drag_stopped()) {
                                            scale_handle_stopped = true;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if (ctrl || box_select_active) && roll_response.drag_started() {
                        if let Some(pos) = roll_response.interact_pointer_pos() {
                            if pos.x >= roll_rect.left() {
                                self.piano_marquee_start = Some(pos);
                                self.piano_marquee_add = shift;
                            }
                        }
                    }
                    if let Some(start) = self.piano_marquee_start {
                        if roll_response.dragged() {
                            if let Some(pos) = roll_response
                                .interact_pointer_pos()
                                .or(pointer_interact)
                            {
                                marquee_rect = Some(egui::Rect::from_two_pos(start, pos));
                            }
                        }
                        if roll_response.drag_stopped() {
                            if let Some(end) = roll_response.interact_pointer_pos() {
                                let select_rect = egui::Rect::from_two_pos(start, end);
                                if let Some(clip_id) = self.selected_clip {
                                    if let Some((track_index, clip_index)) =
                                        self.find_clip_indices_by_id(clip_id)
                                    {
                                        if let Some(clip) = self
                                            .tracks
                                            .get(track_index)
                                            .and_then(|t| t.clips.get(clip_index))
                                        {
                                            let mut hits: Vec<usize> = Vec::new();
                                            for (index, note) in clip.midi_notes.iter().enumerate() {
                                                let x = roll_rect.left()
                                                    + self.piano_pan.x
                                                    + (note.start_beats - clip_offset) * beat_width;
                                                let y = roll_rect.bottom() + self.piano_pan.y
                                                    - (note.midi_note as f32 - 40.0) * note_height;
                                                let w = (note.length_beats * beat_width).max(12.0);
                                                let note_rect = egui::Rect::from_min_size(
                                                    egui::pos2(x, y - note_height),
                                                    egui::vec2(w, note_height),
                                                );
                                                if select_rect.intersects(note_rect) {
                                                    hits.push(index);
                                                }
                                            }
                                            if !self.piano_marquee_add {
                                                self.piano_selected.clear();
                                            }
                                            for index in hits {
                                                self.piano_selected.insert(index);
                                            }
                                        }
                                    }
                                }
                            }
                            self.piano_marquee_start = None;
                            self.piano_marquee_add = false;
                        }
                    }

                    let quantize = self.piano_snap.max(0.03125);
                    if ctrl && roll_response.clicked_by(egui::PointerButton::Primary) {
                        if let Some(pos) = roll_response.interact_pointer_pos() {
                            if pos.x < roll_rect.left() {
                                return;
                            }
                            if let Some((note_index, _)) = hovered_note {
                                if !shift {
                                    self.piano_selected.clear();
                                }
                                self.piano_selected.insert(note_index);
                            } else if !shift {
                                self.piano_selected.clear();
                            }
                        }
                    } else if !box_select_active
                        && self.piano_tool == PianoTool::Pencil
                        && roll_response.clicked_by(egui::PointerButton::Primary)
                    {
                        if let Some(pos) = roll_response.interact_pointer_pos() {
                            if pos.x < roll_rect.left() {
                                return;
                            }
                            if let Some(clip_id) = self.selected_clip {
                                if let Some((track_index, clip_index)) =
                                    self.find_clip_indices_by_id(clip_id)
                                {
                                    if let Some(clip) = self
                                        .tracks
                                        .get_mut(track_index)
                                        .and_then(|t| t.clips.get_mut(clip_index))
                                    {
                                        if clip.is_midi && hovered_note.is_none() {
                                            let local = pos_to_local(pos.x, self.piano_pan.x);
                                            let snapped_local = if alt {
                                                local
                                            } else {
                                                (local / quantize).round() * quantize
                                            };
                                            let snapped = (snapped_local + clip_offset).max(0.0);
                                            let pitch_f =
                                                (roll_rect.bottom() + self.piano_pan.y - pos.y) / note_height;
                                            let pitch = (40.0 + pitch_f).floor() as i32;
                                            let pitch = pitch.clamp(0, 127) as u8;
                                            if shift {
                                                clip.midi_notes.retain(|note| {
                                                    note.midi_note != pitch
                                                        || note.start_beats + note.length_beats <= snapped
                                                        || note.start_beats >= snapped + self.piano_note_len
                                                });
                                            }
                                            clip.midi_notes.push(PianoRollNote::new(
                                                snapped,
                                                self.piano_note_len,
                                                pitch,
                                                100,
                                            ));
                                            if let Some(index) = clip.midi_notes.len().checked_sub(1) {
                                                self.piano_selected.clear();
                                                self.piano_selected.insert(index);
                                            }
                                            self.sync_track_audio_notes(track_index);
                                            self.sync_linked_notes_after_edit(track_index);
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if roll_response.clicked_by(egui::PointerButton::Secondary) {
                        if let Some((note_index, _)) = hovered_note {
                            if let Some(clip_id) = self.selected_clip {
                                if let Some((track_index, clip_index)) =
                                    self.find_clip_indices_by_id(clip_id)
                                {
                                    if let Some(clip) = self
                                        .tracks
                                        .get_mut(track_index)
                                        .and_then(|t| t.clips.get_mut(clip_index))
                                    {
                                        if note_index < clip.midi_notes.len() {
                                            clip.midi_notes.remove(note_index);
                                            self.piano_selected.remove(&note_index);
                                            let shifted: HashSet<usize> = self
                                                .piano_selected
                                                .iter()
                                                .map(|idx| if *idx > note_index { idx - 1 } else { *idx })
                                                .collect();
                                            self.piano_selected = shifted;
                                            self.sync_track_audio_notes(track_index);
                                            self.sync_linked_notes_after_edit(track_index);
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if (roll_response.drag_started() || (pointer_clicked && hovered_note_edge))
                        && !ctrl
                        && !scale_handle_active
                        && !scale_handle_hot
                    {
                        if let Some((note_index, _note_rect)) = hovered_note {
                            if let Some(pos) = roll_response.interact_pointer_pos() {
                                if pos.x < roll_rect.left() {
                                    return;
                                }
                                if !self.piano_selected.contains(&note_index) {
                                    self.piano_selected.clear();
                                    self.piano_selected.insert(note_index);
                                }
                                let kind = if shift {
                                    PianoDragKind::Move
                                } else if hovered_note_edge {
                                    PianoDragKind::Resize
                                } else {
                                    PianoDragKind::Move
                                };
                                let offset_beats = pos_to_abs(pos.x, self.piano_pan.x);
                                let shift_copy = shift;
                                if shift_copy {
                                    self.push_undo_state();
                                }
                                if let Some(clip_id) = self.selected_clip {
                                    if let Some((track_index, clip_index)) =
                                        self.find_clip_indices_by_id(clip_id)
                                    {
                                        if let Some(clip) = self
                                            .tracks
                                            .get_mut(track_index)
                                            .and_then(|t| t.clips.get_mut(clip_index))
                                        {
                                            if shift_copy {
                                                let mut selection: Vec<usize> =
                                                    self.piano_selected.iter().copied().collect();
                                                selection.sort_unstable();
                                                if selection.is_empty() {
                                                    selection.push(note_index);
                                                }
                                                let base_len = clip.midi_notes.len();
                                                let mut new_indices = Vec::new();
                                                for idx in selection.iter().copied() {
                                                    if let Some(note) = clip.midi_notes.get(idx).cloned() {
                                                        clip.midi_notes.push(note);
                                                        new_indices.push(base_len + new_indices.len());
                                                    }
                                                }
                                                self.piano_selected.clear();
                                                for idx in &new_indices {
                                                    self.piano_selected.insert(*idx);
                                                }
                                            }

                                            let (start_beats, start_length, start_pitch) = clip
                                                .midi_notes
                                                .get(note_index)
                                                .map(|note| (note.start_beats, note.length_beats, note.midi_note))
                                                .unwrap_or((0.0, self.piano_note_len.max(0.03125), 60));
                                            let mut selected_notes = Vec::new();
                                            for index in self.piano_selected.iter().copied() {
                                                if let Some(note) = clip.midi_notes.get(index) {
                                                    selected_notes.push((
                                                        index,
                                                        note.start_beats,
                                                        note.midi_note,
                                                        note.length_beats,
                                                    ));
                                                }
                                            }
                                            if selected_notes.is_empty() {
                                                if let Some(note) = clip.midi_notes.get(note_index) {
                                                    selected_notes.push((
                                                        note_index,
                                                        note.start_beats,
                                                        note.midi_note,
                                                        note.length_beats,
                                                    ));
                                                }
                                            }
                                            let primary_index =
                                                selected_notes.first().map(|v| v.0).unwrap_or(note_index);
                                            let primary = clip
                                                .midi_notes
                                                .get(primary_index)
                                                .map(|note| (note.start_beats, note.length_beats, note.midi_note))
                                                .unwrap_or((start_beats, start_length, start_pitch));
                                            self.piano_drag = Some(PianoDragState {
                                                track_index,
                                                note_index: primary_index,
                                                kind,
                                                offset_beats,
                                                start_beats: primary.0,
                                                start_length: primary.1,
                                                start_pitch: primary.2,
                                                start_pos_y: pos.y,
                                                selected_notes,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if (roll_response.dragged()
                        || scale_handle_active
                        || (self.piano_drag.is_some() && pointer_down))
                        && !ctrl
                    {
                        if let Some(scale_drag) = &self.piano_scale_drag {
                            let drag_pos = ctx
                                .input(|i| i.pointer.interact_pos())
                                .or_else(|| roll_response.interact_pointer_pos())
                                .or_else(|| roll_response.hover_pos());
                            if let Some(pos) = drag_pos {
                                if pos.x < roll_rect.left() {
                                    return;
                                }
                                if let Some(clip_id) = self.selected_clip {
                                    let clip_index = self
                                        .find_clip_indices_by_id(clip_id)
                                        .map(|(_, ci)| ci)
                                        .unwrap_or(usize::MAX);
                                    let Some(clip) = self
                                        .tracks
                                        .get_mut(scale_drag.track_index)
                                        .and_then(|t| t.clips.get_mut(clip_index))
                                    else {
                                        return;
                                    };
                                    let local = pos_to_local(pos.x, self.piano_pan.x);
                                    let snapped_local = if alt {
                                        local
                                    } else {
                                        (local / quantize).round() * quantize
                                    };
                                    let snapped = (snapped_local + clip_offset).max(0.0);
                                    let new_end = snapped.max(scale_drag.anchor_start + quantize);
                                    let denom = (scale_drag.anchor_end - scale_drag.anchor_start)
                                        .max(quantize);
                                    let scale = (new_end - scale_drag.anchor_start) / denom;
                                    for (index, start, _pitch, len) in &scale_drag.selected_notes {
                                        if let Some(note) = clip.midi_notes.get_mut(*index) {
                                            note.start_beats =
                                                (scale_drag.anchor_start + (start - scale_drag.anchor_start) * scale)
                                                    .max(0.0);
                                            note.length_beats = (len * scale).max(quantize);
                                        }
                                    }
                                }
                            }
                        } else if let Some(drag) = &self.piano_drag {
                            if let Some(pos) = roll_response.interact_pointer_pos() {
                                if pos.x < roll_rect.left() {
                                    return;
                                }
                                if let Some(clip_id) = self.selected_clip {
                                    let clip_index = self
                                        .find_clip_indices_by_id(clip_id)
                                        .map(|(_, ci)| ci)
                                        .unwrap_or(usize::MAX);
                                    let Some(clip) = self
                                        .tracks
                                        .get_mut(drag.track_index)
                                        .and_then(|t| t.clips.get_mut(clip_index))
                                    else {
                                        return;
                                    };
                                    let beat = pos_to_abs(pos.x, self.piano_pan.x);
                                    match drag.kind {
                                        PianoDragKind::Move => {
                                            let raw_delta = beat - drag.offset_beats;
                                            let delta = if alt {
                                                raw_delta
                                            } else {
                                                let snapped = ((drag.start_beats + raw_delta) / quantize)
                                                    .round()
                                                    * quantize;
                                                snapped - drag.start_beats
                                            };
                                            let delta_pitch =
                                                ((drag.start_pos_y - pos.y) / note_height).round() as i32;
                                            if !drag.selected_notes.is_empty() {
                                                for (index, start, pitch, _) in &drag.selected_notes {
                                                    if let Some(note) = clip.midi_notes.get_mut(*index) {
                                                        note.start_beats = (start + delta).max(0.0);
                                                        let next_pitch = (*pitch as i32 + delta_pitch)
                                                            .clamp(0, 127) as u8;
                                                        note.midi_note = next_pitch;
                                                    }
                                                }
                                            } else if let Some(note) = clip.midi_notes.get_mut(drag.note_index) {
                                                note.start_beats = (drag.start_beats + delta).max(0.0);
                                                let next_pitch = (drag.start_pitch as i32 + delta_pitch)
                                                    .clamp(0, 127) as u8;
                                                note.midi_note = next_pitch;
                                            }
                                        }
                                        PianoDragKind::Resize => {
                                            if alt {
                                                let mut min_start = f32::MAX;
                                                let mut max_end = 0.0f32;
                                                for (_, start, _, len) in &drag.selected_notes {
                                                    min_start = min_start.min(*start);
                                                    max_end = max_end.max(start + len);
                                                }
                                                let anchor = min_start;
                                                let raw_end = beat.max(anchor + quantize);
                                                let snapped_end = (raw_end / quantize).round() * quantize;
                                                let new_end = snapped_end.max(anchor + quantize);
                                                let scale = if max_end > anchor {
                                                    (new_end - anchor) / (max_end - anchor)
                                                } else {
                                                    1.0
                                                };
                                                for (index, start, _pitch, len) in &drag.selected_notes {
                                                    if let Some(note) = clip.midi_notes.get_mut(*index) {
                                                        note.start_beats = (anchor + (start - anchor) * scale).max(0.0);
                                                        note.length_beats = (len * scale).max(quantize);
                                                    }
                                                }
                                            } else {
                                                let length = beat - drag.start_beats;
                                                let snapped = if alt {
                                                    length
                                                } else {
                                                    (length / quantize).round() * quantize
                                                };
                                                let delta_len = snapped - drag.start_length;
                                                if !drag.selected_notes.is_empty() {
                                                    for (index, _, _, start_len) in &drag.selected_notes {
                                                        if let Some(note) = clip.midi_notes.get_mut(*index) {
                                                            note.length_beats = (start_len + delta_len).max(quantize);
                                                        }
                                                    }
                                                } else if let Some(note) = clip.midi_notes.get_mut(drag.note_index) {
                                                    note.length_beats = snapped.max(quantize);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if roll_response.drag_stopped() || scale_handle_stopped {
                        if let Some(drag) = self.piano_scale_drag.take() {
                            self.sync_track_audio_notes(drag.track_index);
                            self.sync_linked_notes_after_edit(drag.track_index);
                        }
                        if let Some(drag) = self.piano_drag.take() {
                            self.sync_track_audio_notes(drag.track_index);
                            self.sync_linked_notes_after_edit(drag.track_index);
                        }
                    }

                    if let Some(rect) = marquee_rect {
                        painter.rect_stroke(
                            rect,
                            0.0,
                            egui::Stroke::new(1.2, egui::Color32::from_rgb(120, 170, 255)),
                        );
                        painter.rect_filled(
                            rect,
                            0.0,
                            egui::Color32::from_rgba_premultiplied(80, 120, 200, 40),
                        );
                    }

                    let playhead_x = roll_rect.left()
                        + self.piano_pan.x
                        + (self.playhead_beats - clip_offset) * beat_width;
                    if playhead_x >= roll_rect.left() && playhead_x <= roll_rect.right() {
                        painter.line_segment(
                            [
                                egui::pos2(playhead_x, roll_rect.top() + 2.0),
                                egui::pos2(playhead_x, roll_rect.bottom() - 4.0),
                            ],
                            egui::Stroke::new(1.2, egui::Color32::from_rgb(255, 86, 70)),
                        );
                    }
                    painter.rect_filled(header_rect, 0.0, egui::Color32::from_rgb(0, 0, 0));
                    painter.line_segment(
                        [
                            egui::pos2(header_rect.left(), header_rect.bottom()),
                            egui::pos2(header_rect.right(), header_rect.bottom()),
                        ],
                        egui::Stroke::new(1.0, egui::Color32::from_rgb(28, 30, 34)),
                    );
                    let mut beat_index = 0;
                    let mut header_x = roll_rect.left() + self.piano_pan.x;
                    while header_x <= header_rect.right() {
                        if beat_index % 4 == 0 {
                            let bar = beat_index / 4 + 1;
                            Self::outlined_text(
                                &painter,
                                egui::pos2(header_x + 4.0, header_rect.top() + 2.0),
                                egui::Align2::LEFT_TOP,
                                &format!("{bar}"),
                                egui::FontId::proportional(10.0),
                                egui::Color32::from_gray(160),
                            );
                        }
                        beat_index += 1;
                        header_x += beat_width;
                    }

                    if lane_rect.height() > 4.0 {
                        let lane_painter = ui.painter_at(lane_rect);
                        lane_painter.rect_filled(
                            lane_rect,
                            0.0,
                            egui::Color32::from_rgb(8, 9, 11),
                        );
                        lane_painter.line_segment(
                            [
                                egui::pos2(lane_rect.left(), lane_rect.top()),
                                egui::pos2(lane_rect.right(), lane_rect.top()),
                            ],
                            egui::Stroke::new(1.0, egui::Color32::from_rgb(24, 26, 30)),
                        );

                        let mut x = roll_rect.left() + self.piano_pan.x;
                        let mut beat_idx = 0;
                        while x <= lane_rect.right() {
                            let major = beat_idx % 4 == 0;
                            let color = if major {
                                egui::Color32::from_rgba_premultiplied(24, 26, 30, 160)
                            } else {
                                egui::Color32::from_rgba_premultiplied(18, 20, 24, 140)
                            };
                            lane_painter.line_segment(
                                [egui::pos2(x, lane_rect.top()), egui::pos2(x, lane_rect.bottom())],
                                egui::Stroke::new(1.0, color),
                            );
                            beat_idx += 1;
                            x += beat_width;
                        }

                        if let Some(clip_id) = self.selected_clip {
                            if let Some((track_index, clip_index)) = self.find_clip_indices_by_id(clip_id) {
                                if let Some(track) = self.tracks.get(track_index) {
                                    let clip = track.clips.get(clip_index);
                                    match self.piano_lane_mode {
                                        PianoLaneMode::Velocity => {
                                            if let Some(clip) = clip {
                                                for note in &clip.midi_notes {
                                                    let x = roll_rect.left()
                                                        + self.piano_pan.x
                                                        + (note.start_beats - clip_offset) * beat_width;
                                                    let w = (note.length_beats * beat_width).max(6.0);
                                                    if x + w < roll_rect.left() || x > roll_rect.right() {
                                                        continue;
                                                    }
                                                    let value =
                                                        (note.velocity as f32 / 127.0).clamp(0.0, 1.0);
                                                    let h = lane_rect.height() * value;
                                                    let pan = note.pan.clamp(-1.0, 1.0);
                                                    let tint = pan.abs();
                                                    let tint_r = if pan > 0.0 { 80.0 * tint } else { 0.0 };
                                                    let tint_b = if pan < 0.0 { 80.0 * tint } else { 0.0 };
                                                    let alpha = (value * 200.0 + 30.0).clamp(30.0, 230.0) as u8;
                                                    let r = (30.0 + tint_r).clamp(0.0, 255.0) as u8;
                                                    let g = (140.0 + value * 100.0).clamp(0.0, 255.0) as u8;
                                                    let b = (30.0 + tint_b).clamp(0.0, 255.0) as u8;
                                                    let bar_rect = egui::Rect::from_min_size(
                                                        egui::pos2(x, lane_rect.bottom() - h),
                                                        egui::vec2(w, h),
                                                    );
                                                    lane_painter.rect_filled(
                                                        bar_rect,
                                                        0.0,
                                                        egui::Color32::from_rgba_premultiplied(r, g, b, alpha),
                                                    );
                                                }
                                            }
                                        }
                                        PianoLaneMode::Pan => {
                                            let center_y = lane_rect.center().y;
                                            lane_painter.line_segment(
                                                [
                                                    egui::pos2(lane_rect.left(), center_y),
                                                    egui::pos2(lane_rect.right(), center_y),
                                                ],
                                                egui::Stroke::new(1.0, egui::Color32::from_rgb(32, 36, 40)),
                                            );
                                            if let Some(clip) = clip {
                                                for note in &clip.midi_notes {
                                                    let pan = note.pan.clamp(-1.0, 1.0);
                                                    let h = lane_rect.height() * 0.5 * pan.abs();
                                                    let vel = (note.velocity as f32 / 127.0).clamp(0.0, 1.0);
                                                    let alpha = (vel * 200.0 + 30.0).clamp(30.0, 230.0) as u8;
                                                    let x = roll_rect.left()
                                                        + self.piano_pan.x
                                                        + (note.start_beats - clip_offset) * beat_width;
                                                    let w = (note.length_beats * beat_width).max(6.0);
                                                    let (y, color) = if pan >= 0.0 {
                                                        (center_y - h, egui::Color32::from_rgba_premultiplied(210, 80, 80, alpha))
                                                    } else {
                                                        (center_y, egui::Color32::from_rgba_premultiplied(80, 120, 210, alpha))
                                                    };
                                                    let bar_rect = egui::Rect::from_min_size(
                                                        egui::pos2(x, y),
                                                        egui::vec2(w, h.max(2.0)),
                                                    );
                                                    lane_painter.rect_filled(bar_rect, 0.0, color);
                                                }
                                            }
                                        }
                                        PianoLaneMode::Cutoff => {
                                            if let Some(clip) = clip {
                                                for note in &clip.midi_notes {
                                                    let value = note.cutoff.clamp(0.0, 1.0);
                                                    let h = lane_rect.height() * value;
                                                    let x = roll_rect.left()
                                                        + self.piano_pan.x
                                                        + (note.start_beats - clip_offset) * beat_width;
                                                    let w = (note.length_beats * beat_width).max(6.0);
                                                    let bar_rect = egui::Rect::from_min_size(
                                                        egui::pos2(x, lane_rect.bottom() - h),
                                                        egui::vec2(w, h.max(2.0)),
                                                    );
                                                    lane_painter.rect_filled(
                                                        bar_rect,
                                                        0.0,
                                                        egui::Color32::from_rgb(90, 200, 120),
                                                    );
                                                }
                                            }
                                        }
                                        PianoLaneMode::Resonance => {
                                            if let Some(clip) = clip {
                                                for note in &clip.midi_notes {
                                                    let value = note.resonance.clamp(0.0, 1.0);
                                                    let h = lane_rect.height() * value;
                                                    let x = roll_rect.left()
                                                        + self.piano_pan.x
                                                        + (note.start_beats - clip_offset) * beat_width;
                                                    let w = (note.length_beats * beat_width).max(6.0);
                                                    let bar_rect = egui::Rect::from_min_size(
                                                        egui::pos2(x, lane_rect.bottom() - h),
                                                        egui::vec2(w, h.max(2.0)),
                                                    );
                                                    lane_painter.rect_filled(
                                                        bar_rect,
                                                        0.0,
                                                        egui::Color32::from_rgb(210, 180, 80),
                                                    );
                                                }
                                            }
                                        }
                                        PianoLaneMode::MidiCc => {
                                            if let Some(lane) = track
                                                .midi_cc_lanes
                                                .iter()
                                                .find(|lane| lane.cc == self.piano_cc)
                                            {
                                                let mut points = lane.points.clone();
                                                points.sort_by(|a, b| {
                                                    a.beat
                                                        .partial_cmp(&b.beat)
                                                        .unwrap_or(std::cmp::Ordering::Equal)
                                                });
                                                for window in points.windows(2) {
                                                    let a = &window[0];
                                                    let b = &window[1];
                                                    let x1 = roll_rect.left()
                                                        + self.piano_pan.x
                                                        + (a.beat - clip_offset) * beat_width;
                                                    let x2 = roll_rect.left()
                                                        + self.piano_pan.x
                                                        + (b.beat - clip_offset) * beat_width;
                                                    let y1 = lane_rect.bottom()
                                                        - a.value.clamp(0.0, 1.0) * lane_rect.height();
                                                    let y2 = lane_rect.bottom()
                                                        - b.value.clamp(0.0, 1.0) * lane_rect.height();
                                                    lane_painter.line_segment(
                                                        [egui::pos2(x1, y1), egui::pos2(x2, y2)],
                                                        egui::Stroke::new(
                                                            1.2,
                                                            egui::Color32::from_rgb(150, 180, 230),
                                                        ),
                                                    );
                                                }
                                                for point in &points {
                                                    let x = roll_rect.left()
                                                        + self.piano_pan.x
                                                        + (point.beat - clip_offset) * beat_width;
                                                    let y = lane_rect.bottom()
                                                        - point.value.clamp(0.0, 1.0) * lane_rect.height();
                                                    lane_painter.circle_filled(
                                                        egui::pos2(x, y),
                                                        3.0,
                                                        egui::Color32::from_rgb(180, 200, 240),
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        if lane_response.hovered() {
                            if let Some(pos) = lane_response.interact_pointer_pos() {
                                if pos.x >= roll_rect.left() {
                                    match self.piano_lane_mode {
                                        PianoLaneMode::MidiCc => {
                                            if let Some(track_index) = self.selected_track {
                                                if let Some(track) = self.tracks.get_mut(track_index) {
                                                    let lane_index = track
                                                        .midi_cc_lanes
                                                        .iter()
                                                        .position(|lane| lane.cc == self.piano_cc)
                                                        .unwrap_or_else(|| {
                                                            track.midi_cc_lanes.push(MidiCcLane {
                                                                cc: self.piano_cc,
                                                                points: Vec::new(),
                                                            });
                                                            track.midi_cc_lanes.len() - 1
                                                        });
                                                    let lane = &mut track.midi_cc_lanes[lane_index];
                                                    let beat = (pos.x - roll_rect.left() - self.piano_pan.x)
                                                        / beat_width
                                                        + clip_offset;
                                                    let value = (lane_rect.bottom() - pos.y)
                                                        / lane_rect.height();
                                                    let value = value.clamp(0.0, 1.0);

                                                    if lane_response.drag_started() || lane_response.clicked() {
                                                        let mut closest: Option<(usize, f32)> = None;
                                                        for (idx, point) in lane.points.iter().enumerate() {
                                                            let px = roll_rect.left()
                                                                + self.piano_pan.x
                                                                + (point.beat - clip_offset) * beat_width;
                                                            let py = lane_rect.bottom()
                                                                - point.value.clamp(0.0, 1.0) * lane_rect.height();
                                                            let dx = px - pos.x;
                                                            let dy = py - pos.y;
                                                            let dist = dx * dx + dy * dy;
                                                            if dist < 64.0 {
                                                                if closest.map_or(true, |(_, best)| dist < best) {
                                                                    closest = Some((idx, dist));
                                                                }
                                                            }
                                                        }
                                                        if let Some((idx, _)) = closest {
                                                            self.piano_cc_drag = Some(idx);
                                                        } else {
                                                            lane.points.push(AutomationPoint { beat, value });
                                                            self.piano_cc_drag = Some(lane.points.len() - 1);
                                                        }
                                                    }

                                                    if lane_response.dragged() {
                                                        if let Some(idx) = self.piano_cc_drag {
                                                            if let Some(point) = lane.points.get_mut(idx) {
                                                                point.beat = beat.max(0.0);
                                                                point.value = value;
                                                            }
                                                        }
                                                    }
                                                    if lane_response.drag_stopped()
                                                        || ctx.input(|i| i.pointer.any_released())
                                                    {
                                                        self.piano_cc_drag = None;
                                                    }
                                                }
                                            }
                                        }
                                        _ => {
                                            if self.selected_clip.is_some() {
                                                if let Some(clip_id) = self.selected_clip {
                                                    if let Some((track_index, clip_index)) =
                                                        self.find_clip_indices_by_id(clip_id)
                                                    {
                                                        let beat = pos_to_abs(pos.x, self.piano_pan.x);
                                                        if beat >= clip_offset {
                                                            if let Some(clip) = self
                                                                .tracks
                                                                .get_mut(track_index)
                                                                .and_then(|t| t.clips.get_mut(clip_index))
                                                            {
                                                                let mut targets: Vec<usize> = Vec::new();
                                                                if !self.piano_selected.is_empty() {
                                                                    for index in self.piano_selected.iter().copied() {
                                                                        if let Some(note) = clip.midi_notes.get(index) {
                                                                            if beat >= note.start_beats
                                                                                && beat <= note.start_beats + note.length_beats
                                                                            {
                                                                                targets.push(index);
                                                                            }
                                                                        }
                                                                    }
                                                                }
                                                                if targets.is_empty() {
                                                                    if let Some(note_index) = clip
                                                                        .midi_notes
                                                                        .iter()
                                                                        .position(|note| {
                                                                            beat >= note.start_beats
                                                                                && beat <= note.start_beats + note.length_beats
                                                                        })
                                                                    {
                                                                        targets.push(note_index);
                                                                    }
                                                                }
                                                                if !targets.is_empty() {
                                                                    let value = (lane_rect.bottom() - pos.y)
                                                                        / lane_rect.height();
                                                                    let value = value.clamp(0.0, 1.0);
                                                                    let pan = (lane_rect.center().y - pos.y)
                                                                        / (lane_rect.height() * 0.5);
                                                                    let pan = pan.clamp(-1.0, 1.0);
                                                                    for note_index in targets {
                                                                        if let Some(note) = clip.midi_notes.get_mut(note_index) {
                                                                            match self.piano_lane_mode {
                                                                                PianoLaneMode::Velocity => {
                                                                                    note.velocity =
                                                                                        (value * 127.0).round() as u8;
                                                                                }
                                                                                PianoLaneMode::Pan => {
                                                                                    note.pan = pan;
                                                                                }
                                                                                PianoLaneMode::Cutoff => {
                                                                                    note.cutoff = value;
                                                                                }
                                                                                PianoLaneMode::Resonance => {
                                                                                    note.resonance = value;
                                                                                }
                                                                                PianoLaneMode::MidiCc => {}
                                                                            }
                                                                        }
                                                                    }
                                                                    self.sync_track_audio_notes(track_index);
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                });
    }
}
