// Local control app for Bobby: HTTP API + SSE live feed + static UI on 127.0.0.1:4100. Closing the UI never stops Bobby.
process.removeAllListeners('warning'); // silence node:sqlite's ExperimentalWarning
const http = require('http');
const fs = require('fs');
const path = require('path');
const { Supervisor, LOGS } = require('./supervisor');
const { Metrics } = require('./metrics');

const PORT = Number(process.env.APP_PORT) || 4100;
const UI = path.join(__dirname, 'ui');
const JOURNAL = path.join(LOGS, 'conversations.jsonl');
const APP_PID = path.join(LOGS, 'app.pid');
const LOG_FILES = { bot: 'bot.log', voice: 'voice.log', llm: 'llm.log', app: 'bobby.log' };
const MIME = { '.html': 'text/html; charset=utf-8', '.js': 'text/javascript', '.css': 'text/css', '.json': 'application/json', '.svg': 'image/svg+xml', '.png': 'image/png', '.ico': 'image/x-icon' };

const sup = new Supervisor();
const metrics = new Metrics(sup);

// ---- SSE ----
const clients = new Set();
function send(event, data, res) {
  const msg = `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`;
  if (res) res.write(msg); else for (const r of clients) r.write(msg);
}
sup.on('status', (s) => send('status', s));
sup.on('log', (l) => send('log', l));
metrics.on('sample', (s) => send('metrics', s));

// ---- journal: tail-follow (poll size every 1 s, read only new bytes) and read-from-the-end ----
let jPos = 0, jRest = Buffer.alloc(0);
try { jPos = fs.statSync(JOURNAL).size; } catch {}
setInterval(() => {
  let size;
  try { size = fs.statSync(JOURNAL).size; } catch { return; }
  if (size < jPos) { jPos = size; jRest = Buffer.alloc(0); return; } // truncated
  if (size === jPos) return;
  const buf = Buffer.alloc(size - jPos);
  const fd = fs.openSync(JOURNAL, 'r');
  fs.readSync(fd, buf, 0, buf.length, jPos);
  fs.closeSync(fd);
  jPos = size;
  const all = Buffer.concat([jRest, buf]);
  const nl = all.lastIndexOf(10);
  if (nl < 0) { jRest = all; return; }
  jRest = all.subarray(nl + 1); // partial last line waits for its end
  for (const line of all.subarray(0, nl).toString('utf8').split('\n')) {
    if (!line.trim()) continue;
    try { send('journal', JSON.parse(line)); } catch {}
  }
}, 1000).unref();

const isoOrNull = (v) => { const d = v && new Date(v); return d && !Number.isNaN(d.getTime()) ? d.toISOString() : null; };

// ponytail: assumes the file is in chronological order (stops at the first item older than `from`)
function readJournal(limit, before, from) {
  const items = [];
  let fd;
  try { fd = fs.openSync(JOURNAL, 'r'); } catch { return items; }
  const CHUNK = 256 * 1024;
  let pos = fs.fstatSync(fd).size, rest = Buffer.alloc(0), done = false;
  while (pos > 0 && !done) {
    const len = Math.min(CHUNK, pos);
    pos -= len;
    const chunk = Buffer.alloc(len);
    fs.readSync(fd, chunk, 0, len, pos);
    const buf = Buffer.concat([chunk, rest]);
    let start = buf.indexOf(10);
    if (pos > 0 && start >= 0) { rest = buf.subarray(0, start); start += 1; } else if (pos > 0) { rest = buf; continue; } else { rest = Buffer.alloc(0); start = 0; }
    const lines = buf.subarray(start).toString('utf8').split('\n');
    for (let i = lines.length - 1; i >= 0 && !done; i--) {
      if (!lines[i].trim()) continue;
      let o; try { o = JSON.parse(lines[i]); } catch { continue; }
      if (from && o.at < from) { done = true; break; }
      if (before && !(o.at < before)) continue;
      items.push(o);
      if (items.length >= limit) done = true;
    }
  }
  fs.closeSync(fd);
  return items;
}

function journalStats(items) {
  const byOutcome = {}, totals = [];
  for (const it of items) {
    byOutcome[it.outcome] = (byOutcome[it.outcome] || 0) + 1;
    if (typeof it.timing?.total === 'number') totals.push(it.timing.total);
  }
  totals.sort((a, b) => a - b);
  const m = totals.length >> 1;
  const medianTotalMs = totals.length ? (totals.length % 2 ? totals[m] : (totals[m - 1] + totals[m]) / 2) : null;
  return { count: items.length, byOutcome, medianTotalMs };
}

