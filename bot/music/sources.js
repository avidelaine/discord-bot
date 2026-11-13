// Where sound comes from: ffmpeg decoding, Radio Browser (web radio by style), yt-dlp (YouTube).
const path = require('path');
const { spawn, execFile } = require('child_process');

const UA = 'Bobby/1.0';
const YTDLP = path.join(__dirname, '..', '..', '_bin', 'yt-dlp.exe');
const RADIO_API = 'https://de1.api.radio-browser.info/json/stations';

// url string or Readable (e.g. a WAV from the TTS) -> raw s16le 48 kHz stereo stream. Destroying it kills ffmpeg.
function decode(src, { live = false } = {}) {
  const url = typeof src === 'string';
  const input = url
    ? [...(live ? ['-reconnect', '1', '-reconnect_streamed', '1', '-reconnect_delay_max', '5'] : []), '-user_agent', UA, '-i', src]
    : ['-i', 'pipe:0'];
  const ff = spawn('ffmpeg', ['-nostdin', '-loglevel', 'error', ...input, '-vn', '-f', 's16le', '-ar', '48000', '-ac', '2', 'pipe:1'], {
    stdio: [url ? 'ignore' : 'pipe', 'pipe', 'ignore'],
  });
  if (!url) {
    ff.stdin.on('error', () => {});
    src.on('error', () => ff.kill()).pipe(ff.stdin);
  }
  ff.on('error', (e) => ff.stdout.destroy(e));
  ff.stdout.on('close', () => ff.kill());
  return ff.stdout;
}

// Stations for a style/genre, best voted first: [{ station, url }]
async function radio(style) {
  const tag = encodeURIComponent(style.trim().toLowerCase());
  const q = '?hidebroken=true&order=votes&reverse=true&limit=10';
  for (const route of [`bytag/${tag}`, `bytagexact/${tag}`, `byname/${tag}`]) {
    const res = await fetch(`${RADIO_API}/${route}${q}`, { headers: { 'User-Agent': UA } });
    const list = (await res.json()).filter((s) => s.url_resolved);
    if (list.length) return list.map((s) => ({ station: s.name.trim(), url: s.url_resolved }));
  }
  throw new Error(`no radio found for "${style}"`);
}

// Search text or URL -> { title, url } of the best audio stream.
// -I 1: an artist name ("Adèle Castillon") makes ytsearch1 return a whole shelf of videos, 33 s instead of 3.5 s.
const youtube = (query) => new Promise((resolve, reject) => {
  const target = /^https?:\/\//.test(query) ? query : `ytsearch1:${query.replace(/^ytsearch1:/, '')}`;
  const child = execFile(YTDLP, ['-f', 'bestaudio/best', '--no-playlist', '-I', '1', '--no-warnings', '-j', target], { maxBuffer: 1 << 26 }, (err, out) => {
    clearTimeout(timer);
    if (err) return reject(new Error(`yt-dlp failed: ${err.message.split('\n')[0]}`));
    let j = null;
    try { j = JSON.parse(out.split('\n')[0]); } catch { /* no result: empty output */ }
    if (!j?.url) return reject(new Error(`nothing found on YouTube for "${query}"`));
    resolve({ title: j.title, url: j.url });
  });
  // execFile's own timeout kills yt-dlp.exe but not its child (PyInstaller), which keeps the pipes open: the
  // callback never came and the request hung forever (2026-11-13). Reject ourselves and kill the whole tree.
  const timer = setTimeout(() => {
    reject(new Error('yt-dlp failed: no answer after 20 s'));
    if (process.platform === 'win32') spawn('taskkill', ['/pid', String(child.pid), '/T', '/F']); else child.kill('SIGKILL');
  }, 20000);
});

module.exports = { decode, radio, youtube };
