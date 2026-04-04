impl DawApp {
    pub(crate) fn plugin_display_name(path: &str) -> String {
        if Self::is_treesynth_path(path) {
            return "TreeSynth".to_string();
        }
        let candidate = Path::new(path)
            .file_stem()
            .or_else(|| Path::new(path).file_name())
            .and_then(|s| s.to_str())
            .unwrap_or(path);
        candidate.replace('_', " ")
    }

    pub(crate) fn find_vst3_plugin_by_name(&mut self, name: &str) -> Option<String> {
        if self.plugin_candidates.is_empty() {
            self.plugin_candidates = self.scan_plugins();
        }
        let needle = name.to_ascii_lowercase();
        self.plugin_candidates
            .iter()
            .filter(|candidate| candidate.kind == PluginKind::Vst3)
            .find(|candidate| {
                let display = candidate.display.to_ascii_lowercase();
                display == needle || display.contains(&needle)
            })
            .map(|candidate| candidate.path.clone())
    }

    pub(crate) fn apply_program_param(track: &mut Track) {
        let Some(program) = track.midi_program else {
            return;
        };
        let program_index = track.params.iter().position(|name| {
            let name = name.to_ascii_lowercase();
            name.contains("program") || name.contains("patch") || name.contains("preset")
        });
        if let Some(index) = program_index {
            if let Some(value) = track.param_values.get_mut(index) {
                *value = (program as f32 / 127.0).clamp(0.0, 1.0);
            }
        }
    }

    pub(crate) fn presets_root_global(&self) -> PathBuf {
        let base = Path::new(self.settings_path())
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("assets").join("presets")
    }

    pub(crate) fn presets_root_project(&self) -> Option<PathBuf> {
        let trimmed = self.project_path.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(Self::normalize_windows_path(&PathBuf::from(trimmed)).join("assets").join("presets"))
    }

    pub(crate) fn preset_plugin_dir(&self, root: &Path, plugin_path: &str) -> PathBuf {
        let name = Self::plugin_display_name(plugin_path);
        let safe = Self::sanitize_folder_name(&name);
        root.join(safe)
    }

    pub(crate) fn preset_file_path(
        &self,
        root: &Path,
        plugin_path: &str,
        preset_name: &str,
    ) -> Result<PathBuf, String> {
        let safe = Self::sanitize_folder_name(preset_name);
        if safe.trim().is_empty() {
            return Err("Preset name required".to_string());
        }
        let file_name = format!("{}.lingpreset.json", safe);
        Ok(self.preset_plugin_dir(root, plugin_path).join(file_name))
    }

    pub(crate) fn preset_name_for_program(&self, program: u8) -> String {
        format!("{:03} {}", program + 1, gm_program_name(program))
    }

    pub(crate) fn preset_path_for_program(&self, root: &Path, plugin_path: &str, program: u8) -> PathBuf {
        let name = self.preset_name_for_program(program);
        self.preset_plugin_dir(root, plugin_path)
            .join(format!("{}.lingpreset.json", Self::sanitize_folder_name(&name)))
    }

    pub(crate) fn load_preset_for_program(&mut self, index: usize, program: u8) -> Result<(), String> {
        let plugin_path = self
            .tracks
            .get(index)
            .and_then(|t| t.instrument_path.as_deref())
            .ok_or_else(|| "No instrument loaded".to_string())?
            .to_string();

        if let Some(project_root) = self.presets_root_project() {
            let project_path = self.preset_path_for_program(&project_root, &plugin_path, program);
            if project_path.exists() {
                return self.load_preset_from_path(index, &project_path);
            }
        }

        let global_root = self.presets_root_global();
        let global_path = self.preset_path_for_program(&global_root, &plugin_path, program);
        if global_path.exists() {
            return self.load_preset_from_path(index, &global_path);
        }

        self.ensure_gm_presets_for_plugin(&plugin_path, program)?;

        if let Some(project_root) = self.presets_root_project() {
            let project_path = self.preset_path_for_program(&project_root, &plugin_path, program);
            if project_path.exists() {
                return self.load_preset_from_path(index, &project_path);
            }
        }
        let global_root = self.presets_root_global();
        let global_path = self.preset_path_for_program(&global_root, &plugin_path, program);
        if global_path.exists() {
            return self.load_preset_from_path(index, &global_path);
        }

        Err("Preset file not found".to_string())
    }

    pub(crate) fn save_preset_for_track(
        &mut self,
        index: usize,
        root: PathBuf,
        preset_name: &str,
    ) -> Result<String, String> {
        let track = self
            .tracks
            .get(index)
            .ok_or_else(|| "Track not found".to_string())?;
        let plugin_path = track
            .instrument_path
            .as_deref()
            .ok_or_else(|| "No instrument loaded".to_string())?;
        let preset_path = self.preset_file_path(&root, plugin_path, preset_name)?;
        if let Some(parent) = preset_path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        if Self::is_treesynth_path(plugin_path) {
            return self.save_treesynth_preset_at_path(index, &preset_path, preset_name);
        }

        let (mut component_bytes, mut controller_bytes) = (Vec::new(), Vec::new());
        if let Some(host) = self.engine.track_audio.get(index).and_then(|state| state.host.as_ref()) {
            let (component, controller) = host.get_state_bytes();
            component_bytes = component;
            controller_bytes = controller;
        }
        if component_bytes.is_empty() {
            if let Some(bytes) = track.plugin_state_component.as_ref() {
                component_bytes = bytes.clone();
            }
        }
        if controller_bytes.is_empty() {
            if let Some(bytes) = track.plugin_state_controller.as_ref() {
                controller_bytes = bytes.clone();
            }
        }

        let preset = Vst3PresetFile {
            version: 1,
            name: preset_name.to_string(),
            plugin: Self::plugin_display_name(plugin_path),
            param_names: track.params.clone(),
            param_ids: track.param_ids.clone(),
            param_values: track.param_values.clone(),
            component_state: BASE64.encode(&component_bytes),
            controller_state: BASE64.encode(&controller_bytes),
            treesynth: None,
        };

        let json = serde_json::to_string_pretty(&preset).map_err(|e| e.to_string())?;
        fs::write(&preset_path, json).map_err(|e| e.to_string())?;
        Ok(preset_path.to_string_lossy().to_string())
    }

    pub(crate) fn save_treesynth_preset_at_path(
        &mut self,
        index: usize,
        preset_path: &Path,
        preset_name: &str,
    ) -> Result<String, String> {
        let track = self
            .tracks
            .get(index)
            .ok_or_else(|| "Track not found".to_string())?;
        let plugin_path = track
            .instrument_path
            .as_deref()
            .ok_or_else(|| "No instrument loaded".to_string())?;
        if !Self::is_treesynth_path(plugin_path) {
            return Err("TreeSynth preset requires native TreeSynth".to_string());
        }
        let mut state = track
            .treesynth
            .clone()
            .ok_or_else(|| "[TreeSynth] サンプル未ロード: プリセット保存不可".to_string())?;
        if let Some(parent) = preset_path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let samples_dir_name = Self::treesynth_samples_dir_name(preset_name);
        let samples_dir = preset_path
            .parent()
            .ok_or_else(|| "Preset folder missing".to_string())?
            .join(&samples_dir_name);
        fs::create_dir_all(&samples_dir).map_err(|e| e.to_string())?;
        let mut used_names: HashMap<String, usize> = HashMap::new();
        for sample in state.samples.iter_mut() {
            let source = Path::new(&sample.path);
            let file_name = source
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("sample.wav");
            let target_name = Self::treesynth_unique_filename(&mut used_names, file_name);
            let target_path = samples_dir.join(&target_name);
            if !target_path.exists() {
                fs::copy(source, &target_path).map_err(|e| e.to_string())?;
            }
            sample.path = format!("{}/{}", samples_dir_name, target_name);
            sample.name = target_name.clone();
        }
        state.folder = Some(samples_dir_name);
        let preset = Vst3PresetFile {
            version: 1,
            name: preset_name.to_string(),
            plugin: Self::plugin_display_name(plugin_path),
            param_names: Vec::new(),
            param_ids: Vec::new(),
            param_values: Vec::new(),
            component_state: String::new(),
            controller_state: String::new(),
            treesynth: Some(state),
        };
        let json = serde_json::to_string_pretty(&preset).map_err(|e| e.to_string())?;
        fs::write(preset_path, json).map_err(|e| e.to_string())?;
        Ok(preset_path.to_string_lossy().to_string())
    }

    pub(crate) fn load_preset_from_path(&mut self, index: usize, path: &Path) -> Result<(), String> {
        let plugin_path = self
            .tracks
            .get(index)
            .and_then(|track| track.instrument_path.as_deref())
            .ok_or_else(|| "No instrument loaded".to_string())?
            .to_string();
        let data = fs::read_to_string(path).map_err(|e| e.to_string())?;
        let preset: Vst3PresetFile = serde_json::from_str(&data).map_err(|e| e.to_string())?;
        let expected = Self::plugin_display_name(&plugin_path).to_ascii_lowercase();
        let actual = preset.plugin.to_ascii_lowercase();
        if expected != actual {
            return Err("Preset plugin does not match current instrument".to_string());
        }

        if Self::is_treesynth_path(&plugin_path) {
            let mut state = preset
                .treesynth
                .ok_or_else(|| "TreeSynth preset data missing".to_string())?;
            let preset_dir = path
                .parent()
                .ok_or_else(|| "Preset folder missing".to_string())?;
            if let Some(folder) = state.folder.clone() {
                if Path::new(&folder).is_relative() {
                    state.folder = Some(preset_dir.join(&folder).to_string_lossy().to_string());
                }
            }
            for sample in state.samples.iter_mut() {
                let sample_path = Path::new(&sample.path);
                if sample_path.is_relative() {
                    let resolved = preset_dir.join(sample_path);
                    sample.path = resolved.to_string_lossy().to_string();
                }
            }
            if let Some(track) = self.tracks.get_mut(index) {
                track.treesynth = Some(state);
            }
            if let Some(audio) = self.engine.track_audio.get_mut(index) {
                let enabled = Self::is_treesynth_path(&plugin_path);
                if let Some(track) = self.tracks.get(index) {
                    audio.sync_treesynth(track, enabled, &self.engine.audio_cache);
                }
                let mut runtime = audio.treesynth_runtime.lock();
                runtime.voices.clear();
                runtime.sequence_index = 0;
            }
            self.preload_audio_clips(&self.engine.audio_cache);
            return Ok(());
        }

        let component_bytes = if preset.component_state.trim().is_empty() {
            Vec::new()
        } else {
            BASE64
                .decode(preset.component_state.as_bytes())
                .map_err(|e| e.to_string())?
        };
        let controller_bytes = if preset.controller_state.trim().is_empty() {
            Vec::new()
        } else {
            BASE64
                .decode(preset.controller_state.as_bytes())
                .map_err(|e| e.to_string())?
        };

        if let Some(track) = self.tracks.get_mut(index) {
            if !component_bytes.is_empty() {
                track.plugin_state_component = Some(component_bytes.clone());
            }
            if !controller_bytes.is_empty() {
                track.plugin_state_controller = Some(controller_bytes.clone());
            }

            if !preset.param_ids.is_empty() && !preset.param_values.is_empty() {
                if track.param_ids.is_empty() || track.param_ids.len() != track.param_values.len() {
                    track.param_ids = preset.param_ids.clone();
                    track.param_values = preset.param_values.clone();
                } else {
                    let mut map = HashMap::new();
                    for (id, value) in preset.param_ids.iter().zip(preset.param_values.iter()) {
                        map.insert(*id, *value);
                    }
                    if track.param_values.len() != track.param_ids.len() {
                        track.param_values.resize(track.param_ids.len(), 0.0);
                    }
                    for (slot, param_id) in track.param_ids.iter().enumerate() {
                        if let Some(value) = map.get(param_id).copied() {
                            if let Some(target) = track.param_values.get_mut(slot) {
                                *target = value;
                            }
                        }
                    }
                }
            } else if !preset.param_names.is_empty() && !preset.param_values.is_empty() {
                if track.param_values.len() != track.params.len() {
                    track.param_values.resize(track.params.len(), 0.0);
                }
                let mut map = HashMap::new();
                for (name, value) in preset.param_names.iter().zip(preset.param_values.iter()) {
                    map.insert(Self::normalize_param_name(name), *value);
                }
                for (slot, name) in track.params.iter().enumerate() {
                    let key = Self::normalize_param_name(name);
                    if let Some(value) = map.get(&key).copied() {
                        if let Some(target) = track.param_values.get_mut(slot) {
                            *target = value;
                        }
                    }
                }
            }
        }

        if let Some(audio) = self.engine.track_audio.get_mut(index) {
            if let Some(host) = audio.host.as_mut() {
                if !component_bytes.is_empty() || !controller_bytes.is_empty() {
                    let _ = host.set_state_bytes(
                        if component_bytes.is_empty() {
                            None
                        } else {
                            Some(component_bytes.as_slice())
                        },
                        if controller_bytes.is_empty() {
                            None
                        } else {
                            Some(controller_bytes.as_slice())
                        },
                    );
                } else if let Some(track) = self.tracks.get(index) {
                    for (param_id, value) in track.param_ids.iter().zip(track.param_values.iter()) {
                        host.push_param_change(*param_id, *value as f64);
                    }
                }
                if let Some(track) = self.tracks.get_mut(index) {
                    for (slot, param_id) in track.param_ids.iter().enumerate() {
                        if let Some(value) = host.get_param_normalized(*param_id) {
                            if let Some(target) = track.param_values.get_mut(slot) {
                                *target = value as f32;
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    pub(crate) fn normalize_param_name(name: &str) -> String {
        name.to_ascii_lowercase()
            .replace([' ', '_', '-'], "")
    }

    pub(crate) fn debug_param_enabled() -> bool {
        std::env::var("LING_DEBUG_PARAMS")
            .map(|value| {
                let value = value.to_ascii_lowercase();
                value == "1" || value == "true" || value == "yes"
            })
            .unwrap_or(false)
    }

    pub(crate) fn log_fm_ratio_param(&self, track_index: usize, stage: &str) {
        if !Self::debug_param_enabled() {
            return;
        }
        let Some(track) = self.tracks.get(track_index) else {
            return;
        };
        Self::log_fm_ratio_param_from(
            track_index,
            stage,
            &track.params,
            &track.param_ids,
            &track.param_values,
        );
    }

    pub(crate) fn log_fm_ratio_param_from(
        track_index: usize,
        stage: &str,
        params: &[String],
        param_ids: &[u32],
        param_values: &[f32],
    ) {
        if !Self::debug_param_enabled() {
            return;
        }
        let mut found = false;
        for (index, name) in params.iter().enumerate() {
            let key = Self::normalize_param_name(name);
            if key.contains("fmratio") || (key.contains("fm") && key.contains("ratio")) {
                let id = param_ids.get(index).copied().unwrap_or(0);
                let value = param_values.get(index).copied().unwrap_or(0.0);
                log::debug!(
                    "[param-debug] {stage} track={track_index} name={name} id={id} value={value}"
                );
                found = true;
            }
        }
        if !found {
            log::debug!("[param-debug] {stage} track={track_index} fm_ratio not found");
        }
    }

    pub(crate) fn log_all_fm_ratio_params(&self, stage: &str) {
        if !Self::debug_param_enabled() {
            return;
        }
        for index in 0..self.tracks.len() {
            self.log_fm_ratio_param(index, stage);
        }
    }

    pub(crate) fn remap_param_values_by_id_or_name(
        old_ids: &[u32],
        old_names: &[String],
        old_values: &[f32],
        new_params: &[vst3::ParamInfo],
    ) -> Vec<f32> {
        let mut id_map: HashMap<u32, f32> = HashMap::new();
        for (id, value) in old_ids.iter().zip(old_values.iter()) {
            id_map.insert(*id, *value);
        }
        let mut name_map: HashMap<String, f32> = HashMap::new();
        for (name, value) in old_names.iter().zip(old_values.iter()) {
            name_map.insert(Self::normalize_param_name(name), *value);
        }
        new_params
            .iter()
            .map(|p| {
                id_map
                    .get(&p.id)
                    .copied()
                    .or_else(|| name_map.get(&Self::normalize_param_name(&p.name)).copied())
                    .unwrap_or(p.default_value as f32)
            })
            .collect()
    }

    pub(crate) fn remap_param_values_by_id_or_name_clap(
        old_ids: &[u32],
        old_names: &[String],
        old_values: &[f32],
        new_params: &[clap_host::ParamInfo],
    ) -> Vec<f32> {
        let mut id_map: HashMap<u32, f32> = HashMap::new();
        for (id, value) in old_ids.iter().zip(old_values.iter()) {
            id_map.insert(*id, *value);
        }
        let mut name_map: HashMap<String, f32> = HashMap::new();
        for (name, value) in old_names.iter().zip(old_values.iter()) {
            name_map.insert(Self::normalize_param_name(name), *value);
        }
        new_params
            .iter()
            .map(|p| {
                id_map
                    .get(&p.id)
                    .copied()
                    .or_else(|| name_map.get(&Self::normalize_param_name(&p.name)).copied())
                    .unwrap_or(p.default_value as f32)
            })
            .collect()
    }

    pub(crate) fn ensure_gm_presets_for_plugin(
        &mut self,
        plugin_path: &str,
        requested_program: u8,
    ) -> Result<(), String> {
        let params = match Self::plugin_kind_from_path(plugin_path) {
            PluginKind::Native => {
                return Err("Native plugins do not support GM presets".to_string());
            }
            PluginKind::Vst3 => vst3::enumerate_params(plugin_path)?,
            PluginKind::Clap => {
                let clap_id = clap_host::default_plugin_id(plugin_path)?;
                let mut host = clap_host::ClapHost::load(
                    plugin_path,
                    &clap_id,
                    self.settings.sample_rate as f64,
                    self.settings.buffer_size,
                    0,
                    2,
                )?;
                host.enumerate_params()
                    .into_iter()
                    .map(|param| vst3::ParamInfo {
                        id: param.id,
                        name: param.name,
                        default_value: param.default_value,
                    })
                    .collect()
            }
        };
        if params.is_empty() {
            return Err("Preset generation failed: no parameters".to_string());
        }

        let mut roots = Vec::new();
        roots.push(self.presets_root_global());
        if let Some(project_root) = self.presets_root_project() {
            roots.push(project_root);
        }

        for root in &roots {
            let preset_path = self.preset_path_for_program(root, plugin_path, requested_program);
            if preset_path.exists() {
                continue;
            }
            if let Some(parent) = preset_path.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
        }

        let targets: Vec<u8> = (0u8..=127).collect();
        for root in &roots {
            for program in &targets {
                let preset_path = self.preset_path_for_program(root, plugin_path, *program);
                if preset_path.exists() {
                    continue;
                }
                let preset = self.build_gm_preset_file(plugin_path, &params, *program);
                let json = serde_json::to_string_pretty(&preset).map_err(|e| e.to_string())?;
                fs::write(&preset_path, json).map_err(|e| e.to_string())?;
            }
        }

        Ok(())
    }

    pub(crate) fn ensure_builtin_gm_presets(&mut self) {
        if self.gm_presets_generated {
            return;
        }
        let synths = [
            "FishSynth",
            "CatSynth",
            "SannySynth",
            "DogSynth",
            "LingSynth",
            "MiceSynth",
        ];
        for name in synths {
            if let Some(path) = self.find_vst3_plugin_by_name(name) {
                let _ = self.ensure_gm_presets_for_plugin(&path, 0);
            }
        }
        self.gm_presets_generated = true;
    }

    pub(crate) fn build_gm_preset_file(
        &self,
        plugin_path: &str,
        params: &[vst3::ParamInfo],
        program: u8,
    ) -> Vst3PresetFile {
        let mut values: Vec<f32> = Vec::with_capacity(params.len());
        for param in params {
            let value = self.gm_param_value(&param.name, param.default_value as f32, program);
            values.push(value);
        }
        Vst3PresetFile {
            version: 1,
            name: self.preset_name_for_program(program),
            plugin: Self::plugin_display_name(plugin_path),
            param_names: params.iter().map(|p| p.name.clone()).collect(),
            param_ids: params.iter().map(|p| p.id).collect(),
            param_values: values,
            component_state: String::new(),
            controller_state: String::new(),
            treesynth: None,
        }
    }

    pub(crate) fn gm_param_value(&self, name: &str, default_value: f32, program: u8) -> f32 {
        let category = GmCategory::from_program(program);
        let values = GmParamValues::from_category(category);
        let key = Self::normalize_param_name(name);

        if key.contains("preset") || key.contains("program") || key.contains("patch") {
            return (program as f32 / 127.0).clamp(0.0, 1.0);
        }
        if key.contains("gain") || key.contains("volume") || key.contains("master") {
            return values.gain;
        }
        if key.contains("attack") || key.ends_with("atk") || key.contains("_atk") {
            return values.attack;
        }
        if key.contains("decay") || key.ends_with("dec") || key.contains("_dec") {
            return values.decay;
        }
        if key.contains("sustain") || key.ends_with("sus") || key.contains("_sus") {
            return values.sustain;
        }
        if key.contains("release") || key.ends_with("rel") || key.contains("_rel") {
            return values.release;
        }
        if key.contains("cutoff") || key.contains("filtercut") || key.contains("filter_cut") || key.contains("filtercutoff") || key.contains("cut") {
            return values.cutoff;
        }
        if key.contains("resonance") || key.contains("filterres") || key.contains("filter_res") || key.contains("res") {
            return values.resonance;
        }
        if key.contains("vibrato") && key.contains("rate") {
            return values.vibrato_rate;
        }
        if key.contains("vibrato") && (key.contains("int") || key.contains("amount")) {
            return values.vibrato_intensity;
        }
        if key.contains("tremolo") && key.contains("rate") {
            return values.tremolo_rate;
        }
        if key.contains("tremolo") && (key.contains("int") || key.contains("amount")) {
            return values.tremolo_intensity;
        }

        default_value.clamp(0.0, 1.0)
    }

    pub(crate) fn is_micesynth_path(path: &str) -> bool {
        path.to_ascii_lowercase().contains("micesynth")
    }

    pub(crate) fn apply_micesynth_program_from_midi(&mut self, index: usize) {
        let (program, path, params, param_ids, mut param_values, has_state) =
            match self.tracks.get(index) {
                Some(track) => (
                    track.midi_program,
                    track.instrument_path.clone(),
                    track.params.clone(),
                    track.param_ids.clone(),
                    track.param_values.clone(),
                    track
                        .plugin_state_component
                        .as_ref()
                        .map(|v| !v.is_empty())
                        .unwrap_or(false)
                        || track
                            .plugin_state_controller
                            .as_ref()
                            .map(|v| !v.is_empty())
                            .unwrap_or(false),
                ),
                None => return,
            };
        let Some(program) = program else {
            return;
        };
        let Some(path) = path else {
            return;
        };
        if has_state {
            return;
        }
        if !Self::is_micesynth_path(&path) {
            return;
        }
        let program_index = params.iter().position(|name| {
            let name = name.to_ascii_lowercase();
            name.contains("program") || name.contains("patch") || name.contains("preset")
        });
        let Some(program_index) = program_index else {
            return;
        };
        let Some(program_param_id) = param_ids.get(program_index).copied() else {
            return;
        };
        if param_values.len() != param_ids.len() {
            param_values.resize(param_ids.len(), 0.0);
        }
        let normalized = (program as f64 / 127.0).clamp(0.0, 1.0);
        if let Some(value) = param_values.get_mut(program_index) {
            *value = normalized as f32;
        }

        if let Some(host) = self.engine.track_audio.get(index).and_then(|state| state.host.as_ref()) {
            host.push_param_change(program_param_id, normalized);
            for (slot, param_id) in param_ids.iter().enumerate() {
                if let Some(value) = host.get_param_normalized(*param_id) {
                    if let Some(target) = param_values.get_mut(slot) {
                        *target = value as f32;
                    }
                }
            }
        }

        if let Some(track) = self.tracks.get_mut(index) {
            track.param_values = param_values;
        }
    }

    pub(crate) fn scan_dir_for_exts(&self, dir: &Path, out: &mut Vec<String>, exts: &[&str]) {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let matches_ext = exts.iter().any(|e| *e == ext);
            if path.is_dir() {
                if matches_ext {
                    out.push(path.to_string_lossy().to_string());
                    continue;
                }
                self.scan_dir_for_exts(&path, out, exts);
            } else if matches_ext {
                out.push(path.to_string_lossy().to_string());
            }
        }
    }

    pub(crate) fn treesynth_color_from_name(name: &str) -> [u8; 3] {
        let mut hash = 2166136261u32;
        for byte in name.as_bytes() {
            hash ^= u32::from(*byte);
            hash = hash.wrapping_mul(16777619);
        }
        let r = ((hash >> 16) & 0xFF) as u8;
        let g = ((hash >> 8) & 0xFF) as u8;
        let b = (hash & 0xFF) as u8;
        [r, g, b]
    }

    pub(crate) fn treesynth_samples_dir_name(preset_name: &str) -> String {
        let safe = Self::sanitize_folder_name(preset_name);
        format!("{}_samples", safe)
    }

    pub(crate) fn treesynth_unique_filename(
        used: &mut HashMap<String, usize>,
        candidate: &str,
    ) -> String {
        if !used.contains_key(candidate) {
            used.insert(candidate.to_string(), 1);
            return candidate.to_string();
        }
        let count = used.entry(candidate.to_string()).or_insert(1);
        *count += 1;
        let stem = Path::new(candidate)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("sample");
        let ext = Path::new(candidate)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if ext.is_empty() {
            format!("{}_{}", stem, count)
        } else {
            format!("{}_{}.{}", stem, count, ext)
        }
    }

    pub(crate) fn scan_dir_for_exts_static(dir: &Path, out: &mut Vec<String>, exts: &[&str]) {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let matches_ext = exts.iter().any(|e| *e == ext);
            if path.is_dir() {
                if matches_ext {
                    out.push(path.to_string_lossy().to_string());
                    continue;
                }
                Self::scan_dir_for_exts_static(&path, out, exts);
            } else if matches_ext {
                out.push(path.to_string_lossy().to_string());
            }
        }
    }

    pub(crate) fn load_treesynth_folder(folder: &Path) -> Vec<TreeSynthSample> {
        let mut paths = Vec::new();
        Self::scan_dir_for_exts_static(
            folder,
            &mut paths,
            &["wav", "wave", "aif", "aiff", "flac", "ogg", "mp3"],
        );
        paths.sort();
        paths
            .into_iter()
            .map(|path| {
                let name = Path::new(&path)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("sample")
                    .to_string();
                TreeSynthSample {
                    path,
                    name: name.clone(),
                    root_note: 60,
                    gain: 1.0,
                    pan: 0.0,
                    start: 0.0,
                    end: 1.0,
                    color: Self::treesynth_color_from_name(&name),
                }
            })
            .collect()
    }

    pub(crate) fn draw_treesynth_panel(
        ui: &mut egui::Ui,
        state: &mut TreeSynthState,
        audio_clip_cache: &Arc<ParkingMutex<AudioClipCache>>,
        project_root: Option<&Path>,
    ) -> bool {
        let mut changed = false;
        ui.heading("TreeSynth");
        ui.label("Native sampler");
        ui.add_space(4.0);

        let folder_label = state
            .folder
            .clone()
            .unwrap_or_else(|| "None".to_string());
        ui.horizontal(|ui| {
            ui.label("Folder");
            ui.label(&folder_label);
            if ui.button("Choose").clicked() {
                if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                    let folder = Self::normalize_windows_path(&folder);
                    let folder = if let Some(project_root) = project_root {
                        Self::import_treesynth_folder_to_project(&folder, project_root)
                            .unwrap_or(folder)
                    } else {
                        folder
                    };
                    state.folder = Some(folder.to_string_lossy().to_string());
                    state.samples = Self::load_treesynth_folder(&folder);
                    {
                        let mut cache = audio_clip_cache.lock();
                        for sample in &state.samples {
                            if cache.get(sample.path.as_str()).is_some() {
                                continue;
                            }
                            if let Some(data) = Self::load_audio_clip_data(Path::new(&sample.path)) {
                                cache.insert(sample.path.clone().into(), Arc::new(data));
                            }
                        }
                    }
                    changed = true;
                }
            }
            let reload_enabled = state.folder.is_some();
            ui.add_enabled_ui(reload_enabled, |ui| {
                if ui.button("Reload").clicked() {
                    if let Some(folder) = state.folder.as_deref() {
                        let folder = PathBuf::from(folder);
                        state.samples = Self::load_treesynth_folder(&folder);
                        {
                            let mut cache = audio_clip_cache.lock();
                            for sample in &state.samples {
                                if cache.get(sample.path.as_str()).is_some() {
                                    continue;
                                }
                                if let Some(data) = Self::load_audio_clip_data(Path::new(&sample.path)) {
                                    cache.insert(sample.path.clone().into(), Arc::new(data));
                                }
                            }
                        }
                        changed = true;
                    }
                }
            });
            ui.add_enabled_ui(reload_enabled, |ui| {
                if ui.button("Clear").clicked() {
                    state.folder = None;
                    state.samples.clear();
                    changed = true;
                }
            });
        });

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("Mode");
            egui::ComboBox::from_id_source("treesynth_mode")
                .selected_text(format!("{:?}", state.mode))
                .show_ui(ui, |ui| {
                    if ui.selectable_value(&mut state.mode, TreeSynthMode::Random, "Random").clicked() {
                        changed = true;
                    }
                    if ui.selectable_value(&mut state.mode, TreeSynthMode::Layer, "Layer").clicked() {
                        changed = true;
                    }
                    if ui.selectable_value(&mut state.mode, TreeSynthMode::Sequential, "Sequential").clicked() {
                        changed = true;
                    }
                    if ui.selectable_value(&mut state.mode, TreeSynthMode::Morph, "Morph").clicked() {
                        changed = true;
                    }
                    if ui.selectable_value(&mut state.mode, TreeSynthMode::Reorder, "Reorder").clicked() {
                        changed = true;
                    }
                });
        });
        if matches!(state.mode, TreeSynthMode::Morph) {
            changed |= ui
                .add(egui::Slider::new(&mut state.morph, 0.0..=1.0).text("Morph"))
                .changed();
        }
        if matches!(state.mode, TreeSynthMode::Reorder) {
            changed |= ui
                .add(egui::Slider::new(&mut state.reorder, 0.0..=1.0).text("Reorder"))
                .changed();
        }

        ui.add_space(6.0);
        ui.label("Amp");
        // パラメータごとに右クリックメニューを追加
        // TreeSynthパラメータに一意のparam_idを割り当て
        let treesynth_param_defs = [
            ("Gain", &mut state.gain, 0, 0.0, 2.0),
            ("Attack", &mut state.attack, 1, 0.0, 5.0),
            ("Decay", &mut state.decay, 2, 0.0, 5.0),
            ("Sustain", &mut state.sustain, 3, 0.0, 1.0),
            ("Release", &mut state.release, 4, 0.0, 8.0),
        ];
        thread_local! {
            static PENDING_MIDI_LEARN: std::cell::RefCell<Option<(usize, u32, String)>> = const { std::cell::RefCell::new(None) };
            static PENDING_AUTOMATION: std::cell::RefCell<Option<(usize, u32, String)>> = const { std::cell::RefCell::new(None) };
        }
        let track_index = 0;
        for (label, value, param_id, min, max) in treesynth_param_defs {
            let slider = egui::Slider::new(value, min..=max).text(label);
            let response = ui.add(slider);
            changed |= response.changed();
            response.context_menu(|ui| {
                if ui.button("Create Automation").clicked() {
                    PENDING_AUTOMATION.with(|pending| {
                        *pending.borrow_mut() = Some((track_index, param_id, label.to_string()));
                    });
                    ui.close_menu();
                }
                if ui.button("Midi Learn").clicked() {
                    PENDING_MIDI_LEARN.with(|pending| {
                        *pending.borrow_mut() = Some((track_index, param_id, label.to_string()));
                    });
                    ui.close_menu();
                }
            });
        }

        ui.add_space(6.0);
        ui.label("Modulation");
        changed |= ui
            .add(egui::Slider::new(&mut state.vibrato_rate, 0.1..=20.0).text("Vibrato Rate"))
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut state.vibrato_depth, 0.0..=1.0).text("Vibrato Depth"))
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut state.tremolo_rate, 0.1..=20.0).text("Tremolo Rate"))
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut state.tremolo_depth, 0.0..=1.0).text("Tremolo Depth"))
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut state.reverb_mix, 0.0..=1.0).text("Reverb Mix"))
            .changed();

        ui.add_space(6.0);
        ui.label("Performance");
        changed |= ui
            .add(egui::Slider::new(&mut state.pitch_bend_range, 1.0..=24.0).text("Pitch Bend"))
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut state.portamento_ms, 0.0..=500.0).text("Portamento ms"))
            .changed();
        changed |= ui.checkbox(&mut state.legato, "Legato").changed();

        ui.add_space(6.0);
        ui.label(format!("Samples: {}", state.samples.len()));
        egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
            for sample in state.samples.iter_mut() {
                ui.horizontal(|ui| {
                    ui.label(&sample.name);
                    changed |= ui
                        .add(egui::DragValue::new(&mut sample.root_note).clamp_range(0..=127))
                        .changed();
                    changed |= ui.add(egui::DragValue::new(&mut sample.gain).speed(0.01)).changed();
                    changed |= ui.add(egui::DragValue::new(&mut sample.pan).speed(0.01)).changed();
                });
            }
        });

        changed
    }

    pub(crate) fn import_treesynth_folder_to_project(source_folder: &Path, project_root: &Path) -> Result<PathBuf, String> {
        let source_folder = Self::normalize_windows_path(source_folder);
        if !source_folder.exists() {
            return Err("TreeSynth source folder not found".to_string());
        }
        let project_root = Self::normalize_windows_path(project_root);
        let samples_root = project_root.join("assets").join("samples").join("treesynth");
        fs::create_dir_all(&samples_root).map_err(|e| e.to_string())?;

        let base_name = source_folder
            .file_name()
            .and_then(|s| s.to_str())
            .map(Self::sanitize_folder_name)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "TreeSynthSamples".to_string());

        let mut dest = samples_root.join(&base_name);
        let mut suffix = 1usize;
        while dest.exists() && !Self::paths_equal(&dest, &source_folder) {
            dest = samples_root.join(format!("{}-{}", base_name, suffix));
            suffix += 1;
        }

        if !Self::paths_equal(&dest, &source_folder) {
            fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
            Self::copy_dir_recursive(&source_folder, &dest)?;
        }
        Ok(dest)
    }
}
