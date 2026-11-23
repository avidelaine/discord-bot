"""Replay driver for the Node/Python line, the counterpart of the Rust app's BOBBY_REPLAY.

Every 20 s, one WAV of the list (16 kHz mono) goes through the same steps as the PC microphone of voice/local.py:
the voice service's /transcribe (wake word + Whisper), then the bot's /local (brain + spoken answer). Prints one CSV
line per sentence: wall-clock ms of each call. Usage: python replay_node.py list.txt minutes > node-replay.csv
"""
import json, sys, time, urllib.request, wave
import numpy as np
from scipy.signal import resample_poly

VOICE, BOT = "http://127.0.0.1:3000", "http://127.0.0.1:3001"


def post(url, data, ctype):
    req = urllib.request.Request(url, data=data, headers={"content-type": ctype})
    t = time.perf_counter()
    with urllib.request.urlopen(req, timeout=120) as r:
        body = json.loads(r.read() or b"{}")
    return body, round((time.perf_counter() - t) * 1000)


def to_48k_stereo(path):
    with wave.open(path) as w:
        pcm = np.frombuffer(w.readframes(w.getnframes()), np.int16).astype(np.float32)
    up = np.clip(resample_poly(pcm, 3, 1), -32768, 32767).astype(np.int16)  # what /transcribe expects: 48 kHz stereo s16
    return np.repeat(up, 2).tobytes()


def main():
    paths = [l.strip() for l in open(sys.argv[1], encoding="utf-8") if l.strip()]
    end = time.time() + 60 * float(sys.argv[2])
    print("at,clip,wake_stt_ms_server,transcribe_ms_wall,local_ms_wall,text", flush=True)
    i = 0
    while time.time() < end:
        time.sleep(20)
        p = paths[i % len(paths)]; i += 1
        tr, t1 = post(f"{VOICE}/transcribe", to_48k_stereo(p), "application/octet-stream")
        t2 = ""
        if tr.get("text"):
            body = {"text": tr["text"], "lang": tr.get("lang", "fr"), "score": tr.get("score"), "ms": tr.get("ms"), "woke": True}
            _, t2 = post(f"{BOT}/local", json.dumps(body).encode(), "application/json")
        print(f'{time.strftime("%H:%M:%S")},{p.rsplit("/", 1)[-1]},{tr.get("ms")},{t1},{t2},"{tr.get("text", "")}"', flush=True)


if __name__ == "__main__":
    main()
