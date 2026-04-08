impl DawApp {
    #[allow(dead_code)]
    pub(crate) fn center_empty(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.centered_and_justified(|ui| {
                ui.label("Arranger hidden");
            });
        });
    }


    pub(crate) fn center_parameters(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            self.render_params_roll_panel(ctx, ui, true, false);
        });
    }

    pub(crate) fn center_piano_roll(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            self.render_params_roll_panel(ctx, ui, false, true);
        });
    }

    pub(crate) fn center_node_editor(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Node Editor");
            ui.label("Routing preview for tracks, effects, and future bus links.");
            ui.horizontal(|ui| {
                ui.label("Map Height");
                ui.add(
                    egui::Slider::new(&mut self.node_map_height, 260.0..=1400.0)
                        .show_value(true),
                );
                if ui.button("Reset View").clicked() {
                    self.node_view_pan = egui::Vec2::ZERO;
                    self.node_view_zoom = 1.0;
                }
            });
            ui.add_space(8.0);

            if self.tracks.is_empty() {
                ui.label("No tracks available.");
                return;
            }

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {

            self.node_route_from_track = self
                .node_route_from_track
                .min(self.tracks.len().saturating_sub(1));
            self.node_route_source_output_pair = self.node_route_source_output_pair.min(7);
            self.node_route_to_track = self
                .node_route_to_track
                .min(self.tracks.len().saturating_sub(1));
            self.sync_node_routes();

            let track_names: Vec<String> = self
                .tracks
                .iter()
                .enumerate()
                .map(|(i, t)| format!("{}: {}", i + 1, t.name))
                .collect();
            let fx_names: Vec<Vec<String>> = self
                .tracks
                .iter()
                .map(|track| {
                    track
                        .effect_paths
                        .iter()
                        .enumerate()
                        .map(|(i, path)| {
                            let label = Path::new(path)
                                .file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or(path);
                            format!("{}: {}", i + 1, label)
                        })
                        .collect()
                })
                .collect();
            let node_activity_snapshot = self.engine.node_activity.lock().clone();

            let max_out_pairs = self
                .engine
                .track_audio
                .iter()
                .filter_map(|state| state.host.as_ref().map(|host| host.io_channels().1.max(1).div_ceil(2)))
                .max()
                .unwrap_or(1)
                .min(8);
            let per_track_h = (110.0 + max_out_pairs as f32 * 10.0).clamp(110.0, 200.0);

            let auto_map_height = ((self.tracks.len().max(4) as f32) * per_track_h).clamp(380.0, 980.0);
            if self.node_map_height < 1.0 {
                self.node_map_height = auto_map_height;
            }
            let map_height = self.node_map_height.clamp(260.0, 1400.0);
            let desired_size = egui::vec2(ui.available_width(), map_height);
            let (map_rect, map_response) = ui.allocate_exact_size(desired_size, egui::Sense::drag());
            let painter = ui.painter_at(map_rect);

            if map_response.hovered() {
                let (scroll_delta, modifiers, key_reset) = ctx.input(|i| {
                    (
                        i.smooth_scroll_delta,
                        i.modifiers,
                        i.key_pressed(egui::Key::Num0),
                    )
                });
                if key_reset {
                    self.node_view_pan = egui::Vec2::ZERO;
                    self.node_view_zoom = 1.0;
                }
                if map_response.dragged_by(egui::PointerButton::Middle) {
                    self.node_view_pan += map_response.drag_delta();
                }
                if modifiers.ctrl {
                    let zoom_delta = (scroll_delta.y + scroll_delta.x) * 0.0015;
                    self.node_view_zoom = (self.node_view_zoom * (1.0 + zoom_delta)).clamp(0.55, 2.2);
                } else if modifiers.shift {
                    self.node_view_pan.x += scroll_delta.y + scroll_delta.x;
                } else {
                    self.node_view_pan.y += scroll_delta.y;
                }
            }

            painter.rect_filled(map_rect, 8.0, egui::Color32::from_rgb(22, 24, 28));
            let grid_color = egui::Color32::from_rgba_premultiplied(56, 62, 72, 80);
            let grid_step = 20.0;
            let mut x = map_rect.left() + self.node_view_pan.x.rem_euclid(grid_step);
            while x <= map_rect.right() {
                painter.line_segment(
                    [egui::pos2(x, map_rect.top()), egui::pos2(x, map_rect.bottom())],
                    egui::Stroke::new(1.0, grid_color),
                );
                x += grid_step;
            }
            let mut y = map_rect.top() + self.node_view_pan.y.rem_euclid(grid_step);
            while y <= map_rect.bottom() {
                painter.line_segment(
                    [egui::pos2(map_rect.left(), y), egui::pos2(map_rect.right(), y)],
                    egui::Stroke::new(1.0, grid_color),
                );
                y += grid_step;
            }

            let center = map_rect.center();
            let zoom = self.node_view_zoom;
            let pan = self.node_view_pan;
            let to_screen = |p: egui::Pos2| {
                egui::pos2(
                    center.x + (p.x - center.x) * zoom + pan.x,
                    center.y + (p.y - center.y) * zoom + pan.y,
                )
            };
            let to_screen_rect = |rect: egui::Rect| {
                egui::Rect::from_min_max(to_screen(rect.min), to_screen(rect.max))
            };

            let left_x = map_rect.left() + 28.0;
            let fx_x = map_rect.left() + map_rect.width() * 0.45;
            let master_x = map_rect.right() - 220.0;
            let row_h = map_rect.height() / (self.tracks.len().max(1) as f32);
            let track_size = egui::vec2(220.0, (50.0 + max_out_pairs as f32 * 14.0).clamp(78.0, 178.0));
            let fx_size = egui::vec2(196.0, 66.0);
            let master_rect = egui::Rect::from_min_size(
                egui::pos2(master_x, map_rect.center().y - 42.0),
                egui::vec2(196.0, 84.0),
            );

            let port_audio = egui::Color32::from_rgb(86, 187, 255);
            let port_midi = egui::Color32::from_rgb(92, 212, 132);
            let port_aux = egui::Color32::from_rgb(235, 131, 255);
            let port_sc = egui::Color32::from_rgb(248, 208, 98);

            let draw_node = |rect: egui::Rect,
                             title: &str,
                             role: &str,
                             tint: egui::Color32,
                             inputs: &[(String, egui::Color32, f32)],
                             outputs: &[(String, egui::Color32, f32)]| {
                let r = to_screen_rect(rect);
                let header_h = (22.0 * zoom).clamp(14.0, 28.0);
                let row_top_pad = (10.0 * zoom).clamp(6.0, 10.0);
                let row_step = (14.0 * zoom).clamp(9.0, 14.0);
                let port_radius = (3.4 * zoom).clamp(2.0, 3.4);
                let port_inset = (9.0 * zoom).clamp(6.0, 9.0);
                let label_inset = (16.0 * zoom).clamp(10.0, 16.0);
                let meter_w = (46.0 * zoom).clamp(24.0, 46.0);
                let meter_h = (4.0 * zoom).clamp(2.0, 4.0);
                let meter_inset = (10.0 * zoom).clamp(6.0, 10.0);
                let show_meters = zoom >= 0.65;
                painter.rect_filled(r, 8.0, egui::Color32::from_rgba_premultiplied(19, 22, 28, 240));
                painter.rect_stroke(r, 8.0, egui::Stroke::new(1.0, tint.gamma_multiply(0.9)));
                let header = egui::Rect::from_min_max(r.min, egui::pos2(r.right(), r.top() + header_h));
                painter.rect_filled(header, 8.0, tint.gamma_multiply(0.45));
                painter.text(
                    egui::pos2(header.left() + 8.0, header.center().y),
                    egui::Align2::LEFT_CENTER,
                    title,
                    egui::TextStyle::Body.resolve(ui.style()),
                    egui::Color32::WHITE,
                );
                painter.text(
                    egui::pos2(header.right() - 8.0, header.center().y),
                    egui::Align2::RIGHT_CENTER,
                    role,
                    egui::TextStyle::Small.resolve(ui.style()),
                    egui::Color32::from_gray(230),
                );

                let mut y_in = header.bottom() + row_top_pad;
                for (name, color, level) in inputs {
                    painter.circle_filled(egui::pos2(r.left() + port_inset, y_in), port_radius, *color);
                    painter.text(
                        egui::pos2(r.left() + label_inset, y_in),
                        egui::Align2::LEFT_CENTER,
                        name,
                        egui::TextStyle::Small.resolve(ui.style()),
                        egui::Color32::from_gray(220),
                    );
                    if show_meters {
                        let meter_x = r.right() - meter_w - meter_inset;
                        let meter_rect = egui::Rect::from_min_size(
                            egui::pos2(meter_x, y_in - meter_h * 0.5),
                            egui::vec2(meter_w, meter_h),
                        );
                        painter.rect_filled(meter_rect, 2.0, egui::Color32::from_gray(46));
                        let fill = meter_rect.width() * level.clamp(0.0, 1.0);
                        if fill > 0.0 {
                            painter.rect_filled(
                                egui::Rect::from_min_size(meter_rect.min, egui::vec2(fill, meter_h)),
                                2.0,
                                *color,
                            );
                        }
                    }
                    y_in += row_step;
                }
                let mut y_out = header.bottom() + row_top_pad;
                for (name, color, level) in outputs {
                    painter.circle_filled(egui::pos2(r.right() - port_inset, y_out), port_radius, *color);
                    painter.text(
                        egui::pos2(r.right() - label_inset, y_out),
                        egui::Align2::RIGHT_CENTER,
                        name,
                        egui::TextStyle::Small.resolve(ui.style()),
                        egui::Color32::from_gray(220),
                    );
                    if show_meters {
                        let meter_x = r.left() + meter_inset;
                        let meter_rect = egui::Rect::from_min_size(
                            egui::pos2(meter_x, y_out - meter_h * 0.5),
                            egui::vec2(meter_w, meter_h),
                        );
                        painter.rect_filled(meter_rect, 2.0, egui::Color32::from_gray(46));
                        let fill = meter_rect.width() * level.clamp(0.0, 1.0);
                        if fill > 0.0 {
                            painter.rect_filled(
                                egui::Rect::from_min_size(meter_rect.min, egui::vec2(fill, meter_h)),
                                2.0,
                                *color,
                            );
                        }
                    }
                    y_out += row_step;
                }
                r
            };

            let mut track_rects = Vec::with_capacity(self.tracks.len());
            let mut track_out_ports: Vec<Vec<egui::Pos2>> = Vec::with_capacity(self.tracks.len());
            let mut fx_rects: Vec<Vec<egui::Rect>> = Vec::with_capacity(self.tracks.len());
            for (track_index, track_name) in track_names.iter().enumerate() {
                let center_y = map_rect.top() + row_h * (track_index as f32 + 0.5);
                let track_rect_world = egui::Rect::from_min_size(
                    egui::pos2(left_x, center_y - track_size.y * 0.5),
                    track_size,
                );

                let inst_kind = self
                    .tracks
                    .get(track_index)
                    .and_then(|t| t.instrument_path.as_deref())
                    .map(|path| {
                        if path.to_ascii_lowercase().ends_with(".vst3") {
                            "VST3"
                        } else if path.to_ascii_lowercase().ends_with(".clap") {
                            "CLAP"
                        } else if Self::is_treesynth_path(path) {
                            "TreeSynth"
                        } else if Self::is_drummachine_path(path) {
                            "Drum Machine"
                        } else {
                            "Inst"
                        }
                    })
                    .unwrap_or("Track");
                let out_channels = self
                    .engine
                    .track_audio
                    .get(track_index)
                    .and_then(|state| state.host.as_ref())
                    .map(|host| host.io_channels().1.max(1))
                    .unwrap_or(2)
                    .min(MAX_PLUGIN_OUTPUT_CHANNELS);
                let out_pairs = out_channels.div_ceil(2);
                let track_role = format!("{} • {} out", inst_kind, out_channels);
                let track_activity = node_activity_snapshot
                    .get(track_index)
                    .cloned()
                    .unwrap_or_default();

                let mut track_inputs: Vec<(String, egui::Color32, f32)> = vec![
                    ("MIDI In".to_string(), port_midi, track_activity.midi_in),
                    (
                        "Audio In".to_string(),
                        port_audio,
                        track_activity.output_pair_peaks[0],
                    ),
                ];
                if out_pairs > 1 {
                    track_inputs.push(("Aux/SC In".to_string(), port_sc, track_activity.midi_out));
                }

                let mut track_outputs: Vec<(String, egui::Color32, f32)> = Vec::new();
                for pair in 0..out_pairs.min(8) {
                    let left = pair * 2 + 1;
                    let right = (pair * 2 + 2).min(out_channels);
                    track_outputs.push((
                        format!("Out {} ({}/{})", pair, left, right),
                        port_audio,
                        track_activity.output_pair_peaks[pair.min(7)],
                    ));
                }
                if out_pairs > 8 {
                    track_outputs.push((format!("+{} more", out_pairs - 8), port_aux, 0.0));
                }

                let track_rect = draw_node(
                    track_rect_world,
                    track_name,
                    &track_role,
                    egui::Color32::from_rgb(79, 121, 199),
                    &track_inputs,
                    &track_outputs,
                );
                let header_h = (22.0 * zoom).clamp(14.0, 28.0);
                let row_top_pad = (10.0 * zoom).clamp(6.0, 10.0);
                let row_step = (14.0 * zoom).clamp(9.0, 14.0);
                let port_inset = (9.0 * zoom).clamp(6.0, 9.0);
                let mut out_ports = Vec::with_capacity(track_outputs.len());
                for pair in 0..track_outputs.len() {
                    let y = track_rect.top() + header_h + row_top_pad + pair as f32 * row_step;
                    let center = egui::pos2(track_rect.right() - port_inset, y);
                    out_ports.push(center);
                    let hot_rect = egui::Rect::from_center_size(center, egui::vec2(18.0, 12.0));
                    let response = ui.interact(
                        hot_rect,
                        ui.id().with(("node_out_port", track_index, pair)),
                        egui::Sense::click(),
                    );
                    if response.hovered() {
                        painter.circle_stroke(
                            center,
                            6.5,
                            egui::Stroke::new(1.0, port_audio.gamma_multiply(0.85)),
                        );
                    }
                    if self.node_route_from_track == track_index
                        && self.node_route_source_output_pair == pair
                    {
                        painter.circle_stroke(
                            center,
                            7.2,
                            egui::Stroke::new(1.8, egui::Color32::WHITE),
                        );
                    }
                    if response.clicked() {
                        self.node_route_from_track = track_index;
                        self.node_route_source_output_pair = pair;
                        self.status = format!(
                            "Route source set to {} (Out {})",
                            track_name,
                            pair
                        );
                    }
                }
                track_rects.push(track_rect);
                track_out_ports.push(out_ports);

                let track_fx = &fx_names[track_index];
                let mut row_fx = Vec::with_capacity(track_fx.len());
                if !track_fx.is_empty() {
                    let total_h = track_fx.len() as f32 * (fx_size.y + 6.0) - 6.0;
                    let mut start_y = center_y - total_h * 0.5;
                    for (fx_index, fx_name) in track_fx.iter().enumerate() {
                        let rect_world = egui::Rect::from_min_size(egui::pos2(fx_x, start_y), fx_size);
                        let fx_in_level = track_activity
                            .fx_input_peaks
                            .get(fx_index)
                            .copied()
                            .unwrap_or(0.0);
                        let fx_out_level = track_activity
                            .fx_output_peaks
                            .get(fx_index)
                            .copied()
                            .unwrap_or(0.0);
                        let rect = draw_node(
                            rect_world,
                            fx_name,
                            "FX",
                            egui::Color32::from_rgb(181, 128, 58),
                            &[
                                ("Audio In".to_string(), port_audio, fx_in_level),
                                ("Mod In".to_string(), port_aux, track_activity.midi_in),
                            ],
                            &[("Audio Out".to_string(), port_audio, fx_out_level)],
                        );
                        row_fx.push(rect);
                        start_y += fx_size.y + 6.0;
                    }
                }
                fx_rects.push(row_fx);
            }

            let master_rect = draw_node(
                master_rect,
                "Master",
                "Bus",
                egui::Color32::from_rgb(74, 130, 96),
                &[
                    ("Mix In".to_string(), port_audio, 0.0),
                    ("Sidechain".to_string(), port_sc, 0.0),
                ],
                &[("Main Out".to_string(), port_audio, 0.0)],
            );

            let mut obstacles = track_rects.clone();
            for row in &fx_rects {
                obstacles.extend(row.iter().copied());
            }
            obstacles.push(master_rect);

            let cubic_point = |p0: egui::Pos2,
                              p1: egui::Pos2,
                              p2: egui::Pos2,
                              p3: egui::Pos2,
                              t: f32| {
                let u = 1.0 - t;
                let x = u * u * u * p0.x
                    + 3.0 * u * u * t * p1.x
                    + 3.0 * u * t * t * p2.x
                    + t * t * t * p3.x;
                let y = u * u * u * p0.y
                    + 3.0 * u * u * t * p1.y
                    + 3.0 * u * t * t * p2.y
                    + t * t * t * p3.y;
                egui::pos2(x, y)
            };

            let draw_bezier_wire = |source: egui::Pos2,
                                        target: egui::Pos2,
                                        stroke: egui::Stroke,
                                        lane_seed: usize| {
                let candidates = [
                    0.0,
                    -26.0,
                    26.0,
                    -48.0,
                    48.0,
                    -72.0,
                    72.0,
                    -96.0,
                    96.0,
                ];
                let rotated = lane_seed % candidates.len();
                let ordered: Vec<f32> = (0..candidates.len())
                    .map(|i| candidates[(i + rotated) % candidates.len()])
                    .collect();

                let mut chosen = (source, source, target, target);
                for offset in ordered {
                    let mut dx = (target.x - source.x).abs().max(60.0) * 0.38;
                    if target.x <= source.x {
                        dx = ((source.x - target.x) * 0.25 + 90.0).clamp(90.0, 220.0);
                    }
                    let c1 = egui::pos2(source.x + dx, source.y + offset);
                    let c2 = if target.x > source.x {
                        egui::pos2(target.x - dx, target.y + offset)
                    } else {
                        egui::pos2(target.x - dx * 0.3, target.y + offset)
                    };

                    let mut clear = true;
                    for t in [0.2, 0.35, 0.5, 0.65, 0.8] {
                        let p = cubic_point(source, c1, c2, target, t);
                        for rect in &obstacles {
                            if rect.expand(6.0).contains(p) {
                                clear = false;
                                break;
                            }
                        }
                        if !clear {
                            break;
                        }
                    }
                    chosen = (source, c1, c2, target);
                    if clear {
                        break;
                    }
                }

                let shape = egui::epaint::CubicBezierShape {
                    points: [chosen.0, chosen.1, chosen.2, chosen.3],
                    closed: false,
                    fill: egui::Color32::TRANSPARENT,
                    stroke,
                };
                painter.add(shape);
            };

            for (idx, track_rect) in track_rects.iter().enumerate() {
                let level = node_activity_snapshot
                    .get(idx)
                    .map(|a| a.output_pair_peaks[0])
                    .unwrap_or(0.0)
                    .clamp(0.0, 1.0);
                let base_wire = egui::Stroke::new(
                    1.3 + level * 1.8,
                    egui::Color32::from_rgb(180, 180, 180).gamma_multiply(0.65 + level * 0.7),
                );
                if let Some(first_fx) = fx_rects[idx].first() {
                    let first_level = node_activity_snapshot
                        .get(idx)
                        .and_then(|a| a.fx_input_peaks.first().copied())
                        .unwrap_or(level)
                        .clamp(0.0, 1.0);
                    let first_wire = egui::Stroke::new(
                        1.3 + first_level * 1.8,
                        egui::Color32::from_rgb(180, 180, 180)
                            .gamma_multiply(0.65 + first_level * 0.7),
                    );
                    draw_bezier_wire(track_rect.right_center(), first_fx.left_center(), first_wire, idx);
                    for (chain_idx, chain) in fx_rects[idx].windows(2).enumerate() {
                        let chain_level = node_activity_snapshot
                            .get(idx)
                            .and_then(|a| a.fx_output_peaks.get(chain_idx))
                            .copied()
                            .unwrap_or(level)
                            .clamp(0.0, 1.0);
                        let chain_wire = egui::Stroke::new(
                            1.3 + chain_level * 1.8,
                            egui::Color32::from_rgb(180, 180, 180)
                                .gamma_multiply(0.65 + chain_level * 0.7),
                        );
                        draw_bezier_wire(
                            chain[0].right_center(),
                            chain[1].left_center(),
                            chain_wire,
                            idx + chain_idx + 1,
                        );
                    }
                    if let Some(last_fx) = fx_rects[idx].last() {
                        let out_level = node_activity_snapshot
                            .get(idx)
                            .and_then(|a| a.fx_output_peaks.last().copied())
                            .unwrap_or(level)
                            .clamp(0.0, 1.0);
                        let out_wire = egui::Stroke::new(
                            1.3 + out_level * 1.8,
                            egui::Color32::from_rgb(180, 180, 180)
                                .gamma_multiply(0.65 + out_level * 0.7),
                        );
                        draw_bezier_wire(
                            last_fx.right_center(),
                            master_rect.left_center(),
                            out_wire,
                            idx + 5,
                        );
                    }
                } else {
                    draw_bezier_wire(track_rect.right_center(), master_rect.left_center(), base_wire, idx + 3);
                }
            }

            for (route_index, route) in self.node_routes.iter().filter(|r| r.enabled).enumerate() {
                if route.from_track >= track_rects.len() || route.to_track >= track_rects.len() {
                    continue;
                }
                let source = track_out_ports
                    .get(route.from_track)
                    .and_then(|ports| ports.get(route.source_output_pair))
                    .copied()
                    .unwrap_or(track_rects[route.from_track].right_center());
                let target = if let Some(fx_index) = route.to_fx {
                    fx_rects
                        .get(route.to_track)
                        .and_then(|nodes| nodes.get(fx_index))
                        .map(|rect| rect.left_center())
                        .unwrap_or(track_rects[route.to_track].left_center())
                } else {
                    track_rects[route.to_track].left_center()
                };
                let color = match route.kind {
                    NodeRouteKind::AudioSend => egui::Color32::from_rgb(75, 191, 255),
                    NodeRouteKind::AudioSidechain => egui::Color32::from_rgb(247, 197, 85),
                    NodeRouteKind::MidiToFx => egui::Color32::from_rgb(213, 120, 255),
                };
                let level = node_activity_snapshot
                    .get(route.from_track)
                    .map(|a| match route.kind {
                        NodeRouteKind::MidiToFx => a.midi_out,
                        _ => a.output_pair_peaks[route.source_output_pair.min(7)],
                    })
                    .unwrap_or(0.0)
                    .clamp(0.0, 1.0);
                let stroke = egui::Stroke::new(1.6 + level * 2.1, color.gamma_multiply(0.55 + level * 0.8));
                draw_bezier_wire(source, target, stroke, route_index + 11);
                if route.kind == NodeRouteKind::MidiToFx {
                    let time = ctx.input(|i| i.time) as f32;
                    let phase = (time * 2.4 + route_index as f32 * 0.17).fract();
                    for step in 0..6 {
                        let t = (phase + step as f32 / 6.0).fract();
                        let p = egui::pos2(
                            source.x + (target.x - source.x) * t,
                            source.y + (target.y - source.y) * t,
                        );
                        let alpha = (1.0 - step as f32 / 6.0) * (0.35 + level * 0.65);
                        painter.circle_filled(
                            p,
                            1.8 + level * 1.8,
                            color.gamma_multiply(alpha.clamp(0.0, 1.0)),
                        );
                    }
                }
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.colored_label(egui::Color32::from_rgb(180, 180, 180), "Base routing");
                ui.separator();
                ui.colored_label(port_audio, "Audio");
                ui.colored_label(port_midi, "MIDI");
                ui.colored_label(port_sc, "Sidechain");
                ui.colored_label(port_aux, "Aux/Mod");
                ui.separator();
                ui.label("MMB: pan  Wheel: vertical  Shift+Wheel: horizontal  Ctrl+Wheel: zoom  0: reset/frame");
            });
            let (sep_rect, sep_resp) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), 8.0),
                egui::Sense::click_and_drag(),
            );
            ui.painter().rect_filled(sep_rect, 3.0, egui::Color32::from_rgb(62, 68, 78));
            if sep_resp.dragged() {
                self.node_map_height = (self.node_map_height + sep_resp.drag_delta().y).clamp(260.0, 1400.0);
            }
            ui.add_space(8.0);

            ui.group(|ui| {
                ui.label("Add Route");
                ui.horizontal(|ui| {
                    ui.label("Kind");
                    egui::ComboBox::from_id_source("node_route_kind")
                        .selected_text(match self.node_route_kind {
                            NodeRouteKind::AudioSidechain => "Audio Sidechain",
                            NodeRouteKind::MidiToFx => "MIDI -> FX",
                            NodeRouteKind::AudioSend => "Audio Send",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.node_route_kind,
                                NodeRouteKind::AudioSend,
                                "Audio Send",
                            );
                            ui.selectable_value(
                                &mut self.node_route_kind,
                                NodeRouteKind::AudioSidechain,
                                "Audio Sidechain",
                            );
                            ui.selectable_value(
                                &mut self.node_route_kind,
                                NodeRouteKind::MidiToFx,
                                "MIDI -> FX",
                            );
                        });
                });

                ui.horizontal(|ui| {
                    ui.label("From");
                    egui::ComboBox::from_id_source("node_route_from_track")
                        .selected_text(track_names[self.node_route_from_track].clone())
                        .show_ui(ui, |ui| {
                            for (idx, name) in track_names.iter().enumerate() {
                                ui.selectable_value(&mut self.node_route_from_track, idx, name);
                            }
                        });
                    let from_out_channels = self
                        .engine
                        .track_audio
                        .get(self.node_route_from_track)
                        .and_then(|state| state.host.as_ref())
                        .map(|host| host.io_channels().1.max(1))
                        .unwrap_or(2)
                        .min(MAX_PLUGIN_OUTPUT_CHANNELS);
                    let from_out_pairs = from_out_channels.div_ceil(2).clamp(1, 8);
                    self.node_route_source_output_pair = self
                        .node_route_source_output_pair
                        .min(from_out_pairs.saturating_sub(1));
                    ui.label("Out");
                    egui::ComboBox::from_id_source("node_route_source_pair")
                        .selected_text(format!("{}", self.node_route_source_output_pair))
                        .show_ui(ui, |ui| {
                            for pair in 0..from_out_pairs {
                                let left = pair * 2 + 1;
                                let right = (pair * 2 + 2).min(from_out_channels);
                                ui.selectable_value(
                                    &mut self.node_route_source_output_pair,
                                    pair,
                                    format!("{} ({}/{})", pair, left, right),
                                );
                            }
                        });
                });

                ui.horizontal(|ui| {
                    ui.label("To");
                    egui::ComboBox::from_id_source("node_route_to_track")
                        .selected_text(track_names[self.node_route_to_track].clone())
                        .show_ui(ui, |ui| {
                            for (idx, name) in track_names.iter().enumerate() {
                                ui.selectable_value(&mut self.node_route_to_track, idx, name);
                            }
                        });
                });

                let target_fx_count = fx_names
                    .get(self.node_route_to_track)
                    .map(|v| v.len())
                    .unwrap_or(0);
                if target_fx_count > 0 {
                    self.node_route_to_fx = self
                        .node_route_to_fx
                        .min(target_fx_count.saturating_sub(1));
                } else {
                    self.node_route_to_fx = 0;
                }

                if self.node_route_kind == NodeRouteKind::MidiToFx {
                    ui.horizontal(|ui| {
                        ui.label("FX");
                        if target_fx_count == 0 {
                            ui.label("No FX on destination track");
                        } else {
                            egui::ComboBox::from_id_source("node_route_to_fx")
                                .selected_text(
                                    fx_names[self.node_route_to_track][self.node_route_to_fx].clone(),
                                )
                                .show_ui(ui, |ui| {
                                    for (idx, name) in fx_names[self.node_route_to_track]
                                        .iter()
                                        .enumerate()
                                    {
                                        ui.selectable_value(&mut self.node_route_to_fx, idx, name);
                                    }
                                });
                        }
                    });
                }

                if ui.button("Add route").clicked() {
                    let to_fx = if self.node_route_kind == NodeRouteKind::MidiToFx {
                        if target_fx_count == 0 {
                            self.status =
                                "Add at least one FX to destination track first".to_string();
                            None
                        } else {
                            Some(self.node_route_to_fx)
                        }
                    } else {
                        None
                    };

                    if self.node_route_kind != NodeRouteKind::MidiToFx || to_fx.is_some() {
                        self.node_routes.push(NodeRouteLink {
                            from_track: self.node_route_from_track,
                            source_output_pair: self.node_route_source_output_pair,
                            to_track: self.node_route_to_track,
                            to_fx,
                            kind: self.node_route_kind,
                            enabled: true,
                            sidechain_amount: default_sidechain_amount(),
                            sidechain_attack_ms: default_sidechain_attack_ms(),
                            sidechain_release_ms: default_sidechain_release_ms(),
                            sidechain_threshold_db: default_sidechain_threshold_db(),
                        });
                        self.sync_node_routes();
                        self.mark_dirty();
                    }
                }
            });

            ui.add_space(8.0);
            ui.separator();
            ui.label(format!("Routes: {}", self.node_routes.len()));

            let mut remove_index = None;
            let mut route_toggled = false;
            let mut route_params_changed = false;
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (idx, route) in self.node_routes.iter_mut().enumerate() {
                    ui.horizontal_wrapped(|ui| {
                        route_toggled |= ui.checkbox(&mut route.enabled, "").changed();

                        let from_name = track_names
                            .get(route.from_track)
                            .cloned()
                            .unwrap_or_else(|| format!("Track {}", route.from_track + 1));
                        let to_name = track_names
                            .get(route.to_track)
                            .cloned()
                            .unwrap_or_else(|| format!("Track {}", route.to_track + 1));
                        let kind_label = match route.kind {
                            NodeRouteKind::AudioSidechain => "Audio Sidechain",
                            NodeRouteKind::MidiToFx => "MIDI -> FX",
                            NodeRouteKind::AudioSend => "Audio Send",
                        };

                        let target_label = if let Some(fx_index) = route.to_fx {
                            let fx_name = fx_names
                                .get(route.to_track)
                                .and_then(|names| names.get(fx_index))
                                .cloned()
                                .unwrap_or_else(|| format!("FX {}", fx_index + 1));
                            format!("{} [{}]", to_name, fx_name)
                        } else {
                            to_name
                        };

                        ui.label(format!("{} -> {} ({})", from_name, target_label, kind_label));

                        if route.kind == NodeRouteKind::AudioSidechain {
                            route_params_changed |= ui
                                .add(
                                    egui::DragValue::new(&mut route.source_output_pair)
                                        .clamp_range(0..=7)
                                        .prefix("Out "),
                                )
                                .changed();
                            route_params_changed |= ui
                                .add(
                                    egui::Slider::new(&mut route.sidechain_amount, 0.0..=1.0)
                                        .text("Amt"),
                                )
                                .changed();
                            route_params_changed |= ui
                                .add(
                                    egui::Slider::new(&mut route.sidechain_attack_ms, 1.0..=80.0)
                                        .text("Atk"),
                                )
                                .changed();
                            route_params_changed |= ui
                                .add(
                                    egui::Slider::new(&mut route.sidechain_release_ms, 40.0..=600.0)
                                        .text("Rel"),
                                )
                                .changed();
                            route_params_changed |= ui
                                .add(
                                    egui::Slider::new(&mut route.sidechain_threshold_db, -60.0..=0.0)
                                        .text("Thr"),
                                )
                                .changed();
                        }

                        if ui.button("Remove").clicked() {
                            remove_index = Some(idx);
                        }
                    });
                }
            });

            if let Some(idx) = remove_index {
                self.node_routes.remove(idx);
                self.sync_node_routes();
                self.mark_dirty();
            }
            if route_toggled {
                self.sync_node_routes();
                self.mark_dirty();
            }
            if route_params_changed {
                self.sync_node_routes();
                self.mark_dirty();
            }
            });
        });
    }
}
