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
