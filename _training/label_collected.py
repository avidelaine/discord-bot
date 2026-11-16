"""Self-training, step 1: Whisper labels the collected sentences the wake word model rejected (2026-11-16).

The voice service keeps the near-misses (_training/collected/, score >= COLLECT_MIN_SCORE) without a transcript: the
wake word model said "no Bobby", so Whisper never ran. Here Whisper (not the wake word model) decides: a near-miss
whose transcript holds "Bobby" is a "Bobby" the model missed, said the owner's natural way, the best positive there
is; the others are real hard negatives. Writes <clip>.txt next to each clip (empty file = nothing understood).
Run when the GPU is free (not during a game), before retrain.sh:  voice/.venv/Scripts/python _training/label_collected.py
"""
import os
import re
import site
import sys
import wave
from pathlib import Path

for sp in site.getsitepackages():  # Windows: CUDA DLLs from the nvidia-* pip packages (as in voice/server.py)
    for sub in ("nvidia/cublas/bin", "nvidia/cudnn/bin"):
        p = Path(sp, sub)
        if p.is_dir():
            os.add_dll_directory(str(p))
            os.environ["PATH"] = f"{p}{os.pathsep}{os.environ['PATH']}"

import numpy as np
from faster_whisper import WhisperModel

HERE = Path(__file__).parent
BOBBY = re.compile(r"bob+(y|i|ie)\b|moby", re.I)


def main(device=sys.argv[1] if len(sys.argv) > 1 else "cuda"):
    todo = [w for w in sorted((HERE / "collected").rglob("*.wav")) if not w.with_suffix(".txt").exists()]
    print(f"{len(todo)} clips to label on {device}")
    if not todo:
        return
    model = WhisperModel("large-v3-turbo", device=device, compute_type="int8", download_root=str(HERE.parent / "models" / "whisper_models"))
    found = 0
    for i, w in enumerate(todo, 1):
        with wave.open(str(w)) as f:
            audio = np.frombuffer(f.readframes(f.getnframes()), np.int16).astype(np.float32) / 32768
        # hotwords: the same nudge the live service gives Whisper, so "Bobby" is spelled the way the filter expects
        segments, _ = model.transcribe(audio, beam_size=1, temperature=0, vad_filter=True, condition_on_previous_text=False, hotwords="Bobby")
        text = " ".join(s.text for s in segments).strip()
        w.with_suffix(".txt").write_text(text, encoding="utf-8")
        if BOBBY.search(text):
            found += 1
            print(f"  missed Bobby: {w.name}  {text!r}")
        if i % 100 == 0:
            print(f"  {i}/{len(todo)}")
    print(f"done: {found} missed 'Bobby' found among {len(todo)} rejected clips")


if __name__ == "__main__":
    main()
