# BobbY — a fully local voice assistant for Discord

Say "Bobby, …" in a voice channel and get an answer in the text channel. Everything runs on your own machine: speech-to-text with Whisper, generation with a quantized Llama 3.2 through llama.cpp, no cloud API anywhere.

Three services, one `docker compose up`.

```
Discord voice ──► bot (Node, discord.js)
                    │  raw PCM stream (48 kHz stereo)
                    ▼
                  stt (Python, FastAPI, faster-whisper on CUDA)
                    │  ffmpeg → 16 kHz mono → Whisper large-v3-turbo
                    │  wake-word "bobby" + debounce
                    ▼
                  llm (Python, FastAPI, llama-cpp-python on CUDA)
                    │  Llama-3.2-3B-Instruct Q4_K_M, Llama 3 chat template
                    ▼
                  bot ──► reply posted in the text channel
```

## Why it is built this way

- **Streaming, not files.** The bot pipes the Opus-decoded voice stream straight into the STT service with a chunked HTTP request; audio is never written to disk on the bot side.
- **One speaker at a time, no GPU contention.** The STT service serializes transcriptions behind a lock, and the bot ignores overlapping streams from the same user.
- **Wake word in the transcript, not in the audio.** Whisper gets an `initial_prompt` that mentions the assistant's name, then a regex detects it; a 2-second debounce drops duplicate triggers.
- **Health-gated startup.** Each container exposes `/health`; Compose waits for the LLM to finish loading before starting the STT service, and for both before starting the bot.
- **GPU by configuration.** `N_GPU_LAYERS` controls how much of the model is offloaded; set it to `0` for a CPU-only run.

## Run it

Requirements: Docker with the NVIDIA container toolkit, a Discord application with a bot token, and a GGUF model file.

```bash
# 1. Model
mkdir -p models && curl -L -o models/Llama-3.2-3B-Instruct-Q4_K_M.gguf \
  https://huggingface.co/bartowski/Llama-3.2-3B-Instruct-GGUF/resolve/main/Llama-3.2-3B-Instruct-Q4_K_M.gguf

# 2. Secrets
cat > .env <<EOF
DISCORD_TOKEN=...
CLIENT_ID=...
GUILD_ID=...
EOF

# 3. Start (first run downloads the Whisper model)
docker compose up --build
```

In Discord, join a voice channel and run `/join`.

## Configuration

| Variable | Service | Default | Role |
|---|---|---|---|
| `STT_MODEL` | stt | `large-v3-turbo` | faster-whisper model |
| `STT_DEVICE` / `STT_COMPUTE_TYPE` | stt | `cuda` / `float16` | Inference device and precision |
| `RECORD_SILENCE_MS` | bot | `2000` | Silence that ends a capture |
| `MODEL_PATH` | llm | `/models/Llama-3.2-3B-Instruct-Q4_K_M.gguf` | GGUF model |
| `N_GPU_LAYERS` | llm | `33` | Layers offloaded to the GPU (`0` = CPU) |

## Layout

```
bot/   discord.js client, voice capture, slash command
stt/   FastAPI service: ffmpeg resampling, faster-whisper, wake word, call to llm
llm/   FastAPI service around llama-cpp-python
```

## Limitations

French is hard-coded as the transcription language (`language="fr"` in `stt/stt.py`). Replies are text only; there is no text-to-speech back into the voice channel.
