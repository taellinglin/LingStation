from __future__ import annotations

import argparse
import json
import math
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any

import mido

NOTE_NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]

# Krumhansl key profiles
MAJOR_PROFILE = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88]
MINOR_PROFILE = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17]


def normalize_counter(counter: Counter[int]) -> list[float]:
    total = sum(counter.values()) or 1
    return [counter.get(i, 0) / total for i in range(12)]


def rotate(lst: list[float], n: int) -> list[float]:
    n %= len(lst)
    return lst[-n:] + lst[:-n]


def dot(a: list[float], b: list[float]) -> float:
    return sum(x * y for x, y in zip(a, b))


def estimate_key(note_events: list[dict[str, Any]]) -> str:
    if not note_events:
        return "Unknown"

    pc_weights: Counter[int] = Counter()
    for ev in note_events:
        pc = ev["midi"] % 12
        weight = max(0.05, float(ev["length_beats"]))
        pc_weights[pc] += weight

    profile = normalize_counter(pc_weights)

    best_name = "Unknown"
    best_score = -1e9

    for root in range(12):
        major_score = dot(profile, rotate(MAJOR_PROFILE, root))
        minor_score = dot(profile, rotate(MINOR_PROFILE, root))
        if major_score > best_score:
            best_score = major_score
            best_name = f"{NOTE_NAMES[root]} major"
        if minor_score > best_score:
            best_score = minor_score
            best_name = f"{NOTE_NAMES[root]} minor"

    return best_name


def detect_chord_name(pcs: set[int]) -> str:
    if len(pcs) < 3:
        return "N/A"

    for root in range(12):
        r = root
        major = {(r) % 12, (r + 4) % 12, (r + 7) % 12}
        minor = {(r) % 12, (r + 3) % 12, (r + 7) % 12}
        dim = {(r) % 12, (r + 3) % 12, (r + 6) % 12}
        aug = {(r) % 12, (r + 4) % 12, (r + 8) % 12}

        if major.issubset(pcs):
            return f"{NOTE_NAMES[root]}"
        if minor.issubset(pcs):
            return f"{NOTE_NAMES[root]}m"
        if dim.issubset(pcs):
            return f"{NOTE_NAMES[root]}dim"
        if aug.issubset(pcs):
            return f"{NOTE_NAMES[root]}aug"

    ordered = sorted(pcs)
    return "pcs:" + "-".join(str(x) for x in ordered)


def quantize(value: float, step: float = 0.25) -> float:
    return round(value / step) * step


