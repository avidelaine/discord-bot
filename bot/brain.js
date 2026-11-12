// Bobby's brain: transcribed sentence -> intent (shortcut or local LLM) -> resolved names -> rights -> actions.
// Bobby understands French and English, and answers in the language it was spoken to in.
//
// Actions live in bot/actions/*.js, one module per theme, loaded in alphabetical order. A module exports:
//   help     { fr, en }   one line for "what can you do"
//   prompt   [lines]      action descriptions for the LLM          rules    [lines]  extra LLM rules
//   examples [lines]      'sentence -> {json}' few-shot examples    quick    [[regex, (match) => actions]] LLM-free shortcuts
//   actions  { type: { role: 'prime'|'admin'|'super', who, one, channel: 'voice'|'text', text, confirm,
//                      announce(lang), run(step, io, lang) -> undo function or nothing, say: { fr(step, names), en } } }
const fs = require('fs');
const path = require('path');
const { findWake } = require('./wake');
const { Say, norm, sim } = require('./util');
const roles = require('./roles');
const { load } = require('./store');

// One JSON line per exchange in _logs/conversations.jsonl (kept out of git), reviewed to improve Bobby.
// Full text only for sentences addressed to Bobby; other speech is logged as metadata by bot.js.
const JOURNAL = path.join(__dirname, '..', '_logs', 'conversations.jsonl');
fs.mkdirSync(path.dirname(JOURNAL), { recursive: true });
const journal = (rec) => fs.appendFile(JOURNAL, `${JSON.stringify(rec)}\n`, () => {});

const LLM_URL = process.env.LLM_URL || 'http://127.0.0.1:8080';
const YES = /\b(oui|ouais|vas-y|confirme|ok|okay|d'accord|go|yes|yeah|yep|sure|confirm)\b/i;

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
- "who" contient des noms de personnes, "moi" pour celui qui parle, ou "tous" pour tout le monde dans son salon, même si la phrase est en anglais. "except" liste les personnes exclues.
- Les noms sont souvent mal transcrits : écris le nom du contexte qui ressemble le plus à ce qui a été dit.
- Plusieurs demandes dans une phrase donnent plusieurs actions, dans l'ordre.
${MODULES.flatMap((m) => m.rules).join('\n')}
- Demande impossible ou incompréhensible : "actions" vide, et "say" le dit en une phrase courte.
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

function prompt(ctx, query, lang) {
  const byChan = {};
  for (const m of ctx.members) (byChan[m.channel] ||= []).push(m.nicknames?.length ? `${m.name} [surnoms : ${m.nicknames.join(', ')}]` : m.name);
  return [
    `Celui qui parle : ${ctx.speaker.name}, dans le salon vocal ${ctx.speaker.channel || 'aucun'}.`,
    `En vocal : ${Object.entries(byChan).map(([c, n]) => `${c} (${n.join(', ')})`).join(' ; ') || 'personne'}.`,
    `Salons vocaux : ${ctx.voiceChannels.map((c) => c.name).join(', ')}.`,
    `Salons texte : ${ctx.textChannels.map((c) => c.name).join(', ')}.`,
    `Date : ${new Date().toLocaleString('fr-FR', { dateStyle: 'full', timeStyle: 'short' })}.`,
    '',
    `Réponds dans "say" en ${lang === 'en' ? 'anglais' : 'français'}.`,
    `Phrase : "${query}"`,
  ].join('\n');
}

const LLM_DOWN = () => new Say('Mon cerveau ne répond pas.', "My brain isn't responding.");

async function understand(query, ctx, lang = 'fr') {
  let res;
  try {
    res = await fetch(`${LLM_URL}/v1/chat/completions`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        messages: [{ role: 'system', content: SYSTEM }, { role: 'user', content: prompt(ctx, query, lang) }],
        temperature: 0,
        max_tokens: 200,
        response_format: { type: 'json_schema', json_schema: { name: 'plan', schema: SCHEMA } },
      }),
    });
  } catch (e) {
    // fetch only throws when the server cannot be reached (ECONNREFUSED: llama-server is down or restarting)
    console.error(`[llm] llama-server unreachable at ${LLM_URL}: ${e.cause?.code || e.message}`);
    throw Object.assign(LLM_DOWN(), { code: 'LLM_DOWN' });
  }
  if (!res.ok) throw new Error(`LLM ${res.status}`);
  return JSON.parse((await res.json()).choices[0].message.content);
}

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

function resolve(actions, ctx) {
  return actions.map((a) => {
    const def = ACTIONS[a.type];
    if (!def) throw new Say("Je n'ai pas compris.", "Sorry, I didn't get that.");
    const step = { ...a, text: a.text || '' };
    delete step.who; delete step.except;
    if (def.who) step.targets = targets(a, ctx);
    if (def.one && step.targets.length !== 1) throw new Say('Une seule personne à la fois pour ça.', 'One person at a time for that.');
    if (def.channel === 'voice') {
      const q = a.channel || '';
      step.channel = HERE.has(norm(q)) ? ctx.voiceChannels.find((c) => c.name === ctx.speaker.channel) : pick(q, ctx.voiceChannels, 'voice');
      if (!step.channel) throw new Say("Tu n'es dans aucun salon vocal.", "You're not in a voice channel.");
    }
    if (def.channel === 'text') step.channel = pick(a.channel || '', ctx.textChannels, 'text');
    if (def.text && !step.text) throw new Say('Il me manque le texte.', "I'm missing the text.");
    return step;
  });
}

