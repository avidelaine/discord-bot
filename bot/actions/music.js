// Bobby's own Discord music (io.music, see bot/music/player.js): play a title or a radio style, queue, transport, volume.
// Bobby is the default target of bare "pause", "skip", "volume N", "joue ..."; the PC (actions/pc.js) needs "sur le PC / Spotify / YouTube Music".
const act = (type, text = '', kind) => [{ type, text, ...(kind && { kind }) }];

// Runs fn(player, step) and keeps the plain-data result in s.res; no player (tests, voice not ready) = error.
const go = (fn) => async (s, io) => { s.res = io.music ? await fn(io.music, s) : { error: 'no player' }; };
// Endless by default (2026-11-16): a song or a style goes on with similar tracks; "que du X" = only that artist;
// "juste ce titre" = that song alone. A web radio is endless by nature.
const item = (s) => {
  if (s.kind === 'radio') return { kind: 'radio', style: s.text };
  if (s.kind === 'style' && s.mode !== 'artiste') return { kind: 'style', style: s.text };
  return { kind: 'youtube', query: s.text, ...(s.mode === 'unique' ? {} : s.mode === 'artiste' ? { endless: 'artist', seed: s.text } : { endless: 'similar' }) };
};
const then = { fr: { similar: ', puis la suite dans le même style' }, en: { similar: ', then more in the same style' } };

const NOTHING = { fr: 'Rien ne joue.', en: 'Nothing is playing.' };
const ERR = {
  'nothing is playing': NOTHING, superseded: { fr: '', en: '' },
  // local mode (Bobby on the PC): the player streams to Discord only
  'no player': { fr: 'La musique ne marche que sur Discord : dis « Bobby, connecte-toi sur Discord ».', en: 'Music only works on Discord: say "Bobby, join Discord".' },
};
// Every line goes through here so errors are spoken plainly in both languages.
// A bare "stop" (s.quiet) with nothing playing says nothing at all.
const quiet = (s) => s.quiet && s.res?.error === 'nothing is playing';
const say = (fr, en) => ({
  fr: (s) => (quiet(s) ? '' : s.res?.error ? (ERR[s.res.error] || { fr: `Ça n'a pas marché : ${s.res.error}.` }).fr : fr(s.res || {}, s)),
  en: (s) => (quiet(s) ? '' : s.res?.error ? (ERR[s.res.error] || { en: `That didn't work: ${s.res.error}.` }).en : en(s.res || {}, s)),
});
const radio = (r, s) => r.kind === 'radio' || s.kind === 'radio';
const playing = say(
  (r, s) => (radio(r, s) ? `Radio ${s.text} : ${r.station || r.title}.` : r.endless === 'artist' ? `Que du ${r.seed} : je lance ${r.title}.` : `Je lance ${r.title}${then.fr[r.endless] || ''}.`),
  (r, s) => (radio(r, s) ? `${s.text} radio: ${r.station || r.title}.` : r.endless === 'artist' ? `Only ${r.seed}: playing ${r.title}.` : `Playing ${r.title}${then.en[r.endless] || ''}.`),
);
const endlessLine = { fr: (e) => (e.mode === 'artist' ? `que du ${e.seed} en continu` : `en continu dans le style de ${e.seed}`), en: (e) => (e.mode === 'artist' ? `only ${e.seed}, nonstop` : `nonstop in the style of ${e.seed}`) };
// echo: done silently after the ack (see brain.js report); errors are always spoken.
const ok = (s) => !s.res?.error;
const simple = (fr, en, fn) => ({ role: 'prime', run: go(fn), echo: ok, say: say(() => fr, () => en) });

