"""Guided recording of "Bobby" (and look-alike sentences) to train and test the wake word model.

Run:  voice\.venv\Scripts\python _training\record_bobby.py
Each prompt: press Enter, then speak right away (3 seconds). Type r + Enter to redo the last one, q to quit.
"""
import re
import subprocess
from pathlib import Path

OUT = Path(__file__).parent / "recordings"
SECONDS = 3

POSITIVE = [
    "Bobby", "Bobby (plus fort)", "Bobby (doucement)", "Bobby (vite)", "Bobby (en t'éloignant du micro)",
    "Bobby ?", "Hé Bobby", "Ok Bobby", "Dis Bobby", "Allez Bobby",
    "Bobby, coupe le micro de Kévin", "Bobby, déplace-moi dans Gaming", "Bobby, annule",
    "Bobby, quelle heure est-il ?", "Bobby, ajoute du pain à ma liste", "Bobby, vire Lucas du vocal",
    "Euh... Bobby, tu peux couper Zyklon ?", "Bobby, rends le son à Marie", "Bobby, tu peux partir",
    "Bobby (comme si tu étais en pleine partie)",
    "Hey Bobby", "Okay Bobby", "Bobby, mute Kevin", "Bobby, move me to Gaming", "Bobby, undo that",
    "Bobby (with an English accent)", "Bobby, deafen Marie", "Bobby, you can go now",
    "Bobby (encore une fois, naturellement)", "Bobby (une dernière fois, comme tu veux)",
]
NEGATIVE = [
    "On lance une partie ?", "J'ai un nouveau hobby", "Le lobby met trois plombes à charger",
    "Mon bébé dort, parlez moins fort", "Bob a encore raté son tir", "C'est probablement le meilleur jeu",
    "Il s'est fait un bobo au genou", "Robbie arrive dans cinq minutes", "Coupe le micro, il y a du bruit",
    "Bonne partie les gars", "Bouge pas, j'arrive", "Let's queue up for another game",
    "My new hobby is painting", "Baby, can you hear me?", "That was probably our best round",
]


def microphones():
    out = subprocess.run(["ffmpeg", "-hide_banner", "-list_devices", "true", "-f", "dshow", "-i", "dummy"],
                         capture_output=True, text=True, encoding="utf-8", errors="replace").stderr
    return re.findall(r'"([^"]+)" \(audio\)', out)


def record(mic, path):
    subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-f", "dshow", "-i", f"audio={mic}",
                    "-t", str(SECONDS), "-ac", "1", "-ar", "16000", str(path)], check=True)


def main():
    mics = microphones()
    for i, m in enumerate(mics, 1):
        print(f"  {i}. {m}")
    mic = mics[int(input("Which microphone do you use on Discord? Number: ")) - 1]
    jobs = [("pos", i, p) for i, p in enumerate(POSITIVE)] + [("neg", i, p) for i, p in enumerate(NEGATIVE)]
    k = 0
    while k < len(jobs):
        kind, i, prompt = jobs[k]
        (OUT / kind).mkdir(parents=True, exist_ok=True)
        label = "WITH Bobby" if kind == "pos" else "WITHOUT Bobby"
        a = input(f"\n[{k + 1}/{len(jobs)}] {label} - say:  {prompt}\n  Enter to record (r = redo previous, q = quit) ").strip().lower()
        if a == "q":
            break
        if a == "r" and k > 0:
            k -= 1
            continue
        record(mic, OUT / kind / f"{kind}_{i:02d}.wav")
        print("  saved")
        k += 1
    print(f"\nDone. Recordings are in {OUT}")


if __name__ == "__main__":
    main()
