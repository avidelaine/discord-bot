// Runs Bobby's three parts (llm, voice, bot): spawn, restart after a crash, stop. Used by start.js (CLI) and app/server.js.
// Events: 'status' (status() snapshot, on any change) and 'log' ({ part, line, at }, one per output line).
const { EventEmitter } = require('events');
const { spawn, spawnSync, execFile } = require('child_process');
const fs = require('fs');
const path = require('path');

const ROOT = path.join(__dirname, '..');
const LOGS = path.join(ROOT, '_logs');
const PID_FILE = path.join(LOGS, 'bobby.pid');

function makeParts() {
  if (process.env.BOBBY_PARTS === 'test') {
    // Test override: dummy parts, one exits after 1 s (restart check), one keeps printing.
    return [
      { name: 'short', cmd: process.execPath, args: ['-e', 'console.log("hi");setTimeout(()=>{},1000)'] },
      { name: 'long', cmd: process.execPath, args: ['-e', 'setInterval(()=>console.log("tick"),500)'] },
    ];
  }
  return [
    {
      name: 'llm', cmd: path.join(ROOT, '_bin', 'llama', 'llama-server.exe'),
      args: ['-m', 'models/Qwen3-4B-Instruct-2507-Q4_K_M.gguf', '-ngl', '99', '-c', '8192', '-np', '1', '-fa', 'on', '-ctk', 'q8_0', '-ctv', 'q8_0', '--host', '127.0.0.1', '--port', '8080', '--log-file', '_logs/llm.log'],
      keep: /listening|error|failed|out of memory/i, // llama-server logs every request; keep what matters
    },
    { name: 'voice', cmd: path.join(ROOT, 'voice', '.venv', 'Scripts', 'python.exe'), args: ['voice/server.py'] },
    { name: 'bot', cmd: process.execPath, args: ['bot.js'], cwd: path.join(ROOT, 'bot') },
  ];
}

class Supervisor extends EventEmitter {
  constructor() {
    super();
    fs.mkdirSync(LOGS, { recursive: true });
    this.logFile = fs.createWriteStream(path.join(LOGS, 'bobby.log'), { flags: 'a' });
    this.parts = makeParts().map((def) => ({ def, child: null, restarts: 0, startedAt: null, timer: null }));
    this.running = false;
    this.since = null;
    this.external = false;
  }

  status() {
    return {
      running: this.running,
      since: this.since,
      external: !this.running && this.external,
      parts: this.parts.map((p) => ({ name: p.def.name, pid: p.child ? p.child.pid ?? null : null, up: !!p.child, restarts: p.restarts, startedAt: p.startedAt })),
    };
  }

  log(part, line) {
    const at = Date.now();
    this.logFile.write(`${new Date(at).toLocaleTimeString('fr-FR')} [${part}] ${line}\n`);
    this.emit('log', { part, line, at });
  }

  _changed() { this.emit('status', this.status()); }

  start() {
    if (this.running) return this.status();
    // One Bobby at a time: a second one would log the bot in twice and fight over the ports.
    try {
      const old = Number(fs.readFileSync(PID_FILE, 'utf8'));
      if (old && old !== process.pid) {
        process.kill(old, 0); // throws if that process is gone (stale pid file)
        const e = new Error(`Bobby is already running (pid ${old}). Run stop.cmd first.`);
        e.code = 'RUNNING';
        throw e;
      }
    } catch (e) { if (e.code === 'RUNNING') throw e; }
    fs.writeFileSync(PID_FILE, String(process.pid)); // lets stop.cmd find us
    this.running = true;
    this.external = false;
    this.since = Date.now();
    this.parts.forEach((p) => { p.restarts = 0; this._run(p); });
    return this.status();
  }

  _run(p) {
    const { def } = p;
    const child = spawn(def.cmd, def.args, {
      cwd: def.cwd || ROOT,
      env: { ...process.env, PYTHONIOENCODING: 'utf-8', PYTHONUNBUFFERED: '1' },
      windowsHide: true,
    });
    p.child = child;
    p.startedAt = Date.now();
    for (const stream of [child.stdout, child.stderr]) {
      let rest = '';
      stream.setEncoding('utf8');
      stream.on('data', (chunk) => {
        const lines = (rest + chunk).split(/\r?\n/);
        rest = lines.pop();
        for (const l of lines) if (l.trim() && (!def.keep || def.keep.test(l))) this.log(def.name, l);
      });
    }
    child.on('error', (e) => this.log(def.name, `cannot start: ${e.message}`));
    child.on('close', (code) => {
      if (p.child === child) p.child = null;
      if (!this.running) return;
      this.log(def.name, `stopped (exit code ${code}), restarting in 3 s`);
      p.timer = setTimeout(() => { p.timer = null; if (this.running) { p.restarts++; this._run(p); } }, 3000);
      this._changed();
    });
    this._changed();
  }

  stop() {
    if (!this.running) {
      if (this.external) { // started elsewhere (start.cmd): let stop.cmd do it, but keep the app server alive
        spawnSync('cmd', ['/c', path.join(ROOT, 'stop.cmd'), 'keep-app'], { cwd: ROOT, windowsHide: true });
        this.external = false;
        this._changed();
      }
      return this.status();
    }
    this.running = false;
    this.since = null;
    // ponytail: taskkill /T kills each part's whole process tree; sync so callers can exit only after they are gone
    for (const p of this.parts) {
      if (p.timer) { clearTimeout(p.timer); p.timer = null; }
      if (p.child) spawnSync('taskkill', ['/T', '/F', '/PID', String(p.child.pid)], { windowsHide: true });
      p.child = null;
      p.startedAt = null;
    }
    try { if (Number(fs.readFileSync(PID_FILE, 'utf8')) === process.pid) fs.unlinkSync(PID_FILE); } catch {}
    this._changed();
    return this.status();
  }

  restart() { this.stop(); return this.start(); }

  // Polls tasklist every 5 s: llama-server running while we are not => Bobby was started by start.cmd.
  watchExternal() {
    const check = () => execFile('tasklist', ['/FI', 'IMAGENAME eq llama-server.exe', '/NH'], { windowsHide: true }, (err, out) => {
      const ext = !err && !this.running && /llama-server\.exe/i.test(out);
      if (ext !== this.external) { this.external = ext; this._changed(); }
    });
    check();
    setInterval(check, 5000).unref();
  }
}

module.exports = { Supervisor, ROOT, LOGS };
