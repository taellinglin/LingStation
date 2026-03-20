impl DawApp {
    pub(crate) fn project_info_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("info_panel")
            .default_width(260.0)
            .resizable(true)
            .show(ctx, |ui| {
            ui.heading("Project Info");
            ui.separator();
            ui.label(format!("Name: {}", self.project_name));
            ui.label("Sample Rate: 48 kHz");
            ui.label(format!("Tracks: {}", self.tracks.len()));
            ui.separator();
            ui.heading("Track List");
            let mut selected_index: Option<usize> = None;
            for (index, track) in self.tracks.iter().enumerate() {
                let selected = self.selected_track == Some(index);
                if ui
                    .selectable_label(selected, format!("{}  {}", index + 1, track.name))
                    .clicked()
                {
                    selected_index = Some(index);
                }
            }

            if let Some(index) = selected_index {
                self.selected_track = Some(index);
                self.refresh_params_for_selected_track(true);
            }
        });
    }

    pub(crate) fn mixer_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("mixer_panel")
            .default_width(200.0)
            .min_width(200.0)
            .max_width(350.0)
            .resizable(true)
            .show(ctx, |ui| {
            let mut style = ui.style().as_ref().clone();
            style.text_styles.insert(
                egui::TextStyle::Heading,
                egui::FontId::proportional(BASE_UI_FONT_SIZE),
            );
            style.text_styles.insert(
                egui::TextStyle::Body,
                egui::FontId::proportional(BASE_UI_FONT_SIZE),
            );
            style.text_styles.insert(
                egui::TextStyle::Button,
                egui::FontId::proportional(BASE_UI_FONT_SIZE),
            );
            ui.set_style(style);

            ui.heading("Mixer");
            let show_hitboxes = self.show_hitboxes;
            ui.spacing_mut().item_spacing = egui::vec2(3.0, 3.0);
            let button_h = 16.0;
            let row_spacing = ui.spacing().item_spacing.x;
            let (top_row_rect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), button_h),
                egui::Sense::hover(),
            );
            let button_w = ((top_row_rect.width() - row_spacing * 4.0) / 5.0).max(40.0);
            let top_colors = [
                egui::Color32::from_rgb(235, 64, 52),
                egui::Color32::from_rgb(255, 140, 40),
                egui::Color32::from_rgb(245, 205, 70),
                egui::Color32::from_rgb(80, 200, 120),
                egui::Color32::from_rgb(60, 120, 220),
                egui::Color32::from_rgb(120, 80, 210),
                egui::Color32::from_rgb(200, 90, 180),
            ];
            let mut x = top_row_rect.left();
            if show_hitboxes {
                ui.painter().rect_stroke(
                    top_row_rect,
                    0.0,
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(80, 140, 255)),
                );
            }
            let top_color = top_colors[0];
            let top_fill = egui::Color32::from_rgba_premultiplied(
                top_color.r(),
                top_color.g(),
                top_color.b(),
                80,
            );
            if ui
                .put(
                    egui::Rect::from_min_size(
                        egui::pos2(x, top_row_rect.top()),
                        egui::vec2(button_w, button_h),
                    ),
                    egui::Button::image(
                        egui::Image::new(egui::include_image!("../../../assets/icons/plus.svg"))
                            .fit_to_exact_size(egui::vec2(12.0, 12.0))
                            .tint(top_color),
                    )
                    .fill(top_fill),
                )
                .on_hover_text("Add")
                .clicked()
            {
                self.add_track();
            }
            if show_hitboxes {
                let rect = egui::Rect::from_min_size(
                    egui::pos2(x, top_row_rect.top()),
                    egui::vec2(button_w, button_h),
                );
                ui.painter().rect_stroke(
                    rect,
                    0.0,
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 120, 120)),
                );
            }
            x += button_w + row_spacing;
            let top_color = top_colors[1];
            let top_fill = egui::Color32::from_rgba_premultiplied(
                top_color.r(),
                top_color.g(),
                top_color.b(),
                80,
            );
            if ui
                .put(
                    egui::Rect::from_min_size(
                        egui::pos2(x, top_row_rect.top()),
                        egui::vec2(button_w, button_h),
                    ),
                    egui::Button::image(
                        egui::Image::new(egui::include_image!("../../../assets/icons/copy.svg"))
                            .fit_to_exact_size(egui::vec2(12.0, 12.0))
                            .tint(top_color),
                    )
                    .fill(top_fill),
                )
                .on_hover_text("Copy")
                .clicked()
            {
                self.duplicate_selected_track();
            }
            if show_hitboxes {
                let rect = egui::Rect::from_min_size(
                    egui::pos2(x, top_row_rect.top()),
                    egui::vec2(button_w, button_h),
                );
                ui.painter().rect_stroke(
                    rect,
                    0.0,
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 120, 120)),
                );
            }
            x += button_w + row_spacing;
            let top_color = top_colors[2];
            let top_fill = egui::Color32::from_rgba_premultiplied(
                top_color.r(),
                top_color.g(),
                top_color.b(),
                80,
            );
            if ui
                .put(
                    egui::Rect::from_min_size(
                        egui::pos2(x, top_row_rect.top()),
                        egui::vec2(button_w, button_h),
                    ),
                    egui::Button::image(
                        egui::Image::new(egui::include_image!("../../../assets/icons/layers.svg"))
                            .fit_to_exact_size(egui::vec2(12.0, 12.0))
                            .tint(top_color),
                    )
                    .fill(top_fill),
                )
                .on_hover_text("Clone")
                .clicked()
            {
                self.clone_selected_track();
            }
            if show_hitboxes {
                let rect = egui::Rect::from_min_size(
                    egui::pos2(x, top_row_rect.top()),
                    egui::vec2(button_w, button_h),
                );
                ui.painter().rect_stroke(
                    rect,
                    0.0,
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 120, 120)),
                );
            }
            x += button_w + row_spacing;
            let top_color = top_colors[3];
            let top_fill = egui::Color32::from_rgba_premultiplied(
                top_color.r(),
                top_color.g(),
                top_color.b(),
                80,
            );
            if ui
                .put(
                    egui::Rect::from_min_size(
                        egui::pos2(x, top_row_rect.top()),
                        egui::vec2(button_w, button_h),
                    ),
                    egui::Button::image(
                        egui::Image::new(egui::include_image!("../../../assets/icons/edit-3.svg"))
                            .fit_to_exact_size(egui::vec2(12.0, 12.0))
                            .tint(top_color),
                    )
                    .fill(top_fill),
                )
                .on_hover_text("Rename")
                .clicked()
            {
                self.begin_rename_selected_track();
            }
            if show_hitboxes {
                let rect = egui::Rect::from_min_size(
                    egui::pos2(x, top_row_rect.top()),
                    egui::vec2(button_w, button_h),
                );
                ui.painter().rect_stroke(
                    rect,
                    0.0,
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 120, 120)),
                );
            }
            x += button_w + row_spacing;
            let top_color = top_colors[4];
            let top_fill = egui::Color32::from_rgba_premultiplied(
                top_color.r(),
                top_color.g(),
                top_color.b(),
                80,
            );
            if ui
                .put(
                    egui::Rect::from_min_size(
                        egui::pos2(x, top_row_rect.top()),
                        egui::vec2(button_w, button_h),
                    ),
                    egui::Button::image(
                        egui::Image::new(egui::include_image!("../../../assets/icons/trash-2.svg"))
                            .fit_to_exact_size(egui::vec2(12.0, 12.0))
                            .tint(top_color),
                    )
                    .fill(top_fill),
                )
                .on_hover_text("Remove")
                .clicked()
            {
                self.remove_selected_track();
            }
            if show_hitboxes {
                let rect = egui::Rect::from_min_size(
                    egui::pos2(x, top_row_rect.top()),
                    egui::vec2(button_w, button_h),
                );
                ui.painter().rect_stroke(
                    rect,
                    0.0,
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 120, 120)),
                );
            }
            ui.separator();
            #[derive(Clone, Copy)]
            #[allow(dead_code)]
            enum MixerAction {
                Select(usize),
                PickInstrument(usize),
                ClearInstrument(usize),
                AddFx(usize),
                RemoveFx(usize, usize),
                MoveFx(usize, usize, i32),
            }

            let mut action: Option<MixerAction> = None;
            let mut selected_track = self.selected_track;
            let mut mix_dirty = false;
            let mut pending_exclusive_solo: Option<usize> = None;

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                ui.set_width(ui.available_width());
                for index in 0..self.tracks.len() {
                    let selected = selected_track == Some(index);
                    let track_color = self.track_color(index);
                    let track = &mut self.tracks[index];
                    let group_response = ui.push_id(index, |ui| {
                        let strip_fill = if selected {
                            Self::tint(track_color, 0.24)
                        } else {
                            egui::Color32::from_rgba_premultiplied(
                                track_color.r(),
                                track_color.g(),
                                track_color.b(),
                                58,
                            )
                        };
                        let strip_response = egui::Frame::none()
                            .fill(strip_fill)
                            .rounding(egui::Rounding::same(8.0))
                            .stroke(egui::Stroke::new(
                                1.0,
                                if selected {
                                    Self::tint(track_color, 0.78)
                                } else {
                                    egui::Color32::from_rgba_premultiplied(
                                        track_color.r(),
                                        track_color.g(),
                                        track_color.b(),
                                        95,
                                    )
                                },
                            ))
                            .inner_margin(egui::Margin::symmetric(8.0, 5.0))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.visuals_mut().override_text_color =
                                    Some(egui::Color32::from_gray(240));
                                let label = if selected {
                                    format!("> {}", track.name)
                                } else {
                                    track.name.clone()
                                };
                                let label_fill = if selected {
                                    Self::tint(track_color, 0.34)
                                } else {
                                    egui::Color32::from_rgba_premultiplied(
                                        track_color.r(),
                                        track_color.g(),
                                        track_color.b(),
                                        108,
                                    )
                                };
                                let (label_rect, _) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), 18.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().rect_filled(label_rect, 6.0, label_fill);
                                Self::outlined_text(
                                    ui.painter(),
                                    egui::pos2(label_rect.left() + 6.0, label_rect.center().y),
                                    egui::Align2::LEFT_CENTER,
                                    &label,
                                    egui::FontId::proportional(BASE_UI_FONT_SIZE),
                                    egui::Color32::from_gray(240),
                                );
                                let swatch_rect = egui::Rect::from_min_max(
                                    egui::pos2(label_rect.left(), label_rect.top()),
                                    egui::pos2(
                                        label_rect.left() + 4.0,
                                        label_rect.bottom(),
                                    ),
                                );
                                ui.painter().rect_filled(swatch_rect, 2.0, track_color);
                                if show_hitboxes {
                                    ui.painter().rect_stroke(
                                        label_rect,
                                        0.0,
                                        egui::Stroke::new(1.0, egui::Color32::from_rgb(80, 200, 140)),
                                    );
                                }
                                let (ms_row_rect, _) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), 14.0),
                                    egui::Sense::hover(),
                                );
                                let mute_rect = egui::Rect::from_min_size(
                                    egui::pos2(ms_row_rect.left(), ms_row_rect.top()),
                                    egui::vec2(24.0, 18.0),
                                );
                                let solo_rect = egui::Rect::from_min_size(
                                    egui::pos2(mute_rect.right() + row_spacing, ms_row_rect.top()),
                                    egui::vec2(24.0, 18.0),
                                );
                                let mute_id = egui::Id::new(format!("mixer_mute_{}", index));
                                let solo_id = egui::Id::new(format!("mixer_solo_{}", index));
                                let mute_resp = ui.interact(mute_rect, mute_id, egui::Sense::click());
                                let solo_resp = ui.interact(solo_rect, solo_id, egui::Sense::click());
                                let mute_bg = if track.muted {
                                    Self::tint(track_color, 0.6)
                                } else {
                                    egui::Color32::from_rgba_premultiplied(
                                        track_color.r(),
                                        track_color.g(),
                                        track_color.b(),
                                        50,
                                    )
                                };
                                let solo_bg = if track.solo {
                                    Self::tint(track_color, 0.85)
                                } else {
                                    egui::Color32::from_rgba_premultiplied(
                                        track_color.r(),
                                        track_color.g(),
                                        track_color.b(),
                                        70,
                                    )
                                };
                                ui.painter().rect_filled(mute_rect, 3.0, mute_bg);
                                ui.painter().rect_filled(solo_rect, 3.0, solo_bg);
                                Self::outlined_text(
                                    ui.painter(),
                                    mute_rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    "M",
                                    egui::FontId::proportional(BASE_UI_FONT_SIZE),
                                    egui::Color32::from_gray(220),
                                );
                                Self::outlined_text(
                                    ui.painter(),
                                    solo_rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    "S",
                                    egui::FontId::proportional(BASE_UI_FONT_SIZE),
                                    egui::Color32::from_gray(220),
                                );
                                let mute_clicked = mute_resp.clicked();
                                let solo_clicked = solo_resp.clicked();
                                if show_hitboxes {
                                    ui.painter().rect_stroke(
                                        ms_row_rect,
                                        0.0,
                                        egui::Stroke::new(1.0, egui::Color32::from_rgb(160, 120, 255)),
                                    );
                                }
                                if mute_clicked {
                                    track.muted = !track.muted;
                                    mix_dirty = true;
                                }
                                if solo_clicked {
                                    let multi_solo = ui.input(|i| i.modifiers.shift || i.modifiers.ctrl);
                                    if track.solo {
                                        track.solo = false;
                                    } else if multi_solo {
                                        track.solo = true;
                                    } else {
                                        track.solo = true;
                                        pending_exclusive_solo = Some(index);
                                    }
                                    mix_dirty = true;
                                }
                                let level_response = ui.add_sized(
                                    [ui.available_width(), 12.0],
                                    egui::Slider::new(&mut track.level, 0.0..=1.0).text("Level"),
                                );
                                if level_response.changed() || level_response.dragged() {
                                    mix_dirty = true;
                                }
                                let meter_height = 12.0;
                                let (meter_rect, _) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), meter_height),
                                    egui::Sense::hover(),
                                );
                                ui.painter().rect_filled(
                                    meter_rect,
                                    3.0,
                                    egui::Color32::from_rgb(16, 20, 24),
                                );
                                let (peak_l, peak_r) = self
                                    .track_audio
                                    .get(index)
                                    .map(|s| {
                                        (
                                            f32::from_bits(s.peak_l_bits.load(Ordering::Relaxed)),
                                            f32::from_bits(s.peak_r_bits.load(Ordering::Relaxed)),
                                        )
                                    })
                                    .unwrap_or((0.0, 0.0));
                                let peak_l = peak_l.clamp(0.0, 1.0);
                                let peak_r = peak_r.clamp(0.0, 1.0);
                                let bar_h = (meter_rect.height() - 2.0) * 0.5;
                                let left_rect = egui::Rect::from_min_size(
                                    meter_rect.min + egui::vec2(0.0, 1.0),
                                    egui::vec2(meter_rect.width(), bar_h),
                                );
                                let right_rect = egui::Rect::from_min_size(
                                    egui::pos2(meter_rect.left(), meter_rect.top() + 1.0 + bar_h),
                                    egui::vec2(meter_rect.width(), bar_h),
                                );
                                let fill_l = left_rect.width() * peak_l;
                                let fill_r = right_rect.width() * peak_r;
                                if fill_l > 0.0 {
                                    let color = if peak_l > 0.9 {
                                        egui::Color32::from_rgb(255, 90, 64)
                                    } else if peak_l > 0.7 {
                                        egui::Color32::from_rgb(250, 200, 80)
                                    } else {
                                        egui::Color32::from_rgb(90, 210, 120)
                                    };
                                    let fill_rect = egui::Rect::from_min_size(
                                        left_rect.min,
                                        egui::vec2(fill_l, left_rect.height()),
                                    );
                                    ui.painter().rect_filled(fill_rect, 2.0, color);
                                }
                                if fill_r > 0.0 {
                                    let color = if peak_r > 0.9 {
                                        egui::Color32::from_rgb(255, 90, 64)
                                    } else if peak_r > 0.7 {
                                        egui::Color32::from_rgb(250, 200, 80)
                                    } else {
                                        egui::Color32::from_rgb(90, 210, 120)
                                    };
                                    let fill_rect = egui::Rect::from_min_size(
                                        right_rect.min,
                                        egui::vec2(fill_r, right_rect.height()),
                                    );
                                    ui.painter().rect_filled(fill_rect, 2.0, color);
                                }
                                ui.separator();
                                ui.label("Effects");
                                let mut bypass_dirty = false;
                                for (fx_index, fx) in track.effect_paths.iter().enumerate() {
                                    ui.horizontal(|ui| {
                                        ui.label(format!("{}:", fx_index + 1));
                                        ui.label(Self::plugin_display_name(fx));
                                        if let Some(bypass) = track.effect_bypass.get_mut(fx_index) {
                                            if ui.checkbox(bypass, "Byp").changed() {
                                                bypass_dirty = true;
                                            }
                                        }
                                        if ui
                                            .add(
                                                egui::Button::image(
                                                    egui::Image::new(egui::include_image!(
                                                        "../../../assets/icons/chevron-up.svg"
                                                    ))
                                                    .fit_to_exact_size(egui::vec2(12.0, 12.0))
                                                    .tint(track_color),
                                                ),
                                            )
                                            .on_hover_text("Up")
                                            .clicked()
                                        {
                                            selected_track = Some(index);
                                            action = Some(MixerAction::MoveFx(index, fx_index, -1));
                                        }
                                        if ui
                                            .add(
                                                egui::Button::image(
                                                    egui::Image::new(egui::include_image!(
                                                        "../../../assets/icons/chevron-down.svg"
                                                    ))
                                                    .fit_to_exact_size(egui::vec2(12.0, 12.0))
                                                    .tint(track_color),
                                                ),
                                            )
                                            .on_hover_text("Down")
                                            .clicked()
                                        {
                                            selected_track = Some(index);
                                            action = Some(MixerAction::MoveFx(index, fx_index, 1));
                                        }
                                        if ui
                                            .add(
                                                egui::Button::image(
                                                    egui::Image::new(egui::include_image!(
                                                        "../../../assets/icons/eye.svg"
                                                    ))
                                                    .fit_to_exact_size(egui::vec2(12.0, 12.0))
                                                    .tint(track_color),
                                                ),
                                            )
                                            .on_hover_text("View")
                                            .clicked()
                                        {
                                            selected_track = Some(index);
                                            self.plugin_ui_target =
                                                Some(PluginUiTarget::Effect(index, fx_index));
                                            self.show_plugin_ui = true;
                                        }
                                        if ui
                                            .add(
                                                egui::Button::image(
                                                    egui::Image::new(egui::include_image!(
                                                        "../../../assets/icons/trash-2.svg"
                                                    ))
                                                    .fit_to_exact_size(egui::vec2(12.0, 12.0))
                                                    .tint(track_color),
                                                ),
                                            )
                                            .on_hover_text("Remove")
                                            .clicked()
                                        {
                                            selected_track = Some(index);
                                            action = Some(MixerAction::RemoveFx(index, fx_index));
                                        }
                                    });
                                }
                                if bypass_dirty {
                                    if let Some(state) = self.track_audio.get(index) {
                                        state.sync_effect_bypass(track);
                                    }
                                }
                                let mut add_rect = None;
                                ui.horizontal(|ui| {
                                    ui.set_height(button_h);
                                    let add = ui
                                        .add(egui::Button::image(
                                            egui::Image::new(egui::include_image!(
                                                "../../../assets/icons/plus.svg"
                                            ))
                                            .fit_to_exact_size(egui::vec2(12.0, 12.0))
                                            .tint(track_color),
                                        ))
                                        .on_hover_text("Add FX");
                                    add_rect = Some(add.rect);
                                    if add.clicked() {
                                        selected_track = Some(index);
                                        action = Some(MixerAction::AddFx(index));
                                    }
                                });
                                if show_hitboxes {
                                    if let Some(rect) = add_rect {
                                        ui.painter().rect_stroke(
                                            rect,
                                            0.0,
                                            egui::Stroke::new(1.0, egui::Color32::from_rgb(80, 200, 255)),
                                        );
                                    }
                                }
                            })
                            .response;
                        if strip_response.hovered()
                            && ui.input(|i| i.pointer.primary_clicked())
                        {
                            selected_track = Some(index);
                            action = Some(MixerAction::Select(index));
                        }
                    });
                    if show_hitboxes {
                        ui.painter().rect_stroke(
                            group_response.response.rect,
                            0.0,
                            egui::Stroke::new(1.0, egui::Color32::from_rgb(80, 160, 255)),
                        );
                    }
                }

                ui.separator();
                let master_color = egui::Color32::from_rgb(200, 210, 230);
                let master_fill = egui::Color32::from_rgba_premultiplied(40, 50, 70, 80);
                egui::Frame::none()
                    .fill(master_fill)
                    .rounding(egui::Rounding::same(0.0))
                    .inner_margin(egui::Margin::symmetric(6.0, 0.0))
                    .show(ui, |ui| {
                        let (label_rect, _) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), 18.0),
                            egui::Sense::hover(),
                        );
                        ui.painter().rect_filled(label_rect, 0.0, master_fill);
                        Self::outlined_text(
                            ui.painter(),
                            egui::pos2(label_rect.left() + 6.0, label_rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            "Master",
                            egui::FontId::proportional(BASE_UI_FONT_SIZE),
                            master_color,
                        );

                        if let Ok(mut master) = self.master_settings.lock() {
                            let level_response = ui.add_sized(
                                [ui.available_width(), 12.0],
                                egui::Slider::new(&mut master.level, 0.0..=1.5).text("Level"),
                            );
                            if level_response.changed() || level_response.dragged() {
                                mix_dirty = true;
                            }
                            let meter_height = 12.0;
                            let (meter_rect, _) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), meter_height),
                                egui::Sense::hover(),
                            );
                            ui.painter().rect_filled(
                                meter_rect,
                                3.0,
                                egui::Color32::from_rgb(16, 20, 24),
                            );
                            let peak = self.master_peak_display.clamp(0.0, 1.0);
                            let fill_w = meter_rect.width() * peak;
                            if fill_w > 0.0 {
                                let color = if peak > 0.9 {
                                    egui::Color32::from_rgb(255, 90, 64)
                                } else if peak > 0.7 {
                                    egui::Color32::from_rgb(250, 200, 80)
                                } else {
                                    egui::Color32::from_rgb(90, 210, 120)
                                };
                                let fill_rect = egui::Rect::from_min_size(
                                    meter_rect.min,
                                    egui::vec2(fill_w, meter_rect.height()),
                                );
                                ui.painter().rect_filled(fill_rect, 2.0, color);
                            }
                            ui.separator();
                            ui.checkbox(&mut master.enabled, "Compressor");
                            ui.horizontal(|ui| {
                                ui.label("Thresh (dB)");
                                ui.add(
                                    egui::DragValue::new(&mut master.threshold_db)
                                        .speed(0.5)
                                        .clamp_range(-60.0..=0.0),
                                );
                            });
                            ui.horizontal(|ui| {
                                ui.label("Ratio");
                                ui.add(
                                    egui::DragValue::new(&mut master.ratio)
                                        .speed(0.1)
                                        .clamp_range(1.0..=20.0),
                                );
                            });
                            ui.horizontal(|ui| {
                                ui.label("Attack (ms)");
                                ui.add(
                                    egui::DragValue::new(&mut master.attack_ms)
                                        .speed(0.5)
                                        .clamp_range(1.0..=200.0),
                                );
                            });
                            ui.horizontal(|ui| {
                                ui.label("Release (ms)");
                                ui.add(
                                    egui::DragValue::new(&mut master.release_ms)
                                        .speed(1.0)
                                        .clamp_range(10.0..=1000.0),
                                );
                            });
                            ui.horizontal(|ui| {
                                ui.label("Makeup (dB)");
                                ui.add(
                                    egui::DragValue::new(&mut master.makeup_db)
                                        .speed(0.5)
                                        .clamp_range(-12.0..=12.0),
                                );
                            });
                        }
                    });
            });

            if let Some(action) = action {
                match action {
                    MixerAction::Select(index) => {
                        self.selected_track = Some(index);
                        self.refresh_params_for_selected_track(true);
                    }
                    MixerAction::PickInstrument(index) => {
                        self.open_plugin_picker(PluginTarget::Instrument(index));
                    }
                    MixerAction::ClearInstrument(index) => {
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
                    MixerAction::AddFx(index) => {
                        self.open_plugin_picker(PluginTarget::Effect(index));
                    }
                    MixerAction::RemoveFx(index, fx_index) => {
                        if self.plugin_ui_matches(PluginUiTarget::Effect(index, fx_index)) {
                            self.show_plugin_ui = false;
                            self.destroy_plugin_ui();
                        }
                        if let Some(track) = self.tracks.get_mut(index) {
                            if fx_index < track.effect_paths.len() {
                                track.effect_paths.remove(fx_index);
                            }
                            if fx_index < track.effect_clap_ids.len() {
                                track.effect_clap_ids.remove(fx_index);
                            }
                            if fx_index < track.effect_bypass.len() {
                                track.effect_bypass.remove(fx_index);
                            }
                            if fx_index < track.effect_params.len() {
                                track.effect_params.remove(fx_index);
                            }
                            if fx_index < track.effect_param_ids.len() {
                                track.effect_param_ids.remove(fx_index);
                            }
                            if fx_index < track.effect_param_values.len() {
                                track.effect_param_values.remove(fx_index);
                            }
                        }
                        if let Some(state) = self.track_audio.get_mut(index) {
                            if fx_index < state.effect_hosts.len() {
                                let host = state.effect_hosts.remove(fx_index);
                                host.prepare_for_drop();
                                self.orphaned_hosts.push(host);
                            }
                        }
                        self.reinit_audio_if_running();
                    }
                    MixerAction::MoveFx(index, fx_index, direction) => {
                        let target_index = if direction < 0 {
                            fx_index.saturating_sub(1)
                        } else {
                            fx_index + 1
                        };
                        let mut moved = false;
                        if let Some(track) = self.tracks.get_mut(index) {
                            if target_index < track.effect_paths.len() {
                                track.effect_paths.swap(fx_index, target_index);
                                if target_index < track.effect_bypass.len() {
                                    track.effect_bypass.swap(fx_index, target_index);
                                }
                                if target_index < track.effect_params.len() {
                                    track.effect_params.swap(fx_index, target_index);
                                }
                                if target_index < track.effect_param_ids.len() {
                                    track.effect_param_ids.swap(fx_index, target_index);
                                }
                                if target_index < track.effect_param_values.len() {
                                    track.effect_param_values.swap(fx_index, target_index);
                                }
                                if target_index < track.effect_clap_ids.len() {
                                    track.effect_clap_ids.swap(fx_index, target_index);
                                }
                                moved = true;
                            }
                        }
                        if moved {
                            if let Some(state) = self.track_audio.get_mut(index) {
                                if target_index < state.effect_hosts.len() {
                                    state.effect_hosts.swap(fx_index, target_index);
                                }
                                if let Some(track) = self.tracks.get(index) {
                                    state.sync_effect_bypass(track);
                                }
                            }
                            if let Some(target) = self.plugin_ui_target {
                                if matches!(
                                    target,
                                    PluginUiTarget::Effect(ti, fi)
                                        if ti == index && (fi == fx_index || fi == target_index)
                                ) {
                                    self.show_plugin_ui = false;
                                    self.destroy_plugin_ui();
                                }
                            }
                        }
                    }
                }
            }
            if mix_dirty {
                self.sync_track_mix();
            }
        });
    }
}
