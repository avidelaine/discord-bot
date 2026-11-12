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
  save('todo', []);
  const sup = { ...ctx, role: 'super' };
  await run(resolve([{ type: 'todo_add', text: 'pain' }], sup), io);
  await run(resolve([{ type: 'todo_add', text: 'lait' }], sup), io);
  await run(resolve([{ type: 'todo_remove', text: 'pain' }], sup), io);
  assert.deepStrictEqual(load('todo', []).map((x) => x.text), ['lait']);
  await run(plan('annule'), io);
  assert.deepStrictEqual(load('todo', []).map((x) => x.text), ['pain', 'lait']);
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

  await batch2();
})().catch((e) => { console.error(e); process.exit(1); });

// ---------- Batch 2 (journal of 2026-11-12, 22:12-22:24) ----------
async function batch2() {
  const { noteGuard, guard, aboutBobby, intent } = require('./brain');
  const info = require('./actions/info');
  const posts = [];
  const io = { leave() {}, post: async (t) => { posts.push(t); return 'dev-bot-test'; }, memberName: async (id) => id };
  const as = (role, name = 'Alex') => ({ ...ctx, role, speaker: ctx.members.find((m) => m.name === name) });
  const go = async (actions, c) => { const p = resolve(actions, c); await run(p, io); return describe(p); };
  const texts = () => load('todo', []).map((x) => x.text);

  // To-do lists: author, date, list, done; general + personal lists; rights
  save('todo', []);
  const prime = as('prime', 'Prime Ilias'), wak = as('super', 'WakandeX'), alex = as('prime', 'Alex');
  assert.strictEqual(await go([{ type: 'todo_add', text: 'Ilias doit avoir absolument tous les droits sur le bot' }], prime), "C'est noté dans la liste générale.");
  const [it] = load('todo', []);
  assert.deepStrictEqual([it.list, it.author, it.done, typeof it.at], ['general', { id: '397478613559148545', name: 'Prime Ilias' }, false, 'string']);
  assert.strictEqual(roles.roleOf(member('397478613559148545')), 'admin'); // a note never grants anything (admin = .env default here)
  await go([{ type: 'todo_add', text: 'acheter du pain', list: 'moi' }], alex);
  assert.strictEqual(load('todo', [])[1].list, 'Alex');
  await assert.rejects(run(resolve([{ type: 'todo_add', text: 'x', list: 'Ilias' }], alex), io), /liste générale ou à ta liste/);
  await assert.rejects(run(resolve([{ type: 'todo_done', text: 'droits' }], alex), io), /tes propres points/); // Ilias's item
  assert.strictEqual(await go([{ type: 'todo_done', text: 'pain' }], alex), 'Fait : acheter du pain');
  assert.strictEqual(await go([{ type: 'todo_done', text: 'dernier', list: 'general' }], wak), 'Fait : Ilias doit avoir absolument tous les droits sur le bot'); // super edits everything
  await go([{ type: 'todo_add', text: 'Mettre des fondus dans la musique' }, { type: 'todo_add', text: 'Une voix différente pour Claude' }, { type: 'todo_add', text: 'Nettoyer dev-bot-test' }, { type: 'todo_add', text: 'Quatrième point' }], wak);
  posts.length = 0;
  const said = await go([{ type: 'todo_list' }], wak);
  assert.match(said, /^La liste générale : 4 points\. Mettre des fondus dans la musique ; Une voix différente pour Claude ; Nettoyer dev-bot-test ; et la suite\. Tout est dans dev-bot-test\.$/);
  assert.match(posts[0], /Liste générale\*\* \(4 à faire, 1 fait\)/);
  assert.match(posts[0], /~~Ilias doit avoir.*~~ ✅ — \*Prime Ilias, \d\d\/\d\d/);
  assert.match(await go([{ type: 'todo_list', list: 'moi' }], alex), /^Ta liste est vide\.$/); // only done items left
  assert.strictEqual(await go([{ type: 'todo_move', text: 'quatrième', to: 'moi' }], wak), 'Je déplace « Quatrième point » dans ta liste.');
  assert.strictEqual(load('todo', []).find((x) => x.text === 'Quatrième point').list, '261531632589078528');
  assert.strictEqual(await go([{ type: 'todo_remove', text: '3' }], wak), 'Je supprime : Une voix différente pour Claude'); // numbers as posted, done items included
  assert.match(await go([{ type: 'todo_remove', text: 'inexistant truc' }], wak), /Je ne trouve pas/);
  // Old format (plain strings) still reads
  save('todo', ['ancien point']);
  assert.match(await go([{ type: 'todo_list' }], wak), /1 point\. ancien point/);
  console.log('to-do lists: ok');

  // Notes: "retiens / note / il faudrait que tu / rajoute dans la liste" keep the full text, before the LLM
  const NOTES = [
    ["il faut que tu retiennes pour la prochaine fois. On va vouloir mettre des fondus là, ça va pas.", 'On va vouloir mettre des fondus là, ça va pas.', 'general'],
    ["il faudrait que tu gardes l'historique des quelques dernières demandes qui t'ont été faites, parce que des fois tu ne comprends pas, et il faudrait que tu réessayes.", "Il faudrait que tu gardes l'historique des quelques dernières demandes qui t'ont été faites, parce que des fois tu ne comprends pas, et il faudrait que tu réessayes.", 'general'],
    ["est-ce que tu peux noter dans la liste des choses à faire ? Il faut que tu aies absolument en connaissance, un beau contexte, que ce soit sur toi ce que tu es capable de faire, que ce soit sur le fait que tu puisses, par exemple, gérer une to-do list ou que tu puisses répondre à des questions te concernant.", "Il faut que tu aies absolument en connaissance, un beau contexte, que ce soit sur toi ce que tu es capable de faire, que ce soit sur le fait que tu puisses, par exemple, gérer une to-do list ou que tu puisses répondre à des questions te concernant.", 'general'],
    ["tu peux rajouter à la to-do list que Ilias doit avoir absolument tous les droits à partir de maintenant sur le bot ?", 'Ilias doit avoir absolument tous les droits à partir de maintenant sur le bot', 'general'],
    ["j'aimerais que quand Claude me réponde, ça soit une voix différente. Est-ce que tu peux le noter dans la liste ?", "J'aimerais que quand Claude me réponde, ça soit une voix différente.", 'general'],
    ["est-ce que tu peux noter dans ta liste que tu dois être capable d'éditer tes propres messages que tu as envoyés sur Discord ? Que ça soit les supprimer, les modifier, enfin voilà, tout ce qui est pour l'édit de tes messages personnels à toi.", "Tu dois être capable d'éditer tes propres messages que tu as envoyés sur Discord ? Que ça soit les supprimer, les modifier, enfin voilà, tout ce qui est pour l'édit de tes messages personnels à toi.", 'general'],
    ["note qu'il faut une voix différente pour Claude", 'Il faut une voix différente pour Claude', 'general'],
    ['ajoute acheter du pain à ma liste', 'Acheter du pain', 'moi'],
    ['retiens que la soirée est samedi', 'La soirée est samedi', 'general'],
    ['add buy milk to my list', 'Buy milk', 'moi'],
  ];
  for (const [phrase, text, list] of NOTES) assert.deepStrictEqual(quick(phrase), [{ type: 'todo_add', text, list }], `note: "${phrase}"`);
  for (const phrase of ["qu'est-ce que j'ai sur ma liste", 'enlève le pain de ma liste', 'joue Notre Dame', 'mute Kévin']) assert(!quick(phrase)?.some((a) => a.type === 'todo_add'), `not a note: "${phrase}"`);
  for (const phrase of ['montre-moi la liste de tout ce que tu as retenu.', "montre ce que tu as retenu", 'affiche la liste']) assert.deepStrictEqual(quick(phrase), [{ type: 'todo_list', list: 'general' }], phrase);
  assert.deepStrictEqual(quick('peux-tu afficher ma liste ?'), [{ type: 'todo_list', list: 'moi' }]);
  assert.strictEqual(quick('qui a quel surnom')[0].type, 'alias_list');
  console.log('notes: ok');

  // Never a fake "noté": a reply claiming it saved something, with no saving step, becomes the offer
  assert.deepStrictEqual(noteGuard("J'ai bien noté : on va vouloir mettre des fondus.", [], 'fr'), { text: "Tu veux que je l'ajoute à la liste ?", offer: true });
  assert.deepStrictEqual(noteGuard('Noted, I will remember that.', [], 'en'), { text: 'Do you want me to add it to the list?', offer: true });
  assert.deepStrictEqual(noteGuard("Merci ! Tu veux que je l'ajoute à la liste ?", [], 'fr'), { text: "Merci ! Tu veux que je l'ajoute à la liste ?", offer: true });
  assert.deepStrictEqual(noteGuard('Il est 23 h.', [], 'fr'), { text: 'Il est 23 h.' });
  assert.deepStrictEqual(noteGuard("C'est noté dans la liste générale.", resolve([{ type: 'todo_add', text: 'x' }], wak), 'fr'), { text: "C'est noté dans la liste générale." });
  console.log('no fake "noted": ok');

  // Confirmation: a long yes passes, any "non" / "pas" cancels
  for (const t of ['Oui je confirme Bobby je confirme', 'oui vas-y tu peux le déconnecter maintenant']) assert(confirms(t), t);
  for (const t of ['oui enfin non', "ok mais pas lui"]) assert(!confirms(t), t);
  console.log('long confirmation: ok');

  // Guards on the LLM output
  const g = (actions, q, history = []) => guard({ actions, say: '' }, q, ctx, history).actions;
  assert.deepStrictEqual(g([{ type: 'leave' }], 'BOBBY STOP'), [{ type: 'music_stop', quiet: true }]);
  assert.deepStrictEqual(g([{ type: 'leave' }], 'quitte le salon'), [{ type: 'leave' }]);
  assert.deepStrictEqual(g([{ type: 'pc_launch', text: 'Apex Legends' }], 'ferme Apex Legends'), [{ type: 'pc_close', text: 'Apex Legends' }]);
  assert.deepStrictEqual(g([{ type: 'alias_list' }], 'montre-moi la liste de tout ce que tu as retenu.'), [{ type: 'todo_list', list: 'general' }]);
  assert.deepStrictEqual(g([{ type: 'admin_add', who: ['Marie'] }], 'passe Marie super admin'), [{ type: 'super_admin' }]);
  for (const q of ["est-ce que tu penses que tu es meilleur que Iris ?", "est-ce que tu peux expliquer à Ilias en une phrase, c'est quoi les permissions qu'il y a sur toi ?", "je trouve que ta détection n'est pas encore parfaite sur ton mode d'activation."]) {
    assert.strictEqual(g([{ type: 'ask', text: q }], q)[0].type, 'about', q);
  }
  assert.strictEqual(g([{ type: 'ask', text: 'x' }], 'Pourquoi il était perdu ?', [{ query: 'lance Apex', reply: 'Je lance Apex Legends.' }])[0].type, 'about');
  for (const q of ["c'est quoi la capitale de l'Australie", 'est-ce que tu peux me dire la population de la République démocratique du Congo ?', 'raconte-moi une blague', 'pourquoi le ciel est bleu']) {
    assert(!aboutBobby(q, ctx), `world question stays with Claude: "${q}"`);
  }
  const long = "est-ce que tu peux noter dans la liste des choses à faire ? Il faut que tu aies absolument en connaissance, un beau contexte, que ce soit sur toi";
  assert.match(g([{ type: 'todo_add', text: 'gérer une to-do list' }], long)[0].text, /^Il faut que tu aies absolument en connaissance/);
  console.log('LLM guards: ok');

  // Stop alone: stops the music if it plays, otherwise silence; leaving only on "pars"
  assert.deepStrictEqual(quick('stop'), [{ type: 'music_stop', text: '', quiet: true }]);
  assert.deepStrictEqual(quick('Bobby stop'.replace(/^bobby\s*/i, '')), [{ type: 'music_stop', text: '', quiet: true }]);
  for (const phrase of ['pars', 'quitte le salon', 'tu peux partir']) assert.strictEqual(quick(phrase)?.[0].type, 'leave', phrase);
  const idle = { nowPlaying: () => null, stop() { throw new Error('nothing to stop'); } };
  const st = resolve(quick('stop'), ctx);
  await run(st, { music: idle });
  assert.strictEqual(describe(st), '');
  const busy = { nowPlaying: () => ({ title: 'x' }), stop: () => ({ stopped: { title: 'x' }, cleared: 0 }) };
  const st2 = resolve(quick('stop'), ctx);
  await run(st2, { music: busy });
  assert.strictEqual(describe(st2), 'Musique arrêtée.');
  console.log('stop: ok');

  // Close an app or game (super admin), never mistaken for launching it
  assert.deepStrictEqual(quick('ferme Apex Legends'), [{ type: 'pc_close', text: 'apex legends' }]);
  assert.strictEqual(quick('ferme le micro de Kévin')[0].type, 'mute');
  assert(!quick('quitte le salon').some((a) => a.type === 'pc_close'));
  const cl = resolve(quick('ferme Apex Legends'), ctx);
  assert.strictEqual(needed(cl), 'super');
  pc._test.calls.length = 0;
  await run(cl, io);
  assert.match(pc._test.calls[0].join(' '), /^taskkill\.exe/);
  assert.strictEqual(describe(cl), 'Je ferme apex legends.');
  console.log('close app: ok');

  // Super admin untouchable: no promotion, no removal, by anyone
  for (const phrase of ['passe Marie super admin', 'nomme Lucas super admin', 'retire le super admin à WakandeX']) {
    const p = resolve(quick(phrase), wak);
    assert.strictEqual(p[0].type, 'super_admin', phrase);
    await assert.rejects(run(p, io), /fichier \.env/);
  }
  await assert.rejects(run(plan("retire l'admin à WakandeX"), io), /super admin/);
  await assert.rejects(run(plan('enlève les permissions de WakandeX'), io), /super admin/);
  assert.strictEqual(roles.roleOf(member(WAKANDEX)), 'super');
  assert.strictEqual(roles.roleOf(member('Marie')), 'other');
  console.log('super admin untouchable: ok');

  // Self-knowledge follows live state: rights of X before / after "passe X admin" and a blacklist
  const rights = async (name) => { const p = resolve(quick(`quels sont les droits de ${name}`), alex); await run(p, io); return describe(p); };
  assert.match(await rights('Marie'), /^Marie n'a aucun rôle/);
  await run(plan('passe Marie admin'), io);
  assert.match(await rights('Marie'), /^Marie est admin : .*la modération.*la musique/);
  assert.match(await info.facts({ by: { name: 'Alex', role: 'prime' }, ctx }, io), /Admins : .*Marie/);
  await run(plan('enlève les permissions de Marie'), io);
  assert.match(await rights('Marie'), /^Marie est sur liste noire/);
  assert.match(await info.facts({ by: { name: 'Alex', role: 'prime' }, ctx }, io), /liste noire\) : .*Marie/);
  assert.doesNotMatch((await info.facts({ by: {}, ctx }, io)).split('\n').find((l) => l.startsWith('Super admin')), /Admins : [^.]*Marie/);
  assert.match(await rights('WakandeX'), /^WakandeX est super admin/);
  assert.strictEqual(needed(resolve(quick('quels sont les droits de Lucas'), alex)), 'prime');
  console.log('self-knowledge in sync: ok');

  // History: last 5 requests within 10 minutes, per person
  const { remember, historyOf } = require('./brain');
  for (let i = 1; i <= 7; i++) remember('h1', `demande ${i}`, `réponse ${i}`);
  assert.deepStrictEqual(historyOf('h1').map((h) => h.query), ['demande 3', 'demande 4', 'demande 5', 'demande 6', 'demande 7']);
  assert.deepStrictEqual(historyOf('h2'), []);
  console.log('history: ok');

  // Own messages: clean needs a "oui", is admin, and defaults to the text channel
  const cp = resolve(quick('nettoie dev-bot-test'.replace('dev-bot-test', 'memes')), alex);
  assert.deepStrictEqual([cp[0].type, cp[0].channel.name, needed(cp)], ['clean', 'memes', 'admin']);
  assert.strictEqual(require('./actions/discord').actions.clean.confirm, true);
  assert.strictEqual(quick('supprime ton dernier message')[0].type, 'msg_delete');
  console.log('own messages: ok');
  void intent;
}
