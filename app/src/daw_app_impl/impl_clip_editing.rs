impl DawApp {
    pub(crate) fn remove_clip_by_id(&mut self, clip_id: usize) -> Option<Clip> {
        for track in &mut self.tracks {
            if let Some(pos) = track.clips.iter().position(|c| c.id == clip_id) {
                return Some(track.clips.remove(pos));
            }
        }
        None
    }

    pub(crate) fn remove_clip_and_notes_by_id(&mut self, clip_id: usize) -> Option<Clip> {
        for (track_index, track) in self.tracks.iter_mut().enumerate() {
            if let Some(pos) = track.clips.iter().position(|c| c.id == clip_id) {
                let clip = track.clips.remove(pos);
                if clip.is_midi {
                    self.sync_track_audio_notes(track_index);
                    self.send_all_notes_off(track_index);
                }
                return Some(clip);
            }
        }
        None
    }

    pub(crate) fn move_clip_by_id(&mut self, clip_id: usize, target_track: usize, start_beats: f32) {
        let mut clip = match self.remove_clip_by_id(clip_id) {
            Some(clip) => clip,
            None => return,
        };
        let safe_track = target_track.min(self.tracks.len().saturating_sub(1));
        clip.track = safe_track;
        clip.start_beats = start_beats.max(0.0);
        if let Some(track) = self.tracks.get_mut(safe_track) {
            track.clips.push(clip);
        }
    }

    pub(crate) fn next_clip_link_id(&self) -> usize {
        self.tracks
            .iter()
            .flat_map(|track| track.clips.iter().filter_map(|clip| clip.link_id))
            .max()
            .unwrap_or(0)
            + 1
    }

    pub(crate) fn ensure_clip_link_id(&mut self, track_index: usize, clip_id: usize) -> Option<usize> {
        let existing = self
            .tracks
            .get(track_index)
            .and_then(|track| track.clips.iter().find(|c| c.id == clip_id))
            .and_then(|clip| clip.link_id);
        if let Some(link_id) = existing {
            return Some(link_id);
        }
        let new_id = self.next_clip_link_id();
        if let Some(track) = self.tracks.get_mut(track_index) {
            if let Some(clip) = track.clips.iter_mut().find(|c| c.id == clip_id) {
                clip.link_id = Some(new_id);
            }
        }
        Some(new_id)
    }

    pub(crate) fn unique_clip_name(&self, track_index: usize, base: &str, exclude_id: usize) -> String {
        let base = if base.trim().is_empty() { "Clip" } else { base.trim() };
        let mut suffix = 2usize;
        loop {
            let candidate = format!("{} {}", base, suffix);
            let exists = self
                .tracks
                .get(track_index)
                .map(|track| {
                    track
                        .clips
                        .iter()
                        .any(|c| c.id != exclude_id && c.name.eq_ignore_ascii_case(&candidate))
                })
                .unwrap_or(false);
            if !exists {
                return candidate;
            }
            suffix = suffix.saturating_add(1);
        }
    }

    pub(crate) fn make_clip_unique(&mut self, track_index: usize, clip_id: usize) {
        let needs_update = self
            .tracks
            .get(track_index)
            .and_then(|track| track.clips.iter().find(|c| c.id == clip_id))
            .map(|clip| clip.link_id.is_some())
            .unwrap_or(false);
        if !needs_update {
            return;
        }
        let current_name = self
            .tracks
            .get(track_index)
            .and_then(|track| track.clips.iter().find(|c| c.id == clip_id))
            .map(|clip| clip.name.clone())
            .unwrap_or_else(|| "Clip".to_string());
        let next_name = self.unique_clip_name(track_index, &current_name, clip_id);
        if let Some(track) = self.tracks.get_mut(track_index) {
            if let Some(clip) = track.clips.iter_mut().find(|c| c.id == clip_id) {
                clip.name = next_name;
                clip.link_id = None;
            }
        }
        self.mark_dirty();
    }

    pub(crate) fn sync_linked_clips_for_clip(&mut self, track_index: usize, source_clip_id: usize) {
        let (link_id, source_start, source_len, notes_snapshot) = {
            let track = match self.tracks.get(track_index) {
                Some(track) => track,
                None => return,
            };
            let Some(source_clip) = track.clips.iter().find(|c| c.id == source_clip_id) else {
                return;
            };
            let Some(link_id) = source_clip.link_id else {
                return;
            };
            (
                link_id,
                source_clip.start_beats,
                source_clip.length_beats,
                source_clip.midi_notes.clone(),
            )
        };

        let source_end = source_start + source_len;
        let mut pattern_notes: Vec<PianoRollNote> = Vec::new();
        for note in &notes_snapshot {
            let note_end = note.start_beats + note.length_beats;
            if note.start_beats < source_end && note_end > source_start {
                let mut relative = note.clone();
                relative.start_beats = (relative.start_beats - source_start).max(0.0);
                pattern_notes.push(relative);
            }
        }
        if pattern_notes.is_empty() {
            return;
        }

        let Some(track) = self.tracks.get_mut(track_index) else {
            return;
        };
        for target in track
            .clips
            .iter_mut()
            .filter(|c| c.link_id == Some(link_id) && c.id != source_clip_id)
        {
            let target_start = target.start_beats;
            let mut shifted_notes = Vec::new();
            for note in &pattern_notes {
                let mut shifted = note.clone();
                shifted.start_beats = (shifted.start_beats + target_start).max(0.0);
                shifted_notes.push(shifted);
            }
            target.midi_notes = shifted_notes;
        }
        self.sync_track_audio_notes(track_index);
        self.mark_dirty();
    }

    pub(crate) fn sync_linked_notes_after_edit(&mut self, track_index: usize) {
        let Some(clip_id) = self.selected_clip else {
            return;
        };
        let mut found_track = None;
        for (ti, track) in self.tracks.iter().enumerate() {
            if track.clips.iter().any(|c| c.id == clip_id) {
                found_track = Some(ti);
                break;
            }
        }
        if found_track == Some(track_index) {
            self.sync_linked_clips_for_clip(track_index, clip_id);
        }
    }

    pub(crate) fn clone_clips_by_ids(&mut self, clip_ids: &[usize]) {
        let mut copies: Vec<(Clip, usize)> = Vec::new();
        for clip_id in clip_ids {
            for (track_index, track) in self.tracks.iter().enumerate() {
                if let Some(clip) = track.clips.iter().find(|c| c.id == *clip_id) {
                    copies.push((clip.clone(), track_index));
                    break;
                }
            }
        }
        if copies.is_empty() {
            return;
        }
        self.push_undo_state();
        let mut new_ids = Vec::new();
        let mut last_track = None;
        for (mut clip, track_index) in copies {
            let link_id = self.ensure_clip_link_id(track_index, clip.id);
            let new_id = self.next_clip_id();
            clip.id = new_id;
            clip.track = track_index;
            clip.link_id = link_id;
            if let Some(track) = self.tracks.get_mut(track_index) {
                track.clips.push(clip.clone());
            }
            if clip.is_midi {
                self.sync_track_audio_notes(track_index);
            }
            new_ids.push(new_id);
            last_track = Some(track_index);
        }
        self.selected_clips.clear();
        for id in &new_ids {
            self.selected_clips.insert(*id);
        }
        self.selected_clip = new_ids.last().copied();
        if let Some(track_index) = last_track {
            self.selected_track = Some(track_index);
        }
        self.refresh_params_for_selected_track(false);
    }

    pub(crate) fn collect_selected_clips_for_merge(&self) -> Option<(usize, bool, Vec<Clip>)> {
        if self.selected_clips.len() < 2 {
            return None;
        }

        let mut clips = Vec::new();
        let mut track_index: Option<usize> = None;
        let mut is_midi: Option<bool> = None;
        for clip_id in &self.selected_clips {
            let mut found = None;
            for (ti, track) in self.tracks.iter().enumerate() {
                if let Some(clip) = track.clips.iter().find(|c| c.id == *clip_id) {
                    found = Some((ti, clip.clone()));
                    break;
                }
            }
            let (ti, clip) = found?;
            if let Some(expected) = track_index {
                if expected != ti {
                    return None;
                }
            } else {
                track_index = Some(ti);
            }
            if let Some(expected_kind) = is_midi {
                if expected_kind != clip.is_midi {
                    return None;
                }
            } else {
                is_midi = Some(clip.is_midi);
            }
            clips.push(clip);
        }

        clips.sort_by(|a, b| {
            a.start_beats
                .partial_cmp(&b.start_beats)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Some((track_index?, is_midi?, clips))
    }

    pub(crate) fn can_merge_selected_clips(&self) -> bool {
        const MERGE_GAP_TOLERANCE_BEATS: f32 = 0.05;

        let Some((_track_index, is_midi, clips)) = self.collect_selected_clips_for_merge() else {
            return false;
        };

        if !is_midi {
            return clips
                .iter()
                .all(|clip| self.resolve_clip_audio_path(clip).is_some());
        }

        for pair in clips.windows(2) {
            let prev = &pair[0];
            let next = &pair[1];
            let prev_end = prev.start_beats + prev.length_beats;
            let gap = next.start_beats - prev_end;
            if gap > MERGE_GAP_TOLERANCE_BEATS {
                return false;
            }
        }
        true
    }

    pub(crate) fn merge_selected_clips(&mut self) {
        let Some((track_index, is_midi, clips)) = self.collect_selected_clips_for_merge() else {
            return;
        };
        if !self.can_merge_selected_clips() {
            return;
        }

        let first = clips.first().cloned();
        let last = clips.last().cloned();
        let (Some(first), Some(last)) = (first, last) else {
            return;
        };
        let start = first.start_beats;
        let end = last.start_beats + last.length_beats;
        let merged = if is_midi {
            let mut merged = first.clone();
            merged.id = self.next_clip_id();
            merged.track = track_index;
            merged.start_beats = start;
            merged.length_beats = (end - start).max(0.0);
            merged.name = if merged.name.trim().is_empty() {
                "Merged".to_string()
            } else {
                merged.name.clone()
            };
            let mut merged_notes: Vec<PianoRollNote> = Vec::new();
            for clip in &clips {
                merged_notes.extend(clip.midi_notes.iter().cloned());
            }
            if !merged_notes.is_empty() {
                merged_notes.sort_by(|a, b| {
                    a.start_beats
                        .partial_cmp(&b.start_beats)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
            merged.midi_notes = merged_notes;
            merged.midi_source_beats = Some(merged.length_beats.max(0.25));
            merged
        } else {
            match self.stitch_selected_audio_clips(track_index, &clips) {
                Ok(merged) => merged,
                Err(err) => {
                    self.status = format!("Audio merge failed: {err}");
                    return;
                }
            }
        };

        let merged_count = clips.len();

        self.push_undo_state();
        for clip in clips {
            self.remove_clip_by_id(clip.id);
        }
        if let Some(track) = self.tracks.get_mut(track_index) {
            track.clips.push(merged.clone());
        }
        if merged.is_midi {
            self.sync_track_audio_notes(track_index);
        } else {
            self.refresh_audio_clip_timeline_if_running();
        }
        self.selected_clips.clear();
        self.selected_clips.insert(merged.id);
        self.selected_clip = Some(merged.id);
        self.selected_track = Some(track_index);
        self.refresh_params_for_selected_track(false);
        self.mark_dirty();
        self.status = format!("Merged {} clip(s)", merged_count.max(1));
    }

    pub(crate) fn stitch_selected_audio_clips(
        &mut self,
        track_index: usize,
        clips: &[Clip],
    ) -> Result<Clip, String> {
        let first = clips
            .first()
            .cloned()
            .ok_or_else(|| "No clips selected".to_string())?;
        let start = clips
            .iter()
            .map(|clip| clip.start_beats)
            .fold(f32::INFINITY, f32::min)
            .max(0.0);
        let end = clips
            .iter()
            .map(|clip| clip.start_beats + clip.length_beats)
            .fold(0.0f32, f32::max)
            .max(start + 0.001);
        let sample_rate = self.settings.sample_rate.max(1);
        let total_frames = self.beats_to_samples((end - start).max(0.001), sample_rate).max(1);

        let mut local_cache = AudioClipCache::new(
            AUDIO_CLIP_CACHE_MAX_BYTES,
            AUDIO_CLIP_CACHE_MAX_ENTRIES,
        );
        let mut channels = 1usize;
        let mut renders: Vec<(AudioClipRender, Arc<AudioClipData>)> = Vec::new();
        for clip in clips {
            let path = self
                .resolve_clip_audio_path(clip)
                .ok_or_else(|| format!("Missing audio file for {}", clip.name))?;
            let path_str = path.to_string_lossy().to_string();
            let data = if let Some(data) = local_cache.get(&path_str) {
                data
            } else {
                let data = Arc::new(
                    Self::load_audio_clip_data(&path)
                        .ok_or_else(|| format!("Unsupported audio file: {}", path.display()))?,
                );
                local_cache.insert(path_str.clone().into(), data.clone());
                data
            };
            channels = channels.max(data.channels.max(1));
            let pitch = self.clip_effective_pitch_semitones(clip);
            renders.push((
                AudioClipRender {
                    clip_id: clip.id,
                    path: path_str.into(),
                    track_index: 0,
                    start_samples: self.beats_to_samples((clip.start_beats - start).max(0.0), sample_rate),
                    length_samples: self.beats_to_samples(clip.length_beats, sample_rate).max(1),
                    offset_samples: self.beats_to_samples(clip.audio_offset_beats, sample_rate),
                    gain: clip.audio_gain,
                    time_mul: Self::audio_playback_time_mul(clip, pitch),
                    pitch_semitones: pitch,
                    stretch_mode: clip.audio_stretch_mode,
                    formant_scale: clip.audio_formant_scale,
                },
                data,
            ));
        }

        let mut samples = vec![0.0f32; total_frames as usize * channels.max(1)];
        for (render, data) in &renders {
            if render.stretch_mode == AudioStretchMode::Speed {
                mix_clip_resample(
                    &mut samples,
                    channels,
                    render,
                    data,
                    0,
                    total_frames,
                    sample_rate as f32,
                );
            } else {
                #[cfg(all(windows, has_rubberband))]
                {
                    let formant_preserve = matches!(
                        render.stretch_mode,
                        AudioStretchMode::StretchFormant
                            | AudioStretchMode::StretchNeutral
                            | AudioStretchMode::StretchVocal
                    );
                    let time_mul = render.time_mul.max(0.01) as f64;
                    let pitch_scale = time_mul
                        * 2.0f64.powf(render.pitch_semitones as f64 / 12.0);
                    let formant_scale = render.formant_scale.max(0.25) as f64;
                    let stretcher = local_cache.get_or_create_stretcher(
                        render.clip_id,
                        sample_rate,
                        channels,
                        pitch_scale,
                        formant_preserve,
                        formant_scale,
                    );
                    mix_clip_stretch(
                        &mut samples,
                        channels,
                        render,
                        data,
                        0,
                        total_frames,
                        sample_rate as f32,
                        &stretcher,
                    );
                }
                #[cfg(any(not(windows), not(has_rubberband)))]
                {
                    mix_clip_resample(
                        &mut samples,
                        channels,
                        render,
                        data,
                        0,
                        total_frames,
                        sample_rate as f32,
                    );
                }
            }
        }

        let project_folder = self.ensure_project_folder()?;
        let audio_dir = project_folder.join("audio");
        fs::create_dir_all(&audio_dir).map_err(|e| e.to_string())?;
        let base_name = if first.name.trim().is_empty() {
            "Merged"
        } else {
            first.name.trim()
        };
        let safe_base = Self::sanitize_folder_name(&format!("{}_merged", base_name));
        let mut target = audio_dir.join(format!("{}.wav", safe_base));
        let mut counter = 2usize;
        while target.exists() {
            target = audio_dir.join(format!("{}_{}.wav", safe_base, counter));
            counter = counter.saturating_add(1);
        }

        let spec = hound::WavSpec {
            channels: channels as u16,
            sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let file = std::fs::File::create(&target).map_err(|e| e.to_string())?;
        let mut writer = hound::WavWriter::new(file, spec).map_err(|e| e.to_string())?;
        for sample in &samples {
            writer.write_sample(*sample).map_err(|e| e.to_string())?;
        }
        writer.finalize().map_err(|e| e.to_string())?;

        if let Some(data) = Self::load_audio_clip_data(&target) {
            if let Ok(mut cache) = self.audio_clip_cache.lock() {
                cache.insert(target.to_string_lossy().to_string().into(), Arc::new(data));
            }
        }

        Ok(Clip {
            id: self.next_clip_id(),
            track: track_index,
            start_beats: start,
            length_beats: (end - start).max(0.25),
            is_midi: false,
            midi_notes: Vec::new(),
            midi_source_beats: None,
            link_id: None,
            name: if first.name.trim().is_empty() {
                "Merged".to_string()
            } else {
                format!("{} Merged", first.name.trim())
            },
            audio_path: Some(format!(
                "audio/{}",
                target.file_name().unwrap_or_default().to_string_lossy()
            )),
            audio_source_beats: Some((end - start).max(0.25)),
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
        })
    }

    pub(crate) fn crop_clip_notes_to_clip_range(&mut self, clip_id: usize, new_start: f32, new_len: f32) {
        let new_end = new_start + new_len;
        let Some((track_index, clip_index)) = self.find_clip_indices_by_id(clip_id) else {
            return;
        };
        let Some(clip) = self
            .tracks
            .get_mut(track_index)
            .and_then(|t| t.clips.get_mut(clip_index))
        else {
            return;
        };
        let mut index = 0usize;
        while index < clip.midi_notes.len() {
            let note = &mut clip.midi_notes[index];
            let note_end = note.start_beats + note.length_beats;
            if note_end <= new_start || note.start_beats >= new_end {
                clip.midi_notes.remove(index);
                continue;
            }
            let clamped_start = note.start_beats.max(new_start);
            let clamped_end = note_end.min(new_end);
            let next_len = clamped_end - clamped_start;
            if next_len <= 0.0 {
                clip.midi_notes.remove(index);
                continue;
            }
            note.start_beats = clamped_start;
            note.length_beats = next_len;
            index += 1;
        }
        self.sync_track_audio_notes(track_index);
        self.send_all_notes_off(track_index);
    }

    pub(crate) fn send_all_notes_off(&self, track_index: usize) {
        let Some(state) = self.track_audio.get(track_index) else {
            return;
        };
        if let Ok(mut events) = state.midi_events.lock() {
            events.extend((0u8..=127).map(|note| vst3::MidiEvent::note_off(0, note, 0)));
        }
    }

    pub(crate) fn update_clip_by_id<F>(&mut self, clip_id: usize, mut apply: F)
    where
        F: FnMut(&mut Clip),
    {
        for track in &mut self.tracks {
            if let Some(clip) = track.clips.iter_mut().find(|c| c.id == clip_id) {
                apply(clip);
                return;
            }
        }
    }

    pub(crate) fn slice_clip_by_id(&mut self, clip_id: usize, split_beat: f32) -> Option<usize> {
        let (track_index, clip_index) = self.find_clip_indices_by_id(clip_id)?;
        let original = self.tracks.get(track_index)?.clips.get(clip_index)?.clone();
        let clip_start = original.start_beats.max(0.0);
        let clip_end = (original.start_beats + original.length_beats).max(clip_start + 0.001);
        let split_beat = split_beat.clamp(clip_start, clip_end);
        if split_beat <= clip_start + 0.05 || split_beat >= clip_end - 0.05 {
            return None;
        }

        let left_length = (split_beat - clip_start).max(0.05);
        let right_length = (clip_end - split_beat).max(0.05);
        let new_clip_id = self.next_clip_id();
        let mut left = original.clone();
        let mut right = original.clone();

        left.length_beats = left_length;
        right.id = new_clip_id;
        right.start_beats = split_beat;
        right.length_beats = right_length;
        right.audio_offset_beats = (original.audio_offset_beats + left_length).max(0.0);

        if original.is_midi {
            let mut left_notes = Vec::new();
            let mut right_notes = Vec::new();
            for note in &original.midi_notes {
                let note_start = note.start_beats;
                let note_end = note.start_beats + note.length_beats;
                if note_end > clip_start && note_start < split_beat {
                    let left_start = note_start.max(clip_start);
                    let left_end = note_end.min(split_beat);
                    if left_end > left_start + 0.01 {
                        let mut clipped = note.clone();
                        clipped.start_beats = left_start;
                        clipped.length_beats = left_end - left_start;
                        left_notes.push(clipped);
                    }
                }
                if note_end > split_beat && note_start < clip_end {
                    let right_start = note_start.max(split_beat);
                    let right_end = note_end.min(clip_end);
                    if right_end > right_start + 0.01 {
                        let mut clipped = note.clone();
                        clipped.start_beats = right_start;
                        clipped.length_beats = right_end - right_start;
                        right_notes.push(clipped);
                    }
                }
            }
            left.midi_notes = left_notes;
            right.midi_notes = right_notes;
            left.midi_source_beats = Some(left.length_beats.max(0.25));
            right.midi_source_beats = Some(right.length_beats.max(0.25));
        }

        let next_name = self.unique_clip_name(track_index, &original.name, clip_id);
        right.name = next_name;

        if let Some(settings) = self.performance_clip_settings.get(&clip_id).cloned() {
            self.performance_clip_settings.insert(new_clip_id, settings);
        }

        if let Some(track) = self.tracks.get_mut(track_index) {
            if clip_index >= track.clips.len() {
                return None;
            }
            track.clips[clip_index] = left;
            track.clips.insert(clip_index + 1, right);
        }

        if original.is_midi {
            self.sync_track_audio_notes(track_index);
            self.send_all_notes_off(track_index);
        }
        self.sync_node_routes();
        self.mark_dirty();
        Some(new_clip_id)
    }

    pub(crate) fn arranger_slice_beat(&self, raw_beat: f32, free_snap: bool) -> f32 {
        let beat = raw_beat.max(0.0);
        if free_snap {
            beat
        } else {
            let snap = AutoSliceMode::Bar.interval_beats();
            (beat / snap).round() * snap
        }
    }

    pub(crate) fn slice_tracks_at_beat(
        &mut self,
        start_track: usize,
        end_track: usize,
        split_beat: f32,
    ) -> Vec<(usize, usize, usize)> {
        let track_min = start_track.min(end_track);
        let track_max = start_track.max(end_track);
        let targets: Vec<(usize, usize)> = self
            .tracks
            .iter()
            .enumerate()
            .filter(|(track_index, _)| *track_index >= track_min && *track_index <= track_max)
            .flat_map(|(track_index, track)| {
                track.clips.iter().filter_map(move |clip| {
                    let clip_start = clip.start_beats.max(0.0);
                    let clip_end = (clip.start_beats + clip.length_beats).max(clip_start + 0.001);
                    if split_beat > clip_start + 0.05 && split_beat < clip_end - 0.05 {
                        Some((track_index, clip.id))
                    } else {
                        None
                    }
                })
            })
            .collect();

        let mut sliced = Vec::new();
        for (track_index, clip_id) in targets {
            if let Some(new_clip_id) = self.slice_clip_by_id(clip_id, split_beat) {
                sliced.push((track_index, clip_id, new_clip_id));
            }
        }
        sliced
    }

    pub(crate) fn quantize_signature_beats(beats: f32) -> i32 {
        (beats.max(0.0) * 4.0).round() as i32
    }

    pub(crate) fn clip_signature_key(clip: &Clip) -> String {
        if !clip.name.trim().is_empty() {
            clip.name.trim().to_ascii_lowercase()
        } else if let Some(path) = clip.audio_path.as_deref() {
            std::path::Path::new(path)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or(path)
                .to_ascii_lowercase()
        } else if clip.is_midi {
            "midi".to_string()
        } else {
            "audio".to_string()
        }
    }

    pub(crate) fn midi_signature_in_window(notes: &[PianoRollNote], window_start: f32, window_len: f32) -> String {
        let window_end = window_start + window_len;
        let mut tokens = Vec::new();
        for note in notes {
            let note_start = note.start_beats;
            let note_end = note.start_beats + note.length_beats;
            if note_end <= window_start + 0.01 || note_start >= window_end - 0.01 {
                continue;
            }
            let rel_start = Self::quantize_signature_beats(note_start - window_start);
            let rel_len = Self::quantize_signature_beats(note.length_beats).max(1);
            tokens.push(format!(
                "{}:{}:{}:{}",
                note.midi_note,
                rel_start,
                rel_len,
                note.velocity,
            ));
        }
        tokens.sort();
        tokens.join("|")
    }

    pub(crate) fn clip_signature_in_window(&self, clip: &Clip, window_start: f32, window_len: f32) -> String {
        let window_end = window_start + window_len;
        let clip_start = clip.start_beats;
        let clip_end = clip.start_beats + clip.length_beats;
        if clip_end <= window_start + 0.01 || clip_start >= window_end - 0.01 {
            return String::new();
        }
        let overlap_start = clip_start.max(window_start);
        let overlap_end = clip_end.min(window_end);
        let overlap_len = (overlap_end - overlap_start).max(0.0);
        let rel_start = Self::quantize_signature_beats(overlap_start - window_start);
        let rel_len = Self::quantize_signature_beats(overlap_len).max(1);
        let key = Self::clip_signature_key(clip);
        if clip.is_midi {
            let note_sig = Self::midi_signature_in_window(&clip.midi_notes, window_start, window_len);
            format!("M:{key}:{rel_start}:{rel_len}:{note_sig}")
        } else {
            let offset = Self::quantize_signature_beats(
                overlap_start - clip.start_beats + clip.audio_offset_beats,
            );
            format!("A:{key}:{rel_start}:{rel_len}:{offset}")
        }
    }

    pub(crate) fn track_signature_in_window(&self, track_index: usize, window_start: f32, window_len: f32) -> String {
        let Some(track) = self.tracks.get(track_index) else {
            return String::new();
        };
        let mut tokens = Vec::new();
        for clip in &track.clips {
            let signature = self.clip_signature_in_window(clip, window_start, window_len);
            if !signature.is_empty() {
                tokens.push(signature);
            }
        }
        if tokens.is_empty() {
            "-".to_string()
        } else {
            tokens.join("+")
        }
    }

    pub(crate) fn arrangement_bar_signatures(&self) -> Vec<String> {
        let max_end = self
            .tracks
            .iter()
            .flat_map(|track| track.clips.iter().map(|clip| clip.start_beats + clip.length_beats))
            .fold(0.0f32, f32::max);
        let total_bars = ((max_end / 4.0).ceil() as usize).max(1);
        let mut signatures = Vec::with_capacity(total_bars);
        for bar in 0..total_bars {
            let start = bar as f32 * 4.0;
            let mut track_tokens = Vec::with_capacity(self.tracks.len());
            for track_index in 0..self.tracks.len() {
                track_tokens.push(self.track_signature_in_window(track_index, start, 4.0));
            }
            signatures.push(track_tokens.join("||"));
        }
        signatures
    }

    pub(crate) fn repeated_bar_run(signatures: &[String], start_bar: usize) -> Option<(usize, usize)> {
        for unit_bars in [8usize, 4, 2, 1] {
            if start_bar + unit_bars * 2 > signatures.len() {
                continue;
            }
            let pattern = &signatures[start_bar..start_bar + unit_bars];
            let mut end_bar = start_bar + unit_bars;
            while end_bar + unit_bars <= signatures.len()
                && signatures[end_bar..end_bar + unit_bars] == *pattern
            {
                end_bar += unit_bars;
            }
            if end_bar >= start_bar + unit_bars * 2 {
                return Some((unit_bars, end_bar - start_bar));
            }
        }
        None
    }

    pub(crate) fn bar_change_count(signatures: &[String], start_bar: usize, len_bars: usize) -> usize {
        let end_bar = (start_bar + len_bars).min(signatures.len());
        let mut changes = 0usize;
        for index in (start_bar + 1)..end_bar {
            if signatures[index] != signatures[index - 1] {
                changes = changes.saturating_add(1);
            }
        }
        changes
    }
}
