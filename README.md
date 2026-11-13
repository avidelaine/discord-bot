# Bobby, assistant vocal Discord

Say "Bobby, move Zyklon to Gaming" (or "Bobby, déplace Zyklon dans Gaming") in a voice channel: Bobby says "Ça marche." and does it, like a coworker, in the language you spoke. French and English both work. Everything runs locally on Windows and costs nothing.

```
Discord voice ──► bot/ (Node, discord.js, @discordjs/voice 0.19 with DAVE)
                    │  one PCM stream per speaker, streamed over a WebSocket while the person speaks
                    ▼
                  voice/ (Python) ── wake word model on the CPU, fed live: "Bobby" heard mid-sentence lights
                    │                Bobby's green ring (near-silent audio); no "Bobby", the sentence stops here
                    │  end of the sentence (600 ms of silence)
                    ▼
                  Whisper large-v3-turbo on the GPU, French/English auto-detected, server names as hotwords
                    │  text
                    ▼
                  bot/brain.js ── "Bobby" found? ── llama-server (Qwen3-4B-Instruct, JSON constrained: a short
                    │                                analysis first, then the actions), with the last 10 exchanges
                    │  names matched against the real server roster, guardrails
                    ▼
                  Discord action done silently; Bobby speaks only to answer, explain an "I can't" or report an error
```

## Install (once)

Requirements: Windows, an NVIDIA GPU, Node 22+, Python 3.11, ffmpeg on the PATH.

- `_bin/yt-dlp.exe`: the Windows binary from the yt-dlp GitHub releases (for YouTube music).

