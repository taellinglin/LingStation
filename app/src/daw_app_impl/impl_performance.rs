impl DawApp {
    pub(crate) fn analyze_performance_sections(&self) -> Vec<PerformanceSectionAnalysis> {
        let signatures = self.arrangement_bar_signatures();
        if signatures.is_empty() {
            return Vec::new();
        }

        let mut sections = Vec::new();
        let mut start_bar = 0usize;
        while start_bar < signatures.len() {
            let remaining = signatures.len() - start_bar;
            if let Some((unit_bars, run_bars)) = Self::repeated_bar_run(&signatures, start_bar) {
                let capped_bars = run_bars.min(16);
                let section_bars = if capped_bars >= unit_bars * 2 {
                    capped_bars - (capped_bars % unit_bars)
                } else {
                    run_bars.min(remaining)
                }
                .max(unit_bars.min(remaining));
                sections.push(PerformanceSectionAnalysis {
                    start_beats: start_bar as f32 * 4.0,
                    length_beats: section_bars as f32 * 4.0,
                    loop_unit_beats: Some(unit_bars as f32 * 4.0),
                });
                start_bar += section_bars;
                continue;
            }

            let section_bars = if remaining >= 8 && Self::bar_change_count(&signatures, start_bar, 8) <= 3 {
                8
            } else if remaining >= 4 {
                4
            } else {
                remaining
            };
            sections.push(PerformanceSectionAnalysis {
                start_beats: start_bar as f32 * 4.0,
                length_beats: section_bars as f32 * 4.0,
                loop_unit_beats: None,
            });
            start_bar += section_bars.max(1);
        }

        sections
    }

    pub(crate) fn find_track_clip_at_section_start(&self, track_index: usize, start_beats: f32) -> Option<usize> {
        self.tracks.get(track_index).and_then(|track| {
            track
                .clips
                .iter()
                .find(|clip| {
                    (clip.start_beats - start_beats).abs() <= 0.05
                        || (clip.start_beats < start_beats + 0.05
                            && clip.start_beats + clip.length_beats > start_beats + 0.05)
                })
                .map(|clip| clip.id)
        })
    }

    pub(crate) fn detect_midi_repeat_unit_beats(&self, clip: &Clip, suggested_unit_beats: f32) -> Option<f32> {
        if !clip.is_midi || clip.midi_notes.is_empty() {
            return None;
        }

        let mut candidates = Vec::new();
        for unit in [suggested_unit_beats, 16.0, 8.0, 4.0, 2.0, 1.0] {
            if unit <= 0.0 || unit > clip.length_beats - 0.05 || clip.length_beats < unit * 2.0 - 0.05 {
                continue;
            }
            if candidates.iter().any(|existing: &f32| (*existing - unit).abs() <= 0.05) {
                continue;
            }
            candidates.push(unit);
        }

        for unit in candidates {
            let cycles = (clip.length_beats / unit).floor() as usize;
            if cycles < 2 {
                continue;
            }
            let baseline = Self::midi_signature_in_window(&clip.midi_notes, clip.start_beats, unit);
            if baseline.is_empty() {
                continue;
            }
            let mut matches = true;
            for cycle in 1..cycles {
                let cycle_start = clip.start_beats + cycle as f32 * unit;
                if Self::midi_signature_in_window(&clip.midi_notes, cycle_start, unit) != baseline {
                    matches = false;
                    break;
                }
            }
            if matches {
                return Some(unit.max(0.25));
            }
        }
        None
    }

    pub(crate) fn apply_smart_performance_sections(
        &mut self,
        sections: &[PerformanceSectionAnalysis],
    ) -> (usize, usize) {
        let mut configured_clips = 0usize;
        let mut loop_clips = 0usize;

        for (section_index, section) in sections.iter().enumerate() {
            let next_section = sections.get(section_index + 1);
            for track_index in 0..self.tracks.len() {
                let Some(clip_id) = self.find_track_clip_at_section_start(track_index, section.start_beats) else {
                    continue;
                };
                let Some((clip_track_index, clip_index)) = self.find_clip_indices_by_id(clip_id) else {
                    continue;
                };
                let Some(clip) = self
                    .tracks
                    .get(clip_track_index)
                    .and_then(|track| track.clips.get(clip_index))
                    .cloned()
                else {
                    continue;
                };
                if (clip.start_beats - section.start_beats).abs() > 0.05 {
                    continue;
                }

                let mut settings = self.performance_clip_settings.get(&clip_id).cloned().unwrap_or_default();
                let mut changed = false;
                let mut is_loop_clip = false;

                let covers_section = clip.length_beats + 0.05 >= section.length_beats;
                if let Some(loop_unit_beats) = section.loop_unit_beats {
                    if clip.is_midi && covers_section {
                        if let Some(repeat_unit) = self.detect_midi_repeat_unit_beats(&clip, loop_unit_beats) {
                            self.update_clip_by_id(clip_id, |target| {
                                target.midi_source_beats = Some(repeat_unit.min(target.length_beats).max(0.25));
                            });
                            settings.trigger_mode = PerformanceTriggerMode::Loop;
                            settings.loop_enabled = true;
                            settings.auto_follow = false;
                            changed = true;
                            is_loop_clip = true;
                        }
                    } else if covers_section && clip
                        .audio_source_beats
                        .map(|beats| beats > 0.0 && beats + 0.05 < clip.length_beats)
                        .unwrap_or(false)
                    {
                        settings.trigger_mode = PerformanceTriggerMode::Loop;
                        settings.loop_enabled = true;
                        settings.auto_follow = false;
                        changed = true;
                        is_loop_clip = true;
                    }
                }

                if !is_loop_clip {
                    settings.trigger_mode = PerformanceTriggerMode::OneShot;
                    settings.loop_enabled = false;
                    if let Some(next_section) = next_section {
                        settings.next_clip_id = self.find_track_clip_at_section_start(track_index, next_section.start_beats);
                        settings.auto_follow = settings.next_clip_id.is_some();
                    } else {
                        settings.next_clip_id = None;
                        settings.auto_follow = false;
                    }
                    changed = true;
                }

                if changed {
                    self.performance_clip_settings.insert(clip_id, settings);
                    configured_clips = configured_clips.saturating_add(1);
                    if is_loop_clip {
                        loop_clips = loop_clips.saturating_add(1);
                    }
                }
            }
        }

        if configured_clips > 0 {
            self.rebuild_all_track_midi_notes();
            self.sync_node_routes();
        }

        (configured_clips, loop_clips)
    }

    pub(crate) fn auto_build_performance_from_arrangement(&mut self) -> AutoPerformanceBuildSummary {
        let sections = self.analyze_performance_sections();
        if sections.is_empty() || self.tracks.is_empty() {
            return AutoPerformanceBuildSummary::default();
        }

        let mut slices_created = 0usize;
        for section in sections.iter().skip(1) {
            slices_created = slices_created.saturating_add(
                self.slice_tracks_at_beat(0, self.tracks.len().saturating_sub(1), section.start_beats)
                    .len(),
            );
        }
        self.update_performance_flow_links_for_tracks(&(0..self.tracks.len()).collect::<Vec<_>>());
        let (configured_clips, loop_clips) = self.apply_smart_performance_sections(&sections);
        AutoPerformanceBuildSummary {
            sections: sections.len(),
            slices_created,
            configured_clips,
            loop_clips,
        }
    }

    pub(crate) fn update_performance_flow_links_for_tracks(&mut self, track_indices: &[usize]) {
        let track_filter: HashSet<usize> = track_indices.iter().copied().collect();
        for (track_index, track) in self.tracks.iter().enumerate() {
            if !track_filter.contains(&track_index) {
                continue;
            }
            let mut clip_refs: Vec<&Clip> = track.clips.iter().collect();
            clip_refs.sort_by(|a, b| a.start_beats.partial_cmp(&b.start_beats).unwrap_or(std::cmp::Ordering::Equal));
            for window in clip_refs.windows(2) {
                let current = window[0];
                let next = window[1];
                let connected = (current.start_beats + current.length_beats - next.start_beats).abs() <= 0.05;
                let mut settings = self.performance_clip_settings.get(&current.id).cloned().unwrap_or_default();
                if connected {
                    settings.auto_follow = true;
                    settings.next_clip_id = Some(next.id);
                } else {
                    settings.auto_follow = false;
                    settings.next_clip_id = None;
                }
                self.performance_clip_settings.insert(current.id, settings);
            }
            if let Some(last) = clip_refs.last() {
                let mut settings = self.performance_clip_settings.get(&last.id).cloned().unwrap_or_default();
                settings.next_clip_id = None;
                self.performance_clip_settings.insert(last.id, settings);
            }
        }
    }

    pub(crate) fn auto_slice_playlist_to_performance(&mut self, mode: AutoSliceMode) -> usize {
        if mode == AutoSliceMode::Smart {
            return self.auto_build_performance_from_arrangement().slices_created;
        }
        let interval = mode.interval_beats().max(0.25);
        let mut sliced_count = 0usize;
        let track_indices: Vec<usize> = (0..self.tracks.len()).collect();
        for track_index in track_indices.iter().copied() {
            let clip_ids: Vec<usize> = self
                .tracks
                .get(track_index)
                .map(|track| track.clips.iter().map(|clip| clip.id).collect())
                .unwrap_or_default();
            for clip_id in clip_ids {
                let Some((source_track_index, clip_index)) = self.find_clip_indices_by_id(clip_id) else {
                    continue;
                };
                if source_track_index != track_index {
                    continue;
                }
                let Some(clip) = self.tracks.get(track_index).and_then(|track| track.clips.get(clip_index)).cloned() else {
                    continue;
                };
                let clip_start = clip.start_beats.max(0.0);
                let clip_end = (clip.start_beats + clip.length_beats).max(clip_start + 0.001);
                let mut current_clip_id = clip.id;
                let mut split = ((clip_start / interval).floor() + 1.0) * interval;
                while split < clip_end - 0.05 {
                    if let Some(new_clip_id) = self.slice_clip_by_id(current_clip_id, split) {
                        sliced_count += 1;
                        current_clip_id = new_clip_id;
                    }
                    split += interval;
                }
            }
        }
        self.update_performance_flow_links_for_tracks(&(0..self.tracks.len()).collect::<Vec<_>>());
        sliced_count
    }

    pub(crate) fn launch_performance_clip_at(
        &mut self,
        track_index: usize,
        clip_id: usize,
        settings: PerformanceClipSettings,
        launch_samples: u64,
    ) -> Result<(), String> {
        let (source_track_index, clip_index) = self
            .find_clip_indices_by_id(clip_id)
            .ok_or_else(|| "Performance clip not found".to_string())?;
        let clip = self
            .tracks
            .get(source_track_index)
            .and_then(|track| track.clips.get(clip_index))
            .cloned()
            .ok_or_else(|| "Performance clip missing".to_string())?;

        if !self.audio_running {
            self.seek_playhead(self.playhead_beats);
            self.set_arrangement_playback_enabled(false);
            self.start_audio_and_midi_internal(false)?;
        }

        let resolved_audio_path = if clip.is_midi {
            None
        } else {
            self.resolve_clip_audio_path(&clip)
                .map(|path| path.to_string_lossy().into_owned())
        };
        if let Some(path) = resolved_audio_path.as_deref() {
            self.preload_performance_audio_clip(path);
        }

        self.sync_performance_runtime();
        {
            let mut runtime = self.engine.performance_runtime.lock();
            if runtime.len() < self.tracks.len() {
                runtime.resize(self.tracks.len(), None);
            }
            if settings.trigger_mode == PerformanceTriggerMode::Toggle {
                let same_active = runtime
                    .get(track_index)
                    .and_then(|slot| slot.as_ref())
                    .map(|active| active.clip.id == clip_id)
                    .unwrap_or(false);
                if same_active {
                    runtime[track_index] = None;
                    self.send_all_notes_off(track_index);
                    return Ok(());
                }
            }
            runtime[track_index] = Some(PerformanceRuntimeClip {
                track_index,
                launch_samples,
                clip,
                loop_enabled: settings.loop_enabled
                    || settings.trigger_mode == PerformanceTriggerMode::Loop,
                trigger_mode: settings.trigger_mode,
                resolved_audio_path,
            });
        }
        Ok(())
    }

    pub(crate) fn next_performance_follow_target(
        &self,
        runtime: &PerformanceRuntimeClip,
        current_samples: u64,
        samples_per_beat: f64,
    ) -> Option<(usize, usize, PerformanceClipSettings, u64)> {
        let mut next_runtime = runtime.clone();
        let mut next_settings = self
            .performance_clip_settings
            .get(&runtime.clip.id)
            .cloned()
            .unwrap_or_default();
        let mut visited = HashSet::new();
        let mut traversed = false;

        loop {
            if next_runtime.loop_enabled {
                break;
            }
            let end_samples = next_runtime
                .launch_samples
                .saturating_add(performance_length_samples(&next_runtime, samples_per_beat));
            if current_samples < end_samples {
                break;
            }
            if !next_settings.auto_follow {
                return None;
            }
            let next_clip_id = next_settings.next_clip_id?;
            if !visited.insert(next_clip_id) {
                return None;
            }
            let (next_track_index, next_clip_index) = self.find_clip_indices_by_id(next_clip_id)?;
            let clip = self
                .tracks
                .get(next_track_index)
                .and_then(|track| track.clips.get(next_clip_index))
                .cloned()?;
            next_settings = self
                .performance_clip_settings
                .get(&next_clip_id)
                .cloned()
                .unwrap_or_default();
            next_runtime = PerformanceRuntimeClip {
                track_index: next_track_index,
                launch_samples: end_samples,
                loop_enabled: next_settings.loop_enabled
                    || next_settings.trigger_mode == PerformanceTriggerMode::Loop,
                trigger_mode: next_settings.trigger_mode,
                resolved_audio_path: if clip.is_midi {
                    None
                } else {
                    self.resolve_clip_audio_path(&clip)
                        .map(|path| path.to_string_lossy().into_owned())
                },
                clip,
            };
            traversed = true;
        }

        if traversed {
            Some((
                next_runtime.track_index,
                next_runtime.clip.id,
                next_settings,
                next_runtime.launch_samples,
            ))
        } else {
            None
        }
    }

    pub(crate) fn update_performance_auto_follow(&mut self) {
        if !self.audio_running {
            return;
        }
        let bpm = self.tempo_bpm.max(1.0);
        let samples_per_beat = self.settings.sample_rate.max(1) as f64 * 60.0 / bpm as f64;
        let current_samples = self.engine.transport_samples.load(Ordering::Relaxed);
        let runtime_snapshot = self.engine.performance_runtime.lock().clone();

        let mut relaunches = Vec::new();
        let mut clears = Vec::new();
        for slot in runtime_snapshot.iter() {
            let Some(runtime) = slot.as_ref() else {
                continue;
            };
            if let Some(target) =
                self.next_performance_follow_target(runtime, current_samples, samples_per_beat)
            {
                if runtime.track_index != target.0 {
                    clears.push((runtime.track_index, runtime.clip.id));
                }
                relaunches.push(target);
            } else if !runtime.loop_enabled {
                let end_samples = runtime
                    .launch_samples
                    .saturating_add(performance_length_samples(runtime, samples_per_beat));
                if current_samples >= end_samples {
                    clears.push((runtime.track_index, runtime.clip.id));
                }
            }
        }

        for (track_index, clip_id, settings, launch_samples) in relaunches {
            let _ = self.launch_performance_clip_at(track_index, clip_id, settings, launch_samples);
        }

        if !clears.is_empty() {
            let mut runtime = self.engine.performance_runtime.lock();
            for (track_index, clip_id) in clears {
                if let Some(Some(active)) = runtime.get(track_index) {
                    if active.clip.id == clip_id {
                        runtime[track_index] = None;
                    }
                }
            }
        }
    }

    pub(crate) fn find_clip_indices_by_id(&self, clip_id: usize) -> Option<(usize, usize)> {
        for (track_index, track) in self.tracks.iter().enumerate() {
            if let Some(clip_index) = track.clips.iter().position(|c| c.id == clip_id) {
                return Some((track_index, clip_index));
            }
        }
        None
    }

    pub(crate) fn shift_clip_notes_by_delta(&mut self, clip_id: usize, delta_beats: f32) {
        if delta_beats.abs() <= f32::EPSILON {
            return;
        }
        let Some((track_index, clip_index)) = self.find_clip_indices_by_id(clip_id) else {
            return;
        };
        if let Some(clip) = self.tracks.get_mut(track_index).and_then(|t| t.clips.get_mut(clip_index)) {
            for note in &mut clip.midi_notes {
                note.start_beats = (note.start_beats + delta_beats).max(0.0);
            }
        }
        self.sync_track_audio_notes(track_index);
    }

    pub(crate) fn remap_track_index(index: usize, from: usize, to: usize) -> usize {
        if from == to {
            return index;
        }
        if index == from {
            return to;
        }
        if from < to {
            if index > from && index <= to {
                return index - 1;
            }
        } else if index >= to && index < from {
            return index + 1;
        }
        index
    }

    pub(crate) fn move_track_order(&mut self, from: usize, to: usize) {
        if from == to || from >= self.tracks.len() || to >= self.tracks.len() {
            return;
        }
        self.push_undo_state();

        let track = self.tracks.remove(from);
        self.tracks.insert(to, track);
        if from < self.engine.track_audio.len() {
            let state = self.engine.track_audio.remove(from);
            if to <= self.engine.track_audio.len() {
                self.engine.track_audio.insert(to, state);
            } else {
                self.engine.track_audio.push(state);
            }
        }

        for (index, track) in self.tracks.iter_mut().enumerate() {
            for clip in &mut track.clips {
                clip.track = index;
            }
        }

        if let Some(selected) = self.selected_track {
            self.selected_track = Some(Self::remap_track_index(selected, from, to));
        }
        if let Some((track_index, lane_index)) = self.automation_active {
            let new_index = Self::remap_track_index(track_index, from, to);
            self.automation_active = Some((new_index, lane_index));
        }
        let mut remapped = HashSet::new();
        for index in &self.automation_rows_expanded {
            remapped.insert(Self::remap_track_index(*index, from, to));
        }
        self.automation_rows_expanded = remapped;

        if let Ok(mut learn) = self.midi_learn.lock() {
            if let Some((track_index, param_id)) = learn.take() {
                let new_index = Self::remap_track_index(track_index, from, to);
                *learn = Some((new_index, param_id));
            }
        }

        {
            let mut recording = self.engine.recording.lock();
            if recording.track_index != usize::MAX {
                recording.track_index = Self::remap_track_index(recording.track_index, from, to);
            }
        }

        self.sync_track_mix();
        self.sync_selected_track_index();
    }
}
