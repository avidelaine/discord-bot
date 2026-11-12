// Who may ask Bobby what. Each action module declares the role its actions need ("role"); this file
// decides which role a person has. Levels: other < prime < admin < super.
// IDs and the role name live in .env; changes made by voice live in data/permissions.json.
const { load, save } = require('./store');

const LEVEL = { other: 0, prime: 1, admin: 2, super: 3 };
const ids = (v) => (v || '').split(',').map((s) => s.trim()).filter(Boolean);
const env = (k, d) => process.env[k] ?? d;

const SUPER = () => ids(env('SUPER_ADMINS', '261531632589078528')); // WakandeX
const ADMINS = () => ids(env('ADMINS', '397478613559148545')); // iisal_ (super admins are admins too)
const PRIME_ROLE = () => env('PRIME_ROLE', 'Team Prime'); // Discord role name or ID

// blacklist: falls to "other" whatever their Discord role; admins / notAdmins: admin granted or removed by voice.
const perms = () => ({ blacklist: [], admins: [], notAdmins: [], ...load('permissions', {}) });
const savePerms = (p) => save('permissions', p);

function roleOf(member) {
  const id = member.id;
  if (SUPER().includes(id)) return 'super';
  const p = perms();
  if (p.blacklist.includes(id)) return 'other';
  if ((ADMINS().includes(id) || p.admins.includes(id)) && !p.notAdmins.includes(id)) return 'admin';
  const role = PRIME_ROLE();
  if (member.roles?.cache?.some((r) => r.name === role || r.id === role)) return 'prime';
  return 'other';
}

const allowed = (role, need) => LEVEL[role] >= LEVEL[need || 'admin'];
const isSuper = (id) => SUPER().includes(id);

module.exports = { roleOf, allowed, isSuper, perms, savePerms, LEVEL };
