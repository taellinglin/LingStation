impl eframe::App for DawApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let frame_start = std::time::Instant::now();
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.project_dirty {
                self.pending_exit = true;
                self.show_close_confirm = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        self.poll_license_job();
        self.poll_audio_analysis_jobs();
        self.sync_last_param_changes();
        self.update_performance_auto_follow();
        let viewport = ctx.input(|i| i.viewport().clone());
        let viewport_has_size = viewport
            .outer_rect
            .map(|rect| rect.width() > 0.0 && rect.height() > 0.0)
            .unwrap_or(false);
        if viewport_has_size {
            self.seen_nonzero_viewport = true;
        }
        if self.pending_startup_maximize
            && self.seen_nonzero_viewport
            && viewport.maximized != Some(true)
        {
            self.pending_startup_maximize = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
            ctx.request_repaint();
        }
        if self.last_viewport_maximized != viewport.maximized {
            self.last_viewport_maximized = viewport.maximized;
            ctx.request_repaint();
        }
        if self.last_viewport_rect != viewport.outer_rect {
            self.last_viewport_rect = viewport.outer_rect;
            ctx.request_repaint();
        }
        if self.pending_viewport_focus {
            self.pending_viewport_focus = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            ctx.request_repaint();
        }
        if self.pending_repaint_frames > 0 {
            self.pending_repaint_frames = self.pending_repaint_frames.saturating_sub(1);
            ctx.request_repaint();
        }
        self.apply_theme(ctx);
        self.paint_wallpaper(ctx);
        if self
            .adaptive_restart_requested
            .swap(false, Ordering::Relaxed)
        {
            let effective = self.adaptive_buffer_size.load(Ordering::Relaxed);
            if effective > 0 {
                let base = if self.settings.triple_buffer {
                    (effective / 3).max(1)
                } else {
                    effective
                };
                self.buffer_override = Some(base);
                if self.audio_running {
                    self.adaptive_restart_pending = true;
                    self.status = format!(
                        "Audio buffer increase pending (stop to apply)"
                    );
                } else {
                    self.adaptive_restart_pending = false;
                    self.status = format!("Audio buffer set to {effective} samples");
                }
            }
        }
        if let Some(when) = self.plugin_ui_resume_at {
            if std::time::Instant::now() >= when {
                self.plugin_ui_resume_at = None;
                if !self.audio_running {
                    if let Err(err) = self.start_audio_and_midi() {
                        self.status = format!("Audio resume failed: {err}");
                    }
                }
            }
        }
        self.sync_selected_track_index();
        self.handle_shortcuts(ctx);
        self.update_playhead(ctx);
        self.update_autosave();
        self.menu_bar(ctx);
        self.view_tabs(ctx);
        self.piano_roll_hovered = if matches!(self.main_tab, MainTab::Parameters | MainTab::PianoRoll) {
            let pointer_pos = ctx.input(|i| i.pointer.hover_pos());
            self.piano_roll_rect
                .and_then(|rect| pointer_pos.map(|pos| rect.contains(pos)))
                .unwrap_or(false)
        } else {
            false
        };
        if self.show_transport {
            self.toolbar(ctx);
        }
        if self.show_sidebar {
            self.left_sidebar(ctx);
        }
        if self.show_mixer {
            self.mixer_panel(ctx);
        }
        if self.show_project_info {
            self.project_info_panel(ctx);
        }
        match self.main_tab {
            MainTab::Arranger => {
                let arranger_start = std::time::Instant::now();
                self.center_arranger(ctx);
                let arranger_ms = arranger_start.elapsed().as_secs_f32() * 1000.0;
                self.ui_arranger_last_ms = arranger_ms;
                if arranger_ms > self.ui_arranger_max_ms {
                    self.ui_arranger_max_ms = arranger_ms;
                }
            }
            MainTab::Parameters => {
                self.ui_arranger_last_ms = 0.0;
                self.center_parameters(ctx);
            }
            MainTab::PianoRoll => {
                self.ui_arranger_last_ms = 0.0;
                self.center_piano_roll(ctx);
            }
            MainTab::NodeEditor => {
                self.ui_arranger_last_ms = 0.0;
                self.center_node_editor(ctx);
            }
            MainTab::Performance => {
                self.ui_arranger_last_ms = 0.0;
                self.center_performance(ctx);
            }
        }
        self.plugin_ui_window(ctx, frame);
        self.modals(ctx);
        if self.pending_project_action.is_some() && !self.show_close_confirm {
            if let Some(action) = self.pending_project_action.take() {
                self.perform_project_action(action);
                self.pending_viewport_focus = true;
                self.pending_repaint_frames = self.pending_repaint_frames.max(12);
                ctx.request_repaint();
            }
        }
        if self.exit_confirmed {
            self.exit_confirmed = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if self.render_job.is_some() {
            ctx.request_repaint();
        }
        if self.audio_running {
            ctx.request_repaint();
        }
        if self.show_render_dialog {
            let mut open = self.show_render_dialog;
            let mut do_render = false;
            let mut close_requested = false;
            let project_end = self.project_end_beats().max(0.25);
            if self.render_range_end <= 0.0 {
                self.render_range_end = project_end;
            }
            if self.render_range_start < 0.0 {
                self.render_range_start = 0.0;
            }
            if let Some(job) = self.render_job.as_ref() {
                let done = job.done.load(Ordering::Relaxed);
                let total = job.total.load(Ordering::Relaxed);
                if total > 0 {
                    self.render_progress = Some((done, total));
                }
                if job.finished.load(Ordering::Relaxed) {
                    if let Ok(mut result) = job.result.lock() {
                        if let Some(result) = result.take() {
                            match result {
                                Ok(msg) => {
                                    self.status = msg;
                                    close_requested = true;
                                }
                                Err(err) => {
                                    self.status = format!("Render failed: {err}");
                                }
                            }
                        }
                    }
                    self.render_job = None;
                }
            }
            egui::Window::new("Render")
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.heading("Export Audio");
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label("Format");
                        let format_label = match self.render_format {
                            RenderFormat::Wav => "WAV",
                            RenderFormat::Ogg => "OGG",
                            RenderFormat::Flac => "FLAC",
                        };
                        ui.label(format_label);
                    });
                    ui.horizontal(|ui| {
                        ui.label("Sample Rate");
                        egui::ComboBox::from_id_source("render_sample_rate")
                            .selected_text(format!("{}", self.render_sample_rate))
                            .show_ui(ui, |ui| {
                                for rate in [44_100u32, 48_000, 88_200, 96_000, 176_400, 192_000] {
                                    if ui.selectable_label(self.render_sample_rate == rate, format!("{}", rate)).clicked() {
                                        self.render_sample_rate = rate;
                                    }
                                }
                            });
                    });
                    if self.render_format == RenderFormat::Wav {
                        ui.horizontal(|ui| {
                            ui.label("Bit Depth");
                            let label = self.render_wav_bit_depth.label();
                            egui::ComboBox::from_id_source("render_wav_bit_depth")
                                .selected_text(label)
                                .show_ui(ui, |ui| {
                                    for depth in RenderWavBitDepth::all() {
                                        let depth_label = depth.label();
                                        if ui
                                            .selectable_label(self.render_wav_bit_depth == depth, depth_label)
                                            .clicked()
                                        {
                                            self.render_wav_bit_depth = depth;
                                        }
                                    }
                                });
                        });
                    }
                    ui.horizontal(|ui| {
                        ui.label("Bitrate");
                        egui::ComboBox::from_id_source("render_bitrate")
                            .selected_text(format!("{} kbps", self.render_bitrate))
                            .show_ui(ui, |ui| {
                                for rate in [96u32, 128, 192, 256, 320] {
                                    if ui.selectable_label(self.render_bitrate == rate, format!("{} kbps", rate)).clicked() {
                                        self.render_bitrate = rate;
                                    }
                                }
                            });
                    });
                    ui.horizontal(|ui| {
                        ui.label("Tail Mode");
                        let label = match self.render_tail_mode {
                            RenderTailMode::Wrap => "Wrap",
                            RenderTailMode::Release => "Release",
                            RenderTailMode::Cut => "Cut",
                        };
                        egui::ComboBox::from_id_source("render_tail_mode")
                            .selected_text(label)
                            .show_ui(ui, |ui| {
                                if ui.selectable_label(self.render_tail_mode == RenderTailMode::Wrap, "Wrap").clicked() {
                                    self.render_tail_mode = RenderTailMode::Wrap;
                                }
                                if ui.selectable_label(self.render_tail_mode == RenderTailMode::Release, "Release").clicked() {
                                    self.render_tail_mode = RenderTailMode::Release;
                                }
                                if ui.selectable_label(self.render_tail_mode == RenderTailMode::Cut, "Cut").clicked() {
                                    self.render_tail_mode = RenderTailMode::Cut;
                                }
                            });
                    });
                    if self.render_tail_mode == RenderTailMode::Release {
                        ui.horizontal(|ui| {
                            ui.label("Release Tail (s)");
                            ui.add(egui::DragValue::new(&mut self.render_release_seconds).speed(0.25));
                        });
                    }
                    ui.checkbox(&mut self.render_split_tracks, "Split tracks + Master");
                    ui.add_space(6.0);
                    if let Some((done, total)) = self.render_progress {
                        let progress = if total == 0 {
                            0.0
                        } else {
                            (done as f32 / total as f32).clamp(0.0, 1.0)
                        };
                        ui.add(egui::ProgressBar::new(progress).show_percentage());
                    }
                    ui.separator();
                    ui.label("Render Range (beats)");
                    ui.horizontal(|ui| {
                        ui.label("Start");
                        ui.add(egui::DragValue::new(&mut self.render_range_start).speed(0.25));
                        ui.label("End");
                        ui.add(egui::DragValue::new(&mut self.render_range_end).speed(0.25));
                        if ui
                            .add(egui::Button::image(
                                egui::Image::new(egui::include_image!("../../../assets/icons/repeat.svg"))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                            ))
                            .on_hover_text("Use Loop")
                            .clicked()
                        {
                            if let (Some(start), Some(end)) = (self.loop_start_beats, self.loop_end_beats) {
                                self.render_range_start = start.max(0.0);
                                self.render_range_end = end.max(start + 0.25);
                            }
                        }
                        if ui
                            .add(egui::Button::image(
                                egui::Image::new(egui::include_image!("../../../assets/icons/music.svg"))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                            ))
                            .on_hover_text("Full Song")
                            .clicked()
                        {
                            self.render_range_start = 0.0;
                            self.render_range_end = project_end;
                        }
                    });
                    if self.render_range_end <= self.render_range_start {
                        ui.label("Range end must be greater than start; end will default to song end.");
                    }
                    ui.horizontal(|ui| {
                        let dir_label = self
                            .render_target_dir
                            .as_ref()
                            .map(|d| d.to_string_lossy().to_string())
                            .unwrap_or_else(|| "(choose folder)".to_string());
                        if ui
                            .add(egui::Button::image(
                                egui::Image::new(egui::include_image!("../../../assets/icons/folder.svg"))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                            ))
                            .on_hover_text("Choose Folder")
                            .clicked()
                        {
                            if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                                self.render_target_dir = Some(folder);
                            }
                        }
                        ui.label(dir_label);
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let rendering = self.render_job.is_some();
                        let render_btn = ui.add_enabled(
                            !rendering,
                            egui::Button::image(
                                egui::Image::new(egui::include_image!("../../../assets/icons/disc.svg"))
                                    .fit_to_exact_size(egui::vec2(12.0, 12.0)),
                            ),
                        );
                        let render_btn = render_btn.on_hover_text("Render");
                        if render_btn.clicked() {
                            do_render = true;
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
                    if self.render_job.is_some() {
                        ui.label("Rendering in background...");
                    }
                });

            if do_render {
                let folder = if let Some(folder) = self.render_target_dir.clone() {
                    folder
                } else if let Some(default_dir) = self.default_render_dir() {
                    default_dir
                } else {
                    PathBuf::from(".")
                };
                if let Err(err) = self.render_with_options(&folder) {
                    self.status = format!("Render failed: {err}");
                }
            }
            if close_requested {
                self.render_progress = None;
                open = false;
            }
            self.show_render_dialog = open;
        }
        let frame_ms = frame_start.elapsed().as_secs_f32() * 1000.0;
        self.ui_frame_last_ms = frame_ms;
        if frame_ms > self.ui_frame_max_ms {
            self.ui_frame_max_ms = frame_ms;
        }
    }
}
