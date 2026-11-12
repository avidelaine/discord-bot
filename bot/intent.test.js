// Comprehension test set: sentence -> expected actions, against the real LLM (llama-server running).
// Run: node bot/intent.test.js   Add every sentence Bobby gets wrong in real use.
const assert = require('assert');
const { understand, resolve, quick, pick, sim } = require('./brain');

// Name resolution: pure, tested without the LLM.
const roster = [{ name: 'Zyklon' }, { name: 'Kévin', aliases: ['kev_du_93'] }, { name: 'Marie' }, { name: 'Mario' }];
assert.strictEqual(pick('ziklone', roster, 'person').name, 'Zyklon');
assert.strictEqual(pick('kevin', roster, 'person').name, 'Kévin');
assert.strictEqual(pick('kev du 93', roster, 'person').name, 'Kévin');
assert.throws(() => pick('Mari', roster, 'person'), /tu veux dire/);
assert.throws(() => pick('Bernard', roster, 'person'), /Je ne trouve pas/);
assert(sim('🎮 Gaming', 'gaming') === 1);
console.log('name resolution: ok');

const m = (name, channel) => ({ id: name, name, channel });
const ctx = {
  speaker: m('Alex', 'Général'),
  members: [m('Alex', 'Général'), { ...m('Zyklon', 'Général'), nicknames: ['le Grec'], aliases: ['le Grec'] }, m('Kévin', 'Général'), m('Marie', 'Gaming'), m('Lucas', 'Gaming')],
  voiceChannels: ['Général', 'Gaming', 'AFK', 'Réunion'].map((name) => ({ name })),
  textChannels: ['général', 'annonces', 'memes'].map((name) => ({ name })),
};

// [sentence, expected] ; expected = "type:targets>channel" per action, joined by " | ". [] = no action.
const CASES = [
  ['coupe le micro de Kévin', 'mute:Kévin'],
  ['mute ziklone', 'mute:Zyklon'],
  ['rends la parole à Kevin', 'unmute:Kévin'],
  ['coupe le son de Marie', 'deafen:Marie'],
  ['Marie peut réentendre', 'undeafen:Marie'],
  ['déplace Zyklon dans gaming', 'move:Zyklon>Gaming'],
  ['envoie Lucas en AFK', 'move:Lucas>AFK'],
  ['mets-moi dans la salle de réunion', 'move:Alex>Réunion'],
  ['ramène Marie ici', 'move:Marie>Général'],
  ['tout le monde dans gaming', 'move:Alex,Kévin,Lucas,Marie,Zyklon>Gaming'],
  ['coupe tout le monde sauf Kévin', 'mute:Zyklon'],
  ['vire Lucas du vocal', 'disconnect:Lucas'],
  ['déplace Kévin dans AFK et coupe son micro', 'move:Kévin>AFK | mute:Kévin'],
  ['écris dans annonces que la soirée commence à 21 heures', 'message:>annonces'],
  ['ajoute acheter du pain à ma liste', 'todo_add:'],
  ["qu'est-ce que j'ai sur ma liste", 'todo_list:'],
  ['enlève le pain de ma liste', 'todo_remove:'],
  ['annule', 'undo:'],
  ['laisse tomber, reviens en arrière', 'undo:'],
  ['tu peux partir', 'leave:'],
  ['quelle heure est-il', []],
  ['raconte-moi une blague', 'ask:'],
  ['déplace Bernard dans gaming', 'ERR'],
  ['mute Zyklon et Kévin', 'mute:Kévin,Zyklon'],
  ['euh coupe le micro de... de Marie', 'mute:Marie'],
  // English
  ['mute Kevin', 'mute:Kévin'],
  ['give Zyklon his mic back', 'unmute:Zyklon'],
  ['deafen Marie please', 'deafen:Marie'],
  ['move Lucas to the AFK channel', 'move:Lucas>AFK'],
  ['bring Marie over here', 'move:Marie>Général'],
  ['take me to gaming', 'move:Alex>Gaming'],
  ['mute everyone except Kevin', 'mute:Zyklon'],
  ['kick Lucas out of voice', 'disconnect:Lucas'],
  ['post in announcements that the game starts at nine', 'message:>annonces'],
  ['add buy milk to my list', 'todo_add:'],
  ['undo that', 'undo:'],
  ['you can go now', 'leave:'],
  ['how are you doing', []],
  // Nicknames (Zyklon is "le Grec" in the test context)
  ['appelle Kévin le Boss', 'alias_add:Kévin'],
  ["Zyklon ne s'appelle plus le Grec", 'alias_remove:Zyklon'],
  ['vire le Grec du vocal', 'disconnect:Zyklon'],
  ['coupe le micro du Grec', 'mute:Zyklon'],
  ['call Lucas the rookie', 'alias_add:Lucas'],
  // Questions forwarded to Claude
  ["c'est quoi la capitale de l'Australie", 'ask:'],
  ['explique-moi comment marche un trou noir', 'ask:'],
  ['combien font 17 fois 23', 'ask:'],
  ['who won the last world cup', 'ask:'],
  ['salut Bobby ça va', []],
];

// LLM-free shortcut: what it takes must be right, what it leaves goes to the LLM (null).
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
];

const fmt = (plan) => plan.map((s) => `${s.type}:${(s.targets || []).map((t) => t.name).sort().join(',')}${s.channel ? `>${s.channel.name}` : ''}`).join(' | ');

for (const [phrase, want] of QUICK_CASES) {
  const a = quick(phrase);
  let got = null;
  try { got = a && fmt(resolve(a, ctx)); } catch { got = null; } // name not found: falls back to the LLM
  assert.strictEqual(got, want, `shortcut: "${phrase}"`);
}
console.log('LLM-free shortcut: ok');

(async () => {
  let ok = 0;
  const times = [];
  for (const [phrase, want] of CASES) {
    const t0 = performance.now();
    let got;
    try {
      const out = await understand(phrase, ctx);
      got = out.actions.length ? fmt(resolve(out.actions, ctx)) : [];
    } catch (e) {
      if (e.code === 'ECONNREFUSED' || e.cause?.code === 'ECONNREFUSED') throw new Error('llama-server is not answering on port 8080');
      got = 'ERR';
    }
    times.push(performance.now() - t0);
    const pass = Array.isArray(want) ? Array.isArray(got) : got === want;
    if (pass) ok++;
    else console.log(`FAIL  "${phrase}"\n   expected ${JSON.stringify(want)}\n   got      ${JSON.stringify(got)}`);
  }
  times.sort((a, b) => a - b);
  const med = Math.round(times[times.length >> 1]);
  console.log(`\nComprehension: ${ok}/${CASES.length}  ·  LLM latency median ${med} ms, max ${Math.round(times.at(-1))} ms`);
  process.exit(ok / CASES.length >= 0.9 ? 0 : 1);
})();
