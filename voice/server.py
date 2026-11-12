"""Bobby's voice service: speech-to-text (Whisper, GPU) and text-to-speech (Piper, CPU)."""
import io
import os
import site
import time
import wave
from pathlib import Path
from urllib.parse import unquote

# Windows: the CUDA DLLs (cuBLAS, cuDNN) come from the nvidia-* pip packages.
for sp in site.getsitepackages():
    for sub in ("nvidia/cublas/bin", "nvidia/cudnn/bin"):
        p = Path(sp, sub)
        if p.is_dir():
            os.add_dll_directory(str(p))
            os.environ["PATH"] = f"{p}{os.pathsep}{os.environ['PATH']}"

import numpy as np
import uvicorn
from fastapi import FastAPI, Request
from fastapi.responses import Response
from faster_whisper import WhisperModel
from piper import PiperVoice
from scipy.signal import resample_poly

MODELS = Path(__file__).resolve().parent.parent / "models"

print("Loading Whisper...", flush=True)
whisper = WhisperModel(
    os.getenv("STT_MODEL", "large-v3-turbo"),
    device="cuda",
    # int8: half the VRAM of float16, same speed measured on an RTX 4060 (int8_float16 was slower).
    compute_type=os.getenv("STT_COMPUTE_TYPE", "int8"),
    download_root=str(MODELS / "whisper_models"),
)
whisper.transcribe(np.zeros(16000, np.float32), language="fr")  # warm up the GPU
LANGS = ("fr", "en")  # Bobby understands and answers in these two languages
voices = {
    "fr": PiperVoice.load(str(MODELS / "piper" / "fr_FR-siwis-medium.onnx")),
    "en": PiperVoice.load(str(MODELS / "piper" / "en_US-lessac-medium.onnx")),
}
print("Voice service ready.", flush=True)

app = FastAPI()


def to_16k(raw: bytes) -> np.ndarray:
    """s16le PCM at 48 kHz stereo (decoded Discord audio) -> float32 at 16 kHz mono."""
    pcm = np.frombuffer(raw[: len(raw) // 4 * 4], np.int16).reshape(-1, 2).mean(axis=1) / 32768
    return resample_poly(pcm, 1, 3).astype(np.float32)


@app.post("/transcribe")
async def transcribe(request: Request):
    # ponytail: async endpoint + blocking Whisper = transcriptions run one at a time, no lock needed.
    audio = to_16k(await request.body())
    t = time.perf_counter()
    opts = dict(
        beam_size=1,  # greedy search: faster, near-identical quality on short sentences
        temperature=0,
        vad_filter=True,
        condition_on_previous_text=False,
        hotwords=unquote(request.headers.get("x-hotwords", "")) or None,
    )
    segments, info = whisper.transcribe(audio, **opts)  # language detected by Whisper
    lang = info.language
    if lang not in LANGS:  # rare: misdetection on a short sentence, redo it in the likelier of fr/en
        probs = dict(info.all_language_probs or [])
        lang = max(LANGS, key=lambda l: probs.get(l, 0.0))
        segments, _ = whisper.transcribe(audio, language=lang, **opts)
    text = " ".join(s.text for s in segments if s.avg_logprob > -1.0).strip()
    ms = round((time.perf_counter() - t) * 1000)
    print(f"[stt] {ms} ms  {len(audio) / 16000:.1f} s of audio  {lang}  {text!r}", flush=True)
    return {"text": text, "lang": lang, "ms": ms}


@app.get("/tts")
def tts(text: str, lang: str = "fr"):
    buf = io.BytesIO()
    with wave.open(buf, "wb") as w:
        voices.get(lang, voices["fr"]).synthesize_wav(text, w)
    return Response(buf.getvalue(), media_type="audio/wav")


if __name__ == "__main__":
    uvicorn.run(app, host="127.0.0.1", port=3000, log_level="warning")
