// Knowledge questions forwarded to Claude Haiku through the Claude Code CLI.
const fs = require('fs');
const path = require('path');
const { spawn } = require('child_process');

const MODEL = process.env.CLAUDE_MODEL || 'haiku';
let ready = false; // Claude Code logged in; checked at startup and again while it is not

function check() {
  return new Promise((resolve) => {
    let out = '';
    const child = spawn('claude auth status', { windowsHide: true, shell: true });
    child.stdout.on('data', (d) => { out += d; });
    child.on('error', () => resolve(false));
    child.on('close', () => {
      try { ready = JSON.parse(out).loggedIn === true; } catch { ready = false; }
      console.log(`[claude] ${ready ? 'logged in: questions go to Claude' : 'not logged in: Bobby answers "Claude n\'est pas encore branché"'}`);
      resolve(ready);
    });
  });
}

// No files, no shell, no MCP, no user settings; web search only.
// written: the answer is posted, not spoken, so it may be longer and use Discord markdown.
function ask(question, lang, written = false) {
  const dir = path.join(require('os').tmpdir(), 'bobby-claude'); // empty working folder: nothing to read
  fs.mkdirSync(dir, { recursive: true });
  const style = written
    ? (lang === 'en' ? 'Answer in English, for a Discord message: complete but concise (under 1500 characters), Discord markdown allowed (bold, bullet lists).'
      : 'Réponds en français, pour un message Discord : complet mais concis (moins de 1500 caractères), markdown Discord autorisé (gras, listes à puces).')
    : lang === 'en'
      ? 'Answer in English, in one or two short spoken sentences, no markdown, no lists.'
      : 'Réponds en français, en une ou deux phrases courtes à dire à voix haute, sans markdown ni liste.';
  return new Promise((resolve, reject) => {
    const child = spawn('claude', [
      '-p', question, '--model', MODEL, '--append-system-prompt', `You answer for Bobby, a Discord voice assistant. ${style}`,
      '--tools', 'WebSearch', '--allowedTools', 'WebSearch', '--setting-sources', '', '--strict-mcp-config',
      '--no-session-persistence', '--output-format', 'text',
    ], { cwd: dir, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
    let out = '', err = '';
    child.stdout.on('data', (d) => { out += d; });
    child.stderr.on('data', (d) => { err += d; });
    const timer = setTimeout(() => child.kill(), 60_000);
    child.on('error', reject);
    child.on('close', (code) => {
      clearTimeout(timer);
      if (code === 0 && out.trim()) resolve(out.trim());
      else reject(new Error(`claude exited ${code}: ${(err || out).trim().slice(0, 200)}`));
    });
  });
}

const OFF = { fr: "Claude n'est pas encore branché.", en: "Claude isn't connected yet." };
const DOWN = { fr: "Je n'arrive pas à joindre Claude.", en: "I can't reach Claude right now." };

module.exports = {
  help: { fr: 'poser les questions de culture générale à Claude', en: 'forward general knowledge questions to Claude' },
  short: { fr: 'les questions à Claude', en: 'questions to Claude' },
  prompt: ['- ask : transmettre à Claude, un assistant plus savant, une vraie question de culture générale "text" (reformulée clairement, complète)'],
  rules: [
    '- ask seulement pour une vraie question sur le monde : culture, actualité, sciences, calcul, explication, recette, conseil général. Jamais pour une question sur toi (Bobby), le serveur, les gens présents, les droits, la conversation en cours, une demande de noter, une remarque ou un avis sur toi : ça, c\'est about, todo_add ou une autre action locale.',
    '- Ne réponds toi-même ("actions" vide, réponse courte dans "say") qu\'aux salutations et aux politesses.',
  ],
  examples: [
    '"c\'est quoi la capitale de l\'Australie" -> {"actions":[{"type":"ask","text":"Quelle est la capitale de l\'Australie ?"}],"say":""}',
    '"who won the last world cup" -> {"actions":[{"type":"ask","text":"Who won the last FIFA World Cup?"}],"say":""}',
    '"qui est le meilleur entre toi et Alexa" -> {"actions":[{"type":"about","text":"qui est le meilleur entre toi et Alexa"}],"say":""}',
  ],
  quick: [],
  actions: {
    ask: {
      role: 'admin',
      text: true,
      // Claude takes a few seconds: say so right away, unless it is not connected.
      announce: (lang) => (ready ? (lang === 'en' ? 'Let me ask Claude.' : 'Je demande à Claude.') : null),
      async run(s, io, lang) {
        if (!ready) { s.answer = OFF[lang] || OFF.fr; check(); return; } // re-check for next time, without waiting
        const t0 = performance.now();
        try { s.answer = await ask(s.text, lang, s.written); s.voice = 'claude'; if (s.written) s.card = `❓ **${s.text}**\n\n${s.answer}`; } catch (e) { console.error('[claude]', e.message); s.answer = DOWN[lang] || DOWN.fr; }
        console.log(`[claude] ${Math.round(performance.now() - t0)} ms: ${s.answer}`);
      },
      say: { fr: (s) => s.answer, en: (s) => s.answer },
    },
  },
  check,
  setReady: (v) => { ready = v; }, // tests
};
