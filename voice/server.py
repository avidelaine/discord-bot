"""Bobby's voice service: speech-to-text (Whisper, GPU) and text-to-speech (Piper, CPU)."""
import asyncio
import io
import os
import site
import sys
import threading
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
from fastapi import FastAPI, Request, WebSocket, WebSocketDisconnect
from fastapi.responses import Response
from faster_whisper import WhisperModel
from piper import PiperVoice
from wakescan import WakeScan, to_16k  # live wake word scoring (testable without Whisper)
from local import Local, post, tone  # local mode: PC microphone and speakers

try:  # PC control is optional: without its packages (requirements-pc.txt), STT and TTS still work
    import pc
except Exception as e:  # noqa: BLE001
    pc = None
    print(f"PC control disabled: {e}", flush=True)

MODELS = Path(__file__).resolve().parent.parent / "models"

# Mirror everything printed (including server errors) to _logs/voice.log, so it can be reviewed later.
(MODELS.parent / "_logs").mkdir(exist_ok=True)
_logfile = open(MODELS.parent / "_logs" / "voice.log", "a", encoding="utf-8", buffering=1)


class _Tee:
    def __init__(self, stream):
        self.stream = stream

    def write(self, s):
        self.stream.write(s)
        _logfile.write(s)

    def flush(self):
        self.stream.flush()

    def __getattr__(self, name):
        return getattr(self.stream, name)


sys.stdout, sys.stderr = _Tee(sys.stdout), _Tee(sys.stderr)

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
# A second pair of voices for Claude's answers, so you hear who is talking.
claude_voices = {
    "fr": PiperVoice.load(str(MODELS / "piper" / "fr_FR-tom-medium.onnx")),
    "en": PiperVoice.load(str(MODELS / "piper" / "en_US-ryan-medium.onnx")),
}

# Wake word gate: a tiny CPU model listens for "Bobby" so Whisper (GPU) only runs on sentences meant for Bobby.
WAKE_MODEL = MODELS / "wakeword" / "bobby.onnx"
# 0.10: 3 real "Bobby" scored 0.15 on 2026-11-12 (missed at 0.2) and 5 near-misses scored 0.06-0.12. On
# _training/recordings (held-out pos_02,05..29 / neg_00..14) 0.10 keeps 9/10 and 1/15; 0.08 lets 2/15 through
# (neg_03 at 0.081). A false trigger only costs one transcription: the transcript must still contain "Bobby".
WAKE_THRESHOLD = float(os.getenv("WAKE_THRESHOLD", "0.10"))
wake = None
if WAKE_MODEL.exists():
    from openwakeword.model import Model

    def new_wake():
        return Model(wakeword_models=[str(WAKE_MODEL)], inference_framework="onnx")

    wake = new_wake()
    # idle models for /listen: a Model keeps state, so each live stream needs its own (the global one stays for /transcribe).
    # ponytail: 3 preloaded (creating one blocks for seconds); a 4th simultaneous speaker pays that once.
    wake_pool = [new_wake() for _ in range(3)]
    print("Wake word model loaded: Whisper only runs after 'Bobby'.", flush=True)
else:
    print("No wake word model: every sentence goes through Whisper.", flush=True)


def bobby_score(audio: np.ndarray, model=None) -> float:
    """Highest wake word score over the sentence, scanned in 80 ms frames."""
    model = model or wake
    pad = np.zeros(8000, np.float32)  # Discord sends no audio during silence; give the model some on both sides
    pcm = (np.clip(np.concatenate([pad, audio, pad]), -1, 1) * 32767).astype(np.int16)
    model.reset()
    # float(): the model returns numpy floats, which FastAPI cannot turn into JSON
    return float(max((max(model.predict(pcm[i : i + 1280]).values()) for i in range(0, len(pcm) - 1279, 1280)), default=0.0))


# Training data for the next wake word model (2026-11-16): real sentences, real mic, real "Bobby, <request>" said
# in one breath. Kept: every sentence that passed the gate (with its transcript) and the near-misses (score >= 0.01:
# the hard negatives, and the "Bobby" the model missed). _training/ is gitignored.
COLLECT = MODELS.parent / "_training" / "collected"
COLLECT_MIN = float(os.getenv("COLLECT_MIN_SCORE", "0.01"))  # set to 2 to turn collection off
COLLECT_MAX_PER_DAY = 600  # ponytail: ~60 MB a day at most; raise it if a day fills up


