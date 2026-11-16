"""Fetch the real room impulse responses and background audio the trainer could not load (run in WSL venv)."""
import io, json, sys, urllib.request
from pathlib import Path
import numpy as np
import pyarrow.parquet as pq
import soundfile as sf
from scipy.signal import resample_poly

DATA = Path.home() / "oww" / "data"
HF = "https://huggingface.co/datasets"

# 1. MIT room impulse responses (271 small wav files, already 16 kHz)
rir = DATA / "mit_rirs"
rir.mkdir(parents=True, exist_ok=True)
files = json.load(urllib.request.urlopen(f"https://huggingface.co/api/datasets/davidscripka/MIT_environmental_impulse_responses/tree/main/16khz"))
for f in files:
    if f["path"].endswith(".wav") and not (rir / Path(f["path"]).name).exists():
        urllib.request.urlretrieve(f"{HF}/davidscripka/MIT_environmental_impulse_responses/resolve/main/{f['path']}", rir / Path(f["path"]).name)
print(f"RIRs: {len(list(rir.glob('*.wav')))} files", flush=True)

# 2. Real background audio: one AudioSet shard (speech, music, noise, game-like sounds), 600 clips at 16 kHz mono
bg = DATA / "audioset_16k"
for old in bg.glob("*.wav"):
    old.unlink()  # drop the synthetic noise fallback
shard = DATA / "audioset_bal_train_00.parquet"
if not shard.exists():
    urllib.request.urlretrieve(f"{HF}/agkphysics/AudioSet/resolve/main/data/bal_train/00.parquet", shard)
table = pq.read_table(shard)
col = [c for c in table.column_names if "audio" in c.lower()][0]
n = 0
for row in table.column(col).to_pylist():
    if n >= 600:
        break
    try:
        a, sr = sf.read(io.BytesIO(row["bytes"]), dtype="float32", always_2d=True)
        a = a.mean(axis=1)
        if sr != 16000:
            g = np.gcd(16000, sr)
            a = resample_poly(a, 16000 // g, sr // g).astype(np.float32)
        if len(a) < 16000:
            continue
        sf.write(bg / f"audioset_{n:04d}.wav", a, 16000)
        n += 1
    except Exception as e:
        if n == 0:
            print("decode error:", e, file=sys.stderr)
print(f"AudioSet: {n} clips", flush=True)
shard.unlink()
