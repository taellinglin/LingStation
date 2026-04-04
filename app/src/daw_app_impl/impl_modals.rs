impl DawApp {
    pub(crate) fn modals(&mut self, ctx: &egui::Context) {
        if self.show_close_confirm {
            let mut open = self.show_close_confirm;
            let mut proceed_action: Option<ProjectAction> = None;
            let mut close_requested = false;
            let mut confirm_exit = false;
            egui::Window::new("Unsaved Changes")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("Current project has unsaved changes.");
                    ui.label("Save before continuing?");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::Button::image(
                                egui::Image::new(egui::include_image!("../../../assets/icons/save.svg"))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                            ))
                            .on_hover_text("Save & Continue")
                            .clicked()
                        {
                            match self.save_project_or_prompt() {
                                Ok(_) => {
                                    self.clear_dirty();
                                    proceed_action = self.pending_project_action.take();
                                    if self.pending_exit {
                                        confirm_exit = true;
                                    }
                                    close_requested = true;
                                }
                                Err(err) => {
                                    self.status = format!("Save failed: {err}");
                                }
                            }
                        }
                        if ui
                            .add(egui::Button::image(
                                egui::Image::new(egui::include_image!("../../../assets/icons/trash-2.svg"))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                            ))
                            .on_hover_text("Discard")
                            .clicked()
                        {
                            self.clear_dirty();
                            proceed_action = self.pending_project_action.take();
                            if self.pending_exit {
                                confirm_exit = true;
                            }
                            close_requested = true;
                        }
                        if ui
                            .add(egui::Button::image(
                                egui::Image::new(egui::include_image!("../../../assets/icons/x.svg"))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                            ))
                            .on_hover_text("Cancel")
                            .clicked()
                        {
                            self.pending_exit = false;
                            close_requested = true;
                        }
                    });
                });
            if close_requested {
                open = false;
            }
            if !open {
                self.show_close_confirm = false;
                self.pending_exit = false;
                if proceed_action.is_none() {
                    self.pending_project_action = None;
                }
            }
            if confirm_exit {
                self.pending_exit = false;
                self.exit_confirmed = true;
            }
            if let Some(action) = proceed_action {
                self.perform_project_action(action);
            }
        }

        if self.show_settings {
            let mut open = self.show_settings;
            egui::Window::new("Settings")
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.settings_tab, SettingsTab::General, "General");
                        ui.selectable_value(&mut self.settings_tab, SettingsTab::Audio, "Audio");
                        ui.selectable_value(&mut self.settings_tab, SettingsTab::Midi, "MIDI");
                        ui.selectable_value(&mut self.settings_tab, SettingsTab::Devices, "Devices");
                        ui.selectable_value(&mut self.settings_tab, SettingsTab::Theme, "Theme");
                    });
                    ui.separator();

                    match self.settings_tab {
                        SettingsTab::General => {
                            ui.heading("General");
                            ui.separator();
                            ui.horizontal(|ui| {
                                ui.label("Key Display");
                                let label = match self.settings.key_display_format.as_str() {
                                    "traditional" => "Traditional (C#m)",
                                    "both" => "Both",
                                    _ => "Camelot (8A)",
                                };
                                egui::ComboBox::from_id_source("key_display_format")
                                    .selected_text(label)
                                    .show_ui(ui, |ui| {
                                        if ui
                                            .selectable_label(
                                                self.settings.key_display_format == "camelot",
                                                "Camelot (8A)",
                                            )
                                            .clicked()
                                        {
                                            self.settings.key_display_format = "camelot".to_string();
                                        }
                                        if ui
                                            .selectable_label(
                                                self.settings.key_display_format == "traditional",
                                                "Traditional (C#m)",
                                            )
                                            .clicked()
                                        {
                                            self.settings.key_display_format = "traditional".to_string();
                                        }
                                        if ui
                                            .selectable_label(
                                                self.settings.key_display_format == "both",
                                                "Both",
                                            )
                                            .clicked()
                                        {
                                            self.settings.key_display_format = "both".to_string();
                                        }
                                    });
                            });
                            ui.checkbox(&mut self.settings.show_clip_labels, "Show clip labels in Arranger");
                        }
                        SettingsTab::Audio => {
                            ui.heading("Audio");
                            ui.separator();
                            let devices = self.list_output_devices();
                            egui::ComboBox::from_label("Soundcard")
                                .selected_text(self.settings.output_device.clone())
                                .show_ui(ui, |ui| {
                                    for name in &devices {
                                        if ui
                                            .selectable_label(self.settings.output_device == *name, name)
                                            .clicked()
                                        {
                                            self.settings.output_device = name.to_string();
                                        }
                                    }
                                });
                            let inputs = self.list_input_devices();
                            egui::ComboBox::from_label("Input Device")
                                .selected_text(self.settings.input_device.clone())
                                .show_ui(ui, |ui| {
                                    for name in &inputs {
                                        if ui
                                            .selectable_label(self.settings.input_device == *name, name)
                                            .clicked()
                                        {
                                            self.settings.input_device = name.to_string();
                                        }
                                    }
                                });
                            ui.horizontal(|ui| {
                                ui.label("Buffer Size");
                                egui::ComboBox::from_id_source("buffer_size")
                                    .selected_text(format!("{}", self.settings.buffer_size))
                                    .show_ui(ui, |ui| {
                                        for size in [128u32, 256, 512, 1024, 2048] {
                                            if ui
                                                .selectable_label(
                                                    self.settings.buffer_size == size,
                                                    format!("{}", size),
                                                )
                                                .clicked()
                                            {
                                                self.settings.buffer_size = size;
                                            }
                                        }
                                    });
                            });
                            ui.horizontal(|ui| {
                                ui.label("Sample Rate");
                                egui::ComboBox::from_id_source("sample_rate")
                                    .selected_text(format!("{}", self.settings.sample_rate))
                                    .show_ui(ui, |ui| {
                                        for rate in [44_100u32, 48_000, 96_000] {
                                            if ui
                                                .selectable_label(
                                                    self.settings.sample_rate == rate,
                                                    format!("{}", rate),
                                                )
                                                .clicked()
                                            {
                                                self.settings.sample_rate = rate;
                                            }
                                        }
                                    });
                            });
                            ui.horizontal(|ui| {
                                ui.label("Interpolation");
                                egui::ComboBox::from_id_source("interpolation")
                                    .selected_text(self.settings.interpolation.clone())
                                    .show_ui(ui, |ui| {
                                        for mode in ["linear", "cubic", "sinc"] {
                                            if ui
                                                .selectable_label(self.settings.interpolation == mode, mode)
                                                .clicked()
                                            {
                                                self.settings.interpolation = mode.to_string();
                                            }
                                        }
                                    });
                            });
                            ui.horizontal(|ui| {
                                ui.label("Autosave (minutes)");
                                ui.add(
                                    egui::DragValue::new(&mut self.settings.autosave_minutes)
                                        .clamp_range(0..=120),
                                );
                            });
                            ui.label("Set to 0 to disable autosave.");
                            ui.checkbox(
                                &mut self.settings.load_last_project,
                                "Load last project at startup",
                            );
                            ui.checkbox(
                                &mut self.settings.play_startup_sound,
                                "Play startup sound",
                            );
                            ui.checkbox(&mut self.settings.triple_buffer, "Triple buffer");
                            ui.checkbox(&mut self.settings.safe_underruns, "Safe underruns");
                            ui.checkbox(&mut self.settings.adaptive_buffer, "Adaptive buffer");
                            ui.checkbox(&mut self.settings.smart_disable_plugins, "Smart disable plugins");
                            ui.checkbox(&mut self.settings.smart_suspend_tracks, "Smart suspend tracks");
                        }
                        SettingsTab::Midi => {
                            ui.heading("MIDI");
                            ui.separator();
                            let midi_inputs = self.list_midi_inputs();
                            egui::ComboBox::from_label("MIDI Input")
                                .selected_text(self.settings.midi_input.clone())
                                .show_ui(ui, |ui| {
                                    for name in &midi_inputs {
                                        if ui
                                            .selectable_label(self.settings.midi_input == *name, name)
                                            .clicked()
                                        {
                                            self.settings.midi_input = name.to_string();
                                        }
                                    }
                                });
                            ui.add_space(8.0);
                            ui.label("Use Devices for controller profiles, per-device ports, and channel filters.");
                        }
                        SettingsTab::Devices => {
                            self.render_devices_settings(ui);
                        }
                        SettingsTab::Theme => {
                            ui.heading("Theme");
                            ui.separator();
                            ui.label("Color Scheme");
                            egui::ComboBox::from_id_source("theme_scheme")
                                .selected_text(self.settings.theme.clone())
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut self.settings.theme,
                                        "Black".to_string(),
                                        "Black (White on Black)",
                                    );
                                    ui.selectable_value(
                                        &mut self.settings.theme,
                                        "Dark".to_string(),
                                        "Dark Gray",
                                    );
                                });
                            ui.add_space(10.0);
                            ui.separator();
                            ui.heading("Wallpaper");
                            if self.is_registered_user() {
                                ui.label("Registered users can set a custom PNG wallpaper for the DAW background.");
                                ui.horizontal(|ui| {
                                    ui.label("File");
                                    let display = if self.settings.wallpaper_path.trim().is_empty() {
                                        "None".to_string()
                                    } else {
                                        self.settings.wallpaper_path.clone()
                                    };
                                    ui.label(display);
                                });
                                ui.horizontal(|ui| {
                                    if ui.button("Choose PNG").clicked() {
                                        if let Some(path) = rfd::FileDialog::new()
                                            .add_filter("PNG", &["png"])
                                            .pick_file()
                                        {
                                            self.settings.wallpaper_path = path.to_string_lossy().to_string();
                                            self.invalidate_wallpaper_texture();
                                            match self.ensure_wallpaper_texture(ctx) {
                                                Ok(()) => {
                                                    self.pending_repaint_frames = self.pending_repaint_frames.max(12);
                                                    self.status = "Wallpaper updated".to_string();
                                                }
                                                Err(err) => {
                                                    self.status = err;
                                                }
                                            }
                                        }
                                    }
                                    if ui.button("Clear Wallpaper").clicked() {
                                        self.settings.wallpaper_path.clear();
                                        self.invalidate_wallpaper_texture();
                                        self.status = "Wallpaper cleared".to_string();
                                    }
                                });
                                ui.horizontal(|ui| {
                                    ui.label("Opacity");
                                    ui.add(
                                        egui::Slider::new(&mut self.settings.wallpaper_opacity, 0.05..=0.8)
                                            .show_value(true),
                                    );
                                });
                            } else {
                                ui.label("Custom wallpaper is available for registered users.");
                                ui.label("Open License to register this copy and unlock wallpapers.");
                            }
                        }
                    }

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::Button::image(
                                egui::Image::new(egui::include_image!("../../../assets/icons/save.svg"))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                            ))
                            .on_hover_text("Save Settings")
                            .clicked()
                        {
                            if let Err(err) = self.save_settings() {
                                self.status = format!("Settings save failed: {err}");
                            } else {
                                self.status = "Settings saved".to_string();
                            }
                        }
                        if ui
                            .add(egui::Button::image(
                                egui::Image::new(egui::include_image!("../../../assets/icons/rotate-cw.svg"))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                            ))
                            .on_hover_text("Reload")
                            .clicked()
                        {
                            self.load_settings_or_default();
                            self.status = "Settings reloaded".to_string();
                        }
                    });
                });
            self.show_settings = open;
        }

        if self.show_plugin_picker {
            let mut open = self.show_plugin_picker;
            let mut chosen: Option<PluginCandidate> = None;
            let mut refresh = false;
            egui::Window::new("Plugin Picker")
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.label("Scanning plugins (VST3/CLAP + native)");
                    ui.horizontal(|ui| {
                        ui.label("Search");
                        ui.text_edit_singleline(&mut self.plugin_search);
                        if ui
                            .add(egui::Button::image(
                                egui::Image::new(egui::include_image!("../../../assets/icons/refresh-cw.svg"))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                            ))
                            .on_hover_text("Refresh")
                            .clicked()
                        {
                            refresh = true;
                        }
                    });
                    ui.separator();

                    let search = self.plugin_search.to_ascii_lowercase();
                    let target_is_instrument = matches!(self.plugin_target, Some(PluginTarget::Instrument(_)));
                    egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                        let mut drew_any = false;
                        for category in [PluginCategory::Native, PluginCategory::Bundled, PluginCategory::System] {
                            let mut category_items = Vec::new();
                            for candidate in &self.plugin_candidates {
                                if candidate.category != category {
                                    continue;
                                }
                                if candidate.instrument_only && !target_is_instrument {
                                    continue;
                                }
                                let display = &candidate.display;
                                if !search.is_empty()
                                    && !candidate.path.to_ascii_lowercase().contains(&search)
                                    && !display.to_ascii_lowercase().contains(&search)
                                {
                                    continue;
                                }
                                category_items.push(candidate);
                            }
                            if category_items.is_empty() {
                                continue;
                            }
                            if drew_any {
                                ui.add_space(6.0);
                            }
                            ui.heading(category.label());
                            ui.add_space(2.0);
                            for candidate in category_items {
                                if ui.selectable_label(false, &candidate.display).clicked() {
                                    chosen = Some(candidate.clone());
                                }
                            }
                            drew_any = true;
                        }
                        if !drew_any {
                            ui.label("No plugins found.");
                        }
                    });
                });

            if refresh {
                self.plugin_candidates = self.scan_plugins();
            }

            if let Some(candidate) = chosen {
                if let Some(target) = self.plugin_target {
                    if self.plugin_ui.is_some() {
                        self.show_plugin_ui = false;
                        self.destroy_plugin_ui();
                    }
                    match target {
                        PluginTarget::Instrument(index) => {
                            self.replace_instrument(index, candidate.path, candidate.clap_id);
                        }
                        PluginTarget::Effect(index) => {
                            if let Some(track) = self.tracks.get_mut(index) {
                                track.effect_paths.push(candidate.path);
                                track.effect_clap_ids.push(candidate.clap_id);
                                track.effect_bypass.push(false);
                                track.effect_params.push(Vec::new());
                                track.effect_param_ids.push(Vec::new());
                                track.effect_param_values.push(Vec::new());
                            }
                            if let Some(state) = self.engine.track_audio.get_mut(index) {
                                for mut host in state.effect_hosts.drain(..) {
                                    host.prepare_for_drop();
                                    self.orphaned_hosts.push(host);
                                }
                            }
                            if self.audio_running {
                                self.status = "Effect added; it will activate after stop/play".to_string();
                            } else {
                                self.status = "Effect added".to_string();
                            }
                            self.refresh_params_for_selected_track(true);
                        }
                    }
                }
                open = false;
            }

            self.show_plugin_picker = open;
            if !open {
                self.plugin_target = None;
            }
        }

        if self.show_midi_import {
            let mut open = self.show_midi_import;
            let mut do_import = false;
            let mut close_requested = false;
            if let Some(state) = self.midi_import_state.as_mut() {
                egui::Window::new("Import MIDI")
                    .open(&mut open)
                    .default_size(egui::vec2(520.0, 420.0))
                    .show(ctx, |ui| {
                        let file_label = Path::new(&state.path)
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or(state.path.as_str());
                        ui.label(format!("File: {file_label}"));
                        ui.separator();
                        ui.label("Tracks");
                        ui.horizontal(|ui| {
                            if ui
                                .add(egui::Button::image(
                                    egui::Image::new(egui::include_image!("../../../assets/icons/check-square.svg"))
                                        .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                ))
                                .on_hover_text("All")
                                .clicked()
                            {
                                for enabled in &mut state.enabled {
                                    *enabled = true;
                                }
                            }
                            if ui
                                .add(egui::Button::image(
                                    egui::Image::new(egui::include_image!("../../../assets/icons/x-square.svg"))
                                        .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                ))
                                .on_hover_text("None")
                                .clicked()
                            {
                                for enabled in &mut state.enabled {
                                    *enabled = false;
                                }
                            }
                        });
                        egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                            for ((track_data, enabled), apply_program) in state
                                .tracks
                                .iter()
                                .zip(state.enabled.iter_mut())
                                .zip(state.apply_program.iter_mut())
                            {
                                let track_name = match track_data.program {
                                    Some(program) if track_data.has_drums => gm_drum_kit_name(program)
                                        .unwrap_or("Drum Kit")
                                        .to_string(),
                                    Some(program) => gm_program_name(program).to_string(),
                                    None => format!("Track {}", track_data.track_index + 1),
                                };
                                let label = format!("Track {} - {}", track_data.track_index + 1, track_name);
                                ui.horizontal(|ui| {
                                    ui.checkbox(enabled, label);
                                    ui.add_enabled_ui(track_data.program.is_some(), |ui| {
                                        ui.checkbox(apply_program, "Use patch");
                                    });
                                });
                            }
                        });

                        ui.separator();
                        let instrument_options = [
                            "None",
                            "MiceSynth",
                            "FishSynth",
                            "SannySynth",
                            "LingSynth",
                            "DogSynth",
                        ];
                        egui::ComboBox::from_label("Instrument Plugin")
                            .selected_text(state.instrument_plugin.clone())
                            .show_ui(ui, |ui| {
                                for name in instrument_options {
                                    if ui.selectable_label(state.instrument_plugin == name, name).clicked() {
                                        state.instrument_plugin = name.to_string();
                                    }
                                }
                            });
                        let percussion_options = ["None", "Catsynth", "PlantSynth"]; 
                        egui::ComboBox::from_label("Percussion Plugin")
                            .selected_text(state.percussion_plugin.clone())
                            .show_ui(ui, |ui| {
                                for name in percussion_options {
                                    if ui.selectable_label(state.percussion_plugin == name, name).clicked() {
                                        state.percussion_plugin = name.to_string();
                                    }
                                }
                            });
                        ui.checkbox(&mut state.import_portamento, "Import Portamento (CC65)");
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui
                                .add(egui::Button::image(
                                    egui::Image::new(egui::include_image!("../../../assets/icons/download.svg"))
                                        .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                ))
                                .on_hover_text("Import")
                                .clicked()
                            {
                                do_import = true;
                            }
                            if ui
                                .add(egui::Button::image(
                                    egui::Image::new(egui::include_image!("../../../assets/icons/x.svg"))
                                        .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                                ))
                                .on_hover_text("Cancel")
                                .clicked()
                            {
                                close_requested = true;
                            }
                        });
                    });
            } else {
                open = false;
            }
            if close_requested {
                open = false;
            }
            self.show_midi_import = open;
            if do_import {
                if let Err(err) = self.apply_midi_import() {
                    self.status = format!("MIDI import failed: {err}");
                }
            }
            if !open {
                self.midi_import_state = None;
            }
        }

        if self.show_rename_track {
            let mut open = self.show_rename_track;
            let mut close_requested = false;
            egui::Window::new("Rename Track")
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.label("Track Name");
                    ui.text_edit_singleline(&mut self.rename_buffer);
                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::Button::image(
                                egui::Image::new(egui::include_image!("../../../assets/icons/check.svg"))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                            ))
                            .on_hover_text("Apply")
                            .clicked()
                        {
                            self.apply_rename();
                            close_requested = true;
                        }
                        if ui
                            .add(egui::Button::image(
                                egui::Image::new(egui::include_image!("../../../assets/icons/x.svg"))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                            ))
                            .on_hover_text("Cancel")
                            .clicked()
                        {
                            close_requested = true;
                        }
                    });
                });
            if close_requested {
                open = false;
            }
            self.show_rename_track = open;
        }

        if self.show_rename_clip {
            let mut open = self.show_rename_clip;
            let mut close_requested = false;
            egui::Window::new("Rename Clip")
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.label("Clip Name");
                    ui.text_edit_singleline(&mut self.rename_clip_buffer);
                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::Button::image(
                                egui::Image::new(egui::include_image!("../../../assets/icons/check.svg"))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                            ))
                            .on_hover_text("Apply")
                            .clicked()
                        {
                            self.apply_rename_clip();
                            close_requested = true;
                        }
                        if ui
                            .add(egui::Button::image(
                                egui::Image::new(egui::include_image!("../../../assets/icons/x.svg"))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                            ))
                            .on_hover_text("Cancel")
                            .clicked()
                        {
                            close_requested = true;
                        }
                    });
                });
            if close_requested {
                open = false;
            }
            self.show_rename_clip = open;
        }

        if self.show_rename_project {
            let mut open = self.show_rename_project;
            let mut close_requested = false;
            egui::Window::new("Rename Project")
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.label("Project Name");
                    ui.text_edit_singleline(&mut self.project_name_buffer);
                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::Button::image(
                                egui::Image::new(egui::include_image!("../../../assets/icons/check.svg"))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                            ))
                            .on_hover_text("Apply")
                            .clicked()
                        {
                            self.apply_rename_project();
                            close_requested = true;
                        }
                        if ui
                            .add(egui::Button::image(
                                egui::Image::new(egui::include_image!("../../../assets/icons/x.svg"))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                            ))
                            .on_hover_text("Cancel")
                            .clicked()
                        {
                            close_requested = true;
                        }
                    });
                });
            if close_requested {
                open = false;
            }
            self.show_rename_project = open;
        }

        if self.show_project_info {
            let mut open = self.show_project_info;
            egui::Window::new("Project Info")
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.label(format!("Project: {}", self.project_name));
                    let mut project_key_changed = false;
                    ui.horizontal(|ui| {
                        ui.label("Key");
                        let display = self.format_key_display(self.project_key, self.project_key_minor);
                        egui::ComboBox::from_id_source("project_key")
                            .selected_text(display)
                            .show_ui(ui, |ui| {
                                if ui
                                    .selectable_label(self.project_key.is_none(), "Unknown")
                                    .clicked()
                                {
                                    self.project_key = None;
                                    project_key_changed = true;
                                }
                                for key in 0u8..12 {
                                    let name = Self::format_key_display_with(
                                        self.settings.key_display_format.as_str(),
                                        Some(key),
                                        self.project_key_minor,
                                    );
                                    if ui
                                        .selectable_label(self.project_key == Some(key), name)
                                        .clicked()
                                    {
                                        self.project_key = Some(key);
                                        project_key_changed = true;
                                    }
                                }
                            });
                        let enabled = self.project_key.is_some();
                        ui.add_enabled_ui(enabled, |ui| {
                            if ui.checkbox(&mut self.project_key_minor, "Minor").changed() {
                                project_key_changed = true;
                            }
                        });
                    });
                    if project_key_changed {
                        self.refresh_audio_clip_timeline_if_running();
                    }
                    ui.label(format!("Tempo: {} BPM", self.tempo_bpm));
                    ui.label("Time Signature: 4/4");
                    ui.label("Sample Rate: 48 kHz");
                });
            self.show_project_info = open;
        }

        if self.show_metadata {
            let mut open = self.show_metadata;
            egui::Window::new("Metadata")
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.label("Artist");
                    ui.text_edit_singleline(&mut self.metadata_artist);
                    ui.label("Title");
                    ui.text_edit_singleline(&mut self.metadata_title);
                    ui.label("Album");
                    ui.text_edit_singleline(&mut self.metadata_album);
                    ui.label("Genre");
                    ui.text_edit_singleline(&mut self.metadata_genre);
                    ui.label("Year");
                    ui.text_edit_singleline(&mut self.metadata_year);
                    ui.label("Comment");
                    ui.text_edit_multiline(&mut self.metadata_comment);
                });
            self.show_metadata = open;
        }

        if self.show_help_about {
            let mut open = self.show_help_about;
            egui::Window::new("About")
                .open(&mut open)
                .default_size(egui::vec2(420.0, 260.0))
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        self.draw_animated_title(ui, ctx);
                        ui.add_space(6.0);
                        ui.label("A warm, focused DAW for fast music making.");
                        ui.label("Special thanks to Sanny and Ling Lin for the spark and support.");
                        ui.add_space(6.0);
                        ui.separator();
                        let version = env!("CARGO_PKG_VERSION");
                        let profile = if cfg!(debug_assertions) { "debug" } else { "release" };
                        ui.label(format!("Version: {version}"));
                        ui.label(format!("Build: {profile}"));
                        ui.label(format!("Engine: {}", env!("CARGO_PKG_NAME")));
                        ui.separator();
                        ui.heading("Bundled Synths");
                        ui.add_space(4.0);
                        ui.label("SannySynth: soft FM flavor with subtractive/wavetable focus.");
                        ui.label("- Carrier waveform selection with wavetable blend and optional custom wavetable.");
                        ui.label("- ADSR for amp plus dual filter envelopes (cutoff and resonance).");
                        ui.label("- LFOs/mod routing, plus vibrato and tremolo controls.");
                        ui.label("- Chorus, delay, reverb, and multi-stage filter options.");
                        ui.add_space(4.0);
                        ui.label("DogSynth: hard FM.");
                        ui.label("- Classic plus wavetable osc blend with wavetable distortion.");
                        ui.label("- FM controls (amount, ratio, feedback) and modulation targets for FM routing.");
                        ui.label("- Unison, glide, and modulation matrix with LFOs and envelopes.");
                        ui.label("- Multi-filter section plus distortion, EQ, saturation, and time FX.");
                        ui.add_space(4.0);
                        ui.label("FishSynth: FM plus wavetable, GM.");
                        ui.label("- FM enable/ratio/amount with modulator waveform, plus sub osc and noise mix.");
                        ui.label("- ADSR amp and filter envelopes with multiple filter types.");
                        ui.label("- Vibrato/tremolo LFOs and chorus.");
                        ui.label("- GM preset browsing and SF2/SoundFont-based program support.");
                        ui.add_space(4.0);
                        ui.label("MiceSynth: additive plus FM.");
                        ui.label("- Additive engine controls (mix, partials, tilt, inharm, morph, decay, drift).");
                        ui.label("- FM controls with modulation routing and classic/wavetable blend.");
                        ui.label("- Mod matrix, unison/glide, and filter envelopes.");
                        ui.label("- Preset system with morphing between snapshots.");
                        ui.add_space(4.0);
                        ui.label("CatSynth: drum synthesizer.");
                        ui.label("- 16 drum slots with per-slot instrument model (kick/snare/hats/etc).");
                        ui.label("- Exciter, resonator, material, and noise shaping per slot.");
                        ui.label("- Pitch/decay/strike controls, plus tone and transient/body shaping.");
                        ui.label("- Built-in 16-step drum sequencer per slot.");
                        ui.add_space(4.0);
                        ui.label("PlantSynth: drum sampler.");
                        ui.label("- 16 sample slots with per-slot pitch, pan, drive, and 3-band tone.");
                        ui.label("- Sample envelope controls (attack, decay, sustain, release).");
                        ui.label("- Velocity sensitivity and pad trigger behavior per slot.");
                        ui.label("- Master gain/drive/comp/clip controls.");
                        ui.separator();
                        ui.label("LingStation");
                        ui.label("Copyright (c) 2026");
                    });
                    ctx.request_repaint();
                });
            self.show_help_about = open;
        }

        if self.show_help_general {
            let mut open = self.show_help_general;
            egui::Window::new("Help")
                .open(&mut open)
                .default_size(egui::vec2(520.0, 380.0))
                .show(ctx, |ui| {
                    ui.heading("Getting Started");
                    ui.separator();
                    ui.label("1. Create a track and choose a synth.");
                    ui.label("2. Add MIDI clips in Arranger or Piano Roll.");
                    ui.label("3. Use Mixer for levels, mute, and solo.");
                    ui.label("4. Render from File -> Render to WAV/OGG/FLAC.");
                    ui.add_space(8.0);
                    ui.heading("Common Tasks");
                    ui.separator();
                    ui.label("- Drag clips to move or copy.");
                    ui.label("- Use the Parameters tab for instrument/effects.");
                    ui.label("- Use Loop Song to loop the current range.");
                    ui.add_space(8.0);
                    ui.heading("Support");
                    ui.separator();
                    ui.label("Check the License page for registration and activation.");
                });
            self.show_help_general = open;
        }

        if self.show_help_shortcuts {
            let mut open = self.show_help_shortcuts;
            egui::Window::new("Shortcuts")
                .open(&mut open)
                .default_size(egui::vec2(520.0, 380.0))
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for (category, entries) in Self::shortcuts_by_category() {
                            ui.heading(category);
                            ui.separator();
                            for (keys, action) in entries {
                                ui.horizontal(|ui| {
                                    ui.label(keys);
                                    ui.label(action);
                                });
                            }
                            ui.add_space(8.0);
                        }
                    });
                });
            self.show_help_shortcuts = open;
        }

        if self.show_help_license {
            let mut open = self.show_help_license;
            egui::Window::new("License")
                .open(&mut open)
                .default_size(egui::vec2(560.0, 520.0))
                .show(ctx, |ui| {
                    ui.label(format!("Status: {}", self.license_status));
                    let registered = if self.settings.registered_to.is_empty() {
                        "Unregistered".to_string()
                    } else {
                        format!("Registered to: {}", self.settings.registered_to)
                    };
                    ui.label(registered);
                    if let Some(limit) = self.settings.license_monthly_activations {
                        ui.label(format!("Monthly activations: {limit}"));
                    }
                    if let Some(remaining) = self.settings.license_remaining_activations {
                        ui.label(format!("Remaining this month: {remaining}"));
                    }
                    ui.label("Policy: 2 activations per month (no rollover).");
                    ui.separator();

                    ui.heading("Account Login");
                    ui.label("Sign in to access your licenses.");
                    ui.horizontal(|ui| {
                        ui.label("Identifier");
                        ui.text_edit_singleline(&mut self.license_identifier);
                    });
                    ui.horizontal(|ui| {
                        ui.label("Password");
                        ui.add(egui::TextEdit::singleline(&mut self.license_password).password(true));
                    });
                    ui.horizontal(|ui| {
                        ui.label("Device Label");
                        ui.text_edit_singleline(&mut self.license_device_label);
                    });
                    ui.horizontal(|ui| {
                        let login_clicked = ui.button("Login").clicked();
                        if login_clicked {
                            self.start_license_login();
                        }
                        let clear_clicked = ui.button("Clear Token").clicked();
                        if clear_clicked {
                            self.settings.auth_token.clear();
                            let _ = self.save_settings();
                            self.status = "Token cleared".to_string();
                        }
                    });

                    ui.add_space(8.0);
                    ui.heading("License Actions");
                    ui.horizontal(|ui| {
                        ui.label("Serial");
                        ui.text_edit_singleline(&mut self.license_serial);
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Claim Serial").clicked() {
                            self.start_license_claim();
                        }
                        if ui.button("Activate Device").clicked() {
                            self.start_license_activate();
                        }
                        if ui.button("Download File").clicked() {
                            self.start_license_fetch_file();
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Import License File").clicked() {
                            if let Some(path) = rfd::FileDialog::new().pick_file() {
                                if let Ok(text) = fs::read_to_string(&path) {
                                    self.settings.license_file = text;
                                    let _ = self.save_settings();
                                    self.refresh_license_status();
                                    self.status = "License file imported".to_string();
                                } else {
                                    self.status = "License file read failed".to_string();
                                }
                            }
                        }
                        if ui.button("Verify License").clicked() {
                            self.refresh_license_status();
                        }
                    });

                    ui.add_space(8.0);
                    ui.separator();
                    ui.label(format!("Device ID: {}", self.settings.device_id));
                    if self.settings.auth_token.is_empty() {
                        ui.label("Auth: not signed in");
                    } else {
                        ui.label("Auth: signed in");
                    }
                    if LICENSE_PUBLIC_KEY_B64.trim().is_empty() {
                        ui.colored_label(egui::Color32::from_rgb(220, 120, 120), "Public key missing");
                    }

                    ui.add_space(8.0);
                    ui.heading("Bundled Synths");
                    ui.separator();
                    for (name, info) in Self::bundled_synths() {
                        ui.horizontal(|ui| {
                            ui.label(name);
                            ui.label(info);
                        });
                    }
                });
            self.show_help_license = open;
        }
    }
}
