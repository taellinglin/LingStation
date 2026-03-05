use crate::timeline::PianoRollNote;
use midly::{MetaMessage, MidiMessage as MidlyMessage, Smf, Timing, TrackEventKind};
use std::collections::HashMap;
use std::fs;

#[derive(Clone, Copy, Debug)]
pub struct MidiMessage {
    pub status: u8,
    pub data1: u8,
    pub data2: u8,
}

pub struct MidiEngine {
    pub queue: Vec<MidiMessage>,
}

impl MidiEngine {
    pub fn new() -> Self {
        Self { queue: Vec::new() }
    }

    pub fn push(&mut self, msg: MidiMessage) {
        self.queue.push(msg);
    }

    pub fn drain(&mut self) -> impl Iterator<Item = MidiMessage> + '_ {
        self.queue.drain(..)
    }
}

pub fn import_midi(path: &str) -> Result<Vec<PianoRollNote>, String> {
    let data = fs::read(path).map_err(|e| e.to_string())?;
    let smf = Smf::parse(&data).map_err(|e| format!("midi parse error: {e}"))?;
    let ticks_per_beat = match smf.header.timing {
        Timing::Metrical(ticks) => ticks.as_int() as u32,
        Timing::Timecode(_, _) => 480,
    };

    let mut tempo_us = 500_000u32;
    for track in &smf.tracks {
        for event in track {
            if let TrackEventKind::Meta(MetaMessage::Tempo(us)) = event.kind {
                tempo_us = us.as_int();
                break;
            }
        }
    }

    let mut notes = Vec::new();
    for track in &smf.tracks {
        let mut abs_ticks = 0u64;
        let mut active: HashMap<u8, (u64, u8)> = HashMap::new();
        for event in track {
            abs_ticks += event.delta.as_int() as u64;
            match event.kind {
                TrackEventKind::Midi {
                    message: MidlyMessage::NoteOn { key, vel },
                    ..
                } => {
                    if vel.as_int() == 0 {
                        if let Some((start, velocity)) = active.remove(&key.as_int()) {
                            let length_ticks = abs_ticks.saturating_sub(start);
                            notes.push(note_from_ticks(
                                start,
                                length_ticks,
                                key.as_int(),
                                velocity,
                                ticks_per_beat,
                            ));
                        }
                    } else {
                        active.insert(key.as_int(), (abs_ticks, vel.as_int()));
                    }
                }
                TrackEventKind::Midi {
                    message: MidlyMessage::NoteOff { key, .. },
                    ..
                } => {
                    if let Some((start, velocity)) = active.remove(&key.as_int()) {
                        let length_ticks = abs_ticks.saturating_sub(start);
                        notes.push(note_from_ticks(
                            start,
                            length_ticks,
                            key.as_int(),
                            velocity,
                            ticks_per_beat,
                        ));
                    }
                }
                _ => {}
            }
        }
    }

    let _bpm = 60.0 * 1_000_000.0 / tempo_us as f32;
    Ok(notes)
}

pub fn export_midi(path: &str, notes: &[PianoRollNote], ticks_per_beat: u16) -> Result<(), String> {
    let tpq = ticks_per_beat as u32;
    let mut events: Vec<(u64, TrackEventKind)> = Vec::new();
    for note in notes {
        let start = (note.start_beats * tpq as f32).round().max(0.0) as u64;
        let end = ((note.start_beats + note.length_beats) * tpq as f32).round().max(0.0) as u64;
        let key = midly::num::u7::from(note.midi_note.min(127));
        let vel = midly::num::u7::from(note.velocity.min(127));
        events.push((start, TrackEventKind::Midi { channel: midly::num::u4::from(0), message: MidlyMessage::NoteOn { key, vel } }));
        events.push((end, TrackEventKind::Midi { channel: midly::num::u4::from(0), message: MidlyMessage::NoteOff { key, vel: midly::num::u7::from(0) } }));
    }

    events.sort_by_key(|(t, _)| *t);
    let mut track = Vec::new();
    let mut last_tick = 0u64;
    for (tick, kind) in events {
        let delta = tick.saturating_sub(last_tick) as u32;
        last_tick = tick;
        track.push(midly::TrackEvent { delta: delta.into(), kind });
    }
    track.push(midly::TrackEvent { delta: 0.into(), kind: TrackEventKind::Meta(MetaMessage::EndOfTrack) });

    let smf = Smf {
        header: midly::Header::new(midly::Format::SingleTrack, Timing::Metrical(ticks_per_beat.into())),
        tracks: vec![track],
    };

    let mut out = Vec::new();
    smf.write_std(&mut out).map_err(|e| e.to_string())?;
    fs::write(path, out).map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Clone, Debug)]
pub struct MidiChannelNotes {
    pub channel: u8,
    pub notes: Vec<PianoRollNote>,
}

pub fn import_midi_channels(path: &str) -> Result<Vec<MidiChannelNotes>, String> {
    let data = fs::read(path).map_err(|e| e.to_string())?;
    let smf = Smf::parse(&data).map_err(|e| format!("midi parse error: {e}"))?;
    let ticks_per_beat = match smf.header.timing {
        Timing::Metrical(ticks) => ticks.as_int() as u32,
        Timing::Timecode(_, _) => 480,
    };

    let mut channel_notes: HashMap<u8, Vec<PianoRollNote>> = HashMap::new();
    for track in &smf.tracks {
        let mut abs_ticks = 0u64;
        let mut active: HashMap<(u8, u8), (u64, u8)> = HashMap::new();
        for event in track {
            abs_ticks += event.delta.as_int() as u64;
            if let TrackEventKind::Midi { channel, message } = event.kind {
                let ch = channel.as_int();
                match message {
                    MidlyMessage::NoteOn { key, vel } => {
                        let k = key.as_int();
                        if vel.as_int() == 0 {
                            if let Some((start, velocity)) = active.remove(&(ch, k)) {
                                let length_ticks = abs_ticks.saturating_sub(start);
                                channel_notes
                                    .entry(ch)
                                    .or_default()
                                    .push(note_from_ticks(
                                        start,
                                        length_ticks,
                                        k,
                                        velocity,
                                        ticks_per_beat,
                                    ));
                            }
                        } else {
                            active.insert((ch, k), (abs_ticks, vel.as_int()));
                        }
                    }
                    MidlyMessage::NoteOff { key, .. } => {
                        let k = key.as_int();
                        if let Some((start, velocity)) = active.remove(&(ch, k)) {
                            let length_ticks = abs_ticks.saturating_sub(start);
                            channel_notes
                                .entry(ch)
                                .or_default()
                                .push(note_from_ticks(
                                    start,
                                    length_ticks,
                                    k,
                                    velocity,
                                    ticks_per_beat,
                                ));
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    let mut result: Vec<MidiChannelNotes> = channel_notes
        .into_iter()
        .map(|(channel, notes)| MidiChannelNotes { channel, notes })
        .collect();
    result.sort_by_key(|item| item.channel);
    Ok(result)
}

fn note_from_ticks(
    start: u64,
    length: u64,
    key: u8,
    velocity: u8,
    ticks_per_beat: u32,
) -> PianoRollNote {
    let start_beats = start as f32 / ticks_per_beat as f32;
    let length_beats = (length as f32 / ticks_per_beat as f32).max(0.25);
    PianoRollNote::new(start_beats, length_beats, key, velocity)
}
