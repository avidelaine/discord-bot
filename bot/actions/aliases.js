// Nicknames given by voice ("appelle Zyklon le Grec"), stored in data/aliases.json (memberId -> [nickname]).
// Everything here is super admin only.
const { load, save } = require('../store');
const { norm } = require('../util');

const has = (list, text) => (list || []).findIndex((x) => norm(x) === norm(text));

// Runs fn(all, step) on the stored nicknames; fn returns the id of the person it changed (none: nickname not found).
// Undo restores the previous file content.
const edit = (fn) => async (s, io) => {
  const all = load('aliases', {});
  const before = JSON.stringify(all);
  const id = fn(all, s);
  s.missing = !id;
  if (id && !s.targets) s.owner = await io.memberName(id);
  for (const k of Object.keys(all)) if (!all[k].length) delete all[k];
  save('aliases', all);
  return () => save('aliases', JSON.parse(before));
};

// Person who has nickname s.text: the one named in the sentence, else anyone.
const owner = (all, s) => (s.targets ? [s.targets[0].id] : Object.keys(all)).find((id) => has(all[id], s.text) >= 0);

// "**Name** : nick1, nick2", one line per person, sorted by name. People may have left voice: names come from io.
async function listText(io) {
  const all = load('aliases', {});
  const rows = await Promise.all(Object.entries(all).filter(([, l]) => l.length).map(async ([id, l]) => [await io.memberName(id), l]));
  return rows.sort((a, b) => a[0].localeCompare(b[0])).map(([n, l]) => `**${n}** : ${l.join(', ')}`).join('\n');
}

const who = (s, n) => n || s.owner;
const notFound = { fr: (s) => `Je ne trouve pas le surnom « ${s.text} ».`, en: (s) => `I can't find the nickname "${s.text}".` };

module.exports = {
  help: { fr: 'retenir, renommer, supprimer et lister les surnoms', en: 'remember, rename, delete and list nicknames' },
  prompt: [
    '- alias_add : donner à "who" (une seule personne) le surnom "text"',
    '- alias_remove : retirer le surnom "text" ("who" facultatif : sans "who", on cherche chez tout le monde)',
    '- alias_rename : remplacer un surnom existant "text" par "to" ("who" facultatif). Jamais sans les mots "le surnom" (ou "the nickname") dans la phrase.',
    '- alias_list : poster dans le salon texte la liste des surnoms de chaque personne',
  ],
  rules: [
    '- Chaque personne peut avoir des surnoms (listés dans le contexte) : un surnom désigne cette personne. "renommer X en Y" avec une personne X, c\'est alias_add ; "renomme le surnom X en Y", c\'est alias_rename. "X ne s\'appelle plus Y", c\'est alias_remove.',
  ],
  examples: [
    '"appelle Zyklon le Grec" -> {"actions":[{"type":"alias_add","who":["Zyklon"],"text":"le Grec"}],"say":""}',
    '"Kévin ne s\'appelle plus Kev" -> {"actions":[{"type":"alias_remove","who":["Kévin"],"text":"Kev"}],"say":""}',
    '"renomme Lucas en le Boss" -> {"actions":[{"type":"alias_add","who":["Lucas"],"text":"le Boss"}],"say":""}',
    '"tu peux renommer Marie en la Reine ?" -> {"actions":[{"type":"alias_add","who":["Marie"],"text":"la Reine"}],"say":""}',
    '"coupe le micro du Rat" (le Rat = surnom de Lucas) -> {"actions":[{"type":"mute","who":["Lucas"]}],"say":""}',
    '"renomme le surnom le Rat en le Spartiate" -> {"actions":[{"type":"alias_rename","text":"le Rat","to":"le Spartiate"}],"say":""}',
    '"delete the nickname the rookie" -> {"actions":[{"type":"alias_remove","text":"the rookie"}],"say":""}',
    '"qui a quel surnom" -> {"actions":[{"type":"alias_list"}],"say":""}',
  ],
  // ponytail: shortcuts see the lowercased sentence, so nicknames set through them are stored in lowercase.
  quick: [
    [/^(?:qui a quels? surnoms?|(?:liste|montre|affiche|donne)(?:-moi)? (?:les|tous les|la liste des) surnoms|who has which nicknames?|(?:list|show)(?: all)?(?: the)? nicknames)$/, () => [{ type: 'alias_list' }]],
    [/^renommer? (?!le surnom )(.+?) en (.+)$/, (m) => [{ type: 'alias_add', who: [m[1]], text: m[2] }]],
    [/^renomme le surnom (.+?) en (.+)$/, (m) => [{ type: 'alias_rename', text: m[1], to: m[2] }]],
    [/^rename(?: the)? nickname (.+?) (?:to|into) (.+)$/, (m) => [{ type: 'alias_rename', text: m[1], to: m[2] }]],
    [/^(?:supprime|efface|retire|enlève) le surnom (.+)$/, (m) => [{ type: 'alias_remove', text: m[1] }]],
    [/^(?:delete|remove) (?:the )?nickname (.+)$/, (m) => [{ type: 'alias_remove', text: m[1] }]],
  ],
  fields: { to: { type: 'string' } }, // merged into the LLM schema: new nickname of alias_rename
  listText,
  actions: {
    alias_add: {
      role: 'super', who: true, one: true, text: true,
      run: edit((all, s) => {
        const id = s.targets[0].id;
        all[id] ||= [];
        if (has(all[id], s.text) < 0) all[id].push(s.text);
        return id;
      }),
      say: { fr: (s, n) => `${n} s'appelle aussi « ${s.text} ».`, en: (s, n) => `${n} can now also be called "${s.text}".` },
    },
    alias_remove: {
      role: 'super', who: 'optional', one: true, text: true,
      run: edit((all, s) => {
        const id = owner(all, s);
        if (id) all[id].splice(has(all[id], s.text), 1);
        return id;
      }),
      say: {
        fr: (s, n) => (s.missing ? notFound.fr(s) : `J'oublie « ${s.text} » pour ${who(s, n)}.`),
        en: (s, n) => (s.missing ? notFound.en(s) : `Forgetting "${s.text}" for ${who(s, n)}.`),
      },
    },
    alias_rename: {
      role: 'super', who: 'optional', one: true, text: true,
      // "renomme Lucas en le Boss": the 4B model sometimes answers rename with text = to; that means giving a nickname.
      normalize: (a) => (a.who?.length && (!a.to || norm(a.to) === norm(a.text)) ? { type: 'alias_add', who: a.who, text: a.to || a.text } : null),
      run: edit((all, s) => {
        if (!s.to) return null;
        const id = owner(all, s);
        if (id) all[id][has(all[id], s.text)] = s.to;
        return id;
      }),
      say: {
        fr: (s, n) => (s.missing ? notFound.fr(s) : `« ${s.text} » devient « ${s.to} » pour ${who(s, n)}.`),
        en: (s, n) => (s.missing ? notFound.en(s) : `"${s.text}" is now "${s.to}" for ${who(s, n)}.`),
      },
    },
    alias_list: {
      role: 'super',
      async run(s, io) {
        const text = await listText(io);
        s.where = text && (await io.post(text));
      },
      say: {
        fr: (s) => (s.where ? `Je l'ai mis dans ${s.where}.` : "Personne n'a de surnom."),
        en: (s) => (s.where ? `I put it in ${s.where}.` : 'Nobody has a nickname.'),
      },
    },
  },
};
