impl DawApp {
    pub(crate) fn clip_palette_color(&self, index: usize) -> egui::Color32 {
        let palette = [
            egui::Color32::from_rgb(237, 74, 55),
            egui::Color32::from_rgb(247, 148, 30),
            egui::Color32::from_rgb(247, 216, 70),
            egui::Color32::from_rgb(69, 200, 112),
            egui::Color32::from_rgb(59, 170, 235),
            egui::Color32::from_rgb(74, 100, 216),
            egui::Color32::from_rgb(154, 83, 214),
        ];
        palette[index % palette.len()]
    }

    pub(crate) fn track_color(&self, track_index: usize) -> egui::Color32 {
        self.clip_palette_color(track_index)
    }

    pub(crate) fn colored_slider(
        ui: &mut egui::Ui,
        value: &mut f32,
        range: std::ops::RangeInclusive<f32>,
        color: Option<egui::Color32>,
    ) -> egui::Response {
        if let Some(color) = color {
            ui.scope(|ui| {
                let mut visuals = ui.visuals().clone();
                visuals.widgets.inactive.bg_fill = color.linear_multiply(0.35);
                visuals.widgets.hovered.bg_fill = color.linear_multiply(0.5);
                visuals.widgets.active.bg_fill = color.linear_multiply(0.8);
                visuals.widgets.inactive.fg_stroke.color = egui::Color32::from_gray(200);
                visuals.widgets.hovered.fg_stroke.color = egui::Color32::from_gray(230);
                visuals.widgets.active.fg_stroke.color = egui::Color32::from_gray(240);
                ui.style_mut().visuals = visuals;
                ui.add(egui::Slider::new(value, range).show_value(false))
            })
            .inner
        } else {
            ui.add(egui::Slider::new(value, range).show_value(false))
        }
    }

    pub(crate) fn ensure_live_params(&mut self) {
        let Some(index) = self.selected_track else {
            return;
        };
        let host = self.selected_track_host();
        let Some(track) = self.tracks.get_mut(index) else {
            return;
        };
        if !track.param_ids.is_empty() && track.param_ids.len() == track.params.len() {
            return;
        }
        let Some(host) = host else {
            return;
        };
        let params = host.enumerate_params();
        if params.is_empty() {
            return;
        }
        track.param_values = Self::remap_param_values_by_id_or_name(
            &track.param_ids,
            &track.params,
            &track.param_values,
            &params,
        );
        track.params = params.iter().map(|p| p.name.clone()).collect();
        track.param_ids = params.iter().map(|p| p.id).collect();
        Self::log_fm_ratio_param_from(index, "ensure_live", &track.params, &track.param_ids, &track.param_values);
    }

    pub(crate) fn refresh_clap_params_if_needed(&mut self) {
        let Some(index) = self.selected_track else {
            return;
        };
        let Some(PluginHostHandle::Clap(host)) = self.selected_track_host() else {
            return;
        };
        let params = if let Some(mut host) = host.try_lock() {
            let flags = host.take_param_rescan();
            if flags == 0 {
                return;
            }
            host.enumerate_params()
        } else {
            return;
        };
        if params.is_empty() {
            return;
        }
        if let Some(track) = self.tracks.get_mut(index) {
            track.param_values = Self::remap_param_values_by_id_or_name_clap(
                &track.param_ids,
                &track.params,
                &track.param_values,
                &params,
            );
            track.params = params.iter().map(|p| p.name.clone()).collect();
            track.param_ids = params.iter().map(|p| p.id).collect();
            Self::log_fm_ratio_param_from(index, "refresh_clap", &track.params, &track.param_ids, &track.param_values);
        }
    }

    pub(crate) fn sync_last_param_changes(&mut self) {
        for (index, state) in self.engine.track_audio.iter().enumerate() {
            let Some(PluginHostHandle::Vst3(host)) = state.host.as_ref() else {
                continue;
            };
            let mut host = match host.try_lock() {
                Some(host) => host,
                None => continue,
            };
            let Some((param_id, value)) = host.take_last_param_change() else {
                continue;
            };
            if let Some(track) = self.tracks.get_mut(index) {
                if let Some(pos) = track.param_ids.iter().position(|id| *id == param_id) {
                    if track.param_values.len() != track.param_ids.len() {
                        track.param_values.resize(track.param_ids.len(), 0.0);
                    }
                    if let Some(target) = track.param_values.get_mut(pos) {
                        *target = value as f32;
                    }
                }
            }
        }
    }

    pub(crate) fn tint(color: egui::Color32, amount: f32) -> egui::Color32 {
        let r = (color.r() as f32 * amount).min(255.0) as u8;
        let g = (color.g() as f32 * amount).min(255.0) as u8;
        let b = (color.b() as f32 * amount).min(255.0) as u8;
        egui::Color32::from_rgb(r, g, b)
    }

