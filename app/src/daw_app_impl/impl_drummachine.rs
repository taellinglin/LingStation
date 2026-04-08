use std::fs;
use std::path::{Path, PathBuf};

impl DawApp {
    fn abbreviate_drum_name(stem: &str) -> String {
        let lower = stem.to_ascii_lowercase();
        if lower.contains("hat") {
            if lower.contains("open") {
                return "OH".to_string();
            }
            if lower.contains("closed") || lower.contains("close") {
                return "CH".to_string();
            }
            return "HH".to_string();
        }
        if lower.contains("kick") {
            return "K".to_string();
        }
        if lower.contains("snare") {
            return "S".to_string();
        }
        if lower.contains("clap") {
            return "CL".to_string();
        }
        if lower.contains("rim") {
            return "Rim".to_string();
        }
        if lower.contains("tom") {
            return "Tom".to_string();
        }
        if lower.contains("crash") {
            return "Cr".to_string();
        }
        if lower.contains("ride") {
            return "Rd".to_string();
        }
        if lower.contains("china") {
            return "Ch".to_string();
        }
        if lower.contains("conga") {
            return "Cg".to_string();
        }
        if lower.contains("cowbell") {
            return "Cb".to_string();
        }
        if lower.contains("maraca") {
            return "Mr".to_string();
        }
        if lower.contains("tamb") {
            return "Tb".to_string();
        }
        if stem.len() <= 4 {
            return stem.to_string();
        }
        stem.chars().take(4).collect::<String>()
    }

