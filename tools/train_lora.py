#!/usr/bin/env python3
"""
train_lora.py

QLoRA fine-tuning of Qwen2.5-3B-Instruct on LingStation2 AI Scores training data.
Produces a LoRA adapter that generates tracks/notes JSON matching the DAW schema.

Requirements (install once):
    pip install peft bitsandbytes transformers accelerate trl datasets

Usage:
    python tools/train_lora.py \
        --model  Qwen/Qwen2.5-3B-Instruct \
        --data   midi_train_structured/training_pairs.jsonl \
        --output lora_adapters/lingstation_v1 \
        [--epochs 3] [--batch-size 2] [--lr 2e-4]

After training, the adapter is loaded by rag_server.py for inference.
"""

import argparse
import json
import os
import sys

# Fix: trl reads jinja templates without explicit encoding on Windows,
# causing UnicodeDecodeError with cp1252. Force UTF-8 for file I/O.
os.environ["PYTHONUTF8"] = "1"

from pathlib import Path

import torch


# ---------------------------------------------------------------------------
# Configuration helpers
# ---------------------------------------------------------------------------
def get_bnb_config():
    from transformers import BitsAndBytesConfig
    return BitsAndBytesConfig(
        load_in_4bit=True,
        bnb_4bit_use_double_quant=True,
        bnb_4bit_quant_type="nf4",
        bnb_4bit_compute_dtype=torch.bfloat16,
    )


def get_lora_config(target_modules):
    from peft import LoraConfig, TaskType
    return LoraConfig(
        task_type=TaskType.CAUSAL_LM,
        r=16,
        lora_alpha=32,
        target_modules=target_modules,
        lora_dropout=0.05,
        bias="none",
        inference_mode=False,
    )


# ---------------------------------------------------------------------------
# Dataset loading
# ---------------------------------------------------------------------------
def load_jsonl_dataset(path: str, tokenizer, max_length: int):
    """Load instruction-tuning JSONL and tokenize to model input_ids."""
    from datasets import Dataset

    rows = []
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if line:
                rows.append(json.loads(line))

    print(f"  Loaded {len(rows)} training examples from {path}")

    def to_text(example):
        """Convert messages list to a single chat-formatted string."""
        return tokenizer.apply_chat_template(
            example["messages"],
            tokenize=False,
            add_generation_prompt=False,
        )

    texts = [to_text(r) for r in rows]

    def tokenize_fn(text):
        encoded = tokenizer(
            text,
            truncation=True,
            max_length=max_length,
            padding=False,
        )
        encoded["labels"] = encoded["input_ids"].copy()
        return encoded

    tokenized = [tokenize_fn(t) for t in texts]
    return Dataset.from_list(tokenized)


# ---------------------------------------------------------------------------
# Training
# ---------------------------------------------------------------------------
def train(args):
    from transformers import AutoModelForCausalLM, AutoTokenizer
    from peft import get_peft_model, prepare_model_for_kbit_training
    from trl import SFTTrainer, SFTConfig
    from datasets import Dataset
    import json

    print(f"Loading tokenizer: {args.model}")
    tokenizer = AutoTokenizer.from_pretrained(
        args.model,
        trust_remote_code=True,
        padding_side="right",
    )
    if tokenizer.pad_token is None:
        tokenizer.pad_token = tokenizer.eos_token

    print(f"Loading model (4-bit QLoRA): {args.model}")
    bnb_config = get_bnb_config()
    model = AutoModelForCausalLM.from_pretrained(
        args.model,
        quantization_config=bnb_config,
        device_map="auto",
        trust_remote_code=True,
    )
    model = prepare_model_for_kbit_training(model)

    target_modules = ["q_proj", "k_proj", "v_proj", "o_proj",
                      "gate_proj", "up_proj", "down_proj"]
    lora_config = get_lora_config(target_modules)
    model = get_peft_model(model, lora_config)
    model.print_trainable_parameters()

    # Load raw messages and format as chat strings for SFTTrainer
    print(f"Loading dataset: {args.data}")
    rows = []
    with open(args.data, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if line:
                rows.append(json.loads(line))

    def format_chat(example):
        text = tokenizer.apply_chat_template(
            example["messages"],
            tokenize=False,
            add_generation_prompt=False,
        )
        return {"text": text}

    raw_ds = Dataset.from_list(rows)
    raw_ds = raw_ds.map(format_chat, remove_columns=raw_ds.column_names)

    split = raw_ds.train_test_split(test_size=0.1, seed=42)
    train_ds = split["train"]
    eval_ds  = split["test"]
    print(f"  Train: {len(train_ds)}  Eval: {len(eval_ds)}")

    output_dir = args.output
    Path(output_dir).mkdir(parents=True, exist_ok=True)

    sft_config = SFTConfig(
        output_dir=output_dir,
        num_train_epochs=args.epochs,
        per_device_train_batch_size=args.batch_size,
        per_device_eval_batch_size=args.batch_size,
        gradient_accumulation_steps=args.grad_accum,
        learning_rate=args.lr,
        lr_scheduler_type="cosine",
        warmup_ratio=0.05,
        fp16=False,
        bf16=torch.cuda.is_bf16_supported(),
        logging_steps=10,
        eval_strategy="steps",
        eval_steps=50,
        save_strategy="steps",
        save_steps=100,
        save_total_limit=3,
        load_best_model_at_end=True,
        metric_for_best_model="eval_loss",
        report_to="none",
        dataloader_num_workers=0,
        max_length=args.max_length,
        dataset_text_field="text",
        packing=False,
    )

    trainer = SFTTrainer(
        model=model,
        args=sft_config,
        train_dataset=train_ds,
        eval_dataset=eval_ds,
        processing_class=tokenizer,
    )

    print("Starting training...")
    trainer.train()

    print(f"Saving LoRA adapter to: {output_dir}")
    model.save_pretrained(output_dir)
    tokenizer.save_pretrained(output_dir)
    print("Done.")


# ---------------------------------------------------------------------------
def main():
    ap = argparse.ArgumentParser(description="QLoRA fine-tuning for LingStation2 AI Scores")
    ap.add_argument("--model",       default="Qwen/Qwen2.5-3B-Instruct",
                    help="Base model id (HuggingFace or local path)")
    ap.add_argument("--data",        default="midi_train_structured/training_pairs.jsonl")
    ap.add_argument("--output",      default="lora_adapters/lingstation_v1")
    ap.add_argument("--epochs",      type=int,   default=3)
    ap.add_argument("--batch-size",  type=int,   default=2)
    ap.add_argument("--grad-accum",  type=int,   default=8,
                    help="Gradient accumulation steps (effective batch = batch_size * grad_accum)")
    ap.add_argument("--lr",          type=float, default=2e-4)
    ap.add_argument("--max-length",  type=int,   default=4096,
                    help="Max token sequence length (longer = more VRAM)")
    args = ap.parse_args()

    # Validate inputs
    if not Path(args.data).exists():
        print(f"ERROR: Training data not found: {args.data}")
        print("  Run:  python tools/build_training_jsonl.py  first")
        return

    train(args)


if __name__ == "__main__":
    main()
