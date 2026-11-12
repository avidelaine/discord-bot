// Fast tests, no LLM and no Discord: names, shortcuts, cancelling, rights, undo.
// Run: node bot/unit.test.js
const assert = require('assert');
const os = require('os');
const path = require('path');
const fs = require('fs');

process.env.DATA_DIR = fs.mkdtempSync(path.join(os.tmpdir(), 'bobby-test-')); // never touch the real data/
process.env.BOBBY_PC_DRY = '1'; // PC actions only log: no sleep, lock, launch or volume change during tests
const { resolve, quick, cancels, pick, sim, describe, run, needed } = require('./brain');
const roles = require('./roles');
const { load, save } = require('./store');
const ask = require('./actions/ask');
const pc = require('./actions/pc');

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
  // Nicknames
  ['qui a quel surnom ?', 'alias_list:'],
  ['who has which nicknames', 'alias_list:'],
  ['renomme le surnom le Grec en le Spartiate', 'alias_rename:'],
  ['rename the nickname le grec to the spartan', 'alias_rename:'],
  ['supprime le surnom le Grec', 'alias_remove:'],
  ['delete the nickname le grec', 'alias_remove:'],
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

// ---------- Confirmation: a short yes, never a sentence that only contains "ok" ----------
const { confirms } = require('./brain');
for (const t of ['oui', 'Oui !', 'je confirme', 'ok vas-y', 'yes', 'Bobby, oui']) assert(confirms(t), t);
for (const t of ['non, pas ok', 'ok oui go mais attends', "tu crois que c'est ok de faire ça demain soir", 'non', 'oui non annule']) assert(!confirms(t), t);
console.log('confirm: ok');

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
for (const sentence of ['qui a quel surnom', 'renomme le surnom le grec en le boss', 'supprime le surnom le grec']) {
  assert.strictEqual(needed(plan(sentence)), 'super');
  assert(!roles.allowed('prime', needed(plan(sentence))) && !roles.allowed('admin', needed(plan(sentence))));
  assert(roles.allowed('super', needed(plan(sentence))));
}
assert.strictEqual(needed(resolve([{ type: 'alias_add', who: ['Kévin'], text: 'x' }], ctx)), 'super');

