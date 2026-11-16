// Writes the prompts Node's brain builds, so the Rust port can be compared byte for byte.
// Run: node rust/bobby-core/tests/fixtures/dump-prompts.js   (never touches data/ or _logs/)
const fs = require('fs');
const os = require('os');
const path = require('path');

// Same fixed to-do list as bot/intent.test.js, in a temp folder.
process.env.DATA_DIR = fs.mkdtempSync(path.join(os.tmpdir(), 'bobby-prompts-'));
fs.writeFileSync(path.join(process.env.DATA_DIR, 'todo.json'), JSON.stringify([
  { id: 1, text: 'Mettre des fondus dans la musique', list: 'general', author: { id: '261531632589078528', name: 'WakandeX' }, at: '2026-11-12T22:15:08.000Z', done: false },
  { id: 2, text: 'Une voix différente pour Claude', list: 'general', author: { id: '261531632589078528', name: 'WakandeX' }, at: '2026-11-12T22:24:03.000Z', done: false },
]));
process.env.JOURNAL_FILE = path.join(os.tmpdir(), `bobby-prompts-${process.pid}.jsonl`);
process.env.TZ = 'Europe/Paris';

// Frozen clock: Date.now() and new Date() give 2026-11-16T18:05:00Z (19:05 in Paris).
const RealDate = Date;
const NOW = RealDate.parse('2026-11-16T18:05:00Z');
global.Date = class extends RealDate {
  constructor(...a) { if (a.length) super(...a); else super(NOW); }
  static now() { return NOW; }
};

// brain.js destructures `llm` from util at load: patch util's export object BEFORE requiring brain.
const calls = [];
require('../../../../bot/util').llm = async (messages, schema) => {
  calls.push({ messages, schema });
  return { analyse: { genre: 'discussion', faisable: true }, actions: [], say: 'ok' };
};
const { understand } = require('../../../../bot/brain');
const base = { ...require('../../../../bot/test-context'), role: 'super' };

(async () => {
  const outsider = { id: 'visiteur', name: 'Visiteur', channel: undefined, nicknames: [], aliases: [] };
  const history = [
    { at: NOW - 5 * 60_000, id: 'Alex', who: 'Alex', query: 'coupe le micro de Kévin', did: 'mute Kévin', reply: '' },
    { at: NOW - 60_000, id: 'Kévin', who: 'Kévin', query: 'et Marie aussi', did: '', reply: 'Désolé, je ne peux pas.' },
  ];
  const cases = [
    [base, 'coupe le micro de Kévin', 'fr', []],
    [{ ...base, speaker: outsider }, 'viens dans Gaming', 'fr', []],
    [base, 'refais pareil pour Marie', 'fr', history],
    [base, 'mute Kevin', 'en', []],
    [base, 'écris "c\'est l\'été à Noël" dans annonces, ça va être génial', 'fr', []],
  ];
  for (const [ctx, q, lang, h] of cases) await understand(q, ctx, lang, h);
  const dir = __dirname;
  fs.writeFileSync(path.join(dir, 'system.txt'), calls[0].messages[0].content);
  fs.writeFileSync(path.join(dir, 'schema.json'), JSON.stringify(calls[0].schema));
  calls.forEach((c, i) => fs.writeFileSync(path.join(dir, `prompt-${i + 1}.txt`), c.messages[1].content));
  console.log(`wrote system.txt, schema.json and ${calls.length} prompts`);
})();
