// Voice moderation and messages: mute, deafen, move, disconnect, post, Bobby's own messages, and Bobby leaving.
const ALL = /^(?:tout le monde|everyone|everybody)$/;
const who = (w) => [ALL.test(w) ? 'tous' : w];

// Server mute / deafen, undone by restoring each person's previous state.
const voiceSet = (key, on) => ({
  role: 'admin',
  who: true,
  echo: true, // done silently after the ack (see brain.js report)
  async run(s) {
    const ms = s.targets.map((t) => t.ref);
    const set = (m, v) => (key === 'serverMute' ? m.voice.setMute(v) : m.voice.setDeaf(v));
    const prev = ms.map((m) => [m, m.voice[key]]);
    await Promise.all(ms.map((m) => set(m, on)));
    return () => Promise.all(prev.map(([m, v]) => set(m, v)));
  },
});

// Bobby's own messages in a channel, newest first (reads up to `max` messages of history).
// Voice echoes ("🗣️ …") are skipped unless all = true. Bobby can delete its own messages without Manage Messages.
async function own(ch, { all = false, max = 300 } = {}) {
  const me = ch.client.user.id;
  const out = [];
  let before;
  for (let seen = 0; seen < max;) {
    const batch = await ch.messages.fetch({ limit: 100, ...(before && { before }) });
    if (!batch.size) break;
    out.push(...[...batch.values()].filter((m) => m.author.id === me && (all || !/^(?:🗣️|🤖)/u.test(m.content))));
    before = batch.last().id;
    seen += batch.size;
  }
  return out;
}
const chan = (s, io) => s.channel?.ref || io.textChannel;

// Where Bobby goes when nobody (but bots) is left in its channel: the channel of the person who typed /join,
// if they are in voice; otherwise it waits ('wait': music paused, leaves after 2 minutes). Ids only, testable.
// voice: [{ id, channel }] people in voice (bots excluded).
function whenAlone({ botChannel, joinerId, voice }) {
  if (voice.some((v) => v.channel === botChannel)) return 'stay';
  const joiner = voice.find((v) => v.id === joinerId);
  return joiner ? { follow: joiner.channel } : 'wait';
}

