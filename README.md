# Bobby, assistant vocal Discord

Say "Bobby, move Zyklon to Gaming" (or "Bobby, déplace Zyklon dans Gaming") in a voice channel: Bobby understands it, does it, and confirms out loud in the language you spoke. French and English both work. Everything runs locally on Windows and costs nothing.

```
Discord voice ──► bot/ (Node, discord.js, @discordjs/voice 0.19 with DAVE)
                    │  one PCM stream per speaker, cut after 600 ms of silence
                    ▼
                  voice/ (Python) ── Whisper large-v3-turbo on the GPU, French/English auto-detected, server names as hotwords
                    │  text
                    ▼
                  bot/brain.js ── "Bobby" found? ── llama-server (Qwen3-4B-Instruct, JSON constrained to the action list)
                    │  names matched against the real server roster, guardrails
                    ▼
                  Discord action + spoken confirmation in the same language (Piper, French and English voices)
```

## Install (once)

Requirements: Windows, an NVIDIA GPU, Node 22+, Python 3.11, ffmpeg on the PATH.

- `_bin/llama/`: llama.cpp Windows CUDA 12.4 build (`llama-bXXXX-bin-win-cuda-12.4-x64.zip` + `cudart-…zip`, from the llama.cpp GitHub releases).
- `models/Qwen3-4B-Instruct-2507-Q4_K_M.gguf`: from `unsloth/Qwen3-4B-Instruct-2507-GGUF` on Hugging Face.
- `models/piper/fr_FR-siwis-medium.onnx`, `models/piper/en_US-lessac-medium.onnx` and their `.onnx.json`: from `rhasspy/piper-voices`.
- Python: `py -3.11 -m venv voice/.venv`, then `voice/.venv/Scripts/pip install faster-whisper fastapi "uvicorn[standard]" scipy piper-tts nvidia-cublas-cu12 "nvidia-cudnn-cu12==9.*"`.
- Node: `cd bot && npm install`.

Whisper downloads itself into `models/whisper_models` on first start.

## Configure

`.env` at the root:

| Variable | Role |
|---|---|
| `DISCORD_TOKEN`, `GUILD_ID` | Bot token and server ID |
| `ALLOWED_USERS` | Discord IDs allowed to command Bobby, comma-separated. Empty: server owner only. |
| `RECORD_SILENCE_MS` | Silence that ends a sentence (default 600). Lower is faster but cuts you off if you pause. |

The bot needs the **Connect**, **Speak**, **Mute Members**, **Deafen Members**, **Move Members** and **Send Messages** permissions. No privileged intent is required.

## Run

`start.cmd` opens the three parts (LLM, voice, bot). In Discord, join a voice channel and type `/join`.

Each command prints its timings: `[temps] stt … · llm … · action … · total …`.

## What Bobby understands

Mute and unmute, deafen and undeafen, move, disconnect, post a message in a text channel, a to-do list, "annule" (undoes the last action), "tu peux partir". "Moi", "ici", "tout le monde", "sauf X" and several requests in one sentence all work. Anything else gets a short spoken answer.

Disconnecting someone, or acting on three people or more, waits for a "oui". If a name is ambiguous, Bobby asks which one you meant.

## Tests

- `node bot/wake.test.js`: detection of "Bobby" in the transcript.
- `node bot/intent.test.js` (llama-server running): comprehension score on French and English sentences, and LLM latency. Add every sentence Bobby gets wrong in real use.