// ---------- PC control: shortcuts, rights, confirmations, dry run ----------
const PC_CASES = [
  ['mets pause sur le PC', 'pc_media', 'pause'], ['Mets la musique en pause sur Spotify.', 'pc_media', 'pause'], ['pause sur le PC', 'pc_media', 'pause'],
  ['pause the music on Spotify', 'pc_media', 'pause'], ['reprends la musique sur le PC', 'pc_media', 'play'],
  ['musique suivante sur Spotify', 'pc_media', 'next'], ['next song on youtube music', 'pc_media', 'next'], ['musique précédente sur le PC', 'pc_media', 'previous'],
  ["c'est quoi la musique sur le PC", 'pc_music', ''], ["what's playing on the PC", 'pc_music', ''],
  ['monte le son du PC', 'pc_volume', 'up'], ['baisse le volume Windows', 'pc_volume', 'down'], ['mets le son du PC à 30', 'pc_volume', '30'],
  ['set the PC volume to 40 percent', 'pc_volume', '40'], ['coupe le son du PC', 'pc_volume', 'mute'], ['remets le son du PC', 'pc_volume', 'unmute'],
  ['lance Valorant', 'pc_launch', 'valorant'], ['ouvre Claude', 'pc_launch', 'claude'], ['open ChatGPT', 'pc_launch', 'chatgpt'],
  ['verrouille le PC', 'pc_lock', ''], ['mets le PC en veille', 'pc_sleep', ''], ["éteins l'ordinateur", 'pc_shutdown', ''],
  ['shut down the PC', 'pc_shutdown', ''], ['température du GPU', 'pc_gpu', ''], ['how hot is the gpu', 'pc_gpu', ''],
];
for (const [phrase, type, text] of PC_CASES) {
  const a = quick(phrase);
  assert(a && a.length === 1 && a[0].type === type && a[0].text === text, `PC shortcut: "${phrase}" -> ${JSON.stringify(a)}`);
}
// Bobby's own Discord player will own these; the PC shortcuts must not take them.
for (const phrase of ['pause', 'skip', 'suivant', 'volume 50', 'monte le son', 'lance de la musique', 'play despacito', 'ouvre un truc inconnu xyz']) {
  assert(!quick(phrase)?.some((s) => s.type.startsWith('pc_')), `not a PC shortcut: "${phrase}"`);
}
for (const phrase of ['mets pause sur le PC', 'monte le son du PC', 'lance Valorant', 'mets le PC en veille', 'éteins le PC', 'verrouille le PC', 'température du GPU']) {
  const p = plan(phrase);
  assert.strictEqual(needed(p), 'super');
  assert(!roles.allowed('admin', needed(p)) && !roles.allowed('prime', needed(p)) && roles.allowed('super', needed(p)));
}
for (const t of ['pc_lock', 'pc_sleep', 'pc_shutdown']) assert.strictEqual(pc.actions[t].confirm, true, `${t} needs confirm`);
for (const t of ['pc_media', 'pc_volume', 'pc_launch', 'pc_gpu']) assert(!pc.actions[t].confirm);
const idx = pc._test.index();
const kinds = idx.reduce((o, e) => ({ ...o, [e.kind]: (o[e.kind] || 0) + 1 }), {});
console.log('PC index:', JSON.stringify(kinds));
console.log('PC shortcuts and rights: ok');

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
  // Nicknames: list (sorted by name, people out of voice included), rename, delete, each undoable
  const posts = [];
  const nio = { memberName: async (id) => ({ b: 'Zoé', a: 'Ana' })[id] || id, post: async (t) => { posts.push(t); return 'dev-bot-test'; } };
  save('aliases', { b: ['le Grec', 'Zo'], a: ['la Reine'], c: [] });
  const lst = plan('qui a quel surnom');
  await run(lst, nio);
  assert.deepStrictEqual(posts, ['**Ana** : la Reine\n**Zoé** : le Grec, Zo']);
  assert.strictEqual(describe(lst), "Je l'ai mis dans dev-bot-test.");
  assert.strictEqual(describe(lst, 'en'), 'I put it in dev-bot-test.');
  const rn = plan('renomme le surnom le grec en le Spartiate');
  await run(rn, nio);
  assert.deepStrictEqual(load('aliases', {}).b, ['le spartiate', 'Zo']); // shortcuts lowercase the sentence
  assert.strictEqual(describe(rn), '« le grec » devient « le spartiate » pour Zoé.');
  await run(plan('annule'), nio);
  assert.deepStrictEqual(load('aliases', {}).b, ['le Grec', 'Zo']);
  const del = plan('supprime le surnom la reine');
  await run(del, nio);
  assert.strictEqual(load('aliases', {}).a, undefined);
  assert.strictEqual(describe(del), "J'oublie « la reine » pour Ana.");
  await run(plan('annule'), nio);
  assert.deepStrictEqual(load('aliases', {}).a, ['la Reine']);
  const nope = plan('supprime le surnom inconnu');
  await run(nope, nio);
  assert.strictEqual(describe(nope), 'Je ne trouve pas le surnom « inconnu ».');
  const one = resolve([{ type: 'alias_remove', who: ['Zyklon'], text: 'le Grec' }], ctx); // with "who": only that person is searched
  await run(one, nio);
  assert.strictEqual(one[0].missing, true);
  save('aliases', {});
  save('todo', ['pain']);
  await run(resolve([{ type: 'todo_add', text: 'lait' }], ctx), io);
  await run(resolve([{ type: 'todo_remove', text: 'pain' }], ctx), io);
  assert.deepStrictEqual(load('todo', []), ['lait']);
  await run(plan('annule'), io);
  assert.deepStrictEqual(load('todo', []), ['pain', 'lait']);
  console.log('undo: ok');

  // PC actions in dry mode: only recorded, never executed
  const dry = async (sentence) => { pc._test.calls.length = 0; const p = plan(sentence); await run(p, io); return [pc._test.calls.map((c) => c.join(' ')).join(' ; '), describe(p)]; };
  assert.match((await dry('mets le PC en veille'))[0], /^powershell\.exe .*SetSuspendState/);
  assert.match((await dry('éteins le PC'))[0], /^shutdown\.exe \/s/);
  assert.match((await dry('verrouille le PC'))[0], /^rundll32\.exe user32\.dll,LockWorkStation/);
  assert.deepStrictEqual(await dry('mets le son du PC à 30'), ['POST /pc/volume?level=30', 'Volume du PC à 30.']);
  assert.strictEqual((await dry('musique suivante sur le PC'))[0], 'POST /pc/media/next');
  if (idx.some((e) => e.name === 'Valorant')) assert.match((await dry('lance Valorant'))[0], /RiotClientServices\.exe --launch-product=valorant --launch-patchline=live/);
  assert.match((await dry('ouvre Claude'))[0], /explorer\.exe/);
  console.log('PC dry run: ok');

  // Music: fake player, shortcuts, rights, spoken lines, no collision with the PC shortcuts
  const MUSIC_CASES = [
    ['pause', 'music_pause', ''], ['mets pause', 'music_pause', ''], ['mets la musique en pause', 'music_pause', ''],
    ['reprends la musique', 'music_resume', ''], ['resume', 'music_resume', ''], ['skip', 'music_skip', ''], ['suivant', 'music_skip', ''],
    ['musique suivante', 'music_skip', ''], ['next song', 'music_skip', ''], ['stop la musique', 'music_stop', ''], ['stop', 'music_stop', ''],
    ['rejoue', 'music_replay', ''], ['monte le son', 'music_volume', 'up'], ['baisse le volume', 'music_volume', 'down'],
    ['volume 50', 'music_volume', '50'], ['mets le volume à 40', 'music_volume', '40'], ['set the volume to 70', 'music_volume', '70'],
    ["c'est quoi ce son", 'music_now', ''], ["what's this song", 'music_now', ''], ["c'est quoi la file d'attente", 'music_queue', ''],
    ['lance de la musique', 'music_play', 'pop'], ['joue Bohemian Rhapsody', 'music_play', 'bohemian rhapsody'], ['play Daft Punk', 'music_play', 'daft punk'],
    ['joue du hardstyle', 'music_play', 'hardstyle'], ['mets une radio jazz', 'music_play', 'jazz'], ['play some lofi', 'music_play', 'lofi'],
  ];
  for (const [phrase, type, text] of MUSIC_CASES) {
    const a = quick(phrase);
    assert(a && a.length === 1 && a[0].type === type && a[0].text === text, `music shortcut: "${phrase}" -> ${JSON.stringify(a)}`);
  }
  assert.strictEqual(quick('joue du hardstyle')[0].kind, 'radio');
  assert.strictEqual(quick('joue Bohemian Rhapsody')[0].kind, 'youtube');
  for (const phrase of ['mets pause sur le PC', 'monte le son du PC', 'musique suivante sur Spotify', 'lance Valorant']) assert(!quick(phrase)?.some((s) => s.type.startsWith('music_')), `not music: "${phrase}"`);
  assert.strictEqual(quick('mets pause sur le PC')[0].type, 'pc_media');
  for (const phrase of ['pause', 'joue Despacito', 'volume 20']) {
    assert.strictEqual(needed(plan(phrase)), 'prime');
    assert(roles.allowed('prime', needed(plan(phrase))) && roles.allowed('admin', needed(plan(phrase))) && roles.allowed('super', needed(plan(phrase))));
    assert(!roles.allowed('other', needed(plan(phrase))));
  }
  const fake = {
    vol: 30, cur: { title: 'Bohemian Rhapsody', kind: 'youtube' }, queue: [{ label: 'Blinding Lights' }, { label: 'x' }], calls: [],
    playNow(i) { this.calls.push(i); return i.kind === 'radio' ? { title: 'Jazz FM', kind: 'radio', station: 'Jazz FM' } : { title: i.query, kind: 'youtube' }; },
    add(i) { this.calls.push(i); return { started: false, position: 3, label: i.query }; },
    pause() { return { paused: true }; }, resume() { return { error: 'nothing is playing' }; }, stop() { return { stopped: this.cur, cleared: 2 }; },
    skip() { return { skipped: this.cur, nowPlaying: { title: 'Blinding Lights' } }; }, replay() { return { nowPlaying: this.cur }; },
    list() { return { nowPlaying: this.cur, queue: this.queue }; }, nowPlaying() { return this.cur; },
    setVolume(n) { this.vol = n; return { volume: n }; }, volumeUp() { return this.setVolume(this.vol + 10); }, volumeDown() { return this.setVolume(this.vol - 10); },
  };
  const say = async (steps, lang = 'fr') => { const p = resolve(steps, ctx); await run(p, { music: fake }, lang); return describe(p, lang); };
  assert.strictEqual(await say([{ type: 'music_play', text: 'Bohemian Rhapsody', kind: 'youtube' }]), 'Je lance Bohemian Rhapsody.');
  assert.deepStrictEqual(fake.calls.pop(), { kind: 'youtube', query: 'Bohemian Rhapsody' });
  assert.strictEqual(await say([{ type: 'music_play', text: 'jazz', kind: 'radio' }]), 'Radio jazz : Jazz FM.');
  assert.deepStrictEqual(fake.calls.pop(), { kind: 'radio', style: 'jazz' });
  assert.strictEqual(await say([{ type: 'music_play', text: 'jazz', kind: 'radio' }], 'en'), 'jazz radio: Jazz FM.');
  assert.strictEqual(await say([{ type: 'music_add', text: 'Blinding Lights', kind: 'youtube' }]), "J'ajoute Blinding Lights à la file, position 3.");
  assert.strictEqual(await say([{ type: 'music_queue' }]), 'File : 2 titres, prochain : Blinding Lights.');
  assert.strictEqual(await say([{ type: 'music_queue' }], 'en'), 'Queue: 2 tracks, next: Blinding Lights.');
  assert.strictEqual(await say([{ type: 'music_volume', text: 'up' }]), 'Volume à 40.');
  assert.strictEqual(await say([{ type: 'music_volume', text: '25' }], 'en'), 'Volume at 25.');
  assert.strictEqual(await say([{ type: 'music_skip' }]), 'Suivant : Blinding Lights.');
  assert.strictEqual(await say([{ type: 'music_now' }]), "C'est Bohemian Rhapsody.");
  assert.strictEqual(await say([{ type: 'music_resume' }]), 'Rien ne joue.');
  assert.strictEqual(await say([{ type: 'music_resume' }], 'en'), 'Nothing is playing.');
  assert.strictEqual(await say([{ type: 'music_stop' }], 'en'), 'Stopped.');
  const none = resolve([{ type: 'music_pause' }], ctx);
  await run(none, {});
  assert.match(describe(none), /Ça n'a pas marché/);
  console.log('music: ok');

  // Claude not logged in: immediate answer, no wait
  ask.setReady(false);
  const q = resolve([{ type: 'ask', text: 'Quelle est la capitale du Congo ?' }], ctx);
  await run(q, io);
  assert.strictEqual(describe(q), "Claude n'est pas encore branché.");
  assert.match(describe(plan('qu\'est-ce que tu sais faire')), /^Je sais .*couper/);
  console.log('claude off + help: ok');
})().catch((e) => { console.error(e); process.exit(1); });