function tailLines(file, n) {
  let fd;
  try { fd = fs.openSync(file, 'r'); } catch { return []; }
  const size = fs.fstatSync(fd).size;
  let len = 64 * 1024, text = '';
  for (;;) {
    len = Math.min(len, size);
    const buf = Buffer.alloc(len);
    fs.readSync(fd, buf, 0, len, size - len);
    text = buf.toString('utf8');
    if (len >= size || text.split('\n').length > n + 1) break;
    len *= 4;
  }
  fs.closeSync(fd);
  const lines = text.split(/\r?\n/);
  if (len < size) lines.shift(); // first line is cut
  while (lines.length && !lines[lines.length - 1]) lines.pop();
  return lines.slice(-n);
}

// ---- HTTP ----
function json(res, code, body) {
  res.writeHead(code, { 'Content-Type': 'application/json; charset=utf-8', 'Cache-Control': 'no-store' });
  res.end(JSON.stringify(body));
}

function serveStatic(res, pathname) {
  const file = path.join(UI, pathname === '/' ? 'index.html' : decodeURIComponent(pathname));
  if (!file.startsWith(UI + path.sep) || !fs.existsSync(file) || !fs.statSync(file).isFile()) return json(res, 404, { error: 'not found' });
  res.writeHead(200, { 'Content-Type': MIME[path.extname(file).toLowerCase()] || 'application/octet-stream', 'Cache-Control': 'no-store' });
  fs.createReadStream(file).pipe(res);
}

function action(res, fn) {
  try { json(res, 200, fn()); } catch (e) { json(res, 409, { error: e.message, status: sup.status() }); }
}

const server = http.createServer((req, res) => {
  try {
    const url = new URL(req.url, 'http://127.0.0.1');
    const q = url.searchParams, p = url.pathname;
    if (req.method === 'POST' && ['/api/start', '/api/stop', '/api/restart'].includes(p)) {
      // A web page open in the browser could POST here (no preflight for a plain POST): only the app's own page may.
      const origin = req.headers.origin;
      if (origin && !/^http:\/\/(?:127\.0\.0\.1|localhost):\d+$/.test(origin)) return json(res, 403, { error: 'forbidden origin' });
      if (p === '/api/stop') return action(res, () => sup.stop());
      if (sup.status().external) return json(res, 409, { error: 'Bobby was started elsewhere (start.cmd): stop it first.', status: sup.status() });
      return action(res, () => { sup.log('bobby', `${p.slice(5)} from the app`); return p === '/api/start' ? sup.start() : sup.restart(); });
    }
    if (req.method !== 'GET') return json(res, 405, { error: 'method not allowed' });
    if (p === '/api/status') return json(res, 200, sup.status());
    if (p === '/api/live') {
      res.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-store', Connection: 'keep-alive' });
      clients.add(res);
      req.on('close', () => clients.delete(res));
      send('status', sup.status(), res);
      return;
    }
    if (p === '/api/metrics') {
      const to = Number(q.get('to')) || Date.now();
      const from = Number(q.get('from')) || to - 3600 * 1000;
      return json(res, 200, metrics.history(from, to, Number(q.get('step')) || 10));
    }
    if (p === '/api/journal') {
      const limit = Math.min(Math.max(Number(q.get('limit')) || 500, 1), 5000);
      const items = readJournal(limit, isoOrNull(q.get('before')), isoOrNull(q.get('from')));
      return json(res, 200, { items, stats: journalStats(items) });
    }
    if (p === '/api/logs') {
      const file = LOG_FILES[q.get('part')];
      if (!file) return json(res, 400, { error: 'part must be bot, voice, llm or app' });
      return json(res, 200, { lines: tailLines(path.join(LOGS, file), Math.min(Math.max(Number(q.get('lines')) || 300, 1), 5000)) });
    }
    if (p.startsWith('/api/')) return json(res, 404, { error: 'not found' });
    serveStatic(res, p);
  } catch (e) {
    json(res, 500, { error: e.message });
  }
});

server.on('error', (e) => { if (e.code === 'EADDRINUSE') process.exit(0); throw e; }); // already running: stay quiet
server.listen(PORT, '127.0.0.1', () => {
  fs.mkdirSync(LOGS, { recursive: true });
  fs.writeFileSync(APP_PID, String(process.pid)); // lets stop.cmd find us
  metrics.start(); // only the instance that owns the port touches the database
  sup.watchExternal();
});

function quit() {
  sup.stop();
  try { if (Number(fs.readFileSync(APP_PID, 'utf8')) === process.pid) fs.unlinkSync(APP_PID); } catch {}
  process.exit(0);
}
process.on('SIGINT', quit);
process.on('SIGTERM', quit);
