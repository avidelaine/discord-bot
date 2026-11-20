# Bobby : analyse audio et latence (2026-11-19)

Analyse en lecture seule, faite à partir des logs réels (12 au 19 novembre), des métriques et du code
(`bot/`, `voice/`, `rust/`, branche R5 `claude/r5-stt-r4-music-001a76`). Aucun benchmark GPU, rien n'a été lancé.
Les gains annoncés sont **estimés** quand ils ne viennent pas des logs ; c'est indiqué à chaque fois.

## 0. Données utilisées

| Source | Ce qu'on en tire |
|---|---|
| `conversations.jsonl` (4 023 lignes réelles, 3 de test exclues) | 3 713 phrases rejetées par le wake word, 310 phrases traitées, dont **272 avec timing** |
| idem, filtré Discord (total ≥ 300 ms) | **248 interactions Discord** ; les 24 du mode PC ont un `total` faux (≈ 0, voir §2.3) |
| `voice.log` | 347 transcriptions Whisper (ms + durée audio), 3 314 temps de gate wake word |
| `llm.log`, `llm-cpu.log` | débit réel de Qwen3-4B (tokens/s, prompt et génération) |
| `metrics.db` (16 au 19 nov.) | VRAM, charge GPU au moment des requêtes |
| `bot.log` | erreurs, `[heard]`, `[intent]`, timings |

## 1. Où passe le temps

### 1.1 Découpage par étape (interactions Discord, ms)

| Étape | n | Médiane | p90 | Commentaire |
|---|---|---|---|---|
| Wake word « Bobby » (bip) | 3 314 | **13** après la fin de phrase | 20 | Le bip part *pendant* la phrase (streaming) : coût nul |
| Fin de parole (silence `RECORD_SILENCE_MS`) | toutes | **600** (fixe) | 600 | Attente pure, mesurée : `total − stt − llm − action` = 612 ms médiane |
| Whisper large-v3-turbo int8 | 248 | **742** | 1 047 | Plancher ~620 ms même pour < 1,5 s d'audio |
| Raccourci regex | 32 | 0 | 0 | |
| LLM Qwen3-4B Q4 | 152 | **841** | 2 203 | 28–36 tokens/s en génération, ~31–39 tokens de sortie |
| Claude (repli) | 1 | 8 341 | — | rare |
| Action (hors musique) | ~118 | ~0–15 | 1 350 (move) | |
| Action `music_play` (yt-dlp) | 24 | **3 216** | 5 254 | Domine tout quand on lance de la musique |
| TTS Piper + lecture | — | non mesuré | — | `say()` n'est pas attendu : le `total` s'arrête avant la synthèse |

### 1.2 De bout en bout (fin de parole → réponse lancée)

| Cas | n | Médiane | p90 |
|---|---|---|---|
| Toutes interactions Discord | 248 | **2 042** | 5 300 |
| Via raccourci | 22 | **1 382** | 5 097 (musique) |
| Via LLM | 151 | **2 405** | 5 675 |
| Via LLM, hors musique | 132 | 2 304 | 4 163 |
| Lancer une musique | 24 | **5 896** | 9 229 |
| « Bobby ? » seul (attente de l'ordre) | 39 | 1 307 | 1 789 |

**Pour un ordre typique passé par le LLM (2,4 s) :** silence 0,6 s (25 %) + Whisper 0,74 s (30 %) + LLM 0,84 s
(35 %) + reste. Pour la musique, yt-dlp ajoute 3,2 s et devient le poste n°1. Le wake word, le réseau
local et le WebSocket ne pèsent rien.

### 1.3 Observations chiffrées

- **Le plancher Whisper (~620 ms)** ne dépend presque pas de la durée : 619 ms (< 1,5 s d'audio), 651 ms (1,5–3 s),
  784 ms (3–6 s), 869 ms (> 6 s). Whisper encode toujours une fenêtre de 30 s : c'est l'**encodeur** (32 couches,
  intact dans « turbo ») qui coûte, pas le décodeur. Et il tourne **deux fois** par phrase : une pour détecter la
  langue, une pour transcrire. C'est certain dans le Rust R5 (`bobby-infer/src/stt.rs` l.280 : « Always a fresh
  encode ») et très probable dans faster-whisper (Python).
- **Le LLM génère lentement pour un 4B sur RTX 4060** : 28–36 tokens/s (`llm.log` du 19/11), prompt à
  1 300–1 550 tokens/s. Un 4B Q4_K_M sur une 4060 libre tourne plutôt vers 70–90 tokens/s (ordre de grandeur, à
  vérifier avec `llama-bench`). Hypothèse : le GPU est partagé avec le jeu. La VRAM monte jusqu'à 7,8 / 8 Go (p90
  7,4 Go), et quand la charge GPU dépasse 50 %, Whisper prend +48 % (994 contre 670 ms) et le LLM +38 % (1 300
  contre 943 ms). Le 13/11, la VRAM saturée a donné une transcription de **305 s** et deux « Mon cerveau ne
  répond pas ».
