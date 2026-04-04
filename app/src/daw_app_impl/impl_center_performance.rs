impl DawApp {
    pub(crate) fn center_performance(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            let panel_fill = if self.wallpaper_enabled() {
                egui::Color32::from_rgba_premultiplied(9, 11, 14, 210)
            } else {
                egui::Color32::from_rgb(9, 11, 14)
            };
            let strip_fill = egui::Color32::from_rgba_premultiplied(18, 21, 26, 244);
            let panel_edge = egui::Color32::from_rgba_premultiplied(88, 98, 116, 180);
            let header_fill = egui::Color32::from_rgba_premultiplied(16, 19, 24, 252);
            let inspector_fill = egui::Color32::from_rgba_premultiplied(15, 18, 23, 244);
            let inspector_card_fill = egui::Color32::from_rgba_premultiplied(24, 28, 34, 232);
            let inspector_card_fill_alt = egui::Color32::from_rgba_premultiplied(19, 23, 29, 236);
            let inspector_soft_text = egui::Color32::from_rgb(156, 166, 180);
            let inspector_muted_text = egui::Color32::from_rgb(118, 128, 142);
            let inspector_line = egui::Color32::from_rgba_premultiplied(108, 120, 140, 96);
            let grid_major = egui::Color32::from_rgba_premultiplied(76, 86, 102, 44);
            let grid_minor = egui::Color32::from_rgba_premultiplied(48, 54, 66, 22);
            let shadow_color = egui::Color32::from_rgba_premultiplied(0, 0, 0, 118);
            let roygbiv = [
                egui::Color32::from_rgb(232, 88, 88),
                egui::Color32::from_rgb(245, 145, 84),
                egui::Color32::from_rgb(244, 205, 92),
                egui::Color32::from_rgb(118, 214, 122),
                egui::Color32::from_rgb(74, 192, 216),
                egui::Color32::from_rgb(96, 132, 234),
                egui::Color32::from_rgb(176, 118, 232),
            ];
            let selected_clip_id = self.performance_selected_clip.or(self.selected_clip);
            let mut click_action: Option<(usize, usize)> = None;
            let mut edit_action: Option<(usize, usize)> = None;
            let mut scene_launch_action: Option<f32> = None;
            let mut stop_track_action: Option<usize> = None;
            let mut performance_status: Option<String> = None;
            let mut trigger_action: Option<(usize, usize, PerformanceClipSettings, String)> = None;
            let runtime_snapshot = self.engine.performance_runtime.lock().clone();
            let current_transport_samples = self.engine.transport_samples.load(Ordering::Relaxed);
            let clock_beat = self.current_transport_beat();
            let animation_t = ctx.input(|i| i.time) as f32;
            let pulse_fast = 0.45 + 0.55 * ((animation_t * 7.0).sin() * 0.5 + 0.5);
            let pulse_slow = 0.55 + 0.45 * ((animation_t * 2.6).sin() * 0.5 + 0.5);
            let track_strip_peaks: Vec<f32> = self
                .engine
                .track_audio
                .iter()
                .map(|state| f32::from_bits(state.peak_bits.load(Ordering::Relaxed)).clamp(0.0, 1.0))
                .collect();
            if self.audio_running || runtime_snapshot.iter().any(|slot| slot.is_some()) {
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
            }

            let shadow_text = |painter: &egui::Painter,
                               pos: egui::Pos2,
                               align: egui::Align2,
                               text: &str,
                               font: egui::FontId,
                               color: egui::Color32| {
                painter.text(
                    pos + egui::vec2(0.0, 1.0),
                    align,
                    text,
                    font.clone(),
                    shadow_color,
                );
                painter.text(pos, align, text, font, color);
            };

            let track_names: Vec<String> = self
                .tracks
                .iter()
                .enumerate()
                .map(|(i, _)| format!("T{}", i + 1))
                .collect();
            let track_labels: Vec<String> = self
                .tracks
                .iter()
                .map(|t| t.name.clone())
                .collect();
            let track_mutes: Vec<bool> = self.tracks.iter().map(|t| t.muted).collect();
            let track_solos: Vec<bool> = self.tracks.iter().map(|t| t.solo).collect();

            let mut scenes: Vec<f32> = self
                .tracks
                .iter()
                .flat_map(|t| t.clips.iter().map(|c| c.start_beats.max(0.0)))
                .collect();
            scenes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            scenes.dedup_by(|a, b| performance_scene_matches(*a, *b));
            let quantize_label = if self.performance_launch_quantize_beats <= f32::EPSILON {
                "Off"
            } else if (self.performance_launch_quantize_beats - 0.25).abs() <= f32::EPSILON {
                "1/16"
            } else if (self.performance_launch_quantize_beats - 0.5).abs() <= f32::EPSILON {
                "1/8"
            } else if (self.performance_launch_quantize_beats - 1.0).abs() <= f32::EPSILON {
                "1 Beat"
            } else if (self.performance_launch_quantize_beats - 2.0).abs() <= f32::EPSILON {
                "1/2 Bar"
            } else if (self.performance_launch_quantize_beats - 4.0).abs() <= f32::EPSILON {
                "1 Bar"
            } else {
                "Custom"
            };

            let max_rect = ui.max_rect();
            let painter = ui.painter().clone();
            painter.rect_filled(max_rect, 0.0, panel_fill);
            for band in 0..18 {
                let y = max_rect.top() + 26.0 + band as f32 * 26.0;
                let color = if band % 4 == 0 { grid_major } else { grid_minor };
                painter.line_segment(
                    [egui::pos2(max_rect.left(), y), egui::pos2(max_rect.right(), y)],
                    egui::Stroke::new(if band % 4 == 0 { 1.0 } else { 0.5 }, color),
                );
            }
            let col_step = 92.0;
            let mut x = max_rect.left() + 40.0;
            let mut col_index = 0usize;
            while x < max_rect.right() {
                let color = if col_index.is_multiple_of(4) { grid_major } else { grid_minor };
                painter.line_segment(
                    [egui::pos2(x, max_rect.top() + 18.0), egui::pos2(x, max_rect.bottom())],
                    egui::Stroke::new(if col_index.is_multiple_of(4) { 1.0 } else { 0.5 }, color),
                );
                x += col_step;
                col_index += 1;
            }
            let top_bar = egui::Rect::from_min_max(
                egui::pos2(max_rect.left(), max_rect.top()),
                egui::pos2(max_rect.right(), max_rect.top() + 64.0),
            );
            painter.rect_filled(top_bar, 0.0, header_fill);
            painter.line_segment(
                [egui::pos2(top_bar.left(), top_bar.bottom() + 0.5), egui::pos2(top_bar.right(), top_bar.bottom() + 0.5)],
                egui::Stroke::new(1.0, panel_edge),
            );
            let accent_width = (top_bar.width() / roygbiv.len() as f32).ceil();
            for (idx, color) in roygbiv.iter().enumerate() {
                let left = top_bar.left() + idx as f32 * accent_width;
                let right = (left + accent_width + 1.0).min(top_bar.right());
                painter.rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(left, top_bar.bottom() - 3.0),
                        egui::pos2(right, top_bar.bottom()),
                    ),
                    0.0,
                    color.gamma_multiply(0.9),
                );
            }

            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    let title_pos = ui.next_widget_position() + egui::vec2(0.0, 2.0);
                    shadow_text(
                        &painter,
                        title_pos,
                        egui::Align2::LEFT_TOP,
                        "Performance",
                        egui::FontId::proportional(26.0),
                        egui::Color32::from_rgb(236, 240, 245),
                    );
                    ui.add_space(30.0);
                    ui.label(
                        egui::RichText::new("Session matrix in the arranger language")
                            .size(12.0)
                            .color(egui::Color32::from_rgb(154, 166, 182)),
                    );
                });
                ui.add_space(10.0);
                ui.separator();
                if self.audio_running {
                    ui.colored_label(egui::Color32::from_rgb(118, 224, 128), "Clock Running");
                } else {
                    ui.colored_label(egui::Color32::from_rgb(170, 176, 188), "Stopped");
                }
                let queued_count = runtime_snapshot
                    .iter()
                    .filter(|slot| {
                        slot.as_ref()
                            .map(|runtime| runtime.launch_samples > current_transport_samples)
                            .unwrap_or(false)
                    })
                    .count();
                let live_count = runtime_snapshot
                    .iter()
                    .filter(|slot| {
                        slot.as_ref()
                            .map(|runtime| runtime.launch_samples <= current_transport_samples)
                            .unwrap_or(false)
                    })
                    .count();
                ui.separator();
                ui.colored_label(
                    egui::Color32::from_rgb(244, 200, 96),
                    format!("Queued {}", queued_count),
                );
                ui.colored_label(
                    egui::Color32::from_rgb(122, 232, 138),
                    format!("Live {}", live_count),
                );
                if self.render_job.is_some() {
                    ui.colored_label(egui::Color32::from_rgb(242, 191, 94), "Rendering");
                }
            });
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                let song_mode = self.arrangement_playback_enabled();
                if ui.selectable_label(song_mode, "Song Play").clicked() {
                    self.set_arrangement_playback_enabled(true);
                    if !self.audio_running {
                        self.seek_playhead(self.playhead_beats);
                        if let Err(err) = self.start_audio_and_midi_internal(false) {
                            self.status = format!("Play failed: {err}");
                        }
                    } else {
                        self.status = "Song playback enabled".to_string();
                    }
                }
                if ui.selectable_label(!song_mode, "Session Only").clicked() {
                    self.set_arrangement_playback_enabled(false);
                    if !self.audio_running {
                        if let Err(err) = self.start_session_clock() {
                            self.status = format!("Session start failed: {err}");
                        }
                    } else {
                        self.status = "Session-only clock enabled".to_string();
                    }
                }
                ui.separator();
                ui.label("Launch Quantize");
                egui::ComboBox::from_id_source("performance_launch_quantize")
                    .selected_text(quantize_label)
                    .show_ui(ui, |ui| {
                        for (value, label) in [
                            (0.0, "Off"),
                            (0.25, "1/16"),
                            (0.5, "1/8"),
                            (1.0, "1 Beat"),
                            (2.0, "1/2 Bar"),
                            (4.0, "1 Bar"),
                        ] {
                            if ui
                                .selectable_label(
                                    (self.performance_launch_quantize_beats - value).abs() <= f32::EPSILON,
                                    label,
                                )
                                .clicked()
                            {
                                self.performance_launch_quantize_beats = value;
                                self.mark_dirty();
                            }
                        }
                    });
                ui.separator();
                ui.label(format!("Clock Beat: {:.2}", clock_beat));
            });
            let selected_clip_snapshot = selected_clip_id
                .and_then(|clip_id| {
                    self.find_clip_indices_by_id(clip_id).and_then(|(track_index, clip_index)| {
                        self.tracks
                            .get(track_index)
                            .and_then(|track| track.clips.get(clip_index))
                            .cloned()
                            .map(|clip| (clip_id, track_index, clip))
                    })
                });
            egui::Frame::none()
                .fill(inspector_card_fill_alt)
                .stroke(egui::Stroke::new(1.0, inspector_line))
                .rounding(egui::Rounding::same(8.0))
                .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                .show(ui, |ui| {
                    if let Some((clip_id, clip_track_index, clip)) = selected_clip_snapshot {
                        let runtime_state = runtime_snapshot
                            .get(clip_track_index)
                            .and_then(|slot| slot.as_ref())
                            .filter(|runtime| runtime.clip.id == clip_id);
                        let mut settings = self
                            .performance_clip_settings
                            .get(&clip_id)
                            .cloned()
                            .unwrap_or_default();
                        ui.horizontal_wrapped(|ui| {
                            let badge_color = if clip.is_midi {
                                egui::Color32::from_rgb(136, 245, 148)
                            } else {
                                egui::Color32::from_rgb(255, 191, 98)
                            };
                            ui.colored_label(
                                badge_color,
                                if clip.is_midi { "MIDI CLIP" } else { "AUDIO CLIP" },
                            );
                            ui.separator();
                            ui.label(
                                egui::RichText::new(if clip.name.trim().is_empty() {
                                    format!("Clip {}", clip.id)
                                } else {
                                    clip.name.clone()
                                })
                                .strong()
                                .color(egui::Color32::from_rgb(234, 238, 244)),
                            );
                            ui.separator();
                            ui.label(
                                egui::RichText::new(format!("T{}  @ {:.2}b  Len {:.2}b", clip_track_index + 1, clip.start_beats, clip.length_beats))
                                    .color(inspector_soft_text),
                            );
                            if let Some(runtime) = runtime_state {
                                ui.separator();
                                ui.colored_label(
                                    if runtime.launch_samples > current_transport_samples {
                                        egui::Color32::WHITE.gamma_multiply(0.5 + 0.5 * pulse_fast)
                                    } else {
                                        egui::Color32::from_rgb(126, 236, 142)
                                    },
                                    if runtime.launch_samples > current_transport_samples {
                                        "QUEUED"
                                    } else {
                                        "LIVE"
                                    },
                                );
                            }
                        });
                        ui.add_space(6.0);
                        ui.horizontal_wrapped(|ui| {
                            let action_fill = egui::Color32::from_rgba_premultiplied(38, 44, 54, 232);
                            let action_stroke = egui::Stroke::new(1.0, inspector_line.gamma_multiply(1.15));
                            if ui
                                .add(egui::Button::new("Launch").fill(action_fill).stroke(action_stroke))
                                .clicked()
                            {
                                let label = if clip.name.trim().is_empty() {
                                    format!("Clip {}", clip.id)
                                } else {
                                    clip.name.clone()
                                };
                                trigger_action = Some((clip_track_index, clip.id, settings.clone(), label));
                            }
                            if clip.is_midi
                                && ui
                                    .add(egui::Button::new("Piano Roll").fill(action_fill).stroke(action_stroke))
                                    .clicked()
                            {
                                edit_action = Some((clip_id, clip_track_index));
                            }
                            if ui
                                .add(egui::Button::new("Focus Scene").fill(action_fill).stroke(action_stroke))
                                .clicked()
                            {
                                scene_launch_action = Some(clip.start_beats.max(0.0));
                            }
                            ui.separator();
                            ui.label(egui::RichText::new("Trigger").size(11.0).color(inspector_muted_text));
                            let mut trigger_changed = false;
                            egui::ComboBox::from_id_source("performance_top_trigger_mode")
                                .selected_text(match settings.trigger_mode {
                                    PerformanceTriggerMode::Gate => "Gate",
                                    PerformanceTriggerMode::Toggle => "Toggle",
                                    PerformanceTriggerMode::OneShot => "One Shot",
                                    PerformanceTriggerMode::Loop => "Loop",
                                })
                                .show_ui(ui, |ui| {
                                    trigger_changed |= ui.selectable_value(&mut settings.trigger_mode, PerformanceTriggerMode::Gate, "Gate").changed();
                                    trigger_changed |= ui.selectable_value(&mut settings.trigger_mode, PerformanceTriggerMode::Toggle, "Toggle").changed();
                                    trigger_changed |= ui.selectable_value(&mut settings.trigger_mode, PerformanceTriggerMode::OneShot, "One Shot").changed();
                                    trigger_changed |= ui.selectable_value(&mut settings.trigger_mode, PerformanceTriggerMode::Loop, "Loop").changed();
                                });

                            let loop_changed = ui.checkbox(&mut settings.loop_enabled, "Loop").changed();
                            let auto_follow_changed = ui.checkbox(&mut settings.auto_follow, "March").changed();
                            let settings_dirty = trigger_changed || loop_changed || auto_follow_changed;

                            if let Some(next_clip_id) = settings.next_clip_id {
                                if let Some((next_track_index, next_clip_index)) = self.find_clip_indices_by_id(next_clip_id) {
                                    if let Some(next_clip) = self.tracks.get(next_track_index).and_then(|track| track.clips.get(next_clip_index)) {
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "Next T{} {}",
                                                next_track_index + 1,
                                                if next_clip.name.trim().is_empty() {
                                                    format!("Clip {}", next_clip.id)
                                                } else {
                                                    next_clip.name.clone()
                                                },
                                            ))
                                            .size(11.0)
                                            .color(inspector_soft_text),
                                        );
                                    }
                                }
                            }

                            if settings_dirty {
                                self.performance_clip_settings.insert(clip_id, settings.clone());
                                self.mark_dirty();
                            }

                            if loop_changed {
                                if settings.loop_enabled {
                                    self.update_clip_by_id(clip_id, |c| {
                                        if c.is_midi {
                                            if c.midi_source_beats.unwrap_or(0.0) <= 0.0 {
                                                c.midi_source_beats = Some(c.length_beats.max(0.25));
                                            }
                                        } else if c.audio_source_beats.unwrap_or(0.0) <= 0.0 {
                                            c.audio_source_beats = Some(c.length_beats.max(0.25));
                                        }
                                    });
                                } else {
                                    self.update_clip_by_id(clip_id, |c| {
                                        if c.is_midi {
                                            c.midi_source_beats = None;
                                        } else {
                                            c.audio_source_beats = None;
                                        }
                                    });
                                }
                            }

                            if settings.loop_enabled {
                                let mut loop_len = if clip.is_midi {
                                    clip.midi_source_beats.unwrap_or(clip.length_beats.max(0.25))
                                } else {
                                    clip.audio_source_beats.unwrap_or(clip.length_beats.max(0.25))
                                };
                                ui.label(egui::RichText::new("Loop Len").size(11.0).color(inspector_muted_text));
                                if ui
                                    .add(egui::DragValue::new(&mut loop_len).speed(0.25).clamp_range(0.25..=256.0))
                                    .changed()
                                {
                                    let next = loop_len.max(0.25);
                                    self.update_clip_by_id(clip_id, |c| {
                                        if c.is_midi {
                                            c.midi_source_beats = Some(next);
                                        } else {
                                            c.audio_source_beats = Some(next);
                                        }
                                    });
                                    self.mark_dirty();
                                }
                            }
                        });
                    } else {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                egui::RichText::new("Select a clip to edit launch, loop, and march settings.")
                                    .size(11.0)
                                    .color(inspector_soft_text),
                            );
                        });
                    }
                });
            ui.add_space(8.0);

            if track_names.is_empty() || scenes.is_empty() {
                ui.label("No clips available yet. Add clips in Arranger to populate the session grid.");
                return;
            }

            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    egui::Frame::none()
                        .fill(strip_fill)
                        .stroke(egui::Stroke::new(1.0, panel_edge.gamma_multiply(0.75)))
                        .rounding(egui::Rounding::same(10.0))
                        .inner_margin(egui::Margin::same(10.0))
                        .show(ui, |ui| {
                            ui.set_min_width((self.tracks.len() as f32 * 138.0 + 110.0).max(ui.available_width() * 0.68));
                            egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                                egui::Grid::new("performance_apc_grid")
                                    .spacing(egui::vec2(8.0, 8.0))
                                    .show(ui, |ui| {
                                        ui.add_sized(
                                            egui::vec2(86.0, 54.0),
                                            egui::Label::new(
                                                egui::RichText::new("SCENES").strong().color(egui::Color32::from_rgb(220, 224, 228)),
                                            ),
                                        );
                                        for track_index in 0..self.tracks.len() {
                                            let track_color = self.track_color(track_index);
                                            let track_runtime = runtime_snapshot
                                                .get(track_index)
                                                .and_then(|slot| slot.as_ref());
                                            let is_pending = track_runtime
                                                .map(|runtime| runtime.launch_samples > current_transport_samples)
                                                .unwrap_or(false);
                                            let is_live = track_runtime
                                                .map(|runtime| runtime.launch_samples <= current_transport_samples)
                                                .unwrap_or(false);
                                            let tint = if track_solos.get(track_index).copied().unwrap_or(false) {
                                                egui::Color32::from_rgb(242, 191, 94)
                                            } else if track_mutes.get(track_index).copied().unwrap_or(false) {
                                                egui::Color32::from_rgb(154, 78, 78)
                                            } else if is_pending {
                                                Self::tint(track_color, 0.38 + 0.22 * pulse_fast)
                                            } else if is_live {
                                                Self::tint(track_color, 0.48 + 0.26 * pulse_slow)
                                            } else {
                                                egui::Color32::from_rgba_premultiplied(
                                                    track_color.r(),
                                                    track_color.g(),
                                                    track_color.b(),
                                                    124,
                                                )
                                            };
                                            egui::Frame::none()
                                                .fill(tint)
                                                .rounding(egui::Rounding::same(8.0))
                                                .stroke(egui::Stroke::new(
                                                    if is_live || is_pending { 2.0 } else { 1.0 },
                                                    if is_pending {
                                                        egui::Color32::WHITE.gamma_multiply(0.45 + 0.55 * pulse_fast)
                                                    } else {
                                                        Self::tint(track_color, 0.82)
                                                    },
                                                ))
                                                .inner_margin(egui::Margin::same(6.0))
                                                .show(ui, |ui| {
                                                    ui.set_min_size(egui::vec2(128.0, 54.0));
                                                    ui.centered_and_justified(|ui| {
                                                        ui.vertical_centered(|ui| {
                                                            ui.label(
                                                                egui::RichText::new(track_names[track_index].clone())
                                                                    .strong()
                                                                    .color(egui::Color32::WHITE),
                                                            );
                                                            ui.label(
                                                                egui::RichText::new(track_labels[track_index].clone())
                                                                    .size(11.0)
                                                                    .color(egui::Color32::from_gray(236)),
                                                            );
                                                            if is_pending {
                                                                ui.label(
                                                                    egui::RichText::new("QUEUED")
                                                                        .size(9.0)
                                                                        .color(egui::Color32::WHITE),
                                                                );
                                                            } else if is_live {
                                                                ui.label(
                                                                    egui::RichText::new("LIVE")
                                                                        .size(9.0)
                                                                        .color(egui::Color32::WHITE),
                                                                );
                                                            }
                                                        });
                                                    });
                                                });
                                        }
                                        ui.end_row();

                                        for (scene_index, scene_beat) in scenes.iter().enumerate() {
                                            let queued_in_scene = runtime_snapshot
                                                .iter()
                                                .filter(|slot| {
                                                    slot.as_ref()
                                                        .map(|runtime| {
                                                            performance_scene_matches(runtime.clip.start_beats, *scene_beat)
                                                                && runtime.launch_samples > current_transport_samples
                                                        })
                                                        .unwrap_or(false)
                                                })
                                                .count();
                                            let live_in_scene = runtime_snapshot
                                                .iter()
                                                .filter(|slot| {
                                                    slot.as_ref()
                                                        .map(|runtime| {
                                                            performance_scene_matches(runtime.clip.start_beats, *scene_beat)
                                                                && runtime.launch_samples <= current_transport_samples
                                                        })
                                                        .unwrap_or(false)
                                                })
                                                .count();
                                            let is_scene_selected = selected_clip_id
                                                .and_then(|clip_id| {
                                                    self.find_clip_indices_by_id(clip_id)
                                                        .and_then(|(ti, ci)| self.tracks.get(ti).and_then(|t| t.clips.get(ci)))
                                                        .map(|clip| performance_scene_matches(clip.start_beats, *scene_beat))
                                                })
                                                .unwrap_or(false);
                                            let scene_button = egui::Button::new(
                                                egui::RichText::new(format!("{:02}\n{:.1}b", scene_index + 1, scene_beat))
                                                    .strong()
                                                    .size(13.0),
                                            )
                                            .min_size(egui::vec2(86.0, 76.0))
                                            .fill(if queued_in_scene > 0 {
                                                egui::Color32::from_rgb(56, 82, 126)
                                                    .gamma_multiply(0.72 + 0.28 * pulse_fast)
                                            } else if live_in_scene > 0 {
                                                egui::Color32::from_rgb(48, 108, 92)
                                                    .gamma_multiply(0.78 + 0.22 * pulse_slow)
                                            } else if is_scene_selected {
                                                egui::Color32::from_rgb(52, 74, 104)
                                            } else {
                                                egui::Color32::from_rgb(26, 30, 36)
                                            });
                                            if ui
                                                .add(scene_button)
                                                .on_hover_text("Launch scene")
                                                .clicked()
                                            {
                                                scene_launch_action = Some(*scene_beat);
                                            }

                                            for (track_index, track) in self.tracks.iter().enumerate() {
                                                let clip = track
                                                    .clips
                                                    .iter()
                                                    .find(|c| performance_scene_matches(c.start_beats, *scene_beat));

                                                if let Some(clip) = clip {
                                                    let is_selected = selected_clip_id == Some(clip.id);
                                                    let settings = self
                                                        .performance_clip_settings
                                                        .get(&clip.id)
                                                        .cloned()
                                                        .unwrap_or_default();
                                                    let track_color = self.track_color(track_index);
                                                    let runtime_state = runtime_snapshot
                                                        .get(track_index)
                                                        .and_then(|slot| slot.as_ref());
                                                    let is_pending = runtime_state
                                                        .map(|runtime| runtime.clip.id == clip.id && runtime.launch_samples > current_transport_samples)
                                                        .unwrap_or(false);
                                                    let is_live = runtime_state
                                                        .map(|runtime| runtime.clip.id == clip.id && runtime.launch_samples <= current_transport_samples)
                                                        .unwrap_or(false);
                                                    let badge_color = if clip.is_midi {
                                                        egui::Color32::from_rgb(136, 245, 148)
                                                    } else {
                                                        egui::Color32::from_rgb(255, 191, 98)
                                                    };
                                                    let base_fill = if is_live {
                                                        Self::tint(track_color, 0.44 + 0.26 * pulse_slow)
                                                    } else if is_pending {
                                                        Self::tint(track_color, 0.32 + 0.22 * pulse_fast)
                                                    } else if is_selected {
                                                        Self::tint(track_color, 0.28)
                                                    } else {
                                                        egui::Color32::from_rgba_premultiplied(
                                                            track_color.r(),
                                                            track_color.g(),
                                                            track_color.b(),
                                                            94,
                                                        )
                                                    };
                                                    let frame = egui::Frame::none()
                                                        .fill(base_fill)
                                                        .stroke(egui::Stroke::new(
                                                            if is_live || is_pending || is_selected { 2.0 } else { 1.0 },
                                                            if is_pending {
                                                                egui::Color32::WHITE.gamma_multiply(0.55 + 0.45 * pulse_fast)
                                                            } else if is_live {
                                                                badge_color.gamma_multiply(0.82 + 0.18 * pulse_slow)
                                                            } else if is_selected {
                                                                egui::Color32::WHITE.gamma_multiply(0.9)
                                                            } else {
                                                                Self::tint(track_color, 0.68)
                                                            },
                                                        ))
                                                        .rounding(egui::Rounding::same(10.0))
                                                        .inner_margin(egui::Margin::same(6.0));
                                                    let response = frame
                                                        .show(ui, |ui| {
                                                            let desired = egui::vec2(128.0, 76.0);
                                                            let (rect, response) = ui.allocate_exact_size(desired, egui::Sense::click());
                                                            let painter = ui.painter_at(rect);
                                                            painter.rect_filled(
                                                                rect.translate(egui::vec2(0.0, 2.0)),
                                                                10.0,
                                                                egui::Color32::from_rgba_premultiplied(0, 0, 0, 44),
                                                            );
                                                            painter.rect_filled(
                                                                egui::Rect::from_min_max(
                                                                    rect.left_top(),
                                                                    rect.right_top() + egui::vec2(0.0, 20.0),
                                                                ),
                                                                8.0,
                                                                egui::Color32::from_rgba_premultiplied(
                                                                    track_color.r(),
                                                                    track_color.g(),
                                                                    track_color.b(),
                                                                    34,
                                                                ),
                                                            );
                                                            let badge = if clip.is_midi { "MIDI" } else { "AUDIO" };
                                                            shadow_text(
                                                                &painter,
                                                                egui::pos2(rect.left() + 6.0, rect.top() + 6.0),
                                                                egui::Align2::LEFT_TOP,
                                                                badge,
                                                                egui::TextStyle::Small.resolve(ui.style()),
                                                                badge_color.gamma_multiply(0.92),
                                                            );
                                                            painter.circle_filled(
                                                                egui::pos2(rect.right() - 12.0, rect.top() + 12.0),
                                                                4.0,
                                                                if is_pending {
                                                                    egui::Color32::WHITE.gamma_multiply(0.45 + 0.55 * pulse_fast)
                                                                } else if is_live {
                                                                    badge_color.gamma_multiply(0.8 + 0.2 * pulse_slow)
                                                                } else {
                                                                    egui::Color32::from_rgba_premultiplied(255, 255, 255, 44)
                                                                },
                                                            );
                                                            let mut label = if clip.name.trim().is_empty() {
                                                                format!("Clip {}", clip.id)
                                                            } else {
                                                                clip.name.clone()
                                                            };
                                                            if label.len() > 18 {
                                                                label.truncate(18);
                                                                label.push_str("...");
                                                            }
                                                            shadow_text(
                                                                &painter,
                                                                rect.center_top() + egui::vec2(0.0, 24.0),
                                                                egui::Align2::CENTER_TOP,
                                                                &label,
                                                                egui::TextStyle::Button.resolve(ui.style()),
                                                                egui::Color32::WHITE,
                                                            );
                                                            let state_label = if is_pending {
                                                                "QUEUED"
                                                            } else if is_live {
                                                                "LIVE"
                                                            } else if is_selected {
                                                                "READY"
                                                            } else {
                                                                "IDLE"
                                                            };
                                                            shadow_text(
                                                                &painter,
                                                                egui::pos2(rect.center().x, rect.center().y + 4.0),
                                                                egui::Align2::CENTER_CENTER,
                                                                state_label,
                                                                egui::TextStyle::Small.resolve(ui.style()),
                                                                egui::Color32::from_rgba_premultiplied(255, 255, 255, 220),
                                                            );
                                                            let trigger_chip = match settings.trigger_mode {
                                                                PerformanceTriggerMode::Gate => "GT",
                                                                PerformanceTriggerMode::Toggle => "TG",
                                                                PerformanceTriggerMode::OneShot => "1S",
                                                                PerformanceTriggerMode::Loop => "LP",
                                                            };
                                                            let mut chips: Vec<(&str, egui::Color32)> = vec![
                                                                (
                                                                    trigger_chip,
                                                                    egui::Color32::from_rgba_premultiplied(255, 255, 255, 52),
                                                                ),
                                                            ];
                                                            if settings.loop_enabled {
                                                                chips.push((
                                                                    "LOOP",
                                                                    badge_color.gamma_multiply(0.42),
                                                                ));
                                                            }
                                                            if settings.auto_follow {
                                                                chips.push((
                                                                    "MARCH",
                                                                    track_color.gamma_multiply(0.46),
                                                                ));
                                                            }
                                                            let mut chip_x = rect.left() + 6.0;
                                                            let chip_y = rect.bottom() - 16.0;
                                                            for (chip, fill) in chips {
                                                                let width = 8.0 + chip.len() as f32 * 5.2;
                                                                let chip_rect = egui::Rect::from_min_size(
                                                                    egui::pos2(chip_x, chip_y),
                                                                    egui::vec2(width, 10.0),
                                                                );
                                                                painter.rect_filled(chip_rect, 4.0, fill);
                                                                painter.rect_stroke(
                                                                    chip_rect,
                                                                    4.0,
                                                                    egui::Stroke::new(
                                                                        0.8,
                                                                        egui::Color32::from_rgba_premultiplied(255, 255, 255, 28),
                                                                    ),
                                                                );
                                                                painter.text(
                                                                    chip_rect.center(),
                                                                    egui::Align2::CENTER_CENTER,
                                                                    chip,
                                                                    egui::FontId::proportional(7.5),
                                                                    egui::Color32::from_rgba_premultiplied(236, 240, 246, 228),
                                                                );
                                                                chip_x += width + 4.0;
                                                            }
                                                            painter.rect_filled(
                                                                egui::Rect::from_min_max(
                                                                    egui::pos2(rect.left() + 1.0, rect.top() + 1.0),
                                                                    egui::pos2(rect.left() + 4.0, rect.bottom() - 1.0),
                                                                ),
                                                                2.0,
                                                                track_color.gamma_multiply(if is_live { 0.95 } else { 0.68 }),
                                                            );
                                                            painter.rect_filled(
                                                                egui::Rect::from_min_max(
                                                                    egui::pos2(rect.left() + 6.0, rect.bottom() - 4.0),
                                                                    egui::pos2(
                                                                        rect.left() + 6.0 + (rect.width() - 12.0)
                                                                            * if is_pending {
                                                                                pulse_fast
                                                                            } else if is_live {
                                                                                0.72 + 0.28 * pulse_slow
                                                                            } else {
                                                                                0.12
                                                                            },
                                                                        rect.bottom() - 1.0,
                                                                    ),
                                                                ),
                                                                2.0,
                                                                if is_pending {
                                                                    egui::Color32::WHITE.gamma_multiply(0.6 + 0.4 * pulse_fast)
                                                                } else if is_live {
                                                                    badge_color.gamma_multiply(0.8 + 0.2 * pulse_slow)
                                                                } else {
                                                                    badge_color.gamma_multiply(0.45)
                                                                },
                                                            );
                                                            response
                                                        })
                                                        .inner
                                                        .on_hover_text(if clip.is_midi {
                                                            "Double-click to edit MIDI clip"
                                                        } else {
                                                            "Audio clip"
                                                        });
                                                    if response.clicked() {
                                                        click_action = Some((clip.id, track_index));
                                                        let label = if clip.name.trim().is_empty() {
                                                            format!("Clip {}", clip.id)
                                                        } else {
                                                            clip.name.clone()
                                                        };
                                                        trigger_action = Some((
                                                            track_index,
                                                            clip.id,
                                                            settings.clone(),
                                                            label,
                                                        ));
                                                    }
                                                    if response.double_clicked() && clip.is_midi {
                                                        edit_action = Some((clip.id, track_index));
                                                    }
                                                } else {
                                                    let empty = egui::Button::new(
                                                        egui::RichText::new("-").size(18.0).color(egui::Color32::from_gray(110)),
                                                    )
                                                    .min_size(egui::vec2(128.0, 76.0))
                                                    .fill(egui::Color32::from_rgb(22, 25, 30));
                                                    ui.add_enabled(false, empty);
                                                }
                                            }
                                            ui.end_row();
                                        }

                                        ui.add_sized(
                                            egui::vec2(86.0, 34.0),
                                            egui::Label::new(
                                                egui::RichText::new("TRACK STOP").strong().size(11.0),
                                            ),
                                        );
                                        for track_index in 0..self.tracks.len() {
                                            let active_on_track = runtime_snapshot
                                                .get(track_index)
                                                .and_then(|slot| slot.as_ref())
                                                .is_some();
                                            if ui
                                                .add(
                                                    egui::Button::new("Stop")
                                                        .min_size(egui::vec2(128.0, 34.0))
                                                        .fill(if active_on_track {
                                                            egui::Color32::from_rgb(132, 64, 64)
                                                                .gamma_multiply(0.75 + 0.25 * pulse_fast)
                                                        } else {
                                                            egui::Color32::from_rgb(92, 54, 54)
                                                        }),
                                                )
                                                .on_hover_text("Stop this track's performance take")
                                                .clicked()
                                            {
                                                stop_track_action = Some(track_index);
                                            }
                                        }
                                        ui.end_row();
                                    });
                            });
                        });
                });

                if !self.show_mixer {
                    ui.add_space(10.0);

                    ui.vertical(|ui| {
                        egui::Frame::none()
                            .fill(inspector_fill)
                            .stroke(egui::Stroke::new(1.0, panel_edge.gamma_multiply(0.75)))
                            .rounding(egui::Rounding::same(10.0))
                            .inner_margin(egui::Margin::same(12.0))
                            .show(ui, |ui| {
                            ui.set_width(320.0);
                            ui.heading("Session Inspector");
                            ui.label(
                                egui::RichText::new("Launch state, pad macros, and routing")
                                    .size(11.0)
                                    .color(inspector_soft_text),
                            );
                            ui.separator();

                            let active_clip = self.performance_selected_clip.or(self.selected_clip);
                            let selected_track_index = active_clip
                                .and_then(|clip_id| self.find_clip_indices_by_id(clip_id).map(|(track_index, _)| track_index))
                                .or(self.selected_track);
                            let selected_track_color = selected_track_index
                                .map(|index| self.track_color(index))
                                .unwrap_or(egui::Color32::from_rgb(96, 108, 128));

                            egui::Frame::none()
                                .fill(egui::Color32::from_rgba_premultiplied(
                                    selected_track_color.r(),
                                    selected_track_color.g(),
                                    selected_track_color.b(),
                                    34,
                                ))
                                .stroke(egui::Stroke::new(1.0, Self::tint(selected_track_color, 0.78)))
                                .rounding(egui::Rounding::same(10.0))
                                .inner_margin(egui::Margin::same(10.0))
                                .show(ui, |ui| {
                                    let runtime_state = selected_track_index
                                        .and_then(|track_index| runtime_snapshot.get(track_index).and_then(|slot| slot.as_ref()));
                                    let (state_label, state_color) = match runtime_state {
                                        Some(runtime) if runtime.launch_samples > current_transport_samples => (
                                            format!("Queued for {:.2}b", self.samples_to_beats(runtime.launch_samples)),
                                            egui::Color32::WHITE.gamma_multiply(0.55 + 0.45 * pulse_fast),
                                        ),
                                        Some(runtime) => (
                                            format!("Live since {:.2}b", self.samples_to_beats(runtime.launch_samples)),
                                            egui::Color32::from_rgb(126, 236, 142).gamma_multiply(0.76 + 0.24 * pulse_slow),
                                        ),
                                        None => (
                                            "Idle".to_string(),
                                            egui::Color32::from_rgb(150, 160, 176),
                                        ),
                                    };
                                    ui.horizontal(|ui| {
                                        ui.colored_label(state_color, "STATE");
                                        ui.separator();
                                        ui.label(egui::RichText::new(state_label).color(egui::Color32::from_rgb(232, 236, 242)));
                                    });
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label(egui::RichText::new(format!("Clock {:.2}b", clock_beat)).color(inspector_soft_text));
                                        ui.separator();
                                        ui.label(if self.arrangement_playback_enabled() {
                                            "Song follows transport"
                                        } else {
                                            "Session-only clock"
                                        });
                                        ui.separator();
                                        ui.label(egui::RichText::new(format!("Quantize {}", quantize_label)).color(inspector_soft_text));
                                    });
                                });

                            ui.add_space(8.0);

                            if let Some(clip_id) = active_clip {
                                let clip_snapshot = self
                                    .find_clip_indices_by_id(clip_id)
                                    .and_then(|(ti, ci)| self.tracks.get(ti).and_then(|t| t.clips.get(ci)).cloned());

                                if let Some(clip) = clip_snapshot {
                                    let mut settings = self
                                        .performance_clip_settings
                                        .get(&clip_id)
                                        .cloned()
                                        .unwrap_or_default();
                                    let clip_track_index = self
                                        .find_clip_indices_by_id(clip_id)
                                        .map(|(track_index, _)| track_index)
                                        .unwrap_or(0);
                                    let runtime_state = runtime_snapshot
                                        .get(clip_track_index)
                                        .and_then(|slot| slot.as_ref())
                                        .filter(|runtime| runtime.clip.id == clip_id);
                                    egui::Frame::none()
                                        .fill(inspector_card_fill)
                                        .stroke(egui::Stroke::new(1.0, inspector_line))
                                        .rounding(egui::Rounding::same(10.0))
                                        .inner_margin(egui::Margin::same(10.0))
                                        .show(ui, |ui| {
                                            ui.horizontal(|ui| {
                                                ui.colored_label(
                                                    if clip.is_midi {
                                                        egui::Color32::from_rgb(136, 245, 148)
                                                    } else {
                                                        egui::Color32::from_rgb(255, 191, 98)
                                                    },
                                                    if clip.is_midi { "MIDI PAD" } else { "AUDIO PAD" },
                                                );
                                                ui.separator();
                                                ui.label(
                                                    egui::RichText::new(if clip.name.trim().is_empty() {
                                                        format!("Clip {}", clip.id)
                                                    } else {
                                                        clip.name.clone()
                                                    })
                                                    .strong()
                                                    .color(egui::Color32::from_rgb(234, 238, 244)),
                                                );
                                            });
                                            ui.horizontal_wrapped(|ui| {
                                                ui.label(egui::RichText::new(format!("T{}", clip_track_index + 1)).color(inspector_soft_text));
                                                ui.separator();
                                                ui.label(egui::RichText::new(format!("Scene {:.2}b", clip.start_beats)).color(inspector_soft_text));
                                                ui.separator();
                                                ui.label(egui::RichText::new(format!("Len {:.2}b", clip.length_beats)).color(inspector_soft_text));
                                                if let Some(runtime) = runtime_state {
                                                    ui.separator();
                                                    ui.colored_label(
                                                        if runtime.launch_samples > current_transport_samples {
                                                            egui::Color32::WHITE.gamma_multiply(0.5 + 0.5 * pulse_fast)
                                                        } else {
                                                            egui::Color32::from_rgb(126, 236, 142)
                                                        },
                                                        if runtime.launch_samples > current_transport_samples {
                                                            "Queued"
                                                        } else {
                                                            "Live"
                                                        },
                                                    );
                                                }
                                            });
                                        });

                                    ui.add_space(8.0);
                                    ui.horizontal(|ui| {
                                        let action_fill = egui::Color32::from_rgba_premultiplied(38, 44, 54, 232);
                                        let action_stroke = egui::Stroke::new(1.0, inspector_line.gamma_multiply(1.15));
                                        if ui
                                            .add(
                                                egui::Button::new("Launch")
                                                    .fill(action_fill)
                                                    .stroke(action_stroke),
                                            )
                                            .clicked()
                                        {
                                            let label = if clip.name.trim().is_empty() {
                                                format!("Clip {}", clip.id)
                                            } else {
                                                clip.name.clone()
                                            };
                                            trigger_action = Some((clip_track_index, clip.id, settings.clone(), label));
                                        }
                                        if clip.is_midi
                                            && ui
                                                .add(
                                                    egui::Button::new("Piano Roll")
                                                        .fill(action_fill)
                                                        .stroke(action_stroke),
                                                )
                                                .clicked()
                                        {
                                            edit_action = self
                                                .find_clip_indices_by_id(clip_id)
                                                .map(|(track_index, _)| (clip_id, track_index));
                                        }
                                        if ui
                                            .add(
                                                egui::Button::new("Focus Scene")
                                                    .fill(action_fill)
                                                    .stroke(action_stroke),
                                            )
                                            .clicked()
                                        {
                                            scene_launch_action = Some(clip.start_beats.max(0.0));
                                        }
                                    });

                                    ui.add_space(8.0);
                                    ui.label(
                                        egui::RichText::new("Launch Macros")
                                            .size(11.0)
                                            .color(inspector_muted_text),
                                    );
                                    let mut settings_dirty = false;
                                    egui::ComboBox::from_id_source("perf_trigger_mode")
                                        .selected_text(match settings.trigger_mode {
                                            PerformanceTriggerMode::Gate => "Gate",
                                            PerformanceTriggerMode::Toggle => "Toggle",
                                            PerformanceTriggerMode::OneShot => "One Shot",
                                            PerformanceTriggerMode::Loop => "Loop",
                                        })
                                        .show_ui(ui, |ui| {
                                            settings_dirty |= ui.selectable_value(&mut settings.trigger_mode, PerformanceTriggerMode::Gate, "Gate").changed();
                                            settings_dirty |= ui.selectable_value(&mut settings.trigger_mode, PerformanceTriggerMode::Toggle, "Toggle").changed();
                                            settings_dirty |= ui.selectable_value(&mut settings.trigger_mode, PerformanceTriggerMode::OneShot, "One Shot").changed();
                                            settings_dirty |= ui.selectable_value(&mut settings.trigger_mode, PerformanceTriggerMode::Loop, "Loop").changed();
                                        });

                                    let loop_changed = ui.checkbox(&mut settings.loop_enabled, "Loop Enabled").changed();
                                    let auto_follow_changed = ui.checkbox(&mut settings.auto_follow, "Auto March To Next Clip").changed();

                                    if let Some(next_clip_id) = settings.next_clip_id {
                                        if let Some((next_track_index, next_clip_index)) = self.find_clip_indices_by_id(next_clip_id) {
                                            if let Some(next_clip) = self.tracks.get(next_track_index).and_then(|track| track.clips.get(next_clip_index)) {
                                                ui.label(
                                                    egui::RichText::new(format!(
                                                        "Next: T{} {} @ {:.2}b",
                                                        next_track_index + 1,
                                                        if next_clip.name.trim().is_empty() {
                                                            format!("Clip {}", next_clip.id)
                                                        } else {
                                                            next_clip.name.clone()
                                                        },
                                                        next_clip.start_beats,
                                                    ))
                                                    .color(inspector_soft_text),
                                                );
                                            }
                                        }
                                    }

                                    if settings_dirty || loop_changed || auto_follow_changed {
                                        self.performance_clip_settings.insert(clip_id, settings.clone());
                                        self.mark_dirty();
                                    }

                                    if loop_changed {
                                        if settings.loop_enabled {
                                            self.update_clip_by_id(clip_id, |c| {
                                                if c.is_midi {
                                                    if c.midi_source_beats.unwrap_or(0.0) <= 0.0 {
                                                        c.midi_source_beats = Some(c.length_beats.max(0.25));
                                                    }
                                                } else if c.audio_source_beats.unwrap_or(0.0) <= 0.0 {
                                                    c.audio_source_beats = Some(c.length_beats.max(0.25));
                                                }
                                            });
                                        } else {
                                            self.update_clip_by_id(clip_id, |c| {
                                                if c.is_midi {
                                                    c.midi_source_beats = None;
                                                } else {
                                                    c.audio_source_beats = None;
                                                }
                                            });
                                        }
                                    }

                                    if settings.loop_enabled {
                                        let mut loop_len = if clip.is_midi {
                                            clip.midi_source_beats.unwrap_or(clip.length_beats.max(0.25))
                                        } else {
                                            clip.audio_source_beats.unwrap_or(clip.length_beats.max(0.25))
                                        };
                                        ui.horizontal(|ui| {
                                            ui.label("Loop Length");
                                            if ui
                                                .add(egui::DragValue::new(&mut loop_len).speed(0.25).clamp_range(0.25..=256.0))
                                                .changed()
                                            {
                                                let next = loop_len.max(0.25);
                                                self.update_clip_by_id(clip_id, |c| {
                                                    if c.is_midi {
                                                        c.midi_source_beats = Some(next);
                                                    } else {
                                                        c.audio_source_beats = Some(next);
                                                    }
                                                });
                                                self.mark_dirty();
                                            }
                                        });
                                    }
                                } else {
                                    ui.label(egui::RichText::new("Selected clip no longer exists.").color(inspector_soft_text));
                                }
                            } else {
                                ui.label(egui::RichText::new("Select a pad to inspect launch state and macros.").color(inspector_soft_text));
                            }

                            ui.add_space(12.0);
                            ui.separator();
                            ui.label(
                                egui::RichText::new("Device Chain")
                                    .size(11.0)
                                    .color(inspector_muted_text),
                            );
                            if let Some(track_index) = selected_track_index {
                                if let Some(track) = self.tracks.get(track_index) {
                                    egui::Frame::none()
                                        .fill(inspector_card_fill_alt)
                                        .stroke(egui::Stroke::new(1.0, inspector_line))
                                        .rounding(egui::Rounding::same(10.0))
                                        .inner_margin(egui::Margin::same(10.0))
                                        .show(ui, |ui| {
                                            ui.colored_label(Self::tint(self.track_color(track_index), 0.75), format!("{} • {}", track_names[track_index], track.name));
                                            ui.add_space(4.0);
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "Instrument: {}",
                                                    track.instrument_path
                                                        .as_deref()
                                                        .map(Self::plugin_display_name)
                                                        .unwrap_or_else(|| "None".to_string())
                                                ))
                                                .color(inspector_soft_text),
                                            );
                                            if track.effect_paths.is_empty() {
                                                ui.label(egui::RichText::new("FX: none").color(inspector_muted_text));
                                            } else {
                                                for (fx_index, fx_path) in track.effect_paths.iter().enumerate() {
                                                    ui.label(
                                                        egui::RichText::new(format!("FX {}: {}", fx_index + 1, Self::plugin_display_name(fx_path)))
                                                            .color(inspector_soft_text),
                                                    );
                                                }
                                            }
                                        });

                                    ui.add_space(10.0);
                                    ui.label("Selected Track Controls");
                                    let mut exclusive_solo = false;
                                    let mut track_mix_changed = false;
                                    if let Some(track) = self.tracks.get_mut(track_index) {
                                        let meter = egui::ProgressBar::new(
                                            track_strip_peaks.get(track_index).copied().unwrap_or(0.0),
                                        )
                                        .desired_width(280.0)
                                        .fill(if track.muted {
                                            egui::Color32::from_rgb(134, 72, 72)
                                        } else if track.solo {
                                            egui::Color32::from_rgb(232, 190, 92)
                                        } else {
                                            Self::tint(selected_track_color, 0.7)
                                        });
                                        ui.add(meter);
                                        ui.horizontal(|ui| {
                                            if ui.selectable_label(track.muted, "Mute").clicked() {
                                                track.muted = !track.muted;
                                                track_mix_changed = true;
                                            }
                                            if ui.selectable_label(track.solo, "Solo").clicked() {
                                                let multi = ui.input(|i| i.modifiers.shift || i.modifiers.ctrl);
                                                if track.solo {
                                                    track.solo = false;
                                                } else if multi {
                                                    track.solo = true;
                                                } else {
                                                    track.solo = true;
                                                    exclusive_solo = true;
                                                }
                                                track_mix_changed = true;
                                            }
                                            if ui.button("Stop Track").clicked() {
                                                stop_track_action = Some(track_index);
                                            }
                                        });
                                        let response = ui.add_sized(
                                            egui::vec2(280.0, 20.0),
                                            egui::Slider::new(&mut track.level, 0.0..=1.2).text("Level"),
                                        );
                                        if response.changed() || response.dragged() {
                                            track_mix_changed = true;
                                        }
                                    }
                                    if exclusive_solo {
                                        for (idx, track) in self.tracks.iter_mut().enumerate() {
                                            track.solo = idx == track_index;
                                        }
                                    }
                                    if track_mix_changed {
                                        self.sync_track_mix();
                                        self.mark_dirty();
                                    }
                                }
                            } else {
                                ui.label("Select a pad or track to inspect instrument and FX routing.");
                            }
                            });
                        });
                    }
            });

            if let Some(scene_beat) = scene_launch_action {
                let launch_samples = self.performance_launch_samples();
                let launch_beat = self.samples_to_beats(launch_samples);
                match self.launch_performance_scene_at(scene_beat, launch_samples) {
                    Ok(launched) => {
                        if self.is_recording && self.record_performance {
                            let _ = self.record_performance_scene_trigger_at(scene_beat, launch_beat);
                        }
                        self.selected_track = self
                            .tracks
                            .iter()
                            .enumerate()
                            .find_map(|(track_index, track)| {
                                track.clips
                                    .iter()
                                    .any(|clip| performance_scene_matches(clip.start_beats, scene_beat))
                                    .then_some(track_index)
                            });
                        self.status = format!("Scene launched: {} clips", launched);
                    }
                    Err(err) => {
                        self.status = format!("Scene launch failed: {err}");
                    }
                }
            }
            if let Some(track_index) = stop_track_action {
                self.selected_track = Some(track_index);
                self.stop_performance_track(track_index);
                if self.is_recording && self.record_performance {
                    self.record_performance_track_stop(track_index);
                    self.status = format!("Recorded track {} stop", track_index + 1);
                } else {
                    self.status = format!("Stopped track {} performance", track_index + 1);
                }
            }
            if let Some((clip_id, track_index)) = click_action {
                self.performance_selected_clip = Some(clip_id);
                self.selected_clip = Some(clip_id);
                self.selected_clips.clear();
                self.selected_clips.insert(clip_id);
                self.selected_track = Some(track_index);
            }
            if let Some((clip_id, track_index)) = edit_action {
                self.performance_selected_clip = Some(clip_id);
                self.selected_clip = Some(clip_id);
                self.selected_clips.clear();
                self.selected_clips.insert(clip_id);
                self.selected_track = Some(track_index);
                self.main_tab = MainTab::PianoRoll;
                self.status = format!("Editing MIDI clip {}", clip_id);
            }
            if let Some((track_index, clip_id, settings, label)) = trigger_action {
                let launch_beat = self.samples_to_beats(self.performance_launch_samples());
                match self.launch_performance_clip(track_index, clip_id, settings.clone()) {
                    Ok(()) => {
                        if self.is_recording && self.record_performance {
                            self.record_performance_clip_trigger(track_index, clip_id, launch_beat, settings);
                            performance_status = Some(format!("Recorded performance trigger: {}", label));
                        } else {
                            performance_status = Some(format!("Launched {}", label));
                        }
                    }
                    Err(err) => {
                        performance_status = Some(format!("Launch failed: {err}"));
                    }
                }
            }
            if let Some(status) = performance_status {
                self.status = status;
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("Session");
                ui.separator();
                ui.label(format!("Tracks: {}", self.tracks.len()));
                ui.label(format!("Scenes: {}", scenes.len()));
                if let Some(clip_id) = selected_clip_id {
                    ui.label(format!("Selected Clip: {}", clip_id));
                }
            });
        });
    }
}
