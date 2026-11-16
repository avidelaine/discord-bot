// Starts Bobby's three parts and merges their logs (the work is done by app/supervisor.js).
// Dev (default): one console, one colored line per event.  Headless (--headless): no window, logs in _logs/bobby.log.
const { Supervisor } = require('./app/supervisor');

const headless = process.argv.includes('--headless');
const COLORS = { llm: 35, voice: 36, bot: 33, short: 35, long: 36, bobby: 32 };

const sup = new Supervisor();
if (!headless) {
  sup.on('log', ({ part, line, at }) => {
    const t = new Date(at).toLocaleTimeString('fr-FR');
    console.log(`\x1b[90m${t}\x1b[0m \x1b[${COLORS[part] || 37}m${part.padEnd(5)}\x1b[0m ${line}`);
  });
}
try { sup.start(); } catch (e) { console.error(e.message); process.exit(1); }

function stop() { sup.stop(); process.exit(0); }
process.on('SIGINT', stop);
process.on('SIGTERM', stop);
sup.log('bobby', headless ? 'started headless, logs in _logs/bobby.log' : 'started, Ctrl+C stops everything');