- **Le LLM sert surtout à des ordres simples** : sur 151 appels, 24 volume, 20 musique, 11 move, 11 pc_launch/close,
  6 skip/stop, 5 leave/disconnect. Les regex volume sont ancrées `^…$` : « baisse le son » passe en raccourci
  (1,6 s de bout en bout), mais « baisse **plus** le son » part au LLM (2,5 s) et « baisse **encore un peu** le son »
  aussi (2,8 s). C'était le 19/11, à 30 s d'écart.
- **« Bobby ? » puis l'ordre** : 50 fois (16 % des phrases traitées). Chaque fois, c'est un deuxième passage complet
  (600 ms de silence + Whisper) plus le temps de réaction humain.

## 2. Problèmes, classés par impact

### P1. Musiques coupées avant la fin (confirmé dans le code)
- `bot/music/sources.js` `decode()` ne passe `-reconnect` à ffmpeg **que pour les radios** (`live`). Pour YouTube,
  l'URL googlevideo est lue au rythme réel : le mixer tire 20 ms à la fois, le tuyau de ffmpeg se remplit (~0,3 s de
  PCM), ffmpeg arrête de lire le socket HTTP et la connexion reste inactive. Si googlevideo coupe, ffmpeg y voit une
  fin de fichier : `_ended()` → `_advance()` → morceau suivant. D'où les « switches ».
- Les erreurs sont invisibles : `-loglevel error` et `stderr: 'ignore'`. Le player ne compare pas non plus les
  octets lus à la durée du morceau. **Aucune trace dans les logs** des coupures : on les voit seulement à travers
  Alexandre, qui demande « pourquoi il y a des cuts dans les musiques et des switches » (16/11 19:23) et « pourquoi
  la musique s'est arrêtée ? » (deux fois). Le bot répond « je ne vois aucune erreur dans ton journal ».
- Deuxième cause des « cuts » : `mixer.js` `channel.take()` remplace une donnée manquante par du **silence**. Avec
  seulement ~0,3 s de tampon, la moindre saccade réseau s'entend.
- Autre facteur possible : ffmpeg envoie `User-Agent: Bobby/1.0` à googlevideo, alors que yt-dlp a obtenu l'URL avec
  d'autres en-têtes (fournis dans `-j` → `http_headers`). Cela peut provoquer du bridage.
- **Même défaut en Rust** : `rust/bobby-discord/src/sources.rs` (branche R5) l.228–232, `-reconnect` seulement si
  `live`, `stderr(Stdio::null())`. Le relanceur l.263 (`Ok(false) if !live => return`) ne relance pas un morceau
  YouTube fini trop tôt.

### P2. 600 ms de silence fixe à chaque phrase
C'est 25 % du temps d'un ordre, et on le paie deux fois avec « Bobby ? » + ordre. Raccourcir est risqué :
« Bobby, mets de la musique et je veux l'album de... » (19/11) a déjà été coupé à 600 ms.