def collect(audio: np.ndarray, score, text: str | None = None, where: str = "discord"):
    if score is not None and score < COLLECT_MIN:
        return
    try:
        day = COLLECT / time.strftime("%Y-%m-%d")
        day.mkdir(parents=True, exist_ok=True)
        if sum(1 for _ in day.glob("*.wav")) >= COLLECT_MAX_PER_DAY:
            return
        name = f"{time.strftime('%H%M%S')}_{int(time.time() * 1000) % 1000:03d}_{where}_{'forced' if score is None else f'{score:.3f}'}"
        with wave.open(str(day / f"{name}.wav"), "wb") as w:
            w.setnchannels(1), w.setsampwidth(2), w.setframerate(16000)
            w.writeframes((np.clip(audio, -1, 1) * 32767).astype(np.int16).tobytes())
        if text is not None:
            (day / f"{name}.txt").write_text(text, encoding="utf-8")
    except Exception as e:  # noqa: BLE001  (never break a sentence for the dataset)
        print(f"[collect] {e}", flush=True)


print("Voice service ready.", flush=True)

app = FastAPI()
if pc:
    app.include_router(pc.router)


gpu = threading.Lock()  # one Whisper run at a time: Discord streams and the local microphone share the GPU


def run_whisper(audio: np.ndarray, hotwords: str | None):
    with gpu:
        return _whisper(audio, hotwords)


def _whisper(audio: np.ndarray, hotwords: str | None):
    opts = dict(
        beam_size=1,  # greedy search: faster, near-identical quality on short sentences
        temperature=0,
        vad_filter=True,
        condition_on_previous_text=False,
        hotwords=hotwords or None,
    )
    segments, info = whisper.transcribe(audio, **opts)  # language detected by Whisper
    lang = info.language
    if lang not in LANGS:  # rare: misdetection on a short sentence, redo it in the likelier of fr/en
        probs = dict(info.all_language_probs or [])
        lang = max(LANGS, key=lambda l: probs.get(l, 0.0))
        segments, _ = whisper.transcribe(audio, language=lang, **opts)
    # -1.5: lenient, the wake word filter already keeps most chatter away from Whisper ("Dis Bobby" scored -0.95).
    text = " ".join(s.text for s in segments if s.avg_logprob > -1.5).strip()
    return text, lang


@app.post("/transcribe")
async def transcribe(request: Request):
    # ponytail: blocking Whisper in an async endpoint = one transcription at a time, no lock needed here.
    audio = to_16k(await request.body())
    t = time.perf_counter()
    heard, score = None, None
    # x-force: Bobby is waiting for an answer from this person (a "yes", or the command after "Bobby" alone).
    if wake is not None and request.headers.get("x-force") != "1":
        score = bobby_score(audio)
        ms = round((time.perf_counter() - t) * 1000)
        if score < WAKE_THRESHOLD:
            print(f"[wake] no 'Bobby' (score {score:.2f}, {ms} ms): Whisper skipped", flush=True)
            return {"text": "", "wake": False, "score": round(score, 3), "seconds": round(len(audio) / 16000, 1), "ms": ms}
        print(f"[wake] 'Bobby' heard (score {score:.2f}, {ms} ms)", flush=True)
        heard = True
    text, lang = run_whisper(audio, unquote(request.headers.get("x-hotwords", "")))
    ms = round((time.perf_counter() - t) * 1000)
    print(f"[stt] {ms} ms  {len(audio) / 16000:.1f} s of audio  {lang}  {text!r}", flush=True)
    return {"text": text, "lang": lang, "wake": heard, "score": None if score is None else round(score, 3), "ms": ms}


whisper_lock = asyncio.Lock()  # one GPU transcription at a time; the event loop keeps scoring the other streams


