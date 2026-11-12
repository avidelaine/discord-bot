// Bobby's own Discord music (io.music, see bot/music/player.js): play a title or a radio style, queue, transport, volume.
// Bobby is the default target of bare "pause", "skip", "volume N", "joue ..."; the PC (actions/pc.js) needs "sur le PC / Spotify / YouTube Music".
const act = (type, text = '', kind) => [{ type, text, ...(kind && { kind }) }];

// Runs fn(player, step) and keeps the plain-data result in s.res; no player (tests, voice not ready) = error.
const go = (fn) => async (s, io) => { s.res = io.music ? await fn(io.music, s) : { error: 'no player' }; };
const item = (s) => (s.kind === 'radio' ? { kind: 'radio', style: s.text } : { kind: 'youtube', query: s.text });

const NOTHING = { fr: 'Rien ne joue.', en: 'Nothing is playing.' };
const ERR = { 'nothing is playing': NOTHING, superseded: { fr: '', en: '' } };
// Every line goes through here so errors are spoken plainly in both languages.
// A bare "stop" (s.quiet) with nothing playing says nothing at all.
const quiet = (s) => s.quiet && s.res?.error === 'nothing is playing';
const say = (fr, en) => ({
  fr: (s) => (quiet(s) ? '' : s.res?.error ? (ERR[s.res.error] || { fr: `Ça n'a pas marché : ${s.res.error}.` }).fr : fr(s.res || {}, s)),
  en: (s) => (quiet(s) ? '' : s.res?.error ? (ERR[s.res.error] || { en: `That didn't work: ${s.res.error}.` }).en : en(s.res || {}, s)),
});
const radio = (r, s) => r.kind === 'radio' || s.kind === 'radio';
const playing = say(
  (r, s) => (radio(r, s) ? `Radio ${s.text} : ${r.station || r.title}.` : `Je lance ${r.title}.`),
  (r, s) => (radio(r, s) ? `${s.text} radio: ${r.station || r.title}.` : `Playing ${r.title}.`),
);
const simple = (fr, en, fn) => ({ role: 'prime', run: go(fn), say: say(() => fr, () => en) });

