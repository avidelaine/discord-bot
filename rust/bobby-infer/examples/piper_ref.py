"""Reference for the Rust port of Piper: phoneme ids and audio length from the Python package.
Run: voice/.venv/Scripts/python.exe -I rust/bobby-infer/examples/piper_ref.py
Writes _training/piper_ref.json (gitignored): [{text, lang, voice, ids, samples, rate, ms}]."""
import io
import json
import time
import wave
from pathlib import Path

from piper import PiperVoice

ROOT = Path("C:/Users/alexa/Desktop/Personal Projects/discord-bot")
PIPER = ROOT / "models" / "piper"
VOICES = {
    ("bobby", "fr"): "fr_FR-siwis-medium",
    ("bobby", "en"): "en_US-lessac-medium",
    ("claude", "fr"): "fr_FR-tom-medium",
    ("claude", "en"): "en_US-ryan-medium",
}

FR = [
    "Salut Bobby, ça va ?",
    "Bobby, lance Valorant s'il te plaît.",
    "Combien font 12 x 7 ?",
    "Mets le volume à 10 %. Ensuite, lance le stream.",
    "Rappelle-moi à 18h30 d'appeler Clément.",
    "Il est 18h30, c'est l'heure de manger !",
    "Je n'ai pas compris, peux-tu répéter ?",
    "C'est fait, j'ai ouvert le browser et le dashboard.",
    "Quelle est la météo à Paris demain ?",
    "Oui, d'accord. Je m'en occupe tout de suite.",
    "Clément a rejoint le salon vocal.",
    "Ouvre le site github.com/Alexnadre/bobby.",
    "Lance le jeu, puis coupe le micro, et mets la musique.",
    "Attention : la batterie est faible !",
    "Tu veux que je joue du rock, du jazz ou du rap ?",
    "Il y a 3 messages non lus, dont 2 de Clément.",
    "C'est l'anniversaire de Léa le 14 juillet.",
    "On a gagné la partie ! Quelle équipe, bravo à tous.",
    "Le rendez-vous est fixé à 9h00 au café de la gare.",
    "Je ne peux pas faire ça, désolé.",
    "Ce soir on joue à Valorant avec les copains. Tu viens ? Dis-moi vite !",
    "Fais un screenshot de l'écran, s'il te plaît.",
    "Éteins l'ordinateur dans 45 minutes.",
    "Ça coûte 25 euros, soit environ 30 dollars.",
    "Bonne nuit Bobby, à demain !",
]
EN = [
    "Hey Bobby, how are you?",
    "Bobby, launch Valorant please.",
    "What is 12 x 7?",
    "Set the volume to 10 %.",
    "Remind me at 18h30 to call Clément.",
    "It's six thirty, time for dinner!",
    "I didn't catch that, could you repeat?",
    "Done, I opened Spotify.",
    "What's the weather in London tomorrow?",
    "Yes, sure. I'll take care of it right away.",
    "Clément joined the voice channel.",
    "Open the website github.com/Alexnadre/bobby.",
    "Start the game, then mute the mic, and play some music.",
    "Warning: the battery is low!",
    "Do you want rock, jazz or rap?",
    "You have 3 unread messages, 2 of them from Clément.",
    "It's Lea's birthday on July 14th.",
    "We won the match! What a team, well done everyone.",
    "The meeting is at 9:00 at the station cafe.",
    "I can't do that, sorry.",
    "Tonight we play Valorant with the guys.",
    "Take a screenshot of the screen, please.",
    "Shut down the computer in 45 minutes.",
    "It costs 25 euros, about 30 dollars.",
    "Good night Bobby, see you tomorrow!",
]


def main():
    loaded = {k: PiperVoice.load(str(PIPER / f"{v}.onnx")) for k, v in VOICES.items()}
    out = []
    for lang, texts in (("fr", FR), ("en", EN)):
        for i, text in enumerate(texts):
            voice = "claude" if i % 5 == 4 else "bobby"
            v = loaded[(voice, lang)]
            ids = [v.phonemes_to_ids(p) for p in v.phonemize(text) if p]
            t0 = time.time()
            buf = io.BytesIO()
            with wave.open(buf, "wb") as w:
                v.synthesize_wav(text, w)
            ms = int((time.time() - t0) * 1000)
            buf.seek(0)
            with wave.open(buf) as w:
                n, rate = w.getnframes(), w.getframerate()
            out.append({"text": text, "lang": lang, "voice": voice, "ids": ids, "samples": n, "rate": rate, "ms": ms})
    dest = ROOT / "_training" / "piper_ref.json"
    dest.parent.mkdir(exist_ok=True)
    dest.write_text(json.dumps(out, ensure_ascii=False), encoding="utf-8")
    print(f"{len(out)} sentences -> {dest}")


main()