    pub(crate) fn apply_theme(&self, ctx: &egui::Context) {
        let mut visuals = egui::Visuals::dark();
        match self.settings.theme.as_str() {
            "Black" => {
                visuals.window_fill = egui::Color32::from_rgb(0, 0, 0);
                visuals.panel_fill = egui::Color32::from_rgb(0, 0, 0);
                visuals.faint_bg_color = egui::Color32::from_rgb(10, 10, 10);
                visuals.extreme_bg_color = egui::Color32::from_rgb(0, 0, 0);
                visuals.override_text_color = Some(egui::Color32::from_rgb(245, 245, 245));
                visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(14, 14, 14);
                visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(22, 22, 22);
                visuals.widgets.active.bg_fill = egui::Color32::from_rgb(36, 36, 36);
                visuals.selection.bg_fill = egui::Color32::from_rgb(60, 60, 60);
                visuals.selection.stroke.color = egui::Color32::from_rgb(240, 240, 240);
            }
            "Dark" => {
                visuals = egui::Visuals::dark();
            }
            _ => {}
        }
        if self.wallpaper_enabled() {
            visuals.window_fill = egui::Color32::from_rgba_premultiplied(0, 0, 0, 208);
            visuals.panel_fill = egui::Color32::from_rgba_premultiplied(0, 0, 0, 176);
            visuals.faint_bg_color = egui::Color32::from_rgba_premultiplied(10, 10, 10, 176);
            visuals.extreme_bg_color = egui::Color32::from_rgba_premultiplied(0, 0, 0, 224);
        }
        ctx.set_visuals(visuals);
    }

    pub(crate) fn outlined_text(
        painter: &egui::Painter,
        pos: egui::Pos2,
        align: egui::Align2,
        text: &str,
        font: egui::FontId,
        color: egui::Color32,
    ) {
        let outline = egui::Color32::from_rgba_premultiplied(0, 0, 0, 150);
        let offsets = [
            egui::vec2(-0.75, 0.0),
            egui::vec2(0.75, 0.0),
            egui::vec2(0.0, -0.75),
            egui::vec2(0.0, 0.75),
            egui::vec2(-0.6, -0.6),
            egui::vec2(0.6, -0.6),
            egui::vec2(-0.6, 0.6),
            egui::vec2(0.6, 0.6),
        ];
        for offset in offsets {
            painter.text(pos + offset, align, text, font.clone(), outline);
        }
        painter.text(pos, align, text, font, color);
    }

    pub(crate) fn draw_midi_preview(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        clip: &Clip,
        clip_left: f32,
        beat_width: f32,
    ) {
        let notes = &clip.midi_notes;
        if notes.is_empty() {
            return;
        }
        let clip_start = clip.start_beats;
        let clip_len = clip.length_beats.max(0.001);
        let loop_len = self.clip_loop_len_beats(clip).unwrap_or(clip_len);
        let painter = painter.with_clip_rect(rect);
        let mut min_note: Option<u8> = None;
        let mut max_note: Option<u8> = None;
        for note in notes {
            if note.start_beats + note.length_beats < clip_start {
                continue;
            }
            if note.start_beats > clip_start + clip_len {
                continue;
            }
            min_note = Some(min_note.map_or(note.midi_note, |v| v.min(note.midi_note)));
            max_note = Some(max_note.map_or(note.midi_note, |v| v.max(note.midi_note)));
        }
        let (min_note, max_note) = match (min_note, max_note) {
            (Some(min_note), Some(max_note)) => (min_note, max_note),
            _ => return,
        };
        let row_count = (max_note.saturating_sub(min_note) as f32 + 1.0).max(1.0);
        let note_height = (rect.height() / row_count).max(1.0);
        let clip_end = clip_start + clip_len;
        for (index, note) in notes.iter().enumerate() {
            let rel = note.start_beats - clip_start;
            if rel < 0.0 || rel >= loop_len {
                continue;
            }
            let mut t = clip_start + rel;
            while t < clip_end {
                let note_end = t + note.length_beats;
                if note_end < clip_start || t > clip_end {
                    t += loop_len;
                    continue;
                }
                let local_start = (t - clip_start).max(0.0);
                let local_len = note.length_beats.min(clip_len - local_start).max(0.0);
                let x = clip_left + local_start * beat_width;
                let w = (local_len * beat_width).max(2.0);
                let row_index = note.midi_note.saturating_sub(min_note) as f32;
                let y = rect.bottom() - (row_index + 1.0) * note_height;
                let note_rect = egui::Rect::from_min_size(
                    egui::pos2(x, y),
                    egui::vec2(w, (note_height * 0.9).max(1.0)),
                );
                let base = if index % 2 == 0 {
                    egui::Color32::from_rgb(88, 210, 180)
                } else {
                    egui::Color32::from_rgb(120, 130, 240)
                };
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
                painter.rect_filled(note_rect, 2.0, color);
                t += loop_len;
            }
        }
    }

    pub(crate) fn key_pitch_offset(target_key: Option<u8>, source_key: Option<u8>) -> f32 {
        let (Some(target_key), Some(source_key)) = (target_key, source_key) else {
            return 0.0;
        };
        let target = (target_key % 12) as i8;
        let source = (source_key % 12) as i8;
        let mut diff = target - source;
        diff = ((diff + 6).rem_euclid(12)) - 6;
        diff as f32
    }

    pub(crate) fn clip_effective_pitch_semitones(&self, clip: &Clip) -> f32 {
        let target_key = clip.audio_key.or(self.project_key);
        let source_key = clip.audio_key_source.or(clip.audio_key);
        let key_offset = Self::key_pitch_offset(target_key, source_key);
        clip.audio_pitch_semitones + (clip.audio_fine_pitch_cents / 100.0) + key_offset
    }

    pub(crate) fn audio_playback_time_mul(clip: &Clip, pitch_semitones: f32) -> f32 {
        let base = clip.audio_time_mul.max(0.01);
        if clip.audio_stretch_mode == AudioStretchMode::Speed {
            let ratio = 2.0f32.powf(pitch_semitones / 12.0);
            (base / ratio).max(0.01)
        } else {
            base
        }
    }
}
