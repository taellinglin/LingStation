impl DawApp {
    pub(crate) fn menu_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.scope(|ui| {
                    let mut style = ui.style().as_ref().clone();
                    style.text_styles.insert(
                        egui::TextStyle::Button,
                        egui::FontId::proportional(BASE_UI_FONT_SIZE),
                    );
                    ui.set_style(style);

                    let icon_size = egui::vec2(12.0, 12.0);
                    let menu_text = |text: &str| egui::RichText::new(text).size(BASE_UI_FONT_SIZE);
                    let file_color = egui::Color32::from_rgb(235, 64, 52);
                    let edit_color = egui::Color32::from_rgb(255, 140, 40);
                    let view_color = egui::Color32::from_rgb(245, 205, 70);
                    let transport_color = egui::Color32::from_rgb(80, 200, 120);
                    let help_color = egui::Color32::from_rgb(120, 80, 210);
                    ui.menu_button("File", |ui| {
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../../assets/icons/file-plus.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(file_color),
                                menu_text("New Project"),
                            ))
                            .clicked()
                        {
                        self.request_project_action(ProjectAction::NewProject);
                        ui.close_menu();
                        }
                        let new_template_resp = ui.menu_button(menu_text("New From Template"), |ui| {
                            let templates = self.list_templates();
                            if templates.is_empty() {
                                ui.label("No templates found");
                            } else {
                                for (name, path) in templates {
                                    let button = egui::Button::new(name).frame(false);
                                    if ui.add(button).clicked() {
                                        self.request_project_action(ProjectAction::NewFromTemplate(path));
                                        ui.close_menu();
                                    }
                                }
                            }
                        });
                        if new_template_resp.response.rect.width() > 0.0 {
                            let icon_rect = egui::Rect::from_min_max(
                                egui::pos2(
                                    new_template_resp.response.rect.right() - 16.0,
                                    new_template_resp.response.rect.top(),
                                ),
                                egui::pos2(
                                    new_template_resp.response.rect.right() - 4.0,
                                    new_template_resp.response.rect.bottom(),
                                ),
                            );
                            let bg = if new_template_resp.response.hovered() {
                                ui.visuals().widgets.hovered.bg_fill
                            } else {
                                ui.visuals().panel_fill
                            };
                            let fg = ui.visuals().widgets.inactive.fg_stroke.color;
                            ui.painter().rect_filled(icon_rect, 0.0, bg);
                            ui.put(
                                icon_rect,
                                egui::Image::new(egui::include_image!("../../../assets/icons/chevron-right.svg"))
                                    .fit_to_exact_size(icon_rect.size())
                                    .tint(fg),
                            );
                        }
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../../assets/icons/folder.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(file_color),
                                menu_text("Open Project"),
                            ))
                            .clicked()
                        {
                        self.request_project_action(ProjectAction::OpenProject);
                        ui.close_menu();
                        }
                        let open_recent_resp = ui.menu_button(menu_text("Open Recent"), |ui| {
                            if self.settings.recent_projects.is_empty() {
                                ui.label("No recent projects");
                            } else {
                                for path in self.settings.recent_projects.clone() {
                                    let display = Path::new(&path)
                                        .file_name()
                                        .and_then(|s| s.to_str())
                                        .unwrap_or(path.as_str())
                                        .to_string();
                                    let exists = Path::new(&path).exists();
                                    ui.add_enabled_ui(exists, |ui| {
                                        let button = egui::Button::new(display).frame(false);
                                        if ui.add(button).on_hover_text(&path).clicked() {
                                            self.request_project_action(
                                                ProjectAction::OpenProjectPath(path),
                                            );
                                            ui.close_menu();
                                        }
                                    });
                                }
                            }
                        });
                        if open_recent_resp.response.rect.width() > 0.0 {
                            let icon_rect = egui::Rect::from_min_max(
                                egui::pos2(
                                    open_recent_resp.response.rect.right() - 16.0,
                                    open_recent_resp.response.rect.top(),
                                ),
                                egui::pos2(
                                    open_recent_resp.response.rect.right() - 4.0,
                                    open_recent_resp.response.rect.bottom(),
                                ),
                            );
                            let bg = if open_recent_resp.response.hovered() {
                                ui.visuals().widgets.hovered.bg_fill
                            } else {
                                ui.visuals().panel_fill
                            };
                            let fg = ui.visuals().widgets.inactive.fg_stroke.color;
                            ui.painter().rect_filled(icon_rect, 0.0, bg);
                            ui.put(
                                icon_rect,
                                egui::Image::new(egui::include_image!("../../../assets/icons/chevron-right.svg"))
                                    .fit_to_exact_size(icon_rect.size())
                                    .tint(fg),
                            );
                        }
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../../assets/icons/edit-3.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(file_color),
                                menu_text("Rename Project..."),
                            ))
                            .clicked()
                        {
                        self.begin_rename_project();
                        ui.close_menu();
                        }
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../../assets/icons/save.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(file_color),
                                menu_text("Save Project"),
                            ))
                            .clicked()
                        {
                        if let Err(err) = self.save_project_or_prompt() {
                            self.status = format!("Save failed: {err}");
                        }
                        ui.close_menu();
                        }
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../../assets/icons/save.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(file_color),
                                menu_text("Save Project As..."),
                            ))
                            .clicked()
                        {
                        if let Err(err) = self.save_project_dialog() {
                            self.status = format!("Save failed: {err}");
                        }
                        ui.close_menu();
                        }
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../../assets/icons/save.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(file_color),
                                menu_text("Save New Version"),
                            ))
                            .clicked()
                        {
                        if let Err(err) = self.save_project_new_version() {
                            self.status = format!("Save failed: {err}");
                        }
                        ui.close_menu();
                        }
                        ui.separator();
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../../assets/icons/download.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(file_color),
                                menu_text("Import MIDI"),
                            ))
                            .clicked()
                        {
                        self.request_project_action(ProjectAction::ImportMidi);
                        ui.close_menu();
                        }
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../../assets/icons/upload.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(file_color),
                                menu_text("Export MIDI"),
                            ))
                            .clicked()
                        {
                        if let Err(err) = self.export_midi_dialog() {
                            self.status = format!("Export failed: {err}");
                        }
                        ui.close_menu();
                        }
                        ui.separator();
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../../assets/icons/disc.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(file_color),
                                menu_text("Render to WAV..."),
                            ))
                            .clicked()
                        {
                        self.render_format = RenderFormat::Wav;
                        self.show_render_dialog = true;
                        ui.close_menu();
                        }
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../../assets/icons/disc.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(file_color),
                                menu_text("Render to OGG..."),
                            ))
                            .clicked()
                        {
                        self.render_format = RenderFormat::Ogg;
                        self.show_render_dialog = true;
                        ui.close_menu();
                        }
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../../assets/icons/disc.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(file_color),
                                menu_text("Render to FLAC..."),
                            ))
                            .clicked()
                        {
                        self.render_format = RenderFormat::Flac;
                        self.show_render_dialog = true;
                        ui.close_menu();
                        }
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../../assets/icons/settings.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(file_color),
                                menu_text("Settings..."),
                            ))
                            .clicked()
                        {
                        self.show_settings = true;
                        ui.close_menu();
                        }
                    });
                    ui.menu_button("Edit", |ui| {
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../../assets/icons/corner-left-up.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(edit_color),
                                menu_text("Undo"),
                            ))
                            .clicked()
                        {
                        self.undo();
                        }
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../../assets/icons/corner-right-up.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(edit_color),
                                menu_text("Redo"),
                            ))
                            .clicked()
                        {
                        self.redo();
                        }
                        ui.separator();
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../../assets/icons/scissors.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(edit_color),
                                menu_text("Cut"),
                            ))
                            .clicked()
                        {
                        self.status = "Cut".to_string();
                        }
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../../assets/icons/copy.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(edit_color),
                                menu_text("Copy"),
                            ))
                            .clicked()
                        {
                        self.status = "Copy".to_string();
                        }
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../../assets/icons/clipboard.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(edit_color),
                                menu_text("Paste"),
                            ))
                            .clicked()
                        {
                        self.status = "Paste".to_string();
                        }
                        ui.separator();
                        let auto_slice_resp = ui.menu_button(menu_text("Auto Slice To Performance"), |ui| {
                            for mode in [AutoSliceMode::Smart, AutoSliceMode::Bar, AutoSliceMode::Phrase] {
                                if ui.button(mode.label()).clicked() {
                                    self.push_undo_state();
                                    let summary = if mode == AutoSliceMode::Smart {
                                        self.auto_build_performance_from_arrangement()
                                    } else {
                                        AutoPerformanceBuildSummary {
                                            sections: 0,
                                            slices_created: self.auto_slice_playlist_to_performance(mode),
                                            configured_clips: 0,
                                            loop_clips: 0,
                                        }
                                    };
                                    if !summary.changed() {
                                        self.undo_stack.pop();
                                        self.status = format!("{} found no useful section changes", mode.label());
                                    } else {
                                        self.mark_dirty();
                                        self.status = if mode == AutoSliceMode::Smart {
                                            summary.status_message()
                                        } else {
                                            format!("{} created {} slices and linked playlist flow", mode.label(), summary.slices_created)
                                        };
                                    }
                                    ui.close_menu();
                                }
                            }
                        });
                        if auto_slice_resp.response.rect.width() > 0.0 {
                            let icon_rect = egui::Rect::from_min_max(
                                egui::pos2(
                                    auto_slice_resp.response.rect.right() - 16.0,
                                    auto_slice_resp.response.rect.top(),
                                ),
                                egui::pos2(
                                    auto_slice_resp.response.rect.right() - 4.0,
                                    auto_slice_resp.response.rect.bottom(),
                                ),
                            );
                            let bg = if auto_slice_resp.response.hovered() {
                                ui.visuals().widgets.hovered.bg_fill
                            } else {
                                ui.visuals().panel_fill
                            };
                            let fg = ui.visuals().widgets.inactive.fg_stroke.color;
                            ui.painter().rect_filled(icon_rect, 0.0, bg);
                            ui.put(
                                icon_rect,
                                egui::Image::new(egui::include_image!("../../../assets/icons/chevron-right.svg"))
                                    .fit_to_exact_size(icon_rect.size())
                                    .tint(fg),
                            );
                        }
                    });
                    ui.menu_button("View", |ui| {
                        let mut show = self.show_project_info;
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::Image::new(egui::include_image!("../../../assets/icons/info.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(view_color),
                            );
                            if ui.checkbox(&mut show, "Project Info").changed() {
                                self.show_project_info = show;
                            }
                        });
                        let mut show_meta = self.show_metadata;
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::Image::new(egui::include_image!("../../../assets/icons/tag.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(view_color),
                            );
                            if ui.checkbox(&mut show_meta, "Metadata").changed() {
                                self.show_metadata = show_meta;
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::Image::new(egui::include_image!("../../../assets/icons/crosshair.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(view_color),
                            );
                            ui.checkbox(&mut self.show_hitboxes, "Debug Hitboxes");
                        });
                    });
                    ui.menu_button("Transport", |ui| {
                        let rendering = self.render_job.is_some();
                        if ui
                            .add_enabled(
                                !rendering,
                                egui::Button::image_and_text(
                                    egui::Image::new(egui::include_image!("../../../assets/icons/play.svg"))
                                        .fit_to_exact_size(icon_size)
                                        .tint(transport_color),
                                    menu_text("Play"),
                                ),
                            )
                            .clicked()
                        {
                        self.set_arrangement_playback_enabled(true);
                        if let Err(err) = self.start_audio_and_midi() {
                            self.status = format!("Play failed: {err}");
                        }
                        }
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../../assets/icons/stop-circle.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(transport_color),
                                menu_text("Stop"),
                            ))
                            .clicked()
                        {
                        if self.is_recording {
                            if let Err(err) = self.end_recording() {
                                self.status = format!("Stop recording failed: {err}");
                            }
                        } else {
                            self.stop_audio_and_midi();
                            self.status = "Stop".to_string();
                        }
                        }
                        if ui
                            .add_enabled(
                                !rendering,
                                egui::Button::image_and_text(
                                    egui::Image::new(egui::include_image!("../../../assets/icons/circle.svg"))
                                        .fit_to_exact_size(icon_size)
                                        .tint(transport_color),
                                    menu_text("Record"),
                                ),
                            )
                            .clicked()
                        {
                        self.toggle_recording();
                        }
                    });
                    ui.menu_button("Help", |ui| {
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../../assets/icons/help-circle.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(help_color),
                                menu_text("About"),
                            ))
                            .clicked()
                        {
                        self.show_help_about = true;
                        ui.close_menu();
                        }
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../../assets/icons/key.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(help_color),
                                menu_text("License"),
                            ))
                            .clicked()
                        {
                        self.show_help_license = true;
                        ui.close_menu();
                        }
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../../assets/icons/command.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(help_color),
                                menu_text("Shortcuts"),
                            ))
                            .clicked()
                        {
                        self.show_help_shortcuts = true;
                        ui.close_menu();
                        }
                        if ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new(egui::include_image!("../../../assets/icons/book-open.svg"))
                                    .fit_to_exact_size(icon_size)
                                    .tint(help_color),
                                menu_text("Help"),
                            ))
                            .clicked()
                        {
                        self.show_help_general = true;
                        ui.close_menu();
                        }
                    });
                });
            });
        });
    }

    pub(crate) fn view_tabs(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("view_tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Views");
                ui.toggle_value(&mut self.show_sidebar, "Sidebar");
                ui.toggle_value(&mut self.show_mixer, "Mixer");
                ui.toggle_value(&mut self.show_transport, "Transport");
                ui.separator();
                ui.label("Editor");
                ui.selectable_value(&mut self.main_tab, MainTab::Arranger, "Arranger");
                ui.selectable_value(&mut self.main_tab, MainTab::Parameters, "Parameters");
                ui.selectable_value(&mut self.main_tab, MainTab::PianoRoll, "Piano Roll");
                ui.selectable_value(&mut self.main_tab, MainTab::NodeEditor, "Node Editor");
                ui.selectable_value(&mut self.main_tab, MainTab::Performance, "Performance");
            });
        });
    }

    pub(crate) fn toolbar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let rendering = self.render_job.is_some();
                let play_icon = egui::Image::new(egui::include_image!("../../../assets/icons/play.svg"))
                    .fit_to_exact_size(egui::vec2(16.0, 16.0));
                if ui
                    .add_enabled(
                        self.audio_running || !rendering,
                        egui::Button::image(if self.audio_running {
                            egui::Image::new(egui::include_image!("../../../assets/icons/pause.svg"))
                                .fit_to_exact_size(egui::vec2(16.0, 16.0))
                        } else {
                            play_icon.clone()
                        }),
                    )
                    .on_hover_text(if self.audio_running { "Pause" } else { "Play" })
                    .clicked()
                {
                    if self.audio_running {
                        self.pause_audio_and_midi();
                        self.status = "Paused".to_string();
                    } else {
                        self.seek_playhead(self.playhead_beats);
                        self.set_arrangement_playback_enabled(true);
                        if let Err(err) = self.start_audio_and_midi_internal(false) {
                            self.status = format!("Play failed: {err}");
                        }
                    }
                }
                let stop_icon = egui::Image::new(egui::include_image!("../../../assets/icons/stop-circle.svg"))
                    .fit_to_exact_size(egui::vec2(16.0, 16.0));
                if ui
                    .add(egui::Button::image(stop_icon))
                    .on_hover_text("Stop")
                    .clicked()
                {
                    self.stop_audio_and_midi();
                    self.status = "Stop".to_string();
                }
                let rec_icon = egui::Image::new(egui::include_image!("../../../assets/icons/circle.svg"))
                    .fit_to_exact_size(egui::vec2(14.0, 14.0));
                if ui
                    .add_enabled(!rendering, egui::Button::image(rec_icon))
                    .on_hover_text("Rec")
                    .clicked()
                {
                    self.toggle_recording();
                }
                if ui
                    .add(egui::Button::image(
                        egui::Image::new(egui::include_image!("../../../assets/icons/repeat.svg"))
                            .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                    ))
                    .on_hover_text("Loop Song")
                    .clicked()
                {
                    if self.loop_start_beats.is_some() && self.loop_end_beats.is_some() {
                        self.loop_start_beats = None;
                        self.loop_end_beats = None;
                        self.status = "Loop: off".to_string();
                    } else if let Some((start, end)) = self.project_clip_range() {
                        self.loop_start_beats = Some(start);
                        self.loop_end_beats = Some(end);
                        self.status = "Loop: song range".to_string();
                    } else {
                        self.status = "Loop: no clips".to_string();
                    }
                }
                ui.label("Record:");
                ui.checkbox(&mut self.record_audio, "Audio");
                ui.checkbox(&mut self.record_midi, "MIDI");
                ui.checkbox(&mut self.record_automation, "Automation");
                ui.checkbox(&mut self.record_performance, "Performance");
                ui.separator();
                ui.label("Tempo");
                let mut tempo = f32::from_bits(self.engine.tempo_bpm.load(Ordering::Relaxed));
                if ui.add(egui::DragValue::new(&mut tempo).speed(1.0)).changed() {
                    self.engine.tempo_bpm.store(tempo.to_bits(), Ordering::Relaxed);
                }
                ui.separator();
                ui.label(&self.status);
                if self.show_hitboxes {
                    if let Some(PluginHostHandle::Vst3(host)) = self.selected_track_host() {
                        if let Some(host) = host.try_lock() {
                            let last = host.debug_last_param_change();
                            let count = host.debug_last_process_param_count();
                            let (param_count, id_count) = self
                                .selected_track
                                .and_then(|i| self.tracks.get(i))
                                .map(|t| (t.params.len(), t.param_ids.len()))
                                .unwrap_or((0, 0));
                            ui.separator();
                            let ui_change = self
                                .last_ui_param_change
                                .map(|(id, value)| format!("ui {id}={value:.3}"))
                                .unwrap_or_else(|| "ui none".to_string());
                            if let Some((id, value)) = last {
                                ui.label(format!(
                                    "Param {id}={value:.3} | block {count} | {ui_change} | params {param_count} ids {id_count}"
                                ));
                            } else {
                                ui.label(format!(
                                    "Param none | block {count} | {ui_change} | params {param_count} ids {id_count}"
                                ));
                            }
                        }
                    }
                    let (blocks, overruns, last_ms, max_ms) = self.engine.stats.snapshot();
                    ui.label(format!(
                        "Audio blocks {blocks} | overruns {overruns} | last {last_ms:.2} ms | max {max_ms:.2} ms"
                    ));
                    ui.label(format!(
                        "UI frame {0:.2} ms | max {1:.2} ms",
                        self.ui_frame_last_ms,
                        self.ui_frame_max_ms
                    ));
                    if matches!(self.main_tab, MainTab::Arranger) {
                        ui.label(format!(
                            "Arranger {0:.2} ms | max {1:.2} ms",
                            self.ui_arranger_last_ms,
                            self.ui_arranger_max_ms
                        ));
                    }
                    {
                        let cache = self.engine.audio_cache.lock();
                        let mb = cache.bytes as f32 / (1024.0 * 1024.0);
                        ui.label(format!(
                            "Clip cache: {} items | {:.1} MB",
                            cache.entries.len(),
                            mb
                        ));
                    }
                    let waveform_bytes = self
                        .waveform_cache
                        .borrow()
                        .values()
                        .map(|v| v.len().saturating_mul(std::mem::size_of::<f32>()))
                        .sum::<usize>();
                    let waveform_color_bytes = self
                        .waveform_color_cache
                        .borrow()
                        .values()
                        .map(|v| v.len().saturating_mul(std::mem::size_of::<[f32; 3]>()))
                        .sum::<usize>();
                    let waveform_mb = waveform_bytes as f32 / (1024.0 * 1024.0);
                    let waveform_color_mb = waveform_color_bytes as f32 / (1024.0 * 1024.0);
                    ui.label(format!(
                        "Waveforms: {} | {:.1} MB | Colors: {} | {:.1} MB",
                        self.waveform_cache.borrow().len(),
                        waveform_mb,
                        self.waveform_color_cache.borrow().len(),
                        waveform_color_mb
                    ));
                }
            });

            let raw_peak = f32::from_bits(self.engine.master_peak_bits.load(Ordering::Relaxed));
            self.master_peak_display = (self.master_peak_display * 0.92).max(raw_peak);
            let meter_value = self.master_peak_display.clamp(0.0, 1.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label("Master");
                let meter_size = egui::vec2(160.0, 14.0);
                let (rect, _) = ui.allocate_exact_size(meter_size, egui::Sense::hover());
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 3.0, egui::Color32::from_rgb(24, 28, 32));
                let fill_w = rect.width() * meter_value;
                if fill_w > 0.0 {
                    let fill_rect = egui::Rect::from_min_size(rect.min, egui::vec2(fill_w, rect.height()));
                    let color = if meter_value > 0.9 {
                        egui::Color32::from_rgb(255, 90, 64)
                    } else if meter_value > 0.7 {
                        egui::Color32::from_rgb(250, 200, 80)
                    } else {
                        egui::Color32::from_rgb(90, 210, 120)
                    };
                    painter.rect_filled(fill_rect, 3.0, color);
                }
            });
        });
    }

    pub(crate) fn plugin_ui_window(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(ui_host) = self.plugin_ui.as_ref() {
            if ui_host.close_requested.swap(false, Ordering::Relaxed) {

                if let Some(ui_host) = self.plugin_ui.as_ref() {
                    if let PluginUiEditor::Vst3(editor) = &ui_host.editor {
                        editor.set_focus(false);

                    }
                    if let PluginUiEditor::Clap(_) = &ui_host.editor {
                        if let PluginHostHandle::Clap(host) = &ui_host.host {
                            if let Some(mut host) = host.try_lock() {
                                host.hide_gui();

                            }
                        }
                    }
                    hide_plugin_window(ui_host.hwnd);

                }
                self.show_plugin_ui = false;
                if self.plugin_ui.is_some() {
                    self.plugin_ui_hidden = true;
                }
                self.pending_viewport_focus = true;
                self.pending_repaint_frames = self.pending_repaint_frames.max(12);

                ctx.request_repaint();
                return;
            }
        }
        let should_close_hidden = self
            .plugin_ui
            .as_ref()
            .map(|ui_host| !ui_host.floating && !is_window_visible(ui_host.hwnd))
            .unwrap_or(false)
            && self.show_plugin_ui
            && !self.plugin_ui_hidden;
        if should_close_hidden {

            if let Some(ui_host) = self.plugin_ui.as_ref() {
                if let PluginUiEditor::Vst3(editor) = &ui_host.editor {
                    editor.set_focus(false);

                }
                if let PluginUiEditor::Clap(_) = &ui_host.editor {
                    if let PluginHostHandle::Clap(host) = &ui_host.host {
                        if let Some(mut host) = host.try_lock() {
                            host.hide_gui();

                        }
                    }
                }
            }
            self.show_plugin_ui = false;
            if self.plugin_ui.is_some() {
                self.plugin_ui_hidden = true;
            }
            self.pending_viewport_focus = true;
            self.pending_repaint_frames = self.pending_repaint_frames.max(12);
            ctx.request_repaint();
            return;
        }
        if let Some(ui_host) = self.plugin_ui.as_ref() {
            if !is_window_visible(ui_host.hwnd) {
                if self.show_plugin_ui {
                    if self.plugin_ui_hidden {
                        show_plugin_window(ui_host.hwnd);
                        self.plugin_ui_hidden = false;
                    } else {
                        self.show_plugin_ui = false;
                        self.plugin_ui_hidden = true;
                        ctx.request_repaint();
                        return;
                    }
                } else {
                    self.plugin_ui_hidden = true;
                    ctx.request_repaint();
                    return;
                }
            }
        }
        if !self.show_plugin_ui {
            let should_focus = self.plugin_ui.is_some() && !self.plugin_ui_hidden;
            if should_focus {
                if let Some(ui_host) = self.plugin_ui.as_ref() {
                    if let PluginUiEditor::Vst3(editor) = &ui_host.editor {
                        editor.set_focus(false);
                    }
                    hide_plugin_window(ui_host.hwnd);
                }
                if self.plugin_ui.is_some() {
                    self.plugin_ui_hidden = true;
                }
                self.pending_viewport_focus = true;
                self.pending_repaint_frames = self.pending_repaint_frames.max(12);
                ctx.request_repaint();
            }
            return;
        }

        if let Some(ui_host) = self.plugin_ui.as_ref() {
            if !is_window_alive(ui_host.hwnd) {
                self.plugin_ui = None;
                self.plugin_ui_target = None;
                self.show_plugin_ui = false;
                self.plugin_ui_hidden = false;
                self.pending_viewport_focus = true;
                ctx.request_repaint();
                return;
            }
            if ui_host.child_hwnd != ui_host.hwnd && !is_window_alive(ui_host.child_hwnd) {
                self.plugin_ui = None;
                self.plugin_ui_target = None;
                self.show_plugin_ui = false;
                self.plugin_ui_hidden = false;
                self.pending_viewport_focus = true;
                ctx.request_repaint();
                return;
            }
        }

        if let Some(ui_host) = self.plugin_ui.as_ref() {
            let mut restored_visibility = false;
            if self.plugin_ui_hidden {
                show_plugin_window(ui_host.hwnd);
                self.plugin_ui_hidden = false;
                restored_visibility = true;
            }
            pump_plugin_messages(ui_host.hwnd);
            show_plugin_window(ui_host.hwnd);
            if restored_visibility {
                bring_window_to_front(ui_host.hwnd);
            }
            match &ui_host.editor {
                PluginUiEditor::Vst3(editor) => {
                    if restored_visibility {
                        editor.set_focus(true);
                    }
                    if let Some((cw, ch)) = client_window_size(ui_host.child_hwnd) {
                        editor.set_size(cw, ch);
                    }
                }
                PluginUiEditor::Clap(_) => {
                    if let PluginHostHandle::Clap(host) = &ui_host.host {
                        if let Some(mut host) = host.try_lock() {
                            let request_hide = host.take_gui_request_hide();
                            let request_show = host.take_gui_request_show();
                            if let Some((gw, gh)) = host.take_gui_resize() {
                                move_plugin_child_window(
                                    ui_host.child_hwnd,
                                    0,
                                    0,
                                    gw.max(200),
                                    gh.max(120),
                                );
                                resize_plugin_top_window(ui_host.hwnd, gw.max(200), gh.max(120));
                            }
                            // Some CLAP plugins emit a transient hide request right after opening.
                            // Ignore it while the editor is meant to stay visible.
                            if request_show || restored_visibility || !host.gui_is_open() || request_hide {
                                host.show_gui();
                            }
                        }
                    }
                }
            }
            invalidate_plugin_window(ui_host.child_hwnd);
            invalidate_plugin_window(ui_host.hwnd);
        }
        if self.show_plugin_ui {
            let desired_target = self
                .plugin_ui_target
                .or_else(|| self.selected_track.map(PluginUiTarget::Instrument));
            let needs_open = match (self.plugin_ui.as_ref(), desired_target) {
                (None, Some(_)) => true,
                (Some(ui_host), Some(target)) => ui_host.target != target,
                _ => false,
            };
            if needs_open {
                self.ensure_plugin_ui();
            }
        }

        let mut open = self.show_plugin_ui;
        let mut close_editor = false;
        egui::Window::new("Plugin UI")
            .open(&mut open)
            .default_size(egui::vec2(520.0, 200.0))
            .show(ctx, |ui| {
                ui.label("Plugin editor is in a native window.");
                if ui
                    .add(egui::Button::image(
                        egui::Image::new(egui::include_image!("../../../assets/icons/external-link.svg"))
                            .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                    ))
                    .on_hover_text("Bring To Front")
                    .clicked()
                {
                    if let Some(ui_host) = self.plugin_ui.as_ref() {
                        bring_window_to_front(ui_host.hwnd);
                        if let PluginUiEditor::Vst3(editor) = &ui_host.editor {
                            editor.set_focus(true);
                        }
                    }
                }
                if ui
                    .add(egui::Button::image(
                        egui::Image::new(egui::include_image!("../../../assets/icons/x.svg"))
                            .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                    ))
                    .on_hover_text("Close Editor")
                    .clicked()
                {
                    close_editor = true;
                }
            });
        if close_editor {

            if let Some(ui_host) = self.plugin_ui.as_ref() {
                if let PluginUiEditor::Vst3(editor) = &ui_host.editor {
                    editor.set_focus(false);

                }
                if let PluginUiEditor::Clap(_) = &ui_host.editor {
                    if let PluginHostHandle::Clap(host) = &ui_host.host {
                        if let Some(mut host) = host.try_lock() {
                            host.hide_gui();

                        }
                    }
                }
                hide_plugin_window(ui_host.hwnd);

                release_mouse_capture();
            }
            open = false;
            if self.plugin_ui.is_some() {
                self.plugin_ui_hidden = true;
            }
            self.pending_viewport_focus = true;
            self.pending_repaint_frames = self.pending_repaint_frames.max(12);
            ctx.request_repaint();
        }
        self.show_plugin_ui = open;
        if !self.show_plugin_ui {
            let should_focus = self.plugin_ui.is_some() && !self.plugin_ui_hidden;
            if should_focus {
                if let Some(ui_host) = self.plugin_ui.as_ref() {
                    if let PluginUiEditor::Vst3(editor) = &ui_host.editor {
                        editor.set_focus(false);
                    }
                    if let PluginUiEditor::Clap(_) = &ui_host.editor {
                        if let PluginHostHandle::Clap(host) = &ui_host.host {
                            if let Some(mut host) = host.try_lock() {
                                host.hide_gui();
                            }
                        }
                    }
                    if is_window_alive(ui_host.hwnd) && is_window_visible(ui_host.hwnd) && !self.plugin_ui_hidden {
                        hide_plugin_window(ui_host.hwnd);
                    }
                }
                if self.plugin_ui.is_some() {
                    self.plugin_ui_hidden = true;
                }
                self.pending_viewport_focus = true;
                self.pending_repaint_frames = self.pending_repaint_frames.max(12);
                ctx.request_repaint();
            }
        }
    }

    pub(crate) fn ensure_plugin_ui(&mut self) {
        vst3::init_windows_com_for_thread();
        let target = self
            .plugin_ui_target
            .or_else(|| self.selected_track.map(PluginUiTarget::Instrument));
        if let Some(ui_host) = self.plugin_ui.as_ref() {
            if let Some(target) = target {
                if ui_host.target == target {
                    return;
                }
            }
            self.destroy_plugin_ui();
        }
        let Some(target) = target else {
            self.status = "No track selected".to_string();
            return;
        };

        let host = match target {
            PluginUiTarget::Instrument(index) => self
                .selected_track_host()
                .or_else(|| self.ensure_track_host(index, 2)),
            PluginUiTarget::Effect(track_index, fx_index) => {
                self.ensure_effect_host(track_index, fx_index, 2)
            }
        };
        let Some(host) = host else {
            self.status = "No plugin host for UI".to_string();
            return;
        };

        match &host {
            PluginHostHandle::Vst3(vst_host) => {
                let mut editor = {
                    let host_guard = match vst_host.try_lock() {
                        Some(host) => host,
                        None => {
                            self.status = "Plugin busy; try again".to_string();
                            return;
                        }
                    };
                    match host_guard.create_editor() {
                        Some(editor) => editor,
                        None => {
                            self.status = "Plugin has no UI".to_string();
                            return;
                        }
                    }
                };
                let (w, h) = editor.get_size().unwrap_or((520, 360));

                let hwnd = match create_plugin_top_window(w, h) {
                    Some(hwnd) => hwnd,
                    None => {
                        self.status = "Failed to create plugin UI window".to_string();
                        return;
                    }
                };
                resize_plugin_top_window(hwnd, w.max(200), h.max(120));

                let mut child_hwnd = match create_plugin_child_window(hwnd) {
                    Some(child_hwnd) => child_hwnd,
                    None => {
                        self.status = "Failed to create plugin UI child window".to_string();
                        destroy_plugin_child_window(hwnd);
                        return;
                    }
                };
                move_plugin_child_window(child_hwnd, 0, 0, w.max(200), h.max(120));
                let mut attached = editor.attach_hwnd(child_hwnd).is_ok();
                if !attached {
                    destroy_plugin_child_window(child_hwnd);
                    child_hwnd = hwnd;
                    attached = editor.attach_hwnd(child_hwnd).is_ok();
                }
                if !attached {
                    self.status = "VST3 view attach failed".to_string();
                    destroy_plugin_child_window(hwnd);
                    return;
                }

                let (cw, ch) = client_window_size(child_hwnd).unwrap_or((w, h));
                editor.set_size(cw, ch);
                editor.set_focus(true);
                bring_window_to_front(hwnd);
                invalidate_plugin_window(child_hwnd);
                invalidate_plugin_window(hwnd);
                let close_requested = Arc::new(AtomicBool::new(false));
                set_plugin_close_flag(hwnd, &close_requested);
                self.plugin_ui = Some(PluginUiHost {
                    hwnd,
                    child_hwnd,
                    editor: PluginUiEditor::Vst3(editor),
                    host: host.clone(),
                    target,
                    close_requested,
                    floating: false,
                });
            }
            PluginHostHandle::Clap(clap_host) => {
                let (w, h) = clap_host.lock().gui_size().unwrap_or((520, 360));

                let hwnd = match create_plugin_top_window(w, h) {
                    Some(hwnd) => hwnd,
                    None => {
                        self.status = "Failed to create plugin UI window".to_string();
                        return;
                    }
                };
                resize_plugin_top_window(hwnd, w.max(200), h.max(120));
                let mut child_hwnd = match create_plugin_child_window(hwnd) {
                    Some(child_hwnd) => child_hwnd,
                    None => {
                        self.status = "Failed to create plugin UI child window".to_string();
                        destroy_plugin_child_window(hwnd);
                        return;
                    }
                };
                move_plugin_child_window(child_hwnd, 0, 0, w.max(200), h.max(120));
                let mut is_embedded = true;
                let mut clap_guard = match clap_host.try_lock() {
                    Some(host) => host,
                    None => {
                        self.status = "CLAP plugin busy; try opening UI again".to_string();
                        destroy_plugin_child_window(hwnd);
                        self.show_plugin_ui = false;
                        self.plugin_ui_hidden = false;
                        return;
                    }
                };
                let mut attached = clap_guard.open_gui(child_hwnd).is_ok();
                if !attached {
                    destroy_plugin_child_window(child_hwnd);
                    child_hwnd = hwnd;
                    attached = clap_guard.open_gui(hwnd).is_ok();
                }
                if attached {
                    is_embedded = clap_guard.gui_embedded();
                    if let Some((gw, gh)) = clap_guard.gui_size() {
                        let target_hwnd = if clap_guard.gui_embedded() { child_hwnd } else { hwnd };
                        move_plugin_child_window(target_hwnd, 0, 0, gw.max(200), gh.max(120));
                        resize_plugin_top_window(hwnd, gw.max(200), gh.max(120));
                    }
                }
                if !attached {
                    self.status = "CLAP view attach failed".to_string();
                    destroy_plugin_child_window(hwnd);
                    self.show_plugin_ui = false;
                    self.plugin_ui_hidden = false;
                    return;
                }
                bring_window_to_front(hwnd);
                invalidate_plugin_window(child_hwnd);
                invalidate_plugin_window(hwnd);
                let close_requested = Arc::new(AtomicBool::new(false));
                set_plugin_close_flag(hwnd, &close_requested);
                self.plugin_ui = Some(PluginUiHost {
                    hwnd,
                    child_hwnd,
                    editor: PluginUiEditor::Clap(clap_host.clone()),
                    host: host.clone(),
                    target,
                    close_requested,
                    floating: !is_embedded,
                });
            }
        }
    }

    pub(crate) fn destroy_plugin_ui(&mut self) {
        let Some(mut ui_host) = self.plugin_ui.take() else {
            return;
        };

        if is_window_alive(ui_host.hwnd) {
            hide_plugin_window(ui_host.hwnd);
            pump_plugin_messages(ui_host.hwnd);
        }

        match &mut ui_host.editor {
            PluginUiEditor::Vst3(editor) => {
                editor.set_focus(false);
                editor.removed();
            }
            PluginUiEditor::Clap(_) => {
                if let PluginHostHandle::Clap(host) = &ui_host.host {
                    if let Some(mut host) = host.try_lock() {
                        host.hide_gui();
                    }
                }
            }
        }

        if ui_host.child_hwnd != ui_host.hwnd && is_window_alive(ui_host.child_hwnd) {
            destroy_plugin_child_window(ui_host.child_hwnd);
        }
        if is_window_alive(ui_host.hwnd) {
            destroy_plugin_child_window(ui_host.hwnd);
        }
        release_mouse_capture();
        self.plugin_ui_target = None;
        self.plugin_ui_hidden = false;
    }
}
