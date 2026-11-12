// To-do lists in data/todo.json: one shared general list plus one personal list per person.
// Item: { id, text, list: 'general' | memberId, author: { id, name }, at (ISO), done }.
// Rights: everyone with a role adds to the general list and to their own, and manages their own items
// (written by them, or on their list); the super admin manages everything.
const { load, save } = require('../store');
const { Say, norm, sim } = require('../util');
const { embed, COLOR } = require('../format');

const GENERAL = 'general';
// Old format (plain strings) is read as unsigned items of the general list.
const items = () => load('todo', []).map((x, i) => (typeof x === 'string' ? { id: i + 1, text: x, list: GENERAL, author: { id: null, name: '?' }, at: null, done: false } : x));

const isGeneral = (raw) => !raw || /^(?:general|générale?|generale?|commune?|partag[ée]e|shared|la liste|the list|todo|to-?do(?: list)?)$/i.test(raw.trim());
const isMine = (raw) => /^(?:moi|me|mine|my|ma liste|my list|perso|personnelle?|personal|la mienne)$/i.test((raw || '').trim());

// Spoken list name -> { id, name }. People come from voice and from authors already in the lists.
function listOf(raw, ctx, pick) {
  if (isGeneral(raw)) return { id: GENERAL, name: null };
  if (isMine(raw)) return { id: ctx.speaker.id, name: ctx.speaker.name };
  const people = [...ctx.members];
  for (const it of items()) if (it.author?.id && !people.some((p) => p.id === it.author.id)) people.push({ id: it.author.id, name: it.author.name });
  const p = pick(raw.replace(/^(?:la liste (?:de |d')|ma liste|la liste|list of |the list of )/i, ''), people, 'person');
  return { id: p.id, name: p.name };
}

const listName = (l, s, lang) => {
  if (l.id === GENERAL) return lang === 'en' ? 'the general list' : 'la liste générale';
  if (l.id === s.by?.id) return lang === 'en' ? 'your list' : 'ta liste';
  return lang === 'en' ? `${l.name}'s list` : `la liste de ${l.name}`;
};

const canEdit = (it, by) => by?.role === 'super' || (by && (it.author?.id === by.id || it.list === by.id));
const canAdd = (list, by) => by?.role === 'super' || list === GENERAL || list === by?.id;
const NOT_YOURS = () => new Say('Tu ne peux modifier que tes propres points, ou ceux de ta liste.', 'You can only change your own items, or the ones on your list.');

// Item named in a sentence: its number in the list, "dernier" / "premier", or words of its text.
function findItem(s, all) {
  const scope = all.filter((it) => (s.list ? it.list === s.list.id : it.list === GENERAL || it.list === s.by?.id || s.by?.role === 'super'));
  const ref = (s.text || '').trim().toLowerCase().replace(/^(?:le |la |l'|the )?(?:point|élément|item|tâche|task|numéro|number|n°)\s*/, '');
  if (/^\d+$/.test(ref)) return (s.list ? scope : scope.filter((it) => it.list === GENERAL))[+ref - 1];
  // numbers, "dernier" and "premier" without a list name count in the general list, as it is posted
  const listed = s.list ? scope : scope.filter((it) => it.list === GENERAL);
  if (/^(?:dernier|derni[eè]re|last)(?: point| item)?$/.test(ref)) return listed.at(-1);
  if (/^(?:premier|premi[eè]re|first)(?: point| item)?$/.test(ref)) return listed[0];
  const q = norm(ref);
  const score = (t) => (q.length >= 4 && norm(t).includes(q) ? 0.9 : sim(ref, t));
  const best = scope.map((it) => ({ it, s: score(it.text) })).sort((a, b) => b.s - a.s)[0];
  return best && best.s >= 0.6 ? best.it : null;
}

// Runs fn(all, step) -> item touched (null: not found) and saves. Undo reverts only that item, so it never
// erases what someone else added meanwhile.
const edit = (fn) => (s) => {
  const all = items();
  const before = new Map(all.map((x, i) => [x.id, { x: { ...x }, i }]));
  const it = fn(all, s);
  s.item = it;
  if (!it) return null;
  save('todo', all);
  const was = before.get(it.id);
  return () => {
    const cur = items();
    const i = cur.findIndex((x) => x.id === it.id);
    if (!was) { if (i >= 0) cur.splice(i, 1); } // added: remove it
    else if (i < 0) cur.splice(Math.min(was.i, cur.length), 0, was.x); // deleted: put it back
    else cur[i] = was.x; // ticked or moved: previous state
    save('todo', cur);
  };
};
const touch = (fn) => edit((all, s) => {
  const it = findItem(s, all);
  if (it && !canEdit(it, s.by)) throw NOT_YOURS();
  if (it) fn(all, it, s);
  return it;
});

const date = (iso) => (iso ? new Date(iso).toLocaleString('fr-FR', { day: '2-digit', month: '2-digit', hour: '2-digit', minute: '2-digit' }) : '?');
// A list as an embed: one field per item, with its done mark, author and date.
function card(l, mine) {
  const open = mine.filter((it) => !it.done).length, done = mine.length - open;
  const fields = mine.map((it, i) => {
    const long = it.text.length > 200;
    return { name: `${it.done ? '✅' : '⬜'} ${i + 1}. ${long ? `${it.text.slice(0, 199)}…` : it.text}`, value: `${long ? `${it.text}\n` : ''}*${it.author?.name || '?'} · ${date(it.at)}*` };
  });
  return { embeds: embed({ title: `📝 ${l.id === GENERAL ? 'Liste générale' : `Liste de ${l.name}`}`, color: COLOR.todo, description: `**${open}** à faire · **${done}** fait${done > 1 ? 's' : ''}`, fields }) };
}
// ponytail: spoken items are cut to 10 words, the full text is posted in the text channel.
const short = (t) => { const w = t.split(/\s+/); return w.length > 10 ? `${w.slice(0, 10).join(' ')}…` : t; };

// ---------- "Note ça" requests, recognized before the LLM (and before the "several requests" guard) ----------

// longest forms first: the regex engine takes the first alternative that fits ("note" would leave the r of "noter")
const VERB = "(?:noter|notes|note|not[ée]|rajouter|rajoutes|rajoute|ajouter|ajoutes|ajoute|inscrire|inscris|retenir|retiens|write down|note down|add|put)";
const LIST = "(?:la|ta|ma|une|the|my|your) (?:liste|to-?do(?: list)?|list)(?: des choses à faire| de tâches| de choses à faire)?";
// \b is ASCII-only in JS: (?<!\p{L}) and (?!\p{L}) also work next to « à » and « noté ».
const ON_LIST = new RegExp(`(?<!\\p{L})${VERB}(?!\\p{L})[^.?!]{0,60}?(?<!\\p{L})(?:dans|à|a|sur|to|on|in) ${LIST}`, 'iu');
const START = /^(?:retiens|note(?:s| bien)?|noter|pour la prochaine fois|il faudrait que tu|il faut que tu retiennes|remember|make a note|note that|for next time)\b/i;
const POLITE = "(?:(?:bobby|alors|ok|bon|euh)[\\s,]+)*(?:(?:est-ce que |est ce que )?(?:tu peux|tu pourrais|peux-tu|peux tu|pourrais-tu|pourrais tu)\\s+|vas-y\\s+|can you\\s+|could you\\s+|please\\s+)?";
const LEAD = new RegExp(`^${POLITE}(?:me |le |l'|bien )*${VERB}(?!\\p{L})(?: bien)?(?: (?:ça|ca|this|that))?(?: (?:dans|à|a|sur|to|on|in) ${LIST})?(?: que | qu'|, | : |: | that |\\s)?`, 'iu');
const TAIL = new RegExp(`\\s*(?:dans|à|a|sur|to|on|in) ${LIST}\\s*(?:s'il te pla[iî]t|stp|please)?\\s*[.?!…]*$`, 'i');
const words = (t) => t.split(/\s+/).filter((w) => /\p{L}/u.test(w)).length;

// -> [todo_add] with the full text of the note, or null.
function note(query) {
  const raw = query.trim();
  if (!ON_LIST.test(raw) && !START.test(raw.replace(new RegExp(`^${POLITE}`, 'i'), ''))) return null;
  const sentences = raw.split(/(?<=[.?!…])\s+/);
  let text = raw;
  // "J'aimerais une voix différente pour Claude. Tu peux le noter dans la liste ?": drop the request sentence.
  const isRequest = (x) => (ON_LIST.test(x) || START.test(x.replace(new RegExp(`^${POLITE}`, 'i'), ''))) && words(x) <= 15;
  if (sentences.length > 1) {
    const rest = sentences.filter((x) => !isRequest(x));
    if (rest.length && rest.length < sentences.length) text = rest.join(' ');
  }
  // "ajoute acheter du pain à ma liste", "note dans la liste que ...": keep only what is to be noted.
  // "il faudrait que tu gardes l'historique ..." is a note as a whole: kept untouched.
  if (LEAD.test(text) && !/^(?:il faudrait|il faut|pour la prochaine)/i.test(text)) {
    const stripped = text.replace(LEAD, '').replace(TAIL, '').trim().replace(/\s*\?$/, ''); // the request was the question
    if (words(stripped) >= 2) text = stripped;
  }
  text = text.replace(/^[\s,:;-]+/, '').replace(/\s*[,;:]+$/, '').trim();
  if (words(text) < 2) return null;
  text = text[0].toUpperCase() + text.slice(1);
  const who = raw.match(/\bla liste (?:de |d')([^\s,.?!]+)/i)?.[1];
  const list = /\b(?:ma liste|ma to-?do|ma propre liste|my list|my to-?do)\b/i.test(raw) ? 'moi' : who && !/^(?:choses|tâches|courses)$/i.test(who) ? who : GENERAL;
  return [{ type: 'todo_add', text, list }];
}

// Lines for the LLM prompt, so "le dernier point de la liste" makes sense.
function promptLines(ctx) {
  const all = items().filter((it) => !it.done);
  const fmt = (l) => l.map((it, i) => `${i + 1}. ${it.text.slice(0, 80)}`).join(' ; ') || 'vide';
  return [
    `Liste générale : ${fmt(all.filter((it) => it.list === GENERAL).slice(-8))}.`,
    `Liste perso de ${ctx.speaker.name} : ${fmt(all.filter((it) => it.list === ctx.speaker.id).slice(-5))}.`,
  ];
}

const LIST_FIELD = { type: 'string' };
const say = (fr, en) => ({ fr: (s) => fr(s, s.list || { id: GENERAL }), en: (s) => en(s, s.list || { id: GENERAL }) });
const missing = { fr: (s) => `Je ne trouve pas « ${s.text} » dans la liste.`, en: (s) => `I can't find "${s.text}" in the list.` };

module.exports = {
  help: { fr: 'tenir la liste générale et la liste perso de chacun (ajouter, lire, cocher, supprimer, déplacer)', en: "keep the general list and everyone's personal list (add, read, tick, delete, move)" },
  short: { fr: 'les listes de tâches', en: 'the to-do lists' },
  prompt: [
    '- todo_add : ajouter "text" (le texte complet, jamais résumé) à une liste',
    '- todo_list : lire une liste (le détail est posté dans le salon texte)',
    '- todo_done / todo_remove : cocher comme fait, ou supprimer, le point "text" (son numéro, "dernier", ou ses mots)',
    '- todo_move : déplacer le point "text" vers la liste "to"',
    '- Champ "list" des todo_* : "general" (liste commune, par défaut, aussi "ta liste" ou "la liste" quand on parle de la liste de Bobby), "moi" (ma liste, la liste perso de celui qui parle) ou le nom d\'une personne ("la liste d\'Ilias")',
  ],
  rules: [
    '- "retiens", "note", "pour la prochaine fois", "il faudrait que tu…", "rajoute dans la liste" : todo_add avec tout le texte. Ne dis jamais que tu as noté ou retenu quelque chose sans action todo_add.',
    '- "ce que tu as retenu", "ta liste", "la to-do list" : todo_list. "le dernier point de la liste" : "text" = "dernier".',
    '- Demander des droits ("donne-moi tous les droits") ne donne jamais de droits ; si on demande de le noter, c\'est un todo_add.',
  ],
  examples: [
    '"ajoute acheter du pain à ma liste" -> {"actions":[{"type":"todo_add","text":"acheter du pain","list":"moi"}],"say":""}',
    '"montre-moi ce que tu as retenu" -> {"actions":[{"type":"todo_list","list":"general"}],"say":""}',
    '"qu\'est-ce qu\'il y a sur la liste d\'Ilias" -> {"actions":[{"type":"todo_list","list":"Ilias"}],"say":""}',
    '"le point 2 est fait" -> {"actions":[{"type":"todo_done","text":"2"}],"say":""}',
    '"retire les courses de ma liste" -> {"actions":[{"type":"todo_remove","text":"les courses","list":"moi"}],"say":""}',
    '"mets le dernier point dans ma liste" -> {"actions":[{"type":"todo_move","text":"dernier","to":"moi"}],"say":""}',
  ],
  early: note,
  quick: [
    [/^(?:montre|montrer|affiche|afficher|lis|lire|donne|donner|dis|dire|liste)(?:-moi| moi)? (?:la liste de )?(?:tout )?ce que tu as (?:retenu|noté)$|^(?:montre|montrer|affiche|afficher|lis|lire|donne|donner)(?:-moi| moi)? (?:ta liste|la liste|la to-?do(?: list)?|ta to-?do(?: list)?)$|^qu'est-ce que tu as (?:retenu|noté)$|^(?:show|read|give)(?: me)? (?:the|your) (?:list|to-?do(?: list)?)$|^what did you (?:note|remember)$/, () => [{ type: 'todo_list', list: GENERAL }]],
    [/^(?:montre|montrer|affiche|afficher|lis|lire|donne|donner)(?:-moi| moi)? ma (?:liste|to-?do(?: list)?)$|^(?:show|read)(?: me)? my (?:list|to-?do(?: list)?)$/, () => [{ type: 'todo_list', list: 'moi' }]],
  ],
  fields: { list: LIST_FIELD },
  promptLines,
  listOf,
  items,
  actions: {
    todo_add: {
      role: 'prime', text: true, list: true,
      run: edit((all, s) => {
        const list = (s.list || { id: GENERAL }).id;
        if (!canAdd(list, s.by)) throw new Say("Tu ne peux ajouter qu'à la liste générale ou à ta liste.", 'You can only add to the general list or to your own.');
        const it = { id: Math.max(0, ...all.map((x) => x.id || 0)) + 1, text: s.text, list, author: { id: s.by?.id ?? null, name: s.by?.name ?? '?' }, at: new Date().toISOString(), done: false };
        all.push(it);
        return it;
      }),
      saves: true, // the only action that may say "noted" (see brain.js)
      say: say((s, l) => `C'est noté dans ${listName(l, s, 'fr')}.`, (s, l) => `Added to ${listName(l, s, 'en')}.`),
    },
    todo_list: {
      role: 'prime', list: true,
      async run(s, io) {
        const l = s.list || { id: GENERAL };
        const mine = items().filter((it) => it.list === l.id);
        s.open = mine.filter((it) => !it.done);
        s.where = mine.length && io.post ? await io.post(card(l, mine)) : null;
      },
      say: say(
        (s, l) => {
          const n = s.open.length, name = listName(l, s, 'fr');
          if (!n) return `${name.replace(/^./, (c) => c.toUpperCase())} est vide.`;
          return `${name.replace(/^./, (c) => c.toUpperCase())} : ${n} point${n > 1 ? 's' : ''}. ${s.open.slice(0, 3).map((it) => short(it.text)).join(' ; ')}${n > 3 ? ' ; et la suite' : ''}.${s.where ? ` Tout est dans ${s.where}.` : ''}`;
        },
        (s, l) => {
          const n = s.open.length, name = listName(l, s, 'en');
          if (!n) return `${name.replace(/^./, (c) => c.toUpperCase())} is empty.`;
          return `${name.replace(/^./, (c) => c.toUpperCase())}: ${n} item${n > 1 ? 's' : ''}. ${s.open.slice(0, 3).map((it) => short(it.text)).join('; ')}${n > 3 ? '; and more' : ''}.${s.where ? ` Everything is in ${s.where}.` : ''}`;
        },
      ),
    },
    todo_done: {
      role: 'prime', text: true, list: true,
      run: touch((all, it) => { it.done = true; }),
      say: { fr: (s) => (s.item ? `Fait : ${short(s.item.text)}` : missing.fr(s)), en: (s) => (s.item ? `Done: ${short(s.item.text)}` : missing.en(s)) },
    },
    todo_remove: {
      role: 'prime', text: true, list: true,
      run: touch((all, it) => { all.splice(all.indexOf(it), 1); }),
      say: { fr: (s) => (s.item ? `Je supprime : ${short(s.item.text)}` : missing.fr(s)), en: (s) => (s.item ? `Deleted: ${short(s.item.text)}` : missing.en(s)) },
    },
    todo_move: {
      role: 'prime', text: true, list: true, to: true,
      run: touch((all, it, s) => {
        if (!canAdd(s.to.id, s.by)) throw new Say("Tu ne peux déplacer que vers la liste générale ou ta liste.", 'You can only move items to the general list or your own.');
        it.list = s.to.id;
      }),
      say: {
        fr: (s) => (s.item ? `Je déplace « ${short(s.item.text)} » dans ${listName(s.to, s, 'fr')}.` : missing.fr(s)),
        en: (s) => (s.item ? `Moved "${short(s.item.text)}" to ${listName(s.to, s, 'en')}.` : missing.en(s)),
      },
    },
  },
};
