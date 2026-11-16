// Extracts the cases of bot/intent.test.js into intents.json, for the Rust runner (examples/intents.rs).
// Run: node rust/bobby-core/tests/fixtures/extract-intents.js   (reads bot/intent.test.js as text, never modifies it)
// The BATCH4 checks are JS functions: they are re-written below as declarative objects, one per BATCH4 line,
// and the Rust runner interprets them. Keys (all must hold):
//   decision / decision_not : out.decision === x / !== x
//   got                     : the plan as fmt() equals the string
//   got_re                  : the plan as fmt() matches the JS regex source
//   written                 : out.written is truthy
//   filter                  : out.actions[0].filter === x
const fs = require('fs');
const path = require('path');

const src = fs.readFileSync(path.join(__dirname, '../../../../bot/intent.test.js'), 'utf8').split(/\r?\n/).join('\n');
const literal = (name) => {
  const m = src.match(new RegExp(`^const ${name} = (\\[[\\s\\S]*?\\n\\]);`, 'm'));
  if (!m) throw new Error(`${name} not found in intent.test.js`);
  return m[1];
};

// History times become relative: `at` = NOW - ago_ms, the runner re-anchors them on its own clock.
const NOW = 0;
const H = (query, reply) => ({ at: NOW, query, reply });
const C = (query, did, reply = '', who = 'Alex') => ({ at: NOW - 60_000, id: who, who, query, did, reply });
const notRun = () => true; // only used to evaluate the BATCH4 literal; the real checks are CHECKS below
const ev = (name) => eval(literal(name));
const hist = (h) => h.map(({ at, ...rest }) => ({ ago_ms: NOW - at, ...rest }));

const todo = eval(src.match(/JSON\.stringify\((\[[\s\S]*?\n\])\)/)[1]);
const CASES = ev('CASES');
const ROUTING = ev('ROUTING');
const WITH_HISTORY = ev('WITH_HISTORY');
const BATCH4 = ev('BATCH4');

const CHECKS = [
  { decision_not: 'run' },
  { decision: 'answer' },
  { decision_not: 'run' },
  { decision_not: 'run' },
  { got: 'music_volume:', decision: 'run' },
  { got: 'help:', written: true },
  { got: 'help:', written: true, decision: 'run' },
  { got: 'help:', written: true },
  { got: 'todo_list:', filter: 'done' },
  { got: 'ping:Alex>dev-bot-test' },
  { got: 'ping:Alex>dev-bot-test' },
  { decision: 'offer' },
  { got: 'mute:Kévin', decision: 'run' },
  { decision_not: 'run' },
  { got_re: '(?:^| )mute:Zyklon' },
  { got: 'move:Marie>AFK' },
  { got: 'mute:Kévin' },
];

const counts = [CASES.length, ROUTING.length + WITH_HISTORY.length, BATCH4.length];
if (counts[0] !== 95 || counts[1] !== 52 || counts[2] !== 17 || CHECKS.length !== BATCH4.length) {
  throw new Error(`unexpected counts ${counts} / ${CHECKS.length} checks`);
}

const out = {
  todo,
  cases: CASES,
  routing: [...ROUTING.map(([p, w]) => [p, [], w]), ...WITH_HISTORY.map(([p, h, w]) => [p, hist(h), w])],
  batch4: BATCH4.map(([p, h], i) => [p, hist(h), CHECKS[i]]),
};
fs.writeFileSync(path.join(__dirname, 'intents.json'), JSON.stringify(out, null, 1) + '\n');
console.log(`intents.json: ${out.cases.length} cases, ${out.routing.length} routing, ${out.batch4.length} batch4, ${todo.length} todo items`);
