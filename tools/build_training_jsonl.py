#!/usr/bin/env python3
"""
build_training_jsonl.py

Converts structured MIDI JSON data (from convert_midi_train.py) into
instruction-tuning JSONL pairs that match the LingStation2 AI Scores
output schema:

  {"schema_version":1, "start_beat":0, "length_beats":<float>,
   "tracks":[{"track_index":<int>, "track_name":<str>, "role":<str>,
              "instrumentation":<str>, "params":{}, "notes":[
                {"start":<float>, "length":<float>, "midi":<int>, "velocity":<int>}
              ]}]}

Each record becomes:
  {"messages": [
      {"role": "system",  "content": "<system_prompt>"},
      {"role": "user",    "content": "<DAW-style prompt with schema + context>"},
      {"role": "assistant","content": "<exact DAW JSON output>"}
  ]}

Usage:
    python tools/build_training_jsonl.py \
        --input  midi_train_structured/all_midis.jsonl \
        --output midi_train_structured/training_pairs.jsonl \
        [--max-notes 256]   # clip long examples to keep context manageable
        [--augment-transpose 0]  # +/-N semitone transposition variants (0 = off)
"""

import argparse
import json
import random
import sys
from pathlib import Path

# ---------------------------------------------------------------------------
# Music-theory snippets embedded as few-shot context for the system prompt
# ---------------------------------------------------------------------------
MUSIC_THEORY_RULES = """
== Music Theory Reference ==
Scales (MIDI intervals from root):
  Major:       0,2,4,5,7,9,11
  Natural minor:0,2,3,5,7,8,10
  Harmonic minor:0,2,3,5,7,8,11
  Pentatonic major:0,2,4,7,9
  Pentatonic minor:0,3,5,7,10
  Blues:        0,3,5,6,7,10
  Dorian:       0,2,3,5,7,9,10
  Phrygian:     0,1,3,5,7,8,10
  Mixolydian:   0,2,4,5,7,9,10
  Lydian:       0,2,4,6,7,9,11

Common chord voicings (semitones from root):
  Major triad:  0, 4, 7
  Minor triad:  0, 3, 7
  Dominant 7:   0, 4, 7, 10
  Major 7:      0, 4, 7, 11
  Minor 7:      0, 3, 7, 10
  Diminished:   0, 3, 6
  Augmented:    0, 4, 8
  Sus2:         0, 2, 7
  Sus4:         0, 5, 7

Rhythm note values (in beats at 4/4):
  whole=4.0, half=2.0, quarter=1.0, eighth=0.5, sixteenth=0.25, triplet=0.333

MIDI note reference:
  C4=60, D4=62, E4=64, F4=65, G4=67, A4=69, B4=71
  C5=72, Middle C = 60

Velocity guidelines:
  pp=32, p=48, mp=64, mf=80, f=96, ff=112

Instrumentation roles:
  melody, bass, chord, pad, arp, percussion, lead, counter_melody, rhythm

Common track setups by genre:
  Pop:    melody(lead synth), chord(piano/strings), bass(bass synth), percussion(drums)
  Jazz:   melody(trumpet/sax), chord(piano), bass(double bass/bass guitar), percussion(drums)
  EDM:    lead(synth), pad, bass(sub bass), arp, percussion
  Ambient:pad, melody(slow strings), bass(low pad)
  LoFi:   melody(piano), chord(guitar), bass(bass guitar), percussion(drum kit)
  Orchestral: melody(strings/brass), chord(strings), bass(low strings/tuba), percussion(timpani)
"""

SYSTEM_PROMPT = (
    "You are a professional music composer AI embedded in LingStation2 DAW. "
    "When given a musical prompt and track context, you output ONLY a valid JSON "
    "object matching the LingStation2 AI Scores schema — no markdown, no prose.\n\n"
    + MUSIC_THEORY_RULES
)