    pub(crate) fn draw_drummachine_panel(
        &mut self,
        ui: &mut egui::Ui,
        state: &mut DrumMachineState,
        audio_clip_cache: &Arc<ParkingMutex<AudioClipCache>>,
        project_root: Option<&Path>,
        track_index: usize,
    ) -> bool {
        let mut changed = false;
        ui.heading("Drum Machine");
        ui.label("Native 32-pad sampler");
        ui.add_space(4.0);

        let samples_roots = Self::drummachine_samples_roots(project_root);
        ui.horizontal(|ui| {
            if ui.button("Randomize Kit").clicked() {
                if !samples_roots.is_empty() {
                    changed |=
                        Self::randomize_drummachine_kit(state, &samples_roots, audio_clip_cache);
                }
            }
            if ui.button("Randomize Pad").clicked() {
                if !samples_roots.is_empty() {
                    let selected = state.selected_pad.min(state.pads.len().saturating_sub(1));
                    if let Some(pad) = state.pads.get_mut(selected) {
                        if let Some(path) = Self::random_pad_sample_path(
                            selected,
                            pad.path.as_deref(),
                            &samples_roots,
                        ) {
                            Self::apply_pad_sample(pad, path, audio_clip_cache);
                            changed = true;
                        }
                    }
                }
            }
        });

        let prev_bank = state.bank;
        ui.horizontal(|ui| {
            if ui.selectable_label(state.bank == 0, "Bank A").clicked() {
                state.bank = 0;
                changed = true;
            }
            if ui.selectable_label(state.bank == 1, "Bank B").clicked() {
                state.bank = 1;
                changed = true;
            }
        });
        if state.bank != prev_bank {
            let bank_base = state.bank * DRUM_MACHINE_BANK_SIZE;
            if state.selected_pad < bank_base
                || state.selected_pad >= bank_base + DRUM_MACHINE_BANK_SIZE
            {
                state.selected_pad = bank_base.min(state.pads.len().saturating_sub(1));
            }
            if let Some(prev) = self.drum_pad_note_down.take() {
                self.piano_preview_note_off(prev);
            }
        }

        let pad_size = egui::vec2(58.0, 58.0);
        let pad_spacing = egui::vec2(6.0, 6.0);
        let pad_base = (state.bank * DRUM_MACHINE_BANK_SIZE).min(state.pads.len());

        for row in 0..4 {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = pad_spacing;
                for col in 0..4 {
                    let pad_index = pad_base + row * 4 + col;
                    if pad_index >= state.pads.len() {
                        continue;
                    }
                    let pad = &state.pads[pad_index];
                    let (rect, response) =
                        ui.allocate_exact_size(pad_size, egui::Sense::click_and_drag());
                    let painter = ui.painter_at(rect);
                    let is_selected = state.selected_pad == pad_index;
                    let bg = if is_selected {
                        egui::Color32::from_rgb(70, 120, 170)
                    } else {
                        egui::Color32::from_rgb(32, 36, 44)
                    };
                    painter.rect_filled(rect, 6.0, bg);
                    painter.rect_stroke(
                        rect,
                        6.0,
                        egui::Stroke::new(1.0, egui::Color32::from_rgb(80, 90, 110)),
                    );
                    painter.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        &pad.name,
                        egui::TextStyle::Body.resolve(ui.style()),
                        egui::Color32::from_gray(230),
                    );

                    if response.clicked() {
                        state.selected_pad = pad_index;
                        changed = true;
                        self.selected_track = Some(track_index);
                        let note = DRUM_MACHINE_BASE_NOTE.saturating_add(pad_index as u8);
                        let vel = if let Some(pos) = response.interact_pointer_pos() {
                            let center = rect.center();
                            let dx = pos.x - center.x;
                            let dy = pos.y - center.y;
                            let radius = 0.5 * rect.width().min(rect.height());
                            let dist = (dx * dx + dy * dy).sqrt().min(radius);
                            let radial = (1.0 - (dist / radius)).clamp(0.0, 1.0);
                            let sensitivity = pad.sensitivity.clamp(0.1, 2.0);
                            let shaped = radial.powf(1.0 / sensitivity);
                            (shaped * 127.0).round().clamp(1.0, 127.0) as u8
                        } else {
                            100
                        };
                        if let Some(prev) = self.drum_pad_note_down.take() {
                            self.piano_preview_note_off(prev);
                        }
                        self.piano_preview_note_on(note, vel);
                        self.drum_pad_note_down = Some(note);
                    }
                }
            });
            ui.add_space(6.0);
        }

        if ui.input(|i| i.pointer.any_released()) {
            if let Some(prev) = self.drum_pad_note_down.take() {
                self.piano_preview_note_off(prev);
            }
        }

        let selected = state.selected_pad.min(state.pads.len().saturating_sub(1));
        let pad = &mut state.pads[selected];
        ui.add_space(6.0);
        ui.separator();
        ui.label(format!("Pad {}", pad.name));
        ui.horizontal(|ui| {
            ui.label("Name");
            if ui.text_edit_singleline(&mut pad.name).changed() {
                changed = true;
            }
        });
        ui.horizontal(|ui| {
            ui.label("Sample");
            let label = pad
                .path
                .as_ref()
                .and_then(|p| Path::new(p).file_name().and_then(|s| s.to_str()))
                .unwrap_or("None");
            ui.label(label);
            if ui.button("Load").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Audio", &["wav", "flac", "ogg"])
                    .pick_file()
                {
                    let path = Self::normalize_windows_path(&path);
                    let final_path = if let Some(project_root) = project_root {
                        Self::import_drummachine_sample_to_project(&path, project_root)
                            .unwrap_or(path)
                    } else {
                        path
                    };
                    pad.path = Some(final_path.to_string_lossy().to_string());
                    if let Some(stem) = final_path.file_stem().and_then(|s| s.to_str()) {
                        if pad.name.starts_with('A') || pad.name.starts_with('B') {
                            pad.name = Self::abbreviate_drum_name(stem);
                        }
                    }
                    let mut cache = audio_clip_cache.lock();
                    let key = pad.path.as_ref().unwrap().clone();
                    if cache.get(key.as_str()).is_none() {
                        if let Some(data) = Self::load_audio_clip_data(Path::new(&key)) {
                            cache.insert(key.into(), Arc::new(data));
                        }
                    }
                    changed = true;
                }
            }
            if ui.button("Clear").clicked() {
                pad.path = None;
                changed = true;
            }
        });

        ui.horizontal(|ui| {
            ui.label("Out");
            changed |= ui
                .add(egui::DragValue::new(&mut pad.output_pair).clamp_range(0..=15))
                .changed();
        });
        changed |= ui.add(egui::Slider::new(&mut pad.gain, 0.0..=2.0).text("Pad Volume")).changed();
        changed |= ui.add(egui::Slider::new(&mut pad.pan, -1.0..=1.0).text("Pad Pan")).changed();
        changed |= ui
            .add(egui::Slider::new(&mut pad.pitch_semitones, -24.0..=24.0).text("Pitch"))
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut pad.attack_ms, 0.0..=200.0).text("Attack ms"))
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut pad.decay_ms, 0.0..=2000.0).text("Decay ms"))
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut pad.sustain, 0.0..=1.0).text("Sustain"))
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut pad.release_ms, 0.0..=2000.0).text("Release ms"))
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut pad.cutoff, 0.0..=1.0).text("Cutoff"))
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut pad.resonance, 0.0..=1.0).text("Resonance"))
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut pad.sensitivity, 0.1..=2.0).text("Sensitivity"))
            .changed();

        ui.add_space(6.0);
        ui.separator();
        ui.label("Global");
        changed |= ui.add(egui::Slider::new(&mut state.gain, 0.0..=2.0).text("Gain")).changed();
        changed |= ui.add(egui::Slider::new(&mut state.pan, -1.0..=1.0).text("Pan")).changed();
        changed |= ui.add(egui::Slider::new(&mut state.cutoff, 0.0..=1.0).text("Cutoff")).changed();
        changed |= ui
            .add(egui::Slider::new(&mut state.resonance, 0.0..=1.0).text("Resonance"))
            .changed();

        changed
    }

    pub(crate) fn import_drummachine_sample_to_project(
        source_file: &Path,
        project_root: &Path,
    ) -> Result<PathBuf, String> {
        let source_file = Self::normalize_windows_path(source_file);
        if !source_file.exists() {
            return Err("Drum machine source file not found".to_string());
        }
        let project_root = Self::normalize_windows_path(project_root);
        let samples_root = project_root.join("assets").join("samples").join("drummachine");
        fs::create_dir_all(&samples_root).map_err(|e| e.to_string())?;

        let file_name = source_file
            .file_name()
            .and_then(|s| s.to_str())
            .map(Self::sanitize_folder_name)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "DrumSample.wav".to_string());

        let mut dest = samples_root.join(&file_name);
        let mut suffix = 1usize;
        while dest.exists() && !Self::paths_equal(&dest, &source_file) {
            let stem = Path::new(&file_name)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("DrumSample");
            let ext = Path::new(&file_name)
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("wav");
            dest = samples_root.join(format!("{}-{}.{}", stem, suffix, ext));
            suffix += 1;
        }

        if !Self::paths_equal(&dest, &source_file) {
            fs::copy(&source_file, &dest).map_err(|e| e.to_string())?;
        }
        Ok(dest)
    }

    #[allow(dead_code)]
    pub(crate) fn apply_default_drummachine_kit(&mut self, track_index: usize) {
        let Some(track) = self.tracks.get_mut(track_index) else {
            return;
        };
        let Some(state) = track.drum_machine.as_mut() else {
            return;
        };
        if state.pads.iter().any(|pad| pad.path.is_some()) {
            return;
        }
        let Some(samples_root) = Self::drummachine_samples_root(None) else {
            return;
        };
        if Self::populate_drummachine_from_samples(state, &samples_root) {
            Self::resolve_and_preload_drummachine_state(state, &self.engine.audio_cache);
            if let Some(audio_state) = self.engine.track_audio.get_mut(track_index) {
                audio_state.sync_drum_machine(track, true);
            }
        }
    }

    #[allow(dead_code)]
    fn drummachine_samples_root(project_root: Option<&Path>) -> Option<PathBuf> {
        Self::drummachine_samples_roots(project_root)
            .into_iter()
            .find(|candidate| candidate.exists())
    }

    pub(crate) fn drummachine_samples_roots(project_root: Option<&Path>) -> Vec<PathBuf> {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(root) = project_root {
            let normalized = Self::normalize_windows_path(root);
            let base = if normalized.is_dir() {
                normalized
            } else {
                normalized.parent().map(Path::to_path_buf).unwrap_or(normalized)
            };
            candidates.push(base.join("samples"));
            candidates.push(base.join("assets").join("samples"));
        }
        candidates.push(PathBuf::from("samples"));
        candidates.push(PathBuf::from("assets").join("samples"));
        if let Ok(cwd) = std::env::current_dir() {
            candidates.push(cwd.join("samples"));
            candidates.push(cwd.join("assets").join("samples"));
            candidates.push(cwd.join("target").join("debug").join("samples"));
            candidates.push(cwd.join("target").join("release").join("samples"));
            if let Some(parent) = cwd.parent() {
                candidates.push(parent.join("samples"));
                candidates.push(parent.join("assets").join("samples"));
                candidates.push(parent.join("target").join("debug").join("samples"));
                candidates.push(parent.join("target").join("release").join("samples"));
            }
        }
        candidates
    }

    fn drummachine_samples_root_for_category(roots: &[PathBuf], folder: &str) -> Option<PathBuf> {
        for root in roots {
            let dir = root.join(folder);
            if dir.exists() {
                return Some(root.clone());
            }
        }
        None
    }

    fn drummachine_category_map() -> [(&'static str, &'static str, usize); 16] {
        [
            ("kick", "kick", 0),
            ("snare", "snare", 1),
            ("hat_closed", "hat_closed", 2),
            ("hat_open", "hat_open", 3),
            ("clap", "clap", 4),
            ("rimshot", "rimshot", 5),
            ("cowbell", "cowbell", 6),
            ("toms", "toms", 7),
            ("crash", "crash", 8),
            ("ride", "ride", 9),
            ("china", "china", 10),
            ("conga", "conga", 11),
            ("maracas", "maracas", 12),
            ("tambourine", "tambourine", 13),
            ("snare", "snare", 14),
            ("kick", "kick", 15),
        ]
    }

    fn randomize_drummachine_kit(
        state: &mut DrumMachineState,
        samples_roots: &[PathBuf],
        audio_clip_cache: &Arc<ParkingMutex<AudioClipCache>>,
    ) -> bool {
        let exts = ["wav", "wave", "aif", "aiff", "flac", "ogg", "mp3"];
        let mut changed = false;
        let seed = Self::random_seed();

        for bank in 0..2usize {
            for (folder, _label, slot) in Self::drummachine_category_map() {
                let index = bank * DRUM_MACHINE_BANK_SIZE + slot;
                if index >= state.pads.len() {
                    continue;
                }
                let Some(root) =
                    Self::drummachine_samples_root_for_category(samples_roots, folder)
                else {
                    continue;
                };
                let dir = root.join(folder);
                if let Some(pad) = state.pads.get_mut(index) {
                    if let Some(path) = Self::random_sample_in_dir_excluding(
                        &dir,
                        &exts,
                        seed ^ index as u64,
                        pad.path.as_deref(),
                    ) {
                        Self::apply_pad_sample(pad, path, audio_clip_cache);
                        changed = true;
                    }
                }
            }
        }

        changed
    }

    fn random_pad_sample_path(
        pad_index: usize,
        current_path: Option<&str>,
        samples_roots: &[PathBuf],
    ) -> Option<PathBuf> {
        let exts = ["wav", "wave", "aif", "aiff", "flac", "ogg", "mp3"];
        let seed = Self::random_seed();
        let slot = pad_index % DRUM_MACHINE_BANK_SIZE;
        let folder = Self::drummachine_category_map()
            .iter()
            .find(|(_, _, idx)| *idx == slot)
            .map(|(folder, _, _)| *folder)
            .unwrap_or("samples");
        let root = match Self::drummachine_samples_root_for_category(samples_roots, folder) {
            Some(root) => root,
            None => {
                return None;
            }
        };
        let dir = root.join(folder);
        Self::random_sample_in_dir_excluding(&dir, &exts, seed ^ (slot as u64), current_path)
    }

    #[allow(dead_code)]
    fn random_sample_in_dir(dir: &Path, exts: &[&str], seed: u64) -> Option<PathBuf> {
        if !dir.exists() {
            return None;
        }
        let mut paths = Vec::new();
        Self::scan_dir_for_exts_static(dir, &mut paths, exts);
        if paths.is_empty() {
            return None;
        }
        paths.sort();
        let pick = (seed % paths.len() as u64) as usize;
        Some(PathBuf::from(&paths[pick]))
    }

    fn random_sample_in_dir_excluding(
        dir: &Path,
        exts: &[&str],
        seed: u64,
        exclude: Option<&str>,
    ) -> Option<PathBuf> {
        if !dir.exists() {
            return None;
        }
        let mut paths = Vec::new();
        Self::scan_dir_for_exts_static(dir, &mut paths, exts);
        if paths.is_empty() {
            return None;
        }
        paths.sort();
        let mut candidates: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
        if candidates.len() == 1 {
            return Some(candidates.remove(0));
        }
        let exclude_path = exclude.map(|p| Self::normalize_windows_path(Path::new(p)));
        let exclude_name = exclude
            .and_then(|p| Path::new(p).file_name().map(|s| s.to_string_lossy().to_string()));
        let current_index = exclude_path.as_ref().and_then(|exclude_path| {
            candidates.iter().position(|candidate| {
                if Self::paths_equal(candidate, exclude_path) {
                    return true;
                }
                if let (Some(candidate_name), Some(ref exclude_name)) = (
                    candidate.file_name().map(|s| s.to_string_lossy().to_string()),
                    exclude_name.as_ref(),
                ) {
                    return candidate_name.eq_ignore_ascii_case(exclude_name);
                }
                false
            })
        });
        if let Some(current_index) = current_index {
            let offset = (seed as usize % (candidates.len() - 1)) + 1;
            let pick = (current_index + offset) % candidates.len();
            return Some(candidates.remove(pick));
        }
        let pick = (seed as usize) % candidates.len();
        Some(candidates.remove(pick))
    }

    fn apply_pad_sample(
        pad: &mut DrumMachinePad,
        path: PathBuf,
        audio_clip_cache: &Arc<ParkingMutex<AudioClipCache>>,
    ) {
        pad.path = Some(path.to_string_lossy().to_string());
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            pad.name = Self::abbreviate_drum_name(stem);
        }
        let mut cache = audio_clip_cache.lock();
        let key = pad.path.as_ref().unwrap().clone();
        if cache.get(key.as_str()).is_none() {
            if let Some(data) = Self::load_audio_clip_data(Path::new(&key)) {
                cache.insert(key.into(), Arc::new(data));
            }
        }
    }

    fn random_seed() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};
        static DRUM_RAND_COUNTER: AtomicU64 = AtomicU64::new(0);
        let counter = DRUM_RAND_COUNTER.fetch_add(1, Ordering::Relaxed);
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        time ^ counter.wrapping_mul(0x9E3779B97F4A7C15)
    }

    #[allow(dead_code)]
    fn populate_drummachine_from_samples(
        state: &mut DrumMachineState,
        samples_root: &Path,
    ) -> bool {
        let exts = ["wav", "wave", "aif", "aiff", "flac", "ogg", "mp3"];
        let mut changed = false;

        let map = [
            ("kick", 0usize),
            ("snare", 1),
            ("hat_closed", 2),
            ("hat_open", 3),
            ("clap", 4),
            ("rimshot", 5),
            ("cowbell", 6),
            ("toms", 7),
            ("crash", 8),
            ("ride", 9),
            ("china", 10),
            ("conga", 11),
            ("maracas", 12),
            ("tambourine", 13),
            ("snare", 14),
            ("kick", 15),
        ];

        for (folder, index) in map {
            if let Some(path) = Self::first_sample_in_dir(samples_root.join(folder), &exts) {
                if let Some(pad) = state.pads.get_mut(index) {
                    pad.path = Some(path.to_string_lossy().to_string());
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        pad.name = Self::abbreviate_drum_name(stem);
                    }
                    changed = true;
                }
            }
        }

        if state.pads.iter().all(|p| p.path.is_some()) {
            return changed;
        }

        let mut all_samples: Vec<String> = Vec::new();
        Self::scan_dir_for_exts_static(samples_root, &mut all_samples, &exts);
        all_samples.sort();
        let mut iter = all_samples.into_iter();
        for pad in &mut state.pads {
            if pad.path.is_some() {
                continue;
            }
            if let Some(path) = iter.next() {
                pad.path = Some(path.clone());
                if let Some(stem) = Path::new(&path).file_stem().and_then(|s| s.to_str()) {
                    pad.name = Self::abbreviate_drum_name(stem);
                }
                changed = true;
            }
        }

        changed
    }

    #[allow(dead_code)]
    fn first_sample_in_dir(dir: PathBuf, exts: &[&str]) -> Option<PathBuf> {
        if !dir.exists() {
            return None;
        }
        let mut paths = Vec::new();
        Self::scan_dir_for_exts_static(&dir, &mut paths, exts);
        paths.sort();
        paths.first().map(|p| PathBuf::from(p))
    }

    pub(crate) fn resolve_drum_sample_path(path: &str) -> Option<PathBuf> {
        let candidate = PathBuf::from(path);
        if candidate.exists() {
            return Some(candidate);
        }
        if candidate.is_absolute() {
            let file_name = candidate.file_name().map(|s| s.to_string_lossy().to_string());
            if let Some(file_name) = file_name {
                for root in Self::drummachine_samples_roots(None) {
                    if let Some(found) = Self::find_sample_by_name(&root, &file_name) {
                        return Some(found);
                    }
                }
            }
            return None;
        }
        for root in Self::drummachine_samples_roots(None) {
            let from_root = root.join(path);
            if from_root.exists() {
                return Some(from_root);
            }
            let file_name = Path::new(path)
                .file_name()
                .map(|s| s.to_string_lossy().to_string());
            if let Some(file_name) = file_name {
                if let Some(found) = Self::find_sample_by_name(&root, &file_name) {
                    return Some(found);
                }
            }
        }
        if let Ok(cwd) = std::env::current_dir() {
            let from_cwd = cwd.join(path);
            if from_cwd.exists() {
                return Some(from_cwd);
            }
        }
        None
    }

    fn find_sample_by_name(root: &Path, file_name: &str) -> Option<PathBuf> {
        let entries = fs::read_dir(root).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(found) = Self::find_sample_by_name(&path, file_name) {
                    return Some(found);
                }
                continue;
            }
            if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                if name.eq_ignore_ascii_case(file_name) {
                    return Some(path);
                }
            }
        }
        None
    }

    pub(crate) fn resolve_and_preload_drummachine_state(
        state: &mut DrumMachineState,
        audio_clip_cache: &Arc<ParkingMutex<AudioClipCache>>,
    ) {
        let mut cache = audio_clip_cache.lock();
        for pad in &mut state.pads {
            let Some(path_str) = pad.path.as_ref() else {
                continue;
            };
            let resolved = Self::resolve_drum_sample_path(path_str)
                .unwrap_or_else(|| PathBuf::from(path_str));
            let resolved_str = resolved.to_string_lossy().to_string();
            if resolved_str != *path_str {
                pad.path = Some(resolved_str.clone());
            }
            if cache.get(resolved_str.as_str()).is_none() {
                if let Some(data) = Self::load_audio_clip_data(Path::new(&resolved_str)) {
                    cache.insert(resolved_str.into(), Arc::new(data));
                }
            }
        }
    }
}
