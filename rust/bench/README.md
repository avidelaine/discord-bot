# Rust vs Node: resources and response times

Same machine, same models, same three recorded sentences on both lines, nobody at the PC.

- Clips (`bench-clips.txt`, 16 kHz mono WAVs from `_training/collected`): "Bobby, quelle heure est-il ?" twice
  (shortcut path: wake word, Whisper, spoken answer) and "Bobby, qui discorde ?" (local LLM path, no action).
  Questions that change nothing: no Discord action, no Claude call.
- One sentence every 20 s. Rust: `BOBBY_REPLAY=bench-clips.txt` (fed into the microphone's sentence path, in
  process). Node: `replay_node.py` (the voice service's `/transcribe`, then the bot's `/local`, which is what
  `voice/local.py` does for a microphone sentence).
- Resources: `sample.ps1` every 2 s (CPU time, working set, dedicated GPU memory per process: `bobby-app`,
  `llama-server`, Bobby's `python`, `node`), summarized by `summary.py`. Idle = 3 min with no sentence;
  load = 6 min of replay. CPU % is of one core.
- Response times per stage: `_logs/conversations.jsonl` (`timing`: stt, shortcut/llm, action, total), written the
  same way by both brains, plus the Rust `[local] N ms (wake W, whisper S)` log line and the voice service's
  `[wake] ... ms` line.

## Redo it

```powershell
# Rust line (from rust/, with the app built in C:\tmp\bobby)
$env:BOBBY_CONFIG = "$env:APPDATA\Bobby\bobby.toml.rust-test"
Start-Process C:\tmp\bobby\release\bobby-app.exe --hidden            # idle run: no BOBBY_REPLAY
powershell -File bench\sample.ps1 -Out rust-idle.csv -Seconds 180
# stop the app, then start it again with $env:BOBBY_REPLAY = "$PWD\bench\bench-clips.txt"
powershell -File bench\sample.ps1 -Out rust-load.csv -Seconds 360

# Node line: the app with the usual bobby.toml (Node bot + Python voice service)
powershell -File bench\sample.ps1 -Out node-idle.csv -Seconds 180
voice\.venv\Scripts\python.exe -I bench\replay_node.py bench\bench-clips.txt 6 > node-replay.csv   # with sample.ps1 -Seconds 360

python bench\summary.py rust-load.csv
```

## Results (2026-11-23, RTX 4060 8 GB, nobody at the PC)

Rust = app 0.2.1 in all-Rust mode. Node = app with the Node bot and the Python voice service. Same LLM
(`llama-server`, Qwen3-4B, identical on both lines). 18 replayed sentences per line.

### Resources (all Bobby processes; CPU % of one core; MB, average with max in brackets)

| | Rust idle | Node idle | Rust under replay | Node under replay |
|---|---|---|---|---|
| CPU | 1.7 % | 1.4 % | 10.2 % | 11.2 % |
| RAM | 4290 | 3791 | 4611 (4975) | 4677 (4975) |
| GPU memory | 4306 | 4288 | 4380 (4506) | 4374 (4506) |
| RAM without llama-server | 1475 (`bobby-app`) | 975 (app 59 + node 77 + python 839) | 1596 | 1552 |
| GPU memory without llama-server | 1125 | 1107 | 1192 (1317) | 1187 (1317) |

### Response times (ms, median with max in brackets)

| Stage | Rust | Node |
|---|---|---|
| Wake word scan | 36 (42) | 82 (88) |
| Whisper | 477 (751) | 496 (1227) |
| Shortcut question, heard to answer ready ("quelle heure") | 548 (785) | 583 (1313) |
| LLM stage ("qui discorde") | 1050 (2842) | 1118 (2787) |
| LLM question, heard to answer ready | 2188 (3317) | 2098 (3332) |
| Voice, first audio (warm) | 23, in process | 34, over HTTP |

Reading: response times are on par. Rust is ahead on the wake word (2.3x) and steadier on Whisper (no
1.2 s outlier), and the LLM dominates every LLM question on both lines. Rust's idle RAM is 500 MB higher: the
Discord client loads a second copy of the Piper voices (242 MB of models) next to the PC's, and a second wake word
model. Sharing them is the next change. The LLM path's end-to-end numbers mix in a small action step
(n = 2 or 3), so their 90 ms gap is noise.
