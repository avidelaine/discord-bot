"""Scores wake word models the way the voice service does (max over the sentence, 0.5 s of silence on both sides).

python bench.py models/bobby.onnx [other.onnx ...]
Sets: test_cont (held-out synthetic "Bobby, <request>"), recordings (the owner's own, held-out ones), collected
(real Discord/PC sentences: positive when the transcript holds "Bobby", if any were collected).
"""
import re
import sys
import wave
from pathlib import Path

import numpy as np
from openwakeword.model import Model

HERE = Path(__file__).parent
HELD_OUT_POS = {f"pos_{i:02d}" for i in [2] + list(range(5, 30))}  # see voice/server.py WAKE_THRESHOLD note


def load(p):
    with wave.open(str(p)) as w:
        a = np.frombuffer(w.readframes(w.getnframes()), np.int16)
        return a if w.getframerate() == 16000 else None


def score(model, pcm):
    pad = np.zeros(8000, np.int16)
    pcm = np.concatenate([pad, pcm, pad])
    model.reset()
    return max((max(model.predict(pcm[i : i + 1280]).values()) for i in range(0, len(pcm) - 1279, 1280)), default=0.0)


def sets():
    t = HERE / "test_cont"
    yield "synthetic Bobby+request", sorted((t / "pos").glob("*.wav")), sorted((t / "neg").glob("*.wav"))
    r = HERE / "recordings"
    yield "owner recordings", [p for p in sorted((r / "pos").glob("*.wav")) if p.stem in HELD_OUT_POS], sorted((r / "neg").glob("*.wav"))
    pos, neg = [], []
    for w in sorted((HERE / "collected").rglob("*.wav")):
        txt = w.with_suffix(".txt")
        (pos if txt.exists() and re.search(r"bob+(y|i|ie)\b|moby", txt.read_text(encoding="utf-8"), re.I) else neg).append(w)
    if pos or neg:
        yield "collected real", pos, neg


def lcg_noise(n):
    """Same generator as rust/bobby-audio/src/wake.rs, so Rust and Python start from identical 'random' noise."""
    s, out = 12345, []
    for _ in range(n):
        s = (s * 6364136223846793005 + 1442695040888963407) % 2**64
        out.append((s >> 33) % 2000 - 1000)
    return np.array(out, np.int64)


def write_csv(mp, out):
    """path,score for every clip of every set, with deterministic initial noise (CSV mode only)."""
    noise = lcg_noise(64000)
    np.random.randint = lambda lo, hi, size=None: noise.copy()
    model = Model(wakeword_models=[mp], inference_framework="onnx")
    with open(out, "w") as f:
        for _, pos, neg in sets():
            for p in pos + neg:
                if (a := load(p)) is not None:
                    f.write(f"{p},{score(model, a):.6f}\n")


def main(paths, thresholds=(0.05, 0.10, 0.20, 0.30)):
    for mp in paths:
        model = Model(wakeword_models=[mp], inference_framework="onnx")
        print(f"\n== {mp}")
        for name, pos, neg in sets():
            ps = [score(model, a) for a in map(load, pos) if a is not None]
            ns = [score(model, a) for a in map(load, neg) if a is not None]
            cells = "  ".join(f"@{t:.2f}: {sum(s >= t for s in ps)}/{len(ps)} found, {sum(s >= t for s in ns)}/{len(ns)} false" for t in thresholds)
            print(f"{name:26} median pos {np.median(ps) if ps else 0:.2f}  {cells}")


if __name__ == "__main__":
    if "--csv" in sys.argv:
        i = sys.argv.index("--csv")
        out = sys.argv.pop(i + 1)
        sys.argv.pop(i)
        write_csv((sys.argv[1:] or [str(HERE.parent / "models" / "wakeword" / "bobby.onnx")])[0], out)
        sys.exit()
    main(sys.argv[1:] or [str(HERE.parent / "models" / "wakeword" / "bobby.onnx")])
