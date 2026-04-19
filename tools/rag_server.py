#!/usr/bin/env python3
"""
rag_server.py

Drop-in replacement for transformers_server.py that adds:
  1. RAG (Retrieval-Augmented Generation) — injects relevant music-theory
     and MIDI examples into the system prompt before generation.
  2. Optional LoRA adapter loading from lora_adapters/lingstation_v1.
  3. JSON schema validation + auto-repair loop at inference time.

Fully compatible with LingStation2 — exposes the same OpenAI-compatible
HTTP API on port 8001 (or --port).

Usage:
    python tools/rag_server.py
    python tools/rag_server.py --adapter lora_adapters/lingstation_v1
    python tools/rag_server.py --rag-index midi_train_structured/all_midis.jsonl

Requirements (same as transformers_server.py, plus):
    pip install flask sentence-transformers faiss-cpu peft
"""

import argparse
import json
import logging
import os
import re
import sys
import time
from pathlib import Path
from typing import Optional

import torch
from flask import Flask, Response, jsonify, request

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s - %(name)s - %(levelname)s - %(message)s",
)
logger = logging.getLogger(__name__)

app = Flask(__name__)

# ---------------------------------------------------------------------------
# Global model / RAG state (loaded at startup)
# ---------------------------------------------------------------------------
_model      = None
_tokenizer  = None
_rag_index  = None   # RAGIndex instance or None
_args       = None


# ---------------------------------------------------------------------------
# JSON Schema validation
# ---------------------------------------------------------------------------
REQUIRED_NOTE_KEYS = {"start", "length", "midi", "velocity"}


def validate_score_json(obj: dict) -> list[str]:
    """Return list of validation errors (empty = valid)."""
    errors = []
    if not isinstance(obj, dict):
        return ["root is not an object"]
    if "tracks" not in obj:
        errors.append("missing 'tracks' key")
        return errors
    if not isinstance(obj["tracks"], list) or len(obj["tracks"]) == 0:
        errors.append("'tracks' must be a non-empty array")
    for ti, t in enumerate(obj.get("tracks", [])):
        if not isinstance(t.get("notes"), list):
            errors.append(f"track[{ti}].notes is not an array")
            continue
        for ni, n in enumerate(t["notes"]):
            for k in ("start", "length", "midi"):
                if k not in n:
                    errors.append(f"track[{ti}].notes[{ni}] missing '{k}'")
    return errors


def repair_score_json(text: str) -> Optional[dict]:
    """
    Try multiple strategies to extract and repair a valid score JSON from
    raw model output.
    """
    # 1. Try direct parse
    try:
        obj = json.loads(text)
        errs = validate_score_json(obj)
        if not errs:
            return obj
    except json.JSONDecodeError:
        pass

    # 2. Extract from code fences
    for fence_match in re.finditer(r"```(?:json)?\s*([\s\S]*?)```", text):
        try:
            obj = json.loads(fence_match.group(1))
            if not validate_score_json(obj):
                return obj
        except json.JSONDecodeError:
            pass

    # 3. Balanced brace scan looking for {"tracks": ...}
    for start_idx in range(len(text)):
        if text[start_idx] != "{":
            continue
        depth = 0
        for end_idx in range(start_idx, len(text)):
            c = text[end_idx]
            if c == "{":
                depth += 1
            elif c == "}":
                depth -= 1
                if depth == 0:
                    candidate = text[start_idx : end_idx + 1]
                    try:
                        obj = json.loads(candidate)
                        errs = validate_score_json(obj)
                        if not errs:
                            return obj
                    except json.JSONDecodeError:
                        pass
                    break

    # 4. Array format [ {...}, ... ]
    for start_idx in range(len(text)):
        if text[start_idx] != "[":
            continue
        depth = 0
        for end_idx in range(start_idx, len(text)):
            c = text[end_idx]
            if c == "[":
                depth += 1
            elif c == "]":
                depth -= 1
                if depth == 0:
                    candidate = text[start_idx : end_idx + 1]
                    try:
                        arr = json.loads(candidate)
                        if isinstance(arr, list) and arr and "notes" in arr[0]:
                            wrapped = {
                                "schema_version": 1,
                                "start_beat": 0.0,
                                "length_beats": 16.0,
                                "tracks": arr,
                            }
                            if not validate_score_json(wrapped):
                                return wrapped
                    except json.JSONDecodeError:
                        pass
                    break

    return None


