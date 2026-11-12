// Questions answered locally, open to Prime: the time, what Bobby can do, and anything about Bobby itself
// (rights, people, voice, wake word, the conversation), from facts rebuilt from live state at each question.
const fs = require('fs');
const roles = require('../roles');
const { llm } = require('../util');

const modules = () => fs.readdirSync(__dirname).filter((f) => f.endsWith('.js')).sort().map((f) => require(`./${f}`));

// Each action module describes itself in "help"; the list follows whatever modules exist.
const capabilities = (lang) => modules().map((m) => m.help?.[lang]).filter(Boolean).join(', ');

// Lowest role that can use a module, read-only answers ("info" actions) aside.
const moduleRole = (m) => Object.values(m.actions || {}).filter((a) => !a.info).map((a) => a.role || 'admin')
  .sort((a, b) => roles.LEVEL[a] - roles.LEVEL[b])[0];
// What a role may do, from the roles the action modules declare: "la musique, les listes de tâches, ..."
const abilities = (role, lang = 'fr') => modules().filter((m) => m.short && moduleRole(m) && roles.LEVEL[moduleRole(m)] <= roles.LEVEL[role])
  .map((m) => m.short[lang]).join(', ');

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
    ...(ctx.history?.length ? ['Conversation récente avec lui :', ...ctx.history.map((h) => `- "${h.query}" -> toi : "${h.reply}"`)] : []),
  ].join('\n');
}

async function aboutMe(s, io, lang) {
  const system = `${await facts(s, io)}

Réponds à la question ou à la remarque sur toi en une ou deux phrases courtes, à dire à voix haute, en ${lang === 'en' ? 'anglais' : 'français'}, à partir de ces faits uniquement. Si on te fait une remarque, une critique ou une suggestion, remercie et termine par « Tu veux que je l'ajoute à la liste ? » (en anglais : « Do you want me to add it to the list? »). Ne dis jamais que tu as noté ou retenu quelque chose. Réponds en JSON {"say": "..."}.`;
  const out = await llm([{ role: 'system', content: system }, { role: 'user', content: s.text }], { type: 'object', properties: { say: { type: 'string' } }, required: ['say'] }, 200);
  return out.say;
}

const now = (lang) => new Date().toLocaleString(lang === 'en' ? 'en-GB' : 'fr-FR', { weekday: 'long', day: 'numeric', month: 'long', hour: '2-digit', minute: '2-digit' });

module.exports = {
  help: { fr: "dire l'heure et répondre aux questions sur moi", en: 'tell the time and answer questions about me' },
  short: { fr: "l'heure et les questions sur moi", en: 'the time and questions about me' },
  prompt: [
    '- time : dire la date et l\'heure',
    '- help : dire ce que tu sais faire (lister tes fonctionnalités, ce qu\'on t\'a implémenté)',
    '- about : question ou remarque sur Bobby (toi), tes réglages, ta voix, ton mot d\'activation, qui a quels droits, les gens du serveur, ou la conversation en cours ; "text" = la phrase complète',
  ],
  rules: [],
  examples: [
    '"il est quelle heure" -> {"actions":[{"type":"time"}],"say":""}',
    '"qu\'est-ce que tu sais faire" -> {"actions":[{"type":"help"}],"say":""}',
    '"liste-moi tes fonctionnalités" -> {"actions":[{"type":"help"}],"say":""}',
    '"how are you doing" -> {"actions":[],"say":"I\'m doing great, thanks!"}',
    '"t\'es qui toi" -> {"actions":[{"type":"about","text":"t\'es qui toi"}],"say":""}',
    '"est-ce que tu es meilleur que Siri" -> {"actions":[{"type":"about","text":"est-ce que tu es meilleur que Siri"}],"say":""}',
    '"ta voix est bizarre" -> {"actions":[{"type":"about","text":"ta voix est bizarre"}],"say":""}',
    '"pourquoi tu n\'as pas compris tout à l\'heure" -> {"actions":[{"type":"about","text":"pourquoi tu n\'as pas compris tout à l\'heure"}],"say":""}',
  ],
  quick: [
    [/^(?:quelle heure (?:est-il|il est|est il)|il est quelle heure|what time is it|what's the time)$/, () => [{ type: 'time' }]],
    [/\blist\w* (?:moi )?(?:tes |les |your )?(?:features|fonctionnalit|commandes|capacit|commands)|qu'est-ce que tu sais faire|que sais-tu faire|tu sais faire quoi|what can you do/, () => [{ type: 'help' }]],
  ],
  actions: {
    time: {
      role: 'prime',
      run() {},
      say: { fr: () => `Nous sommes ${now('fr')}.`, en: () => `It's ${now('en')}.` },
    },
    help: {
      role: 'prime',
      run() {},
      say: { fr: () => `Je sais ${capabilities('fr')}.`, en: () => `I can ${capabilities('en')}.` },
    },
    about: {
      role: 'prime', text: true,
      async run(s, io, lang) { s.answer = await aboutMe(s, io, lang); },
      say: { fr: (s) => s.answer, en: (s) => s.answer },
    },
  },
  capabilities,
  abilities,
  facts,
  ROLE_NAME,
};
