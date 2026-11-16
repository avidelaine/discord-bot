"""Local mode: while Bobby is not in a Discord voice channel, it listens to the PC microphone and answers on the
PC speakers. The bot turns it on and off (POST /local/on|off); sentences go to the bot like Discord ones."""
import io
import json
import threading
import time
import urllib.request
import wave
import winsound

import numpy as np

RATE = 16000
BLOCK = 1280  # 80 ms: the wake word model's frame
SILENCE_S = 0.6  # end of a sentence, like RECORD_SILENCE_MS on Discord
PREROLL = 4  # blocks kept before the voice starts (the "B" of "Bobby")
MAX_S = 15


class Local:
    """Cuts the microphone into sentences with an energy gate, then hands each one to `on_sentence(audio)`."""

    def __init__(self, on_sentence):
        self.on_sentence = on_sentence
        self.enabled = False
        self.speaking = False  # Bobby talks on the speakers: the mic is ignored (no echo of its own voice)
        self.force_until = 0.0  # Bobby waits for the next sentence ("Bobby" said alone): no wake word needed
        self._thread = None

    def set(self, on: bool):
        self.enabled = on
        if on and not (self._thread and self._thread.is_alive()):
            self._thread = threading.Thread(target=self._run, daemon=True)
            self._thread.start()

    def _run(self):
        import sounddevice as sd  # only imported when local mode is used

        noise, voiced, quiet, pre, buf = 0.005, False, 0, [], []
        try:
            with sd.InputStream(samplerate=RATE, channels=1, dtype="float32", blocksize=BLOCK) as mic:
                print(f"[local] listening on the PC microphone ({sd.query_devices(kind='input')['name']})", flush=True)
                while self.enabled:
                    block = mic.read(BLOCK)[0][:, 0].copy()
                    if self.speaking:
                        voiced, buf, pre = False, [], []
                        continue
                    rms = float(np.sqrt(np.mean(block**2)))
                    # ponytail: energy gate with a slow noise floor; a VAD model would cut better in a noisy room
                    if not voiced:
                        noise = 0.98 * noise + 0.02 * rms if rms < noise * 3 else noise
                    loud = rms > max(0.01, noise * 3)
                    if not voiced:
                        pre = (pre + [block])[-PREROLL:]
                        if loud:
                            voiced, quiet, buf = True, 0, list(pre)
                        continue
                    buf.append(block)
                    quiet = 0 if loud else quiet + 1
                    if quiet * BLOCK / RATE >= SILENCE_S or len(buf) * BLOCK / RATE >= MAX_S:
                        audio = np.concatenate(buf)
                        voiced, buf, pre = False, [], []
                        if len(audio) / RATE >= 0.4:
                            threading.Thread(target=self.on_sentence, args=(audio,), daemon=True).start()
        except Exception as e:  # noqa: BLE001  (no microphone, device unplugged)
            print(f"[local] microphone unavailable: {e}", flush=True)
        print("[local] stopped listening on the PC microphone", flush=True)

    def play(self, wav: bytes):
        """Plays a WAV on the PC speakers, blocking; the mic is ignored meanwhile."""
        self.speaking = True
        try:
            winsound.PlaySound(wav, winsound.SND_MEMORY)
        finally:
            time.sleep(0.2)  # room echo
            self.speaking = False

    def hush(self):
        winsound.PlaySound(None, winsound.SND_PURGE)


def tone(freqs, ms=110, rate=22050):
    """Short beep(s) as a WAV: [880] = heard, [330, 247] = not understood / no rights."""
    parts = []
    for f in freqs:
        t = np.arange(int(rate * ms / 1000)) / rate
        env = np.minimum(1, np.minimum(t, t[::-1]) * 200)  # 5 ms fade in/out: no click
        parts.append(np.sin(2 * np.pi * f * t) * env * 0.3)
    pcm = (np.concatenate(parts) * 32767).astype(np.int16)
    out = io.BytesIO()
    with wave.open(out, "wb") as w:
        w.setnchannels(1), w.setsampwidth(2), w.setframerate(rate), w.writeframes(pcm.tobytes())
    return out.getvalue()


def post(url: str, body: dict) -> dict:
    req = urllib.request.Request(url, json.dumps(body).encode(), {"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=120) as r:
        return json.loads(r.read() or b"{}")


if __name__ == "__main__":  # self-check without microphone or speakers
    w = tone([880])
    assert w[:4] == b"RIFF" and len(w) > 2000
    print("local: ok")
