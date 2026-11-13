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
const { Say, norm, sim, llm, words, talk } = require('./util');
const roles = require('./roles');
const { load } = require('./store');
const claude = require('./actions/ask');

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
ACTIONS.undo = { role: 'prime', run() {}, say: {}, echo: (s) => s.done }; // handled by run() itself; its role is the one of what it undoes

// Reasoning first: what kind of request it is and whether Bobby can do it, then the actions. ~10 more tokens.
const KINDS = ['ordre', 'capacite', 'correction', 'discussion'];
const SCHEMA = {
  type: 'object',
  properties: {
    analyse: {
      type: 'object',
      properties: {
        genre: { enum: KINDS },
        faisable: { type: 'boolean' },
        pourquoi: { type: 'string', maxLength: 80 },
      },
      required: ['genre', 'faisable'],
    },
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
    say: { type: 'string', maxLength: 160 }, // one short spoken sentence; also keeps the JSON under max_tokens
    ecrit: { type: 'boolean' },
  },
  required: ['analyse', 'actions', 'say'],
};

// Fixed and first: llama-server caches this prefix, so only the tail is processed for each sentence.
// The prompt stays in French, the language it was tuned and tested in; the model handles English input fine.
const SYSTEM = `Tu es Bobby, l'assistant vocal d'un serveur Discord. Tu reçois une phrase transcrite à l'oral (parfois avec des fautes), en français ou en anglais, et le contexte du serveur. Tu réponds uniquement en JSON.

Actions possibles (champ "type") :
${MODULES.flatMap((m) => m.prompt).join('\n')}
- undo : annuler la dernière action

Ce que tu sais faire : ${require('./actions/info').capabilities('fr')}.

Réfléchis d'abord dans "analyse" :
- "genre" : ordre (le cas normal : on te demande d'agir ou de répondre, y compris toute question sur toi, ton avis, tes fonctionnalités ou le monde ; "peux-tu…", "tu peux…", "est-ce que tu peux…" sont des ordres polis), capacite (seulement quand on demande si tu es capable de faire une action sans te demander de la faire : "es-tu capable de…", "sais-tu + verbe", ou tes limites : "quelle est ta plage de volume"), correction (on corrige ta dernière réponse ou action : "non pas ça", "pas lui", "je voulais dire…", "j'aurais voulu…"), discussion (salutation, politesse, bavardage).
- "faisable" : true dès qu'une action de la liste le fait (Discord, musique, volume, PC, Spotify, GPU, listes, surnoms, droits, questions à Claude…). false seulement si aucune action ne le couvre (passer un appel, supprimer les messages des autres, bannir, crier) : alors "actions" vide, la raison en quelques mots dans "pourquoi", et "say" le dit en une phrase courte, sans jamais choisir une action voisine.
- capacite : "actions" = l'action qui le ferait (elle n'est exécutée qu'après un oui), "say" = la réponse courte ("Oui, je peux.", "Non, …").
- correction : refais la dernière demande de la conversation récente en appliquant la correction (mêmes actions, corrigées).
- La conversation récente (tous ceux qui parlent à Bobby) sert à comprendre "ça", "il", "le dernier", "refais", "pareil pour lui", "non pas ça".
- "ecrit" : true seulement si on veut la réponse écrite dans le salon texte (par écrit, dans le chat, en tableau, en liste mise en forme).

Règles :
- Tout se traite en local par défaut. Questions sur toi, tes capacités, tes réglages, ta voix, ton mot d'activation, les droits, les gens du serveur, la conversation en cours, ou remarque / avis / critique sur toi : action about avec la question complète dans "text".
- "réessaie" ou "try again" : refais la dernière demande de la conversation (mêmes actions).
- "stop" tout seul arrête la musique (music_stop), jamais leave. leave seulement si on dit à Bobby de partir ("pars", "quitte le salon", "tu peux partir").
- "who" contient des noms de personnes, "moi" pour celui qui parle, ou "tous" pour tout le monde dans son salon, même si la phrase est en anglais. "except" liste les personnes exclues.
- Les noms sont souvent mal transcrits : écris le nom du contexte qui ressemble le plus à ce qui a été dit.
- Plusieurs demandes dans une phrase donnent plusieurs actions, dans l'ordre.
${MODULES.flatMap((m) => m.rules).join('\n')}
- Demande impossible ou incompréhensible : "actions" vide, et "say" le dit en une phrase courte. Ne prétends jamais avoir fait, noté ou retenu quelque chose sans l'action correspondante.
- Quand il y a des actions (hors capacite), "say" reste vide : Bobby s'en occupe sans commenter.
- "say" : une seule phrase courte, comme un collègue (pas de "je suis un assistant vocal"), toujours dans la langue de la phrase : français si on te parle français, anglais si on te parle anglais.

Exemples :
"non annule ça" -> {"analyse":{"genre":"correction","faisable":true},"actions":[{"type":"undo"}],"say":""}
"es-tu capable de couper le micro de Lucas" -> {"analyse":{"genre":"capacite","faisable":true},"actions":[{"type":"mute","who":["Lucas"]}],"say":"Oui, je peux."}
"est-ce que tu peux monter le volume à 300" -> {"analyse":{"genre":"ordre","faisable":true},"actions":[{"type":"music_volume","text":"300"}],"say":""}
"es-tu capable de supprimer mes messages" -> {"analyse":{"genre":"capacite","faisable":false,"pourquoi":"je ne supprime que mes propres messages"},"actions":[],"say":"Non, je ne peux supprimer que mes propres messages."}
"est-ce que tu sais crier" -> {"analyse":{"genre":"capacite","faisable":false,"pourquoi":"ma voix a un volume fixe"},"actions":[],"say":"Non, ma voix garde toujours le même volume."}
${MODULES.flatMap((m) => m.examples).join('\n')}`; // older examples have no "analyse": the JSON grammar adds it (writing it in each cost 1100 tokens)

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