- `_bin/llama/`: llama.cpp Windows CUDA 12.4 build (`llama-bXXXX-bin-win-cuda-12.4-x64.zip` + `cudart-…zip`, from the llama.cpp GitHub releases).
- `models/Qwen3-4B-Instruct-2507-Q4_K_M.gguf`: from `unsloth/Qwen3-4B-Instruct-2507-GGUF` on Hugging Face.
- `models/piper/fr_FR-siwis-medium.onnx`, `models/piper/en_US-lessac-medium.onnx` (Bobby's voices), `models/piper/fr_FR-tom-medium.onnx`, `models/piper/en_US-ryan-medium.onnx` (Claude's voices) and their `.onnx.json`: from `rhasspy/piper-voices`.
- Python: `py -3.11 -m venv voice/.venv`, then `voice/.venv/Scripts/pip install faster-whisper fastapi "uvicorn[standard]" scipy piper-tts openwakeword==0.6.0 nvidia-cublas-cu12 "nvidia-cudnn-cu12==9.*"`, then `voice/.venv/Scripts/python -c "from openwakeword.utils import download_models; download_models(['hey_jarvis'])"` (fetches openWakeWord's shared feature models).
- `models/wakeword/bobby.onnx` and `bobby.onnx.data`: the custom "Bobby" wake word model (see below). Without it, every sentence goes through Whisper.
- PC control: `voice/.venv/Scripts/pip install -r voice/requirements-pc.txt` (Windows media sessions and volume).
- Node: `cd bot && npm install`.

Whisper downloads itself into `models/whisper_models` on first start.

## Configure

`.env` at the root:

| Variable | Role |
|---|---|
| `DISCORD_TOKEN`, `GUILD_ID` | Bot token and server ID |
| `SUPER_ADMINS` | Discord IDs with every right, including PC control and managing rights by voice. |
| `ADMINS` | Discord IDs of admins (moderation, Bobby's messages, Claude questions). Super admins are admins too. |
| `PRIME_ROLE` | Discord role (name or ID) whose members get music, the to-do lists and questions about Bobby (default `Team Prime`). |
| `RECORD_SILENCE_MS` | Silence that ends a sentence (default 600). Lower is faster but cuts you off if you pause. |
| `CLAUDE_MODEL` | Claude model for those questions (default `haiku`). |
| `TEXT_CHANNEL` | Text channel (name or ID) where Bobby posts lists and answers asked in writing (it never echoes what it says). Empty: the channel where `/join` was typed. |
| `WAKE_THRESHOLD` | Wake word score needed to wake Whisper (default 0.10). Higher means fewer wasted transcriptions but more missed "Bobby". |

The bot needs the **Connect**, **Speak**, **Mute Members**, **Deafen Members**, **Move Members**, **Send Messages** and **Read Message History** permissions (the last one to find its own messages to edit or delete). It deliberately does not have **Manage Messages**: it can only delete its own messages. No privileged intent is required.

## Run

- `start.cmd` opens the three parts (LLM, voice, bot) in three minimized windows. Each part restarts by itself 3 s after a crash.
- `node start.js`: the same in one console, one colored line per event, also written to `_logs/bobby.log`.
- `start-headless.cmd`: the same with no window at all (logs in `_logs/`), for gaming.
- `stop.cmd` stops whichever of these is running.

In Discord, join a voice channel and type `/join`. Bobby then follows the group: a move that includes the speaker ("déplace-nous dans PRIME", "mets-moi dans Général", "tout le monde dans Working") takes Bobby along, music included, and "viens dans PRIME" / "rejoins-nous" / "come here" bring it alone (Prime). If everyone leaves its channel, Bobby joins whoever typed `/join`; if that person is not in voice, the music pauses and Bobby leaves after 2 minutes unless someone comes back. Logs: `_logs/bot.log`, `_logs/voice.log`, `_logs/llm.log`, and the journal of every exchange `_logs/conversations.jsonl`.

Each command prints its timings: `[temps] stt … · llm … · action … · total …`.

## Rights

Each action needs a role (printed at startup as `[rights] role needed per action`):

| Role | Who | Can |
|---|---|---|
| super | `SUPER_ADMINS` | everything below, PC control, nicknames, rights by voice, every to-do item |
| admin | super admins + `ADMINS` | mute, deafen, move, disconnect, messages (and Bobby's own), Claude questions |
| prime | members with the `PRIME_ROLE` Discord role | music, the time, calling Bobby to a channel, questions about Bobby and who has which rights, the to-do lists (add to the general list and their own, manage their own items) |
| others | everyone else | nothing: Bobby says "Je n'ai pas le droit de te répondre" (at most once per 30 s per person) |

The super admin changes rights by voice, stored in `data/permissions.json`: "enlève les permissions de X" (X falls to "others", whatever their Discord role), "redonne les permissions à X", "passe X admin", "retire l'admin à X". "Annule" undoes it. "Quels sont les droits de X ?" answers from the live state. Nobody can make or remove a super admin by voice, the super admin included: that only changes by editing `SUPER_ADMINS` in `.env` by hand. Asking for rights ("donne-moi tous les droits") never grants any; asked to note it, Bobby adds it to the list, signed by whoever asked.

## What Bobby understands

- **Moderation:** mute and unmute ("dé-mute" too), deafen and undeafen, move, drag someone to your channel ("drague X"), disconnect, post a message in a text channel. "Moi", "ici", "tout le monde", "sauf X" and several requests in one sentence all work.
- **Bobby's own messages (admin):** "modifie ton dernier message en …", "supprime ton dernier message", "nettoie dev-bot-test" (deletes every message Bobby wrote there, after a "oui"). Never other people's messages.
- **Music (Prime):** "joue Bohemian Rhapsody" (YouTube), "joue du hardstyle" / "mets une radio jazz" (web radios from Radio Browser), pause, resume, skip, stop, replay, "ajoute X à la file", the queue, volume up/down/to N, "c'est quoi ce son". Music ducks while Bobby speaks, crossfades between tracks (2 s) and fades out on skip and stop (1 s). "Stop" alone stops the music if any and otherwise does nothing; Bobby only leaves on "pars", "quitte le salon", "tu peux partir".
- **PC (super admin):** media on the PC ("mets pause sur le PC", "musique suivante sur Spotify", through Windows media sessions, no simulated keys), Windows volume ("monte le son du PC"), launch games (Steam, Valorant through the Riot Client, Epic) and apps from the Start menu ("ouvre Claude"), close them ("ferme Apex Legends": the open window whose title or process matches), GPU info, lock / sleep / shut down after a spoken "oui". Only launching an app takes the focus.
- **Nicknames (super admin):** "appelle Zyklon le Grec", then "vire le Grec"; "renomme le surnom X en Y", "supprime le surnom X", "qui a quel surnom ?" (table posted in the text channel, only when "surnom" is said), and the `/surnoms` slash command. Stored in `data/aliases.json`.
- **To-do lists (Prime):** one shared general list and one personal list per person, in `data/todo.json`; each item keeps its author, date, list and done flag. "Retiens …", "note …", "pour la prochaine fois …", "il faudrait que tu …", "rajoute dans la liste …" add the full sentence, untruncated. "Montre ce que tu as retenu" / "la liste d'Ilias" / "ma liste" post the full list (author and date) in the text channel and say the count and the first 3 items; "le point 2 est fait", "supprime le dernier point", "mets le dernier point dans ma liste". Filters: "qu'est-ce qui a déjà été fait ?", "ce qui reste à faire", "les points ajoutés par Marie" (numbers stay the list's own). Bobby never says it noted something unless an item was really saved: otherwise it asks "Tu veux que je l'ajoute à la liste ?" and a "oui" adds it.
- **About Bobby (Prime):** questions about Bobby, its voice, wake word, who has which rights, the people on the server or the conversation are answered locally from facts rebuilt at each question (capabilities declared by the action modules, roles, `.env`, `data/permissions.json`, nicknames, lists), never sent to Claude. "Quelle heure est-il", "qu'est-ce que tu sais faire".
- **Interrupting Bobby:** while Bobby speaks, the person it is answering can say "stop", "arrête", "chut", "tais-toi", "c'est bon", "ok merci" (or "shut up", "that's enough", "ok thanks") without "Bobby": the voice stops at once, the music keeps playing ("Bobby, stop la musique" for that). An admin can stop it too, with "Bobby, stop"; anyone else's stop is ignored. Interruptions are written to the journal.
- **Answers in writing:** "dis-moi par écrit ce que tu sais faire", "écris-moi la liste", "envoie-moi les surnoms", "in writing": Bobby posts the full answer in `TEXT_CHANNEL` and only says "Je t'envoie ça dans dev-bot-test". It works for capabilities, to-do lists, rights ("qui a quels droits", "les droits de Lucas"), nicknames and Claude's answers (longer, with markdown, when asked in writing). "Écris-le dans général" posts the last answer in another channel (admin outside `TEXT_CHANNEL`). "Dans le chat", "montre-moi" and "affiche" ask for writing too, with the channel anywhere in the sentence ("explique dans le chat dev-bot-test ce que tu sais faire" posts the capabilities card).
- **Posts:** capabilities (one field per role), to-do lists (one field per item, with a done mark, author and date), rights and nicknames are Discord embeds with consistent colors; nicknames are an aligned monospace table. Everything is split to Discord's limits (2000 characters per message, 25 fields and 6000 characters per embed). Spoken replies are never echoed in text.
- **Conversation context:** the last 10 exchanges of the voice channel (everyone who talked to Bobby, within 30 minutes: what was said, what Bobby did, what it answered) go to the LLM, to the "about" answers and to Claude, so "ça", "le dernier", "pareil pour Marie", "non pas lui, Zyklon", "réessaie" or "refais" (the speaker's own last request) make sense. It is rebuilt from the journal at startup: a restart loses nothing.
- **Thinking before acting:** the LLM first writes a short analysis (order, capability question, correction or chat; feasible or not, and why; sure or not), then the actions. A capability question is answered, never run ("es-tu capable de couper le micro de Lucas ?" → "Oui, je peux… Tu veux que je le fasse ?", and a "oui" does it; read-only answers like the time or the capabilities are just given). Something impossible gets a real "I can't" with the reason instead of the nearest action ("supprime mes messages": Bobby only deletes its own; "volume à 200 %": "le maximum c'est 100 %, je le mets à fond"). A correction redoes the previous request ("j'aurais voulu des tableaux" posts it in writing). "Appelle-moi sur Discord": bots cannot call, Bobby offers a mention instead.
- **Coworker tone:** an order gets a short ack ("D'accord.", "Ça marche.", "Je m'en occupe.", "C'est parti.", "Entendu.", never twice in a row), synthesized once at startup and played as soon as the transcript shows an order; the action is then done silently. A question gets its answer with no ack. "Bobby ?" alone gets "Oui ?", and the next sentence is the order without "Bobby" (8 s). Otherwise "Bobby" is always required: Bobby never answers a sentence that was not addressed to it. Bobby speaks again only for an answer, an error or something useful (which track, which list item). A distinct low two-tone sound means "heard Bobby, but did not understand" or "you have no rights". Spoken replies end with 0.4 s of silence so Discord does not cut the last syllable.
- **Mentions (admin):** "arrobase-moi dans dev-bot-test pour me rappeler de lancer Apex", "ping Lucas sur annonces": users only, never @everyone / @here. A text channel's name never resolves to a voice channel for a move.
- **Claude as a second brain (admins):** when the local model says it is unsure (an ambiguous or complex request), Bobby says "Je réfléchis." and Claude Haiku reads the same prompt and picks among the same actions (5-10 s); the same guards and rights apply.
- **Undo and cancel:** "annule" undoes the last action (moderation, nicknames, to-do list, rights); a sentence that ends in "annule" or "laisse tomber" is dropped entirely.
- **Claude:** genuine world-knowledge questions ("c'est quoi la capitale de l'Australie ?": culture, news, science, maths, explanations) are forwarded to Claude Haiku through the Claude Code CLI, for admins, and the answer is spoken with a different voice (Piper tom / ryan). Everything else stays local: a question about Bobby, the server, the people present or the conversation, a note request, feedback or an opinion about Bobby never goes to Claude. Claude runs in an empty folder with every tool disabled except web search, no MCP servers and no user settings. Bobby checks `claude auth status` at startup; while Claude Code is not logged in, it answers "Claude n'est pas encore branché" at once.

Polite wrappers ("est-ce que tu peux …", "s'il te plaît", "can you … please") are ignored by the LLM-free shortcuts. Disconnecting someone, acting on three people or more, cleaning a channel, and lock / sleep / shutdown wait for a "oui": any sentence that starts with a yes and contains neither "non" nor "pas" ("oui je confirme Bobby je confirme"), with rights checked again at that moment. If a name is ambiguous, Bobby asks which one you meant. If the LLM is down, Bobby says "Mon cerveau ne répond pas".

## Code layout

`bot/brain.js` is the core (shortcuts, LLM with its analysis step, guards on the LLM's choice, names, rights, confirm, offers, undo, the shared conversation, the coworker-tone report, the "never a fake noted" rule). `voice/server.py` serves `/listen` (WebSocket: live wake word, gate, then Whisper), `/transcribe` (the same in one POST) and `/tts`. Each feature is a module in `bot/actions/` that brings its action types, prompt lines, examples, shortcuts, required role, `run` and spoken reply (contract at the top of `brain.js`): adding a feature means adding a file. `bot/format.js` builds the embeds and tables and splits them to Discord's limits. `bot/music/` is the player (PCM mixer with ducking, radios, yt-dlp). `voice/pc.py` serves Windows media and volume to the bot.

## The wake word model

"Bobby" is detected by a tiny [openWakeWord](https://github.com/dscripka/openWakeWord) model (about 400 KB, roughly 0.1 s of CPU per sentence). It was trained in WSL2 on the GPU from:

- 30,000 synthetic "Bobby" spoken by the 900 English voices of Piper LibriTTS,
- 2,100 French-accented "Bobby" generated with three French Piper voices,
- 20 real recordings of the owner's voice, each duplicated 100 times with different noise and room echo,
- the standard openWakeWord negative set (2,000 hours of speech, noise and music).

On held-out real recordings, at the default threshold of 0.10, it lets through 9 of 10 "Bobby" and stops 14 of 15 look-alike sentences (0.08 was tried after near-misses at 0.06-0.12 in real use: it let a second look-alike through, so it was not kept) ("hobby", "lobby", "bébé"...) before Whisper. The threshold is deliberately low: a false trigger only costs one transcription, because the bot still requires "Bobby" in the transcript before acting. A model trained on English voices only detected none of the real recordings, so real recordings matter.

The model is kept out of git because it was trained partly on a personal voice.

## Tests

- `node bot/wake.test.js`: detection of "Bobby" in the transcript, including the spellings Whisper produces ("Bobbie", "Dibobby"...).
- `node bot/unit.test.js`: shortcuts, names, rights, cancel, undo, nicknames, music and PC commands (dry run: nothing is launched, closed, locked or shut down), to-do lists and their rights, note requests, the "never a fake noted" rule, long confirmations, guards on the LLM's choice, the untouchable super admin, rights answers that follow "passe X admin" and blacklists, history, Bobby following the group, interruptions (only the person answered, admins with "Bobby"), answers in writing and Discord's limits, and every miss of the 2026-11-12/13 tests (conversation rebuilt from the journal, capability questions, "I can't" guards, mentions, text vs voice channels, written mode, list filters, acks and silent actions through `hear`), without the LLM.
- `node bot/music/music.test.js`: the mixer's ducking and fades, and a radio and a YouTube track streamed to a sink without Discord (needs the network).
- `node bot/intent.test.js` (llama-server running): comprehension score on French and English sentences, a routing set (sentences that must stay local, including every wrong Claude routing seen in the journal, and some that must go to Claude), sentences with history, a batch 4 set (capability questions, "I can't", corrections, "pareil pour…", mentions) checked on what Bobby would do (run, offer or answer), how often the model is unsure (each one would cost a Claude call), and LLM latency. Add every sentence Bobby gets wrong in real use.