# Target schema description (kept in user prompt to reinforce format)
SCHEMA_HINT = (
    'Schema: {"schema_version":1,"start_beat":<float>,"length_beats":<float>,'
    '"tracks":[{"track_index":<int>,"track_name":<str>,"role":<str>,'
    '"instrumentation":<str>,"params":{},'
    '"notes":[{"start":<float>,"length":<float>,"midi":<int>,"velocity":<int>}]}]}'
)

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
KEY_TO_ROOT = {
    "C": 0, "C#": 1, "Db": 1, "D": 2, "D#": 3, "Eb": 3, "E": 4,
    "F": 5, "F#": 6, "Gb": 6, "G": 7, "G#": 8, "Ab": 8, "A": 9,
    "A#": 10, "Bb": 10, "B": 11,
}

ROLE_KEYWORDS = {
    "bass": "bass", "drum": "percussion", "perc": "percussion",
    "pad": "pad", "lead": "lead", "arp": "arp",
    "chord": "chord", "string": "chord", "piano": "chord",
    "melody": "melody", "trumpet": "melody", "sax": "melody", "flute": "melody",
    "guitar": "chord", "synth": "lead",
}


def guess_role(track_name: str) -> str:
    lower = (track_name or "").lower()
    for kw, role in ROLE_KEYWORDS.items():
        if kw in lower:
            return role
    return "melody"


def midi_item(start: float, length: float, midi: int, velocity: int) -> dict:
    return {"start": round(start, 4), "length": round(length, 4),
            "midi": int(midi), "velocity": int(velocity)}


def note_events_to_schema(
    note_events: list[dict],
    track_idx: int,
    track_name: str,
    instrumentation: str,
    max_notes: int,
    transpose: int = 0,
    start_beat_offset: float = 0.0,
) -> dict | None:
    notes = []
    for ev in note_events[:max_notes]:
        midi = int(ev.get("pitch", 60)) + transpose
        if not (0 <= midi <= 127):
            continue
        notes.append(midi_item(
            ev.get("time_beats", 0.0) - start_beat_offset,
            ev.get("duration_beats", 0.5),
            midi,
            ev.get("velocity", 80),
        ))
    if not notes:
        return None
    return {
        "track_index": track_idx,
        "track_name": track_name,
        "role": guess_role(track_name),
        "instrumentation": instrumentation,
        "params": {},
        "notes": notes,
    }


def build_user_prompt(
    midi_record: dict, track_name_list: list[str], max_bars: int
) -> str:
    bpm = midi_record.get("tempo_bpm", 120)
    key = midi_record.get("key_estimate", "C major")
    ts = midi_record.get("time_signature", {})
    num = ts.get("numerator", 4)
    den = ts.get("denominator", 4)
    source = midi_record.get("source_file", "unknown")
    bars = min(midi_record.get("bars", 8), max_bars)
    length_beats = bars * num

    track_list = "\n".join(
        f"  Track {i}: {n}" for i, n in enumerate(track_name_list)
    )
    style_hint = midi_record.get("training_text", "")[:300]

    prompt = (
        f"Track list:\n{track_list}\n\n"
        f"Start beat: 0.0\n"
        f"Bars: {bars} (length_beats = {length_beats:.2f})\n"
        f"Tempo: {bpm:.1f} BPM\n"
        f"Time signature: {num}/{den}\n"
        f"Key: {key}\n"
        f"Style/context: {style_hint}\n"
        f"Source: {source}\n\n"
        f"Compose notes for all tracks listed.\n\n"
        f"{SCHEMA_HINT}"
    )
    return prompt


