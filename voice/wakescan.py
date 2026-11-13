"""Wake word scoring fed while the sentence is still being spoken (used by server.py's /listen).

Same score as scanning the whole sentence at once: 8000 samples of silence before and after the audio,
1280-sample frames, highest score. Only the new audio is converted at each chunk: the cost stays linear
in the sentence length (re-converting the whole buffer was quadratic and blocked the event loop).
"""
import numpy as np
from scipy.signal import resample_poly

ALIGN = 12  # bytes: 3 stereo s16 frames at 48 kHz = 1 sample at 16 kHz
CTX = 38400  # bytes (0.2 s) of audio already converted, resampled again so the filter has its history
HOLD = 19200  # bytes (0.1 s) kept for the next chunk: the filter needs what comes after too
PAD = np.zeros(8000, np.int16)  # Discord sends no audio during silence; give the model some on both sides


def to_16k(raw: bytes) -> np.ndarray:
    """s16le PCM at 48 kHz stereo (decoded Discord audio) -> float32 at 16 kHz mono."""
    pcm = np.frombuffer(raw[: len(raw) // 4 * 4], np.int16).reshape(-1, 2).mean(axis=1) / 32768
    return resample_poly(pcm, 1, 3).astype(np.float32)


class WakeScan:
    def __init__(self, model):
        self.model, self.score, self.done = model, 0.0, 0  # done: bytes of raw audio already converted
        self.buf = PAD.copy()  # 16 kHz int16 samples not fed to the model yet
        model.reset()

    def feed(self, raw: bytes, final: bool = False) -> float:
        end = len(raw) if final else len(raw) - HOLD
        end -= end % ALIGN
        if end > self.done:
            start = max(0, self.done - CTX)
            start -= start % ALIGN
            new = to_16k(raw[start:end])[(self.done - start) // ALIGN :]
            self.buf = np.concatenate([self.buf, (np.clip(new, -1, 1) * 32767).astype(np.int16)])
            self.done = end
        if final:
            self.buf = np.concatenate([self.buf, PAD])
        n = len(self.buf) // 1280 * 1280
        for i in range(0, n, 1280):
            self.score = max(self.score, float(max(self.model.predict(self.buf[i : i + 1280]).values())))
        self.buf = self.buf[n:]
        return self.score


if __name__ == "__main__":  # self-check against a whole-sentence scan, on the training recordings
    import sys
    import wave
    from pathlib import Path

    from openwakeword.model import Model

    root = Path(__file__).resolve().parent.parent
    rec = root / "_training" / "recordings"
    model = Model(wakeword_models=[str(root / "models" / "wakeword" / "bobby.onnx")], inference_framework="onnx")

    def whole(raw: bytes) -> float:
        pcm = (np.clip(np.concatenate([np.zeros(8000, np.float32), to_16k(raw), np.zeros(8000, np.float32)]), -1, 1) * 32767).astype(np.int16)
        model.reset()
        return max(max(model.predict(pcm[i : i + 1280]).values()) for i in range(0, len(pcm) - 1279, 1280))

    worst = 0.0
    for f in [rec / "pos" / f"pos_{i:02d}.wav" for i in (2, 5, 8, 20)] + [rec / "neg" / f"neg_{i:02d}.wav" for i in (0, 3, 4)]:
        mono = np.frombuffer(wave.open(str(f)).readframes(10**9), np.int16)
        raw = np.repeat(np.repeat(mono, 3)[:, None], 2, axis=1).tobytes()  # 16 kHz mono -> 48 kHz stereo, like Discord
        scan = WakeScan(model)
        for i in range(0, len(raw), 3840):  # 20 ms Discord chunks
            scan.feed(raw[: i + 3840])
        live = scan.feed(raw, final=True)
        ref = whole(raw)
        worst = max(worst, abs(live - ref))
        print(f"{f.name}: live {live:.3f}  whole {ref:.3f}")
    print(f"max difference {worst:.3f}")
    sys.exit(0 if worst < 0.06 else 1)  # a negative near 0.4 moves by ~0.05; positives by ~0.01
