// Bobby's brain: transcribed sentence -> intent (local LLM) -> resolved names -> Discord actions.
// Bobby understands French and English, and answers in the language it was spoken to in.
const { findWake } = require('./wake');
const { load, save } = require('./store');

const LLM_URL = process.env.LLM_URL || 'http://127.0.0.1:8080';
const ALLOWED = (process.env.ALLOWED_USERS || '').split(',').map((s) => s.trim()).filter(Boolean);
const YES = /\b(oui|ouais|vas-y|confirme|ok|okay|d'accord|go|yes|yeah|yep|sure|confirm)\b/i;

const SCHEMA = {
  type: 'object',
  properties: {
    actions: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          type: { enum: ['mute', 'unmute', 'deafen', 'undeafen', 'move', 'disconnect', 'message',
            'todo_add', 'todo_remove', 'todo_list', 'undo', 'leave'] },
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
- mute / unmute : couper ou rétablir le micro de "who"
- deafen / undeafen : couper ou rétablir le son (le casque) de "who"
- move : déplacer "who" dans le salon vocal "channel"
- disconnect : déconnecter "who" du vocal
- message : envoyer "text" dans le salon texte "channel"
- todo_add / todo_remove : ajouter ou retirer "text" de la liste de tâches
- todo_list : lire la liste de tâches
- undo : annuler la dernière action
- leave : Bobby quitte le salon vocal

Règles :
- "who" contient des noms de personnes, "moi" pour celui qui parle, ou "tous" pour tout le monde dans son salon, même si la phrase est en anglais. "except" liste les personnes exclues.
- "virer", "dégager" ou "kicker" une personne nommée du vocal, c'est disconnect. Si on dit à Bobby lui-même de partir, c'est leave.
- "ici" désigne le salon vocal de celui qui parle : mets son nom exact dans "channel".
- Les noms sont souvent mal transcrits : écris le nom du contexte qui ressemble le plus à ce qui a été dit.
- Plusieurs demandes dans une phrase donnent plusieurs actions, dans l'ordre.
- Question ou discussion sans action : "actions" vide, et réponds dans "say" en une phrase courte.
- Demande impossible ou incompréhensible : "actions" vide, et "say" le dit en une phrase courte.
- "say" est toujours dans la langue de la phrase : français si on te parle français, anglais si on te parle anglais.

Exemples :
"coupe le micro de Kevin" -> {"actions":[{"type":"mute","who":["Kévin"]}],"say":""}
"ramène tout le monde ici sauf Marie" -> {"actions":[{"type":"move","who":["tous"],"except":["Marie"],"channel":"Général"}],"say":""}
"mets-moi dans Gaming et rends le son à Zyklon" -> {"actions":[{"type":"move","who":["moi"],"channel":"Gaming"},{"type":"undeafen","who":["Zyklon"]}],"say":""}
"rends le micro à tout le monde sauf Zyklon" -> {"actions":[{"type":"unmute","who":["tous"],"except":["Zyklon"]}],"say":""}
"retire les courses de ma liste" -> {"actions":[{"type":"todo_remove","text":"les courses"}],"say":""}
"non annule ça" -> {"actions":[{"type":"undo"}],"say":""}
"c'est bon tu peux y aller" -> {"actions":[{"type":"leave"}],"say":""}
"t'es qui toi" -> {"actions":[],"say":"Je suis Bobby, l'assistant vocal du serveur."}
"move me and Zyklon to Gaming" -> {"actions":[{"type":"move","who":["moi","Zyklon"],"channel":"Gaming"}],"say":""}
"kick Lucas from voice" -> {"actions":[{"type":"disconnect","who":["Lucas"]}],"say":""}
"what can you do" -> {"actions":[],"say":"I can mute, move or disconnect people, post messages and keep your to-do list."}`;

// ---------- Context ----------

// Snapshot of the server as plain objects (ref = Discord object), so the rest is testable without Discord.
function context(member, guild) {
  const members = [...guild.voiceStates.cache.values()]
    .filter((v) => v.channel && v.member && !v.member.user.bot)
    .map((v) => ({
      id: v.id, name: v.member.displayName, channel: v.channel.name, ref: v.member,
      aliases: [v.member.user.username, v.member.user.globalName].filter(Boolean),
    }));
  const chans = (type) => [...guild.channels.cache.values()].filter((c) => c.type === type).map((c) => ({ name: c.name, ref: c }));
  return {
    speaker: members.find((m) => m.id === member.id) || { id: member.id, name: member.displayName, ref: member },
    members,
    voiceChannels: chans(2),
    textChannels: chans(0),
  };
}

const hotwords = (ctx) => ['Bobby', ...ctx.members.map((m) => m.name), ...ctx.voiceChannels.map((c) => c.name)].join(' ').slice(0, 400);

function prompt(ctx, query) {
  const byChan = {};
  for (const m of ctx.members) (byChan[m.channel] ||= []).push(m.name);
  return [
    `Celui qui parle : ${ctx.speaker.name}, dans le salon vocal ${ctx.speaker.channel || 'aucun'}.`,
    `En vocal : ${Object.entries(byChan).map(([c, n]) => `${c} (${n.join(', ')})`).join(' ; ') || 'personne'}.`,
    `Salons vocaux : ${ctx.voiceChannels.map((c) => c.name).join(', ')}.`,
    `Salons texte : ${ctx.textChannels.map((c) => c.name).join(', ')}.`,
    `Date : ${new Date().toLocaleString('fr-FR', { dateStyle: 'full', timeStyle: 'short' })}.`,
    '',
    `Phrase : "${query}"`,
  ].join('\n');
}

async function understand(query, ctx) {
  const res = await fetch(`${LLM_URL}/v1/chat/completions`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      messages: [{ role: 'system', content: SYSTEM }, { role: 'user', content: prompt(ctx, query) }],
      temperature: 0,
      max_tokens: 200,
      response_format: { type: 'json_schema', json_schema: { name: 'plan', schema: SCHEMA } },
    }),
  });
  if (!res.ok) throw new Error(`LLM ${res.status}`);
  return JSON.parse((await res.json()).choices[0].message.content);
}

// LLM-free shortcut for simple commands (saves ~1 s). Anything more complex returns null and goes to the LLM.
// ponytail: a handful of phrasings per language; add the ones that keep showing up in the logs.
const QUICK = [
  [/^(?:coupe|ferme|mute)(?: le)? micro (?:de|à|a) (.+)$/, 'mute'],
  [/^mute (.+)$/, 'mute'],
  [/^(?:rends|remets|rétablis|redonne|rallume)(?: le)? micro (?:de|à|a) (.+)$/, 'unmute'],
  [/^unmute (.+)$/, 'unmute'],
  [/^(?:coupe le son (?:de|à|a)|deafen) (.+)$/, 'deafen'],
  [/^(?:(?:rends|remets|rétablis|redonne) le son (?:de|à|a)|undeafen) (.+)$/, 'undeafen'],
  [/^(?:déplace|mets|envoie|bouge) (.+?) (?:dans|en|sur|vers) (?:le salon |la salle (?:de )?)?(.+)$/, 'move'],
  [/^mets[- ]moi (?:dans|en) (?:le salon |la salle (?:de )?)?(.+)$/, 'move-me'],
  [/^(?:move|put|send|drag) (.+?) (?:to|in|into) (?:the )?(.+?)(?: channel)?$/, 'move'],
  [/^(?:annule(?: ça)?|undo(?: that)?)$/, 'undo'],
];

function quick(query) {
  const q = query.toLowerCase().replace(/[.!?,]+$/, '').trim();
  // several requests, people or exclusions: leave it to the LLM
  if (/ et | puis |sauf| and | then |except|,/.test(q)) return null;
  for (const [re, type] of QUICK) {
    const m = q.match(re);
    if (!m) continue;
    if (type === 'undo') return [{ type }];
    if (type === 'move-me') return [{ type: 'move', who: ['moi'], channel: m[1] }];
    const who = [/^(?:tout le monde|everyone|everybody)$/.test(m[1]) ? 'tous' : m[1]];
    return [{ type, who, channel: m[2] }];
  }
  return null;
}

// ---------- Name resolution ----------

// Error spoken to the user as is, in their language.
class Say extends Error {
  constructor(fr, en) { super(fr); this.en = en; }
  in(lang) { return lang === 'en' ? this.en : this.message; }
}

const WHAT = {
  person: ['la personne', 'anyone called'],
  voice: ['le salon vocal', 'a voice channel called'],
  text: ['le salon texte', 'a text channel called'],
};

const norm = (s) => s.normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase().replace(/[^a-z0-9]/g, '');

function lev(a, b) {
  let prev = [...Array(b.length + 1).keys()];
  for (let i = 1; i <= a.length; i++) {
    const cur = [i];
    for (let j = 1; j <= b.length; j++) cur[j] = Math.min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (a[i - 1] === b[j - 1] ? 0 : 1));
    prev = cur;
  }
  return prev[b.length];
}

function sim(q, c) {
  const a = norm(q), b = norm(c);
  if (!a || !b) return 0;
  if (a === b) return 1;
  if (Math.min(a.length, b.length) >= 3 && (a.includes(b) || b.includes(a))) return 0.9;
  return 1 - lev(a, b) / Math.max(a.length, b.length);
}

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

const NEEDS_WHO = new Set(['mute', 'unmute', 'deafen', 'undeafen', 'move', 'disconnect']);

function resolve(actions, ctx) {
  return actions.map((a) => {
    const step = { type: a.type, text: a.text || '' };
    if (NEEDS_WHO.has(a.type)) step.targets = targets(a, ctx);
    if (a.type === 'move') {
      const q = a.channel || '';
      step.channel = HERE.has(norm(q)) ? ctx.voiceChannels.find((c) => c.name === ctx.speaker.channel) : pick(q, ctx.voiceChannels, 'voice');
    }
    if (a.type === 'message') step.channel = pick(a.channel || '', ctx.textChannels, 'text');
    if (['message', 'todo_add', 'todo_remove'].includes(a.type) && !step.text) throw new Say('Il me manque le texte.', "I'm missing the text.");
    return step;
  });
}

const needsConfirm = (plan) => plan.some((s) => s.type === 'disconnect') || plan.reduce((n, s) => n + (s.targets?.length || 0), 0) >= 3;

// ---------- What Bobby says ----------

const names = (ts, lang) => ts.map((t) => t.name).join(', ').replace(/, ([^,]*)$/, lang === 'en' ? ' and $1' : ' et $1');

const LINES = {
  fr: {
    mute: (n) => `Je coupe le micro de ${n}.`,
    unmute: (n) => `Je rétablis le micro de ${n}.`,
    deafen: (n) => `Je coupe le son de ${n}.`,
    undeafen: (n) => `Je rétablis le son de ${n}.`,
    move: (n, c) => `Je déplace ${n} dans ${c}.`,
    disconnect: (n) => `Je déconnecte ${n}.`,
    message: (n, c) => `J'envoie le message dans ${c}.`,
    todo_add: (n, c, t) => `J'ajoute « ${t} » à ta liste.`,
    todo_remove: (n, c, t, missing) => (missing ? `Je ne trouve pas « ${t} » dans ta liste.` : `Je retire « ${t} » de ta liste.`),
    todo_list: (l) => (l.length ? `Ta liste : ${l.join(', ')}.` : 'Ta liste est vide.'),
    undo: (done) => (done ? "J'annule." : "Il n'y a rien à annuler."),
    leave: () => 'À plus.',
    confirm: 'Tu confirmes ?',
    cancelled: "D'accord, j'annule.",
    notUnderstood: "Je n'ai pas compris.",
    noRights: "Je n'ai pas les droits Discord pour ça.",
    failed: "Ça n'a pas marché.",
  },
  en: {
    mute: (n) => `Muting ${n}.`,
    unmute: (n) => `Unmuting ${n}.`,
    deafen: (n) => `Deafening ${n}.`,
    undeafen: (n) => `Undeafening ${n}.`,
    move: (n, c) => `Moving ${n} to ${c}.`,
    disconnect: (n) => `Disconnecting ${n}.`,
    message: (n, c) => `Message sent to ${c}.`,
    todo_add: (n, c, t) => `Added "${t}" to your list.`,
    todo_remove: (n, c, t, missing) => (missing ? `I can't find "${t}" in your list.` : `Removed "${t}" from your list.`),
    todo_list: (l) => (l.length ? `Your list: ${l.join(', ')}.` : 'Your list is empty.'),
    undo: (done) => (done ? 'Undone.' : 'Nothing to undo.'),
    leave: () => 'See you.',
    confirm: 'Confirm?',
    cancelled: 'Okay, cancelled.',
    notUnderstood: "Sorry, I didn't get that.",
    noRights: "I don't have the Discord permissions for that.",
    failed: "That didn't work.",
  },
};

let lastUndo = null;

function describe(plan, lang = 'fr') {
  const L = LINES[lang] || LINES.fr;
  return plan.map((s) => {
    if (s.type === 'todo_list') return L.todo_list(load('todo', []));
    if (s.type === 'undo') return L.undo(Boolean(lastUndo));
    return L[s.type](s.targets && names(s.targets, lang), s.channel?.name, s.text, s.missing);
  }).join(' ');
}

// ---------- Execution ----------

async function run(plan, io) {
  const undos = [];
  for (const s of plan) {
    const ms = (s.targets || []).map((t) => t.ref);
    switch (s.type) {
      case 'mute': case 'unmute': case 'deafen': case 'undeafen': {
        const key = s.type.endsWith('mute') ? 'serverMute' : 'serverDeaf';
        const set = (m, v) => (key === 'serverMute' ? m.voice.setMute(v) : m.voice.setDeaf(v));
        const prev = ms.map((m) => [m, m.voice[key]]);
        await Promise.all(ms.map((m) => set(m, !s.type.startsWith('un'))));
        undos.push(() => Promise.all(prev.map(([m, v]) => set(m, v))));
        break;
      }
      case 'move': {
        const prev = ms.map((m) => [m, m.voice.channel]);
        await Promise.all(ms.map((m) => m.voice.setChannel(s.channel.ref)));
        undos.push(() => Promise.all(prev.map(([m, c]) => m.voice.setChannel(c))));
        break;
      }
      case 'disconnect': await Promise.all(ms.map((m) => m.voice.disconnect())); break;
      case 'message': await s.channel.ref.send(s.text); break;
      case 'todo_add': save('todo', [...load('todo', []), s.text]); break;
      case 'todo_remove': {
        const l = load('todo', []);
        const i = l.findIndex((x) => sim(s.text, x) >= 0.7);
        s.missing = i < 0;
        if (!s.missing) { l.splice(i, 1); save('todo', l); }
        break;
      }
      case 'undo': if (lastUndo) await lastUndo(); break;
      case 'leave': io.leave(); break;
    }
  }
  if (undos.length) lastUndo = () => Promise.all(undos.reverse().map((u) => u())).then(() => { lastUndo = null; });
  else if (plan.some((s) => s.type === 'undo')) lastUndo = null;
}

// ---------- Entry point ----------

const pending = new Map(); // userId -> { until, plan? }: "Bobby" said alone, or a pending confirmation

async function hear({ text, lang = 'fr', member, guild, tEnd, sttMs, io }) {
  let p = pending.get(member.id);
  if (p && Date.now() > p.until) p = null;
  const wake = findWake(text);
  if (!wake && !p) return;
  if (!(ALLOWED.length ? ALLOWED.includes(member.id) : member.id === guild.ownerId)) {
    return console.log(`[bobby] ${member.displayName} is not allowed to give commands`);
  }
  pending.delete(member.id);
  const L = LINES[lang] || LINES.fr;
  const say = (s) => io.say(s, lang);
  const t = { stt: sttMs };
  try {
    if (p?.plan) {
      if (!YES.test(text)) return say(L.cancelled);
      const t0 = performance.now();
      await run(p.plan, io);
      t.action = Math.round(performance.now() - t0);
      return say(describe(p.plan, lang));
    }
    const query = wake ? text.slice(text.indexOf(wake) + wake.length).replace(/^[\s,.:;!?]+/, '').trim() : text;
    io.beep();
    if (query.replace(/[^\p{L}]/gu, '').length < 2) { pending.set(member.id, { until: Date.now() + 6000 }); return; }

    const ctx = context(member, guild);
    let t0 = performance.now();
    let plan = null;
    const fast = quick(query);
    if (fast) try { plan = resolve(fast, ctx); t.shortcut = 0; } catch { /* name not found: let the LLM try */ }
    if (!plan) {
      const out = await understand(query, ctx);
      t.llm = Math.round(performance.now() - t0);
      console.log(`[bobby] "${query}" -> ${JSON.stringify(out)}`);
      if (!out.actions.length) return say(out.say || L.notUnderstood);
      plan = resolve(out.actions, ctx);
    }
    if (needsConfirm(plan)) {
      pending.set(member.id, { until: Date.now() + 10000, plan });
      return say(`${describe(plan, lang)} ${L.confirm}`);
    }
    t0 = performance.now();
    await run(plan, io);
    t.action = Math.round(performance.now() - t0);
    say(describe(plan, lang));
  } catch (e) {
    if (e instanceof Say) return say(e.in(lang));
    console.error('[bobby]', e);
    say(e.code === 50013 ? L.noRights : L.failed);
  } finally {
    t.total = Math.round(performance.now() - tEnd);
    console.log(`[timing] ${Object.entries(t).filter(([, v]) => v != null).map(([k, v]) => `${k} ${v} ms`).join(' · ')}  (total = from end of speech to reply)`);
  }
}

module.exports = { hear, hotwords, context, understand, resolve, quick, pick, sim, describe };
