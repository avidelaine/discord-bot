// Replays the real journal through Node's brain intent(): same sentences, same history, one JSON line each.
// The Rust twin is examples/replay.rs; `replay --compare node.jsonl rust.jsonl` diffs the two outputs.
//   node rust/bobby-core/tests/fixtures/replay-node.js --make-ctx    builds replay-ctx.json next to the journal (real names: never in git)
//   LLM_URL=http://127.0.0.1:8080 node rust/bobby-core/tests/fixtures/replay-node.js [journal] > node.jsonl
// Never touches data/ or _logs/ (temp DATA_DIR and JOURNAL_FILE); the journal is only read.
const fs = require('fs');
const os = require('os');
const path = require('path');

const ROOT = path.join(__dirname, '..', '..', '..', '..');
const SUPER = '261531632589078528';
const journalPath = process.argv.slice(2).find((a) => !a.startsWith('--')) || [path.join(ROOT, '_logs'), path.join(ROOT, '..', '..', '..', '_logs')].map((d) => path.join(d, 'conversations.jsonl')).find((p) => fs.existsSync(p)); // the journal is not in git: a worktree reads the main checkout's
const CTX_FILE = path.join(path.dirname(journalPath), 'replay-ctx.json'); // _logs/, out of git like the journal
const records = fs.readFileSync(journalPath, 'utf8').split('\n').filter(Boolean).map((l) => JSON.parse(l));

// ---------- --make-ctx: one fixed server snapshot built from the journal ----------
if (process.argv.includes('--make-ctx')) {
  const speakers = {}; // id -> { name, role }
  for (const r of records) if (r.id && r.who && !speakers[r.id]) speakers[r.id] = { name: r.who, role: r.id === SUPER ? 'super' : 'admin' };
  delete speakers.u1; // the first test lines: a second "WakandeX" that would only make every name ambiguous
  const names = new Set(Object.values(speakers).map((s) => s.name));
  const voice = new Set(['Général']), text = new Set();
  // plan texts: `type -> A, B @ Chan`, `message @ chan "text"`
  for (const r of records) for (const part of (r.plan || '').split(' | ')) {
    const m = part.match(/^(\w+)(?: -> (.+?))?(?: @ ([^"]+?))?(?: "[\s\S]*)?$/);
    if (!m) continue;
    const [, type, who, chan] = m;
    if (['mute', 'unmute', 'deafen', 'undeafen', 'move', 'disconnect'].includes(type) && who) who.split(', ').forEach((n) => names.add(n.trim()));
    if (chan) (type === 'message' ? text : voice).add(chan.trim());
  }
  const members = [...names].map((name) => {
    const id = Object.keys(speakers).find((k) => speakers[k].name === name) || name;
    return { id, name, channel: 'Général', nicknames: [], aliases: [] };
  });
  for (const id of Object.keys(speakers)) if (!members.some((m) => m.id === id)) delete speakers[id];
  // a text channel never resolves as a voice one
  const out = { speakers, members, voiceChannels: [...voice].filter((c) => !text.has(c)), textChannels: [...text] };
  fs.writeFileSync(CTX_FILE, `${JSON.stringify(out, null, 2)}\n`);
  console.log(`replay-ctx.json: ${members.length} members, ${voice.size} voice, ${text.size} text channels`);
  process.exit(0);
}

// ---------- Replay ----------
// The same to-do list as bot/intent.test.js, so the list lines in the prompt are identical on both sides.
process.env.DATA_DIR = fs.mkdtempSync(path.join(os.tmpdir(), 'bobby-replay-'));
fs.writeFileSync(path.join(process.env.DATA_DIR, 'todo.json'), JSON.stringify([
  { id: 1, text: 'Mettre des fondus dans la musique', list: 'general', author: { id: SUPER, name: 'WakandeX' }, at: '2026-11-12T22:15:08.000Z', done: false },
  { id: 2, text: 'Une voix différente pour Claude', list: 'general', author: { id: SUPER, name: 'WakandeX' }, at: '2026-11-12T22:24:03.000Z', done: false },
]));
process.env.JOURNAL_FILE = path.join(process.env.DATA_DIR, 'conversations.jsonl'); // empty: reload() finds nothing
const { intent, resolve, remember, conversation } = require(path.join(ROOT, 'bot', 'brain'));

// copied from bot/intent.test.js
const merge = (plan) => plan.reduce((out, s) => {
  const last = out.at(-1);
  if (last && last.type === s.type && last.channel === s.channel && last.targets && s.targets) last.targets = [...last.targets, ...s.targets];
  else out.push({ ...s });
  return out;
}, []);
const fmt = (plan) => merge(plan).map((s) => `${s.type}:${(s.targets || []).map((t) => t.name).sort().join(',')}${s.channel ? `>${s.channel.name}` : ''}`).join(' | ');

const snap = JSON.parse(fs.readFileSync(CTX_FILE, 'utf8'));
const members = snap.members.map((m) => ({ ...m }));
const ctxFor = (id, who) => {
  const sp = snap.speakers[id];
  const speaker = members.find((m) => m.id === id) || { id, name: who || sp?.name || id };
  return { speaker, members, voiceChannels: snap.voiceChannels.map((name) => ({ name })), textChannels: snap.textChannels.map((name) => ({ name })), role: sp?.role || 'admin' };
};

const realNow = Date.now;
(async () => {
  let i = 0;
  for (const rec of records) {
    const at = Date.parse(rec.at) || realNow();
    // the conversation window follows the record's time; the clock is real again while the prompt is built, like in Rust
    // (its "il y a N min" lines use the real now), so both prompts carry the same minutes
    Date.now = () => at;
    if (rec.query && !rec.confirm) {
      const ctx = ctxFor(rec.id, rec.who);
      const history = conversation();
      Date.now = realNow;
      const lang = rec.lang || 'fr';
      let line;
      try {
        const out = await intent(rec.query, ctx, lang, history);
        line = { via: out.via, plan: out.actions.length ? fmt(resolve(out.actions, ctx)) : '(answered)', decision: out.decision, written: Boolean(out.written) };
      } catch (e) { line = { via: '', plan: `ERR ${e.message}`, decision: '', written: false }; }
      process.stdout.write(`${JSON.stringify({ i, at: rec.at, query: rec.query, ...line })}\n`);
      i++;
    }
    Date.now = () => at;
    remember(rec);
  }
  process.exit(0);
})();
