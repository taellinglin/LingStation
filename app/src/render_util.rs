pub(crate) fn collect_block_events(
    notes: &[PianoRollNote],
    block_start: u64,
    block_end: u64,
    samples_per_beat: f64,
) -> Vec<vst3::MidiEvent> {
    let mut events = Vec::new();
    for note in notes {
        let start_sample = (note.start_beats as f64 * samples_per_beat).round() as u64;
        let mut end_sample = ((note.start_beats + note.length_beats) as f64 * samples_per_beat)
            .round() as u64;
        if end_sample <= start_sample {
            end_sample = start_sample.saturating_add(1);
        }
        if start_sample >= block_start && start_sample < block_end {
            let offset = (start_sample - block_start) as i32;
            events.push(vst3::MidiEvent::note_on_at(0, note.midi_note, note.velocity, offset));
        }
        if end_sample >= block_start && end_sample < block_end {
            let offset = (end_sample - block_start) as i32;
            events.push(vst3::MidiEvent::note_off_at(0, note.midi_note, 0, offset));
        }
    }
    events
}

pub(crate) fn collect_block_events_into(
    notes: &[PianoRollNote],
    block_start: u64,
    block_end: u64,
    samples_per_beat: f64,
    out: &mut Vec<vst3::MidiEvent>,
) {
    out.clear();
    for note in notes {
        let start_sample = (note.start_beats as f64 * samples_per_beat).round() as u64;
        let mut end_sample =
            ((note.start_beats + note.length_beats) as f64 * samples_per_beat).round() as u64;
        if end_sample <= start_sample {
            end_sample = start_sample.saturating_add(1);
        }
        if start_sample >= block_start && start_sample < block_end {
            let offset = (start_sample - block_start) as i32;
            out.push(vst3::MidiEvent::note_on_at(0, note.midi_note, note.velocity, offset));
        }
        if end_sample >= block_start && end_sample < block_end {
            let offset = (end_sample - block_start) as i32;
            out.push(vst3::MidiEvent::note_off_at(0, note.midi_note, 0, offset));
        }
    }
}

pub(crate) fn db_to_gain(db: f32) -> f32 {
    10.0f32.powf(db / 20.0)
}

