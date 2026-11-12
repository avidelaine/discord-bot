// Rights managed by voice, super admin only: blacklist someone, give them back their rights, grant or remove admin.
// Anyone with a role may ask who has which rights (answered from live state). Nobody, the super admin included,
// can make or unmake a super admin by voice: that is only SUPER_ADMINS in .env, edited by hand.
const roles = require('../roles');
const { perms, savePerms, isSuper } = roles;
const { Say } = require('../util');

const without = (l, id) => l.filter((x) => x !== id);
const SUPER_ONLY_BY_HAND = () => new Say(
  'Le super admin ne se change pas à la voix : seulement à la main, dans le fichier .env.',
  'The super admin cannot be changed by voice: only by hand, in the .env file.',
);

// edit(p, id) changes the permission lists in place; undo restores the previous file.
const change = (edit) => (s) => {
  const id = s.targets[0].id;
  if (isSuper(id)) throw new Say('On ne touche pas aux droits du super admin.', "The super admin's rights can't be changed.");
  const p = perms();
  const before = JSON.stringify(p);
  edit(p, id);
  savePerms(p);
  return () => savePerms(JSON.parse(before));
};

const line = (fr, en) => ({ fr: (s, n) => fr(n), en: (s, n) => en(n) });

// "X est admin : ...", from the role X has right now and the roles the action modules declare.
function rightsOf(s, n, lang) {
  const info = require('./info');
  const t = s.targets[0];
  const role = roles.roleOf(t.ref || { id: t.id });
  const can = info.abilities(role, lang);
  if (lang === 'en') {
    if (role === 'other') return perms().blacklist.includes(t.id) ? `${n} is blacklisted: I don't answer them.` : `${n} has no role: I don't answer them.`;
    return `${n} is ${role === 'super' ? 'super admin, with every right' : role === 'admin' ? 'an admin' : 'Prime'}: ${can}.`;
  }
  if (role === 'other') return perms().blacklist.includes(t.id) ? `${n} est sur liste noire : je ne lui réponds pas.` : `${n} n'a aucun rôle : je ne lui réponds pas.`;
  return `${n} est ${role === 'super' ? 'super admin, avec tous les droits' : role === 'admin' ? 'admin' : 'Prime'} : ${can}.`;
}

module.exports = {
  help: { fr: 'gérer les droits de chacun et dire qui a quels droits', en: "manage everyone's rights and tell who has which rights" },
  short: { fr: 'les droits', en: 'the rights' },
  prompt: [
    '- perm_revoke / perm_restore : enlever ou redonner à "who" le droit de donner des ordres à Bobby',
    '- admin_add / admin_remove : passer "who" admin, ou lui retirer l\'admin',
    '- perm_show : dire quels droits a "who" (une seule personne)',
    '- super_admin : nommer quelqu\'un super admin, ou retirer le super admin (toujours refusé)',
  ],
  rules: [
    '- "super admin" dans la demande de changement de droits : super_admin, jamais admin_add ni admin_remove.',
    '- perm_show seulement pour une personne nommée. "Qui est admin", "qui a le droit de…" sans personne précise : about.',
  ],
  examples: [
    '"enlève les permissions de Lucas" -> {"actions":[{"type":"perm_revoke","who":["Lucas"]}],"say":""}',
    '"passe Marie admin" -> {"actions":[{"type":"admin_add","who":["Marie"]}],"say":""}',
    '"explique à Lucas quels sont ses droits" -> {"actions":[{"type":"perm_show","who":["Lucas"]}],"say":""}',
    '"passe Kévin super admin" -> {"actions":[{"type":"super_admin"}],"say":""}',
  ],
  quick: [
    [/\bsuper[\s-]?admin\b/, (m) => (/^(?:passe|mets|nomme|fais|rends|retire|enlève|enleve|supprime|make|remove|promote|demote|donne)\b/.test(m.input) ? [{ type: 'super_admin' }] : null)],
    [/^(?:enlève|enleve|retire|supprime) (?:les |ses |tous les )?(?:permissions|droits) (?:de|à|a|du) (.+)$/, (m) => [{ type: 'perm_revoke', who: [m[1]] }]],
    [/^(?:redonne|rends|remets) (?:les |ses )?(?:permissions|droits) (?:de|à|a|au) (.+)$/, (m) => [{ type: 'perm_restore', who: [m[1]] }]],
    [/^(?:passe|mets|nomme) (.+?) admin$/, (m) => [{ type: 'admin_add', who: [m[1]] }]],
    [/^(?:retire|enlève|enleve) (?:l'|le rôle )?admin (?:de|à|a|au) (.+)$/, (m) => [{ type: 'admin_remove', who: [m[1]] }]],
    [/^(?:quels sont |c'est quoi )?(?:les )?(?:droits|permissions) (?:de |d'|du )(.+)$|^quels droits (?:a|as) (.+)$|^what are (.+?)'s (?:rights|permissions)$/, (m) => [{ type: 'perm_show', who: [m[1] || m[2] || m[3]] }]],
  ],
  actions: {
    perm_revoke: {
      role: 'super', who: true, one: true,
      run: change((p, id) => { p.blacklist = [...without(p.blacklist, id), id]; }),
      say: line((n) => `${n} n'a plus le droit de me parler.`, (n) => `${n} can no longer give me orders.`),
    },
    perm_restore: {
      role: 'super', who: true, one: true,
      run: change((p, id) => { p.blacklist = without(p.blacklist, id); }),
      say: line((n) => `${n} retrouve ses droits.`, (n) => `${n} has their rights back.`),
    },
    admin_add: {
      role: 'super', who: true, one: true,
      run: change((p, id) => { p.admins = [...without(p.admins, id), id]; p.notAdmins = without(p.notAdmins, id); }),
      say: line((n) => `${n} est admin.`, (n) => `${n} is now an admin.`),
    },
    admin_remove: {
      role: 'super', who: true, one: true,
      run: change((p, id) => { p.admins = without(p.admins, id); p.notAdmins = [...without(p.notAdmins, id), id]; }),
      say: line((n) => `${n} n'est plus admin.`, (n) => `${n} is no longer an admin.`),
    },
    perm_show: {
      role: 'prime', info: true, who: true, one: true,
      run() {},
      say: { fr: (s, n) => rightsOf(s, n, 'fr'), en: (s, n) => rightsOf(s, n, 'en') },
    },
    super_admin: {
      role: 'prime', info: true, // whoever asks gets the same answer
      run() { throw SUPER_ONLY_BY_HAND(); },
      say: {},
    },
  },
};