const needsConfirm = (plan) => plan.some((s) => ACTIONS[s.type].confirm) || plan.reduce((n, s) => n + (s.targets?.length || 0), 0) >= 3;

// Highest role the plan needs; undo needs the role of what it undoes.
let lastUndo = null; // { run, role }
const needed = (plan) => plan.map((s) => (s.type === 'undo' ? lastUndo?.role || 'prime' : ACTIONS[s.type].role || 'admin'))
  .reduce((a, b) => (roles.LEVEL[b] > roles.LEVEL[a] ? b : a), 'prime');

// Which role each action needs, for the startup log and the README.
const roleTable = () => Object.fromEntries(Object.entries(ACTIONS).map(([k, d]) => [k, d.role || 'admin']));

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
  },
};

function describe(plan, lang = 'fr') {
  const L = lang === 'en' ? LINES.en : LINES.fr;
  return plan.map((s) => {
    if (s.type === 'undo') return L.undo(Boolean(lastUndo));
    return ACTIONS[s.type].say[lang === 'en' ? 'en' : 'fr'](s, s.targets && names(s.targets, lang));
  }).filter(Boolean).join(' ');
}

// ---------- Execution ----------

async function run(plan, io, lang = 'fr') {
  const undos = [];
  for (const s of plan) {
    if (s.type === 'undo') { if (lastUndo) await lastUndo.run(); continue; }
    const undo = await ACTIONS[s.type].run(s, io, lang);
    if (typeof undo === 'function') undos.push(undo);
  }
  if (undos.length) {
    lastUndo = { role: needed(plan), run: () => Promise.all(undos.reverse().map((u) => u())).then(() => { lastUndo = null; }) };
  } else if (plan.some((s) => s.type === 'undo')) lastUndo = null;
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
  const say = (s) => { rec.reply = s; return io.say(s, lang); };
  const t = { stt: sttMs };
  try {
    if (p?.plan) {
      rec.plan = planText(p.plan);
      if (!YES.test(text)) { rec.outcome = 'cancelled'; return say(L.cancelled); }
      rec.outcome = 'confirmed';
      const t0 = performance.now();
      await run(p.plan, io, lang);
      t.action = Math.round(performance.now() - t0);
      return say(describe(p.plan, lang));
    }
    const query = wake ? text.slice(text.indexOf(wake) + wake.length).replace(/^[\s,.:;!?]+/, '').trim() : text;
    rec.query = query;
    console.log(`[query] "${query}" (${lang})`);
    io.beep();
    if (query.replace(/[^\p{L}]/gu, '').length < 2) { pending.set(member.id, { until: Date.now() + 6000 }); rec.outcome = 'waiting'; return console.log('[waiting] "Bobby" alone: next sentence is the command (6 s)'); }
    if (cancels(query)) { rec.outcome = 'cancelled_by_speaker'; return say(L.cancelled); }

    const ctx = context(member, guild);
    let t0 = performance.now();
    let plan = null;
    const fast = quick(query);
    if (fast) try { plan = resolve(fast, ctx); t.shortcut = 0; rec.intent = { via: 'shortcut', actions: fast }; console.log(`[intent] shortcut: ${JSON.stringify(fast)}`); } catch { /* name not found: let the LLM try */ }
    if (!plan) {
      const out = await understand(query, ctx, lang);
      t.llm = Math.round(performance.now() - t0);
      rec.intent = { via: 'llm', ...out };
      console.log(`[intent] LLM (${t.llm} ms): ${JSON.stringify(out)}`);
      if (!out.actions.length) { rec.outcome = 'answered'; return say(out.say || L.notUnderstood); }
      plan = resolve(out.actions, ctx);
    }
    rec.plan = planText(plan);
    console.log(`[plan] ${rec.plan}`);
    const need = needed(plan);
    if (!roles.allowed(role, need)) {
      rec.outcome = 'forbidden';
      console.log(`[rights] ${member.displayName} is ${role}, "${rec.plan}" needs ${need}`);
      return say(L.forbidden);
    }
    if (needsConfirm(plan)) {
      pending.set(member.id, { until: Date.now() + 10000, plan });
      rec.outcome = 'confirm_asked';
      return say(`${describe(plan, lang)} ${L.confirm}`);
    }
    for (const s of plan) { const a = ACTIONS[s.type].announce?.(lang); if (a) io.say(a, lang); }
    t0 = performance.now();
    await run(plan, io, lang);
    t.action = Math.round(performance.now() - t0);
    rec.outcome = 'done';
    say(describe(plan, lang));
  } catch (e) {
    if (e instanceof Say) { rec.outcome = e.code === 'LLM_DOWN' ? 'llm_down' : 'resolve_error'; rec.error = e.message; console.log(`[resolve] ${e.message}`); return say(e.in(lang)); }
    rec.outcome = 'error'; rec.error = String(e.message || e);
    console.error('[bobby]', e);
    say(e.code === 50013 ? L.noRights : L.failed);
  } finally {
    t.total = Math.round(performance.now() - tEnd);
    rec.timing = t;
    journal(rec);
    console.log(`[timing] ${Object.entries(t).filter(([, v]) => v != null).map(([k, v]) => `${k} ${v} ms`).join(' · ')}  (total = from end of speech to reply)`);
  }
}

module.exports = { hear, journal, expecting, hotwords, context, understand, resolve, quick, cancels, pick, sim, describe, run, needed, roleTable, SYSTEM };