// The conversation of everyone who talks to Bobby (one voice channel at a time), so "ça", "le dernier", "refais" or
// "non pas ça" make sense whoever said what before: last 10 exchanges within 30 minutes, with what Bobby did and
// answered. Rebuilt from the journal at startup, so a restart loses nothing.
const HISTORY_MAX = 10, HISTORY_MS = 30 * 60_000;
const NOT_EXCHANGES = new Set(['waiting', 'ignored', 'skipped', 'not_allowed', 'stop_ignored', 'interrupted']);
let CONVO = []; // [{ at, id, who, query, did, reply }]
const conversation = () => CONVO.filter((h) => Date.now() - h.at < HISTORY_MS).slice(-HISTORY_MAX);
// Only what really ran counts as done (a plan offered, refused or awaiting a yes is in the reply, never in "did").
const DID = new Set(["done", "confirmed"]);
function remember(rec) {
  if (!rec.query || NOT_EXCHANGES.has(rec.outcome)) return;
  CONVO = [...conversation(), { at: Date.parse(rec.at) || Date.now(), id: rec.id, who: rec.who, query: rec.query, did: DID.has(rec.outcome) ? rec.plan || '' : '', ...(rec.confirm && { confirm: true }), reply: (rec.reply || '').slice(0, 200) }].slice(-HISTORY_MAX);
}
// ponytail: reads the journal's last 256 KB, plenty for 10 exchanges among the "skipped" lines of other speech.
function reload() {
  CONVO = [];
  try {
    const fd = fs.openSync(JOURNAL, 'r');
    const size = fs.fstatSync(fd).size, n = Math.min(size, 256 * 1024), buf = Buffer.alloc(n);
    fs.readSync(fd, buf, 0, n, size - n);
    fs.closeSync(fd);
    for (const line of buf.toString('utf8').split('\n').slice(size > n ? 1 : 0)) { try { remember(JSON.parse(line)); } catch { /* partial line */ } }
  } catch { /* no journal yet */ }
  return CONVO.length;
}
reload();
const mineOf = (history, id) => history.filter((h) => !h.id || h.id === id); // "réessaie", "écris-le": the speaker's own last request

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
    ...(history.length ? ['Conversation récente avec Bobby (du plus ancien au plus récent) :', ...talk(history)] : []),
    '',
    `Réponds dans "say" en ${lang === 'en' ? 'anglais' : 'français'}.`,
    `Phrase : "${query}"`,
  ].join('\n');
}

