// Questions answered locally, open to Prime: the time, what Bobby can do, and anything about Bobby itself
// (rights, people, voice, wake word, the conversation), from facts rebuilt from live state at each question.
const fs = require('fs');
const roles = require('../roles');
const { llm, talk } = require('../util');
const { embed, COLOR } = require('../format');

const modules = () => fs.readdirSync(__dirname).filter((f) => f.endsWith('.js')).sort().map((f) => require(`./${f}`));

// Each action module describes itself in "help"; the list follows whatever modules exist.
const capabilities = (lang) => modules().map((m) => m.help?.[lang]).filter(Boolean).join(', ');

// Lowest role that can use a module, read-only answers ("info" actions) aside.
const moduleRole = (m) => Object.values(m.actions || {}).filter((a) => !a.info).map((a) => a.role || 'admin')
  .sort((a, b) => roles.LEVEL[a] - roles.LEVEL[b])[0];
// What a role may do, from the roles the action modules declare: "la musique, les listes de tâches, ..."
const abilities = (role, lang = 'fr') => modules().filter((m) => m.short && moduleRole(m) && roles.LEVEL[moduleRole(m)] <= roles.LEVEL[role])
  .map((m) => m.short[lang]).join(', ');

// "Ce que je sais faire" as an embed, one field per role (what each role adds).
function capabilitiesCard(lang = 'fr') {
  const en = lang === 'en';
  const names = { prime: `Prime (${en ? 'role' : 'rôle'} ${roles.people().primeRole})`, admin: en ? 'Admin (in addition)' : 'Admin (en plus)', super: en ? 'Super admin (everything)' : 'Super admin (tout)' };
  const fields = Object.keys(names).map((r) => ({ name: names[r], value: modules().filter((m) => m.help && moduleRole(m) === r).map((m) => `• ${m.help[lang] || m.help.fr}`).join('\n') || '-' }));
  return { embeds: embed({ title: en ? '🤖 What I can do' : '🤖 Ce que je sais faire', color: COLOR.info, description: en ? 'Say « Bobby, … » in voice.' : 'Dis « Bobby, … » en vocal.', fields }) };
}

const ROLE_NAME = { super: 'super admin', admin: 'admin', prime: 'Prime', other: 'sans rôle' };

// Everything Bobby knows about itself, rebuilt at each question: nothing here goes stale when rights change.
async function facts(s, io) {
  const ctx = s.ctx || { members: [], speaker: {} };
  const name = async (id) => ctx.members.find((m) => m.id === id)?.name || (io?.memberName ? await io.memberName(id) : id);
  const names = async (ids) => (await Promise.all(ids.map(name))).join(', ') || 'personne';
  const p = roles.people();
  const nicknames = require('../store').load('aliases', {});
  const nick = (await Promise.all(Object.entries(nicknames).filter(([, l]) => l.length).map(async ([id, l]) => `${await name(id)} = ${l.join(', ')}`))).join(' ; ');
  const todo = require('./todo').items().filter((it) => !it.done);
  return [
    "Tu es Bobby, l'assistant vocal du serveur Discord. Tout tourne en local sur le PC du super admin, gratuitement.",
    "Mot d'activation : « Bobby », repéré par un petit modèle openWakeWord entraîné sur des voix de synthèse et 20 enregistrements du super admin. Il laisse passer environ 9 « Bobby » sur 10 : pas encore parfait, on prévoit de l'améliorer avec plus d'enregistrements.",
    'Oreilles : Whisper large-v3-turbo (français et anglais). Cerveau : Qwen3-4B en local. Voix : Piper (siwis en français, lessac en anglais). Les questions de culture générale partent à Claude Haiku, qui répond avec une autre voix (tom en français, ryan en anglais). Tout le reste est traité en local.',
    `Ce que tu sais faire : ${capabilities('fr')}.`,
    `Rôles : Prime (rôle Discord « ${p.primeRole} ») peut : ${abilities('prime')}. Admin peut en plus : ${abilities('admin')}. Super admin peut tout : ${abilities('super')}. Les autres n'ont aucun droit, tu leur dis que tu n'as pas le droit de leur répondre.`,
    `Super admin : ${await names(p.supers)}. Il ne change qu'à la main dans le fichier .env, jamais à la voix. Admins : ${await names(p.admins)}. Privés de droits (liste noire) : ${await names(p.blacklist)}.`,
    `Listes de tâches : une liste générale partagée et une liste perso par personne. Chacun ajoute à la générale et à la sienne et gère ses propres points ; le super admin gère tout. Liste générale : ${todo.filter((it) => it.list === 'general').length} points à faire.`,
    `Surnoms : ${nick || 'aucun'}.`,
    `Celui qui te parle : ${s.by?.name || '?'} (${ROLE_NAME[s.by?.role] || '?'}).`,
    ...(ctx.history?.length ? ['Conversation récente avec toi (tous ceux qui te parlent) :', ...talk(ctx.history)] : []),
  ].join('\n');
}

