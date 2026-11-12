// Jeu de test de compréhension : phrase -> actions attendues, contre le vrai LLM (llama-server lancé).
// Lancer : node bot/intent.test.js   Ajoute ici chaque phrase que Bobby a mal comprise en vrai.
const assert = require('assert');
const { understand, resolve, quick, pick, sim } = require('./brain');

// Résolution des noms : pure, testée sans LLM.
const roster = [{ name: 'Zyklon' }, { name: 'Kévin', aliases: ['kev_du_93'] }, { name: 'Marie' }, { name: 'Mario' }];
assert.strictEqual(pick('ziklone', roster, 'x').name, 'Zyklon');
assert.strictEqual(pick('kevin', roster, 'x').name, 'Kévin');
assert.strictEqual(pick('kev du 93', roster, 'x').name, 'Kévin');
assert.throws(() => pick('Mari', roster, 'x'), /tu veux dire/);
assert.throws(() => pick('Bernard', roster, 'x'), /Je ne trouve pas/);
assert(sim('🎮 Gaming', 'gaming') === 1);
console.log('résolution des noms : ok');

const m = (name, channel) => ({ id: name, name, channel });
const ctx = {
  speaker: m('Alex', 'Général'),
  members: [m('Alex', 'Général'), m('Zyklon', 'Général'), m('Kévin', 'Général'), m('Marie', 'Gaming'), m('Lucas', 'Gaming')],
  voiceChannels: ['Général', 'Gaming', 'AFK', 'Réunion'].map((name) => ({ name })),
  textChannels: ['général', 'annonces', 'memes'].map((name) => ({ name })),
};

// [phrase, résultat attendu] ; résultat = "type:cibles>salon" par action, séparés par " | ". [] = aucune action.
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
  ['raconte-moi une blague', []],
  ['déplace Bernard dans gaming', 'ERR'],
  ['mute Zyklon et Kévin', 'mute:Kévin,Zyklon'],
  ['euh coupe le micro de... de Marie', 'mute:Marie'],
];

// Raccourci sans LLM : ce qu'il prend doit être juste, ce qu'il laisse passe au LLM (null).
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
];

const fmt = (plan) => plan.map((s) => `${s.type}:${(s.targets || []).map((t) => t.name).sort().join(',')}${s.channel ? `>${s.channel.name}` : ''}`).join(' | ');

for (const [phrase, want] of QUICK_CASES) {
  const a = quick(phrase);
  let got = null;
  try { got = a && fmt(resolve(a, ctx)); } catch { got = null; } // nom introuvable : bascule sur le LLM
  assert.strictEqual(got, want, `raccourci : « ${phrase} »`);
}
console.log('raccourci sans LLM : ok');

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
      if (e.code === 'ECONNREFUSED' || e.cause?.code === 'ECONNREFUSED') throw new Error('llama-server ne répond pas sur le port 8080');
      got = 'ERR';
    }
    times.push(performance.now() - t0);
    const pass = Array.isArray(want) ? Array.isArray(got) : got === want;
    if (pass) ok++;
    else console.log(`ÉCHEC  « ${phrase} »\n   attendu ${JSON.stringify(want)}\n   obtenu  ${JSON.stringify(got)}`);
  }
  times.sort((a, b) => a - b);
  const med = Math.round(times[times.length >> 1]);
  console.log(`\nCompréhension : ${ok}/${CASES.length}  ·  temps LLM médian ${med} ms, max ${Math.round(times.at(-1))} ms`);
  process.exit(ok / CASES.length >= 0.9 ? 0 : 1);
})();
