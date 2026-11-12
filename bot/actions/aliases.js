// Nicknames given by voice ("appelle Zyklon le Grec"), stored in data/aliases.json (memberId -> [nickname]).
const { load, save } = require('../store');
const { norm } = require('../util');

const change = (add) => async (s) => {
  const all = load('aliases', {});
  const before = JSON.stringify(all);
  const id = s.targets[0].id, list = all[id] || [];
  const i = list.findIndex((x) => norm(x) === norm(s.text));
  if (add && i < 0) list.push(s.text);
  if (!add) { s.missing = i < 0; if (i >= 0) list.splice(i, 1); }
  all[id] = list;
  save('aliases', all);
  return () => save('aliases', JSON.parse(before));
};

module.exports = {
  help: { fr: 'retenir des surnoms', en: 'remember nicknames' },
  prompt: ['- alias_add / alias_remove : donner ou retirer à "who" (une seule personne) le surnom "text"'],
  rules: ['- Chaque personne peut avoir des surnoms (listés dans le contexte) : un surnom désigne cette personne. "renommer X en Y", c\'est alias_add.'],
  examples: [
    '"appelle Zyklon le Grec" -> {"actions":[{"type":"alias_add","who":["Zyklon"],"text":"le Grec"}],"say":""}',
    '"Kévin ne s\'appelle plus Kev" -> {"actions":[{"type":"alias_remove","who":["Kévin"],"text":"Kev"}],"say":""}',
  ],
  quick: [],
  actions: {
    alias_add: {
      role: 'admin', who: true, one: true, text: true, run: change(true),
      say: { fr: (s, n) => `${n} s'appelle aussi « ${s.text} ».`, en: (s, n) => `${n} can now also be called "${s.text}".` },
    },
    alias_remove: {
      role: 'admin', who: true, one: true, text: true, run: change(false),
      say: {
        fr: (s, n) => (s.missing ? `${n} n'avait pas le surnom « ${s.text} ».` : `J'oublie « ${s.text} » pour ${n}.`),
        en: (s, n) => (s.missing ? `${n} had no nickname "${s.text}".` : `Forgetting "${s.text}" for ${n}.`),
      },
    },
  },
};