def parse_midi_file(path: Path) -> dict[str, Any]:
    mid = mido.MidiFile(path)
    ticks_per_beat = mid.ticks_per_beat

    tempo_events: list[tuple[int, int]] = []
    time_sig_events: list[tuple[int, tuple[int, int]]] = []

    # Collect global meta events
    for track in mid.tracks:
        abs_tick = 0
        for msg in track:
            abs_tick += msg.time
            if msg.type == "set_tempo":
                tempo_events.append((abs_tick, msg.tempo))
            elif msg.type == "time_signature":
                time_sig_events.append((abs_tick, (msg.numerator, msg.denominator)))

    tempo_events.sort(key=lambda x: x[0])
    time_sig_events.sort(key=lambda x: x[0])

    initial_tempo = tempo_events[0][1] if tempo_events else 500000
    bpm = float(mido.tempo2bpm(initial_tempo))

    time_sig = time_sig_events[0][1] if time_sig_events else (4, 4)
    num, den = time_sig
    beats_per_bar = num * (4.0 / den)

    note_events: list[dict[str, Any]] = []

    # Parse notes per track
    for track_idx, track in enumerate(mid.tracks):
        abs_tick = 0
        active: dict[tuple[int, int], list[tuple[int, int]]] = defaultdict(list)

        for msg in track:
            abs_tick += msg.time
            if msg.type == "note_on" and msg.velocity > 0:
                channel = getattr(msg, "channel", 0)
                active[(channel, msg.note)].append((abs_tick, msg.velocity))
            elif msg.type == "note_off" or (msg.type == "note_on" and msg.velocity == 0):
                channel = getattr(msg, "channel", 0)
                key = (channel, msg.note)
                if active[key]:
                    start_tick, vel = active[key].pop()
                    length_tick = max(1, abs_tick - start_tick)
                    start_beat = start_tick / ticks_per_beat
                    length_beats = length_tick / ticks_per_beat
                    note_events.append(
                        {
                            "track": track_idx,
                            "channel": channel,
                            "start_beat": round(start_beat, 4),
                            "length_beats": round(length_beats, 4),
                            "midi": int(msg.note),
                            "velocity": int(vel),
                            "pitch_class": int(msg.note % 12),
                            "note_name": NOTE_NAMES[msg.note % 12],
                        }
                    )

        # Flush any dangling notes to end of track
        for (channel, midi_note), starts in active.items():
            for start_tick, vel in starts:
                length_tick = max(1, abs_tick - start_tick)
                start_beat = start_tick / ticks_per_beat
                length_beats = length_tick / ticks_per_beat
                note_events.append(
                    {
                        "track": track_idx,
                        "channel": channel,
                        "start_beat": round(start_beat, 4),
                        "length_beats": round(length_beats, 4),
                        "midi": int(midi_note),
                        "velocity": int(vel),
                        "pitch_class": int(midi_note % 12),
                        "note_name": NOTE_NAMES[midi_note % 12],
                    }
                )

    note_events.sort(key=lambda e: (e["start_beat"], e["track"], e["midi"]))

    max_end = 0.0
    for ev in note_events:
        max_end = max(max_end, ev["start_beat"] + ev["length_beats"])
    bars = int(math.ceil(max_end / beats_per_bar)) if beats_per_bar > 0 else 0

    # Chord snapshots by half-beat onset buckets
    chord_buckets: dict[float, set[int]] = defaultdict(set)
    for ev in note_events:
        b = quantize(float(ev["start_beat"]), 0.5)
        chord_buckets[b].add(int(ev["pitch_class"]))

    chord_events: list[dict[str, Any]] = []
    for beat_pos in sorted(chord_buckets.keys()):
        pcs = chord_buckets[beat_pos]
        if len(pcs) < 3:
            continue
        bar_index = int(beat_pos // beats_per_bar) + 1 if beats_per_bar > 0 else 1
        beat_in_bar = (beat_pos % beats_per_bar) + 1 if beats_per_bar > 0 else beat_pos + 1
        chord_events.append(
            {
                "bar": bar_index,
                "beat": round(beat_in_bar, 3),
                "global_beat": round(beat_pos, 3),
                "pitches": sorted(int(x) for x in pcs),
                "chord": detect_chord_name(pcs),
            }
        )

    # Rhythm patterns
    duration_hist: Counter[float] = Counter()
    onset_hist: Counter[float] = Counter()

    for ev in note_events:
        duration_hist[quantize(float(ev["length_beats"]), 0.25)] += 1

    starts = [float(e["start_beat"]) for e in note_events]
    starts.sort()
    for i in range(1, len(starts)):
        interval = max(0.0, starts[i] - starts[i - 1])
        onset_hist[quantize(interval, 0.25)] += 1

    key_estimate = estimate_key(note_events)

    track_activity: Counter[int] = Counter(ev["track"] for ev in note_events)

    training_text = (
        f"tempo={bpm:.2f} bpm; key={key_estimate}; time_signature={num}/{den}; "
        f"bars={bars}; notes={len(note_events)}; chords={len(chord_events)}; "
        f"top_tracks={track_activity.most_common(4)}"
    )

    return {
        "source_file": path.name,
        "source_path": str(path).replace('\\\\', '/'),
        "tempo_bpm": round(bpm, 3),
        "time_signature": {"numerator": num, "denominator": den},
        "key_estimate": key_estimate,
        "ticks_per_beat": ticks_per_beat,
        "beats_per_bar": round(beats_per_bar, 4),
        "bars": bars,
        "total_notes": len(note_events),
        "track_note_counts": {str(k): v for k, v in sorted(track_activity.items())},
        "chords": chord_events,
        "rhythm_patterns": {
            "duration_histogram": [
                {"length_beats": k, "count": v}
                for k, v in duration_hist.most_common(16)
            ],
            "onset_interval_histogram": [
                {"interval_beats": k, "count": v}
                for k, v in onset_hist.most_common(16)
            ],
        },
        "note_events": note_events,
        "training_text": training_text,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description="Convert MIDI files to structured training events")
    parser.add_argument("--input", default="midi_train", help="Input folder containing .mid/.midi files")
    parser.add_argument("--output", default="midi_train_structured", help="Output folder for JSON files")
    parser.add_argument(
        "--jsonl",
        default="midi_train_structured/all_midis.jsonl",
        help="Consolidated JSONL output path",
    )
    args = parser.parse_args()

    input_dir = Path(args.input)
    output_dir = Path(args.output)
    jsonl_path = Path(args.jsonl)

    output_dir.mkdir(parents=True, exist_ok=True)
    jsonl_path.parent.mkdir(parents=True, exist_ok=True)

    midi_files = sorted(
        [
            p
            for p in input_dir.rglob("*")
            if p.is_file() and p.suffix.lower() in {".mid", ".midi"}
        ]
    )

    if not midi_files:
        print(f"No MIDI files found in {input_dir}")
        return

    converted = 0
    failed: list[tuple[str, str]] = []

    with jsonl_path.open("w", encoding="utf-8") as jsonl_out:
        for midi_path in midi_files:
            try:
                data = parse_midi_file(midi_path)
                out_name = midi_path.stem + ".json"
                out_path = output_dir / out_name
                with out_path.open("w", encoding="utf-8") as f:
                    json.dump(data, f, ensure_ascii=False, indent=2)

                jsonl_out.write(json.dumps(data, ensure_ascii=False) + "\n")
                converted += 1
            except Exception as exc:  # noqa: BLE001
                failed.append((str(midi_path), str(exc)))

    print(f"Converted: {converted} files")
    print(f"JSON output dir: {output_dir}")
    print(f"JSONL output: {jsonl_path}")
    if failed:
        print(f"Failed: {len(failed)} files")
        for path, err in failed[:20]:
            print(f"  - {path}: {err}")


if __name__ == "__main__":
    main()
