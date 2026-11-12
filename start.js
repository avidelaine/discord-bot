// Starts Bobby's three parts and merges their logs.
// Dev (default): one console, one colored line per event.  Headless (--headless): no window, logs in _logs/bobby.log.
const { spawn, spawnSync } = require('child_process');
const fs = require('fs');
const path = require('path');

const ROOT = __dirname;
const headless = process.argv.includes('--headless');
const LOGS = path.join(ROOT, '_logs');
fs.mkdirSync(LOGS, { recursive: true });
const logFile = fs.createWriteStream(path.join(LOGS, 'bobby.log'), { flags: 'a' });
const pidFile = path.join(LOGS, 'bobby.pid');
// One launcher at a time: a second one would log the bot in twice and fight over the ports.
try {
  const old = Number(fs.readFileSync(pidFile, 'utf8'));
  process.kill(old, 0); // throws if that process is gone (stale pid file)
  console.error(`Bobby is already running (pid ${old}). Run stop.cmd first.`);
  process.exit(1);
} catch {}
fs.writeFileSync(pidFile, String(process.pid)); // lets stop.cmd find us

let PARTS = [
  {
    name: 'llm', color: 35, cmd: path.join(ROOT, '_bin', 'llama', 'llama-server.exe'),
    args: ['-m', 'models/Qwen3-4B-Instruct-2507-Q4_K_M.gguf', '-ngl', '99', '-c', '8192', '-np', '1', '-fa', 'on', '--host', '127.0.0.1', '--port', '8080', '--log-file', '_logs/llm.log'],
    keep: /listening|error|failed|out of memory/i, // llama-server logs every request; keep what matters
  },
  { name: 'voice', color: 36, cmd: path.join(ROOT, 'voice', '.venv', 'Scripts', 'python.exe'), args: ['voice/server.py'] },
  { name: 'bot', color: 33, cmd: process.execPath, args: ['bot.js'], cwd: path.join(ROOT, 'bot') },
];
// Test override: dummy parts, one exits after 1 s (restart check), one keeps printing.
if (process.env.BOBBY_PARTS === 'test') PARTS = [
  { name: 'short', color: 35, cmd: process.execPath, args: ['-e', 'console.log("hi");setTimeout(()=>{},1000)'] },
  { name: 'long', color: 36, cmd: process.execPath, args: ['-e', 'setInterval(()=>console.log("tick"),500)'] },
];

function log(part, line) {
  const t = new Date().toLocaleTimeString('fr-FR');
  logFile.write(`${t} [${part.name}] ${line}\n`);
  if (!headless) console.log(`\x1b[90m${t}\x1b[0m \x1b[${part.color}m${part.name.padEnd(5)}\x1b[0m ${line}`);
}

let stopping = false;
const children = new Set();

function run(part) {
  const child = spawn(part.cmd, part.args, {
    cwd: part.cwd || ROOT,
    env: { ...process.env, PYTHONIOENCODING: 'utf-8', PYTHONUNBUFFERED: '1' },
    windowsHide: true,
  });
  children.add(child);
  for (const stream of [child.stdout, child.stderr]) {
    let rest = '';
    stream.setEncoding('utf8');
    stream.on('data', (chunk) => {
      const lines = (rest + chunk).split(/\r?\n/);
      rest = lines.pop();
      for (const l of lines) if (l.trim() && (!part.keep || part.keep.test(l))) log(part, l);
    });
  }
  child.on('error', (e) => log(part, `cannot start: ${e.message}`));
  child.on('close', (code) => {
    children.delete(child);
    if (stopping) return;
    log(part, `stopped (exit code ${code}), restarting in 3 s`);
    setTimeout(() => run(part), 3000);
  });
}
PARTS.forEach(run);

function stop() {
  if (stopping) return;
  stopping = true;
  // ponytail: taskkill /T kills each part's whole process tree; sync so we exit only after they are gone
  for (const c of children) spawnSync('taskkill', ['/T', '/F', '/PID', String(c.pid)], { windowsHide: true });
  try { fs.unlinkSync(pidFile); } catch {}
  process.exit(0);
}
process.on('SIGINT', stop);
process.on('SIGTERM', stop);
log({ name: 'bobby', color: 32 }, headless ? 'started headless, logs in _logs/bobby.log' : 'started, Ctrl+C stops everything');