# ---------------------------------------------------------------------------
# Main conversion
# ---------------------------------------------------------------------------
def convert(
    input_path: Path,
    output_path: Path,
    max_notes: int,
    augment_transpose: int,
    max_bars: int,
    seed: int,
):
    random.seed(seed)
    written = 0
    skipped = 0

    transpositions = [0]
    if augment_transpose > 0:
        for s in range(1, augment_transpose + 1):
            transpositions += [s, -s]

    with input_path.open(encoding="utf-8") as fin, \
         output_path.open("w", encoding="utf-8") as fout:

        for line_no, line in enumerate(fin, 1):
            line = line.strip()
            if not line:
                continue

            try:
                rec = json.loads(line)
            except json.JSONDecodeError as e:
                print(f"  [warn] line {line_no}: JSON decode error — {e}", file=sys.stderr)
                skipped += 1
                continue

            note_events_raw = rec.get("note_events", [])
            # note_events is a flat list; group by (track, channel)
            if not note_events_raw:
                skipped += 1
                continue

            # Group events by track index
            track_groups: dict[int, list[dict]] = {}
            for ev in note_events_raw:
                key = ev.get("track", 0)
                track_groups.setdefault(key, []).append(ev)

            sorted_tracks = sorted(track_groups.items())  # (track_num, [events])
            # Use "Track N" as names (original MIDI track names not stored)
            track_names = [f"Track {t}" for t, _ in sorted_tracks]

            # Remap event fields: start_beat/length_beats -> time_beats/duration_beats
            for _, evs in sorted_tracks:
                for ev in evs:
                    if "time_beats" not in ev:
                        ev["time_beats"] = ev.get("start_beat", 0.0)
                    if "duration_beats" not in ev:
                        ev["duration_beats"] = ev.get("length_beats", 0.5)
                    if "pitch" not in ev:
                        ev["pitch"] = ev.get("midi", 60)

            # Determine start_beat (minimum note time across all tracks)
            all_times = [
                ev.get("time_beats", 0.0)
                for _, evs in sorted_tracks
                for ev in evs
            ]
            start_beat = min(all_times) if all_times else 0.0
            bars = min(rec.get("bars", 8), max_bars)
            length_beats = bars * rec.get("time_signature", {}).get("numerator", 4)

            user_prompt = build_user_prompt(rec, [f"Track {t}" for t, _ in sorted_tracks], max_bars)

            for transpose in transpositions:
                tracks_out = []
                for tidx, (tnum, evs) in enumerate(sorted_tracks):
                    # tnum is the original MIDI track number (int)
                    tname = f"Track {tnum}"
                    instr = tname
                    schema_track = note_events_to_schema(
                        evs, tidx, tname, instr,
                        max_notes, transpose, start_beat
                    )
                    if schema_track:
                        tracks_out.append(schema_track)

                if not tracks_out:
                    continue

                # Clip notes whose start falls after length_beats
                for t in tracks_out:
                    t["notes"] = [n for n in t["notes"] if 0.0 <= n["start"] < length_beats]
                tracks_out = [t for t in tracks_out if t["notes"]]

                if not tracks_out:
                    continue

                assistant_response = {
                    "schema_version": 1,
                    "start_beat": 0.0,
                    "length_beats": round(length_beats, 4),
                    "tracks": tracks_out,
                }

                record = {
                    "messages": [
                        {"role": "system",    "content": SYSTEM_PROMPT},
                        {"role": "user",      "content": user_prompt},
                        {"role": "assistant", "content": json.dumps(assistant_response, ensure_ascii=False)},
                    ]
                }
                fout.write(json.dumps(record, ensure_ascii=False) + "\n")
                written += 1

    print(f"Written: {written} training pairs  |  Skipped: {skipped} records")
    print(f"Output: {output_path}")


# ---------------------------------------------------------------------------
def main():
    ap = argparse.ArgumentParser(description="Build LingStation2 instruction-tuning JSONL from MIDI structured data")
    ap.add_argument("--input",  default="midi_train_structured/all_midis.jsonl")
    ap.add_argument("--output", default="midi_train_structured/training_pairs.jsonl")
    ap.add_argument("--max-notes",          type=int, default=256,
                    help="Max notes per track per example (keeps token count manageable)")
    ap.add_argument("--max-bars",           type=int, default=16,
                    help="Max bars to include per example")
    ap.add_argument("--augment-transpose",  type=int, default=0,
                    help="Generate ±N semitone transposition variants (0 = disabled)")
    ap.add_argument("--seed",               type=int, default=42)
    args = ap.parse_args()

    input_path  = Path(args.input)
    output_path = Path(args.output)
    output_path.parent.mkdir(parents=True, exist_ok=True)

    if not input_path.exists():
        print(f"ERROR: Input file not found: {input_path}", file=sys.stderr)
        sys.exit(1)

    print(f"Reading: {input_path}")
    convert(input_path, output_path, args.max_notes, args.augment_transpose,
            args.max_bars, args.seed)


if __name__ == "__main__":
    main()