// "Appelle-moi sur Discord": a bot cannot call. Bobby says so and offers a mention instead (offer: see brain.js).
// Never "appelle-moi le Boss", which gives a nickname.
const CALL = /(?:appelle[- ]moi|m'appeler|call me|passe[- ]moi un (?:appel|coup de fil))(?: (?:sur|on|via) discord| en (?:appel|vocal|privé|mp)| au téléphone)|(?:passe|lance|fais)[- ](?:moi |nous )?un appel(?: discord)?/i;
const call = (query) => (CALL.test(query.replace(/[’`]/g, "'")) ? [{ type: 'ping', who: ['moi'], text: "📞 Tu m'as demandé de t'appeler : je ne peux pas, mais me voilà.", offer: 'call' }] : null);

module.exports = {
  early: call,
  help: {
    fr: 'couper ou rétablir le micro et le son, déplacer, ramener ou déconnecter des gens, écrire dans un salon, mentionner quelqu\'un, modifier ou supprimer mes propres messages',
    en: 'mute, deafen, move, drag or disconnect people, post in a channel, mention someone, edit or delete my own messages',
  },
  short: { fr: 'la modération et les messages', en: 'moderation and messages' },
  prompt: [
    '- mute / unmute : couper ou rétablir le micro de "who"',
    '- deafen / undeafen : couper ou rétablir le son (le casque) de "who"',
    '- move : déplacer "who" dans le salon vocal "channel"',
    '- disconnect : déconnecter "who" du vocal',
    '- message : envoyer "text" dans le salon texte "channel"',
    '- msg_edit : remplacer le texte de ton dernier message écrit par "text" ("channel" facultatif)',
    '- msg_delete : supprimer ton dernier message écrit ("channel" facultatif)',
    '- clean : supprimer tous tes propres messages d\'un salon texte ("channel" facultatif)',
    '- ping : mentionner (notifier, arrobase, @, tag, ping) "who" dans le salon texte "channel" (facultatif), avec le message "text" (facultatif)',
    '- leave : Bobby quitte le salon vocal',
  ],
  rules: [
    '- "virer", "dégager" ou "kicker" une personne nommée du vocal, c\'est disconnect. Si on dit à Bobby lui-même de partir, c\'est leave.',
    '- "dé-mute", "démute" ou "des mute" (mal transcrit), c\'est unmute. "drag" ou "draguer" quelqu\'un, c\'est le déplacer (move) dans le salon de celui qui parle.',
    '- "couper" quelqu\'un sans préciser, c\'est mute (le micro seulement).',
    '- "nettoie" un salon ou "supprime les messages" d\'un salon : clean (tu ne supprimes que tes propres messages).',
    '- Bobby, c\'est toi : tu ne peux pas te muter ("actions" vide, dis-le dans "say").',
    '- "ici" désigne le salon vocal de celui qui parle : mets son nom exact dans "channel".',
    '- Tu ne supprimes que tes propres messages : supprimer ceux des autres ("mes messages", "ceux que j\'ai mis") n\'est pas faisable.',
    '- Un salon texte n\'est jamais la destination d\'un move : "arrobase-moi dans X", "ping-moi sur X", c\'est ping.',
    '- Tu ne peux pas passer d\'appel Discord : propose de mentionner la personne (ping) à la place.',
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
    '"fais un clean-up du salon memes" -> {"actions":[{"type":"clean","channel":"memes"}],"say":""}',
    '"corrige ton dernier message en : rendez-vous à 21 h" -> {"actions":[{"type":"msg_edit","text":"rendez-vous à 21 h"}],"say":""}',
    '"arrobase-moi dans annonces pour me rappeler de lancer Apex" -> {"actions":[{"type":"ping","who":["moi"],"channel":"annonces","text":"pense à lancer Apex"}],"say":""}',
    '"supprime les messages que j\'ai mis dans memes" -> {"analyse":{"genre":"ordre","faisable":false,"pourquoi":"je ne supprime que mes propres messages"},"actions":[],"say":"Je ne peux supprimer que mes propres messages."}',
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
    [/^(?:déplace|mets|envoie|bouge) (?!la musique |le pc |l'ordi|mon pc |pause |le son |le volume |du |de la |le dernier |le point |ça |ca )(.+?) (?:dans|en|sur|vers) (?:le salon |la salle (?:de )?)?(.+)$/, (m) => [{ type: 'move', who: who(m[1]), channel: m[2] }]],
    [/^mets[- ]moi (?:dans|en) (?:le salon |la salle (?:de )?)?(.+)$/, (m) => [{ type: 'move', who: ['moi'], channel: m[1] }]],
    [/^(?:move|put|send|drag) (.+?) (?:to|in|into) (?:the )?(.+?)(?: channel)?$/, (m) => [{ type: 'move', who: who(m[1]), channel: m[2] }]],
    [/^(?:drag|draguer|drague|ramène) (.+?)(?: ici| here)?$/, (m) => [{ type: 'move', who: who(m[1]), channel: 'ici' }]],
    [/^(?:pars|partir|va-t'en|vas-t'en|quitte le (?:salon|vocal)(?: vocal)?|tu peux partir|leave|go away|leave the (?:channel|voice channel))$/, () => [{ type: 'leave' }]],
    [/^(?:nettoie|clean(?: up)?|fais (?:le |un )?(?:clean-?up|ménage)(?: de| du| dans)?)(?: le salon| the channel| salon)? ?#?(.*)$/, (m) => [{ type: 'clean', ...(m[1] && { channel: m[1] }) }]],
    [/^(?:supprime|efface|delete) ton dernier message$/, () => [{ type: 'msg_delete' }]],
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
      echo: true,
      // A move that includes the speaker ("déplace-nous", "mets-moi", "tout le monde") takes Bobby along.
      async run(s, io) {
        const ms = s.targets.map((t) => t.ref);
        const prev = ms.map((m) => [m, m.voice.channel]);
        await Promise.all(ms.map((m) => m.voice.setChannel(s.channel.ref)));
        const back = s.targets.some((t) => t.id === s.by?.id) ? io.follow?.(s.channel.ref) : null;
        return () => Promise.all(prev.map(([m, c]) => m.voice.setChannel(c))).then(() => back && io.follow(back));
      },
      say: { fr: (s, n) => `Je déplace ${n} dans ${s.channel.name}.`, en: (s, n) => `Moving ${n} to ${s.channel.name}.` },
    },
    disconnect: {
      role: 'admin',
      who: true,
      confirm: true,
      echo: true,
      run: (s) => Promise.all(s.targets.map((t) => t.ref.voice.disconnect())),
      say: { fr: (s, n) => `Je déconnecte ${n}.`, en: (s, n) => `Disconnecting ${n}.` },
    },
    message: {
      role: 'admin',
      channel: 'text',
      text: true,
      echo: true,
      run: (s) => s.channel.ref.send(s.text),
      say: { fr: (s) => `J'envoie le message dans ${s.channel.name}.`, en: (s) => `Message sent to ${s.channel.name}.` },
    },
    msg_edit: {
      role: 'admin', channel: 'text?', text: true, echo: (s) => s.found,
      async run(s, io) { const [m] = await own(chan(s, io)); s.found = Boolean(m); if (m) await m.edit(s.text); },
      say: { fr: (s) => (s.found ? 'Message modifié.' : "Je n'ai pas de message à modifier ici."), en: (s) => (s.found ? 'Message edited.' : 'I have no message to edit here.') },
    },
    msg_delete: {
      role: 'admin', channel: 'text?', echo: (s) => s.found,
      async run(s, io) { const [m] = await own(chan(s, io)); s.found = Boolean(m); if (m) await m.delete(); },
      say: { fr: (s) => (s.found ? 'Message supprimé.' : "Je n'ai pas de message à supprimer ici."), en: (s) => (s.found ? 'Message deleted.' : 'I have no message to delete here.') },
    },
    // Only Bobby's own messages: deleting other people's would need Manage Messages, which Bobby does not have.
    clean: {
      role: 'admin', channel: 'text?', confirm: true,
      async run(s, io) {
        const ch = chan(s, io);
        const mine = await own(ch, { all: true, max: 1000 });
        s.count = mine.length;
        s.where = ch.name;
        // one by one, in the background (rate limits): the spoken reply does not wait
        (async () => { for (const m of mine) await m.delete().catch(() => {}); console.log(`[clean] ${mine.length} message(s) deleted in ${ch.name}`); })();
      },
      say: {
        fr: (s) => (s.where ? (s.count ? `Je supprime mes ${s.count} messages dans ${s.where}.` : `Je n'ai aucun message dans ${s.where}.`) : `Je nettoie ${s.channel?.name || 'le salon'}.`),
        en: (s) => (s.where ? (s.count ? `Deleting my ${s.count} messages in ${s.where}.` : `I have no messages in ${s.where}.`) : `Cleaning ${s.channel?.name || 'the channel'}.`),
      },
    },
    // Mentions people in a text channel: users only, never @everyone / @here (allowedMentions lists the users).
    ping: {
      role: 'admin', who: true, channel: 'text?', echo: true, // admin, like posting a message
      async run(s, io) {
        const ids = s.targets.map((t) => t.id);
        await chan(s, io).send({ content: `${ids.map((id) => `<@${id}>`).join(' ')} ${s.text.replace(/@(everyone|here)/gi, '$1')}`.trim(), allowedMentions: { users: ids } });
      },
      say: { fr: (s, n) => `Je mentionne ${n} dans ${s.channel?.name || 'le salon'}.`, en: (s, n) => `Mentioning ${n} in ${s.channel?.name || 'the channel'}.` },
    },
    leave: {
      role: 'admin',
      run: (s, io) => io.leave(),
      say: { fr: () => 'À plus.', en: () => 'See you.' },
    },
  },
  whenAlone,
};
