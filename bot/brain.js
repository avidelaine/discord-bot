// Bobby's brain: transcribed sentence -> intent (shortcut or local LLM) -> resolved names -> rights -> actions.
// Bobby understands French and English, and answers in the language it was spoken to in.
//
// Actions live in bot/actions/*.js, one module per theme, loaded in alphabetical order. A module exports:
//   help     { fr, en }   one line for "what can you do"
//   prompt   [lines]      action descriptions for the LLM          rules    [lines]  extra LLM rules
//   examples [lines]      'sentence -> {json}' few-shot examples    quick    [[regex, (match) => actions]] LLM-free shortcuts
//   fields   { name: schema }   extra optional step fields for the LLM schema (e.g. alias_rename's "to")
//   early(query) -> actions or null   checked first, on the raw sentence (e.g. "note que ...", which keeps its full text)
//   promptLines(ctx) -> [lines]        live context added to each LLM request (e.g. the to-do lists)
//   short    { fr, en }   a few words for "who can do what"
//   actions  { type: { role: 'prime'|'admin'|'super', who (true, or 'optional'), one, channel: 'voice'|'text'|'text?', text, confirm,
//                      list / to (to-do list names, resolved by actions/todo.js), info (read-only answer, not a right),
//                      saves (the step stores something: only then may Bobby say it noted it),
//                      normalize(action) -> fixed action or null, announce(lang), run(step, io, lang) -> undo function or nothing, say: { fr(step, names), en } } }
// Every step gets step.by = { id, name, role } (who asked) and step.ctx (server snapshot + history).
const fs = require('fs');
const path = require('path');
const { findWake } = require('./wake');
const { Say, norm, sim, llm, words } = require('./util');
const roles = require('./roles');
const { load } = require('./store');

// One JSON line per exchange in _logs/conversations.jsonl (kept out of git), reviewed to improve Bobby.
// Full text only for sentences addressed to Bobby; other speech is logged as metadata by bot.js.
const JOURNAL = process.env.JOURNAL_FILE || path.join(__dirname, '..', '_logs', 'conversations.jsonl'); // tests use a temp file
fs.mkdirSync(path.dirname(JOURNAL), { recursive: true });
const journal = (rec) => fs.appendFile(JOURNAL, `${JSON.stringify(rec)}\n`, () => {});

// A confirmation starts with a yes and has no "no" in it, however long ("oui je confirme Bobby je confirme"):
// "non, pas ok" or "ok mais attends" must never confirm a disconnect or a shutdown.
const YES = /^(?:bobby[\s,.!?:;-]*)?(?:oui|ouais|vas-y|confirme|je confirme|ok|okay|d'accord|go|yes|yeah|yep|sure|confirm)\b/i;
const NO = /\b(?:non|no|pas|nope|don't|annule|cancel|stop|attends|wait)\b/i;
const confirms = (text) => {
  const t = text.trim().replace(/[.!?…]+$/, '');
  return YES.test(t) && !NO.test(t);
};

// "Stop" while Bobby speaks (see hear). Whole sentence only: "stop la musique" is a music command.
const STOP = /^(?:(?:bobby|ok|okay|bon|non|euh|allez|hey)[\s,.!?]*)*(?:stop|arr[eê]te(?:[- ]toi)?|chut|tais[- ]toi|c['’]est bon|[çc]a suffit|merci|shut up|be quiet|quiet|enough|that['’]s enough|thanks|thank you|that['’]s fine)(?:[\s,.!?]*(?:stop|bobby|merci|thanks|c['’]est bon|[çc]a suffit))*[\s,.!?…]*$/i;

const DIR = path.join(__dirname, 'actions');
const MODULES = fs.readdirSync(DIR).filter((f) => f.endsWith('.js')).sort().map((f) => require(path.join(DIR, f)));
const ACTIONS = Object.assign({}, ...MODULES.map((m) => m.actions));
ACTIONS.undo = { role: 'prime', run() {}, say: {} }; // handled by run() itself; its role is the one of what it undoes

const SCHEMA = {
  type: 'object',
  properties: {
    actions: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          type: { enum: Object.keys(ACTIONS) },
          who: { type: 'array', items: { type: 'string' } },
          except: { type: 'array', items: { type: 'string' } },
          channel: { type: 'string' },
          text: { type: 'string' },
          ...Object.assign({}, ...MODULES.map((m) => m.fields)),
        },
        required: ['type'],
      },
    },
    say: { type: 'string' },
  },
  required: ['actions', 'say'],
};