const UP = /^(?:(?:monte|augmente) (?:le )?(?:son|volume)|monte|plus fort|louder|turn (?:it )?up|(?:raise|increase|turn up) (?:the )?volume)$/;
const DOWN = /^(?:(?:baisse|diminue|descends) (?:le )?(?:son|volume)|baisse|moins fort|quieter|softer|turn (?:it )?down|(?:lower|decrease|reduce|turn down) (?:the )?volume)$/;
const SET = /^(?:(?:mets|met|règle|regle) (?:le )?(?:son|volume) (?:à|a|sur) |volume (?:à |a |to )?|set (?:the )?volume to )(\d+)(?: ?%| pour cent| percent)?$/;
const STYLE = /^(?:joue|lance|mets|play|put on) (?:(?:une? )?radio (?:de |d')?|(?:du |de la |de l'|des |d'|some |a bit of )(?!(?:la )?(?:musique|music)$))(.+?)(?: radio)?$/;

module.exports = {
  help: {
    fr: "jouer de la musique (un titre sur YouTube ou une radio par style), pause, suivant, file d'attente, volume",
    en: 'play music (a YouTube title or a radio by style), pause, skip, queue, volume',
  },
  short: { fr: 'la musique', en: 'music' },
  prompt: [
    '- music_play : jouer de la musique avec Bobby ; "text" est le titre ou l\'artiste (champ "kind" = youtube) ou le style/genre (champ "kind" = radio)',
    '- music_add : ajouter à la file d\'attente, "text" et "kind" comme music_play',
    '- music_pause / music_resume / music_stop / music_skip / music_replay : pause, reprendre, arrêter (vide la file), passer au suivant, rejouer',
    '- music_queue : dire la file d\'attente',
    '- music_volume : volume de Bobby, "text" vaut up, down ou un nombre de 0 à 100',
    '- music_now : dire quel morceau joue',
  ],
  rules: [
    '- Pause, reprise, suivant, volume et "c\'est quoi ce son" sans mention du PC, de Spotify ou de YouTube Music concernent la musique de Bobby (music_*), pas pc_media. "kind" = radio pour un style, un genre ou le mot radio, sinon youtube.',
  ],
  fields: { kind: { enum: ['youtube', 'radio'] } },
  examples: [
    '"joue Bohemian Rhapsody" -> {"actions":[{"type":"music_play","text":"Bohemian Rhapsody","kind":"youtube"}],"say":""}',
    '"mets une radio jazz" -> {"actions":[{"type":"music_play","text":"jazz","kind":"radio"}],"say":""}',
    '"play some lofi" -> {"actions":[{"type":"music_play","text":"lofi","kind":"radio"}],"say":""}',
    '"play Around the World by Daft Punk" -> {"actions":[{"type":"music_play","text":"Around the World Daft Punk","kind":"youtube"}],"say":""}',
    '"passe à la suivante" -> {"actions":[{"type":"music_skip"}],"say":""}',
    '"mets le volume à 40" -> {"actions":[{"type":"music_volume","text":"40"}],"say":""}',
    '"ajoute Blinding Lights à la file" -> {"actions":[{"type":"music_add","text":"Blinding Lights","kind":"youtube"}],"say":""}',
    '"what song is this" -> {"actions":[{"type":"music_now"}],"say":""}',
  ],
  // ponytail: a handful of phrasings per language; the LLM takes the rest (e.g. "add X to the queue").
  quick: [
    [/^(?:mets? (?:la musique )?en pause|mets? pause|pause(?: la musique| the music| music)?)$/, () => act('music_pause')],
    [/^(?:reprends|relance|remets|continue|resume|unpause)(?: la musique| the music| music)?$/, () => act('music_resume')],
    [/^(?:skip(?: (?:this|the) (?:song|track|music))?|next(?: song| track)?|suivante?|passe|saute|(?:musique|morceau|titre|chanson|piste) suivante?|(?:passe|saute) (?:cette |la )?(?:musique|chanson|piste|titre|morceau)|prochaine (?:musique|chanson))$/, () => act('music_skip')],
    [/^(?:stop|arrête|arrete)$/, () => [{ type: 'music_stop', text: '', quiet: true }]], // stops the music if any, else silence; never leaves
    [/^(?:(?:stop|arrête|arrete|coupe)(?: la| le| the)? (?:musique|music|radio))$/, () => act('music_stop')],
    [/^(?:rejoue|replay|recommence)(?: (?:ça|ce son|cette musique|le morceau|this|it|the song|this song))?$|^(?:joue|play) (?:ça|it|this|that)(?: again| encore)$/, () => act('music_replay')],
    [UP, () => act('music_volume', 'up')],
    [DOWN, () => act('music_volume', 'down')],
    [SET, (m) => act('music_volume', m[1])],
    [/^(?:c'est quoi|quelle est|quel est|qu'est-ce qui (?:joue|passe))(?: ce| cette| la| le)? (?:son|musique|morceau|titre|chanson)(?: qui (?:joue|passe))?$|^what(?:'s| is) (?:this|that|the) (?:song|track|music|sound)(?: playing)?$|^what(?:'s| is) playing$|^what (?:song|music|track) is (?:this|playing)$|^qu'est-ce qui (?:joue|passe)$/, () => act('music_now')],
    [/^(?:c'est quoi |quelle est |montre |dis-moi )?(?:la )?file(?: d'attente)?$|^(?:what'?s in |show )?(?:the )?queue$/, () => act('music_queue')],
    [/^(?:joue|lance|mets|play|put on)(?: de la| some)? (?:musique|music)$/, () => act('music_play', 'pop', 'radio')], // ponytail: no style given = pop radio
    [STYLE, (m) => act('music_play', m[1], 'radio')],
    [/^(?:joue|play) (.+)$/, (m) => act('music_play', m[1], 'youtube')],
  ],
  actions: {
    music_play: { role: 'prime', text: true, run: go((p, s) => p.playNow(item(s))), say: playing },
    music_add: {
      role: 'prime', text: true, run: go((p, s) => p.add(item(s))),
      say: say(
        (r, s) => (r.started ? playing.fr(s) : `J'ajoute ${r.label} à la file, position ${r.position}.`),
        (r, s) => (r.started ? playing.en(s) : `Added ${r.label} to the queue, position ${r.position}.`),
      ),
    },
    music_pause: simple('Pause.', 'Paused.', (p) => p.pause()),
    music_resume: simple('Je reprends.', 'Resuming.', (p) => p.resume()),
    music_stop: { role: 'prime', run: go((p) => (p.nowPlaying() ? p.stop() : { error: 'nothing is playing' })), say: say(() => 'Musique arrêtée.', () => 'Stopped.') },
    music_skip: {
      role: 'prime', run: go((p) => p.skip()),
      say: say((r) => (r.nowPlaying ? `Suivant : ${r.nowPlaying.title}.` : 'Plus rien dans la file.'), (r) => (r.nowPlaying ? `Next: ${r.nowPlaying.title}.` : 'Nothing left in the queue.')),
    },
    music_replay: {
      role: 'prime', run: go((p) => p.replay()),
      say: say((r) => `Je relance ${r.nowPlaying.title}.`, (r) => `Replaying ${r.nowPlaying.title}.`),
    },
    music_queue: {
      role: 'prime', run: go((p) => p.list()),
      say: say(
        (r) => (r.queue.length ? `File : ${r.queue.length} titre${r.queue.length > 1 ? 's' : ''}, prochain : ${r.queue[0].label}.` : r.nowPlaying ? `File vide, en cours : ${r.nowPlaying.title}.` : NOTHING.fr),
        (r) => (r.queue.length ? `Queue: ${r.queue.length} track${r.queue.length > 1 ? 's' : ''}, next: ${r.queue[0].label}.` : r.nowPlaying ? `Queue empty, playing: ${r.nowPlaying.title}.` : NOTHING.en),
      ),
    },
    music_volume: {
      role: 'prime', text: true,
      run: go((p, s) => { const t = s.text.toLowerCase().trim(); return t === 'up' ? p.volumeUp() : t === 'down' ? p.volumeDown() : /^\d+$/.test(t) ? p.setVolume(+t) : { error: `unknown volume "${s.text}"` }; }),
      say: say((r) => `Volume à ${r.volume}.`, (r) => `Volume at ${r.volume}.`),
    },
    music_now: {
      role: 'prime', run: go((p) => ({ nowPlaying: p.nowPlaying() })),
      say: say(
        (r) => (r.nowPlaying ? (r.nowPlaying.kind === 'radio' ? `C'est la radio ${r.nowPlaying.title}.` : `C'est ${r.nowPlaying.title}.`) : NOTHING.fr),
        (r) => (r.nowPlaying ? (r.nowPlaying.kind === 'radio' ? `It's the radio ${r.nowPlaying.title}.` : `It's ${r.nowPlaying.title}.`) : NOTHING.en),
      ),
    },
  },
};
