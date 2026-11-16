"""'Bobby' followed right away by the request ("Bobby mets du rock"), the way the owner talks (2026-11-16).

train: cont_clips/CONT_*.wav  the sentence cut 0.08-0.4 s after "Bobby" (training voices upmc x2, tom)
test:  test_cont/pos/*.wav     whole sentences "Bobby, <request>" (held-out voices siwis, gilles: never trained on)
       test_cont/neg/*.wav     requests without Bobby and look-alike words (same held-out voices)
"""
import io
import random
import shutil
import wave
from pathlib import Path

import numpy as np
from piper import PiperVoice, SynthesisConfig
from scipy.signal import resample_poly

HERE = Path(__file__).parent
VOICES = Path(r"C:\Users\alexa\AppData\Local\Temp\claude\d1c9e4f09c6d20ce\5ea78eaf-20eb-4e04-8fef-424fc7e830fb\scratchpad\voices")
PIPER = HERE.parent / "models" / "piper"
TRAIN = [(VOICES / "fr_FR-upmc-medium.onnx", 0), (VOICES / "fr_FR-upmc-medium.onnx", 1), (VOICES / "fr_FR-tom-medium.onnx", None)]
TEST = [(PIPER / "fr_FR-siwis-medium.onnx", None), (VOICES / "fr_FR-gilles-low.onnx", None)]
REQUESTS = [
    "mets du rock", "coupe le micro de Lucas", "joue de la hardstyle", "quelle heure est-il", "passe à la suivante",
    "baisse le son", "monte le volume", "déplace-moi dans Général", "est-ce que tu peux mute Kévin", "ajoute ça à la liste",
    "c'est quoi ce son", "mets la musique en pause", "lance Valorant", "kick Sorciax", "dis-moi la météo",
    "rejoins-nous", "connecte-toi sur Discord", "arrête la musique", "rejoue ça", "tu peux me rappeler de manger",
    "joue que du Stromae", "mets une radio jazz", "c'est qui le meilleur", "explique-moi les trous noirs", "montre la liste",
    "ferme Steam", "verrouille le PC", "mute tout le monde", "pareil pour Zyklon", "non pas lui",
]
LOOKALIKES = ["Bob", "bobine", "bobo", "baby", "hobby", "lobby", "beau bilan", "Bobo le clown", "Robbie", "Toby", "Bibi", "Bambi"]
FILLERS = ["", "Euh ", "Hé ", "Dis ", "Ok "]


def synth(voice, text, spk, cfg_seed):
    rnd = random.Random(cfg_seed)
    cfg = SynthesisConfig(speaker_id=spk, length_scale=rnd.uniform(0.8, 1.25), noise_scale=rnd.uniform(0.4, 0.9), noise_w_scale=rnd.uniform(0.5, 1.1))
    buf = io.BytesIO()
    with wave.open(buf, "wb") as w:
        voice.synthesize_wav(text, w, syn_config=cfg)
    with wave.open(io.BytesIO(buf.getvalue())) as w:
        a, sr = np.frombuffer(w.readframes(w.getnframes()), np.int16).astype(np.float32), w.getframerate()
    g = np.gcd(16000, sr)
    return resample_poly(a, 16000 // g, sr // g)


def voiced_end(a):
    nz = np.nonzero(np.abs(a) > 300)[0]
    return int(nz[-1]) if len(nz) else len(a)


def save(path, a):
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1), w.setsampwidth(2), w.setframerate(16000)
        w.writeframes(np.clip(a, -32768, 32767).astype(np.int16).tobytes())


def main(n_train=2100, n_test=200):
    random.seed(1)
    out = HERE / "cont_clips"
    shutil.rmtree(out, ignore_errors=True)
    out.mkdir()
    k = 0
    for path, spk in TRAIN:
        v = PiperVoice.load(str(path))
        for _ in range(n_train // len(TRAIN)):
            seed, filler, req = random.random(), random.choice(FILLERS), random.choice(REQUESTS)
            # where "Bobby" ends: the same voice settings saying the head alone (Piper is deterministic per settings)
            end = voiced_end(synth(v, f"{filler}Bobby", spk, seed))
            a = synth(v, f"{filler}Bobby{random.choice([' ', ', '])}{req}", spk, seed)
            cut = min(len(a), end + int(16000 * random.uniform(0.08, 0.4)))
            save(out / f"CONT_{k:05d}.wav", a[:cut])
            k += 1
    print(f"{k} training clips in {out}")
    for sub in ("pos", "neg"):
        shutil.rmtree(HERE / "test_cont" / sub, ignore_errors=True)
        (HERE / "test_cont" / sub).mkdir(parents=True)
    j = 0
    for path, spk in TEST:
        v = PiperVoice.load(str(path))
        for i in range(n_test // len(TEST)):
            seed = random.random()
            save(HERE / "test_cont" / "pos" / f"P{j:04d}.wav", synth(v, f"{random.choice(FILLERS)}Bobby{random.choice([' ', ', '])}{random.choice(REQUESTS)}", spk, seed))
            neg = random.choice(REQUESTS) if i % 2 else f"{random.choice(LOOKALIKES)}, {random.choice(REQUESTS)}"
            save(HERE / "test_cont" / "neg" / f"N{j:04d}.wav", synth(v, neg, spk, seed))
            j += 1
    print(f"{j} test positives and {j} test negatives in {HERE / 'test_cont'}")


if __name__ == "__main__":
    main()
