#[derive(serde::Deserialize, Default)]
struct AiScoreResponse {
    #[serde(default)]
    start_beat: Option<f32>,
    #[serde(default)]
    length_beats: Option<f32>,
    #[serde(default)]
    tracks: Vec<AiScoreTrack>,
}

#[derive(serde::Deserialize, Default)]
struct AiScoreTrack {
    #[serde(default)]
    track_index: Option<usize>,
    #[serde(default)]
    track_name: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    instrumentation: Option<String>,
    #[serde(default)]
    params: Option<serde_json::Value>,
    #[serde(default)]
    notes: Vec<AiScoreNote>,
}

#[derive(serde::Deserialize, Default)]
struct AiScoreNote {
    start: f32,
    length: f32,
    midi: u8,
    #[serde(default)]
    velocity: Option<u8>,
}

impl DawApp {
    pub(crate) fn center_ai_scores(&mut self, ctx: &egui::Context) {
        if let Some(rx) = self.ai_score_job.as_ref() {
            if let Ok(result) = rx.try_recv() {
                self.ai_score_job = None;
                self.ai_score_busy = false;
                match result.response {
                    Ok(content) => {
                        self.ai_score_response = content.clone();
                        let mut apply_err = None;
                        if self.ai_score_auto_apply {
                            match self.parse_ai_score_response(&content) {
                                Ok(parsed) => {
                                    if let Err(err) = self.apply_ai_score_response(&parsed) {
                                        apply_err = Some(err);
                                    }
                                }
                                Err(err) => {
                                    apply_err = Some(err);
                                }
                            }
                        }
                        self.ai_score_journal.push(AiScorePromptEntry {
                            prompt: result.prompt,
                            at_beats: result.at_beats,
                            response: Some(content),
                        });
                        if let Some(err) = apply_err {
                            self.ai_score_status = format!("AI generated, apply failed: {err}");
                        } else if self.ai_score_auto_apply {
                            self.ai_score_status = "AI generated and applied".to_string();
                        } else {
                            self.ai_score_status = "AI score generated".to_string();
                        }
                    }
                    Err(err) => {
                        self.ai_score_status = format!("AI error: {err}");
                    }
                }
            }
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("AI Scores");
            ui.label("Describe a song or list tracks with prompts.");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("vLLM URL");
                ui.text_edit_singleline(&mut self.ai_vllm_url);
            });
            ui.horizontal(|ui| {
                ui.label("Model");
                ui.text_edit_singleline(&mut self.ai_vllm_model);
            });
            ui.horizontal(|ui| {
                ui.label("Temperature");
                ui.add(
                    egui::DragValue::new(&mut self.ai_vllm_temperature)
                        .speed(0.05)
                        .clamp_range(0.0..=2.0),
                );
                ui.label("Max Tokens");
                ui.add(
                    egui::DragValue::new(&mut self.ai_vllm_max_tokens)
                        .speed(16.0)
                        .clamp_range(64.0..=4096.0),
                );
            });
            ui.horizontal(|ui| {
                ui.label("Bars");
                ui.add(
                    egui::DragValue::new(&mut self.ai_score_bars)
                        .speed(1.0)
                        .clamp_range(1.0..=128.0),
                );
                ui.checkbox(&mut self.ai_score_auto_apply, "Auto-apply");
                if ui.button("Apply Last Response").clicked() {
                    if self.ai_score_response.trim().is_empty() {
                        self.ai_score_status = "No response to apply".to_string();
                    } else {
                        match self.parse_ai_score_response(&self.ai_score_response) {
                            Ok(parsed) => match self.apply_ai_score_response(&parsed) {
                                Ok(_) => {
                                    self.ai_score_status = "Applied last response".to_string();
                                }
                                Err(err) => {
                                    self.ai_score_status = format!("Apply failed: {err}");
                                }
                            },
                            Err(err) => {
                                self.ai_score_status = format!("Parse failed: {err}");
                            }
                        }
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label(format!("Current Beat: {:.2}", self.playhead_beats.max(0.0)));
                if ui.button("Insert Track Template").clicked() {
                    if !self.ai_score_prompt.trim().is_empty() {
                        self.ai_score_prompt.push_str("\n\n");
                    }
                    self.ai_score_prompt.push_str("Track 1: \nTrack 2: ");
                }
            });
            ui.label("Prompt");
            let prompt = egui::TextEdit::multiline(&mut self.ai_score_prompt)
                .desired_rows(12)
                .lock_focus(true)
                .hint_text("Track 1: Upbeat jazzy sonata in B major...\nTrack 2: Piano...\n\nSong: Marching band in China");
            ui.add(prompt);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Add to Journal").clicked() {
                    let trimmed = self.ai_score_prompt.trim();
                    if trimmed.is_empty() {
                        self.ai_score_status = "Prompt is empty".to_string();
                    } else {
                        self.ai_score_journal.push(AiScorePromptEntry {
                            prompt: trimmed.to_string(),
                            at_beats: self.playhead_beats.max(0.0),
                            response: None,
                        });
                        self.ai_score_status = "Prompt added to journal".to_string();
                    }
                }
                if ui
                    .add_enabled(!self.ai_score_busy, egui::Button::new("Generate"))
                    .clicked()
                {
                    let trimmed = self.ai_score_prompt.trim();
                    if trimmed.is_empty() {
                        self.ai_score_status = "Prompt is empty".to_string();
                    } else {
                        self.ai_score_status = "Generating...".to_string();
                        self.start_ai_score_job(trimmed.to_string(), self.ai_score_bars);
                    }
                }
                if ui.button("Clear").clicked() {
                    self.ai_score_prompt.clear();
                    self.ai_score_status.clear();
                }
            });
            ui.add_space(10.0);
            ui.separator();
            ui.heading("Prompt Journal");
            ui.label("These entries are saved with the project.");
            egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                if self.ai_score_journal.is_empty() {
                    ui.label("No prompts yet.");
                }
                let mut remove_index: Option<usize> = None;
                for (index, entry) in self.ai_score_journal.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(format!("#{:02} @ {:.2}b", index + 1, entry.at_beats));
                        if ui.button("Remove").clicked() {
                            remove_index = Some(index);
                        }
                    });
                    ui.add(egui::Label::new(entry.prompt.clone()).wrap(true));
                    if let Some(response) = entry.response.as_ref() {
                        ui.add_space(2.0);
                        ui.add(egui::Label::new(response.clone()).wrap(true));
                    }
                    ui.add_space(6.0);
                }
                if let Some(index) = remove_index {
                    if index < self.ai_score_journal.len() {
                        self.ai_score_journal.remove(index);
                    }
                }
            });
            if !self.ai_score_status.is_empty() {
                ui.add_space(6.0);
                ui.label(&self.ai_score_status);
            }
            if !self.ai_score_response.trim().is_empty() {
                ui.add_space(6.0);
                ui.separator();
                ui.heading("Last Response");
                egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                    ui.add(egui::Label::new(self.ai_score_response.clone()).wrap(true));
                });
            }
        });
    }

    fn start_ai_score_job(&mut self, prompt: String, bars: u32) {
        use serde_json::json;
        if self.ai_score_busy {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let url = self.ai_vllm_url.trim().trim_end_matches('/').to_string();
        let model = self.ai_vllm_model.trim().to_string();
        let temperature = self.ai_vllm_temperature;
        let max_tokens = self.ai_vllm_max_tokens;
        let beat = self.playhead_beats.max(0.0);
        let tempo = self.tempo_bpm.max(1.0);
        let bars = bars.max(1);
        let length_beats = bars as f32 * 4.0;
        let track_list = self
            .tracks
            .iter()
            .enumerate()
            .map(|(index, track)| format!("Track {}: {}", index + 1, track.name))
            .collect::<Vec<_>>()
            .join("\n");
        self.ai_score_busy = true;
        self.ai_score_job = Some(rx);
        std::thread::spawn(move || {
            let result = (|| {
                let client = reqwest::blocking::Client::new();
                let system = "You are a music assistant. Return ONLY JSON that matches the schema. Use 1-based track_index values that match the provided track list. Use note times in beats relative to start_beat.";
                let user = format!(
                    "Track list:\n{}\n\nStart beat: {:.2}\nBars: {} (length_beats = {:.2})\nTempo: {:.1} BPM\nPrompt:\n{}\n\nSchema:\n{{\"schema_version\":1,\"start_beat\":<float>,\"length_beats\":<float>,\"tracks\":[{{\"track_index\":<int>,\"track_name\":<string>,\"role\":<string>,\"instrumentation\":<string>,\"params\":<object>,\"notes\":[{{\"start\":<float>,\"length\":<float>,\"midi\":<int>,\"velocity\":<int>}}]}}]}}",
                    track_list, beat, bars, length_beats, tempo, prompt
                );
                let body = json!({
                    "model": model,
                    "messages": [
                        {"role": "system", "content": system},
                        {"role": "user", "content": user}
                    ],
                    "temperature": temperature,
                    "max_tokens": max_tokens
                });
                let endpoint = format!("{}/v1/chat/completions", url);
                let resp = client
                    .post(endpoint)
                    .json(&body)
                    .send()
                    .map_err(|e| e.to_string())?;
                if !resp.status().is_success() {
                    let code = resp.status();
                    let text = resp.text().unwrap_or_default();
                    return Err(format!("vLLM error {code}: {text}"));
                }
                let value: serde_json::Value = resp.json().map_err(|e| e.to_string())?;
                let content = value
                    .get("choices")
                    .and_then(|c| c.get(0))
                    .and_then(|c| c.get("message"))
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_string();
                Ok(content)
            })();
            let _ = tx.send(AiScoreJobResult {
                prompt,
                at_beats: beat,
                response: result,
            });
        });
    }

    fn parse_ai_score_response(&self, content: &str) -> Result<AiScoreResponse, String> {
        let Some(start) = content.find('{') else {
            return Err("No JSON object found in response".to_string());
        };
        let Some(end) = content.rfind('}') else {
            return Err("No JSON object found in response".to_string());
        };
        if end <= start {
            return Err("Invalid JSON bounds".to_string());
        }
        let json_str = &content[start..=end];
        serde_json::from_str::<AiScoreResponse>(json_str)
            .map_err(|e| format!("JSON parse error: {e}"))
    }

    fn apply_ai_score_response(&mut self, response: &AiScoreResponse) -> Result<usize, String> {
        if response.tracks.is_empty() {
            return Err("Response has no tracks".to_string());
        }
        let start_beat = response
            .start_beat
            .unwrap_or(self.playhead_beats.max(0.0));
        let default_len = (self.ai_score_bars.max(1) as f32) * 4.0;
        let length_beats = response.length_beats.unwrap_or(default_len).max(0.25);
        let mut applied_tracks = 0usize;

        for entry in &response.tracks {
            let track_index = self.resolve_ai_track_index(entry)?;
            if track_index >= self.tracks.len() {
                return Err(format!("Track {} out of range", track_index + 1));
            }
            if entry.notes.is_empty() {
                continue;
            }
            let mut notes = Vec::new();
            let mut max_note_end = start_beat;
            for note in &entry.notes {
                let rel_start = note.start.max(0.0);
                let length = note.length.max(0.05);
                let abs_start = start_beat + rel_start;
                let abs_end = abs_start + length;
                max_note_end = max_note_end.max(abs_end);
                let velocity = note.velocity.unwrap_or(100).min(127);
                let midi = note.midi.min(127);
                notes.push(PianoRollNote::new(abs_start, length, midi, velocity));
            }
            if notes.is_empty() {
                continue;
            }
            let clip_len = (length_beats.max(max_note_end - start_beat)).max(0.25);
            let clip_id = self.next_clip_id();
            let clip = Clip {
                id: clip_id,
                track: track_index,
                start_beats: start_beat,
                length_beats: clip_len,
                is_midi: true,
                midi_notes: notes,
                midi_source_beats: Some(clip_len),
                link_id: None,
                name: entry
                    .role
                    .clone()
                    .unwrap_or_else(|| "AI Score".to_string()),
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
            };
            if let Some(track) = self.tracks.get_mut(track_index) {
                track.clips.push(clip);
            }
            self.rebuild_track_midi_notes(track_index);
            applied_tracks += 1;
        }
        if applied_tracks == 0 {
            return Err("No notes applied".to_string());
        }
        self.mark_dirty();
        Ok(applied_tracks)
    }

    fn resolve_ai_track_index(&self, entry: &AiScoreTrack) -> Result<usize, String> {
        if let Some(index) = entry.track_index {
            if index == 0 {
                return Ok(0);
            }
            return Ok(index.saturating_sub(1));
        }
        if let Some(name) = entry.track_name.as_ref() {
            let needle = name.trim().to_ascii_lowercase();
            if !needle.is_empty() {
                if let Some((index, _)) = self
                    .tracks
                    .iter()
                    .enumerate()
                    .find(|(_, track)| track.name.to_ascii_lowercase() == needle)
                {
                    return Ok(index);
                }
                if let Some((index, _)) = self
                    .tracks
                    .iter()
                    .enumerate()
                    .find(|(_, track)| track.name.to_ascii_lowercase().contains(&needle))
                {
                    return Ok(index);
                }
            }
        }
        self.selected_track.ok_or_else(|| "No track mapping found".to_string())
    }
}
