# Transformers Server Documentation

This document explains how to use the Transformers backend for AI Scores in LingStation.

## Installation

Before using the Transformers backend, you need to install the required dependencies:

```bash
pip install transformers torch flask
```

For GPU support, also install the appropriate CUDA version of torch:

```bash
pip install torch torchvision torchaudio --index-url https://download.pytorch.org/whl/cu118
```

## Usage

1. **Select Backend**: In LingStation's AI Scores tab, select "Transformers" from the Backend dropdown
2. **Configure**: Set the model, URL, temperature, and max tokens as needed
3. **Start Server**: Click "Start Transformers"
4. **Generate Scores**: Use the AI Scores feature as normal

## Advantages of Transformers vs vLLM

| Aspect | Transformers | vLLM |
|--------|-------------|------|
| **Installation** | Simpler (just pip) | Requires Docker on Windows |
| **Memory Usage** | Lower | Higher (optimized for throughput) |
| **Speed** | Slower (single query) | Faster (batch optimized) |
| **Supported Models** | Any HuggingFace model | Specific models |
| **Cross-platform** | Native (Windows/Linux/Mac) | Docker-based on Windows |
| **VRAM Usage** | More efficient | Requires more VRAM for KV cache |

## Supported Models

The Transformers backend works with any causal language model from Hugging Face Hub:

- `meta-llama/Llama-2-7b-chat-hf`
- `mistralai/Mistral-7B-Instruct-v0.1`
- `Qwen/Qwen2.5-3B-Instruct`
- `Qwen/Qwen2.5-7B-Instruct`
- `OpenAssistant/oasst-sft-6-llama-30b`
- And many others...

## Performance Tuning

- **Temperature** (0.0 - 2.0): Lower values produce more deterministic/focused output
- **Max Tokens** (64 - 4096): Maximum length of generated response
- **GPU Memory**: Use smaller models (3B, 7B) for better compatibility

## Troubleshooting

### "transformers, torch, or flask not installed"
Make sure all dependencies are installed in the venv:
```bash
.venv/Scripts/pip install transformers torch flask
```

### Out of Memory (OOM)
Choose a smaller model:
- Try `Qwen/Qwen2.5-3B-Instruct` instead of 7B
- Reduce `max_tokens` setting
- Close other GPU-consuming applications

### Model Takes Too Long to Load
First load is slow as the model is downloaded and compiled. Subsequent loads are cached.

### Server Won't Connect
- Ensure the URL is correct (default: `http://127.0.0.1:8001`)
- Check the transformers_server.log file for errors
- Verify port is not in use by another application
