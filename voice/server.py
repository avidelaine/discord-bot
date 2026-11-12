"""Service voix de Bobby : transcription (Whisper, GPU) et synthèse vocale (Piper, CPU)."""
import io
import os
import site
import time
import wave
from pathlib import Path
from urllib.parse import unquote

# Windows : les DLL CUDA (cuBLAS, cuDNN) viennent des paquets pip nvidia-*.
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

print("Chargement de Whisper…", flush=True)
whisper = WhisperModel(
    os.getenv("STT_MODEL", "large-v3-turbo"),
    device="cuda",
    # int8 : moitié de la VRAM de float16, même vitesse mesurée sur RTX 4060 (int8_float16 était plus lent).
    compute_type=os.getenv("STT_COMPUTE_TYPE", "int8"),
    download_root=str(MODELS / "whisper_models"),
)
whisper.transcribe(np.zeros(16000, np.float32), language="fr")  # préchauffe le GPU
voice = PiperVoice.load(str(MODELS / "piper" / "fr_FR-siwis-medium.onnx"))
print("Service voix prêt.", flush=True)

app = FastAPI()


def to_16k(raw: bytes) -> np.ndarray:
    """PCM s16le 48 kHz stéréo (format Discord décodé) -> float32 16 kHz mono."""
    pcm = np.frombuffer(raw[: len(raw) // 4 * 4], np.int16).reshape(-1, 2).mean(axis=1) / 32768
    return resample_poly(pcm, 1, 3).astype(np.float32)


@app.post("/transcribe")
async def transcribe(request: Request):
    # ponytail: endpoint async + Whisper bloquant = les transcriptions passent une par une, sans verrou.
    audio = to_16k(await request.body())
    t = time.perf_counter()
    segments, _ = whisper.transcribe(
        audio,
        language="fr",
        beam_size=1,  # recherche simple : plus rapide, quasi même qualité sur des phrases courtes
        temperature=0,
        vad_filter=True,
        condition_on_previous_text=False,
        hotwords=unquote(request.headers.get("x-hotwords", "")) or None,
    )
    text = " ".join(s.text for s in segments if s.avg_logprob > -1.0).strip()
    ms = round((time.perf_counter() - t) * 1000)
    print(f"[stt] {ms} ms  {len(audio) / 16000:.1f} s d'audio  {text!r}", flush=True)
    return {"text": text, "ms": ms}


@app.get("/tts")
def tts(text: str):
    buf = io.BytesIO()
    with wave.open(buf, "wb") as w:
        voice.synthesize_wav(text, w)
    return Response(buf.getvalue(), media_type="audio/wav")


if __name__ == "__main__":
    uvicorn.run(app, host="127.0.0.1", port=3000, log_level="warning")