pub(crate) fn apply_master_processing(
    samples: &mut [f32],
    channels: usize,
    sample_rate: f32,
    settings: &MasterCompSettings,
    state: &mut MasterCompState,
) {
    if samples.is_empty() {
        return;
    }
    let mut gain = settings.level.clamp(0.0, 2.0);
    if settings.enabled {
        let threshold = db_to_gain(settings.threshold_db);
        let ratio = settings.ratio.max(1.0);
        let attack = (settings.attack_ms.max(0.1) / 1000.0).max(0.0001);
        let release = (settings.release_ms.max(0.1) / 1000.0).max(0.0001);
        let attack_coeff = (-1.0 / (attack * sample_rate.max(1.0))).exp();
        let release_coeff = (-1.0 / (release * sample_rate.max(1.0))).exp();
        let makeup = db_to_gain(settings.makeup_db);
        gain *= makeup;

        for frame in samples.chunks_mut(channels.max(1)) {
            let mut level = 0.0f32;
            for sample in frame.iter() {
                level = level.max(sample.abs());
            }
            let target_gain = if level > threshold {
                let over = (level / threshold).max(1.0);
                let compressed = over.powf(1.0 / ratio);
                (compressed / over).clamp(0.0, 1.0)
            } else {
                1.0
            };
            if target_gain < state.gain {
                state.gain = attack_coeff * (state.gain - target_gain) + target_gain;
            } else {
                state.gain = release_coeff * (state.gain - target_gain) + target_gain;
            }
            let frame_gain = state.gain * gain;
            for sample in frame.iter_mut() {
                *sample *= frame_gain;
            }
        }
    } else if gain != 1.0 {
        for sample in samples.iter_mut() {
            *sample *= gain;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenderFormat {
    Wav,
    Ogg,
    Flac,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RenderWavBitDepth {
    Int16,
    Int24,
    Int32,
    Float32,
}

impl RenderWavBitDepth {
    pub(crate) fn all() -> [Self; 4] {
        [Self::Int16, Self::Int24, Self::Int32, Self::Float32]
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Int16 => "16-bit",
            Self::Int24 => "24-bit",
            Self::Int32 => "32-bit int",
            Self::Float32 => "32f",
        }
    }

    pub(crate) fn bits_per_sample(self) -> u16 {
        match self {
            Self::Int16 => 16,
            Self::Int24 => 24,
            Self::Int32 => 32,
            Self::Float32 => 32,
        }
    }

    pub(crate) fn sample_format(self) -> hound::SampleFormat {
        match self {
            Self::Float32 => hound::SampleFormat::Float,
            _ => hound::SampleFormat::Int,
        }
    }
}

pub(crate) fn default_midi_params() -> Vec<String> {
    vec![
        "CC1 Modwheel".to_string(),
        "CC7 Volume".to_string(),
        "CC10 Pan".to_string(),
        "CC11 Expression".to_string(),
        "CC64 Sustain".to_string(),
    ]
}

pub(crate) fn gm_program_name(program: u8) -> &'static str {
    const GM_NAMES: [&str; 128] = [
        "Acoustic Grand Piano",
        "Bright Acoustic Piano",
        "Electric Grand Piano",
        "Honky-tonk Piano",
        "Electric Piano 1",
        "Electric Piano 2",
        "Harpsichord",
        "Clavinet",
        "Celesta",
        "Glockenspiel",
        "Music Box",
        "Vibraphone",
        "Marimba",
        "Xylophone",
        "Tubular Bells",
        "Dulcimer",
        "Drawbar Organ",
        "Percussive Organ",
        "Rock Organ",
        "Church Organ",
        "Reed Organ",
        "Accordion",
        "Harmonica",
        "Tango Accordion",
        "Acoustic Guitar (nylon)",
        "Acoustic Guitar (steel)",
        "Electric Guitar (jazz)",
        "Electric Guitar (clean)",
        "Electric Guitar (muted)",
        "Overdriven Guitar",
        "Distortion Guitar",
        "Guitar Harmonics",
        "Acoustic Bass",
        "Electric Bass (finger)",
        "Electric Bass (pick)",
        "Fretless Bass",
        "Slap Bass 1",
        "Slap Bass 2",
        "Synth Bass 1",
        "Synth Bass 2",
        "Violin",
        "Viola",
        "Cello",
        "Contrabass",
        "Tremolo Strings",
        "Pizzicato Strings",
        "Orchestral Harp",
        "Timpani",
        "String Ensemble 1",
        "String Ensemble 2",
        "Synth Strings 1",
        "Synth Strings 2",
        "Choir Aahs",
        "Voice Oohs",
        "Synth Voice",
        "Orchestra Hit",
        "Trumpet",
        "Trombone",
        "Tuba",
        "Muted Trumpet",
        "French Horn",
        "Brass Section",
        "Synth Brass 1",
        "Synth Brass 2",
        "Soprano Sax",
        "Alto Sax",
        "Tenor Sax",
        "Baritone Sax",
        "Oboe",
        "English Horn",
        "Bassoon",
        "Clarinet",
        "Piccolo",
        "Flute",
        "Recorder",
        "Pan Flute",
        "Blown Bottle",
        "Shakuhachi",
        "Whistle",
        "Ocarina",
        "Lead 1 (square)",
        "Lead 2 (sawtooth)",
        "Lead 3 (calliope)",
        "Lead 4 (chiff)",
        "Lead 5 (charang)",
        "Lead 6 (voice)",
        "Lead 7 (fifths)",
        "Lead 8 (bass + lead)",
        "Pad 1 (new age)",
        "Pad 2 (warm)",
        "Pad 3 (polysynth)",
        "Pad 4 (choir)",
        "Pad 5 (bowed)",
        "Pad 6 (metallic)",
        "Pad 7 (halo)",
        "Pad 8 (sweep)",
        "FX 1 (rain)",
        "FX 2 (soundtrack)",
        "FX 3 (crystal)",
        "FX 4 (atmosphere)",
        "FX 5 (brightness)",
        "FX 6 (goblins)",
        "FX 7 (echoes)",
        "FX 8 (sci-fi)",
        "Sitar",
        "Banjo",
        "Shamisen",
        "Koto",
        "Kalimba",
        "Bag pipe",
        "Fiddle",
        "Shanai",
        "Tinkle Bell",
        "Agogo",
        "Steel Drums",
        "Woodblock",
        "Taiko Drum",
        "Melodic Tom",
        "Synth Drum",
        "Reverse Cymbal",
        "Guitar Fret Noise",
        "Breath Noise",
        "Seashore",
        "Bird Tweet",
        "Telephone Ring",
        "Helicopter",
        "Applause",
        "Gunshot",
    ];
    GM_NAMES[program.min(127) as usize]
}

pub(crate) fn gm_drum_kit_name(program: u8) -> Option<&'static str> {
    match program {
        0 => Some("Standard Kit"),
        8 => Some("Room Kit"),
        16 => Some("Power Kit"),
        24 => Some("Electronic Kit"),
        25 => Some("TR-808 Kit"),
        32 => Some("Jazz Kit"),
        40 => Some("Brush Kit"),
        48 => Some("Orchestra Kit"),
        56 => Some("Sound FX Kit"),
        _ => None,
    }
}

pub(crate) fn default_instrument_params() -> Vec<String> {
    vec![
        "Gain".to_string(),
        "Cutoff".to_string(),
        "Resonance".to_string(),
        "Attack".to_string(),
        "Release".to_string(),
    ]
}