# ---------------------------------------------------------------------------
# RAG Index
# ---------------------------------------------------------------------------
class RAGIndex:
    """
    Lightweight FAISS-backed semantic retrieval over music theory snippets
    and MIDI structured summaries.
    """

    # Embedded music theory chunks
    THEORY_CHUNKS = [
        "C major scale: C D E F G A B (MIDI intervals 0,2,4,5,7,9,11)",
        "A minor scale (natural): A B C D E F G (MIDI intervals 0,2,3,5,7,8,10)",
        "G major pentatonic: G A B D E (MIDI intervals 0,2,4,7,9)",
        "A minor pentatonic: A C D E G (MIDI intervals 0,3,5,7,10)",
        "Blues scale (A): A C D Eb E G (MIDI 69,72,74,75,76,79)",
        "Dorian mode (D): D E F G A B C (minor with raised 6th)",
        "Mixolydian mode (G): G A B C D E F (major with flat 7th)",
        "Chord: C major triad C-E-G (MIDI 60,64,67)",
        "Chord: A minor triad A-C-E (MIDI 69,72,76)",
        "Chord: G dominant 7 G-B-D-F (MIDI 67,71,74,77)",
        "Chord: F major 7 F-A-C-E (MIDI 65,69,72,76)",
        "Chord: D minor 7 D-F-A-C (MIDI 62,65,69,72)",
        "Rhythm: whole note=4 beats, half=2 beats, quarter=1, eighth=0.5, sixteenth=0.25",
        "Rhythm: swing eighth notes — long-short pattern, long≈0.67 beats, short≈0.33 beats",
        "Velocity: pianissimo=32, piano=48, mezzo-piano=64, mezzo-forte=80, forte=96, fortissimo=112",
        "Bass writing: bass notes typically in octave 2-3 (MIDI 36-47), follow chord roots",
        "Melody writing: commonly in octave 4-5 (MIDI 60-84), stepwise motion with occasional leaps",
        "Chord voicing: spread chords use wider intervals, closed chords use narrow intervals",
        "Pop song structure: intro 4 bars, verse 8 bars, chorus 8 bars, bridge 4 bars",
        "Jazz comping: syncopated chord hits on off-beats, use 7th and 9th chords",
        "EDM drop: high-energy section with stabs on beat 1 and 3, sub-bass sustains",
        "Ambient music: long sustained pads, slow harmonic rhythm, legato melodies",
        "LoFi hip hop: slightly swung triplet feel, pitch-down samples, mellow chords",
        "Orchestral strings: violins play melody, violas play inner voices, cello/bass play bass line",
        "Percussion GM mapping: kick=36, snare=38, closed hi-hat=42, open hi-hat=46, crash=49, ride=51",
        "MIDI note reference: C4=60, D4=62, E4=64, F4=65, G4=67, A4=69, B4=71, C5=72",
        "Common chord progressions: I-V-vi-IV (C-G-Am-F); ii-V-I (jazz: Dm7-G7-Cmaj7); i-VI-III-VII (Am-F-C-G)",
        "Transposition: adding N semitones shifts pitch up N half-steps; +12 = one octave up",
        "Counterpoint: melody and bass should move in contrary or oblique motion when possible",
        "Syncopation: placing accents on weak beats (beats 2,4 or off-beats) for rhythmic interest",
    ]

    def __init__(self, jsonl_path: Optional[str] = None, top_k: int = 3):
        try:
            import faiss
            from sentence_transformers import SentenceTransformer
        except ImportError:
            logger.warning("RAG disabled: install sentence-transformers and faiss-cpu")
            self._enabled = False
            return

        self._enabled = True
        self._top_k = top_k
        logger.info("Loading sentence encoder for RAG…")
        self._encoder = SentenceTransformer(
            "all-MiniLM-L6-v2", device="cuda" if torch.cuda.is_available() else "cpu"
        )

        self._chunks: list[str] = list(self.THEORY_CHUNKS)

        # Optionally load MIDI summaries from structured JSONL
        if jsonl_path and Path(jsonl_path).exists():
            logger.info(f"Indexing MIDI summaries from {jsonl_path}…")
            loaded = 0
            with open(jsonl_path, encoding="utf-8") as f:
                for line in f:
                    line = line.strip()
                    if not line:
                        continue
                    try:
                        rec = json.loads(line)
                        summary = rec.get("training_text", "")
                        if summary:
                            self._chunks.append(summary[:512])
                            loaded += 1
                    except json.JSONDecodeError:
                        pass
            logger.info(f"  Indexed {loaded} MIDI training summaries + {len(self.THEORY_CHUNKS)} theory chunks")

        logger.info(f"Encoding {len(self._chunks)} RAG chunks…")
        embeddings = self._encoder.encode(
            self._chunks, batch_size=64, show_progress_bar=True, convert_to_numpy=True
        )
        import faiss
        self._index = faiss.IndexFlatL2(embeddings.shape[1])
        self._index.add(embeddings)
        logger.info("RAG index ready.")

    def retrieve(self, query: str) -> str:
        """Return top-K relevant chunks concatenated as a string."""
        if not self._enabled:
            return ""
        import numpy as np
        q_emb = self._encoder.encode([query], convert_to_numpy=True)
        distances, indices = self._index.search(q_emb, self._top_k)
        results = [self._chunks[i] for i in indices[0] if i < len(self._chunks)]
        if not results:
            return ""
        return "\n".join(f"• {r}" for r in results)


