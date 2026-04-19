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
        self.update_vllm_server_status();
        if self.ai_model_download_busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        }
        if self.ai_vllm_process.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(300));
        }
        if let Some(rx) = self.ai_model_download_job.as_ref() {
            let mut events = Vec::new();
            while let Ok(event) = rx.try_recv() {
                events.push(event);
            }
            let mut finished = false;
            for event in events {
                match event {
                    ModelDownloadEvent::Progress { downloaded, total } => {
                        self.ai_model_download_progress = Some((downloaded, total));
                    }
                    ModelDownloadEvent::Finished(result) => {
                        finished = true;
                        self.ai_model_download_busy = false;
                        match result {
                            Ok(path) => {
                                self.ai_model_download_status = format!("Download complete: {path}");
                                self.ai_model_download_progress = None;
                                self.refresh_ai_model_candidates();
                            }
                            Err(err) => {
                                self.ai_model_download_status = err;
                            }
                        }
                    }
                }
            }
            if finished {
                self.ai_model_download_job = None;
            }
        }
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
            egui::ScrollArea::vertical()
                .id_source("ai_scores_root_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
            ui.heading("AI Scores");
            ui.label("Describe a song or list tracks with prompts.");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("Backend");
                egui::ComboBox::from_id_source("ai_backend_selector")
                    .selected_text(if self.ai_backend == AiBackend::VLLm { "vLLM" } else { "Transformers" })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.ai_backend, AiBackend::VLLm, "vLLM");
                        ui.selectable_value(&mut self.ai_backend, AiBackend::Transformers, "Transformers");
                    });
            });
            ui.horizontal(|ui| {
                ui.label("Device");
                egui::ComboBox::from_id_source("ai_torch_device_selector")
                    .selected_text(if self.ai_torch_device == TorchDevice::Cuda { "CUDA (GPU)" } else { "CPU" })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.ai_torch_device, TorchDevice::Cuda, "CUDA (GPU)");
                        ui.selectable_value(&mut self.ai_torch_device, TorchDevice::Cpu, "CPU");
                    });
                if self.ai_torch_device == TorchDevice::Cpu {
                    ui.label(egui::RichText::new("⚠ CPU will be slow").weak().small().color(egui::Color32::YELLOW));
                }
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("URL");
                ui.text_edit_singleline(&mut self.ai_vllm_url);
            });
            ui.horizontal(|ui| {
                let is_starting = self.ai_vllm_process.is_some() && !self.ai_vllm_online;
                let status_color = if self.ai_vllm_online {
                    egui::Color32::from_rgb(64, 200, 120)
                } else if is_starting {
                    egui::Color32::from_rgb(230, 180, 80)
                } else {
                    egui::Color32::from_rgb(230, 105, 105)
                };
                let status_text = if self.ai_vllm_online {
                    "Online"
                } else if is_starting {
                    "Starting"
                } else {
                    "Offline"
                };
                ui.label(
                    egui::RichText::new(format!("Server: {status_text}"))
                        .color(status_color)
                        .strong(),
                );
                let backend_name = if self.ai_backend == AiBackend::VLLm { "vLLM" } else { "Transformers" };
                if ui
                    .add_enabled(self.ai_vllm_process.is_none(), egui::Button::new(format!("Start {backend_name}")))
                    .clicked()
                {
                    self.start_vllm_server();
                }
                if ui
                    .add_enabled(self.ai_vllm_process.is_some(), egui::Button::new(format!("Stop {backend_name}")))
                    .clicked()
                {
                    self.stop_vllm_server();
                }
                if ui.button("Refresh Status").clicked() {
                    self.ai_vllm_last_probe = None;
                    self.update_vllm_server_status();
                }
            });
            if !self.ai_vllm_status.trim().is_empty() {
                ui.label(&self.ai_vllm_status);
            }
            if self.ai_backend == AiBackend::VLLm && self.ai_vllm_process.is_some() {
                ui.label(egui::RichText::new("ℹ vLLM requires GPU. Ensure Docker GPU support is enabled: docker run --gpus all").weak().small());
            }
            ui.horizontal(|ui| {
                ui.label("Model");
                ui.text_edit_singleline(&mut self.ai_vllm_model);
            });
            if self.ai_model_candidates.is_empty() {
                self.refresh_ai_model_candidates();
            }
            ui.horizontal(|ui| {
                let selected_label = if self.ai_vllm_model.trim().is_empty() {
                    "Select model".to_string()
                } else {
                    self.ai_vllm_model.clone()
                };
                let model_candidates = self.ai_model_candidates.clone();
                egui::ComboBox::from_id_source("ai_model_selector")
                    .selected_text(selected_label)
                    .show_ui(ui, |ui| {
                        for model in &model_candidates {
                            if ui.selectable_label(self.ai_vllm_model == *model, model).clicked() {
                                self.ai_vllm_model = model.clone();
                            }
                        }
                    });
                if ui.button("Refresh Models").clicked() {
                    self.refresh_ai_model_candidates();
                }
            });
            ui.horizontal(|ui| {
                ui.label("Download URL");
                ui.text_edit_singleline(&mut self.ai_model_download_url);
                if ui
                    .add_enabled(
                        !self.ai_model_download_busy,
                        egui::Button::new("Download Model"),
                    )
                    .clicked()
                {
                    self.start_ai_model_download();
                }
            });
            if let Some((downloaded, total)) = self.ai_model_download_progress {
                let progress = match total {
                    Some(total) if total > 0 => (downloaded as f32 / total as f32).clamp(0.0, 1.0),
                    _ => 0.0,
                };
                let text = match total {
                    Some(total) => format!("{} / {} bytes", downloaded, total),
                    None => format!("{} bytes", downloaded),
                };
                ui.add(egui::ProgressBar::new(progress).show_percentage().text(text));
            }
            if !self.ai_model_download_status.trim().is_empty() {
                ui.label(&self.ai_model_download_status);
            }
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
            egui::ScrollArea::vertical()
                .id_source("ai_scores_prompt_journal_scroll")
                .max_height(240.0)
                .show(ui, |ui| {
                if self.ai_score_journal.is_empty() {
                    ui.label("No prompts yet.");
                }
                let mut remove_index: Option<usize> = None;
                for (index, entry) in self.ai_score_journal.iter().enumerate() {
                    ui.push_id(index, |ui| {
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
                    });
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
                egui::ScrollArea::vertical()
                    .id_source("ai_scores_last_response_scroll")
                    .max_height(180.0)
                    .show(ui, |ui| {
                    ui.add(egui::Label::new(self.ai_score_response.clone()).wrap(true));
                });
            }
            });
        });
    }

    fn refresh_ai_model_candidates(&mut self) {
        let mut models = Vec::new();
        if !self.ai_vllm_model.trim().is_empty() {
            models.push(self.ai_vllm_model.clone());
        }

        if let Ok(cwd) = std::env::current_dir() {
            let mut scan_dirs = vec![cwd.clone()];
            scan_dirs.push(cwd.join("models"));
            for dir in scan_dirs {
                let entries = match std::fs::read_dir(&dir) {
                    Ok(entries) => entries,
                    Err(_) => continue,
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    let file_type = match entry.file_type() {
                        Ok(file_type) => file_type,
                        Err(_) => continue,
                    };
                    if file_type.is_dir() {
                        if !path.join("config.json").exists() {
                            continue;
                        }
                    } else if file_type.is_file() {
                        let ext = path
                            .extension()
                            .and_then(|value| value.to_str())
                            .unwrap_or("")
                            .to_ascii_lowercase();
                        if ext != "gguf" {
                            continue;
                        }
                    } else {
                        continue;
                    }

                    let label = path
                        .strip_prefix(&cwd)
                        .ok()
                        .map(|value| value.to_string_lossy().replace('\\', "/"))
                        .unwrap_or_else(|| path.to_string_lossy().replace('\\', "/"));
                    if !models.iter().any(|existing| existing == &label) {
                        models.push(label);
                    }
                }
            }
        }

        self.ai_model_candidates = models;
    }

    fn start_ai_model_download(&mut self) {
        if self.ai_model_download_busy {
            return;
        }
        let source = self.ai_model_download_url.trim().to_string();
        if source.is_empty() {
            self.ai_model_download_status = "Enter a model download URL".to_string();
            return;
        }

        let Some(filename) = source
            .split('/')
            .filter(|segment| !segment.trim().is_empty())
            .next_back()
            .map(|segment| segment.to_string())
        else {
            self.ai_model_download_status = "Could not infer filename from URL".to_string();
            return;
        };

        let base = match std::env::current_dir() {
            Ok(path) => path,
            Err(err) => {
                self.ai_model_download_status = format!("Download path unavailable: {err}");
                return;
            }
        };
        let target_dir = base.join("models");
        let target = target_dir.join(filename);

        let (tx, rx) = std::sync::mpsc::channel();
        self.ai_model_download_job = Some(rx);
        self.ai_model_download_busy = true;
        self.ai_model_download_progress = Some((0, None));
        self.ai_model_download_status = format!("Downloading to {}", target.to_string_lossy());

        std::thread::spawn(move || {
            let result = (|| -> Result<String, String> {
                std::fs::create_dir_all(&target_dir).map_err(|e| format!("Create dir failed: {e}"))?;
                let client = reqwest::blocking::Client::new();
                let mut response = client
                    .get(&source)
                    .send()
                    .map_err(|e| format!("Download request failed: {e}"))?;
                if !response.status().is_success() {
                    let status = response.status();
                    let body = response.text().unwrap_or_default();
                    return Err(format!("Download failed ({status}): {body}"));
                }
                let total = response.content_length();
                let mut file = std::fs::File::create(&target)
                    .map_err(|e| format!("Open output failed: {e}"))?;
                let mut downloaded = 0u64;
                let mut buffer = [0u8; 128 * 1024];
                loop {
                    let n = std::io::Read::read(&mut response, &mut buffer)
                        .map_err(|e| format!("Download stream failed: {e}"))?;
                    if n == 0 {
                        break;
                    }
                    std::io::Write::write_all(&mut file, &buffer[..n])
                        .map_err(|e| format!("Write failed: {e}"))?;
                    downloaded += n as u64;
                    let _ = tx.send(ModelDownloadEvent::Progress { downloaded, total });
                }
                Ok(target.to_string_lossy().to_string())
            })();
            let _ = tx.send(ModelDownloadEvent::Finished(result));
        });
    }

    fn start_vllm_server(&mut self) {
        if self.ai_vllm_process.is_some() {
            self.ai_vllm_status = "Server already running".to_string();
            return;
        }
        let base_url = self.ai_vllm_url.trim().trim_end_matches('/').to_string();
        if !base_url.is_empty() {
            let probe_url = format!("{base_url}/v1/models");
            if let Ok(client) = reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_millis(900))
                .build()
            {
                if let Ok(response) = client.get(&probe_url).send() {
                    if response.status().is_success() {
                        let backend_name = if self.ai_backend == AiBackend::VLLm {
                            "vLLM"
                        } else {
                            "Transformers"
                        };
                        self.ai_vllm_online = true;
                        self.ai_vllm_last_probe = Some(std::time::Instant::now());
                        self.ai_vllm_status = format!(
                            "{backend_name} already running at {base_url} (using existing server)"
                        );
                        return;
                    }
                }
            }
        }
        match self.ai_backend {
            AiBackend::VLLm => self.start_vllm_backend(),
            AiBackend::Transformers => self.start_transformers_backend(),
        }
    }

    fn start_vllm_backend(&mut self) {
        let (host, port) = Self::parse_vllm_host_port(&self.ai_vllm_url);
        let model = self.ai_vllm_model.trim().to_string();
        if model.is_empty() {
            self.ai_vllm_status = "Set model before starting vLLM".to_string();
            return;
        }

        let log_path = Self::vllm_log_path();
        let _ = std::fs::remove_file(&log_path);
        let log_file = match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            Ok(file) => file,
            Err(err) => {
                self.ai_vllm_online = false;
                self.ai_vllm_status = format!("Failed to create vLLM log file: {err}");
                return;
            }
        };
        let log_file_err = match log_file.try_clone() {
            Ok(file) => file,
            Err(err) => {
                self.ai_vllm_online = false;
                self.ai_vllm_status = format!("Failed to prepare vLLM log stream: {err}");
                return;
            }
        };

        if cfg!(windows) {
            let docker_check = std::process::Command::new("docker")
                .arg("--version")
                .status();
            match docker_check {
                Ok(status) if status.success() => {}
                Ok(_) | Err(_) => {
                    self.ai_vllm_online = false;
                    self.ai_vllm_status =
                        "Docker is required on Windows for vLLM. Install/start Docker Desktop."
                            .to_string();
                    return;
                }
            }

            let hf_cache = std::env::var("USERPROFILE")
                .map(|home| format!("{}\\.cache\\huggingface:/root/.cache/huggingface", home))
                .unwrap_or_else(|_| "C:\\Users\\Public\\.cache\\huggingface:/root/.cache/huggingface".to_string());

            let mut command = std::process::Command::new("docker");
            command
                .arg("run")
                .arg("--rm")
                .arg("--gpus")
                .arg("all")
                .arg("-p")
                .arg(format!("{}:8000", port))
                .arg("-v")
                .arg(hf_cache)
                .arg("-e")
                .arg("VLLM_LOGGING_LEVEL=DEBUG")
                .arg("-e")
                .arg("CUDA_VISIBLE_DEVICES=0")
                .arg("vllm/vllm-openai:latest")
                .arg("--host")
                .arg("0.0.0.0")
                .arg("--port")
                .arg("8000")
                .arg("--model")
                .arg(&model)
                .arg("--gpu-memory-utilization")
                .arg("0.9")
                .stdout(std::process::Stdio::from(log_file))
                .stderr(std::process::Stdio::from(log_file_err));

            match command.spawn() {
                Ok(child) => {
                    self.ai_vllm_process = Some(child);
                    self.ai_vllm_online = false;
                    self.ai_vllm_last_probe = None;
                    self.ai_vllm_status = format!(
                        "Starting Docker vLLM on {host}:{port} with {model} (GPU enabled, logs: {})",
                        log_path.to_string_lossy()
                    );
                }
                Err(err) => {
                    self.ai_vllm_online = false;
                    self.ai_vllm_status = format!("Failed to start Docker vLLM: {err}");
                }
            }
            return;
        }

        let python = if std::path::Path::new(".venv/Scripts/python.exe").exists() {
            ".venv/Scripts/python.exe".to_string()
        } else {
            "python".to_string()
        };

        // Fail fast with a clear message when vLLM is not installed.
        let import_check = std::process::Command::new(&python)
            .arg("-c")
            .arg("import vllm")
            .status();
        match import_check {
            Ok(status) if !status.success() => {
                self.ai_vllm_online = false;
                self.ai_vllm_status =
                    "vLLM is not installed in this Python environment (.venv).".to_string();
                return;
            }
            Err(err) => {
                self.ai_vllm_online = false;
                self.ai_vllm_status = format!("Failed to run Python for vLLM check: {err}");
                return;
            }
            _ => {}
        }

        let mut command = std::process::Command::new(&python);
        command
            .arg("-m")
            .arg("vllm.entrypoints.openai.api_server")
            .arg("--host")
            .arg(&host)
            .arg("--port")
            .arg(port.to_string())
            .arg("--model")
            .arg(&model)
            .stdout(std::process::Stdio::from(log_file))
            .stderr(std::process::Stdio::from(log_file_err));

        match command.spawn() {
            Ok(child) => {
                self.ai_vllm_process = Some(child);
                self.ai_vllm_online = false;
                self.ai_vllm_last_probe = None;
                self.ai_vllm_status = format!(
                    "Starting vLLM on {host}:{port} with {model} (logs: {})",
                    log_path.to_string_lossy()
                );
            }
            Err(err) => {
                self.ai_vllm_online = false;
                self.ai_vllm_status = format!("Failed to start vLLM: {err}");
            }
        }
    }

    fn start_transformers_backend(&mut self) {
        let (host, port) = Self::parse_vllm_host_port(&self.ai_vllm_url);
        let model = self.ai_vllm_model.trim().to_string();
        if model.is_empty() {
            self.ai_vllm_status = "Set model before starting Transformers server".to_string();
            return;
        }

        let log_path = Self::transformers_log_path();
        let _ = std::fs::remove_file(&log_path);
        let log_file = match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            Ok(file) => file,
            Err(err) => {
                self.ai_vllm_online = false;
                self.ai_vllm_status = format!("Failed to create Transformers log file: {err}");
                return;
            }
        };
        let log_file_err = match log_file.try_clone() {
            Ok(file) => file,
            Err(err) => {
                self.ai_vllm_online = false;
                self.ai_vllm_status = format!("Failed to prepare Transformers log stream: {err}");
                return;
            }
        };

        let python = if std::path::Path::new(".venv/Scripts/python.exe").exists() {
            ".venv/Scripts/python.exe".to_string()
        } else {
            "python".to_string()
        };

        // Check if transformers is installed
        let import_check = std::process::Command::new(&python)
            .arg("-c")
            .arg("import transformers; import torch; import flask")
            .status();
        match import_check {
            Ok(status) if !status.success() => {
                self.ai_vllm_online = false;
                self.ai_vllm_status =
                    "transformers, torch, or flask not installed. Run: pip install transformers torch flask".to_string();
                return;
            }
            Err(err) => {
                self.ai_vllm_online = false;
                self.ai_vllm_status = format!("Failed to check Python dependencies: {err}");
                return;
            }
            _ => {}
        }

        let script_path = if cfg!(windows) {
            "transformers_server.pyw"
        } else {
            "transformers_server.py"
        };

        // Resolve script path - try relative first, then current directory
        let script_path = {
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let relative_path = cwd.join(script_path);
            if relative_path.exists() {
                relative_path.to_string_lossy().to_string()
            } else {
                script_path.to_string()
            }
        };

        let device_arg = if self.ai_torch_device == TorchDevice::Cuda {
            "cuda"
        } else {
            "cpu"
        };

        let mut command = std::process::Command::new(&python);
        command
            .arg(&script_path)
            .arg("--host")
            .arg(&host)
            .arg("--port")
            .arg(port.to_string())
            .arg("--model")
            .arg(&model)
            .arg("--device")
            .arg(device_arg)
            .stdout(std::process::Stdio::from(log_file))
            .stderr(std::process::Stdio::from(log_file_err));

        match command.spawn() {
            Ok(child) => {
                self.ai_vllm_process = Some(child);
                self.ai_vllm_online = false;
                self.ai_vllm_last_probe = None;
                self.ai_vllm_status = format!(
                    "Starting Transformers server on {host}:{port} with {model} (logs: {})",
                    log_path.to_string_lossy()
                );
            }
            Err(err) => {
                self.ai_vllm_online = false;
                self.ai_vllm_status = format!("Failed to start Transformers server: {}. Check logs: {}", err, log_path.to_string_lossy());
            }
        }
    }

    fn stop_vllm_server(&mut self) {
        if let Some(mut child) = self.ai_vllm_process.take() {
            let _ = child.kill();
            let _ = child.wait();
            self.ai_vllm_online = false;
            self.ai_vllm_last_probe = None;
            self.ai_vllm_status = "Server stopped".to_string();
        } else {
            self.ai_vllm_online = false;
            self.ai_vllm_status = "Server is not running".to_string();
        }
    }

    fn update_vllm_server_status(&mut self) {
        let backend_name = if self.ai_backend == AiBackend::VLLm {
            "vLLM"
        } else {
            "Transformers"
        };
        if let Some(child) = self.ai_vllm_process.as_mut() {
            if let Ok(Some(exit)) = child.try_wait() {
                self.ai_vllm_process = None;
                self.ai_vllm_online = false;
                let log_tail = Self::read_active_log_tail(self.ai_backend);
                if let Some(tail) = log_tail {
                    self.ai_vllm_status = format!("{backend_name} exited with status {exit}: {tail}");
                } else {
                    self.ai_vllm_status = format!("{backend_name} exited with status {exit}");
                }
                return;
            }
        }

        let now = std::time::Instant::now();
        if let Some(last_probe) = self.ai_vllm_last_probe {
            if now.duration_since(last_probe) < std::time::Duration::from_secs(2) {
                return;
            }
        }
        self.ai_vllm_last_probe = Some(now);

        let base_url = self.ai_vllm_url.trim().trim_end_matches('/');
        if base_url.is_empty() {
            self.ai_vllm_online = false;
            self.ai_vllm_status = "Set vLLM URL".to_string();
            return;
        }
        let probe_url = format!("{base_url}/v1/models");
        let client = match reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_millis(700))
            .build()
        {
            Ok(client) => client,
            Err(err) => {
                self.ai_vllm_online = false;
                self.ai_vllm_status = format!("Probe client error: {err}");
                return;
            }
        };

        match client.get(&probe_url).send() {
            Ok(response) if response.status().is_success() => {
                self.ai_vllm_online = true;
                self.ai_vllm_status = format!("{backend_name} online");
            }
            Ok(response) => {
                self.ai_vllm_online = false;
                if self.ai_vllm_process.is_some() {
                    self.ai_vllm_status = if let Some(tail) = Self::read_active_log_tail(self.ai_backend) {
                        format!("{backend_name} starting... {tail}")
                    } else {
                        format!("{backend_name} starting...")
                    };
                } else {
                    self.ai_vllm_status = format!("{backend_name} offline: {}", response.status());
                }
            }
            Err(err) => {
                self.ai_vllm_online = false;
                if self.ai_vllm_process.is_some() {
                    self.ai_vllm_status = if let Some(tail) = Self::read_active_log_tail(self.ai_backend) {
                        format!("{backend_name} starting... {tail}")
                    } else {
                        format!("{backend_name} starting...")
                    };
                } else {
                    self.ai_vllm_status = format!("{backend_name} offline: {err}");
                }
            }
        }
    }

    fn parse_vllm_host_port(url: &str) -> (String, u16) {
        let trimmed = url.trim();
        let without_scheme = trimmed
            .strip_prefix("http://")
            .or_else(|| trimmed.strip_prefix("https://"))
            .unwrap_or(trimmed);
        let authority = without_scheme.split('/').next().unwrap_or("127.0.0.1:8001");
        let mut split = authority.rsplitn(2, ':');
        let port_part = split.next().unwrap_or("8001");
        let host_part = split.next().unwrap_or("127.0.0.1");
        let port = port_part.parse::<u16>().unwrap_or(8001);
        let host = if host_part.trim().is_empty() {
            "127.0.0.1".to_string()
        } else {
            host_part.to_string()
        };
        (host, port)
    }

    fn vllm_log_path() -> std::path::PathBuf {
        std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join("vllm_server.log")
    }

    fn transformers_log_path() -> std::path::PathBuf {
        std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join("transformers_server.log")
    }

    fn read_vllm_log_tail() -> Option<String> {
        let data = std::fs::read_to_string(Self::vllm_log_path()).ok()?;
        data.lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .map(|line| line.trim().to_string())
    }

    fn read_transformers_log_tail() -> Option<String> {
        let data = std::fs::read_to_string(Self::transformers_log_path()).ok()?;
        data.lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .map(|line| line.trim().to_string())
    }

    fn read_active_log_tail(backend: AiBackend) -> Option<String> {
        match backend {
            AiBackend::VLLm => Self::read_vllm_log_tail(),
            AiBackend::Transformers => Self::read_transformers_log_tail(),
        }
    }

    fn start_ai_score_job(&mut self, prompt: String, bars: u32) {
        use serde_json::json;
        if self.ai_score_busy {
            return;
        }
        let url = self.ai_vllm_url.trim().trim_end_matches('/').to_string();
        if url.is_empty() {
            self.ai_score_status = "Set AI server URL before generating".to_string();
            return;
        }
        let backend_name = if self.ai_backend == AiBackend::VLLm {
            "vLLM"
        } else {
            "Transformers"
        };
        let probe_url = format!("{url}/v1/models");
        let probe_client = match reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(2))
            .build()
        {
            Ok(client) => client,
            Err(err) => {
                self.ai_score_status = format!("Failed to create probe client: {err}");
                return;
            }
        };
        match probe_client.get(&probe_url).send() {
            Ok(response) if response.status().is_success() => {}
            Ok(response) => {
                self.ai_score_status = format!(
                    "{backend_name} is not ready yet (status {}). Wait for Server: Online.",
                    response.status()
                );
                return;
            }
            Err(err) => {
                self.ai_score_status = format!(
                    "Cannot reach {backend_name} at {url}. Start server and wait for Online ({err})."
                );
                return;
            }
        }
        let (tx, rx) = std::sync::mpsc::channel();
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
                let client = reqwest::blocking::Client::builder()
                    .connect_timeout(std::time::Duration::from_secs(2))
                    .timeout(std::time::Duration::from_secs(180))
                    .build()
                    .map_err(|e| format!("Failed to create AI request client: {e}"))?;
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
                let mut last_send_err: Option<String> = None;
                let mut resp_opt = None;
                for attempt in 1..=3 {
                    match client.post(&endpoint).json(&body).send() {
                        Ok(resp) => {
                            resp_opt = Some(resp);
                            break;
                        }
                        Err(err) => {
                            last_send_err = Some(err.to_string());
                            if attempt < 3 {
                                std::thread::sleep(std::time::Duration::from_millis(450));
                            }
                        }
                    }
                }
                let resp = match resp_opt {
                    Some(resp) => resp,
                    None => {
                        let detail = last_send_err.unwrap_or_else(|| "unknown send error".to_string());
                        return Err(format!(
                            "AI request failed after retries. Ensure {backend_name} is Online and try again: {detail}"
                        ));
                    }
                };
                if !resp.status().is_success() {
                    let code = resp.status();
                    let text = resp.text().unwrap_or_default();
                    return Err(format!("{backend_name} error {code}: {text}"));
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
        let trimmed = content.trim();
        if (trimmed.contains("\"tracks\"") || trimmed.contains("\"notes\""))
            && (trimmed.matches('{').count() > trimmed.matches('}').count()
                || trimmed.matches('[').count() > trimmed.matches(']').count())
        {
            return Err(
                "Response appears truncated. Increase Max Tokens and generate again.".to_string(),
            );
        }

        let candidates = Self::extract_ai_json_payloads(content);
        if candidates.is_empty() {
            return Err("No JSON payload found in response".to_string());
        }

        for json_str in &candidates {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(json_str) {
                if let Some(parsed) = Self::parse_ai_score_value(value) {
                    return Ok(parsed);
                }
            }
        }

        Err("JSON parse error: expected score object, track array, or single track object".to_string())
    }

    fn parse_ai_score_value(value: serde_json::Value) -> Option<AiScoreResponse> {
        // Preferred schema: { start_beat, length_beats, tracks: [...] }
        if let Ok(parsed) = serde_json::from_value::<AiScoreResponse>(value.clone()) {
            if !parsed.tracks.is_empty() && parsed.tracks.iter().any(|t| !t.notes.is_empty()) {
                return Some(parsed);
            }
        }

        // Compatibility schema: [ { track_index, notes, ... }, ... ]
        if let Ok(tracks) = serde_json::from_value::<Vec<AiScoreTrack>>(value.clone()) {
            if !tracks.is_empty() && tracks.iter().any(|t| !t.notes.is_empty()) {
                return Some(AiScoreResponse {
                    start_beat: None,
                    length_beats: None,
                    tracks,
                });
            }
        }

        // Compatibility schema: { track_index, notes, ... }
        if let Ok(track) = serde_json::from_value::<AiScoreTrack>(value) {
            if !track.notes.is_empty() {
                return Some(AiScoreResponse {
                    start_beat: None,
                    length_beats: None,
                    tracks: vec![track],
                });
            }
        }

        None
    }

    fn extract_ai_json_payloads(content: &str) -> Vec<String> {
        let text = content.trim();
        if text.is_empty() {
            return Vec::new();
        }

        let mut out = Vec::new();

        // 1) Entire response may already be JSON.
        out.push(text.to_string());

        // 2) Pull JSON code fences: ```json ... ``` or plain ``` ... ```.
        let mut fence_scan = text;
        while let Some(start) = fence_scan.find("```") {
            let after_start = &fence_scan[start + 3..];
            let Some(end) = after_start.find("```") else {
                break;
            };
            let block = &after_start[..end];
            let block = block
                .strip_prefix("json")
                .map(|s| s.trim_start_matches(['\r', '\n', ' ']))
                .unwrap_or(block)
                .trim();
            if !block.is_empty() {
                out.push(block.to_string());
            }
            fence_scan = &after_start[end + 3..];
        }

        // 3) Scan balanced {...} and [...] candidates.
        let bytes = text.as_bytes();
        for start in 0..bytes.len() {
            let open = bytes[start] as char;
            if open != '{' && open != '[' {
                continue;
            }
            let close = if open == '{' { '}' } else { ']' };
            let mut depth = 0i32;
            let mut in_string = false;
            let mut escape = false;
            for end in start..bytes.len() {
                let ch = bytes[end] as char;
                if in_string {
                    if escape {
                        escape = false;
                    } else if ch == '\\' {
                        escape = true;
                    } else if ch == '"' {
                        in_string = false;
                    }
                    continue;
                }
                if ch == '"' {
                    in_string = true;
                    continue;
                }
                if ch == open {
                    depth += 1;
                } else if ch == close {
                    depth -= 1;
                    if depth == 0 {
                        out.push(text[start..=end].to_string());
                        break;
                    }
                }
            }
        }

        out.sort();
        out.dedup();
        out
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