const UP = /^(?:(?:monte|augmente) (?:le )?(?:son|volume)|monte|plus fort|louder|turn (?:it )?up|(?:raise|increase|turn up) (?:the )?volume)$/;
const DOWN = /^(?:(?:baisse|diminue|descends) (?:le )?(?:son|volume)|baisse|moins fort|quieter|softer|turn (?:it )?down|(?:lower|decrease|reduce|turn down) (?:the )?volume)$/;
const SET = /^(?:(?:mets|met|règle|regle) (?:le )?(?:son|volume) (?:à|a|sur) |volume (?:à |a |to )?|set (?:the )?volume to )(\d+)(?: ?%| pour cent| percent)?$/;
// 'musique hardstyle en boucle' -> 'hardstyle': the filler words would pollute the YouTube search
const clean = (t) => t.replace(/^(?:la |de la )?(?:musique|music|chansons?|songs?) (?:de |d'|du |by )?/i, '').replace(/ (?:en boucle|en continu|non[- ]?stop|sans arr[êe]t|à l'infini|on repeat|nonstop|songs?|chansons?|musique|music)$/i, '').trim();
const RADIO = /^(?:joue|lance|mets|play|put on) (?:une? |a )?radio (?:de |d'|of )?(.+?)$|^(?:joue|lance|mets|play|put on) (?:some |a )?(.+?) radio$/;
const STYLE = /^(?:joue|lance|mets|play|put on)(?:-moi| me)? (?:du |de la |de l'|des |d'|some |a bit of )(?!(?:la )?(?:musique|music)$)(.+?)$/;
const ONLY = /^(?:joue|lance|mets|play|put on)(?:-moi| me)? (?:que |seulement |uniquement |only )(?:du |de la |de l'|des |d')?(?:musique de |music by |songs by )?(.+?)$/;
const JUST = /^(?:joue|lance|mets|play|put on)(?:-moi| me)? (?:juste |just )(?:le titre |la chanson |le morceau |the song )?(.+?)$/;

module.exports = {
  help: {
    fr: "jouer de la musique en continu (un style, un titre, ou que un artiste), une radio, pause, suivant, file d'attente, volume",
    en: 'play music nonstop (a style, a song, or only one artist), a radio, pause, skip, queue, volume',
  },
  short: { fr: 'la musique', en: 'music' },
  prompt: [
    '- music_play : jouer de la musique avec Bobby, en continu : "text" = un titre ou un artiste ("kind" = youtube), un style ou un genre ("kind" = style), ou une radio quand le mot radio est dit ("kind" = radio). "mode" : similaire par défaut (des morceaux du même style s\'enchaînent sans fin, pas forcément du même artiste), artiste quand on ne veut que cet artiste ("que du…", "seulement…", "uniquement…", "only…"), unique pour ce titre seul ("juste ce titre", "rien après")',
    '- music_add : ajouter à la file d\'attente, "text" et "kind" comme music_play',
    '- music_pause / music_resume / music_stop / music_skip / music_replay : pause, reprendre, arrêter (vide la file), passer au suivant, rejouer',
    '- music_queue : dire la file d\'attente',
    '- music_volume : volume de Bobby, de 0 à 100 (100 = le maximum) ; "text" vaut up, down ou le nombre demandé (même au-dessus de 100 : Bobby met le maximum et le dit), ou -N%, +N% ou /N pour un changement relatif',
    '- music_now : dire quel morceau joue',
  ],
  rules: [
    '- Pause, reprise, suivant, volume et "c\'est quoi ce son" sans mention du PC, de Spotify ou de YouTube Music concernent la musique de Bobby (music_*), pas pc_media. "kind" = radio seulement si le mot radio est dit, style pour un style ou un genre, sinon youtube. Un seul music_play par demande de musique : la suite dans le même style est automatique.',
  ],
  fields: { kind: { enum: ['youtube', 'style', 'radio'] }, mode: { enum: ['similaire', 'artiste', 'unique'] } },
  examples: [
    '"joue Bohemian Rhapsody" -> {"actions":[{"type":"music_play","text":"Bohemian Rhapsody","kind":"youtube"}],"say":""}',
    '"mets une radio jazz" -> {"actions":[{"type":"music_play","text":"jazz","kind":"radio"}],"say":""}',
    '"play some lofi" -> {"actions":[{"type":"music_play","text":"lofi","kind":"style"}],"say":""}',
    '"joue-moi de la hardstyle en boucle" -> {"actions":[{"type":"music_play","text":"hardstyle","kind":"style"}],"say":""}',
    '"mets que du Adèle Castillon" -> {"actions":[{"type":"music_play","text":"Adèle Castillon","kind":"youtube","mode":"artiste"}],"say":""}',
    '"joue Bohemian Rhapsody et ensuite des trucs dans le genre" -> {"actions":[{"type":"music_play","text":"Bohemian Rhapsody","kind":"youtube"}],"say":""}',
    '"joue juste Bohemian Rhapsody, rien après" -> {"actions":[{"type":"music_play","text":"Bohemian Rhapsody","kind":"youtube","mode":"unique"}],"say":""}',
    '"play Around the World by Daft Punk" -> {"actions":[{"type":"music_play","text":"Around the World Daft Punk","kind":"youtube"}],"say":""}',
    '"passe à la suivante" -> {"actions":[{"type":"music_skip"}],"say":""}',
    '"mets le volume à 40" -> {"actions":[{"type":"music_volume","text":"40"}],"say":""}',
    '"baisse le son de 50 %" -> {"actions":[{"type":"music_volume","text":"-50%"}],"say":""}',
    '"ajoute Blinding Lights à la file" -> {"actions":[{"type":"music_add","text":"Blinding Lights","kind":"youtube"}],"say":""}',
    '"what song is this" -> {"actions":[{"type":"music_now"}],"say":""}',
    '"quelle est ta plage de volume" -> {"analyse":{"genre":"capacite","faisable":true},"actions":[],"say":"Mon volume va de 0 à 100 %."}',
  ],
  // ponytail: a handful of phrasings per language; the LLM takes the rest (e.g. "add X to the queue").
  quick: [
    [/^(?:mets? (?:la musique )?en pause|mets? pause|pause(?: la musique| the music| music)?)$/, () => act('music_pause')],
    [/^(?:reprends|relance|remets|continue|resume|unpause)(?: la musique| the music| music)?$/, () => act('music_resume')],
    [/^(?:skip(?: (?:this|the) (?:song|track|music))?|next(?: song| track)?|suivante?|passe|saute|(?:musique|morceau|titre|chanson|piste) suivante?|(?:passe|saute) (?:cette |la )?(?:musique|chanson|piste|titre|morceau)|prochaine (?:musique|chanson))$/, () => act('music_skip')],
    [/^(?:stop|arrête|arrete)$/, () => [{ type: 'music_stop', text: '', quiet: true }]], // stops the music if any, else silence; never leaves
    [/^(?:(?:stop|arrête|arrete|coupe)(?: la| le| the)? (?:musique|music|radio))$/, () => act('music_stop')],
    [/^(?:(?:passe|mets-toi|va|entre|bascule) en )?(?:deep sleep|veille profonde)$|^(?:dors|va dormir|endors-toi|go to sleep)$/, () => act('deep_sleep')], // Bobby's own sleep, never the PC's
    [/^(?:rejoue|replay|recommence)(?: (?:ça|ce son|cette musique|le morceau|this|it|the song|this song))?$|^(?:joue|play) (?:ça|it|this|that)(?: again| encore)$/, () => act('music_replay')],
    [UP, () => act('music_volume', 'up')],
    [DOWN, () => act('music_volume', 'down')],
    [SET, (m) => act('music_volume', m[1])],
    [/^(?:c'est quoi|quelle est|quel est|qu'est-ce qui (?:joue|passe))(?: ce| cette| la| le)? (?:son|musique|morceau|titre|chanson)(?: qui (?:joue|passe))?$|^what(?:'s| is) (?:this|that|the) (?:song|track|music|sound)(?: playing)?$|^what(?:'s| is) playing$|^what (?:song|music|track) is (?:this|playing)$|^qu'est-ce qui (?:joue|passe)$/, () => act('music_now')],
    [/^(?:c'est quoi |quelle est |montre |dis-moi )?(?:la )?file(?: d'attente)?$|^(?:what'?s in |show )?(?:the )?queue$/, () => act('music_queue')],
    [/^(?:joue|lance|mets|play|put on)(?:-moi| me)?(?: de la| some)? (?:musique|music)$/, () => act('music_play', 'pop', 'style')], // ponytail: no style given = pop
    [RADIO, (m) => act('music_play', clean(m[1] || m[2]), 'radio')],
    [ONLY, (m) => [{ type: 'music_play', text: clean(m[1]), kind: 'youtube', mode: 'artiste' }]],
    [JUST, (m) => [{ type: 'music_play', text: clean(m[1]), kind: 'youtube', mode: 'unique' }]],
    [STYLE, (m) => act('music_play', clean(m[1]), 'style')],
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
    // Deep Sleep (Whisper unloaded, LLM stopped until "Bobby") lives in the Rust brain's sleep machine: on this line
    // Bobby says so plainly instead of mistaking it for the PC's sleep.
    deep_sleep: {
      role: 'admin', run: async () => ({}),
      say: {
        fr: () => "Le deep sleep ne marche qu'avec mon cerveau Rust. Sur cette configuration, je reste éveillé.",
        en: () => 'Deep sleep only works with my Rust brain. On this setup I stay awake.',
      },
    },
    music_pause: simple('Pause.', 'Paused.', (p) => p.pause()),
    music_resume: simple('Je reprends.', 'Resuming.', (p) => p.resume()),
    music_stop: { role: 'prime', echo: ok, // always stop: a track still being looked up (current not set yet) must not start after "stop"
    run: go((p) => { const r = p.stop(); return r.stopped || r.cleared ? r : { error: 'nothing is playing' }; }), say: say(() => 'Musique arrêtée.', () => 'Stopped.') },
    music_skip: {
      role: 'prime', run: go((p) => p.skip()),
      say: say((r) => (r.nowPlaying ? `Suivant : ${r.nowPlaying.title}.` : 'Plus rien dans la file.'), (r) => (r.nowPlaying ? `Next: ${r.nowPlaying.title}.` : 'Nothing left in the queue.')),
    },
    music_replay: {
      role: 'prime', echo: ok, run: go((p) => p.replay()),
      say: say((r) => `Je relance ${r.nowPlaying.title}.`, (r) => `Replaying ${r.nowPlaying.title}.`),
    },
    music_queue: {
      role: 'prime', run: go((p) => p.list()),
      say: say(
        (r) => (r.queue.length ? `File : ${r.queue.length} titre${r.queue.length > 1 ? 's' : ''}, prochain : ${r.queue[0].label}.` : r.endless ? `En cours : ${r.nowPlaying?.title}, ${endlessLine.fr(r.endless)}.` : r.nowPlaying ? `File vide, en cours : ${r.nowPlaying.title}.` : NOTHING.fr),
        (r) => (r.queue.length ? `Queue: ${r.queue.length} track${r.queue.length > 1 ? 's' : ''}, next: ${r.queue[0].label}.` : r.endless ? `Playing ${r.nowPlaying?.title}, ${endlessLine.en(r.endless)}.` : r.nowPlaying ? `Queue empty, playing: ${r.nowPlaying.title}.` : NOTHING.en),
      ),
    },
    music_volume: {
      role: 'prime', text: true,
      // "200 %" sets the maximum and says so (it used to set 100 without a word); a plain change stays silent
      echo: (s) => ok(s) && !s.res.clamped,
      run: go((p, s) => {
        const t = s.text.toLowerCase().replace(/\s*(?:%|pour ?cent|percent)$/, '').trim();
        if (t === 'up') return p.volumeUp();
        if (t === 'down') return p.volumeDown();
        const rel = /^([+-])(\d+)\s*(?:%|pour ?cent|percent)$/.exec(s.text.toLowerCase().trim()), div = /^\/(\d+)$/.exec(t);
        if (rel || (div && +div[1] >= 1)) {
          const cur = p.volume().volume, dir = rel ? (rel[1] === '-' ? -1 : 1) * (+rel[2] > 0) : (+div[1] > 1 ? -1 : 0);
          const raw = rel ? Math.round(cur * (1 + dir * rel[2] / 100)) : Math.round(cur / +div[1]);
          // a change too small to move the rounded volume ("-10 %" at 4) still moves it by one step
          const n = raw === cur && dir ? cur + dir : raw;
          return { ...p.setVolume(Math.max(0, Math.min(100, n))), clamped: raw > 100 };
        }
        return /^\d+$/.test(t) ? { ...p.setVolume(+t), clamped: +t > 100 } : { error: `unknown volume "${s.text}"` };
      }),
      say: say(
        (r) => (r.clamped ? "Le maximum c'est 100 %, je le mets à fond." : `Volume à ${r.volume}.`),
        (r) => (r.clamped ? "100 % is the maximum, so I'm turning it all the way up." : `Volume at ${r.volume}.`),
      ),
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
