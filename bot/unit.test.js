// Fast tests, no LLM and no Discord: names, shortcuts, cancelling, rights, undo.
// Run: node bot/unit.test.js
const assert = require('assert');
const os = require('os');
const path = require('path');
const fs = require('fs');

process.env.DATA_DIR = fs.mkdtempSync(path.join(os.tmpdir(), 'bobby-test-')); // never touch the real data/
const { resolve, quick, cancels, pick, sim, describe, run, needed } = require('./brain');
const roles = require('./roles');
const { load, save } = require('./store');
const ask = require('./actions/ask');

// ---------- Name resolution ----------
const roster = [{ name: 'Zyklon' }, { name: 'Kévin', aliases: ['kev_du_93'] }, { name: 'Marie' }, { name: 'Mario' }];
assert.strictEqual(pick('ziklone', roster, 'person').name, 'Zyklon');
assert.strictEqual(pick('kevin', roster, 'person').name, 'Kévin');
assert.strictEqual(pick('kev du 93', roster, 'person').name, 'Kévin');
assert.throws(() => pick('Mari', roster, 'person'), /tu veux dire/);
assert.throws(() => pick('Bernard', roster, 'person'), /Je ne trouve pas/);
assert(sim('🎮 Gaming', 'gaming') === 1);
console.log('name resolution: ok');

const ctx = require('./test-context');

// ---------- LLM-free shortcuts: what they take must be right, what they leave goes to the LLM (null) ----------
const fmt = (plan) => plan.map((s) => `${s.type}:${(s.targets || []).map((t) => t.name).sort().join(',')}${s.channel ? `>${s.channel.name}` : ''}`).join(' | ');
const QUICK_CASES = [
  ['Coupe le micro de Kévin.', 'mute:Kévin'],
  ['mute ziklone', 'mute:Zyklon'],
  ['rends le micro à Kevin', 'unmute:Kévin'],
  ['coupe le son de Marie', 'deafen:Marie'],
  ['déplace Zyklon dans gaming', 'move:Zyklon>Gaming'],
  ['envoie Lucas en AFK', 'move:Lucas>AFK'],
  ['mets-moi dans la salle de réunion', 'move:Alex>Réunion'],
  ['annule', 'undo:'],
  ['coupe le micro de Zyklon et Kévin', null],
  ['déplace Kévin dans AFK et coupe son micro', null],
  ['raconte-moi une blague', null],
  ['Mute Zyklon.', 'mute:Zyklon'],
  ['deafen marie', 'deafen:Marie'],
  ['move Lucas to the AFK channel', 'move:Lucas>AFK'],
  ['move me to gaming', 'move:Alex>Gaming'],
  ['undo that', 'undo:'],
  ['mute Zyklon and Kevin', null],
  ['mute le grec', 'mute:Zyklon'],
  // Missed on 2026-11-12 (journal): polite fillers, "dé-mute", "draguer"
  ['est-ce que tu peux kick Sorciax s\'il te plaît ?', 'disconnect:Sorciax'],
  ['des mute Ilias', 'unmute:Prime Ilias'],
  ['dé-mute WakandeX', 'unmute:WakandeX'],
  ['démute Kévin', 'unmute:Kévin'],
  ['peux-tu draguer Sorciax ?', 'move:Sorciax>Général'],
  ['est-ce que tu peux draguer Vinice ?', 'move:Sorciax>Général'],
  ['vas-y mute moi', 'mute:Alex'],
  ['can you mute Prime Ilias please?', 'mute:Prime Ilias'],
  ['Mute Bobby', null],
  ['quelle heure est-il ?', 'time:'],
  ['il est quelle heure ?', 'time:'],
  ['est-ce que tu peux me lister les features qu\'on t\'a déjà implémentées ?', 'help:'],
  ["qu'est-ce que tu sais faire", 'help:'],
  ['est-ce que tu peux me mute micro et casque s\'il te plaît ?', null],
  // Rights by voice (super admin)
  ['enlève les permissions de Lucas', 'perm_revoke:Lucas'],
  ['redonne les permissions à Lucas', 'perm_restore:Lucas'],
  ['passe Marie admin', 'admin_add:Marie'],
  ["retire l'admin à Marie", 'admin_remove:Marie'],
];
for (const [phrase, want] of QUICK_CASES) {
  const a = quick(phrase);
  let got = null;
  try { got = a && fmt(resolve(a, ctx)); } catch { got = null; } // name not found: falls back to the LLM
  assert.strictEqual(got, want, `shortcut: "${phrase}"`);
}
console.log('LLM-free shortcuts: ok');