async function understand(query, ctx, lang = 'fr', history = []) {
  const out = await llm([{ role: 'system', content: SYSTEM }, { role: 'user', content: prompt(ctx, query, lang, history) }], SCHEMA, 400);
  return guard(out, query, ctx, history, lang);
}

// The 4B model is lost: an order it calls feasible, with no action and nothing to say. (A self-reported "sure" flag
// was tried: it said "unsure" on 10 of 95 plain commands like "mute Kevin", each worth a 5-10 s Claude call.)
const unsure = (out) => out.analyse.genre === 'ordre' && out.analyse.faisable && !out.actions.length && !out.say;

// Fallback brain for admins: when the 4B model is unsure, Claude reads the same prompt and picks among the same
// actions (5-10 s). Its answer goes through the same guards; any failure keeps the local answer.
async function rethink(out, query, ctx, lang, history) {
  try {
    const better = await claude.think(SYSTEM, prompt(ctx, query, lang, history));
    if (!Array.isArray(better.actions) || better.actions.some((a) => !ACTIONS[a?.type])) throw new Error(`bad plan ${JSON.stringify(better).slice(0, 200)}`);
    return { ...guard({ say: '', ...better, analyse: { genre: 'ordre', faisable: true, ...better.analyse } }, query, ctx, history, lang), via: 'claude' };
  } catch (e) { console.error(`[claude] fallback brain failed: ${e.message}`); return out; }
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

// "es-tu capable de…", "quelle est ta plage de volume": a question about what Bobby can do, never an order
// ("peux-tu…" stays a polite order). Backs up the model's own "genre".
const CAPABLE = /^(?:(?:bobby|alors|et|ok|euh)[\s,]+)*(?:(?:est-ce que |est ce que )?(?:tu es|t'es|tu serais) capable|es-tu capable|serais-tu capable|(?:est-ce que |est ce que )?tu sais (?!faire quoi|quoi)|sais-tu |are you (?:able|capable)|is it possible for you)|\b(?:quelle?s? (?:est|sont) (?:ta|tes) (?:range|plage|limites?|capacités?)|what(?:'s| is) your (?:range|limit))/i;
const FORM_FIX = /j'aurais (?:voulu|préféré|aimé)|tu (?:aurais dû|es censé|étais censé)|je voulais (?:que|plutôt)|plutôt (?:en|sous forme)|i(?:'d| would) have (?:liked|wanted)|i wanted/i;
const RESTORE =/\b(?:rend|remet|rétabl|retabl|redonn|rallum|réentend|reentend|réactiv|dé-?mute|démute|des mute|de-?mute|unmute|undeafen|back|give|restore)|peut (?:re)?parler|peut (?:ré)?entendre/i;
const POLITE_ORDER =/^(?:(?:bobby|alors|et|ok|euh)[\s,]+)*(?:(?:est-ce que |est ce que )?(?:tu peux|tu pourrais)|peux-tu|pourrais-tu|can you|could you)\b/i;
const WRITE_HINT = /tableau|par écrit|dans le chat|format|mis(?:e)? en forme|à puces|embed|table|in writing/i;
// "Supprime les messages que j'ai mis": Bobby deletes only its own messages (it did confirm, then deleted nothing).
const OTHERS_MSGS = /\b(?:mes|ses|leurs|nos|vos) (?:propres )?messages\b|\bmessages? (?:que |qu')(?:j'ai|il a|elle a|ils ont|on a|tu m'as)|\b(?:my|his|her|their|our) (?:own )?messages\b/i;
const OTHERS_SAY = { fr: 'Je ne peux supprimer que mes propres messages, pas ceux des autres.', en: 'I can only delete my own messages, not other people’s.' };
// "Arrobase-moi dans dev-bot-test" moved the user to the voice channel dev-bot: a mention is a ping, never a move.
// Only when the destination is a text channel (or none): "déplace Lucas dans Ping Pong" stays a move.
const PING = /arrobas|arobas|\bping|\bmentionne|\btag(?:ue)?\b|@/i;
// The text channel a name means better than any voice channel, or undefined ("dev-bot-test" vs the voice "dev-bot").
function isText(q, ctx) {
  const best = (list) => Math.max(0, ...list.map((c) => sim(q, c.name)));
  const t = best(ctx.textChannels);
  return q && t >= 0.8 && t > best(ctx.voiceChannels) ? ctx.textChannels.find((c) => sim(q, c.name) === t) : undefined;
}

function guard(out, query, ctx, history = [], lang = 'fr') {
  const q = query.toLowerCase().replace(/[’`]/g, "'");
  out = { ...out, analyse: { genre: 'ordre', faisable: true, ...out.analyse } };
  if (CAPABLE.test(q)) out.analyse.genre = 'capacite';
  else if (out.analyse.genre === 'capacite' && out.actions.length && POLITE_ORDER.test(q)) out.analyse.genre = 'ordre'; // "peux-tu…" is an order
  // "j'aurais voulu des tableaux": a correction about the form means the same request, in writing
  if (out.analyse.genre === 'correction' && WRITE_HINT.test(q)) out.ecrit = true;
  // A question about Bobby the model answered by itself ("c'est quoi ton mot d'activation"): the local facts answer it
  // (only what it classed as a capability question: greetings and orders it refused keep their own answer)
  if (!out.actions.length && out.analyse.genre === 'capacite' && !CAPABLE.test(q) && aboutBobby(query, ctx, history)) {
    return { ...out, actions: [{ type: 'about', text: query }], say: '' };
  }
  // not feasible: no "nearest action" (volume 200 % does go through: music_volume says the maximum is 100)
  if (!out.analyse.faisable &&!out.actions.some((a) => a.type === 'music_volume')) {
    return { ...out, actions: [], say: out.say || (lang === 'en' ? "I can't do that." : 'Je ne peux pas faire ça.') };
  }
  let say = out.say;
  const actions = out.actions.flatMap((a) => {
    if ((a.type === 'clean' || a.type === 'msg_delete') && OTHERS_MSGS.test(q)) { say = OTHERS_SAY[lang === 'en' ? 'en' : 'fr']; return []; }
    if ((a.type === 'move' || a.type === 'come') && PING.test(q)) {
      // the text channel the sentence names wins over the LLM's (it picked the voice channel "dev-bot")
      const named = findChannel(query, ctx).channel?.name || (a.channel && isText(a.channel, ctx)?.name);
      if (named || !a.channel) return [{ type: 'ping', who: a.who?.length ? a.who : ['moi'], channel: named, text: '' }];
    }
    if (a.type === 'leave' && !LEAVE.test(q)) return /\b(?:stop|arrête|arrete|tais-toi|chut)\b/.test(q) ? [{ type: 'music_stop', quiet: true }] : [];
    if (a.type === 'pc_launch' && /\b(?:ferme|quitte|close|quit|kill)\b/.test(q)) return [{ ...a, type: 'pc_close' }];
    // "mute-moi micro et casque" read as undeafen: giving back needs a word that gives back
    if ((a.type === 'unmute' || a.type === 'undeafen') && !RESTORE.test(q)) return [{ ...a, type: a.type.slice(2) }];
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
  return { ...out, actions, say };
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
      // a text channel's name never resolves to a voice channel ("dev-bot-test" moved people to "dev-bot")
      const txt = isText(q, ctx);
      if (txt) {
        throw new Say(`${txt.name} est un salon texte, pas un salon vocal.`, `${txt.name} is a text channel, not a voice channel.`);
      }
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
    yes: 'Oui ?',
    notUnderstood: "Je n'ai pas compris.",
    noRights: "Je n'ai pas les droits Discord pour ça.",
    failed: "Ça n'a pas marché.",
    notAllowed: "Je n'ai pas le droit de te répondre.",
    forbidden: "Tu n'as pas le droit de me demander ça.",
    wrote: (where) => `Je t'envoie ça dans ${where}.`,
    canDo: 'Oui, je peux.',
    wantMe: 'Tu veux que je le fasse ?',
    call: "Je ne peux pas passer d'appel, mais je peux te mentionner dans le salon texte. Tu veux ?",
    thinking: 'Je réfléchis.',
  },
  en: {
    undo: (done) => (done ? 'Undone.' : 'Nothing to undo.'),
    confirm: 'Confirm?',
    cancelled: 'Okay, cancelled.',
    yes: 'Yes?',
    notUnderstood: "Sorry, I didn't get that.",
    noRights: "I don't have the Discord permissions for that.",
    failed: "That didn't work.",
    notAllowed: "I'm not allowed to answer you.",
    forbidden: "You're not allowed to ask me that.",
    wrote: (where) => `Sent to ${where}.`,
    canDo: 'Yes, I can.',
    wantMe: 'Do you want me to do it?',
    call: "I can't make calls, but I can mention you in the text channel. Want me to?",
    thinking: 'Let me think.',
  },
};

function stepSay(s, plan, lang = 'fr') {
  if (s.type === 'undo') return (lang === 'en' ? LINES.en : LINES.fr).undo(s.done ?? Boolean(undoOf(plan)));
  return ACTIONS[s.type].say[lang === 'en' ? 'en' : 'fr']?.(s, s.targets && names(s.targets, lang));
}
const describe = (plan, lang = 'fr') => plan.map((s) => stepSay(s, plan, lang)).filter(Boolean).join(' ');

// Coworker tone: an order gets a short ack ("Ça marche.") and is then done without a word. A step whose action is
// an echo ("Je déplace WakandeX dans…") stays silent; answers, errors and anything useful are still spoken.
const echoes = (s) => { const e = ACTIONS[s.type]?.echo; return typeof e === 'function' ? Boolean(e(s)) : Boolean(e); };
const silentKind = (plan) => plan.some((s) => ACTIONS[s.type]?.echo); // known before running: worth an ack
const report = (plan, lang = 'fr') => describe(plan.filter((s) => !echoes(s)), lang);

// Read-only answers: asked as a capability question ("es-tu capable de me dire l'heure ?"), they are just done.
const READONLY = new Set(['time', 'help', 'about', 'ask', 'todo_list', 'music_now', 'music_queue', 'perm_show', 'rights_list', 'alias_list', 'pc_music', 'pc_gpu']);
// What to do with an understood plan: 'run' it, or 'offer' it ("Tu veux que je le fasse ?", runs on a "oui").
const decide = (out, plan) => (plan.some((s) => s.offer) || (out?.analyse?.genre === 'capacite' && !plan.every((s) => READONLY.has(s.type))) ? 'offer' : 'run');

// ---------- Answers in writing ----------

// "dis-moi par écrit…", "in writing" (anywhere), or "écris-moi…", "envoie-moi…", "send me…" (at the start).
// "écris dans général bonjour" is not one of them: that is the message action.
// Also "dans le chat" anywhere, and "montre-moi…" / "affiche…" at the start ("explique dans le chat ce que tu sais
// faire" posted a plain LLM paragraph instead of the capabilities card).
const IN_WRITING = /[\s,]*(?:par écrit|à l'écrit|a l'ecrit|in writing|by text|as a message|(?:dans|sur|in|on) (?:le |the )?chat)(?![\p{L}])[\s,]*/iu;
const SHOW = /^(?:(?:est-ce que |est ce que )?(?:tu peux|tu pourrais|peux-tu|pourrais-tu)(?: me| nous)? )?(?:montre|affiche|afficher|montrer)(?:[- ]moi|[- ]nous)?(?![\p{L}])/iu;
// A text channel named anywhere: "dans le chat dev-bot-test", "dans le channel devbot test", "dans général".
const CHANNEL_AT = /(?:^|\s)(?:dans|sur|en|in|on|to) (?:le |la |the )?(?:(?:chat|channel|salon|canal)(?: texte)? )?#?((?:[\p{L}\d_-]+)(?: [\p{L}\d_-]+){0,2})/giu;
function findChannel(text, ctx) {
  for (const m of text.matchAll(CHANNEL_AT)) {
    const ws = m[1].split(' ');
    // the 1 to 3 words that best match a channel; on a tie the fewest ("DevBotTest ce que" also contains it)
    const best = ws.map((_, i) => ws.slice(0, i + 1).join(' ')).filter((n) => norm(n).length >= 3)
      .flatMap((name) => ctx.textChannels.map((x) => ({ name, x, s: sim(name, x.name) })))
      .reduce((a, b) => (b.s > a.s ? b : a), { s: 0 });
    if (best.s >= 0.8) {
      const said = m[0].slice(0, m[0].length - m[1].length) + best.name; // the preposition and those words
      return { channel: best.x, rest: text.replace(said, ' ').replace(/\s+/g, ' ').trim() };
    }
  }
  return { channel: null, rest: text };
}
// Never "mets" or "envoie-le" ("mets-moi dans Gaming", "envoie-le en AFK" are moves); "écris-le" repeats the last answer.
const WRITE_ME = /^(?:(?:est-ce que |est ce que )?(?:tu peux|tu pourrais|peux-tu|can you|could you|please)[\s,]+)?((?:écris|ecris)-(?:le|la|les|ça|ca)(?:-moi)?|(?:écris|ecris|envoie)[- ](?:moi|nous)|(?:write|send|text)(?: it| that| this)?(?: to)? me|write (?:it|that) down)(?![\p{L}])[\s,:]*/iu;
const REPEAT = /^(?:écris|ecris)-|^write (?:it|that) down/i; // no request of its own: the last one, in writing
const RIGHTS_Q = /droit|admin|permission|rights|liste noire|blacklist/i;

// -> null, or { query (the request without its writing words; "écris-le" alone repeats the last request), channel }
function writing(query, ctx, history = []) {
  if (MODULES.some((x) => x.early?.(query))) return null; // "note que le rapport est à rendre par écrit" is a note
  // "j'aurais voulu des tableaux" after a request: the same request again, in writing (the 4B model read it as a note)
  if (FORM_FIX.test(query) && WRITE_HINT.test(query) && history.length) {
    const last = history.at(-1).query;
    return { query: writing(last, ctx, [])?.query || last, channel: findChannel(query, ctx).channel };
  }
  const q = query.replace(/[’`]/g, "'").replace(/[.!?…]+$/, '').trim();
  const m = q.match(WRITE_ME);
  if (!m && !SHOW.test(q) && !IN_WRITING.test(q)) return null;
  let rest = m ? q.slice(m[0].length) : q; // "montre-moi la liste" keeps its verb: the shortcuts know it
  // "envoie-moi en AFK", "envoie-nous dans Gaming": a move, not an answer in writing
  if (m && !REPEAT.test(m[1]) && /^(?:dans|en|sur|vers|to|in|into)\b/i.test(rest)) return null;
  let channel;
  ({ channel, rest } = findChannel(rest, ctx));
  rest = rest.replace(IN_WRITING, ' ').replace(/\s+/g, ' ').trim();
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
async function hear({ text, lang = 'fr', score = null, member, guild, tEnd, sttMs, io, acked = false, ackMs = null, expected = false }) {
  let p = pending.get(member.id);
  // expected: the window was open when the sentence started (bot.js), even if it closed while the person was talking
  if (p && Date.now() > p.until && !expected) p = null;
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
    io.nope?.(); // heard, but no rights: the low sound, and the sentence at most every 30 s
    if (Date.now() - (refused.get(member.id) || 0) > 30_000) { refused.set(member.id, Date.now()); io.say(L.notAllowed, lang); }
    return;
  }
  pending.delete(member.id);
  const history = conversation();
  // Nothing to say (e.g. "stop" with no music): stay silent.
  // Not awaited: the reply plays on while the journal is written; member.id may interrupt it.
  const say = (s, voice) => { rec.reply = s; if (s) io.say(s, lang, voice, member.id); };
  const fail = (s) => { io.nope?.(); say(s); }; // heard "Bobby" but not understood, or not allowed
  // One short ack per order, before acting: at the wake gate (bot.js, acked), else here.
  const ack = () => { if (!acked) { acked = true; if (!io.ack?.()) io.beep?.(); } };
  // A plan Bobby proposes instead of running ("Tu veux que je le fasse ?"): a "oui" within 15 s runs it.
  const offer = (plan, line, w) => {
    pending.set(member.id, { until: Date.now() + 15000, plan: plan.map(({ offer: _, ...s }) => s), w });
    rec.outcome = 'offered';
    say(line);
  };
  const t = { stt: sttMs, ack: ackMs };
  try {
    // "Bobby, …" while a yes is awaited: a new order, not a "no". The pending plan is dropped silently.
    if (p?.plan && wake && !confirms(text) && !NO.test(text)) { console.log(`[confirm] dropped "${planText(p.plan)}": new order`); p = null; }
    if (p?.plan) {
      rec.plan = planText(p.plan);
      rec.query = text;
      rec.confirm = true; // a yes / no: never replayed by "refais"
      if (!confirms(text)) { rec.outcome = 'cancelled'; return say(L.cancelled); }
      if (!roles.allowed(role, needed(p.plan))) { rec.outcome = 'forbidden'; return fail(L.forbidden); } // rights may have changed meanwhile
      rec.outcome = 'confirmed';
      if (silentKind(p.plan)) ack();
      const t0 = performance.now();
      const pio = p.w ? writingIo(io, p.w) : io; // asked in writing before the "oui"
      await run(p.plan, pio, lang);
      t.action = Math.round(performance.now() - t0);
      if (p.w) return say(await postWritten(p.plan, pio, lang));
      return say(report(p.plan, lang), voiceOf(p.plan));
    }
    // "OK STOP BOBBY BOBBY STOP": the command follows the last of the repeated "Bobby".
    let query = retry((wake ? text.slice(text.indexOf(wake) + wake.length) : text).replace(/^(?:[\s,.:;!?]+|bobby\b)+/i, '').trim(), mineOf(history, member.id), history);
    rec.query = query;
    console.log(`[query] "${query}" (${lang})`);
    if (query.replace(/[^\p{L}]/gu, '').length < 2) {
      // "Bobby ?": like a coworker, "Oui ?", then the next sentence is the order (no "Bobby" needed).
      if (!io.yes?.()) say(L.yes);
      pending.set(member.id, { until: Date.now() + 8000 });
      rec.outcome = 'waiting';
      return console.log('[waiting] "Bobby" alone: next sentence is the command (8 s)');
    }
    const ctx = { ...context(member, guild), role, history };
    // "dis-moi par écrit…": the request without its writing words; the answer is posted, Bobby only says where.
    let w = null;
    const written = (x) => {
      w = x;
      rec.written = w.channel?.name || io.textChannel?.name || true;
      console.log(`[written] "${w.query}" -> ${w.channel?.name || 'default text channel'}`);
    };
    const asked = writing(query, ctx, mineOf(history, member.id));
    if (asked) {
      query = asked.query;
      written(asked);
      if (!query) { rec.outcome = 'resolve_error'; return fail(L.notUnderstood); }
      if (w.channel && w.channel.name !== io.textChannel?.name && !roles.allowed(role, 'admin')) { rec.outcome = 'forbidden'; return fail(L.forbidden); }
    }
    // A song title may end in a cancel word ("joue Nevermind"): playing wins.
    if (!quick(query)?.some((a) => a.type.startsWith('music_')) && cancels(query)) { rec.outcome = 'cancelled_by_speaker'; return say(L.cancelled); }
    let t0 = performance.now();
    let plan = null, out = null;
    const fast = quick(query);
    if (fast) try { plan = resolve(fast, ctx); t.shortcut = 0; rec.intent = { via: 'shortcut', actions: fast }; console.log(`[intent] shortcut: ${JSON.stringify(fast)}`); } catch { /* name not found: let the LLM try */ }
    if (!plan) {
      out = await understand(query, ctx, lang, history);
      t.llm = Math.round(performance.now() - t0);
      console.log(`[intent] LLM (${t.llm} ms): ${JSON.stringify(out)}`);
      if (unsure(out) && roles.allowed(role, 'admin') && claude.ready()) {
        io.say(L.thinking, lang, undefined, member.id);
        t0 = performance.now();
        out = await rethink(out, query, ctx, lang, history);
        t.claude = Math.round(performance.now() - t0);
        console.log(`[intent] Claude (${t.claude} ms): ${JSON.stringify(out)}`);
      }
      rec.intent = { via: out.via || 'llm', ...out };
      // "j'aurais voulu des tableaux": the model asks for a written answer itself
      if (out.ecrit && !w) written({ query, channel: null });
      if (!out.actions.length) {
        rec.outcome = 'answered';
        if (w && out.say) return say(L.wrote(await writingIo(io, w).post(out.say)));
        if (!out.say) return fail(L.notUnderstood);
        const g = noteGuard(out.say, [], lang);
        if (g.offer) offerNote(member.id, query, ctx);
        return say(g.text);
      }
      plan = resolve(out.actions, ctx);
    }
    const rio = w ? writingIo(io, w) : io;
    if (w) for (const s of plan) { s.written = true; if (s.type === 'about' && RIGHTS_Q.test(s.text)) s.type = 'rights_list'; }
    rec.plan = planText(plan);
    console.log(`[plan] ${rec.plan}`);
    const need = needed(plan);
    if (!roles.allowed(role, need)) {
      rec.outcome = 'forbidden';
      console.log(`[rights] ${member.displayName} is ${role}, "${rec.plan}" needs ${need}`);
      return fail(L.forbidden);
    }
    // "es-tu capable de supprimer tes messages ?" answers and offers, never runs; "appelle-moi" offers a mention.
    if (decide(out, plan) === 'offer') return offer(plan, plan.some((s) => s.offer === 'call') ? L.call : `${out?.say || L.canDo} ${L.wantMe}`, w);
    if (needsConfirm(plan)) {
      pending.set(member.id, { until: Date.now() + 10000, plan, w });
      rec.outcome = 'confirm_asked';
      return say(`${describe(plan, lang)} ${L.confirm}`);
    }
    if (silentKind(plan)) ack();
    for (const s of plan) { const a = ACTIONS[s.type].announce?.(lang); if (a) io.say(a, lang, undefined, member.id); }
    t0 = performance.now();
    await run(plan, rio, lang);
    t.action = Math.round(performance.now() - t0);
    rec.outcome = 'done';
    if (w) return say(await postWritten(plan, rio, lang));
    const g = noteGuard(report(plan, lang), plan, lang);
    if (g.offer) offerNote(member.id, query, ctx);
    say(g.text, voiceOf(plan));
  } catch (e) {
    if (e instanceof Say) {
      rec.outcome = e.code === 'LLM_DOWN' ? 'llm_down' : 'resolve_error';
      rec.error = e.message;
      console.log(`[resolve] ${e.message}`);
      return e.message === LINES.fr.notUnderstood ? fail(e.in(lang)) : say(e.in(lang));
    }
    rec.outcome = 'error'; rec.error = String(e.message || e);
    console.error('[bobby]', e);
    say(e.code === 50013 ? L.noRights : L.failed);
  } finally {
    remember(rec);
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
// "Réessaie" / "refais" replays the speaker's own last request, else the conversation's last one.
// "Recommence" stays music replay.
const RETRY = /^(?:(?:bobby|vas-y|allez)[\s,]+)*(?:réessaie|réessaye|reessaie|essaie encore|essaye encore|refais(?:-le| le| ça)?|try again|retry)[\s.!?]*$/i;
const lastOf = (list) => list.filter((h) => !h.confirm && !RETRY.test(h.query)).at(-1)?.query;
const retry = (query, mine, all = mine) => (RETRY.test(query) ? lastOf(mine) || lastOf(all) || query : query);
// Claude's answers are spoken with another voice.
const voiceOf = (plan) => plan.find((s) => s.voice)?.voice;

// Shortcut or LLM, as hear() does it, without running anything (tests). Also says whether it would be written,
// and what hear would do with the plan ('run', 'offer'), or 'answer' when there is no action.
async function intent(query, ctx, lang = 'fr', history = []) {
  const mine = mineOf(history, ctx.speaker.id);
  query = retry(query, mine, history);
  const w = writing(query, ctx, mine);
  if (w?.query) query = w.query;
  const fast = quick(query);
  if (fast) try { const plan = resolve(fast, ctx); return { via: 'shortcut', actions: fast, say: '', written: Boolean(w), decision: decide(null, plan) }; } catch { /* the LLM tries */ }
  const out = await understand(query, ctx, lang, history);
  let decision = 'answer';
  try { if (out.actions.length) decision = decide(out, resolve(out.actions, ctx)); } catch { decision = 'error'; }
  return { via: 'llm', ...out, written: Boolean(w || out.ecrit), decision };
}

module.exports = { STOP, writing, hear, intent, guard, noteGuard, aboutBobby, remember, conversation, reload, report, decide, unsure, journal, expecting, hotwords, context, understand, resolve, quick, cancels, confirms, pick, sim, describe, run, needed, roleTable, SYSTEM };
