impl DawApp {
    pub(crate) fn center_arranger(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Arranger");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut hide_labels = !self.settings.show_clip_labels;
                    if ui.checkbox(&mut hide_labels, "Hide clip labels").changed() {
                        self.settings.show_clip_labels = !hide_labels;
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label("Grid Snap");
                let snap_label = if self.arranger_snap_beats < 0.0 {
                    "Cell"
                } else if self.arranger_snap_beats <= 0.0 {
                    "None"
                } else if (self.arranger_snap_beats - 4.0).abs() <= f32::EPSILON {
                    "Bar"
                } else {
                    "Beat"
                };
                egui::ComboBox::from_id_source("arranger_grid_snap")
                    .selected_text(snap_label)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(self.arranger_snap_beats.abs() <= f32::EPSILON, "None")
                            .clicked()
                        {
                            self.arranger_snap_beats = 0.0;
                        }
                        if ui.selectable_label(self.arranger_snap_beats < 0.0, "Cell").clicked() {
                            self.arranger_snap_beats = -1.0;
                        }
                        if ui
                            .selectable_label((self.arranger_snap_beats - 1.0).abs() <= f32::EPSILON, "Beat")
                            .clicked()
                        {
                            self.arranger_snap_beats = 1.0;
                        }
                        if ui
                            .selectable_label((self.arranger_snap_beats - 4.0).abs() <= f32::EPSILON, "Bar")
                            .clicked()
                        {
                            self.arranger_snap_beats = 4.0;
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.label("Tools");
                let tool_size = egui::vec2(110.0, 22.0);
                let icon_size = egui::vec2(14.0, 14.0);
                let button_bg = egui::Color32::from_rgba_premultiplied(18, 20, 24, 220);
                let button_on = egui::Color32::from_rgba_premultiplied(46, 94, 130, 220);
                let icon_tint = egui::Color32::from_gray(220);
                let mut tool_button = |tool: ArrangerTool, icon: egui::ImageSource<'static>, label: &str| {
                    let selected = self.arranger_tool == tool;
                    let button = egui::Button::image_and_text(
                        egui::Image::new(icon).fit_to_exact_size(icon_size).tint(icon_tint),
                        label,
                    )
                    .min_size(tool_size)
                    .fill(if selected { button_on } else { button_bg });
                    if ui.add(button).clicked() {
                        self.arranger_tool = tool;
                    }
                };
                tool_button(
                    ArrangerTool::Draw,
                    egui::include_image!("../../../assets/icons/arranger-write.svg"),
                    "Draw MIDI",
                );
                tool_button(
                    ArrangerTool::Select,
                    egui::include_image!("../../../assets/icons/arranger-box-select.svg"),
                    "Select (Box)",
                );
                tool_button(
                    ArrangerTool::Move,
                    egui::include_image!("../../../assets/icons/arranger-move.svg"),
                    "Move",
                );
                tool_button(
                    ArrangerTool::Slice,
                    egui::include_image!("../../../assets/icons/scissors.svg"),
                    "Slice",
                );
                ui.separator();
                let auto_perf_response = ui.button("Auto Performance");
                if auto_perf_response.clicked() {
                    self.push_undo_state();
                    let summary = self.auto_build_performance_from_arrangement();
                    if !summary.changed() {
                        self.undo_stack.pop();
                        self.status = "Smart performance build found no useful section changes".to_string();
                    } else {
                        self.mark_dirty();
                        self.status = summary.status_message();
                    }
                }
                auto_perf_response.on_hover_text("Analyze the arrangement, slice it into performance sections, and enable loop pads for repetitive material.");
            });
            ui.add_space(6.0);
            ui.add_space(6.0);
            let row_height = 52.0;
            let header_height = 24.0;
            let lane_label_w = 160.0;
            #[derive(Clone, Copy)]
            enum ArrangerRow {
                Track { track_index: usize },
                Automation { track_index: usize, lane_index: usize },
            }
            let mut rows: Vec<ArrangerRow> = Vec::new();
            let mut track_row_indices = vec![0usize; self.tracks.len()];
            for (track_index, track) in self.tracks.iter().enumerate() {
                track_row_indices[track_index] = rows.len();
                rows.push(ArrangerRow::Track { track_index });
                if self.automation_rows_expanded.contains(&track_index) {
                    for (lane_index, _lane) in track.automation_lanes.iter().enumerate() {
                        rows.push(ArrangerRow::Automation {
                            track_index,
                            lane_index,
                        });
                    }
                }
            }
            let (rect, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
            let pointer_pos = response
                .hover_pos()
                .or_else(|| ctx.input(|i| i.pointer.hover_pos()));
            let over_arranger = pointer_pos
                .map(|pos| rect.contains(pos))
                .unwrap_or(false);
            let box_select_active = ctx.input(|i| i.key_down(egui::Key::B) || i.modifiers.ctrl);
            if over_arranger && !self.piano_roll_hovered {
                let input = ctx.input(|i| i.clone());
                let mmb_down = input.pointer.button_down(egui::PointerButton::Middle);
                if mmb_down {
                    self.arranger_pan += input.pointer.delta();
                    response.clone().on_hover_cursor(egui::CursorIcon::Move);
                } else if input.modifiers.ctrl {
                    let pointer_x = pointer_pos
                        .map(|pos| pos.x)
                        .unwrap_or(rect.left() + lane_label_w);
                    let local_x = pointer_x - (rect.left() + lane_label_w);
                    let before_zoom = self.arranger_zoom;
                    let zoom = input.zoom_delta();
                    if (zoom - 1.0).abs() > f32::EPSILON {
                        self.arranger_zoom = (self.arranger_zoom * zoom).clamp(0.05, 4.0);
                    } else {
                        let mut delta = input.smooth_scroll_delta;
                        if delta == egui::Vec2::ZERO {
                            delta = input.raw_scroll_delta;
                        }
                        let zoom_delta = (delta.x + delta.y) * 0.01;
                        self.arranger_zoom = (self.arranger_zoom + zoom_delta).clamp(0.05, 4.0);
                    }
                    let scale = if before_zoom > 0.0 {
                        self.arranger_zoom / before_zoom
                    } else {
                        1.0
                    };
                    self.arranger_pan.x = (self.arranger_pan.x - local_x) * scale + local_x;
                } else {
                    let mut delta = input.smooth_scroll_delta;
                    if delta == egui::Vec2::ZERO {
                        delta = input.raw_scroll_delta;
                    }
                    if input.modifiers.shift && delta.x.abs() < f32::EPSILON {
                        delta = egui::vec2(delta.y, 0.0);
                    }
                    self.arranger_pan += egui::vec2(-delta.x, -delta.y);
                }
            }
            let mut max_end_beats = self.playhead_beats.max(4.0);
            for track in &self.tracks {
                for clip in &track.clips {
                    let end = clip.start_beats + clip.length_beats;
                    if end > max_end_beats {
                        max_end_beats = end;
                    }
                }
            }
            let view_width = (rect.width() - lane_label_w - 8.0).max(1.0);
            self.arranger_zoom = self.arranger_zoom.clamp(0.05, 4.0);
            let beat_width = 22.0 * self.arranger_zoom;
            let beats_per_view = view_width / beat_width.max(0.001);
            let draw_step = if beats_per_view >= 64.0 {
                16.0
            } else if beats_per_view >= 32.0 {
                4.0
            } else if beats_per_view >= 20.0 {
                2.0
            } else if beats_per_view >= 12.0 {
                1.0
            } else if beats_per_view >= 8.0 {
                0.5
            } else {
                0.25
            };
            let major_step = 4.0f32;
            let band_step = if draw_step >= major_step {
                draw_step
            } else {
                major_step
            };
            let arranger_snap = if self.arranger_snap_beats.abs() <= f32::EPSILON {
                0.0
            } else {
                draw_step
            };
            let content_width = max_end_beats * beat_width + 160.0;
            let min_pan_x = (view_width - content_width).min(0.0);
            let view_height = rect.height().max(1.0);
            let content_height = header_height + rows.len().max(1) as f32 * row_height + 8.0;
            // Allow extra vertical pan only while the piano roll panel is in use.
            let piano_roll_open = self.selected_clip.is_some();
            let piano_roll_margin = if piano_roll_open {
                self.piano_roll_panel_height
            } else {
                0.0
            };
            let min_pan_y = (view_height - content_height - piano_roll_margin).min(0.0);
            self.arranger_pan.x = self.arranger_pan.x.clamp(min_pan_x, 0.0);
            self.arranger_pan.y = self.arranger_pan.y.clamp(min_pan_y, 0.0);
            let row_top_offset = header_height + self.arranger_pan.y;
            let track_for_pos = |pos: egui::Pos2| -> Option<usize> {
                let row_index = ((pos.y - rect.top() - row_top_offset) / row_height).floor() as i32;
                if row_index < 0 {
                    return None;
                }
                rows.get(row_index as usize).and_then(|row| match *row {
                    ArrangerRow::Track { track_index } => Some(track_index),
                    ArrangerRow::Automation { track_index, .. } => Some(track_index),
                })
            };
            let painter = ui.painter_at(rect);
            let arranger_bg = egui::Color32::from_rgb(8, 9, 11);
            let playlist_bg = egui::Color32::from_rgb(18, 20, 24);
            painter.rect_filled(rect, 0.0, arranger_bg);
            let header_rect = egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.top()),
                egui::pos2(rect.right(), rect.top() + header_height),
            );
            painter.rect_filled(header_rect, 0.0, playlist_bg);
            let timeline_bottom = header_rect.bottom();
            let row_left = rect.left() + lane_label_w + self.arranger_pan.x;
            let header_id = egui::Id::new("arranger_timeline");
            let header_response = ui.interact(header_rect, header_id, egui::Sense::click());
            let header_pos = header_response.interact_pointer_pos();
            let header_clicked = header_response.clicked();
            let menu_color = self
                .selected_track
                .map(|index| self.track_color(index))
                .unwrap_or_else(|| egui::Color32::from_gray(200));
            if header_clicked {
                if let Some(pos) = header_pos {
                    let beats = self.beats_from_pos(pos.x, row_left, beat_width);
                    self.seek_playhead(beats);
                }
            }
            header_response.context_menu(|ui| {
                let Some(pos) = header_pos else {
                    ui.label("No cursor position");
                    return;
                };
                let beats = self.beats_from_pos(pos.x, row_left, beat_width);
                if ui
                    .add(egui::Button::image_and_text(
                        egui::Image::new(egui::include_image!("../../../assets/icons/flag.svg"))
                            .fit_to_exact_size(egui::vec2(12.0, 12.0))
                            .tint(menu_color),
                        egui::RichText::new("Set Loop Start").color(menu_color),
                    ))
                    .clicked()
                {
                    self.loop_start_beats = Some(beats);
                    if let Some(end) = self.loop_end_beats {
                        if end < beats {
                            self.loop_end_beats = Some(beats);
                        }
                    }
                    ui.close_menu();
                }
                if ui
                    .add(egui::Button::image_and_text(
                        egui::Image::new(egui::include_image!("../../../assets/icons/flag.svg"))
                            .fit_to_exact_size(egui::vec2(12.0, 12.0))
                            .tint(menu_color),
                        egui::RichText::new("Set Loop End").color(menu_color),
                    ))
                    .clicked()
                {
                    self.loop_end_beats = Some(beats);
                    if let Some(start) = self.loop_start_beats {
                        if beats < start {
                            self.loop_start_beats = Some(beats);
                            self.loop_end_beats = Some(start);
                        }
                    }
                    ui.close_menu();
                }
                if ui
                    .add(egui::Button::image_and_text(
                        egui::Image::new(egui::include_image!("../../../assets/icons/move.svg"))
                            .fit_to_exact_size(egui::vec2(12.0, 12.0))
                            .tint(menu_color),
                        egui::RichText::new("Move Loop Point Here").color(menu_color),
                    ))
                    .clicked()
                {
                    let beats = beats.max(0.0);
                    match (self.loop_start_beats, self.loop_end_beats) {
                        (Some(start), Some(end)) => {
                            let dist_start = (beats - start).abs();
                            let dist_end = (beats - end).abs();
                            if dist_start <= dist_end {
                                let new_start = beats.min(end - 0.25).max(0.0);
                                self.loop_start_beats = Some(new_start);
                            } else {
                                let new_end = beats.max(start + 0.25);
                                self.loop_end_beats = Some(new_end);
                            }
                        }
                        (Some(_start), None) => {
                            self.loop_start_beats = Some(beats);
                        }
                        (None, Some(_end)) => {
                            self.loop_end_beats = Some(beats.max(0.25));
                        }
                        (None, None) => {
                            self.loop_start_beats = Some(beats);
                            self.loop_end_beats = Some((beats + 4.0).max(beats + 0.25));
                        }
                    }
                    ui.close_menu();
                }
                if ui
                    .add(egui::Button::image_and_text(
                        egui::Image::new(egui::include_image!("../../../assets/icons/x.svg"))
                            .fit_to_exact_size(egui::vec2(12.0, 12.0))
                            .tint(menu_color),
                        egui::RichText::new("Clear Loop").color(menu_color),
                    ))
                    .clicked()
                {
                    self.loop_start_beats = None;
                    self.loop_end_beats = None;
                    ui.close_menu();
                }
            });
            let playhead_x = row_left + self.playhead_beats * beat_width;
            let grid_top = (rect.top() + row_top_offset).max(header_rect.bottom());
            let grid_bottom = rect.bottom() - 8.0;
            let grid_left = rect.left() + lane_label_w;
            let grid_right = rect.right() - 8.0;
            let grid_clip = egui::Rect::from_min_max(
                egui::pos2(grid_left, grid_top),
                egui::pos2(grid_right, grid_bottom),
            );
            let grid_painter = painter.with_clip_rect(grid_clip);
            let clip_painter = painter.with_clip_rect(grid_clip);
            let shelf_clip = egui::Rect::from_min_max(
                egui::pos2(rect.left(), grid_top),
                egui::pos2(grid_left, grid_bottom),
            );
            let shelf_painter = painter.with_clip_rect(shelf_clip);
            let major_div = if draw_step >= major_step {
                1
            } else {
                (major_step / draw_step).round() as i32
            };
            let mut x = row_left;
            let step_px = beat_width * draw_step;
            let skip_steps = ((grid_left - row_left) / step_px).floor() as i32;
            let mut minor_index = skip_steps.max(0);
            x += minor_index as f32 * step_px;
            while x <= grid_right {
                let major = major_div > 0 && minor_index % major_div == 0;
                let line_x = x.round() + 0.5;
                let line_width = if major { 2.0 } else { 1.0 };
                let color = if major {
                    egui::Color32::from_rgba_premultiplied(20, 22, 26, 110)
                } else {
                    egui::Color32::from_rgba_premultiplied(14, 16, 20, 90)
                };
                grid_painter.line_segment(
                    [egui::pos2(line_x, grid_top), egui::pos2(line_x, grid_bottom)],
                    egui::Stroke::new(line_width, color),
                );
                if major {
                    let band_rect = egui::Rect::from_min_max(
                        egui::pos2(x, grid_top),
                        egui::pos2(x + beat_width * band_step, grid_bottom),
                    );
                    let band_index = if major_div > 0 { minor_index / major_div } else { 0 };
                    let band_color = if band_index % 2 == 0 {
                        egui::Color32::from_rgba_premultiplied(0, 0, 0, 0)
                    } else {
                        egui::Color32::from_rgba_premultiplied(4, 6, 8, 120)
                    };
                    grid_painter.rect_filled(band_rect, 0.0, band_color);
                }
                minor_index += 1;
                x += step_px;
            }
            if let (Some(start), Some(end)) = (self.loop_start_beats, self.loop_end_beats) {
                if end > start {
                    let loop_x1 = row_left + start * beat_width;
                    let loop_x2 = row_left + end * beat_width;
                    let loop_rect = egui::Rect::from_min_max(
                        egui::pos2(loop_x1, grid_top),
                        egui::pos2(loop_x2, grid_bottom),
                    );
                    grid_painter.rect_filled(
                        loop_rect,
                        0.0,
                        egui::Color32::from_rgba_premultiplied(90, 120, 220, 36),
                    );
                    grid_painter.line_segment(
                        [egui::pos2(loop_x1, grid_top), egui::pos2(loop_x1, grid_bottom)],
                        egui::Stroke::new(1.2, egui::Color32::from_rgb(140, 180, 255)),
                    );
                    grid_painter.line_segment(
                        [egui::pos2(loop_x2, grid_top), egui::pos2(loop_x2, grid_bottom)],
                        egui::Stroke::new(1.2, egui::Color32::from_rgb(140, 180, 255)),
                    );
                }
            }

            let shelf_rect = egui::Rect::from_min_max(
                egui::pos2(rect.left(), grid_top),
                egui::pos2(rect.left() + lane_label_w, grid_bottom),
            );
            shelf_painter.rect_filled(shelf_rect, 0.0, playlist_bg);
            let timeline_clip = egui::Rect::from_min_max(
                egui::pos2(row_left, header_rect.top()),
                egui::pos2(header_rect.right(), header_rect.bottom()),
            );
            let _header_painter = painter.with_clip_rect(timeline_clip);

            let dropped_files = ctx.input(|i| i.raw.dropped_files.clone());
            if !dropped_files.is_empty() {
                let pointer = response
                    .hover_pos()
                    .or_else(|| ctx.input(|i| i.pointer.latest_pos()))
                    .or_else(|| ctx.input(|i| i.pointer.hover_pos()));
                let mut target_track = self.selected_track.unwrap_or(0).min(self.tracks.len().saturating_sub(1));
                let mut start_beats = self.playhead_beats.max(0.0);
                if let Some(pos) = pointer {
                    if rect.contains(pos) {
                        if let Some(track_index) = track_for_pos(pos) {
                            target_track = track_index;
                        }
                        start_beats = ((pos.x - row_left) / beat_width).max(0.0);
                    }
                }
                self.push_undo_state();
                let mut midi_started = false;
                for (index, file) in dropped_files.iter().enumerate() {
                    let Some(path) = file.path.as_ref() else {
                        continue;
                    };
                    let offset = index as f32 * 0.5;
                    match Self::fs_drag_kind_for_path(path) {
                        Some(FsDragKind::Midi) => {
                            if !midi_started {
                                let _ = self.begin_midi_import_with_mode(
                                    path.to_string_lossy().to_string(),
                                    MidiImportMode::AppendTracks {
                                        start_beats: start_beats + offset,
                                    },
                                );
                                midi_started = true;
                            }
                        }
                        Some(FsDragKind::Audio) => match self.add_audio_clip_from_path(
                            target_track,
                            start_beats + offset,
                            path,
                        ) {
                            Ok(()) => {
                                self.status = format!("Added clip: {}", path.to_string_lossy());
                            }
                            Err(err) => {
                                self.status = format!("Drop import failed: {err}");
                            }
                        },
                        None => {}
                    }
                }
            }

            if ctx.input(|i| i.pointer.any_released()) {
                if let Some(fs_drag) = self.fs_drag.take() {
                    if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) {
                        if rect.contains(pos) {
                            let mut target_track = self
                                .selected_track
                                .unwrap_or(0)
                                .min(self.tracks.len().saturating_sub(1));
                            if let Some(track_index) = track_for_pos(pos) {
                                target_track = track_index;
                            }
                            let start_beats = ((pos.x - row_left) / beat_width).max(0.0);
                            match fs_drag.kind {
                                FsDragKind::Audio => {
                                    self.push_undo_state();
                                    match self.add_audio_clip_from_path(
                                        target_track,
                                        start_beats,
                                        &fs_drag.path,
                                    ) {
                                        Ok(()) => {
                                            self.status = format!(
                                                "Added clip: {}",
                                                fs_drag.path.to_string_lossy()
                                            );
                                        }
                                        Err(err) => {
                                            self.status = format!("Drop import failed: {err}");
                                        }
                                    }
                                }
                                FsDragKind::Midi => {
                                    let _ = self.begin_midi_import_with_mode(
                                        fs_drag.path.to_string_lossy().to_string(),
                                        MidiImportMode::AppendTracks { start_beats },
                                    );
                                }
                            }
                        }
                    }
                }
            }

            #[derive(Clone, Copy)]
            enum TrackContextAction {
                CloneOnly(usize),
                DuplicateWithClips(usize),
                Delete(usize),
                MoveUp(usize),
                MoveDown(usize),
                ToggleSolo(usize),
                ToggleMute(usize),
            }

            let mut pending_select: Option<(usize, usize, bool)> = None;
            let mut pending_multi_select: Option<Vec<(usize, usize)>> = None;
            let mut pending_delete: Option<usize> = None;
            let mut pending_clip_rename: Option<(usize, usize)> = None;
            let mut pending_drag_start: Option<ClipDragState> = None;
            let mut pending_track_select: Option<usize> = None;
            let mut pending_track_move: Option<(usize, usize)> = None;
            let mut pending_track_action: Option<TrackContextAction> = None;
            let mut pending_stamp_copy: Option<(Clip, usize, usize, f32, f32, f32)> = None;
            let mut over_clip = false;
            let mut switch_to_move = false;

            let mut pending_lane_edit: Vec<(usize, usize, f32, f32)> = Vec::new();
            let row_start_index = ((-self.arranger_pan.y) / row_height).floor() as i32;
            let row_start_index = row_start_index.max(0) as usize;
            let row_end_index = ((-self.arranger_pan.y + rect.height()) / row_height).ceil() as i32;
            let row_end_index = (row_end_index + 1).min(rows.len() as i32) as usize;

            for row_index in row_start_index..row_end_index {
                let row = &rows[row_index];
                let y = rect.top() + row_top_offset + row_index as f32 * row_height;
                let label_rect = egui::Rect::from_min_max(
                    egui::pos2(rect.left(), y),
                    egui::pos2(rect.left() + lane_label_w, y + row_height),
                );
                let row_rect = egui::Rect::from_min_max(
                    egui::pos2(label_rect.right(), y),
                    egui::pos2(rect.right() - 8.0, y + row_height),
                );
                let row_click_rect = egui::Rect::from_min_max(
                    egui::pos2(rect.left(), y),
                    egui::pos2(rect.right() - 8.0, y + row_height),
                );
                let row_click_top = row_click_rect.top().max(timeline_bottom);
                let row_click_rect = egui::Rect::from_min_max(
                    egui::pos2(row_click_rect.left(), row_click_top),
                    row_click_rect.max,
                );
                let label_rect = label_rect.intersect(shelf_clip);
                let row_rect = row_rect.intersect(grid_clip);
                let row_click_rect = row_click_rect.intersect(grid_clip);
                if row_click_rect.height() <= 0.0 || row_rect.height() <= 0.0 {
                    continue;
                }
                match *row {
                    ArrangerRow::Track { track_index } => {
                        let track_clips = match self.tracks.get(track_index) {
                            Some(track) => track.clips.clone(),
                            None => continue,
                        };
                        clip_painter.rect_filled(row_rect, 0.0, egui::Color32::from_rgb(0, 0, 0));
                        let row_id = egui::Id::new(format!("arranger_track_row_{}", track_index));
                        let row_response = ui.interact(row_click_rect, row_id, egui::Sense::click());
                        if row_response.clicked() {
                            pending_track_select = Some(track_index);
                        }
                        clip_painter.rect_stroke(
                            row_rect,
                            0.0,
                            egui::Stroke::new(1.0, egui::Color32::from_rgb(0, 0, 0)),
                        );
                let visible_start_beats = (-self.arranger_pan.x / beat_width).max(0.0) - 1.0;
                let visible_end_beats = ((-self.arranger_pan.x + rect.width()) / beat_width) + 1.0;
                for clip in &track_clips {
                    if clip.start_beats + clip.length_beats < visible_start_beats || clip.start_beats > visible_end_beats {
                        continue;
                    }
                            let clip_x = row_left + clip.start_beats * beat_width;
                            let clip_w = (clip.length_beats * beat_width).max(1.0);
                            let clip_left = clip_x.max(row_rect.left());
                            let clip_right = (clip_x + clip_w).min(row_rect.right());
                            if clip_right <= clip_left {
                                continue;
                            }
                            let clip_rect = egui::Rect::from_min_max(
                                egui::pos2(clip_left, row_rect.top()),
                                egui::pos2(clip_right, row_rect.bottom()),
                            );
                            let clip_interact_top = clip_rect.top().max(timeline_bottom);
                            if clip_interact_top >= clip_rect.bottom() {
                                continue;
                            }
                            let clip_interact_rect = egui::Rect::from_min_max(
                                egui::pos2(clip_rect.left(), clip_interact_top),
                                clip_rect.max,
                            );
                            let selected = self.selected_clips.contains(&clip.id);
                            let base = self.track_color(track_index);
                            let header_h = 14.0;
                            let header_rect = egui::Rect::from_min_size(
                                clip_rect.min,
                                egui::vec2(clip_rect.width(), header_h),
                            );
                            let body_rect = egui::Rect::from_min_max(
                                egui::pos2(clip_rect.left(), clip_rect.top() + header_h),
                                clip_rect.max,
                            );
                            let header_alpha = if selected { 220 } else { 170 };
                            let header_color = egui::Color32::from_rgba_premultiplied(
                                base.r(),
                                base.g(),
                                base.b(),
                                header_alpha,
                            );
                            let body_color = egui::Color32::from_rgba_premultiplied(
                                base.r(),
                                base.g(),
                                base.b(),
                                70,
                            );
                            clip_painter.rect_filled(body_rect, 0.0, body_color);
                            clip_painter.rect_filled(header_rect, 0.0, header_color);
                            let block_beats = 4.0;
                            let clip_start = clip.start_beats.max(0.0);
                            let clip_end = (clip.start_beats + clip.length_beats).max(clip_start);
                            let mut block_start = (clip_start / block_beats).floor() * block_beats;
                            while block_start < clip_end {
                                let block_end = block_start + block_beats;
                                let seg_start = clip_start.max(block_start);
                                let seg_end = clip_end.min(block_end);
                                if seg_end > seg_start {
                                    let x1 = row_left + seg_start * beat_width;
                                    let x2 = row_left + seg_end * beat_width;
                                    let overlay_rect = egui::Rect::from_min_max(
                                        egui::pos2(x1, clip_rect.top()),
                                        egui::pos2(x2, clip_rect.bottom()),
                                    );
                                    let block_index = (block_start / block_beats) as i32;
                                    let overlay = if block_index % 2 == 0 {
                                        egui::Color32::from_rgba_premultiplied(0, 0, 0, 0)
                                    } else {
                                        egui::Color32::from_rgba_premultiplied(0, 0, 0, 28)
                                    };
                                    clip_painter.rect_filled(overlay_rect, 0.0, overlay);
                                }
                                block_start = block_end;
                            }
                            let clip_stroke = if selected {
                                egui::Stroke::new(2.0, Self::tint(base, 1.0))
                            } else {
                                egui::Stroke::new(1.0, Self::tint(base, 0.7))
                            };
                            clip_painter.rect_stroke(clip_rect, 0.0, clip_stroke);
                            if let Some(loop_len) = self.clip_loop_len_beats(clip) {
                                if loop_len > 0.0 && clip.length_beats > loop_len + 0.0001 {
                                    let mut marker = clip.start_beats + loop_len;
                                    while marker < clip.start_beats + clip.length_beats - 0.0001 {
                                        let x = row_left + marker * beat_width;
                                        if x >= clip_rect.left() && x <= clip_rect.right() {
                                            clip_painter.line_segment(
                                                [
                                                    egui::pos2(x, clip_rect.top()),
                                                    egui::pos2(x, clip_rect.bottom()),
                                                ],
                                                egui::Stroke::new(
                                                    1.0,
                                                    egui::Color32::from_rgba_premultiplied(220, 230, 255, 120),
                                                ),
                                            );
                                        }
                                        marker += loop_len;
                                    }
                                }
                            }
                            let handle_w = 12.0;
                            let name = if clip.name.trim().is_empty() {
                                if clip.is_midi { "MIDI" } else { "Audio" }
                            } else {
                                clip.name.as_str()
                            };
                            if self.settings.show_clip_labels {
                                let header_text = ui.fonts(|f| {
                                    let font = egui::FontId::proportional(
                                        (BASE_UI_FONT_SIZE - 1.0).max(6.0),
                                    );
                                    let max_width = (header_rect.width() - 10.0).max(4.0);
                                    let mut text = name.to_string();
                                    while text.len() > 1
                                        && f
                                            .layout_no_wrap(
                                                text.clone(),
                                                font.clone(),
                                                egui::Color32::WHITE,
                                            )
                                            .size()
                                            .x
                                            > max_width
                                    {
                                        text.pop();
                                    }
                                    if text.len() < name.len() {
                                        if text.len() > 3 {
                                            text.truncate(text.len().saturating_sub(3));
                                        }
                                        text.push_str("...");
                                    }
                                    text
                                });
                                Self::outlined_text(
                                    &clip_painter,
                                    egui::pos2(header_rect.left() + 4.0, header_rect.center().y),
                                    egui::Align2::LEFT_CENTER,
                                    &header_text,
                                    egui::FontId::proportional((BASE_UI_FONT_SIZE - 1.0).max(6.0)),
                                    egui::Color32::WHITE,
                                );
                            }
                            if clip.is_midi {
                                let preview_rect = body_rect.shrink2(egui::vec2(2.0, 2.0));
                                self.draw_midi_preview(
                                    &clip_painter,
                                    preview_rect,
                                    clip,
                                    clip_x,
                                    beat_width,
                                );
                            } else {
                                let preview_rect = body_rect.shrink2(egui::vec2(6.0, 8.0));
                                let waveform = self.get_waveform_for_clip(clip);
                                let waveform_color = self.get_waveform_color_for_clip(clip);
                                self.draw_audio_preview(
                                    &clip_painter,
                                    preview_rect,
                                    clip.id,
                                    waveform.as_deref(),
                                    waveform_color.as_deref(),
                                    clip,
                                    Some((row_left, beat_width)),
                                );
                            }

                            let trim_h = 10.0;
                            let header_left = egui::Rect::from_min_size(
                                egui::pos2(header_rect.left(), header_rect.top()),
                                egui::vec2(handle_w, header_rect.height()),
                            );
                            let header_right = egui::Rect::from_min_size(
                                egui::pos2(header_rect.right() - handle_w, header_rect.top()),
                                egui::vec2(handle_w, header_rect.height()),
                            );
                            let header_label_rect = egui::Rect::from_min_max(
                                egui::pos2(header_left.right(), header_rect.top()),
                                egui::pos2(header_right.left(), header_rect.bottom()),
                            );
                            let trim_left = egui::Rect::from_min_size(
                                egui::pos2(body_rect.left(), clip_rect.bottom() - trim_h),
                                egui::vec2(handle_w, trim_h),
                            );
                            let trim_right = egui::Rect::from_min_size(
                                egui::pos2(body_rect.right() - handle_w, clip_rect.bottom() - trim_h),
                                egui::vec2(handle_w, trim_h),
                            );

                            let header_left_id = egui::Id::new(format!("clip_header_left_{}", clip.id));
                            let header_right_id = egui::Id::new(format!("clip_header_right_{}", clip.id));
                            let header_label_id = egui::Id::new(format!("clip_header_label_{}", clip.id));
                            let trim_left_id = egui::Id::new(format!("clip_trim_left_{}", clip.id));
                            let trim_right_id = egui::Id::new(format!("clip_trim_right_{}", clip.id));
                            let header_visible = header_rect.top() >= timeline_bottom;
                            let header_left_resp = header_visible.then(|| {
                                ui.interact(header_left, header_left_id, egui::Sense::click_and_drag())
                            });
                            let header_right_resp = header_visible.then(|| {
                                ui.interact(header_right, header_right_id, egui::Sense::click_and_drag())
                            });
                            let header_label_resp = header_visible.then(|| {
                                ui.interact(header_label_rect, header_label_id, egui::Sense::click())
                            });
                            let trim_left_resp = header_visible.then(|| {
                                ui.interact(trim_left, trim_left_id, egui::Sense::click_and_drag())
                            });
                            let trim_right_resp = header_visible.then(|| {
                                ui.interact(trim_right, trim_right_id, egui::Sense::click_and_drag())
                            });

                            let clip_id = egui::Id::new(format!("clip_{}", clip.id));
                            let mut clip_response =
                                ui.interact(clip_interact_rect, clip_id, egui::Sense::click_and_drag());
                            if clip_response.hovered() {
                                if let Some(pos) = clip_response.interact_pointer_pos() {
                                    let edge_pad = 10.0;
                                    let near_left = (pos.x - clip_rect.left()).abs() <= edge_pad;
                                    let near_right = (clip_rect.right() - pos.x).abs() <= edge_pad;
                                    let icon = if self.arranger_tool == ArrangerTool::Slice {
                                        egui::CursorIcon::Crosshair
                                    } else if near_left || near_right {
                                        egui::CursorIcon::ResizeHorizontal
                                    } else {
                                        egui::CursorIcon::Move
                                    };
                                    clip_response = clip_response.on_hover_cursor(icon);
                                }
                            }
                            if clip_response.hovered() {
                                over_clip = true;
                            }
                            if self.arranger_tool != ArrangerTool::Slice && clip_response.double_clicked() {
                                pending_select = Some((clip.id, track_index, false));
                                if let Some(pos) = clip_response.interact_pointer_pos() {
                                    let beat_at_click = (pos.x - row_left) / beat_width;
                                    let local = (beat_at_click - clip.start_beats).max(0.0);
                                    self.piano_focus_beats = Some(local);
                                }
                                self.main_tab = MainTab::PianoRoll;
                            }
                            let clip_header_clicked = header_left_resp
                                .as_ref()
                                .map_or(false, |resp| resp.clicked())
                                || header_right_resp
                                    .as_ref()
                                    .map_or(false, |resp| resp.clicked());
                            if header_label_resp.as_ref().map_or(false, |resp| resp.clicked()) && selected {
                                pending_clip_rename = Some((track_index, clip.id));
                            }
                            if !header_clicked
                                && self.arranger_tool != ArrangerTool::Draw
                                && self.arranger_tool != ArrangerTool::Slice
                                && (clip_response.clicked() || clip_header_clicked)
                            {
                                let add = ctx.input(|i| i.modifiers.shift || i.modifiers.ctrl);
                                pending_select = Some((clip.id, track_index, add));
                                if self.arranger_tool == ArrangerTool::Select {
                                    switch_to_move = true;
                                }
                            }

                            let can_grab = pending_drag_start.is_none();
                            let mut start_drag =
                                |this: &mut DawApp, kind: ClipDragKind, pos: Option<egui::Pos2>| {
                                if let Some(pos) = pos {
                                    let offset_beats = (pos.x - row_left) / beat_width - clip.start_beats;
                                    let shift_copy = ui.input(|i| i.modifiers.shift);
                                    let mut clip_id = clip.id;
                                    let mut copy_mode = false;
                                    let mut undo_pushed = false;
                                    let mut group: Option<Vec<ClipDragGroupItem>> = None;
                                    let multi_selected = this.selected_clips.len() > 1
                                        && this.selected_clips.contains(&clip.id);
                                    if shift_copy && kind == ClipDragKind::Move && multi_selected {
                                        let mut new_ids = Vec::new();
                                        let mut group_items = Vec::new();
                                        let mut primary_new_id = None;
                                        this.push_undo_state();
                                        undo_pushed = true;
                                        let selected_ids: Vec<usize> =
                                            this.selected_clips.iter().copied().collect();
                                        for selected_id in selected_ids {
                                            let mut found = None;
                                            for (ti, track) in this.tracks.iter().enumerate() {
                                                if let Some(found_clip) =
                                                    track.clips.iter().find(|c| c.id == selected_id)
                                                {
                                                    found = Some((ti, found_clip.clone()));
                                                    break;
                                                }
                                            }
                                            let Some((ti, mut copy)) = found else {
                                                continue;
                                            };
                                            let new_id = this.next_clip_id();
                                            copy.id = new_id;
                                            copy.track = ti;
                                            copy.link_id = this.ensure_clip_link_id(ti, selected_id);
                                            if let Some(track) = this.tracks.get_mut(ti) {
                                                track.clips.push(copy.clone());
                                            }
                                            if copy.is_midi {
                                                this.sync_track_audio_notes(ti);
                                            }
                                            group_items.push(ClipDragGroupItem {
                                                clip_id: new_id,
                                                source_track: ti,
                                                start_beats: copy.start_beats,
                                                length_beats: copy.length_beats,
                                                is_midi: copy.is_midi,
                                            });
                                            if selected_id == clip.id {
                                                primary_new_id = Some(new_id);
                                            }
                                            new_ids.push(new_id);
                                        }
                                        if let Some(primary_id) = primary_new_id {
                                            clip_id = primary_id;
                                        }
                                        this.selected_clips.clear();
                                        for id in &new_ids {
                                            this.selected_clips.insert(*id);
                                        }
                                        this.selected_clip = Some(clip_id);
                                        group = Some(group_items);
                                        copy_mode = true;
                                    }
                                    if shift_copy && kind == ClipDragKind::Move {
                                        if group.is_none() {
                                            let new_id = this.next_clip_id();
                                            let link_id = this.ensure_clip_link_id(track_index, clip.id);
                                            if let Some(track) = this.tracks.get_mut(track_index) {
                                                let mut copy = clip.clone();
                                                copy.id = new_id;
                                                copy.link_id = link_id;
                                                track.clips.push(copy);
                                                if clip.is_midi {
                                                    this.sync_track_audio_notes(track_index);
                                                }
                                                clip_id = new_id;
                                                copy_mode = true;
                                                undo_pushed = true;
                                                this.push_undo_state();
                                            }
                                        }
                                    }
                                    pending_drag_start = Some(ClipDragState {
                                        clip_id,
                                        source_track: track_index,
                                        origin_track: track_index,
                                        offset_beats,
                                        start_beats: clip.start_beats,
                                        length_beats: clip.length_beats,
                                        origin_start_beats: clip.start_beats,
                                        origin_length_beats: clip.length_beats,
                                        audio_offset_beats: clip.audio_offset_beats,
                                        audio_source_beats: clip.audio_source_beats,
                                        kind,
                                        undo_pushed,
                                        grabbed: false,
                                        copy_mode,
                                        group,
                                    });
                                }
                            };

                            if clip_response.hovered()
                                && can_grab
                                && ctx.input(|i| i.key_pressed(egui::Key::G))
                            {
                                pending_select = Some((clip.id, track_index, false));
                                let pos = clip_response
                                    .interact_pointer_pos()
                                    .or_else(|| ctx.input(|i| i.pointer.interact_pos()));
                                start_drag(self, ClipDragKind::Move, pos);
                            }

                            if let Some(resp) = header_left_resp.as_ref() {
                                if resp.drag_started() {
                                    pending_select = Some((clip.id, track_index, false));
                                    start_drag(self, ClipDragKind::ResizeStart, resp.interact_pointer_pos());
                                }
                            }
                            if let Some(resp) = header_right_resp.as_ref() {
                                if resp.drag_started() {
                                    pending_select = Some((clip.id, track_index, false));
                                    start_drag(self, ClipDragKind::ResizeEnd, resp.interact_pointer_pos());
                                }
                            }
                            if let Some(resp) = trim_left_resp.as_ref() {
                                if resp.drag_started() {
                                    pending_select = Some((clip.id, track_index, false));
                                    start_drag(self, ClipDragKind::TrimStart, resp.interact_pointer_pos());
                                }
                            }
                            if let Some(resp) = trim_right_resp.as_ref() {
                                if resp.drag_started() {
                                    pending_select = Some((clip.id, track_index, false));
                                    start_drag(self, ClipDragKind::TrimEnd, resp.interact_pointer_pos());
                                }
                            }
                            if clip_response.drag_started() {
                                pending_select = Some((clip.id, track_index, false));
                                let pos = clip_response.interact_pointer_pos();
                                let edge_pad = 10.0;
                                let kind = if let Some(pos) = pos {
                                    let near_left = (pos.x - clip_rect.left()).abs() <= edge_pad;
                                    let near_right = (clip_rect.right() - pos.x).abs() <= edge_pad;
                                    if near_left {
                                        ClipDragKind::ResizeStart
                                    } else if near_right {
                                        ClipDragKind::ResizeEnd
                                    } else {
                                        ClipDragKind::Move
                                    }
                                } else {
                                    ClipDragKind::Move
                                };
                                start_drag(self, kind, pos);
                            }

                            clip_response.context_menu(|ui| {
                                let clone_label = if self.selected_clips.len() > 1
                                    && self.selected_clips.contains(&clip.id)
                                {
                                    "Clone Selected Clips"
                                } else {
                                    "Clone Clip"
                                };
                                if ui
                                    .add(egui::Button::image_and_text(
                                        egui::Image::new(egui::include_image!("../../../assets/icons/copy.svg"))
                                            .fit_to_exact_size(egui::vec2(12.0, 12.0))
                                            .tint(base),
                                        egui::RichText::new(clone_label).color(base),
                                    ))
                                    .clicked()
                                {
                                    let clone_ids: Vec<usize> = if self.selected_clips.contains(&clip.id) {
                                        self.selected_clips.iter().copied().collect()
                                    } else {
                                        vec![clip.id]
                                    };
                                    self.clone_clips_by_ids(&clone_ids);
                                    ui.close_menu();
                                }
                                if clip.link_id.is_some() {
                                    if ui
                                        .add(egui::Button::image_and_text(
                                            egui::Image::new(egui::include_image!("../../../assets/icons/link-2.svg"))
                                                .fit_to_exact_size(egui::vec2(12.0, 12.0))
                                                .tint(base),
                                            egui::RichText::new("Make Unique").color(base),
                                        ))
                                        .clicked()
                                    {
                                        self.make_clip_unique(track_index, clip.id);
                                        ui.close_menu();
                                    }
                                }
                                let can_merge = self.can_merge_selected_clips()
                                    && self.selected_clips.contains(&clip.id);
                                if ui
                                    .add_enabled(
                                        can_merge,
                                        egui::Button::image_and_text(
                                            egui::Image::new(egui::include_image!("../../../assets/icons/git-merge.svg"))
                                                .fit_to_exact_size(egui::vec2(12.0, 12.0))
                                                .tint(base),
                                            egui::RichText::new("Merge Clips").color(base),
                                        ),
                                    )
                                    .clicked()
                                {
                                    self.merge_selected_clips();
                                    ui.close_menu();
                                }
                                if ui
                                    .add(egui::Button::image_and_text(
                                        egui::Image::new(egui::include_image!(
                                            "../../../assets/icons/trash-2.svg"
                                        ))
                                        .fit_to_exact_size(egui::vec2(12.0, 12.0))
                                        .tint(base),
                                        egui::RichText::new("Delete Clip").color(base),
                                    ))
                                    .clicked()
                                {
                                    pending_delete = Some(clip.id);
                                    ui.close_menu();
                                }
                            });
                        }
                    }
                    ArrangerRow::Automation { track_index, lane_index } => {
                        let track = &self.tracks[track_index];
                        let Some(lane) = track.automation_lanes.get(lane_index) else {
                            continue;
                        };
                        let is_active = self.automation_active == Some((track_index, lane_index));
                        let row_color = if is_active {
                            egui::Color32::from_rgb(10, 12, 18)
                        } else {
                            egui::Color32::from_rgb(6, 8, 12)
                        };
                        clip_painter.rect_filled(row_rect, 0.0, row_color);
                        clip_painter.rect_stroke(
                            row_rect,
                            0.0,
                            egui::Stroke::new(1.0, egui::Color32::from_rgb(0, 0, 0)),
                        );
                        let target_key = match lane.target {
                            AutomationTarget::Instrument => "inst".to_string(),
                            AutomationTarget::Effect(fx_index) => format!("fx_{fx_index}"),
                        };
                        let lane_id = egui::Id::new(format!(
                            "automation_lane_row_{}_{}_{}",
                            track_index, target_key, lane.param_id
                        ));
                        let lane_resp = ui.interact(row_click_rect, lane_id, egui::Sense::click_and_drag());
                        let mut queue_lane_edit = |pos: egui::Pos2| {
                            let beat = ((pos.x - row_left) / beat_width).max(0.0);
                            let value = (1.0 - (pos.y - row_rect.top()) / row_rect.height())
                                .clamp(0.0, 1.0);
                            pending_lane_edit.push((track_index, lane_index, beat, value));
                        };
                        if lane_resp.clicked() {
                            self.automation_active = Some((track_index, lane_index));
                            if let Some(pos) = lane_resp.interact_pointer_pos() {
                                queue_lane_edit(pos);
                            }
                        }
                        if lane_resp.dragged() {
                            self.automation_active = Some((track_index, lane_index));
                            if let Some(pos) = lane_resp.interact_pointer_pos() {
                                queue_lane_edit(pos);
                            }
                        }
                        if !lane.points.is_empty() {
                            let mut points = Vec::new();
                            for point in &lane.points {
                                let x = row_left + point.beat * beat_width;
                                if x < row_rect.left() - 2.0 || x > row_rect.right() + 2.0 {
                                    continue;
                                }
                                let y = row_rect.bottom() - point.value * row_rect.height();
                                points.push(egui::pos2(x, y));
                            }
                            if points.len() >= 2 {
                                clip_painter.add(egui::Shape::line(
                                    points,
                                    egui::Stroke::new(1.2, egui::Color32::from_rgb(180, 200, 255)),
                                ));
                            } else if points.len() == 1 {
                                clip_painter.circle_filled(
                                    points[0],
                                    2.5,
                                    egui::Color32::from_rgb(200, 220, 255),
                                );
                            }
                        }
                        Self::outlined_text(
                            &shelf_painter,
                            egui::pos2(label_rect.left() + 18.0, label_rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            &format!("• {}", lane.name),
                            egui::FontId::proportional(BASE_UI_FONT_SIZE),
                            egui::Color32::from_rgb(140, 160, 200),
                        );
                    }
                }
            }

            let mut marquee_rect: Option<egui::Rect> = None;
            let mut draw_rect: Option<egui::Rect> = None;
            let mut slice_preview_rect: Option<egui::Rect> = None;
            if let Some(pos) = response.interact_pointer_pos() {
                let in_grid = grid_clip.contains(pos);
                let select_mode = self.arranger_tool == ArrangerTool::Select || box_select_active;
                if select_mode && in_grid && !over_clip {
                    if response.drag_started() {
                        self.arranger_select_start = Some(pos);
                        self.arranger_select_add = ctx.input(|i| i.modifiers.shift || i.modifiers.ctrl);
                    }
                }
                if self.arranger_tool == ArrangerTool::Draw && in_grid && !over_clip {
                    if response.drag_started() {
                        let target_track = track_for_pos(pos)
                            .unwrap_or(0)
                            .min(self.tracks.len().saturating_sub(1));
                        let start_beats = ((pos.x - row_left) / beat_width).max(0.0);
                        self.arranger_draw = Some(ArrangerDrawState {
                            track_index: target_track,
                            start_beats,
                            start_pos: pos,
                        });
                    }
                }
                if self.arranger_tool == ArrangerTool::Slice && in_grid {
                    let free_snap = ctx.input(|i| i.modifiers.shift);
                    let beat = self.arranger_slice_beat((pos.x - row_left) / beat_width, free_snap);
                    if response.drag_started() {
                        if let Some(track_index) = track_for_pos(pos) {
                            self.arranger_slice_drag = Some(ArrangerSliceDragState {
                                beat,
                                start_track: track_index,
                                end_track: track_index,
                                free_snap,
                            });
                        }
                    } else if response.clicked() {
                        if let Some(track_index) = track_for_pos(pos) {
                            self.push_undo_state();
                            let sliced = self.slice_tracks_at_beat(track_index, track_index, beat);
                            if sliced.is_empty() {
                                self.undo_stack.pop();
                            } else {
                                self.update_performance_flow_links_for_tracks(&[track_index]);
                                if let Some((_, _, new_clip_id)) = sliced.last().copied() {
                                    self.selected_clips.clear();
                                    self.selected_clips.insert(new_clip_id);
                                    self.selected_clip = Some(new_clip_id);
                                    self.performance_selected_clip = Some(new_clip_id);
                                }
                                self.status = format!("Sliced {} clip(s) at {:.2} beats", sliced.len(), beat);
                            }
                        }
                    }
                }
            }

            if response.clicked()
                && ctx.input(|i| i.modifiers.shift)
                && self.arranger_tool != ArrangerTool::Draw
                && self.arranger_tool != ArrangerTool::Slice
                && !over_clip
            {
                if let (Some(clip_id), Some(pos)) = (self.selected_clip, response.interact_pointer_pos()) {
                    if let Some(target_track) = track_for_pos(pos) {
                        let mut source_clip = None;
                        let mut source_track = None;
                        for (track_index, track) in self.tracks.iter().enumerate() {
                            if let Some(clip) = track.clips.iter().find(|c| c.id == clip_id) {
                                source_clip = Some(clip.clone());
                                source_track = Some(track_index);
                                break;
                            }
                        }
                        if let (Some(clip), Some(source_track)) = (source_clip, source_track) {
                            let source_start = clip.start_beats;
                            let source_length = clip.length_beats;
                            let start_beats = ((pos.x - row_left) / beat_width).max(0.0);
                            let snap = arranger_snap;
                            let snapped_start = if snap > 0.0 {
                                let snap = snap.max(0.25);
                                (start_beats / snap).round() * snap
                            } else {
                                start_beats
                            };
                            let delta = snapped_start - source_start;
                            pending_stamp_copy = Some((
                                clip,
                                source_track,
                                target_track,
                                delta,
                                source_start,
                                source_length,
                            ));
                        }
                    }
                }
            }

            if let Some(start) = self.arranger_select_start {
                if response.dragged() {
                    if let Some(pos) = response.interact_pointer_pos() {
                        marquee_rect = Some(egui::Rect::from_two_pos(start, pos));
                    }
                }
                if response.drag_stopped() {
                    if let Some(end) = response.interact_pointer_pos() {
                        let select_rect = egui::Rect::from_two_pos(start, end);
                        let mut hits: Vec<(usize, usize)> = Vec::new();
                        for (track_index, track) in self.tracks.iter().enumerate() {
                            let row_index = track_row_indices.get(track_index).copied().unwrap_or(track_index);
                            let y = rect.top() + row_top_offset + row_index as f32 * row_height;
                            let row_rect = egui::Rect::from_min_max(
                                egui::pos2(rect.left() + lane_label_w + 16.0, y),
                                egui::pos2(rect.right() - 8.0, y + row_height),
                            );
                            for clip in &track.clips {
                                let clip_x = row_left + clip.start_beats * beat_width;
                                let clip_w = (clip.length_beats * beat_width).max(1.0);
                                let clip_left = clip_x.max(row_rect.left());
                                let clip_right = (clip_x + clip_w).min(row_rect.right());
                                if clip_right <= clip_left {
                                    continue;
                                }
                                let clip_rect = egui::Rect::from_min_max(
                                    egui::pos2(clip_left, row_rect.top()),
                                    egui::pos2(clip_right, row_rect.bottom()),
                                );
                                if select_rect.intersects(clip_rect) {
                                    hits.push((clip.id, track_index));
                                }
                            }
                        }
                        if !hits.is_empty() {
                            pending_multi_select = Some(hits);
                            switch_to_move = true;
                        }
                    }
                    self.arranger_select_start = None;
                }
            }

            if response.dragged() {
                if self.arranger_tool == ArrangerTool::Slice {
                    if let Some(pos) = response.interact_pointer_pos() {
                        let free_snap = ctx.input(|i| i.modifiers.shift)
                            || self
                                .arranger_slice_drag
                                .as_ref()
                                .map(|slice| slice.free_snap)
                                .unwrap_or(false);
                        let beat = self.arranger_slice_beat((pos.x - row_left) / beat_width, free_snap);
                        if let Some(slice) = self.arranger_slice_drag.as_mut() {
                            slice.beat = beat;
                            if ctx.input(|i| i.modifiers.ctrl) {
                                if let Some(track_index) = track_for_pos(pos) {
                                    slice.end_track = track_index;
                                }
                            } else {
                                slice.end_track = slice.start_track;
                            }
                        }
                    }
                }
                if let Some(draw) = self.arranger_draw.as_ref() {
                    if let Some(pos) = response.interact_pointer_pos() {
                        let end_beats = ((pos.x - row_left) / beat_width).max(0.0);
                        let start_beats = draw.start_beats;
                        let snap = arranger_snap;
                        let (snapped_start, snapped_end) = if snap > 0.0 {
                            let snap = snap.max(0.25);
                            (
                                (start_beats / snap).round() * snap,
                                (end_beats / snap).round() * snap,
                            )
                        } else {
                            (start_beats, end_beats)
                        };
                        let left = row_left + snapped_start.min(snapped_end) * beat_width;
                        let right = row_left + snapped_start.max(snapped_end) * beat_width;
                        let row_index = track_row_indices.get(draw.track_index).copied().unwrap_or(draw.track_index);
                        let y = rect.top() + row_top_offset + row_index as f32 * row_height;
                        draw_rect = Some(egui::Rect::from_min_max(
                            egui::pos2(left, y),
                            egui::pos2(right, y + row_height),
                        ));
                    }
                }
            }
            if self.arranger_tool == ArrangerTool::Slice {
                let preview = if let Some(slice) = self.arranger_slice_drag {
                    Some(slice)
                } else if let Some(pos) = response.interact_pointer_pos() {
                    if grid_clip.contains(pos) {
                        track_for_pos(pos).map(|track_index| ArrangerSliceDragState {
                            beat: self.arranger_slice_beat(
                                (pos.x - row_left) / beat_width,
                                ctx.input(|i| i.modifiers.shift),
                            ),
                            start_track: track_index,
                            end_track: track_index,
                            free_snap: ctx.input(|i| i.modifiers.shift),
                        })
                    } else {
                        None
                    }
                } else {
                    None
                };
                if let Some(slice) = preview {
                    let track_min = slice.start_track.min(slice.end_track);
                    let track_max = slice.start_track.max(slice.end_track);
                    let top_row = track_row_indices.get(track_min).copied().unwrap_or(track_min);
                    let bottom_row = track_row_indices.get(track_max).copied().unwrap_or(track_max);
                    let top = rect.top() + row_top_offset + top_row as f32 * row_height;
                    let bottom = rect.top() + row_top_offset + (bottom_row + 1) as f32 * row_height;
                    let x = row_left + slice.beat * beat_width;
                    slice_preview_rect = Some(egui::Rect::from_min_max(
                        egui::pos2(x - 1.5, top.max(timeline_bottom)),
                        egui::pos2(x + 1.5, bottom),
                    ));
                }
            }
            if response.drag_stopped() {
                if self.arranger_tool == ArrangerTool::Slice {
                    if let Some(mut slice) = self.arranger_slice_drag.take() {
                        if let Some(pos) = response.interact_pointer_pos() {
                            let free_snap = ctx.input(|i| i.modifiers.shift) || slice.free_snap;
                            slice.beat = self.arranger_slice_beat((pos.x - row_left) / beat_width, free_snap);
                            if ctx.input(|i| i.modifiers.ctrl) {
                                if let Some(track_index) = track_for_pos(pos) {
                                    slice.end_track = track_index;
                                }
                            }
                        }
                        self.push_undo_state();
                        let sliced = self.slice_tracks_at_beat(slice.start_track, slice.end_track, slice.beat);
                        if sliced.is_empty() {
                            self.undo_stack.pop();
                        } else {
                            let affected_tracks: Vec<usize> = sliced.iter().map(|(track_index, _, _)| *track_index).collect();
                            self.update_performance_flow_links_for_tracks(&affected_tracks);
                            if let Some((_, _, new_clip_id)) = sliced.last().copied() {
                                self.selected_clips.clear();
                                self.selected_clips.insert(new_clip_id);
                                self.selected_clip = Some(new_clip_id);
                                self.performance_selected_clip = Some(new_clip_id);
                            }
                            self.status = format!(
                                "Sliced {} clip(s) at {:.2} beats across {} lane(s)",
                                sliced.len(),
                                slice.beat,
                                slice.start_track.max(slice.end_track) - slice.start_track.min(slice.end_track) + 1,
                            );
                        }
                    }
                }
                if let Some(draw) = self.arranger_draw.take() {
                    if let Some(pos) = response.interact_pointer_pos() {
                        let end_beats = ((pos.x - row_left) / beat_width).max(0.0);
                        let snap = arranger_snap;
                        let min_len = 0.25;
                        let (mut start, mut end) = if snap > 0.0 {
                            let snap = snap.max(0.25);
                            (
                                (draw.start_beats / snap).round() * snap,
                                (end_beats / snap).round() * snap,
                            )
                        } else {
                            (draw.start_beats, end_beats)
                        };
                        if (end - start).abs() < min_len {
                            end = start + min_len;
                        }
                        if end < start {
                            std::mem::swap(&mut start, &mut end);
                        }
                        let track_index = draw.track_index;
                        let clip_id = self.next_clip_id();
                        self.push_undo_state();
                        if let Some(track) = self.tracks.get_mut(track_index) {
                            track.clips.push(Clip {
                                id: clip_id,
                                track: track_index,
                                start_beats: start,
                                length_beats: (end - start).max(min_len),
                                is_midi: true,
                                midi_notes: Vec::new(),
                                midi_source_beats: Some((end - start).max(min_len)),
                                link_id: None,
                                name: "MIDI Clip".to_string(),
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
                            self.selected_track = Some(track_index);
                            self.selected_clip = Some(clip_id);
                        }
                    }
                }
            }

            if let Some(rect) = marquee_rect {
                painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.2, egui::Color32::from_rgb(120, 170, 255)));
                painter.rect_filled(rect, 0.0, egui::Color32::from_rgba_premultiplied(80, 120, 200, 40));
            }
            if let Some(rect) = draw_rect {
                painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.2, egui::Color32::from_rgb(120, 220, 160)));
                painter.rect_filled(rect, 0.0, egui::Color32::from_rgba_premultiplied(60, 140, 90, 40));
            }
            if let Some(rect) = slice_preview_rect {
                painter.rect_filled(rect, 1.5, egui::Color32::from_rgba_premultiplied(255, 210, 92, 160));
                painter.rect_stroke(
                    rect.expand2(egui::vec2(1.0, 0.0)),
                    1.5,
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 230, 150)),
                );
            }

            if playhead_x >= row_left && playhead_x <= rect.right() - 8.0 {
                painter.line_segment(
                    [
                        egui::pos2(playhead_x, rect.top() + 4.0),
                        egui::pos2(playhead_x, rect.bottom() - 4.0),
                    ],
                    egui::Stroke::new(1.4, egui::Color32::from_rgb(255, 86, 70)),
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
            // Overlay timeline bar and grid/loop/playhead lines above clips.
            let timeline_overlay_rect = header_rect;
            painter.rect_filled(timeline_overlay_rect, 0.0, egui::Color32::from_rgb(0, 0, 0));
            let overlay_painter = painter.with_clip_rect(timeline_clip);
            let mut overlay_x = row_left;
            let mut overlay_step_index = 0i32;
            let overlay_major_div = if draw_step >= major_step {
                1
            } else {
                (major_step / draw_step).round() as i32
            };
            while overlay_x <= rect.right() - 8.0 {
                let major = overlay_major_div > 0 && overlay_step_index % overlay_major_div == 0;
                let line_x = overlay_x.round() + 0.5;
                let line_width = if major { 2.0 } else { 1.0 };
                let color = if major {
                    egui::Color32::from_rgba_premultiplied(48, 52, 60, 170)
                } else {
                    egui::Color32::from_rgba_premultiplied(32, 36, 44, 120)
                };
                if major {
                    let band_rect = egui::Rect::from_min_max(
                        egui::pos2(overlay_x, timeline_overlay_rect.top()),
                        egui::pos2(
                            (overlay_x + beat_width * band_step).min(timeline_overlay_rect.right()),
                            timeline_overlay_rect.bottom(),
                        ),
                    );
                    let band_index = if overlay_major_div > 0 {
                        overlay_step_index / overlay_major_div
                    } else {
                        0
                    };
                    let shade = if band_index % 2 == 0 {
                        egui::Color32::from_rgb(8, 8, 8)
                    } else {
                        egui::Color32::from_rgb(0, 0, 0)
                    };
                    overlay_painter.rect_filled(band_rect, 0.0, shade);
                }
                grid_painter.line_segment(
                    [egui::pos2(line_x, grid_top), egui::pos2(line_x, grid_bottom)],
                    egui::Stroke::new(line_width, color),
                );
                if major {
                    let bar = ((overlay_step_index as f32 * draw_step) / 4.0).floor() as i32 + 1;
                    Self::outlined_text(
                        &overlay_painter,
                        egui::pos2(overlay_x + 4.0, timeline_overlay_rect.top() + 2.0),
                        egui::Align2::LEFT_TOP,
                        &format!("{bar}"),
                        egui::FontId::proportional(BASE_UI_FONT_SIZE),
                        egui::Color32::from_gray(200),
                    );
                }
                overlay_step_index += 1;
                overlay_x += beat_width * draw_step;
            }
            painter.line_segment(
                [
                    egui::pos2(timeline_overlay_rect.left(), timeline_overlay_rect.bottom()),
                    egui::pos2(timeline_overlay_rect.right(), timeline_overlay_rect.bottom()),
                ],
                egui::Stroke::new(1.2, egui::Color32::from_rgb(28, 30, 34)),
            );
            if let (Some(start), Some(end)) = (self.loop_start_beats, self.loop_end_beats) {
                if end > start {
                    let loop_x1 = row_left + start * beat_width;
                    let loop_x2 = row_left + end * beat_width;
                    painter.line_segment(
                        [egui::pos2(loop_x1, grid_top), egui::pos2(loop_x1, grid_bottom)],
                        egui::Stroke::new(1.4, egui::Color32::from_rgb(150, 190, 255)),
                    );
                    painter.line_segment(
                        [egui::pos2(loop_x2, grid_top), egui::pos2(loop_x2, grid_bottom)],
                        egui::Stroke::new(1.4, egui::Color32::from_rgb(150, 190, 255)),
                    );
                }
            }
            if playhead_x >= row_left && playhead_x <= rect.right() - 8.0 {
                painter.line_segment(
                    [
                        egui::pos2(playhead_x, rect.top() + 4.0),
                        egui::pos2(playhead_x, rect.bottom() - 4.0),
                    ],
                    egui::Stroke::new(1.6, egui::Color32::from_rgb(255, 96, 80)),
                );
            }

            for (track_index, track) in self.tracks.iter().enumerate() {
                let row_index = track_row_indices.get(track_index).copied().unwrap_or(track_index);
                let y = rect.top() + row_top_offset + row_index as f32 * row_height;
                let label_rect = egui::Rect::from_min_max(
                    egui::pos2(rect.left() + 8.0, y),
                    egui::pos2(rect.left() + lane_label_w, y + row_height),
                );
                let tile_rect = label_rect;
                let is_selected = self.selected_track == Some(track_index);
                let base = self.track_color(track_index);
                let has_automation = !track.automation_lanes.is_empty();
                let tile_color = if is_selected {
                    Self::tint(base, 0.35)
                } else {
                    egui::Color32::from_rgba_premultiplied(base.r(), base.g(), base.b(), 90)
                };
                shelf_painter.rect_filled(tile_rect, 0.0, tile_color);
                let expanded = self.automation_rows_expanded.contains(&track_index);
                let mut toggle_response = None;
                let mut toggle_rect_opt: Option<egui::Rect> = None;
                if has_automation {
                    let toggle_rect = egui::Rect::from_min_size(
                        egui::pos2(tile_rect.left() + 4.0, tile_rect.center().y - 6.0),
                        egui::vec2(12.0, 12.0),
                    );
                    toggle_rect_opt = Some(toggle_rect);
                    let toggle_icon = if expanded {
                        egui::include_image!("../../../assets/icons/chevron-down.svg")
                    } else {
                        egui::include_image!("../../../assets/icons/chevron-right.svg")
                    };
                    let response = ui.put(
                        toggle_rect,
                        egui::ImageButton::new(
                            egui::Image::new(toggle_icon).fit_to_exact_size(toggle_rect.size()),
                        )
                        .frame(false),
                    );
                    if response.clicked() {
                        if expanded {
                            self.automation_rows_expanded.remove(&track_index);
                        } else {
                            self.automation_rows_expanded.insert(track_index);
                        }
                    }
                    toggle_response = Some(response);
                }
                if let (Some(rect), Some(resp)) = (toggle_rect_opt, toggle_response.as_ref()) {
                    if resp.hovered() {
                        shelf_painter.rect_filled(
                            rect,
                            2.0,
                            egui::Color32::from_rgba_premultiplied(0, 0, 0, 90),
                        );
                    }
                }
                let label_click_rect = if has_automation {
                    egui::Rect::from_min_max(
                        egui::pos2(tile_rect.left() + 20.0, tile_rect.top()),
                        tile_rect.max,
                    )
                } else {
                    tile_rect
                };
                if label_click_rect.top() >= grid_top {
                    let label_id = egui::Id::new(format!("arranger_tracklist_{}", track_index));
                    let label_response =
                        ui.interact(label_click_rect, label_id, egui::Sense::click_and_drag());
                    if label_response.clicked()
                        && !toggle_response.as_ref().map_or(false, |resp| resp.clicked())
                    {
                        pending_track_select = Some(track_index);
                    }
                    if label_response.drag_started() {
                        self.track_drag = Some(TrackDragState { source_index: track_index });
                    }
                    if label_response.drag_stopped() {
                        if let Some(drag) = self.track_drag.take() {
                            let pos = label_response
                                .interact_pointer_pos()
                                .or_else(|| response.interact_pointer_pos())
                                .or_else(|| ctx.input(|i| i.pointer.interact_pos()));
                            if let Some(pos) = pos {
                                if let Some(target_track) = track_for_pos(pos) {
                                    pending_track_move = Some((drag.source_index, target_track));
                                }
                            }
                        }
                    }
                    label_response.context_menu(|ui| {
                        if ui.button("Clone Track Only").clicked() {
                            pending_track_action = Some(TrackContextAction::CloneOnly(track_index));
                            ui.close_menu();
                        }
                        if ui.button("Duplicate Track With Clips").clicked() {
                            pending_track_action =
                                Some(TrackContextAction::DuplicateWithClips(track_index));
                            ui.close_menu();
                        }
                        if ui.button("Delete Track").clicked() {
                            pending_track_action = Some(TrackContextAction::Delete(track_index));
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Move Up").clicked() {
                            pending_track_action = Some(TrackContextAction::MoveUp(track_index));
                            ui.close_menu();
                        }
                        if ui.button("Move Down").clicked() {
                            pending_track_action = Some(TrackContextAction::MoveDown(track_index));
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Solo").clicked() {
                            pending_track_action = Some(TrackContextAction::ToggleSolo(track_index));
                            ui.close_menu();
                        }
                        if ui.button("Mute").clicked() {
                            pending_track_action = Some(TrackContextAction::ToggleMute(track_index));
                            ui.close_menu();
                        }
                    });
                }
                let name_rect = egui::Rect::from_min_max(
                    egui::pos2(
                        tile_rect.left() + if has_automation { 22.0 } else { 6.0 },
                        tile_rect.top(),
                    ),
                    egui::pos2(tile_rect.right() - 46.0, tile_rect.bottom()),
                );
                let name_color = if is_selected {
                    egui::Color32::from_rgb(220, 235, 255)
                } else {
                    egui::Color32::from_gray(220)
                };
                Self::outlined_text(
                    &shelf_painter,
                    egui::pos2(name_rect.left(), name_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    &track.name,
                    egui::FontId::proportional(BASE_UI_FONT_SIZE),
                    name_color,
                );
                let meter_rect = egui::Rect::from_center_size(
                    egui::pos2(tile_rect.right() - 24.0, tile_rect.center().y),
                    egui::vec2(36.0, 8.0),
                );
                shelf_painter.rect_filled(meter_rect, 3.0, egui::Color32::from_rgb(16, 20, 24));
                let peak = self
                    .track_audio
                    .get(track_index)
                    .map(|s| f32::from_bits(s.peak_bits.load(Ordering::Relaxed)))
                    .unwrap_or(0.0)
                    .clamp(0.0, 1.0);
                let fill_w = meter_rect.width() * peak;
                if fill_w > 0.0 {
                    let fill_rect = egui::Rect::from_min_size(
                        meter_rect.min,
                        egui::vec2(fill_w, meter_rect.height()),
                    );
                    let color = if peak > 0.9 {
                        egui::Color32::from_rgb(255, 90, 64)
                    } else if peak > 0.7 {
                        egui::Color32::from_rgb(250, 200, 80)
                    } else {
                        egui::Color32::from_rgb(90, 210, 120)
                    };
                    shelf_painter.rect_filled(fill_rect, 3.0, color);
                }
            }

            for (track_index, lane_index, beat, value) in pending_lane_edit {
                if let Some(track) = self.tracks.get_mut(track_index) {
                    if let Some(lane) = track.automation_lanes.get_mut(lane_index) {
                        let mut updated = false;
                        for point in lane.points.iter_mut() {
                            if (point.beat - beat).abs() <= 0.1 {
                                point.beat = beat;
                                point.value = value;
                                updated = true;
                                break;
                            }
                        }
                        if !updated {
                            lane.points.push(AutomationPoint { beat, value });
                        }
                        lane.points.sort_by(|a, b| a.beat.partial_cmp(&b.beat).unwrap_or(std::cmp::Ordering::Equal));
                        if let Some(state) = self.track_audio.get(track_index) {
                            if let Ok(mut lanes) = state.automation_lanes.lock() {
                                *lanes = track.automation_lanes.clone();
                            }
                        }
                    }
                }
            }

            if let Some((mut copy, source_track, target_track, delta, _source_start, _source_len)) =
                pending_stamp_copy
            {
                let source_clip_id = copy.id;
                let link_id = self.ensure_clip_link_id(source_track, source_clip_id);
                let new_id = self.next_clip_id();
                copy.id = new_id;
                copy.track = target_track;
                let base_start = copy.start_beats;
                copy.start_beats = (base_start + delta).max(0.0);
                copy.link_id = link_id;
                self.push_undo_state();
                if let Some(track) = self.tracks.get_mut(target_track) {
                    track.clips.push(copy.clone());
                }
                if copy.is_midi {
                    self.shift_clip_notes_by_delta(new_id, delta);
                }
                if copy.is_midi {
                    self.sync_track_audio_notes(target_track);
                }
                pending_select = Some((new_id, target_track, false));
                switch_to_move = true;
            }

            let has_pending_drag = pending_drag_start.is_some();
            let mut selection_changed = false;
            if let Some(hits) = pending_multi_select {
                if !self.arranger_select_add {
                    self.selected_clips.clear();
                }
                let mut last_clip = None;
                let mut last_track = None;
                for (clip_id, track_index) in hits {
                    self.selected_clips.insert(clip_id);
                    last_clip = Some(clip_id);
                    last_track = Some(track_index);
                }
                self.selected_clip = last_clip;
                self.selected_track = last_track;
                selection_changed = true;
            }
            if let Some((clip_id, track_index, add)) = pending_select {
                if !add {
                    self.selected_clips.clear();
                }
                self.selected_clips.insert(clip_id);
                self.selected_clip = Some(clip_id);
                self.selected_track = Some(track_index);
                selection_changed = true;
            }
            if let Some(drag) = pending_drag_start {
                let clip_id = drag.clip_id;
                let source_track = drag.source_track;
                self.clip_drag = Some(drag);
                self.selected_clip = Some(clip_id);
                self.selected_track = Some(source_track);
                selection_changed = true;
            }
            if pending_select.is_none() && !has_pending_drag {
                if let Some(track_index) = pending_track_select {
                    self.selected_track = Some(track_index);
                    selection_changed = true;
                }
            }
            if selection_changed {
                self.refresh_params_for_selected_track(false);
                self.piano_selected.clear();
            }
            if let Some((track_index, clip_id)) = pending_clip_rename {
                self.begin_rename_clip(track_index, clip_id);
            }
            if switch_to_move {
                self.arranger_tool = ArrangerTool::Move;
            }
            if let Some(clip_id) = pending_delete {
                self.push_undo_state();
                self.remove_clip_and_notes_by_id(clip_id);
                self.selected_clips.remove(&clip_id);
                if self.selected_clip == Some(clip_id) {
                    self.selected_clip = None;
                }
            }

            let mut track_mix_dirty = false;
            if let Some(action) = pending_track_action {
                match action {
                    TrackContextAction::CloneOnly(track_index) => {
                        self.selected_track = Some(track_index);
                        self.clone_selected_track();
                    }
                    TrackContextAction::DuplicateWithClips(track_index) => {
                        self.selected_track = Some(track_index);
                        self.duplicate_selected_track();
                    }
                    TrackContextAction::Delete(track_index) => {
                        self.selected_track = Some(track_index);
                        self.remove_selected_track();
                    }
                    TrackContextAction::MoveUp(track_index) => {
                        if track_index > 0 {
                            pending_track_move = Some((track_index, track_index - 1));
                        }
                    }
                    TrackContextAction::MoveDown(track_index) => {
                        let last = self.tracks.len().saturating_sub(1);
                        if track_index < last {
                            pending_track_move = Some((track_index, track_index + 1));
                        }
                    }
                    TrackContextAction::ToggleSolo(track_index) => {
                        if let Some(track) = self.tracks.get_mut(track_index) {
                            if track.solo {
                                track.solo = false;
                            } else {
                                for (other_index, other) in self.tracks.iter_mut().enumerate() {
                                    other.solo = other_index == track_index;
                                }
                            }
                            track_mix_dirty = true;
                        }
                    }
                    TrackContextAction::ToggleMute(track_index) => {
                        if let Some(track) = self.tracks.get_mut(track_index) {
                            track.muted = !track.muted;
                            track_mix_dirty = true;
                        }
                    }
                }
            }

            if track_mix_dirty {
                self.sync_track_mix();
            }

            if let Some((from, to)) = pending_track_move {
                self.move_track_order(from, to);
            }

            if let Some(mut drag) = self.clip_drag.take() {
                let (pointer_down, pointer_pos, pointer_released) = ctx.input(|i| {
                    (i.pointer.primary_down(), i.pointer.interact_pos(), i.pointer.any_released())
                });
                if pointer_down {
                    if let Some(pos) = pointer_pos {
                        if !drag.undo_pushed {
                            self.push_undo_state();
                            drag.undo_pushed = true;
                        }
                        let min_len = 0.25;
                        let target_track = track_for_pos(pos)
                            .unwrap_or(0)
                            .min(self.tracks.len().saturating_sub(1));
                        let cursor_beats = (pos.x - row_left) / beat_width;
                        let snap = arranger_snap.max(0.0);
                        let snap_value = |value: f32| {
                            if snap > 0.0 {
                                let snap = snap.max(0.25);
                                (value / snap).round() * snap
                            } else {
                                value
                            }
                        };

                        match drag.kind {
                            ClipDragKind::Move => {
                                let raw_start = (cursor_beats - drag.offset_beats).max(0.0);
                                let new_start = snap_value(raw_start).max(0.0);
                                let delta = new_start - drag.start_beats;
                                if let Some(group) = drag.group.as_mut() {
                                    for item in group.iter_mut() {
                                        let old_track = item.source_track;
                                        let new_track = item.source_track;
                                        let new_item_start = (item.start_beats + delta).max(0.0);
                                        if item.is_midi
                                            && (delta.abs() > f32::EPSILON
                                                || new_track != item.source_track)
                                        {
                                            self.shift_clip_notes_by_delta(item.clip_id, delta);
                                        }
                                        self.move_clip_by_id(item.clip_id, new_track, new_item_start);
                                        if item.is_midi {
                                            self.sync_track_audio_notes(old_track);
                                            if new_track != old_track {
                                                self.sync_track_audio_notes(new_track);
                                            }
                                        }
                                        item.start_beats = new_item_start;
                                        item.source_track = new_track;
                                    }
                                    drag.source_track = target_track;
                                    drag.start_beats = new_start;
                                } else {
                                    let old_track = drag.source_track;
                                    let is_midi = self
                                        .tracks
                                        .get(drag.source_track)
                                        .and_then(|track| track.clips.iter().find(|c| c.id == drag.clip_id))
                                        .map(|clip| clip.is_midi)
                                        .unwrap_or(false);
                                    if is_midi
                                        && (delta.abs() > f32::EPSILON
                                            || target_track != drag.source_track)
                                    {
                                        self.shift_clip_notes_by_delta(drag.clip_id, delta);
                                    }
                                    self.move_clip_by_id(drag.clip_id, target_track, new_start);
                                    if is_midi {
                                        self.sync_track_audio_notes(old_track);
                                        if target_track != old_track {
                                            self.sync_track_audio_notes(target_track);
                                        }
                                    }
                                    drag.source_track = target_track;
                                    drag.start_beats = new_start;
                                }
                            }
                            ClipDragKind::ResizeStart => {
                                let end = drag.start_beats + drag.length_beats;
                                let raw_start = cursor_beats.min(end - min_len).max(0.0);
                                let new_start = snap_value(raw_start)
                                    .min(end - min_len)
                                    .max(0.0);
                                let new_len = (end - new_start).max(min_len);
                                self.update_clip_by_id(drag.clip_id, |clip| {
                                    clip.start_beats = new_start;
                                    clip.length_beats = new_len;
                                });
                            }
                            ClipDragKind::ResizeEnd => {
                                let raw_end = cursor_beats.max(drag.start_beats + min_len);
                                let snapped_end = snap_value(raw_end).max(drag.start_beats + min_len);
                                let new_len = (snapped_end - drag.start_beats).max(min_len);
                                self.update_clip_by_id(drag.clip_id, |clip| {
                                    if clip.is_midi && clip.midi_source_beats.is_none() {
                                        clip.midi_source_beats = Some(clip.length_beats.max(min_len));
                                    }
                                    clip.length_beats = new_len;
                                });
                            }
                            ClipDragKind::TrimStart => {
                                let end = drag.start_beats + drag.length_beats;
                                let raw_start = cursor_beats.min(end - min_len);
                                let new_start = snap_value(raw_start).min(end - min_len);
                                let delta = (new_start - drag.start_beats).max(0.0);
                                let new_len = (drag.length_beats - delta).max(min_len);
                                self.update_clip_by_id(drag.clip_id, |clip| {
                                    clip.start_beats = new_start;
                                    clip.length_beats = new_len;
                                    let mut offset = (drag.audio_offset_beats + delta).max(0.0);
                                    if let Some(source) = clip.audio_source_beats {
                                        if source > 0.0 {
                                            offset %= source;
                                        }
                                    }
                                    clip.audio_offset_beats = offset;
                                });
                            }
                            ClipDragKind::TrimEnd => {
                                let raw_end = cursor_beats.max(drag.start_beats + min_len);
                                let snapped_end = snap_value(raw_end).max(drag.start_beats + min_len);
                                let new_len = (snapped_end - drag.start_beats).max(min_len);
                                self.update_clip_by_id(drag.clip_id, |clip| {
                                    clip.length_beats = new_len;
                                    if let Some(source) = clip.audio_source_beats {
                                        if source > 0.0 {
                                            clip.audio_offset_beats %= source;
                                        }
                                    }
                                });
                            }
                        }
                    }
                }
                if pointer_released || !pointer_down {
                    if matches!(
                        drag.kind,
                        ClipDragKind::ResizeStart
                            | ClipDragKind::ResizeEnd
                            | ClipDragKind::TrimStart
                            | ClipDragKind::TrimEnd
                    ) {
                        if let Some(track) = self.tracks.get(drag.source_track) {
                            if let Some(clip) = track.clips.iter().find(|c| c.id == drag.clip_id) {
                                if clip.is_midi {
                                    self.crop_clip_notes_to_clip_range(
                                        clip.id,
                                        clip.start_beats,
                                        clip.length_beats,
                                    );
                                }
                            }
                        }
                    }
                    if drag.copy_mode {
                        let is_midi = self
                            .tracks
                            .get(drag.source_track)
                            .and_then(|track| track.clips.iter().find(|c| c.id == drag.clip_id))
                            .map(|clip| clip.is_midi)
                            .unwrap_or(false);
                        let _ = is_midi;
                    }
                    self.clip_drag = None;
                } else {
                    self.clip_drag = Some(drag);
                }
            }
        });
    }
}
