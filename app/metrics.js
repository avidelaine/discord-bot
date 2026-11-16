// Samples system, GPU and per-part load every 2 s, keeps one averaged row per 10 s in _logs/metrics.db (30 days).
// Events: 'sample' (live sample, see sample()).
const { EventEmitter } = require('events');
const { spawn } = require('child_process');
const os = require('os');
const fs = require('fs');
const path = require('path');
const { DatabaseSync } = require('node:sqlite');
const { LOGS } = require('./supervisor');

const TICK = 2000, ROW_EVERY = 10000, KEEP_MS = 30 * 24 * 3600 * 1000, STALE = 6000;
const COLS = ['cpu', 'ram_used', 'ram_total', 'gpu', 'vram_used', 'vram_total', 'gpu_temp', 'llm_cpu', 'llm_ram', 'voice_cpu', 'voice_ram', 'bot_cpu', 'bot_ram'];
const camel = (c) => c.replace(/_(\w)/g, (_, x) => x.toUpperCase());
const PART_NAMES = ['llm', 'voice', 'bot'];
const round = (n) => (n == null ? null : Math.round(n * 10) / 10);

// Samples the pids listed in the pid file: "pid cpuMs workingSetBytes" lines, then "---". Children (one level) are summed in,
// because voice's venv python.exe is only a launcher.
const PS_SCRIPT = `param($PidFile)
$ErrorActionPreference = 'SilentlyContinue'
while ($true) {
  foreach ($id in (Get-Content $PidFile)) {
    if (-not $id) { continue }
    $ids = @([int]$id) + @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$id" | ForEach-Object { $_.ProcessId })
    $cpu = 0; $ws = 0; $alive = $false
    foreach ($p in (Get-Process -Id $ids)) { $alive = $true; $cpu += $p.TotalProcessorTime.TotalMilliseconds; $ws += $p.WorkingSet64 }
    if ($alive) { '{0} {1} {2}' -f $id, [long]$cpu, $ws }
  }
  '---'
  Start-Sleep -Seconds 2
}
`;

function cpuTimes() {
  let idle = 0, total = 0;
  for (const { times } of os.cpus()) { idle += times.idle; total += times.user + times.nice + times.sys + times.idle + times.irq; }
  return { idle, total };
}

class Metrics extends EventEmitter {
  constructor(supervisor) {
    super();
    this.sup = supervisor;
    this.gpu = null; this.gpuAt = 0;
    this.ps = { parts: {}, at: 0 }; // last finished PowerShell block
    this.prevPs = new Map(); // pid -> { cpuMs, t }
    this.pidNames = new Map(); // pid -> part name
    this.pidKey = '';
    this.acc = {}; this.lastRow = Date.now();
  }

  start() {
    this.db = new DatabaseSync(path.join(LOGS, 'metrics.db'));
    this.db.exec(`CREATE TABLE IF NOT EXISTS metrics(ts INTEGER, ${COLS.map((c) => c + ' REAL').join(', ')});
      CREATE INDEX IF NOT EXISTS metrics_ts ON metrics(ts);`);
    this.db.prepare('DELETE FROM metrics WHERE ts < ?').run(Date.now() - KEEP_MS);
    this.insert = this.db.prepare(`INSERT INTO metrics VALUES (?${', ?'.repeat(COLS.length)})`);
    this.prevCpu = cpuTimes();
    this._startGpu();
    this._startPs();
    this.sup.on('status', () => this._writePids());
    this._writePids();
    setInterval(() => this._sample(), TICK);
  }

  _startGpu() {
    let child;
    try {
      child = spawn('nvidia-smi', ['--query-gpu=utilization.gpu,memory.used,memory.total,temperature.gpu', '--format=csv,noheader,nounits', '-l', '2'], { windowsHide: true });
    } catch { return; }
    const off = () => { this.gpu = null; };
    child.on('error', off);
    child.on('close', off);
    let rest = '';
    child.stdout.setEncoding('utf8');
    child.stdout.on('data', (chunk) => {
      const lines = (rest + chunk).split(/\r?\n/);
      rest = lines.pop();
      for (const l of lines) {
        const v = l.split(',').map((x) => { const n = Number(x.trim()); return x.trim() === '' || Number.isNaN(n) ? null : n; });
        if (v.length === 4) { this.gpu = { util: v[0], vramUsed: v[1], vramTotal: v[2], temp: v[3] }; this.gpuAt = Date.now(); }
      }
    });
  }