async function aboutMe(s, io, lang) {
  const system = `${await facts(s, io)}

Réponds à la question ou à la remarque sur toi en une ou deux phrases courtes, à dire à voix haute, en ${lang === 'en' ? 'anglais' : 'français'}, à partir de ces faits uniquement. Si (et seulement si) on te fait une critique ou une suggestion d'amélioration, pas une question, remercie et termine par « Tu veux que je l'ajoute à la liste ? » (en anglais : « Do you want me to add it to the list? »). Ne dis jamais que tu as noté ou retenu quelque chose. Réponds en JSON {"say": "..."}.`;
  const out = await llm([{ role: 'system', content: system }, { role: 'user', content: s.text }], { type: 'object', properties: { say: { type: 'string' } }, required: ['say'] }, 200);
  // ponytail: a question never gets the "add it to the list?" offer, whatever the model thought (2026-11-13).
  const question = /\?\s*$|^(?:pourquoi|comment|est-ce|qu(?:'|e |oi|el|i )|où|quand|combien|why|how|what|who|when|where|do |does |did |can |are |is )/i.test(s.text.trim());
  return question ? out.say.replace(/\s*(?:Tu veux que je l'ajoute à la liste|Do you want me to add it to the list)\s*\?\s*$/i, '') : out.say;
}

const now = (lang) => new Date().toLocaleString(lang === 'en' ? 'en-GB' : 'fr-FR', { weekday: 'long', day: 'numeric', month: 'long', hour: '2-digit', minute: '2-digit' });

module.exports = {
  help: { fr: "dire l'heure, venir dans ton salon vocal et répondre aux questions sur moi", en: 'tell the time, come to your voice channel and answer questions about me' },
  short: { fr: "l'heure, me faire venir et les questions sur moi", en: 'the time, calling me over and questions about me' },
  prompt: [
    '- time : dire la date et l\'heure',
    '- help : dire ce que tu sais faire (lister tes fonctionnalités, ce qu\'on t\'a implémenté)',
    '- come : Bobby (toi) vient dans le salon vocal "channel" ("ici" = celui de celui qui parle)',
    '- about : question ou remarque sur Bobby (toi), tes réglages, ta voix, ton mot d\'activation, qui a quels droits, les gens du serveur, ou la conversation en cours ; "text" = la phrase complète',
  ],
  rules: ['- "viens", "rejoins-nous", "come here", "déplace-toi dans X" : come (Bobby vient seul). Déplacer les autres : move.'],
  examples: [
    '"il est quelle heure" -> {"actions":[{"type":"time"}],"say":""}',
    '"qu\'est-ce que tu sais faire" -> {"actions":[{"type":"help"}],"say":""}',
    '"liste-moi tes fonctionnalités" -> {"actions":[{"type":"help"}],"say":""}',
    '"viens dans Gaming" -> {"actions":[{"type":"come","channel":"Gaming"}],"say":""}',
    '"rejoins-nous" -> {"actions":[{"type":"come","channel":"Général"}],"say":""}',
    '"how are you doing" -> {"actions":[],"say":"I\'m doing great, thanks!"}',
    '"t\'es qui toi" -> {"actions":[{"type":"about","text":"t\'es qui toi"}],"say":""}',
    '"est-ce que tu es meilleur que Siri" -> {"actions":[{"type":"about","text":"est-ce que tu es meilleur que Siri"}],"say":""}',
    '"ta voix est bizarre" -> {"actions":[{"type":"about","text":"ta voix est bizarre"}],"say":""}',
    '"pourquoi tu n\'as pas compris tout à l\'heure" -> {"actions":[{"type":"about","text":"pourquoi tu n\'as pas compris tout à l\'heure"}],"say":""}',
  ],
  quick: [
    [/^(?:quelle heure (?:est-il|il est|est il)|il est quelle heure|what time is it|what's the time)$/, () => [{ type: 'time' }]],
    [/^(?:viens|ram[eè]ne-toi|rejoins(?:[- ](?:nous|moi))?|come(?: over)?|join (?:us|me))(?: (?:ici|here|là|la))?$/, () => [{ type: 'come', channel: 'ici' }]],
    [/^(?:viens|rejoins(?:[- ](?:nous|moi))?|déplace-toi|come|join(?: us| me)?) (?:dans|en|sur|to|in|into|over to) (?:le salon |la salle (?:de )?|the )?(.+?)(?: channel)?$/, (m) => [{ type: 'come', channel: m[1] }]],
    [/\blist\w* (?:moi )?(?:tes |les |your )?(?:features|fonctionnalit|commandes|capacit|commands)|ce que tu sais faire|que sais-tu faire|tu sais faire quoi|what can you do/, () => [{ type: 'help' }]],
  ],
  actions: {
    time: {
      role: 'prime',
      run() {},
      say: { fr: () => `Nous sommes ${now('fr')}.`, en: () => `It's ${now('en')}.` },
    },
    help: {
      role: 'prime',
      run(s, io, lang) { s.card = capabilitiesCard(lang); }, // posted only when asked in writing
      say: { fr: () => `Je sais ${capabilities('fr')}.`, en: () => `I can ${capabilities('en')}.` },
    },
    // Bobby alone comes to a channel ("viens dans PRIME", "rejoins-nous"); moving other people stays admin.
    come: {
      role: 'prime',
      channel: 'voice',
      echo: true, // done silently after the ack (see brain.js report)
      normalize: (a) => (a.channel ? null : { ...a, channel: 'ici' }),
      run(s, io) {
        const back = io.follow?.(s.channel.ref);
        return back ? () => io.follow(back) : undefined;
      },
      say: { fr: (s) => `J'arrive dans ${s.channel.name}.`, en: (s) => `Coming to ${s.channel.name}.` },
    },
    about: {
      role: 'prime', text: true,
      async run(s, io, lang) { s.answer = await aboutMe(s, io, lang); },
      say: { fr: (s) => s.answer, en: (s) => s.answer },
    },
  },
  capabilities,
  capabilitiesCard,
  abilities,
  facts,
  ROLE_NAME,
};