# ---------------------------------------------------------------------------
# Generation with retry/repair
# ---------------------------------------------------------------------------
MAX_REPAIR_ATTEMPTS = 3
REPAIR_PROMPT_SUFFIX = (
    "\n\nThe previous response had JSON errors. "
    "Return ONLY valid JSON matching the schema. No prose, no markdown."
)


def generate_with_repair(messages: list[dict], max_tokens: int, temperature: float) -> str:
    """Generate and attempt to repair JSON up to MAX_REPAIR_ATTEMPTS times."""
    for attempt in range(MAX_REPAIR_ATTEMPTS):
        input_ids = _tokenizer.apply_chat_template(
            messages,
            tokenize=True,
            add_generation_prompt=True,
            return_tensors="pt",
        ).to(_model.device)

        with torch.no_grad():
            output = _model.generate(
                input_ids,
                max_new_tokens=max_tokens,
                temperature=temperature if temperature > 0 else 1.0,
                do_sample=(temperature > 0),
                pad_token_id=_tokenizer.eos_token_id,
            )

        generated = output[0][input_ids.shape[1]:]
        text = _tokenizer.decode(generated, skip_special_tokens=True)

        repaired = repair_score_json(text)
        if repaired is not None:
            return json.dumps(repaired, ensure_ascii=False)

        # Repair attempt — append error hint to messages
        if attempt < MAX_REPAIR_ATTEMPTS - 1:
            logger.warning(f"Attempt {attempt + 1}: invalid JSON, retrying with repair prompt")
            messages = messages + [
                {"role": "assistant", "content": text},
                {"role": "user",      "content": REPAIR_PROMPT_SUFFIX},
            ]

    logger.warning("All repair attempts exhausted; returning raw text")
    return text


# ---------------------------------------------------------------------------
# Flask endpoints
# ---------------------------------------------------------------------------
@app.route("/v1/models", methods=["GET"])
def list_models():
    return jsonify({
        "object": "list",
        "data": [{"id": _args.model, "object": "model"}],
    })