### P3. Le LLM traite des ordres que des regex sauraient traiter
Environ 50 % des appels LLM (volume, musique « mets/joue X », skip/stop, leave, move « dans <salon> », pc_launch)
ont une forme simple. Chacun coûte ~0,85 s (p90 2,2 s). Et le LLM se trompe sur certains : « baisse le son de 50 % »
et « divise le son par deux » sont refusés, « tu peux répéter ? » devient `music_skip`, « quelle est ta range de
volume » met le volume à 100, « Là il a capté mais… » devient `music_volume`. Environ 10 erreurs sur 151 (~7 %).

### P4. Whisper : un encodeur deux fois par phrase, et aucun vocabulaire de commande
- Double encodage (voir 1.3). La langue est fr dans 91 % des cas, et sur les 31 « en », 21 sont des transcriptions
  **vides** (bruit détecté comme de l'anglais).
- Erreurs sur les mots de commande et les artistes : « qui te discorde » ×2 et « Que descone » (= « quitte Discord »,
  ce qui a donné un `undo` et deux réponses absurdes), « diviser par deux leçons » (le son), « Hearthstyle » (le
  bot a lancé HEARTSTEEL au lieu de hardstyle), Tame Impala entendu « Té Mimpala / Taming Palace / Temimpala /
  timing pas là » (YouTube a rattrapé 3 cas sur 4), « des mute », « May moins fort ». Environ 15 sur ~280, soit
  ~5 %. Les `hotwords` (`brain.js` l.142) ne contiennent que Bobby, les membres et les salons vocaux.

### P5. Lancer une musique prend 3,2 s (médiane) d'action
`yt-dlp.exe` (PyInstaller onefile) se décompresse à chaque appel, puis fait recherche + résolution. Un
`music_play` passé par le LLM fait 5,9 s de bout en bout (p90 9,2 s).

### P6. LLM ralenti par le partage du GPU, et beaucoup de tokens de sortie
Le schéma impose `analyse:{genre, faisable, pourquoi}` avant les actions. Cela fait ~12–15 tokens sur ~35, soit
environ 400 ms à 30 tokens/s. Le message utilisateur change dès sa première ligne (orateur, membres, date à la
minute) : seul le SYSTEM (6 200 tokens) reste en cache, et ~450 tokens sont réévalués à chaque fois (~350 ms).

### P7. Robustesse
- Aucun délai maximal sur Whisper : le blocage de 305 s du 13/11 a tenu le `gpu` lock tout ce temps. Le LLM, lui, a
  un délai de 20 s (`util.js`).
- `MaxListenersExceededWarning` sur `AudioReceiveStream` (27 fois, le 12/11) : quand le service voix est injoignable,
  `listen()` ne détruit pas le flux opus et réabonne le même flux, ce qui empile les écouteurs `end`.
- TTS non mesurée. Piper synthétise **tout** le WAV avant de répondre, ffmpeg est relancé pour chaque réponse, et la
  réponse attend la fin de l'ack (`await clip`).
- Un bip (wake word d'une autre personne) coupe la réponse en cours : `mixer.overlay()` remplace l'overlay actif.

### Ce qui va bien
Le wake word en streaming (bip pendant la phrase, gate en 13 ms) fait bien son travail : 3 713 phrases jamais
transcrites, et seulement 27 phrases passées sans « Bobby » dans le texte. Le mixer (ducking progressif, crossfade,
fondu) est simple et sain. Le cache de prompt du SYSTEM marche.

## 3. Recommandations

| # | Quoi | Gain attendu | Effort | Risque |
|---|---|---|---|---|
| R1 | **Musique : `-reconnect 1 -reconnect_on_network_error 1 -reconnect_delay_max 5` pour toute URL**, plus les `http_headers` de yt-dlp au lieu de `Bobby/1.0`, plus stderr ffmpeg dans `bot.log`. Mêmes changements dans `sources.rs` | Fin des coupures dues aux connexions coupées (le défaut le plus visible) | 1 h (Node + Rust) | Faible |
| R2 | Détecter une fin anticipée : comparer `bytes` à `duration` (déjà dans `yt-dlp -j`). Si le morceau s'arrête à plus de 5 s de la fin, relancer avec `-ss <position>` et le journaliser | Filet de sécurité pour toute autre cause de coupure | 2 h | Faible |
| R3 | Plus robuste : télécharger le morceau entier (≈ 4 Mo opus) dans `_cache/<id>` avant ou pendant la lecture, puis décoder le fichier. Cela sert aussi de cache pour les morceaux rejoués (Justice, Tame Impala…) | Plus de dépendance au réseau pendant la lecture : plus de trous ni de coupures. Relecture instantanée | 0,5–1 j | Faible (disque) |
| R4 | **Élargir les raccourcis** : volume relatif et en % (« baisse plus/encore/un peu », « de 50 % », « divise par deux », « plus fort »), « mets/joue <X> », skip/stop/pause, « quitte Discord », « move dans <salon> », lancer/fermer <jeu> ; enlever les formules de politesse avant les regex | ~50 % des appels LLM évités, soit **−0,85 s** sur ces ordres (2,4 → ~1,5 s) ; moins d'erreurs du 4B | 0,5–1 j + tests sur le journal | Faible si on rejoue `conversations.jsonl` |
| R5 | **Hotwords de commande** : ajouter « Discord », « hardstyle », « volume », et les artistes/titres récents de l'historique musique | Moins d'erreurs du type « qui te discorde » / « Hearthstyle » / « Taming Palace » | 1 h | Faible (400 caractères max) |
| R6 | **Un seul encodage Whisper** : réutiliser la sortie de l'encodeur de la détection de langue, ou ne détecter qu'en cas de doute (forcer `fr` quand le locuteur parle toujours fr ; lancer en `en` seulement si la proba fr est basse) | −150 à −250 ms par phrase (**estimation**, à confirmer par `stt_compare`) ; moins de transcriptions « en » vides | 0,5 j (dans R5 Rust) | Moyen : le commentaire R5 dit que la réutilisation « changeait des tokens », il faut comprendre pourquoi |
| R7 | **Fin de parole spéculative** : lancer Whisper dès ~300 ms de silence, et annuler si la personne reprend (le GPU est libre à ce moment-là). On garde 600 ms pour valider | **−300 ms** sur toutes les phrases, sans couper davantage | 1 j | Moyen (annulation, file GPU) |
| R8 | **LLM : moins de tokens et un meilleur cache** : clés courtes, `pourquoi` supprimé, éléments variables (date, orateur) à la fin du message, partie stable (salons, membres) en tête | −300 à −500 ms par appel LLM (**estimation**) | 0,5 j + rejouer les tests intent | Moyen (garde-fous qui lisent `analyse`) |
| R9 | **Mesurer le débit réel du LLM** : `llama-bench` à vide puis en jeu. Si on est à 30 tokens/s même à vide, revoir les options. Si c'est le jeu, envisager le LLM sur CPU (Ryzen 9 : 11 tokens/s mesurés, trop lent) ou un modèle plus petit **seulement en jeu** | Comprendre pourquoi on est 2× sous la normale | 30 min | Nul |
| R10 | yt-dlp plus rapide : build « onedir » (`yt-dlp_win.zip`, sans décompression à chaque appel, à vérifier) ou yt-dlp en module dans un process qui reste lancé ; cache requête → id | −0,5 à −1,5 s par `music_play` (**estimation**) | 2–4 h | Faible |
| R11 | Robustesse : délai max sur Whisper (10 s, puis « je n'ai pas entendu »), `opus.destroy()` dans le `catch` de `listen()`, mesurer TTS (premier octet et durée) dans `[timing]` ; un bip ne coupe plus une réponse en cours | Pas de blocage de 5 min, mesures complètes | 2–3 h | Faible |
| R12 | Piper en streaming phrase par phrase (déjà simple en Rust avec `decode_wav`) | Premier son plus tôt sur les réponses longues (Claude, about) | 0,5 j | Faible |

**Ordre conseillé :** R1, R5 et R11 (une demi-journée, gros gain de fiabilité), puis R4 (le plus gros gain de
latence par effort), puis R6 et R8 dans le port Rust R5, puis R7. R3 si R1 et R2 ne suffisent pas.

**Gain cumulé estimé** sur un ordre courant : de 2,4 s aujourd'hui à **~1,2–1,5 s** (raccourci + −300 ms de fin de
parole + −200 ms Whisper), et de ~1,4 s à **~0,9 s** pour un ordre déjà en raccourci.

## 4. Verdict : modèles plus petits ou pas ?

**On garde les modèles. C'est la chaîne autour qu'il faut changer.**

- **Whisper : garder large-v3-turbo int8.** Le temps part dans l'encodeur (fenêtre fixe de 30 s, encodée deux fois),
  et turbo a déjà un décodeur réduit à 4 couches.
  - distil-large-v3 / v3.5 ne gèrent que l'anglais.
  - Les distillations françaises (`whisper-large-v3-french-distil-decN`) ne réduisent que le décodeur : gain
    négligeable ici.
  - small ou medium réduiraient fortement l'encodeur (−300 à −500 ms), mais la précision en français baisse
    nettement. Or les logs montrent que **turbo se trompe déjà** sur exactement les mots qui comptent (« quitte
    Discord », noms d'artistes, « le son »). Une erreur coûte une commande ratée, soit 5 à 10 s, bien plus que les
    300 ms gagnées.
  - faster-whisper/CTranslate2 est **déjà** utilisé (Python, et `ct2rs` en Rust R5).
  - Piste à tester plus tard, pas maintenant : NVIDIA Parakeet-TDT-0.6B-v3 (multilingue, français compris, très
    rapide). Il faudrait une intégration ONNX et vérifier la précision sur le corpus `_training/collected`.
- **LLM : garder Qwen3-4B.** Il se trompe déjà dans ~7 % des cas. Un 1.7B ferait gagner ~0,4 s sur les appels
  restants, mais coûterait plus d'erreurs sur un schéma riche (multi-actions, droits, offres). Le vrai levier est
  d'**appeler le LLM deux fois moins** (R4), de lui faire générer moins (R8), et de comprendre pourquoi il tourne à
  30 tokens/s (R9).
- **Wake word : garder openWakeWord.** Il coûte 13 ms. Le seuil de 0,10 laisse 28 phrases entre 0,05 et 0,10
  (peut-être des « Bobby » ratés). Le réentraînement sur `_training/collected` est la bonne suite, c'est hors du
  sujet latence.
- **Piper : garder les voix medium.** Le streaming (R12) compte plus qu'un changement de voix.

## 4 bis. Les deux modèles face aux demandes réelles

**Ce qu'on demande vraiment à Bobby** (269 requêtes) :
- Langue : 97 % en français (262), 7 en anglais.
- Longueur : 8 mots en médiane (p90 19), 2,1 s d'audio en médiane (p90 6,3 s). Phrases courtes, parlées en jeu,
  par-dessus de la musique, avec beaucoup de noms propres : pseudos dans 29 requêtes, artistes, jeux, salons.
- Ce que fait le LLM sur ses 151 appels : 113 ordres à une action (volume, musique, move, pc…), 9 ordres à
  plusieurs actions, 29 réponses sans action (bavardage, « es-tu capable de… »). Les questions de culture partent
  à Claude, pas au 4B.

**Whisper large-v3-turbo : bon choix.**
- Pour du français court, bruité, plein de noms propres, c'est le meilleur rapport qualité/vitesse disponible
  dans faster-whisper. Il garde l'encodeur complet de large-v3 (donc la robustesse au bruit et aux noms), avec un
  décodeur léger.
- Ses ~5 % d'erreurs tombent sur des mots hors vocabulaire (« quitte Discord », Tame Impala). Cela se corrige par
  les hotwords (R5), pas en changeant de modèle.
- Un modèle plus petit gagnerait 300 à 500 ms, mais au prix de plus de commandes ratées. Les variantes distillées
  sont soit anglaises seulement, soit sans gain ici (elles ne réduisent que le décodeur).
- Le seul vrai défaut est la double passe d'encodeur (R6), qui vient de la chaîne, pas du modèle.

**Qwen3-4B-Instruct-2507 (Q4_K_M) : bon choix de taille, mais il fait trop de travail simple.**
- C'est le meilleur 4B actuel pour suivre des instructions et produire un JSON sous schéma. Il tient avec le jeu
  dans 8 Go (~2,5 Go plus le cache).
- Un 8B serait plus fiable sur les 7 % d'erreurs, mais ne tient pas en VRAM à côté d'un jeu.
- Un 1.7B serait ~2× plus rapide mais plus fragile sur les ordres à plusieurs actions et le bavardage.
- 113 appels sur 151 sont des ordres à une action, qui n'ont pas besoin d'un LLM : c'est là qu'est le gain (R4).
  Une fois les raccourcis élargis, le 4B ne garderait que ce qui demande de la compréhension (~70 appels sur 151),
  et c'est exactement le travail pour lequel il est dimensionné.

**À ne pas faire :**
- passer Whisper sur CPU ;
- passer le LLM sur CPU (11 tokens/s mesurés le 13/11, soit 3 à 7 s par réponse) ;
- prendre un modèle plus gros que 4B tant que le jeu partage la carte.

## 4 ter. Et sur le processeur (Ryzen 9 7900X, DDR5-6000) ?

**Mesuré** (`llm-cpu.log`, 122 appels du 13/11, Qwen3-4B sur CPU, 8 threads) :
- 4,0 s par appel en médiane, 5,9 s au p90, contre 0,84 s sur le GPU ;
- génération : 10,4 tokens/s ;
- petits prompts : 26 tokens/s ;
- et ces chiffres viennent de prompts minuscules (13 tokens). Un vrai appel réévalue ~450 tokens, donc ce serait
  pire.

**Plafond théorique :**
- Un LLM sur CPU est limité par la bande passante mémoire. La DDR5-6000 en double canal donne ~96 Go/s en théorie,
  ~70 en pratique.
- Les poids font ~2,5 Go, donc on plafonne à ~25 tokens/s en génération, avec 12 threads (un par cœur physique)
  et sans jeu lancé. 36 tokens prendraient alors ~1,5 s.
- Les ~450 tokens de prompt prendraient 1,5 à 3 s de plus (estimé ~150 à 300 tokens/s sur Zen 4).
- Au total, ~3 à 4 s par appel, soit **4× plus lent que le GPU** même partagé.
- En plus, pendant l'inférence le jeu perd la bande passante mémoire (saccades).

**Whisper sur CPU** : turbo doit encoder une fenêtre de 30 s. Sur CPU, cela fait 1,5 à 3 s par phrase (estimé),
contre 0,74 s sur le GPU. Non.

**Ce que le CPU fait déjà bien et doit garder** : wake word, VAD Silero, Piper. Tous sont petits et sensibles à
la latence, pas à la bande passante.

**Seul usage utile du CPU pour le LLM : secours automatique** quand la VRAM est saturée (le cas du 13/11). Aujourd'hui,
Bobby dit « mon cerveau ne répond pas » après 20 s. Il pourrait basculer sur un llama-server CPU (le 4B en ~4 s, ou un
1.7B en ~2 s) le temps que le jeu libère la carte. Ce secours devient bien moins nécessaire une fois les raccourcis
élargis (R4), puisque la plupart des ordres ne passeront plus par le LLM.

## 5. À vérifier avec GPU, micro ou appli (pas fait ici)
- `llama-bench` Qwen3-4B à vide puis en jeu (R9).
- `stt_compare` R5 : temps d'encodage seul, et effet de la réutilisation de l'encodeur sur les transcriptions (R6).
- Durée de la synthèse Piper par réponse (instrumenter, R11).
- Reproduire une coupure de musique avec stderr ffmpeg visible (R1) pour confirmer la cause exacte (EOF vs 403 vs
  bridage).
