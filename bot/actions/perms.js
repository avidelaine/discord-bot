// Rights managed by voice, super admin only: blacklist someone, give them back their rights, grant or remove admin.
const { perms, savePerms, isSuper } = require('../roles');
const { Say } = require('../util');

const without = (l, id) => l.filter((x) => x !== id);

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

module.exports = {
  help: { fr: 'gérer les droits de chacun', en: "manage everyone's rights" },
  prompt: [
    '- perm_revoke / perm_restore : enlever ou redonner à "who" le droit de donner des ordres à Bobby',
    '- admin_add / admin_remove : passer "who" admin, ou lui retirer l\'admin',
  ],
  rules: [],
  examples: [
    '"enlève les permissions de Lucas" -> {"actions":[{"type":"perm_revoke","who":["Lucas"]}],"say":""}',
    '"passe Marie admin" -> {"actions":[{"type":"admin_add","who":["Marie"]}],"say":""}',
  ],
  quick: [
    [/^(?:enlève|enleve|retire|supprime) (?:les |ses |tous les )?(?:permissions|droits) (?:de|à|a|du) (.+)$/, (m) => [{ type: 'perm_revoke', who: [m[1]] }]],
    [/^(?:redonne|rends|remets) (?:les |ses )?(?:permissions|droits) (?:de|à|a|au) (.+)$/, (m) => [{ type: 'perm_restore', who: [m[1]] }]],
    [/^(?:passe|mets|nomme) (.+?) admin$/, (m) => [{ type: 'admin_add', who: [m[1]] }]],
    [/^(?:retire|enlève|enleve) (?:l'|le rôle )?admin (?:de|à|a|au) (.+)$/, (m) => [{ type: 'admin_remove', who: [m[1]] }]],
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
  },
};
