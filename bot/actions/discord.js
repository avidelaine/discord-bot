// Voice moderation and messages: mute, deafen, move, disconnect, post, and Bobby leaving.
const ALL = /^(?:tout le monde|everyone|everybody)$/;
const who = (w) => [ALL.test(w) ? 'tous' : w];

// Server mute / deafen, undone by restoring each person's previous state.
const voiceSet = (key, on) => ({
  role: 'admin',
  who: true,
  async run(s) {
    const ms = s.targets.map((t) => t.ref);
    const set = (m, v) => (key === 'serverMute' ? m.voice.setMute(v) : m.voice.setDeaf(v));
    const prev = ms.map((m) => [m, m.voice[key]]);
    await Promise.all(ms.map((m) => set(m, on)));
    return () => Promise.all(prev.map(([m, v]) => set(m, v)));
  },
});

module.exports = {
  help: {
    fr: 'couper ou rétablir le micro et le son, déplacer, ramener ou déconnecter des gens, écrire dans un salon',
    en: 'mute, deafen, move, drag or disconnect people, post in a channel',
  },
  prompt: [
    '- mute / unmute : couper ou rétablir le micro de "who"',
    '- deafen / undeafen : couper ou rétablir le son (le casque) de "who"',
    '- move : déplacer "who" dans le salon vocal "channel"',
    '- disconnect : déconnecter "who" du vocal',
    '- message : envoyer "text" dans le salon texte "channel"',
    '- leave : Bobby quitte le salon vocal',
  ],
  rules: [
    '- "virer", "dégager" ou "kicker" une personne nommée du vocal, c\'est disconnect. Si on dit à Bobby lui-même de partir, c\'est leave.',
    '- "dé-mute", "démute" ou "des mute" (mal transcrit), c\'est unmute. "drag" ou "draguer" quelqu\'un, c\'est le déplacer (move) dans le salon de celui qui parle.',
    '- "couper" quelqu\'un sans préciser, c\'est mute (le micro seulement).',
    '- Bobby, c\'est toi : tu ne peux ni te muter ni te déplacer ("actions" vide, dis-le dans "say").',
    '- "ici" désigne le salon vocal de celui qui parle : mets son nom exact dans "channel".',
  ],
  examples: [
    '"coupe le micro de Kevin" -> {"actions":[{"type":"mute","who":["Kévin"]}],"say":""}',
    '"tout le monde en AFK" -> {"actions":[{"type":"move","who":["tous"],"channel":"AFK"}],"say":""}',
    '"ramène tout le monde ici sauf Marie" -> {"actions":[{"type":"move","who":["tous"],"except":["Marie"],"channel":"Général"}],"say":""}',
    '"mets-moi dans Gaming et rends le son à Zyklon" -> {"actions":[{"type":"move","who":["moi"],"channel":"Gaming"},{"type":"undeafen","who":["Zyklon"]}],"say":""}',
    '"rends le micro à tout le monde sauf Zyklon" -> {"actions":[{"type":"unmute","who":["tous"],"except":["Zyklon"]}],"say":""}',
    '"coupe tout le monde sauf Marie" -> {"actions":[{"type":"mute","who":["tous"],"except":["Marie"]}],"say":""}',
    '"drague Lucas" -> {"actions":[{"type":"move","who":["Lucas"],"channel":"Général"}],"say":""}',
    '"c\'est bon tu peux y aller" -> {"actions":[{"type":"leave"}],"say":""}',
    '"move me and Zyklon to Gaming" -> {"actions":[{"type":"move","who":["moi","Zyklon"],"channel":"Gaming"}],"say":""}',
    '"you can leave now" -> {"actions":[{"type":"leave"}],"say":""}',
    '"kick Lucas from voice" -> {"actions":[{"type":"disconnect","who":["Lucas"]}],"say":""}',
  ],
  // LLM-free shortcuts. ponytail: a handful of phrasings per language; add the ones that keep showing up in the logs.
  quick: [
    [/^(?:coupe|ferme|mute)(?: le)? micro (?:de|à|a) (.+)$/, (m) => [{ type: 'mute', who: who(m[1]) }]],
    [/^(?:dé|de|des|dés) ?-?mute (.+)$/, (m) => [{ type: 'unmute', who: who(m[1]) }]],
    [/^mute (.+)$/, (m) => [{ type: 'mute', who: who(m[1]) }]],
    [/^(?:rends|remets|rétablis|redonne|rallume)(?: le)? micro (?:de|à|a) (.+)$/, (m) => [{ type: 'unmute', who: who(m[1]) }]],
    [/^unmute (.+)$/, (m) => [{ type: 'unmute', who: who(m[1]) }]],
    [/^(?:coupe le son (?:de|à|a)|deafen) (.+)$/, (m) => [{ type: 'deafen', who: who(m[1]) }]],
    [/^(?:(?:rends|remets|rétablis|redonne) le son (?:de|à|a)|undeafen) (.+)$/, (m) => [{ type: 'undeafen', who: who(m[1]) }]],
    [/^(?:déplace|mets|envoie|bouge) (?!la musique |le pc |l'ordi|mon pc |pause |le son |le volume |du |de la )(.+?) (?:dans|en|sur|vers) (?:le salon |la salle (?:de )?)?(.+)$/, (m) => [{ type: 'move', who: who(m[1]), channel: m[2] }]],
    [/^mets[- ]moi (?:dans|en) (?:le salon |la salle (?:de )?)?(.+)$/, (m) => [{ type: 'move', who: ['moi'], channel: m[1] }]],
    [/^(?:move|put|send|drag) (.+?) (?:to|in|into) (?:the )?(.+?)(?: channel)?$/, (m) => [{ type: 'move', who: who(m[1]), channel: m[2] }]],
    [/^(?:drag|draguer|drague|ramène) (.+?)(?: ici| here)?$/, (m) => [{ type: 'move', who: who(m[1]), channel: 'ici' }]],
    [/^(?:kick|vire|dégage|déconnecte) (.+?)(?: du vocal| from voice| out of voice)?$/, (m) => [{ type: 'disconnect', who: who(m[1]) }]],
  ],
  actions: {
    mute: { ...voiceSet('serverMute', true), say: { fr: (s, n) => `Je coupe le micro de ${n}.`, en: (s, n) => `Muting ${n}.` } },
    unmute: { ...voiceSet('serverMute', false), say: { fr: (s, n) => `Je rétablis le micro de ${n}.`, en: (s, n) => `Unmuting ${n}.` } },
    deafen: { ...voiceSet('serverDeaf', true), say: { fr: (s, n) => `Je coupe le son de ${n}.`, en: (s, n) => `Deafening ${n}.` } },
    undeafen: { ...voiceSet('serverDeaf', false), say: { fr: (s, n) => `Je rétablis le son de ${n}.`, en: (s, n) => `Undeafening ${n}.` } },
    move: {
      role: 'admin',
      who: true,
      channel: 'voice',
      async run(s) {
        const ms = s.targets.map((t) => t.ref);
        const prev = ms.map((m) => [m, m.voice.channel]);
        await Promise.all(ms.map((m) => m.voice.setChannel(s.channel.ref)));
        return () => Promise.all(prev.map(([m, c]) => m.voice.setChannel(c)));
      },
      say: { fr: (s, n) => `Je déplace ${n} dans ${s.channel.name}.`, en: (s, n) => `Moving ${n} to ${s.channel.name}.` },
    },
    disconnect: {
      role: 'admin',
      who: true,
      confirm: true,
      run: (s) => Promise.all(s.targets.map((t) => t.ref.voice.disconnect())),
      say: { fr: (s, n) => `Je déconnecte ${n}.`, en: (s, n) => `Disconnecting ${n}.` },
    },
    message: {
      role: 'admin',
      channel: 'text',
      text: true,
      run: (s) => s.channel.ref.send(s.text),
      say: { fr: (s) => `J'envoie le message dans ${s.channel.name}.`, en: (s) => `Message sent to ${s.channel.name}.` },
    },
    leave: {
      role: 'admin',
      run: (s, io) => io.leave(),
      say: { fr: () => 'À plus.', en: () => 'See you.' },
    },
  },
};
