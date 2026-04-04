impl DawApp {
    pub(crate) fn draw_audio_analysis_controls(
        ui: &mut egui::Ui,
        clip: &mut Clip,
        clip_path: Option<&Path>,
        key_display_format: &str,
        tempo_bpm: f32,
        analyzing: bool,
    ) -> (bool, bool) {
        let mut changed = false;
        ui.add_space(6.0);
        ui.label("Analysis");
        ui.horizontal(|ui| {
            ui.label("Key");
            let display = Self::format_key_display_with(
                key_display_format,
                clip.audio_key,
                clip.audio_key_minor,
            );
            ui.label(display);
            if ui.button("-").clicked() {
                Self::nudge_clip_key(clip, -1);
                changed = true;
            }
            if ui.button("+").clicked() {
                Self::nudge_clip_key(clip, 1);
                changed = true;
            }
            if ui.checkbox(&mut clip.audio_key_minor, "Minor").changed() {
                changed = true;
            }
        });
        ui.horizontal(|ui| {
            ui.label("BPM");
            let mut bpm = clip.audio_bpm.unwrap_or(tempo_bpm.max(1.0));
            if ui
                .add(egui::DragValue::new(&mut bpm).speed(0.1).clamp_range(60.0..=200.0))
                .changed()
            {
                Self::apply_clip_bpm(clip, bpm, clip_path);
                changed = true;
            }
            if ui
                .add_enabled(clip.audio_bpm.is_some(), egui::Button::new("Clear"))
                .clicked()
            {
                clip.audio_bpm = None;
                changed = true;
            }
        });
        ui.horizontal(|ui| {
            ui.label("Fine Pitch");
            if ui
                .add(
                    egui::DragValue::new(&mut clip.audio_fine_pitch_cents)
                        .speed(0.5)
                        .clamp_range(-100.0..=100.0),
                )
                .changed()
            {
                changed = true;
            }
            ui.label("cents");
        });
        let analyze_label = if analyzing { "Analyzing..." } else { "Analyze" };
        let analyze_clicked = ui
            .add_enabled(!analyzing, egui::Button::new(analyze_label))
            .clicked();
        (analyze_clicked && clip_path.is_some(), changed)
    }

    pub(crate) fn format_key_display(&self, key: Option<u8>, minor: bool) -> String {
        Self::format_key_display_with(self.settings.key_display_format.as_str(), key, minor)
    }

    pub(crate) fn format_key_display_with(format: &str, key: Option<u8>, minor: bool) -> String {
        let Some(key) = key else {
            return "Unknown".to_string();
        };
        let key = key % 12;
        match format {
            "traditional" => Self::key_name_traditional(key, minor),
            "both" => {
                let cam = Self::key_name_camelot(key, minor);
                let trad = Self::key_name_traditional(key, minor);
                format!("{cam} / {trad}")
            }
            _ => Self::key_name_camelot(key, minor).to_string(),
        }
    }

    pub(crate) fn key_name_traditional(key: u8, minor: bool) -> String {
        const NAMES: [&str; 12] = [
            "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
        ];
        let name = NAMES[key as usize];
        if minor {
            format!("{name}m")
        } else {
            name.to_string()
        }
    }

    pub(crate) fn key_name_camelot(key: u8, minor: bool) -> &'static str {
        const MAJOR: [&str; 12] = [
            "8B", "3B", "10B", "5B", "12B", "7B", "2B", "9B", "4B", "11B", "6B",
            "1B",
        ];
        const MINOR: [&str; 12] = [
            "5A", "12A", "7A", "2A", "9A", "4A", "11A", "6A", "1A", "8A", "3A",
            "10A",
        ];
        if minor {
            MINOR[key as usize]
        } else {
            MAJOR[key as usize]
        }
    }

    pub(crate) fn analyze_audio_clip_path(path: &Path) -> Option<AudioAnalysis> {
        let (samples, channels, sample_rate) = Self::decode_audio_samples(path)?;
        if samples.is_empty() || sample_rate == 0 || channels == 0 {
            return None;
        }
        let mono = Self::downmix_to_mono(&samples, channels);
        let bpm = Self::estimate_bpm(&mono, sample_rate);
        let key = Self::estimate_key(&mono, sample_rate);
        let fine_pitch_cents = Self::estimate_fine_pitch_cents(&mono, sample_rate);
        Some(AudioAnalysis {
            bpm,
            key,
            fine_pitch_cents,
        })
    }

    pub(crate) fn apply_audio_analysis_to_clip(clip: &mut Clip, path: Option<&Path>, analysis: AudioAnalysis) {
        if let Some((key, minor)) = analysis.key {
            clip.audio_key = Some(key);
            clip.audio_key_minor = minor;
            clip.audio_key_source = Some(key);
        }
        if let Some(bpm) = analysis.bpm {
            let bpm = bpm.clamp(60.0, 200.0);
            clip.audio_bpm = Some(bpm);
            if let Some(path) = path {
                if let Some(seconds) = Self::audio_length_seconds(path) {
                    clip.audio_source_beats = Some((seconds * bpm / 60.0).max(0.001));
                }
            }
        }
        if let Some(cents) = analysis.fine_pitch_cents {
            clip.audio_fine_pitch_cents = cents.clamp(-100.0, 100.0);
        }
    }

    pub(crate) fn apply_clip_bpm(clip: &mut Clip, bpm: f32, path: Option<&Path>) {
        let bpm = bpm.clamp(60.0, 200.0);
        clip.audio_bpm = Some(bpm);
        if let Some(path) = path {
            if let Some(seconds) = Self::audio_length_seconds(path) {
                clip.audio_source_beats = Some((seconds * bpm / 60.0).max(0.001));
            }
        }
    }

    pub(crate) fn nudge_clip_key(clip: &mut Clip, delta: i8) {
        if clip.audio_key_source.is_none() {
            clip.audio_key_source = clip.audio_key;
        }
        let key = clip.audio_key.get_or_insert(0);
        let next = ((*key as i8) + delta).rem_euclid(12) as u8;
        *key = next;
    }

    pub(crate) fn downmix_to_mono(samples: &[f32], channels: usize) -> Vec<f32> {
        if channels <= 1 {
            return samples.to_vec();
        }
        let mut mono = Vec::with_capacity(samples.len() / channels.max(1));
        for frame in samples.chunks_exact(channels) {
            let sum: f32 = frame.iter().copied().sum();
            mono.push(sum / channels as f32);
        }
        mono
    }

    pub(crate) fn estimate_bpm(samples: &[f32], sample_rate: u32) -> Option<f32> {
        if samples.len() < sample_rate as usize {
            return None;
        }
        let hop = 1024usize;
        let frame = 2048usize;
        if samples.len() <= frame {
            return None;
        }
        let mut envelope = Vec::new();
        let mut idx = 0usize;
        while idx + frame <= samples.len() {
            let mut sum = 0.0f32;
            for sample in &samples[idx..idx + frame] {
                sum += sample.abs();
            }
            envelope.push(sum / frame as f32);
            idx += hop;
        }
        if envelope.len() < 8 {
            return None;
        }
        let mean = envelope.iter().copied().sum::<f32>() / envelope.len() as f32;
        for value in &mut envelope {
            *value = (*value - mean).max(0.0);
        }
        let min_bpm = 60.0f32;
        let max_bpm = 200.0f32;
        let frame_rate = sample_rate as f32 / hop as f32;
        let min_lag = ((frame_rate * 60.0) / max_bpm).round().max(1.0) as usize;
        let max_lag = ((frame_rate * 60.0) / min_bpm).round() as usize;
        if max_lag <= min_lag || max_lag >= envelope.len() {
            return None;
        }
        let mut best_lag = 0usize;
        let mut best_score = 0.0f32;
        for lag in min_lag..=max_lag {
            let mut score = 0.0f32;
            let limit = envelope.len().saturating_sub(lag);
            for i in 0..limit {
                score += envelope[i] * envelope[i + lag];
            }
            if score > best_score {
                best_score = score;
                best_lag = lag;
            }
        }
        if best_lag == 0 {
            return None;
        }
        let bpm = (60.0 * frame_rate) / best_lag as f32;
        Some(bpm)
    }

    pub(crate) fn estimate_key(samples: &[f32], sample_rate: u32) -> Option<(u8, bool)> {
        if samples.len() < 4096 || sample_rate == 0 {
            return None;
        }
        let fft_size = 4096usize;
        let hop = 2048usize;
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(fft_size);
        let mut buffer = vec![Complex { re: 0.0, im: 0.0 }; fft_size];
        let mut window = Vec::with_capacity(fft_size);
        let denom = (fft_size - 1) as f32;
        for i in 0..fft_size {
            let phase = 2.0 * std::f32::consts::PI * (i as f32 / denom);
            window.push(0.5 - 0.5 * phase.cos());
        }
        let mut chroma = [0.0f32; 12];
        let mut pos = 0usize;
        while pos + fft_size <= samples.len() {
            for i in 0..fft_size {
                buffer[i].re = samples[pos + i] * window[i];
                buffer[i].im = 0.0;
            }
            fft.process(&mut buffer);
            let half = fft_size / 2;
            for bin in 1..half {
                let freq = bin as f32 * sample_rate as f32 / fft_size as f32;
                if !(30.0..=5000.0).contains(&freq) {
                    continue;
                }
                let mag = buffer[bin].norm();
                if mag <= 0.0 {
                    continue;
                }
                let midi = 69.0 + 12.0 * (freq / 440.0).log2();
                let pc = (midi.round() as i32).rem_euclid(12) as usize;
                chroma[pc] += mag;
            }
            pos += hop;
        }
        let sum: f32 = chroma.iter().copied().sum();
        if sum <= 0.0 {
            return None;
        }
        for value in &mut chroma {
            *value /= sum;
        }
        let major_template: [f32; 12] = [
            6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
        ];
        let minor_template: [f32; 12] = [
            6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
        ];
        let mut best_score = -1.0f32;
        let mut best_key = 0u8;
        let mut best_minor = false;
        for key in 0..12 {
            let mut major_score = 0.0f32;
            let mut minor_score = 0.0f32;
            for i in 0..12 {
                let idx = (i + key) % 12;
                major_score += major_template[i] * chroma[idx];
                minor_score += minor_template[i] * chroma[idx];
            }
            if major_score > best_score {
                best_score = major_score;
                best_key = key as u8;
                best_minor = false;
            }
            if minor_score > best_score {
                best_score = minor_score;
                best_key = key as u8;
                best_minor = true;
            }
        }
        Some((best_key, best_minor))
    }

    pub(crate) fn estimate_fine_pitch_cents(samples: &[f32], sample_rate: u32) -> Option<f32> {
        if samples.len() < 2048 || sample_rate == 0 {
            return None;
        }
        let mut fft_size = 4096usize;
        if samples.len() < fft_size {
            fft_size = 2048usize.min(samples.len());
        }
        if fft_size < 1024 {
            return None;
        }
        let hop = (fft_size / 2).max(512);
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(fft_size);
        let mut buffer = vec![Complex { re: 0.0, im: 0.0 }; fft_size];
        let mut window = Vec::with_capacity(fft_size);
        let denom = (fft_size - 1) as f32;
        for i in 0..fft_size {
            let phase = 2.0 * std::f32::consts::PI * (i as f32 / denom);
            window.push(0.5 - 0.5 * phase.cos());
        }
        let mut weighted = 0.0f32;
        let mut total = 0.0f32;
        let mut pos = 0usize;
        while pos + fft_size <= samples.len() {
            for i in 0..fft_size {
                buffer[i].re = samples[pos + i] * window[i];
                buffer[i].im = 0.0;
            }
            fft.process(&mut buffer);
            let half = fft_size / 2;
            for bin in 1..half {
                let freq = bin as f32 * sample_rate as f32 / fft_size as f32;
                if !(50.0..=2000.0).contains(&freq) {
                    continue;
                }
                let mag = buffer[bin].norm();
                if mag <= 0.0 {
                    continue;
                }
                let midi = 69.0 + 12.0 * (freq / 440.0).log2();
                let cents = (midi - midi.round()) * 100.0;
                weighted += cents * mag;
                total += mag;
            }
            pos += hop;
        }
        if total <= 0.0 {
            return None;
        }
        Some((weighted / total).clamp(-100.0, 100.0))
    }

    pub(crate) fn ensure_audio_analysis_worker(&mut self) {
        if self.analysis_sender.is_some() {
            return;
        }
        let (req_tx, req_rx) = mpsc::channel::<AudioAnalysisRequest>();
        let (res_tx, res_rx) = mpsc::channel::<AudioAnalysisResult>();
        thread::spawn(move || {
            while let Ok(req) = req_rx.recv() {
                let analysis = Self::analyze_audio_clip_path(&req.path);
                let _ = res_tx.send(AudioAnalysisResult {
                    clip_id: req.clip_id,
                    path: req.path,
                    analysis,
                });
            }
        });
        self.analysis_sender = Some(req_tx);
        self.analysis_receiver = Some(res_rx);
    }

    pub(crate) fn enqueue_audio_analysis(&mut self, clip_id: usize, path: PathBuf) {
        if self.analysis_pending.contains(&clip_id) {
            return;
        }
        self.ensure_audio_analysis_worker();
        if let Some(sender) = self.analysis_sender.as_ref() {
            if sender
                .send(AudioAnalysisRequest { clip_id, path })
                .is_ok()
            {
                self.analysis_pending.insert(clip_id);
                self.status = "Analyzing clip...".to_string();
            }
        }
    }

    pub(crate) fn poll_audio_analysis_jobs(&mut self) {
        let Some(receiver) = self.analysis_receiver.as_ref() else {
            return;
        };
        while let Ok(result) = receiver.try_recv() {
            self.analysis_pending.remove(&result.clip_id);
            let Some((ti, ci)) = self.find_clip_indices_by_id(result.clip_id) else {
                continue;
            };
            let current_path = self
                .tracks
                .get(ti)
                .and_then(|t| t.clips.get(ci))
                .and_then(|clip| self.resolve_clip_audio_path(clip));
            let matches_path = current_path
                .as_ref()
                .map(|p| Self::paths_equal(p, &result.path))
                .unwrap_or(false);
            if !matches_path {
                continue;
            }
            if let Some(analysis) = result.analysis {
                if let Some(clip) = self
                    .tracks
                    .get_mut(ti)
                    .and_then(|t| t.clips.get_mut(ci))
                {
                    Self::apply_audio_analysis_to_clip(clip, Some(&result.path), analysis);
                    self.status = "Analysis complete".to_string();
                }
            } else {
                self.status = "Analyze failed: unsupported audio file".to_string();
            }
        }
    }

    pub(crate) fn refresh_audio_clip_timeline_if_running(&mut self) {
        if !self.audio_running {
            return;
        }
        let timeline = self.build_audio_clip_timeline(self.settings.sample_rate);
        {
            let mut guard = self.engine.audio_clips.lock();
            *guard = timeline;
        }
    }

    pub(crate) fn audio_source_beats(&self, clip: &Clip) -> Option<f32> {
        let tempo = self.tempo_bpm.max(1.0);
        self.get_waveform_seconds_for_clip(clip)
            .map(|seconds| (seconds * tempo / 60.0).max(0.001))
            .or_else(|| clip.audio_source_beats.map(|beats| beats.max(0.001)))
    }

    pub(crate) fn clip_loop_len_beats(&self, clip: &Clip) -> Option<f32> {
        if clip.is_midi {
            return Self::midi_loop_len_for_clip(clip);
        }
        let pitch = self.clip_effective_pitch_semitones(clip);
        let time_mul = Self::audio_playback_time_mul(clip, pitch);
        let source_beats = self.audio_source_beats(clip)?;
        let loop_len = source_beats * time_mul;
        if loop_len > 0.0 {
            Some(loop_len)
        } else {
            None
        }
    }

    pub(crate) fn draw_audio_preview(
        &self,
        painter: &egui::Painter,
        rect: egui::Rect,
        seed: usize,
        waveform: Option<&[f32]>,
        waveform_color: Option<&[[f32; 3]]>,
        clip: &Clip,
        timeline: Option<(f32, f32)>,
    ) {
        let mid_y = rect.center().y;
        if let Some(waveform) = waveform {
            let count = waveform.len().max(1);
            let step = rect.width() / count as f32;
            let pitch = self.clip_effective_pitch_semitones(clip);
            let time_mul = Self::audio_playback_time_mul(clip, pitch);
            let clip_len = clip.length_beats.max(0.001);
            let source_beats = self
                .get_waveform_seconds_for_clip(clip)
                .map(|seconds| (seconds * self.tempo_bpm.max(1.0) / 60.0).max(0.001))
                .unwrap_or_else(|| {
                    clip.audio_source_beats
                        .unwrap_or(clip_len / time_mul)
                        .max(0.001)
                });
            let offset_beats = clip.audio_offset_beats.max(0.0);
            for index in 0..count {
                let amp = if let Some((row_left, beat_width)) = timeline {
                    let x = rect.left() + index as f32 * step;
                    let beat = (x - row_left) / beat_width;
                    let local_beat = beat - clip.start_beats;
                    if local_beat < 0.0 || local_beat > clip_len {
                        0.0
                    } else {
                        let mut src_beat = (offset_beats + local_beat) / time_mul;
                        if source_beats > 0.0 {
                            src_beat = src_beat.rem_euclid(source_beats);
                        }
                        let src_pos = if source_beats > 0.0 {
                            (src_beat / source_beats) * (count as f32 - 1.0)
                        } else {
                            index as f32
                        };
                        let left = src_pos.floor().clamp(0.0, (count - 1) as f32) as usize;
                        let right = (left + 1).min(count - 1);
                        let frac = src_pos - left as f32;
                        let amp = waveform
                            .get(left)
                            .copied()
                            .unwrap_or(0.0)
                            + (waveform.get(right).copied().unwrap_or(0.0)
                                - waveform.get(left).copied().unwrap_or(0.0))
                                * frac;
                        amp
                    }
                } else {
                    let t = if count > 1 {
                        index as f32 / (count as f32 - 1.0)
                    } else {
                        0.0
                    };
                    let mut src_beat = (offset_beats + t * clip_len) / time_mul;
                    if source_beats > 0.0 {
                        src_beat = src_beat.rem_euclid(source_beats);
                    }
                    let src_pos = if source_beats > 0.0 {
                        (src_beat / source_beats) * (count as f32 - 1.0)
                    } else {
                        index as f32
                    };
                    let left = src_pos.floor().clamp(0.0, (count - 1) as f32) as usize;
                    let right = (left + 1).min(count - 1);
                    let frac = src_pos - left as f32;
                    let amp = waveform
                        .get(left)
                        .copied()
                        .unwrap_or(0.0)
                        + (waveform.get(right).copied().unwrap_or(0.0)
                            - waveform.get(left).copied().unwrap_or(0.0))
                            * frac;
                    amp
                };
                let x = rect.left() + index as f32 * step;
                let amp = amp.clamp(0.0, 1.0) * rect.height() * 0.45;
                let top = mid_y - amp;
                let bottom = mid_y + amp;
                let color = if let Some(bands) = waveform_color {
                    let (low, mid, high) = if let Some((row_left, beat_width)) = timeline {
                        let x = rect.left() + index as f32 * step;
                        let beat = (x - row_left) / beat_width;
                        let local_beat = beat - clip.start_beats;
                        if local_beat < 0.0 || local_beat > clip_len {
                            (0.0, 0.0, 0.0)
                        } else {
                            let mut src_beat = (offset_beats + local_beat) / time_mul;
                            if source_beats > 0.0 {
                                src_beat = src_beat.rem_euclid(source_beats);
                            }
                            let src_pos = if source_beats > 0.0 {
                                (src_beat / source_beats) * (bands.len() as f32 - 1.0)
                            } else {
                                index as f32
                            };
                            let left = src_pos.floor().clamp(0.0, (bands.len() - 1) as f32) as usize;
                            let right = (left + 1).min(bands.len() - 1);
                            let frac = src_pos - left as f32;
                            let l = bands[left];
                            let r = bands[right];
                            (
                                l[0] + (r[0] - l[0]) * frac,
                                l[1] + (r[1] - l[1]) * frac,
                                l[2] + (r[2] - l[2]) * frac,
                            )
                        }
                    } else {
                        let t = if bands.len() > 1 {
                            index as f32 / (bands.len() as f32 - 1.0)
                        } else {
                            0.0
                        };
                        let mut src_beat = (offset_beats + t * clip_len) / time_mul;
                        if source_beats > 0.0 {
                            src_beat = src_beat.rem_euclid(source_beats);
                        }
                        let src_pos = if source_beats > 0.0 {
                            (src_beat / source_beats) * (bands.len() as f32 - 1.0)
                        } else {
                            t * (bands.len() as f32 - 1.0)
                        };
                        let left = src_pos.floor().clamp(0.0, (bands.len() - 1) as f32) as usize;
                        let right = (left + 1).min(bands.len() - 1);
                        let frac = src_pos - left as f32;
                        let l = bands[left];
                        let r = bands[right];
                        (
                            l[0] + (r[0] - l[0]) * frac,
                            l[1] + (r[1] - l[1]) * frac,
                            l[2] + (r[2] - l[2]) * frac,
                        )
                    };
                    let alpha = ((amp / rect.height()) * 220.0 + 30.0).clamp(40.0, 230.0) as u8;
                    let r = (low * 255.0).clamp(0.0, 255.0) as u8;
                    let g = (high * 255.0).clamp(0.0, 255.0) as u8;
                    let b = (mid * 255.0).clamp(0.0, 255.0) as u8;
                    egui::Color32::from_rgba_premultiplied(r, g, b, alpha)
                } else {
                    egui::Color32::from_rgba_premultiplied(200, 220, 255, 200)
                };
                painter.line_segment(
                    [egui::pos2(x, top), egui::pos2(x, bottom)],
                    egui::Stroke::new(1.0, color),
                );
            }
            return;
        }
        let step = (rect.width() / 48.0).max(4.0);
        let mut x = rect.left();
        let mut points = Vec::new();
        let seed_f = (seed as f32 * 13.7).sin().abs().max(0.2);
        while x <= rect.right() {
            let t = (x - rect.left()) / rect.width() * std::f32::consts::TAU * 3.0;
            let amp = (t.sin() * 0.6 + (t * 0.5 + seed_f).sin() * 0.4) * rect.height() * 0.25;
            points.push(egui::pos2(x, mid_y + amp));
            x += step;
        }
        painter.add(egui::Shape::line(
            points,
            egui::Stroke::new(1.2, egui::Color32::from_rgba_premultiplied(255, 255, 255, 180)),
        ));
    }

    pub(crate) fn resolve_clip_audio_path(&self, clip: &Clip) -> Option<PathBuf> {
        let rel = clip.audio_path.as_ref()?;
        let path = PathBuf::from(rel);
        if path.is_absolute() {
            return Some(path);
        }
        if !self.project_path.trim().is_empty() {
            return Some(PathBuf::from(self.project_path.trim()).join(rel));
        }
        self.default_project_dir().map(|dir| dir.join(rel))
    }

    pub(crate) fn touch_cache_key(order: &mut VecDeque<Arc<str>>, key: &str) {
        order.retain(|entry| entry.as_ref() != key);
        order.push_back(Arc::from(key));
    }

    pub(crate) fn trim_cache_entries<T>(
        cache: &mut HashMap<Arc<str>, T>,
        order: &mut VecDeque<Arc<str>>,
        max_entries: usize,
    ) {
        if max_entries == 0 {
            return;
        }
        while cache.len() > max_entries {
            let Some(oldest) = order.pop_front() else {
                break;
            };
            cache.remove(&oldest);
        }
    }

    pub(crate) fn get_waveform_for_clip(&self, clip: &Clip) -> Option<Vec<f32>> {
        let path = self.resolve_clip_audio_path(clip)?;
        let key = path.to_string_lossy().to_string();
        {
            let mut cache = self.waveform_cache.borrow_mut();
            if !cache.contains_key(key.as_str()) {
                if let Some(data) = Self::build_waveform(&path, 768) {
                    cache.insert(key.clone().into(), data);
                }
            }
            if cache.contains_key(key.as_str()) {
                let mut order = self.waveform_cache_order.borrow_mut();
                Self::touch_cache_key(&mut order, &key);
                Self::trim_cache_entries(&mut cache, &mut order, WAVEFORM_CACHE_MAX_ENTRIES);
            }
            cache.get(key.as_str()).cloned()
        }
    }

    pub(crate) fn get_waveform_color_for_clip(&self, clip: &Clip) -> Option<Vec<[f32; 3]>> {
        let path = self.resolve_clip_audio_path(clip)?;
        let key = path.to_string_lossy().to_string();
        {
            let mut cache = self.waveform_color_cache.borrow_mut();
            if !cache.contains_key(key.as_str()) {
                if let Some(data) = Self::build_waveform_color(&path, 768) {
                    cache.insert(key.clone().into(), data);
                }
            }
            if cache.contains_key(key.as_str()) {
                let mut order = self.waveform_color_cache_order.borrow_mut();
                Self::touch_cache_key(&mut order, &key);
                Self::trim_cache_entries(
                    &mut cache,
                    &mut order,
                    WAVEFORM_COLOR_CACHE_MAX_ENTRIES,
                );
            }
            cache.get(key.as_str()).cloned()
        }
    }

    pub(crate) fn get_waveform_seconds_for_clip(&self, clip: &Clip) -> Option<f32> {
        let path = self.resolve_clip_audio_path(clip)?;
        let key = path.to_string_lossy().to_string();
        {
            let mut cache = self.waveform_len_seconds_cache.borrow_mut();
            if !cache.contains_key(key.as_str()) {
                if let Some(seconds) = Self::audio_length_seconds(&path) {
                    cache.insert(key.clone().into(), seconds);
                }
            }
            if cache.contains_key(key.as_str()) {
                let mut order = self.waveform_len_seconds_cache_order.borrow_mut();
                Self::touch_cache_key(&mut order, &key);
                Self::trim_cache_entries(
                    &mut cache,
                    &mut order,
                    WAVEFORM_LEN_CACHE_MAX_ENTRIES,
                );
            }
            cache.get(key.as_str()).copied()
        }
    }

    pub(crate) fn build_waveform(path: &Path, buckets: usize) -> Option<Vec<f32>> {
        if path.extension().and_then(|s| s.to_str()).map(|e| !e.eq_ignore_ascii_case("wav")).unwrap_or(true) {
            return None;
        }
        let mut reader = hound::WavReader::open(path).ok()?;
        let spec = reader.spec();
        let channels = spec.channels.max(1) as usize;
        let total_samples = reader.duration() as usize;
        let total_frames = total_samples / channels;
        if total_frames == 0 {
            return None;
        }
        let bucket_count = buckets.max(1).min(total_frames);
        let frames_per_bucket = (total_frames as f32 / bucket_count as f32).ceil() as usize;
        let mut peaks = vec![0.0f32; bucket_count];

        match spec.sample_format {
            hound::SampleFormat::Float => {
                for (index, sample) in reader.samples::<f32>().enumerate() {
                    let sample = sample.ok()?.abs();
                    let frame = index / channels;
                    let bucket = (frame / frames_per_bucket).min(bucket_count - 1);
                    if sample > peaks[bucket] {
                        peaks[bucket] = sample;
                    }
                }
            }
            hound::SampleFormat::Int => {
                if spec.bits_per_sample <= 16 {
                    let max = i16::MAX as f32;
                    for (index, sample) in reader.samples::<i16>().enumerate() {
                        let sample = (sample.ok()? as f32 / max).abs();
                        let frame = index / channels;
                        let bucket = (frame / frames_per_bucket).min(bucket_count - 1);
                        if sample > peaks[bucket] {
                            peaks[bucket] = sample;
                        }
                    }
                } else {
                    let max = i32::MAX as f32;
                    for (index, sample) in reader.samples::<i32>().enumerate() {
                        let sample = (sample.ok()? as f32 / max).abs();
                        let frame = index / channels;
                        let bucket = (frame / frames_per_bucket).min(bucket_count - 1);
                        if sample > peaks[bucket] {
                            peaks[bucket] = sample;
                        }
                    }
                }
            }
        }

        Some(peaks)
    }

    pub(crate) fn build_waveform_color(path: &Path, buckets: usize) -> Option<Vec<[f32; 3]>> {
        if path.extension().and_then(|s| s.to_str()).map(|e| !e.eq_ignore_ascii_case("wav")).unwrap_or(true) {
            return None;
        }
        let mut reader = hound::WavReader::open(path).ok()?;
        let spec = reader.spec();
        let channels = spec.channels.max(1) as usize;
        let sample_rate = spec.sample_rate.max(1) as f32;
        let total_samples = reader.duration() as usize;
        let total_frames = total_samples / channels;
        if total_frames == 0 {
            return None;
        }
        let bucket_count = buckets.max(1).min(total_frames);
        let frames_per_bucket = (total_frames as f32 / bucket_count as f32).ceil() as usize;
        let mut low_sum = vec![0.0f32; bucket_count];
        let mut mid_sum = vec![0.0f32; bucket_count];
        let mut high_sum = vec![0.0f32; bucket_count];
        let mut counts = vec![0u32; bucket_count];

        let low_cut = 200.0;
        let high_cut = 2000.0;
        let alpha_low = (1.0 - (-2.0 * std::f32::consts::PI * low_cut / sample_rate).exp())
            .clamp(0.0, 1.0);
        let alpha_high = (1.0 - (-2.0 * std::f32::consts::PI * high_cut / sample_rate).exp())
            .clamp(0.0, 1.0);

        let mut low = 0.0f32;
        let mut high = 0.0f32;
        let mut frame_index = 0usize;
        let mut frame_sum = 0.0f32;
        let mut frame_count = 0usize;

        let mut push_frame = |frame_value: f32| {
            let x = frame_value;
            low += alpha_low * (x - low);
            high += alpha_high * (x - high);
            let low_band = low;
            let mid_band = (high - low).clamp(-1.0, 1.0);
            let high_band = x - high;
            let bucket = (frame_index / frames_per_bucket).min(bucket_count - 1);
            low_sum[bucket] += low_band * low_band;
            mid_sum[bucket] += mid_band * mid_band;
            high_sum[bucket] += high_band * high_band;
            counts[bucket] += 1;
            frame_index += 1;
        };

        match spec.sample_format {
            hound::SampleFormat::Float => {
                for sample in reader.samples::<f32>() {
                    let sample = sample.ok()?;
                    frame_sum += sample;
                    frame_count += 1;
                    if frame_count == channels {
                        let mono = (frame_sum / channels as f32).clamp(-1.0, 1.0);
                        push_frame(mono);
                        frame_sum = 0.0;
                        frame_count = 0;
                    }
                }
            }
            hound::SampleFormat::Int => {
                if spec.bits_per_sample <= 16 {
                    let max = i16::MAX as f32;
                    for sample in reader.samples::<i16>() {
                        let sample = sample.ok()? as f32 / max;
                        frame_sum += sample;
                        frame_count += 1;
                        if frame_count == channels {
                            let mono = (frame_sum / channels as f32).clamp(-1.0, 1.0);
                            push_frame(mono);
                            frame_sum = 0.0;
                            frame_count = 0;
                        }
                    }
                } else {
                    let max = i32::MAX as f32;
                    for sample in reader.samples::<i32>() {
                        let sample = sample.ok()? as f32 / max;
                        frame_sum += sample;
                        frame_count += 1;
                        if frame_count == channels {
                            let mono = (frame_sum / channels as f32).clamp(-1.0, 1.0);
                            push_frame(mono);
                            frame_sum = 0.0;
                            frame_count = 0;
                        }
                    }
                }
            }
        }

        let mut bands = Vec::with_capacity(bucket_count);
        let mut max_val = 0.001f32;
        for i in 0..bucket_count {
            let count = counts[i].max(1) as f32;
            let low = (low_sum[i] / count).sqrt();
            let mid = (mid_sum[i] / count).sqrt();
            let high = (high_sum[i] / count).sqrt();
            max_val = max_val.max(low.max(mid).max(high));
            bands.push([low, mid, high]);
        }
        for band in &mut bands {
            band[0] = (band[0] / max_val).clamp(0.0, 1.0);
            band[1] = (band[1] / max_val).clamp(0.0, 1.0);
            band[2] = (band[2] / max_val).clamp(0.0, 1.0);
        }
        Some(bands)
    }

    pub(crate) fn beats_to_samples(&self, beats: f32, sample_rate: u32) -> u64 {
        let bpm = self.tempo_bpm.max(1.0);
        let samples_per_beat = sample_rate as f64 * 60.0 / bpm as f64;
        (beats.max(0.0) as f64 * samples_per_beat).round().max(0.0) as u64
    }

    pub(crate) fn build_audio_clip_timeline(&self, sample_rate: u32) -> Vec<AudioClipRender> {
        let mut renders = Vec::new();
        for (track_index, track) in self.tracks.iter().enumerate() {
            for clip in &track.clips {
                if clip.is_midi {
                    continue;
                }
                let Some(path) = self.resolve_clip_audio_path(clip) else {
                    continue;
                };
                let path_str = path.to_string_lossy().to_string();
                let start_samples = self.beats_to_samples(clip.start_beats, sample_rate);
                let length_samples = self.beats_to_samples(clip.length_beats, sample_rate).max(1);
                let offset_samples = self.beats_to_samples(clip.audio_offset_beats, sample_rate);
                let pitch = self.clip_effective_pitch_semitones(clip);
                renders.push(AudioClipRender {
                    clip_id: clip.id,
                    path: path_str,
                    track_index,
                    start_samples,
                    length_samples,
                    offset_samples,
                    gain: clip.audio_gain,
                    time_mul: Self::audio_playback_time_mul(clip, pitch),
                    pitch_semitones: pitch,
                    stretch_mode: clip.audio_stretch_mode,
                    formant_scale: clip.audio_formant_scale,
                });
            }
        }
        renders
    }

    pub(crate) fn preload_audio_clips(&self, cache: &Arc<ParkingMutex<AudioClipCache>>) {
        for track in &self.tracks {
            for clip in &track.clips {
                if clip.is_midi {
                    continue;
                }
                let Some(path) = self.resolve_clip_audio_path(clip) else {
                    continue;
                };
                let key = path.to_string_lossy().to_string();
                let mut guard = cache.lock();
                if guard.get(key.as_str()).is_some() {
                    continue;
                }
                if let Some(data) = Self::load_audio_clip_data(&path) {
                    guard.insert(key.into(), Arc::new(data));
                }
            }
            if let Some(treesynth) = track.treesynth.as_ref() {
                for sample in &treesynth.samples {
                    let key = sample.path.clone();
                    let mut guard = cache.lock();
                    if guard.get(key.as_str()).is_some() {
                        continue;
                    }
                    if let Some(data) = Self::load_audio_clip_data(Path::new(&sample.path)) {
                        guard.insert(key.into(), Arc::new(data));
                    }
                }
            }
        }
    }

    pub(crate) fn load_audio_clip_data(path: &Path) -> Option<AudioClipData> {
        let (samples, channels, sample_rate) = Self::decode_audio_samples(path)?;
        if sample_rate == 0 || channels == 0 {
            return None;
        }
        Some(AudioClipData {
            samples,
            channels,
            sample_rate,
        })
    }

    pub(crate) fn start_audio_preview(&mut self, clip: &Clip) -> Result<(), String> {
        self.stop_audio_preview();
        let path = self
            .resolve_clip_audio_path(clip)
            .ok_or_else(|| "Clip has no audio file".to_string())?;
        let file = std::fs::File::open(&path).map_err(|e| e.to_string())?;
        let reader = BufReader::new(file);
        let (stream, handle) = OutputStream::try_default().map_err(|e| e.to_string())?;
        let sink = Sink::try_new(&handle).map_err(|e| e.to_string())?;
        let source = Decoder::new(reader).map_err(|e| e.to_string())?;
        let source = source.convert_samples::<f32>().amplify(clip.audio_gain.max(0.0));
        let source: Box<dyn Source<Item = f32> + Send> = if self.audio_preview_loop {
            Box::new(source.repeat_infinite())
        } else {
            Box::new(source)
        };
        sink.append(source);
        self.audio_preview_stream = Some(stream);
        self.audio_preview_sink = Some(sink);
        self.audio_preview_clip_id = Some(clip.id);
        Ok(())
    }

    pub(crate) fn stop_audio_preview(&mut self) {
        self.audio_preview_sink = None;
        self.audio_preview_stream = None;
        self.audio_preview_clip_id = None;
    }
}