// Fixed and first: llama-server caches this prefix, so only the tail is processed for each sentence.
// The prompt stays in French, the language it was tuned and tested in; the model handles English input fine.
const SYSTEM = `Tu es Bobby, l'assistant vocal d'un serveur Discord. Tu reçois une phrase transcrite à l'oral (parfois avec des fautes), en français ou en anglais, et le contexte du serveur. Tu réponds uniquement en JSON.

Actions possibles (champ "type") :
${MODULES.flatMap((m) => m.prompt).join('\n')}
- undo : annuler la dernière action

Ce que tu sais faire : ${require('./actions/info').capabilities('fr')}.

Règles :
- Tout se traite en local par défaut. Questions sur toi, tes capacités, tes réglages, ta voix, ton mot d'activation, les droits, les gens du serveur, la conversation en cours, ou remarque / avis / critique sur toi : action about avec la question complète dans "text".
- "réessaie" ou "try again" : refais la dernière demande de l'historique (mêmes actions). L'historique sert aussi à comprendre "il", "ça", "le dernier point".
- "stop" tout seul arrête la musique (music_stop), jamais leave. leave seulement si on dit à Bobby de partir ("pars", "quitte le salon", "tu peux partir").
- "who" contient des noms de personnes, "moi" pour celui qui parle, ou "tous" pour tout le monde dans son salon, même si la phrase est en anglais. "except" liste les personnes exclues.
- Les noms sont souvent mal transcrits : écris le nom du contexte qui ressemble le plus à ce qui a été dit.
- Plusieurs demandes dans une phrase donnent plusieurs actions, dans l'ordre.
${MODULES.flatMap((m) => m.rules).join('\n')}
- Demande impossible ou incompréhensible : "actions" vide, et "say" le dit en une phrase courte. Ne prétends jamais avoir fait, noté ou retenu quelque chose sans l'action correspondante.
- Quand il y a des actions, "say" reste vide : Bobby annonce lui-même ce qu'il fait.
- "say" est toujours dans la langue de la phrase : français si on te parle français, anglais si on te parle anglais.

Exemples :
"non annule ça" -> {"actions":[{"type":"undo"}],"say":""}
${MODULES.flatMap((m) => m.examples).join('\n')}`;

// ---------- Context ----------

// Snapshot of the server as plain objects (ref = Discord object), so the rest is testable without Discord.
function context(member, guild) {
  const nicknames = load('aliases', {}); // memberId -> [nickname], set by voice ("appelle Zyklon le Grec")
  const members = [...guild.voiceStates.cache.values()]
    .filter((v) => v.channel && v.member && !v.member.user.bot)
    .map((v) => ({
      id: v.id, name: v.member.displayName, channel: v.channel.name, ref: v.member,
      nicknames: nicknames[v.id] || [],
      aliases: [v.member.user.username, v.member.user.globalName, ...(nicknames[v.id] || [])].filter(Boolean),
    }));
  const chans = (type) => [...guild.channels.cache.values()].filter((c) => c.type === type).map((c) => ({ name: c.name, ref: c }));
  return {
    speaker: members.find((m) => m.id === member.id) || { id: member.id, name: member.displayName, ref: member },
    members,
    voiceChannels: chans(2),
    textChannels: chans(0),
  };
}

const hotwords = (ctx) => ['Bobby', ...ctx.members.flatMap((m) => [m.name, ...(m.nicknames || [])]), ...ctx.voiceChannels.map((c) => c.name)].join(' ').slice(0, 400);

// Last requests of each person (5 at most, 10 minutes), so "réessaie" and "pourquoi il était perdu ?" make sense.
const HISTORY = new Map(); // userId -> [{ at, query, reply }]
const historyOf = (id) => (HISTORY.get(id) || []).filter((h) => Date.now() - h.at < 10 * 60_000).slice(-5);
function remember(id, query, reply) {
  if (!query) return;
  HISTORY.set(id, [...historyOf(id), { at: Date.now(), query, reply: (reply || '').slice(0, 150) }].slice(-5));
}

function prompt(ctx, query, lang, history = []) {
  const byChan = {};
  for (const m of ctx.members) (byChan[m.channel] ||= []).push(m.nicknames?.length ? `${m.name} [surnoms : ${m.nicknames.join(', ')}]` : m.name);
  return [
    `Celui qui parle : ${ctx.speaker.name}, dans le salon vocal ${ctx.speaker.channel || 'aucun'}.`,
    `En vocal : ${Object.entries(byChan).map(([c, n]) => `${c} (${n.join(', ')})`).join(' ; ') || 'personne'}.`,
    `Salons vocaux : ${ctx.voiceChannels.map((c) => c.name).join(', ')}.`,
    `Salons texte : ${ctx.textChannels.map((c) => c.name).join(', ')}.`,
    `Date : ${new Date().toLocaleString('fr-FR', { dateStyle: 'full', timeStyle: 'short' })}.`,
    ...MODULES.flatMap((m) => m.promptLines?.(ctx) || []),
    ...(history.length ? [`Historique récent de ${ctx.speaker.name} (du plus ancien au plus récent) :`, ...history.map((h) => `- "${h.query}" -> Bobby : "${h.reply}"`)] : []),
    '',
    `Réponds dans "say" en ${lang === 'en' ? 'anglais' : 'français'}.`,
    `Phrase : "${query}"`,
  ].join('\n');
}