@app.websocket("/listen")
async def listen(ws: WebSocket, force: str = "0", hotwords: str = ""):
    """One utterance: binary PCM chunks (s16le 48 kHz stereo) then the text 'end'. Replies: wake, gate, text."""
    await ws.accept()
    raw = bytearray()
    gated = wake is not None and force != "1"
    model = (wake_pool.pop() if wake_pool else new_wake()) if gated else None
    scan = WakeScan(model) if gated else None
    woke = False
    try:
        while True:
            msg = await ws.receive()
            if msg["type"] == "websocket.disconnect":
                return
            if msg.get("bytes"):
                raw += msg["bytes"]
                if scan and not woke and scan.feed(raw) >= WAKE_THRESHOLD:
                    woke = True  # the green ring can light up while the person is still talking
                    await ws.send_json({"type": "wake", "score": round(scan.score, 3)})
            elif msg.get("text") == "end":
                break
        t = time.perf_counter()
        audio = to_16k(bytes(raw)) if len(raw) >= 4 else np.zeros(0, np.float32)
        seconds = round(len(audio) / 16000, 1)
        score = None
        if scan:
            score = round(scan.feed(raw, final=True), 3)
        ms = round((time.perf_counter() - t) * 1000)
        passed = None if score is None else score >= WAKE_THRESHOLD
        await ws.send_json({"type": "gate", "wake": passed, "score": score, "seconds": seconds, "ms": ms})
        if passed is not None:
            print(f"[wake] {'Bobby heard' if passed else 'no Bobby'} (score {score:.2f}, gate {ms} ms after the end)", flush=True)
        if passed is False:
            collect(audio, score)  # a near-miss (the function keeps only score >= COLLECT_MIN)
            return
        t = time.perf_counter()
        async with whisper_lock:
            text, lang = await asyncio.to_thread(run_whisper, audio, hotwords)
        ms = round((time.perf_counter() - t) * 1000)
        print(f"[stt] {ms} ms  {seconds:.1f} s of audio  {lang}  {text!r}", flush=True)
        await ws.send_json({"type": "text", "text": text, "lang": lang, "ms": ms})
        collect(audio, score, text)
    except (WebSocketDisconnect, RuntimeError):  # client left mid-sentence (RuntimeError: send after close)
        pass
    finally:
        if model:
            wake_pool.append(model)
        try:
            await ws.close()
        except Exception:  # noqa: BLE001
            pass


# ---------- Local mode (PC microphone and speakers), driven by the bot ----------
BOT_URL = os.getenv("BOT_URL", "http://127.0.0.1:3001")
local_wake = new_wake() if wake is not None else None
local_wake_lock = threading.Lock()


def local_sentence(audio: np.ndarray):
    t = time.perf_counter()
    score = None
    if local_wake is not None and time.time() > local.force_until:
        with local_wake_lock:
            score = bobby_score(audio, local_wake)
        if score < WAKE_THRESHOLD:
            collect(audio, score, where="pc")
            return  # not for Bobby: never transcribed, like on Discord
    text, lang = run_whisper(audio, None)
    collect(audio, score, text, where="pc")
    ms = round((time.perf_counter() - t) * 1000)
    print(f"[local] {ms} ms  {len(audio) / 16000:.1f} s  {lang}  {text!r}", flush=True)
    if not text:
        return
    try:
        r = post(f"{BOT_URL}/local", {"text": text, "lang": lang, "score": None if score is None else round(score, 3), "ms": ms})
        local.force_until = time.time() + 8 if r.get("expect") else 0
    except Exception as e:  # noqa: BLE001
        print(f"[local] bot not reachable at {BOT_URL}: {e}", flush=True)


local = Local(local_sentence)
BEEPS = {"yes": tone([880]), "nope": tone([330, 247], 140)}


@app.post("/local/{state}")
def local_switch(state: str):
    on = state == "on"
    if on != local.enabled:
        print(f"[local] {'on: Bobby listens on the PC' if on else 'off: Bobby is on Discord'}", flush=True)
    local.set(on)
    return {"local": local.enabled}


@app.post("/speak")
async def speak(text: str, lang: str = "fr", voice: str = "bobby"):
    await asyncio.to_thread(local.play, tts(text, lang, voice).body)  # returns once spoken (the bot awaits it)
    return {"ok": True}


@app.post("/beep")
async def beep(kind: str = "yes"):
    await asyncio.to_thread(local.play, BEEPS.get(kind, BEEPS["yes"]))
    return {"ok": True}


@app.post("/hush")
def hush():
    local.hush()
    return {"ok": True}


@app.get("/tts")
def tts(text: str, lang: str = "fr", voice: str = "bobby"):
    pair = claude_voices if voice == "claude" else voices
    buf = io.BytesIO()
    with wave.open(buf, "wb") as w:
        pair.get(lang, pair["fr"]).synthesize_wav(text, w)
    return Response(buf.getvalue(), media_type="audio/wav")


if __name__ == "__main__":
    uvicorn.run(app, host="127.0.0.1", port=int(os.getenv("VOICE_PORT", "3000")), log_level="warning")
