#!/usr/bin/env python3
"""
Simple OpenAI-compatible API server using Hugging Face Transformers.
Compatible with the vLLM API client.
Windows version (.pyw for no console window).
"""

import argparse
import json
import logging
import sys
import warnings
from typing import Optional

import torch
from flask import Flask, request, jsonify
from transformers import AutoModelForCausalLM, AutoTokenizer

# Suppress deprecation warnings
warnings.filterwarnings('ignore', category=DeprecationWarning)

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)

app = Flask(__name__)

# Global state
model = None
tokenizer = None
model_name = None
device = None  # Will be set based on command line arg or auto-detect


def load_model(model_id: str):
    """Load model and tokenizer from Hugging Face."""
    global model, tokenizer, model_name
    
    try:
        logger.info(f"Loading model: {model_id}")
        logger.info(f"Using device: {device}")
        
        tokenizer = AutoTokenizer.from_pretrained(model_id)
        
        # Load model with appropriate dtype
        dtype = torch.float16 if device == "cuda" else torch.float32
        logger.info(f"Loading model weights from Hugging Face (may take a few minutes on first run)...")
        model = AutoModelForCausalLM.from_pretrained(
            model_id,
            dtype=dtype,
            # Don't use device_map on Windows/simple setups - move manually instead
        )
        
        # Move model to device manually
        if device == "cuda":
            model = model.to("cuda")
        
        # Set pad token if not set
        if tokenizer.pad_token is None:
            tokenizer.pad_token = tokenizer.eos_token
        
        model_name = model_id
        logger.info(f"Model loaded successfully: {model_id}")
        return True
    except Exception as e:
        logger.error(f"Failed to load model: {e}", exc_info=True)
        return False


def generate_response(messages: list, temperature: float = 0.7, max_tokens: int = 512) -> str:
    """Generate response using the model."""
    try:
        # Format messages into a single prompt
        prompt = ""
        for msg in messages:
            role = msg.get("role", "")
            content = msg.get("content", "")
            if role == "system":
                prompt += f"System: {content}\n"
            elif role == "user":
                prompt += f"User: {content}\n"
            elif role == "assistant":
                prompt += f"Assistant: {content}\n"
        
        prompt += "Assistant: "
        
        # Tokenize
        inputs = tokenizer.encode(prompt, return_tensors="pt").to(device)
        
        # Generate
        with torch.no_grad():
            outputs = model.generate(
                inputs,
                max_length=inputs.shape[1] + max_tokens,
                temperature=temperature,
                top_p=0.95,
                do_sample=True,
                pad_token_id=tokenizer.eos_token_id,
            )
        
        # Decode
        response = tokenizer.decode(outputs[0], skip_special_tokens=True)
        
        # Extract just the assistant's response (after "Assistant: ")
        if "Assistant: " in response:
            response = response.split("Assistant: ")[-1].strip()
        
        return response
    except Exception as e:
        logger.error(f"Error generating response: {e}", exc_info=True)
        raise


@app.route("/v1/models", methods=["GET"])
def models():
    """List available models (compatible with OpenAI API)."""
    if model_name is None:
        return jsonify({"object": "list", "data": []}), 200
    
    return jsonify({
        "object": "list",
        "data": [{"id": model_name, "object": "model", "owned_by": "transformers"}]
    }), 200


@app.route("/v1/chat/completions", methods=["POST"])
def chat_completions():
    """Chat completions endpoint (compatible with OpenAI API)."""
    if model is None or tokenizer is None:
        return jsonify({"error": "Model not loaded"}), 503
    
    try:
        data = request.get_json()
        
        messages = data.get("messages", [])
        temperature = data.get("temperature", 0.7)
        max_tokens = data.get("max_tokens", 512)
        
        # Validate inputs
        if not messages:
            return jsonify({"error": "No messages provided"}), 400
        
        logger.info(f"Generating response for {len(messages)} messages")
        
        # Generate response
        response_text = generate_response(messages, temperature, max_tokens)
        
        # Return in OpenAI-compatible format
        return jsonify({
            "object": "chat.completion",
            "model": model_name,
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": response_text
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 0,  # Not accurately tracked
                "completion_tokens": 0,
                "total_tokens": 0
            }
        }), 200
    
    except Exception as e:
        logger.error(f"Error in chat completions: {e}", exc_info=True)
        return jsonify({"error": str(e)}), 500


@app.route("/health", methods=["GET"])
def health():
    """Health check endpoint."""
    if model is None:
        return jsonify({"status": "loading"}), 503
    return jsonify({"status": "ready"}), 200


def main():
    parser = argparse.ArgumentParser(description="Transformers OpenAI-compatible API server")
    parser.add_argument("--model", type=str, required=True, help="Model ID to load")
    parser.add_argument("--host", type=str, default="127.0.0.1", help="Server host")
    parser.add_argument("--port", type=int, default=8001, help="Server port")
    parser.add_argument("--device", type=str, default="auto", choices=["auto", "cuda", "cpu"], help="Device: auto (GPU if available), cuda, or cpu")
    parser.add_argument("--debug", action="store_true", help="Enable debug mode")
    
    args = parser.parse_args()
    
    # Set device
    global device
    if args.device == "auto":
        device = "cuda" if torch.cuda.is_available() else "cpu"
    else:
        device = args.device
    
    logger.info(f"Using device: {device}")
    logger.info(f"CUDA available: {torch.cuda.is_available()}")
    if torch.cuda.is_available():
        logger.info(f"GPU: {torch.cuda.get_device_name(0)}")
    
    logger.info(f"Starting Transformers server with model: {args.model}")
    
    # Load model
    if not load_model(args.model):
        logger.error("Failed to load model, exiting")
        sys.exit(1)
    
    # Start server
    logger.info(f"Listening on {args.host}:{args.port}")
    app.run(host=args.host, port=args.port, debug=args.debug, threaded=True)


if __name__ == "__main__":
    main()