async function understand(query, ctx, lang = 'fr', history = []) {
  const out = await llm([{ role: 'system', content: SYSTEM }, { role: 'user', content: prompt(ctx, query, lang, history) }], SCHEMA);
  return guard(out, query, ctx, history);
}

// ---------- Guards on what the LLM picked (each one fixes a miss seen in the journal) ----------

const LEAVE = /\b(?:pars|partir|quitte|quitter|leave|go away|va-t'en|vas-t'en|casse-toi|bye|au revoir|à plus|a plus|y aller|go now|you can go|déconnecte-toi|sors|get out)\b/i;
// Questions about Bobby, the server, its people or the conversation are answered locally (about), never by Claude.
const ASK_LEAD = /^(?:(?:est-ce que |est ce que )?(?:tu peux|tu pourrais|peux-tu|pourrais-tu)(?: me| nous)? (?:dire|expliquer|rappeler|donner)|(?:dis|explique|raconte|donne)[- ]moi|can you tell me|tell me)\s*/i;
const SELF = /(?:^|[^\p{L}])(?:bobby|toi|tu es|t'es|es-tu|ta|ton|tes|tu (?:penses|crois|trouves|préfères|aimes)|you are|are you|your|yourself|do you think|droits?|permissions?|admins?|surnoms?|nicknames?|liste|to-?do|serveur|server|rights)(?![\p{L}])/iu;
const BACKREF = /^(?:pourquoi|comment ça|comment|qu'est-ce qui|c'était quoi|why|what happened)\b.*\b(?:il|elle|ils|elles|ça|ca|c'était|t'as|it|he|she|they)\b/i;
function aboutBobby(query, ctx, history = []) {
  const q = query.toLowerCase().replace(/[’`]/g, "'").replace(ASK_LEAD, '');
  if (SELF.test(q) || (history.length && BACKREF.test(q))) return true;
  const names = ctx.members.flatMap((m) => [m.name, ...(m.aliases || [])]).map(norm).filter((n) => n.length >= 4);
  return q.split(/[^\p{L}0-9]+/u).some((w) => w.length >= 4 && names.some((n) => sim(w, n) >= 0.85));
}

function guard(out, query, ctx, history = []) {
  const q = query.toLowerCase();
  const actions = out.actions.flatMap((a) => {
    if (a.type === 'leave' && !LEAVE.test(q)) return /\b(?:stop|arrête|arrete|tais-toi|chut)\b/.test(q) ? [{ type: 'music_stop', quiet: true }] : [];
    if (a.type === 'pc_launch' && /\b(?:ferme|quitte|close|quit|kill)\b/.test(q)) return [{ ...a, type: 'pc_close' }];
    if (a.type === 'alias_list' && !/surnom|nickname/.test(q)) return [{ type: 'todo_list', list: 'general' }];
    if (/^(?:admin_add|admin_remove|perm_restore|perm_revoke)$/.test(a.type) && /super[\s-]?admin/.test(q)) return [{ type: 'super_admin' }];
    if (a.type === 'ask' && aboutBobby(query, ctx, history)) return [{ type: 'about', text: query }];
    // "qui est admin" read as perm_show of someone called "admin": a general question about rights
    const known = (w) => { try { return ME.has(norm(w)) || Boolean(pick(w, ctx.members, 'person')); } catch { return false; } };
    if (a.type === 'perm_show' && !(a.who?.length && a.who.every(known))) return [{ type: 'about', text: query }];
    // "renomme Lucas en le Boss" read as renaming the nickname "Lucas": Lucas is a person, so it gives a nickname
    if (a.type === 'alias_rename' && !a.who?.length && a.to && !/surnom|nickname/.test(q)) {
      const named = ctx.members.find((m) => q.split(/[^\p{L}0-9]+/u).some((w) => w.length >= 3 && sim(w, m.name) >= 0.85));
      if (named) return [{ type: 'alias_add', who: [named.name], text: a.to }];
    }
    // the 4B model shortens long notes ("gérer une to-do list"): keep the whole sentence
    if (a.type === 'todo_add' && words(query) >= 12 && words(a.text) < 0.6 * words(query)) return [{ ...a, text: noteText(query) }];
    return [a];
  });
  return { ...out, actions };
}
const noteText = (query) => MODULES.map((m) => m.early?.(query)).find((x) => x?.[0]?.type === 'todo_add')?.[0].text || query;

// ---------- LLM-free shortcuts ----------

// Polite wrappers around a command ("est-ce que tu peux mute Kévin s'il te plaît").
const LEAD = /^(?:(?:est-ce que |est ce que )?(?:tu peux|tu pourrais|peux-tu|peux tu|pourrais-tu|pourrais tu)|vas-y|vas y|can you|could you|would you|please)[\s,]+/;
const TRAIL = /[\s,]+(?:stp|svp|s'il te pla[iî]t|s'il vous pla[iî]t|please)$/;

function clean(query) {
  let q = query.toLowerCase().replace(/[’`]/g, "'").replace(/[.!?…]+/g, ' ').replace(/\s+/g, ' ').trim().replace(/,$/, '');
  for (let prev = ''; prev !== q;) { prev = q; q = q.replace(LEAD, '').replace(TRAIL, '').trim(); }
  return q;
}

// Simple commands without the LLM (saves ~1 s). Anything more complex returns null and goes to the LLM.
function quick(query) {
  for (const m of MODULES) { const hit = m.early?.(query); if (hit) return hit; }
  const q = clean(query);
  // several requests, people or exclusions: leave it to the LLM
  if (/ et | puis |sauf| and | then |except|,/.test(q)) return null;
  for (const m of MODULES) {
    for (const [re, make] of m.quick) {
      const hit = q.match(re);
      if (hit) return make(hit);
    }
  }
  if (/^(?:annule(?: ça)?|undo(?: that)?)$/.test(q)) return [{ type: 'undo' }];
  return null;
}

// A sentence ending in "annule" / "laisse tomber" cancels the whole request; "annule" alone (or "non annule") is undo.
const FILLER = /^(?:non|euh|ah|bon|ok|bobby|en fait|no|oh)$/;
function cancels(query) {
  const q = clean(query).replace(/,/g, ' ');
  const m = q.match(/^(.*?)\s*\b(?:annule|laisse tomber|cancel|never mind|nevermind|forget it)$/);
  return Boolean(m) && m[1].split(/\s+/).some((w) => w && !FILLER.test(w));
}

// ---------- Name resolution ----------

const WHAT = {
  person: ['la personne', 'anyone called'],
  voice: ['le salon vocal', 'a voice channel called'],
  text: ['le salon texte', 'a text channel called'],
};

// ponytail: 0.6 threshold and 0.1 gap picked by hand; tune them on the server's real nicknames.
function pick(q, items, what) {
  const ranked = items
    .map((it) => ({ it, s: Math.max(...[it.name, ...(it.aliases || [])].map((c) => sim(q, c))) }))
    .sort((x, y) => y.s - x.s);
  const [a, b] = ranked;
  const [fr, en] = WHAT[what];
  if (!a || a.s < 0.6) throw new Say(`Je ne trouve pas ${fr} « ${q} ».`, `I can't find ${en} "${q}".`);
  if (b && b.s >= 0.6 && a.s < 1 && a.s - b.s < 0.1) {
    throw new Say(`« ${q} » : tu veux dire ${a.it.name} ou ${b.it.name} ?`, `"${q}": do you mean ${a.it.name} or ${b.it.name}?`);
  }
  return a.it;
}

const ME = new Set(['moi', 'me', 'je', 'myself', 'i']);
const ALL = new Set(['tous', 'toutlemonde', 'toutes', 'everyone', 'everybody', 'all']);
const HERE = new Set(['ici', 'here']);

function targets(step, ctx) {
  const out = [];
  for (const w of step.who || []) {
    const n = norm(w);
    if (ME.has(n)) out.push(ctx.speaker);
    // "tous" (everyone): for a move, everyone in voice on the server; otherwise, the other people in the speaker's channel.
    else if (ALL.has(n)) out.push(...ctx.members.filter((m) => (step.type === 'move' ? true : m.channel === ctx.speaker.channel && m.id !== ctx.speaker.id)));
    else out.push(pick(w, ctx.members, 'person'));
  }
  const ex = (step.except || []).map((w) => (ME.has(norm(w)) ? ctx.speaker : pick(w, ctx.members, 'person')));
  const res = [...new Set(out)].filter((m) => !ex.includes(m));
  if (!res.length) throw new Say('Je ne sais pas sur qui agir.', "I don't know who you mean.");
  return res;
}

const todo = () => require('./actions/todo');

function resolve(actions, ctx) {
  return actions.map((raw) => {
    const a = ACTIONS[raw.type]?.normalize?.(raw) || raw; // a module may fix a known LLM confusion
    const def = ACTIONS[a.type];
    if (!def) throw new Say("Je n'ai pas compris.", "Sorry, I didn't get that.");
    const step = { ...a, text: a.text || '', by: { id: ctx.speaker.id, name: ctx.speaker.name, role: ctx.role }, ctx };
    delete step.who; delete step.except;
    step.list = def.list && a.list ? todo().listOf(a.list, ctx, pick) : null;
    if (def.to) step.to = todo().listOf(a.to || 'general', ctx, pick);
    if (def.who && (def.who !== 'optional' || a.who?.length)) step.targets = targets(a, ctx);
    if (def.one && step.targets && step.targets.length !== 1) throw new Say('Une seule personne à la fois pour ça.', 'One person at a time for that.');
    if (def.channel === 'voice') {
      const q = a.channel || '';
      step.channel = HERE.has(norm(q)) ? ctx.voiceChannels.find((c) => c.name === ctx.speaker.channel) : pick(q, ctx.voiceChannels, 'voice');
      if (!step.channel) throw new Say("Tu n'es dans aucun salon vocal.", "You're not in a voice channel.");
    }
    if (def.channel === 'text' || (def.channel === 'text?' && a.channel)) step.channel = pick(a.channel || '', ctx.textChannels, 'text');
    if (def.text && !step.text) throw new Say('Il me manque le texte.', "I'm missing the text.");
    return step;
  });
}

const needsConfirm = (plan) => plan.some((s) => ACTIONS[s.type].confirm) || plan.reduce((n, s) => n + (s.targets?.length || 0), 0) >= 3;

// Highest role the plan needs; undo needs the role of what it undoes.
// One undo per person: "annule" only ever undoes your own last action.
const UNDO = new Map(); // userId -> { run, role }
const undoOf = (plan) => UNDO.get(plan[0]?.by?.id ?? '');
const needed = (plan) => plan.map((s) => (s.type === 'undo' ? undoOf(plan)?.role || 'prime' : ACTIONS[s.type].role || 'admin'))
  .reduce((a, b) => (roles.LEVEL[b] > roles.LEVEL[a] ? b : a), 'prime');

// Which role each action needs, for the startup log and the README.
const roleTable = () => Object.fromEntries(Object.entries(ACTIONS).map(([k, d]) => [k, d.role || 'admin']));

// ---------- Nothing is "noted" unless a step saved it ----------

const CLAIM = /(?:^|[^\p{L}])(?:not[ée]e?s?|retenu|enregistr[ée]e?s?|ajout[ée]e?s?|je (?:le |l'|la )?(?:note|retiens|garde en (?:t[eê]te|mémoire))|bien reçu|c'est dans la liste|noted|saved|i'?ll remember|i(?:'ve| have) (?:added|written))(?![\p{L}])/iu;
const OFFER = /(?:tu veux|veux-tu|vous voulez|voulez-vous) que je (?:l'|le |la )?(?:ajoute|note|rajoute)|want me to (?:add|note|write)/i;
const OFFER_LINE = { fr: "Tu veux que je l'ajoute à la liste ?", en: 'Do you want me to add it to the list?' };
// A reply with no saving step that claims to have noted something becomes the offer; an offer waits for a "oui".
// Only free text is checked (the LLM's own reply, or an "about" answer): Claude's answers, list readouts or
// song titles may legitimately contain "ajouté" or "saved".
function noteGuard(text, plan, lang) {
  if (plan.some((s) => s.type !== 'about')) return { text };
  if (OFFER.test(text)) return { text, offer: true };
  if (CLAIM.test(text)) return { text: OFFER_LINE[lang === 'en' ? 'en' : 'fr'], offer: true };
  return { text };
}

// ---------- What Bobby says ----------

const names = (ts, lang) => ts.map((t) => t.name).join(', ').replace(/, ([^,]*)$/, lang === 'en' ? ' and $1' : ' et $1');

const LINES = {
  fr: {
    undo: (done) => (done ? "J'annule." : "Il n'y a rien à annuler."),
    confirm: 'Tu confirmes ?',
    cancelled: "D'accord, j'annule.",
    notUnderstood: "Je n'ai pas compris.",
    noRights: "Je n'ai pas les droits Discord pour ça.",
    failed: "Ça n'a pas marché.",
    notAllowed: "Je n'ai pas le droit de te répondre.",
    forbidden: "Tu n'as pas le droit de me demander ça.",
    wrote: (where) => `Je t'envoie ça dans ${where}.`,
  },
  en: {
    undo: (done) => (done ? 'Undone.' : 'Nothing to undo.'),
    confirm: 'Confirm?',
    cancelled: 'Okay, cancelled.',
    notUnderstood: "Sorry, I didn't get that.",
    noRights: "I don't have the Discord permissions for that.",
    failed: "That didn't work.",
    notAllowed: "I'm not allowed to answer you.",
    forbidden: "You're not allowed to ask me that.",
    wrote: (where) => `Sent to ${where}.`,
  },
};

function stepSay(s, plan, lang = 'fr') {
  if (s.type === 'undo') return (lang === 'en' ? LINES.en : LINES.fr).undo(s.done ?? Boolean(undoOf(plan)));
  return ACTIONS[s.type].say[lang === 'en' ? 'en' : 'fr']?.(s, s.targets && names(s.targets, lang));
}
const describe = (plan, lang = 'fr') => plan.map((s) => stepSay(s, plan, lang)).filter(Boolean).join(' ');

// ---------- Answers in writing ----------

// "dis-moi par écrit…", "in writing" (anywhere), or "écris-moi…", "envoie-moi…", "send me…" (at the start).
// "écris dans général bonjour" is not one of them: that is the message action.
const IN_WRITING = /[\s,]*(?:par écrit|à l'écrit|a l'ecrit|in writing|by text|as a message)(?![\p{L}])[\s,]*/iu;
// Never "mets" or "envoie-le" ("mets-moi dans Gaming", "envoie-le en AFK" are moves); "écris-le" repeats the last answer.
const WRITE_ME = /^(?:(?:est-ce que |est ce que )?(?:tu peux|tu pourrais|peux-tu|can you|could you|please)[\s,]+)?((?:écris|ecris)-(?:le|la|les|ça|ca)(?:-moi)?|(?:écris|ecris|envoie)[- ](?:moi|nous)|(?:write|send|text)(?: it| that| this)?(?: to)? me|write (?:it|that) down)(?![\p{L}])[\s,:]*/iu;
const REPEAT = /^(?:écris|ecris)-|^write (?:it|that) down/i; // no request of its own: the last one, in writing
const TO_CHANNEL = /\s*(?:dans|sur|en|in|on|to) (?:le salon |the channel |the )?#?([\p{L}\d_-]+)(?: channel)?$/iu;
const RIGHTS_Q = /droit|admin|permission|rights|liste noire|blacklist/i;

// -> null, or { query (the request without its writing words; "écris-le" alone repeats the last request), channel }
function writing(query, ctx, history = []) {
  if (MODULES.some((x) => x.early?.(query))) return null; // "note que le rapport est à rendre par écrit" is a note
  const q = query.replace(/[’`]/g, "'").replace(/[.!?…]+$/, '').trim();
  const m = q.match(WRITE_ME);
  let rest;
  if (m) rest = q.slice(m[0].length);
  else if (IN_WRITING.test(q)) rest = q.replace(IN_WRITING, ' ').trim();
  else return null;
  // "envoie-moi en AFK", "envoie-nous dans Gaming": a move, not an answer in writing
  if (m && !REPEAT.test(m[1]) && /^(?:dans|en|sur|vers|to|in|into)\b/i.test(rest)) return null;
  let channel = null;
  const c = rest.match(TO_CHANNEL) || (rest ? null : q.match(TO_CHANNEL));
  const hit = c && ctx.textChannels.map((x) => ({ x, s: sim(c[1], x.name) })).sort((a, b) => b.s - a.s)[0];
  if (hit && hit.s >= 0.8) { channel = hit.x; rest = rest.replace(TO_CHANNEL, '').trim(); }
  if (!rest) return m && !REPEAT.test(m[1]) ? null : { query: history.at(-1)?.query || '', channel };
  // "écris-moi la liste" -> "donne-moi la liste", which the shortcuts know
  return { query: m ? `${/^(?:write|send|text)/i.test(m[1]) ? 'give me' : 'donne-moi'} ${rest}` : rest, channel };
}

const writingIo = (io, w) => ({ ...io, post: (c) => io.post(c, w.channel?.ref) }); // posts go to the channel asked for

// After a plan asked in writing: steps that posted by themselves (lists) set s.where; the others post their card,
// or what they would have said. Returns the short spoken line.
async function postWritten(plan, rio, lang) {
  let where = plan.find((s) => s.where)?.where;
  for (const s of plan.filter((x) => !x.where)) { const c = s.card || stepSay(s, plan, lang); if (c) where = await rio.post(c); }
  return where ? (lang === 'en' ? LINES.en : LINES.fr).wrote(where) : describe(plan, lang);
}

// ---------- Execution ----------

async function run(plan, io, lang = 'fr') {
  const undos = [];
  const who = plan[0]?.by?.id ?? '';
  for (const s of plan) {
    if (s.type === 'undo') { const u = UNDO.get(who); s.done = Boolean(u); if (u) await u.run(); continue; }
    const undo = await ACTIONS[s.type].run(s, io, lang);
    if (typeof undo === 'function') undos.push(undo);
  }
  if (undos.length) {
    UNDO.set(who, { role: needed(plan), run: () => Promise.all(undos.reverse().map((u) => u())).then(() => UNDO.delete(who)) });
  } else if (plan.some((s) => s.type === 'undo')) UNDO.delete(who);
}

// ---------- Entry point ----------

const pending = new Map(); // userId -> { until, plan? }: "Bobby" said alone, or a pending confirmation
const refused = new Map(); // userId -> last time Bobby told them it may not answer

// True while Bobby waits for this person's next sentence, which then needs no "Bobby".
const expecting = (userId) => (pending.get(userId)?.until ?? 0) > Date.now();

const planText = (plan) => plan.map((x) => `${x.type}${x.targets ? ` -> ${x.targets.map((m) => m.name).join(', ')}` : ''}${x.channel ? ` @ ${x.channel.name}` : ''}${x.text ? ` "${x.text}"` : ''}`).join(' | ');

// The wake word model upstream is a cheap, lenient filter; the transcript must still contain "Bobby",
// so a false trigger ("beau bilan") costs one transcription and never an action.
async function hear({ text, lang = 'fr', score = null, member, guild, tEnd, sttMs, io }) {
  let p = pending.get(member.id);
  if (p && Date.now() > p.until) p = null;
  const wake = findWake(text);
  const rec = { at: new Date().toISOString(), who: member.displayName, id: member.id, lang, wakeScore: score, heard: text };
  // Bobby is speaking: the person it answers stops it with a plain "stop"; an admin needs "Bobby, stop";
  // anyone else's "stop" is ignored. The voice stops, never the music. A pending "oui ?" keeps its own rules.
  const talking = io.speaking?.();
  if (talking && !p?.plan && STOP.test(text.trim())) {
    const ok = talking.id === member.id || (wake && roles.allowed(roles.roleOf(member), 'admin'));
    journal({ ...rec, outcome: ok ? 'interrupted' : 'stop_ignored', answering: talking.id });
    if (!ok) return console.log(`[stop] ignored: ${member.displayName} is not the one Bobby answers`);
    io.hush();
    return console.log(`[stop] ${member.displayName} interrupted Bobby`);
  }
  if (!wake && !p) { journal({ ...rec, outcome: 'ignored' }); return console.log(`[ignored] no "Bobby" in "${text}"`); }
  const L = lang === 'en' ? LINES.en : LINES.fr;
  const role = roles.roleOf(member);
  rec.role = role;
  if (role === 'other') {
    journal({ ...rec, outcome: 'not_allowed' });
    console.log(`[rights] ${member.displayName} (${member.id}) has no role that lets them talk to Bobby`);
    if (Date.now() - (refused.get(member.id) || 0) > 30_000) { refused.set(member.id, Date.now()); io.say(L.notAllowed, lang); }
    return;
  }
  pending.delete(member.id);
  const history = historyOf(member.id);
  // Nothing to say (e.g. "stop" with no music): stay silent.
  // Not awaited: the reply plays on while the journal is written; member.id may interrupt it.
  const say = (s, voice) => { rec.reply = s; if (s) io.say(s, lang, voice, member.id); };
  const t = { stt: sttMs };
  try {
    if (p?.plan) {
      rec.plan = planText(p.plan);
      rec.query = text;
      if (!confirms(text)) { rec.outcome = 'cancelled'; return say(L.cancelled); }
      if (!roles.allowed(role, needed(p.plan))) { rec.outcome = 'forbidden'; return say(L.forbidden); } // rights may have changed meanwhile
      rec.outcome = 'confirmed';
      const t0 = performance.now();
      const pio = p.w ? writingIo(io, p.w) : io; // asked in writing before the "oui"
      await run(p.plan, pio, lang);
      t.action = Math.round(performance.now() - t0);
      if (p.w) return say(await postWritten(p.plan, pio, lang));
      return say(describe(p.plan, lang), voiceOf(p.plan));
    }
    // "OK STOP BOBBY BOBBY STOP": the command follows the last of the repeated "Bobby".
    let query = retry((wake ? text.slice(text.indexOf(wake) + wake.length) : text).replace(/^(?:[\s,.:;!?]+|bobby\b)+/i, '').trim(), history);
    rec.query = query;
    console.log(`[query] "${query}" (${lang})`);
    io.beep();
    if (query.replace(/[^\p{L}]/gu, '').length < 2) { pending.set(member.id, { until: Date.now() + 6000 }); rec.outcome = 'waiting'; return console.log('[waiting] "Bobby" alone: next sentence is the command (6 s)'); }
    const ctx = { ...context(member, guild), role, history };
    // "dis-moi par écrit…": the request without its writing words; the answer is posted, Bobby only says where.
    const w = writing(query, ctx, history);
    if (w) {
      query = w.query;
      rec.written = w.channel?.name || io.textChannel?.name || true;
      console.log(`[written] "${query}" -> ${w.channel?.name || 'default text channel'}`);
      if (!query) { rec.outcome = 'resolve_error'; return say(L.notUnderstood); }
      if (w.channel && w.channel.name !== io.textChannel?.name && !roles.allowed(role, 'admin')) { rec.outcome = 'forbidden'; return say(L.forbidden); }
    }
    // A song title may end in a cancel word ("joue Nevermind"): playing wins.
    if (!quick(query)?.some((a) => a.type.startsWith('music_')) && cancels(query)) { rec.outcome = 'cancelled_by_speaker'; return say(L.cancelled); }
    const rio = w ? writingIo(io, w) : io;
    let t0 = performance.now();
    let plan = null;
    const fast = quick(query);
    if (fast) try { plan = resolve(fast, ctx); t.shortcut = 0; rec.intent = { via: 'shortcut', actions: fast }; console.log(`[intent] shortcut: ${JSON.stringify(fast)}`); } catch { /* name not found: let the LLM try */ }
    if (!plan) {
      const out = await understand(query, ctx, lang, history);
      t.llm = Math.round(performance.now() - t0);
      rec.intent = { via: 'llm', ...out };
      console.log(`[intent] LLM (${t.llm} ms): ${JSON.stringify(out)}`);
      if (!out.actions.length) {
        rec.outcome = 'answered';
        if (w && out.say) return say(L.wrote(await rio.post(out.say)));
        const g = noteGuard(out.say || L.notUnderstood, [], lang);
        if (g.offer) offerNote(member.id, query, ctx);
        return say(g.text);
      }
      plan = resolve(out.actions, ctx);
    }
    if (w) for (const s of plan) { s.written = true; if (s.type === 'about' && RIGHTS_Q.test(s.text)) s.type = 'rights_list'; }
    rec.plan = planText(plan);
    console.log(`[plan] ${rec.plan}`);
    const need = needed(plan);
    if (!roles.allowed(role, need)) {
      rec.outcome = 'forbidden';
      console.log(`[rights] ${member.displayName} is ${role}, "${rec.plan}" needs ${need}`);
      return say(L.forbidden);
    }
    if (needsConfirm(plan)) {
      pending.set(member.id, { until: Date.now() + 10000, plan, w });
      rec.outcome = 'confirm_asked';
      return say(`${describe(plan, lang)} ${L.confirm}`);
    }
    for (const s of plan) { const a = ACTIONS[s.type].announce?.(lang); if (a) io.say(a, lang, undefined, member.id); }
    t0 = performance.now();
    await run(plan, rio, lang);
    t.action = Math.round(performance.now() - t0);
    rec.outcome = 'done';
    if (w) return say(await postWritten(plan, rio, lang));
    const g = noteGuard(describe(plan, lang), plan, lang);
    if (g.offer) offerNote(member.id, query, ctx);
    say(g.text, voiceOf(plan));
  } catch (e) {
    if (e instanceof Say) { rec.outcome = e.code === 'LLM_DOWN' ? 'llm_down' : 'resolve_error'; rec.error = e.message; console.log(`[resolve] ${e.message}`); return say(e.in(lang)); }
    rec.outcome = 'error'; rec.error = String(e.message || e);
    console.error('[bobby]', e);
    say(e.code === 50013 ? L.noRights : L.failed);
  } finally {
    if (rec.query && rec.outcome !== 'waiting') remember(member.id, rec.query, rec.reply);
    t.total = Math.round(performance.now() - tEnd);
    rec.timing = t;
    journal(rec);
    console.log(`[timing] ${Object.entries(t).filter(([, v]) => v != null).map(([k, v]) => `${k} ${v} ms`).join(' · ')}  (total = from end of speech to reply)`);
  }
}

// "Tu veux que je l'ajoute à la liste ?": a "oui" within 15 s adds the whole sentence to the general list.
function offerNote(userId, query, ctx) {
  pending.set(userId, { until: Date.now() + 15000, plan: resolve([{ type: 'todo_add', text: noteText(query), list: 'general' }], ctx) });
}
// "Réessaie" replays the person's last request (the LLM tended to read it as undo). "Recommence" stays music replay.
const RETRY = /^(?:(?:bobby|vas-y|allez)[\s,]+)*(?:réessaie|réessaye|reessaie|essaie encore|essaye encore|refais(?:-le| le| ça)?|try again|retry)[\s.!?]*$/i;
const retry = (query, history) => (RETRY.test(query) && history.length ? history.filter((h) => !RETRY.test(h.query)).at(-1)?.query || query : query);
// Claude's answers are spoken with another voice.
const voiceOf = (plan) => plan.find((s) => s.voice)?.voice;

// Shortcut or LLM, as hear() does it, without running anything (tests).
async function intent(query, ctx, lang = 'fr', history = []) {
  query = retry(query, history);
  const fast = quick(query);
  if (fast) try { resolve(fast, ctx); return { via: 'shortcut', actions: fast, say: '' }; } catch { /* the LLM tries */ }
  return { via: 'llm', ...(await understand(query, ctx, lang, history)) };
}

module.exports = { STOP, writing, hear, intent, guard, noteGuard, aboutBobby, remember, historyOf, journal, expecting, hotwords, context, understand, resolve, quick, cancels, confirms, pick, sim, describe, run, needed, roleTable, SYSTEM };