// ---------- A sentence ending in "annule" cancels the whole request ----------
assert(cancels('est-ce que tu peux mettre un surnom sur Sorciax et que ça... Bobby, Godmode, annule !'));
assert(cancels('mute Zyklon, laisse tomber'));
assert(cancels('move Lucas to gaming, never mind'));
assert(!cancels('annule'));
assert(!cancels('non annule'));
assert(!cancels('laisse tomber, reviens en arrière'));
assert(!cancels('annule ça'));
console.log('cancel: ok');

// ---------- Rights ----------
const member = (id, roleNames = []) => ({ id, roles: { cache: roleNames.map((name) => ({ name, id: `r-${name}` })) } });
const WAKANDEX = '261531632589078528', IISAL = '397478613559148545';
assert.strictEqual(roles.roleOf(member(WAKANDEX)), 'super');
assert.strictEqual(roles.roleOf(member(IISAL)), 'admin');
assert.strictEqual(roles.roleOf(member('42', ['Team Prime'])), 'prime');
assert.strictEqual(roles.roleOf(member('43', ['Membre'])), 'other');
const plan = (sentence) => resolve(quick(sentence), ctx);
assert.strictEqual(needed(plan('mute Zyklon')), 'admin');
assert.strictEqual(needed(plan('quelle heure est-il')), 'prime');
assert.strictEqual(needed(plan('passe Marie admin')), 'super');
assert(roles.allowed('prime', needed(plan('quelle heure est-il'))));
assert(!roles.allowed('prime', needed(plan('mute Zyklon'))));
assert(!roles.allowed('admin', needed(plan('passe Marie admin'))));

(async () => {
  // Voice-managed rights, persisted in data/permissions.json; blacklist beats admin and Team Prime.
  const io = { leave() {} };
  const ilias = ctx.members.find((m) => m.name === 'Prime Ilias'); // id 397478613559148545 in the test context
  await run(plan('enlève les permissions de Ilias'), io);
  assert.strictEqual(roles.roleOf(member(ilias.id, ['Team Prime'])), 'other');
  await run(plan('redonne les permissions à Ilias'), io);
  assert.strictEqual(roles.roleOf(member(ilias.id)), 'admin');
  await run(plan("retire l'admin à Ilias"), io);
  assert.strictEqual(roles.roleOf(member(ilias.id, ['Team Prime'])), 'prime');
  await run(plan('passe Ilias admin'), io);
  assert.strictEqual(roles.roleOf(member(ilias.id)), 'admin');
  await run(plan('passe Lucas admin'), io);
  assert.strictEqual(roles.roleOf(member('lucas')), 'admin');
  await run(plan('annule'), io); // undo also covers rights
  assert.strictEqual(roles.roleOf(member('lucas')), 'other');
  await assert.rejects(run(plan('enlève les permissions de WakandeX'), io), /super admin/);
  assert.strictEqual(roles.roleOf(member(WAKANDEX)), 'super');
  console.log('rights: ok');

  // Undo covers nicknames and the to-do list
  await run(resolve([{ type: 'alias_add', who: ['Kévin'], text: 'le Boss' }], ctx), io);
  assert.deepStrictEqual(load('aliases', {}).Kévin, ['le Boss']);
  await run(plan('annule'), io);
  assert.deepStrictEqual(load('aliases', {}).Kévin, undefined);
  save('todo', ['pain']);
  await run(resolve([{ type: 'todo_add', text: 'lait' }], ctx), io);
  await run(resolve([{ type: 'todo_remove', text: 'pain' }], ctx), io);
  assert.deepStrictEqual(load('todo', []), ['lait']);
  await run(plan('annule'), io);
  assert.deepStrictEqual(load('todo', []), ['pain', 'lait']);
  console.log('undo: ok');

  // Claude not logged in: immediate answer, no wait
  ask.setReady(false);
  const q = resolve([{ type: 'ask', text: 'Quelle est la capitale du Congo ?' }], ctx);
  await run(q, io);
  assert.strictEqual(describe(q), "Claude n'est pas encore branché.");
  assert.match(describe(plan('qu\'est-ce que tu sais faire')), /^Je sais .*couper/);
  console.log('claude off + help: ok');
})().catch((e) => { console.error(e); process.exit(1); });
