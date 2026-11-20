"""Python side of the gate parity check: WakeScan scores of every collected Discord clip.

  voice/.venv/Scripts/python.exe -I rust/bobby-infer/examples/gate_ref.py <main checkout> [out.csv]

Same chunks as the Rust example (20 ms at 48 kHz stereo), noise patched to bench.py's lcg_noise so both start equal.
"""
import sys
import wave
from pathlib import Path

root = Path(sys.argv[1])
out = Path(sys.argv[2]) if len(sys.argv) > 2 else root / "_training" / "gate_ref.csv"
sys.path.insert(0, str(root / "voice"))
import numpy as np  # noqa: E402
from openwakeword.model import Model  # noqa: E402
from wakescan import WakeScan  # noqa: E402


def lcg_noise(n):
    s, o = 12345, []
    for _ in range(n):
        s = (s * 6364136223846793005 + 1442695040888963407) % 2**64
        o.append((s >> 33) % 2000 - 1000)
    return np.array(o, np.int64)


noise = lcg_noise(64000)
np.random.randint = lambda lo, hi, size=None: noise.copy()
model = Model(wakeword_models=[str(root / "models" / "wakeword" / "bobby.onnx")], inference_framework="onnx")
files = sorted((root / "_training" / "collected").glob("*/*_discord_*.wav"))
with open(out, "w") as f:
    for p in files:
        with wave.open(str(p)) as w:
            if w.getframerate() != 16000 or w.getnchannels() != 1:
                continue
            mono = np.frombuffer(w.readframes(w.getnframes()), np.int16)
        raw = np.repeat(np.repeat(mono, 3)[:, None], 2, axis=1).tobytes()  # 16 kHz mono -> 48 kHz stereo, like wakescan's self-check
        scan = WakeScan(model)
        for i in range(0, len(raw), 3840):
            scan.feed(raw[: i + 3840])
        f.write(f"{p},{scan.feed(raw, final=True):.6f}\n")
print(f"{len(files)} clips -> {out}")
