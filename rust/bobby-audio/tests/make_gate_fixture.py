"""Builds the gate fixture: a synthetic 16 kHz signal and the cut points the Python Gate (voice/local.py) finds.
Run: voice/.venv/Scripts/python.exe -I rust/bobby-audio/tests/make_gate_fixture.py"""
import sys
import wave
from pathlib import Path

import numpy as np

here = Path(__file__).resolve().parent
sys.path.insert(0, str(here.parents[2] / "voice"))
import local  # noqa: E402

RATE = 16000
rng = np.random.RandomState(1)
total = 28.0
n = int(total * RATE)
t = np.arange(n) / RATE
# noise floor rises slowly from rms ~0.0012 to ~0.006
x = rng.uniform(-1, 1, n) * (0.002 + 0.009 * t / total)


def burst(a, b):  # speech-like: a few partials with a slow amplitude wobble
    i, j = int(a * RATE), int(b * RATE)
    tt = t[i:j]
    env = 0.6 + 0.4 * np.sin(2 * np.pi * 3 * tt)
    x[i:j] += 0.25 * env * (np.sin(2 * np.pi * 220 * tt) + 0.5 * np.sin(2 * np.pi * 660 * tt) + 0.3 * np.sin(2 * np.pi * 1500 * tt))


burst(1.0, 2.5)    # normal sentence
burst(3.3, 4.0)    # gap of 0.8 s before it, then ...
burst(4.3, 5.0)    # ... a 0.3 s gap: same sentence as the previous burst
burst(6.0, 6.25)   # shorter than 0.4 s
burst(7.0, 23.0)   # longer than 15 s: cut by the buffer limit, the rest starts a new sentence
burst(25.0, 26.2)  # with the raised noise floor

pcm = (np.clip(x, -1, 1) * 32767).astype(np.int16)
out = here / "fixtures"
out.mkdir(exist_ok=True)
with wave.open(str(out / "gate.wav"), "wb") as w:
    w.setnchannels(1), w.setsampwidth(2), w.setframerate(RATE), w.writeframes(pcm.tobytes())

with wave.open(str(out / "gate.wav"), "rb") as w:  # what both sides really see
    samples = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float32) / 32768

gate, lines = local.Gate(), []
for k in range(len(samples) // local.BLOCK):
    audio = gate.feed(samples[k * local.BLOCK:(k + 1) * local.BLOCK])
    if audio is not None:
        lines.append(f"{k} {len(audio)}")
(out / "gate.txt").write_text("\n".join(lines) + "\n")
print("\n".join(lines))