  _startPs() {
    this.pidFile = path.join(LOGS, 'app-pids.txt');
    fs.writeFileSync(this.pidFile, '');
    const script = path.join(LOGS, 'app-sampler.ps1');
    fs.writeFileSync(script, PS_SCRIPT);
    const child = spawn('powershell', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', script, this.pidFile], { windowsHide: true });
    child.on('error', () => {});
    let rest = '', block = [];
    child.stdout.setEncoding('utf8');
    child.stdout.on('data', (chunk) => {
      const lines = (rest + chunk).split(/\r?\n/);
      rest = lines.pop();
      for (const l of lines) {
        if (l.trim() === '---') { this._psBlock(block); block = []; } else if (l.trim()) block.push(l.trim().split(/\s+/).map(Number));
      }
    });
  }

  _psBlock(block) {
    const t = Date.now(), ncpu = os.cpus().length, parts = {}, next = new Map();
    for (const [pid, cpuMs, ws] of block) {
      const prev = this.prevPs.get(pid), name = this.pidNames.get(pid);
      next.set(pid, { cpuMs, t });
      if (!name) continue;
      const cpu = prev ? Math.max(0, (cpuMs - prev.cpuMs) / ((t - prev.t) * ncpu) * 100) : null;
      parts[name] = { cpu: round(cpu), ram: round(ws / 1048576) };
    }
    this.prevPs = next;
    this.ps = { parts, at: t };
  }

  // Rewrites the pid file only when the pids changed.
  _writePids() {
    const pairs = this.sup.status().parts.filter((p) => p.pid).map((p) => [p.pid, p.name]);
    const key = pairs.map((x) => x.join(':')).join(',');
    if (key === this.pidKey) return;
    this.pidKey = key;
    this.pidNames = new Map(pairs);
    try { fs.writeFileSync(this.pidFile, pairs.map((x) => x[0]).join('\n')); } catch {}
  }

  _sample() {
    const now = Date.now(), cur = cpuTimes();
    const dTotal = cur.total - this.prevCpu.total, dIdle = cur.idle - this.prevCpu.idle;
    this.prevCpu = cur;
    const ramTotal = os.totalmem() / 1048576;
    const gpu = this.gpu && now - this.gpuAt < STALE ? this.gpu : null;
    const fresh = now - this.ps.at < STALE;
    const parts = {};
    for (const n of PART_NAMES) parts[n] = (fresh && this.ps.parts[n]) || null;
    const s = {
      ts: now,
      cpu: round(dTotal > 0 ? 100 * (1 - dIdle / dTotal) : 0),
      ram: { used: round(ramTotal - os.freemem() / 1048576), total: round(ramTotal) },
      gpu, parts,
    };
    const flat = {
      cpu: s.cpu, ram_used: s.ram.used, ram_total: s.ram.total,
      gpu: gpu?.util, vram_used: gpu?.vramUsed, vram_total: gpu?.vramTotal, gpu_temp: gpu?.temp,
    };
    for (const n of PART_NAMES) { flat[`${n}_cpu`] = parts[n]?.cpu; flat[`${n}_ram`] = parts[n]?.ram; }
    for (const [k, v] of Object.entries(flat)) {
      if (v == null) continue;
      const a = (this.acc[k] ||= { sum: 0, n: 0 });
      a.sum += v; a.n++;
    }
    if (now - this.lastRow >= ROW_EVERY) {
      this.insert.run(now, ...COLS.map((c) => (this.acc[c] ? this.acc[c].sum / this.acc[c].n : null)));
      this.acc = {}; this.lastRow = now;
    }
    this.emit('sample', s);
  }

  // Rows averaged per `step` seconds bucket (ts = bucket start).
  history(from, to, step) {
    const b = Math.max(1, Math.floor(step) || 10) * 1000; // inlined: node:sqlite binds numbers as REAL, which would break integer division
    const rows = this.db.prepare(
      `SELECT (CAST(ts AS INTEGER)/${b})*${b} AS ts, ${COLS.map((c) => `AVG(${c}) AS ${c}`).join(', ')}
       FROM metrics WHERE ts BETWEEN ? AND ? GROUP BY CAST(ts AS INTEGER)/${b} ORDER BY ts`).all(from, to);
    return rows.map((r) => Object.fromEntries(Object.entries(r).map(([k, v]) => [camel(k), v])));
  }
}

module.exports = { Metrics };
