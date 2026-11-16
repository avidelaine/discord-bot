"""Generate French-accented "Bobby" clips (word at the end of the clip) with Piper voices kept out of the test set."""
import io, random, wave
from pathlib import Path
import numpy as np
from piper import PiperVoice, SynthesisConfig
from scipy.signal import resample_poly

OUT = Path(__file__).parent / "fr_clips"
OUT.mkdir(exist_ok=True)
VOICES = Path(r"C:\Users\alexa\AppData\Local\Temp\claude\d1c9e4f09c6d20ce\5ea78eaf-20eb-4e04-8fef-424fc7e830fb\scratchpad\voices")
# Training voices only: siwis, gilles, Hortense and the English voices stay for testing.
TRAIN = [(VOICES / "fr_FR-upmc-medium.onnx", 0), (VOICES / "fr_FR-upmc-medium.onnx", 1), (VOICES / "fr_FR-tom-medium.onnx", None)]
PHRASES = ["Bobby.", "Bobby !", "Bobby ?", "Hé Bobby.", "Dis Bobby.", "Ok Bobby.", "Allez Bobby.", "Euh, Bobby.", "Salut Bobby."]
random.seed(0)
n = 0
for path, spk in TRAIN:
    v = PiperVoice.load(str(path))
    for k in range(700):
        cfg = SynthesisConfig(speaker_id=spk, length_scale=random.uniform(0.75, 1.35),
                              noise_scale=random.uniform(0.4, 0.9), noise_w_scale=random.uniform(0.5, 1.1))
        buf = io.BytesIO()
        with wave.open(buf, "wb") as w:
            v.synthesize_wav(random.choice(PHRASES), w, syn_config=cfg)
        with wave.open(io.BytesIO(buf.getvalue())) as w:
            a, sr = np.frombuffer(w.readframes(w.getnframes()), np.int16).astype(np.float32), w.getframerate()
        g = np.gcd(16000, sr)
        a = resample_poly(a, 16000 // g, sr // g)
        nz = np.nonzero(np.abs(a) > 300)[0]  # trim trailing silence so the clip ends right after "Bobby"
        a = a[: nz[-1] + 1600] if len(nz) else a
        with wave.open(str(OUT / f"FR_{n:05d}.wav"), "wb") as w:
            w.setnchannels(1); w.setsampwidth(2); w.setframerate(16000)
            w.writeframes(np.clip(a, -32768, 32767).astype(np.int16).tobytes())
        n += 1
print(f"{n} French clips in {OUT}")