@app.route("/v1/chat/completions", methods=["POST"])
def chat_completions():
    data = request.get_json(force=True)
    messages: list[dict] = data.get("messages", [])
    max_tokens:  int   = int(data.get("max_tokens",  2048))
    temperature: float = float(data.get("temperature", 0.7))

    if not messages:
        return jsonify({"error": "messages is required"}), 400

    # RAG injection — augment system prompt with retrieved context
    if _rag_index is not None:
        # Build query from the last user message
        user_query = next(
            (m["content"] for m in reversed(messages) if m.get("role") == "user"), ""
        )
        context = _rag_index.retrieve(user_query)
        if context:
            # Inject into system prompt (prepend to existing)
            augmented = list(messages)
            sys_idx = next((i for i, m in enumerate(augmented) if m.get("role") == "system"), None)
            rag_addition = f"\n\n== Retrieved music theory context ==\n{context}"
            if sys_idx is not None:
                augmented[sys_idx] = {
                    "role": "system",
                    "content": augmented[sys_idx]["content"] + rag_addition,
                }
            else:
                augmented.insert(0, {"role": "system", "content": rag_addition.strip()})
            messages = augmented

    t0 = time.time()
    result_text = generate_with_repair(messages, max_tokens, temperature)
    elapsed = time.time() - t0

    return jsonify({
        "id": "chatcmpl-rag",
        "object": "chat.completion",
        "model": _args.model,
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": result_text},
            "finish_reason": "stop",
        }],
        "usage": {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0},
        "_elapsed_seconds": round(elapsed, 2),
    })


# ---------------------------------------------------------------------------
# Startup
# ---------------------------------------------------------------------------
def load_model(model_path: str, adapter_path: Optional[str]):
    global _model, _tokenizer
    from transformers import AutoModelForCausalLM, AutoTokenizer

    logger.info(f"Loading tokenizer: {model_path}")
    _tokenizer = AutoTokenizer.from_pretrained(model_path, trust_remote_code=True)
    if _tokenizer.pad_token is None:
        _tokenizer.pad_token = _tokenizer.eos_token

    logger.info(f"Loading model: {model_path}")
    _model = AutoModelForCausalLM.from_pretrained(
        model_path,
        torch_dtype=torch.bfloat16 if torch.cuda.is_bf16_supported() else torch.float16,
        device_map="auto",
        trust_remote_code=True,
    )

    if adapter_path and Path(adapter_path).exists():
        logger.info(f"Loading LoRA adapter: {adapter_path}")
        from peft import PeftModel
        _model = PeftModel.from_pretrained(_model, adapter_path)
        _model = _model.merge_and_unload()
        logger.info("LoRA adapter merged.")

    _model.eval()
    logger.info(f"Model loaded successfully: {model_path}")


def main():
    global _rag_index, _args

    ap = argparse.ArgumentParser(description="LingStation2 RAG inference server")
    ap.add_argument("--model",     default="Qwen/Qwen2.5-3B-Instruct")
    ap.add_argument("--adapter",   default=None,
                    help="Path to LoRA adapter (e.g. lora_adapters/lingstation_v1)")
    ap.add_argument("--rag-index", default=None,
                    help="Path to MIDI JSONL for RAG index (e.g. midi_train_structured/all_midis.jsonl)")
    ap.add_argument("--top-k",     type=int, default=3,
                    help="Number of RAG chunks to retrieve per query")
    ap.add_argument("--host",      default="127.0.0.1")
    ap.add_argument("--port",      type=int, default=8001)
    _args = ap.parse_args()

    # Load RAG index (may take ~30s on first run to download MiniLM)
    logger.info("Initialising RAG index…")
    _rag_index = RAGIndex(jsonl_path=_args.rag_index, top_k=_args.top_k)

    # Load model
    load_model(_args.model, _args.adapter)

    logger.info(f"Listening on {_args.host}:{_args.port}")
    from werkzeug.serving import run_simple
    run_simple(_args.host, _args.port, app, use_reloader=False, threaded=False)


if __name__ == "__main__":
    main()
