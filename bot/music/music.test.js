// Run: node bot/music/music.test.js  (the network part needs internet, ffmpeg and _bin/yt-dlp.exe)
const assert = require('assert');
const { PassThrough } = require('stream');
const { Mixer, FRAME, FADE_OUT, XFADE } = require('./mixer');
const { Player } = require('./player');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const dc = (value, frames) => {
  const b = Buffer.alloc(FRAME * frames);
  for (let i = 0; i < b.length; i += 2) b.writeInt16LE(value, i);
  return new PassThrough().end(b);
};

// Sink pulling frames every `ms`; records the first sample of each frame and the peak.
function sink(mixer, ms) {
  const s = { frames: [], peak: 0, ended: false };
  mixer.play = (out) => {
    const iv = setInterval(() => {
      for (let b, n = 0; n++ < 5 && (b = out.read(FRAME)); ) { // bounded: 'end' only fires between ticks
        s.frames.push(b.readInt16LE(0));
        for (let i = 0; i < FRAME; i += 2) s.peak = Math.max(s.peak, Math.abs(b.readInt16LE(i)));
      }
    }, ms);
    out.on('end', () => { s.ended = true; clearInterval(iv); });
  };
  return s;
}

async function synthetic() {
  const mixer = new Mixer(null);
  mixer.volume = 1;
  const s = sink(mixer, 1);
  mixer.setMusic(dc(8000, 300));
  while (s.frames.length < 60) await sleep(5);
  await mixer.overlay(dc(2000, 60)); // 1.2 s of "TTS"
  while (!s.ended) await sleep(5);
  const f = s.frames, near = (x, y) => Math.abs(x - y) < 50;
  assert(near(f[40], 8000), `music alone at full volume (${f[40]})`);
  const ducked = f.findIndex((v, i) => i > 50 && near(v, 8000 * 0.25 + 2000));
  assert(ducked > 55 && ducked < 80, `music ducked to 25% with overlay on top (frame ${ducked})`);
  assert(f.slice(ducked, ducked + 40).every((v) => v < 5000), 'stays ducked while the overlay plays');
  assert(near(f[f.length - 20], 8000), `music restored (${f[f.length - 20]})`);
  console.log('ok  mixer: ducking lowers music during the overlay, overlay audible, restored after, ends when idle');
}

async function fades() {
  const near = (x, y) => Math.abs(x - y) < 60;
  // fade out: ramps down to silence within FADE_OUT, then the stream ends
  let mixer = new Mixer(null), s = sink(mixer, 1);
  mixer.volume = 1;
  mixer.setMusic(dc(8000, 400));
  while (s.frames.length < 30) await sleep(5);
  assert(mixer.stopMusic(true), 'stopMusic(true) fades a playing track');
  const n0 = s.frames.length;
  while (!s.ended) await sleep(5);
  let f = s.frames.slice(n0 - 4); // a few frames were already buffered
  const start = f.findIndex((v) => v < 7900), zero = f.length;
  assert(start >= 0 && f[zero - 1] < 250 && zero <= start + FADE_OUT * 50 + 3, `fades to ~0 within ${FADE_OUT}s (${start}..${zero})`);
  assert(f.slice(start, zero).every((v, i, a) => !i || v <= a[i - 1]), 'monotonic decrease');
  // paused: no fade
  mixer = new Mixer(null); sink(mixer, 1);
  mixer.setMusic(dc(8000, 10)); mixer.pause(true);
  assert(!mixer.stopMusic(true) && !mixer.fading, 'paused: plain stop');
  // crossfade: old fades out while the new fades in, both audible in between
  mixer = new Mixer(null); s = sink(mixer, 1);
  mixer.volume = 1;
  mixer.setMusic(dc(8000, 400));
  while (s.frames.length < 30) await sleep(5);
  mixer.stopMusic(XFADE);
  mixer.setMusic(dc(4000, 120), null, true);
  while (!s.ended) await sleep(5);
  f = s.frames;
  assert(f.some((v) => v > 4600 && v < 7400), 'both tracks mixed during the crossfade');
  assert(f.slice(-30).some((v) => near(v, 4000)), 'new track reaches full level');
  console.log('ok  mixer: fade out reaches silence in time, crossfade mixes both tracks, stream ends when all is gone');
}

async function network() {
  const mixer = new Mixer(null);
  const s = sink(mixer, 20);
  const player = new Player(mixer);
  for (const item of [{ kind: 'radio', style: 'jazz' }, { kind: 'youtube', query: 'ytsearch1:lofi hip hop' }]) {
    s.peak = 0;
    const np = await player.playNow(item);
    assert(!np.error, np.error);
    await sleep(5000);
    console.log(`    ${np.kind}: "${np.title}" peak ${s.peak}`);
    assert(s.peak > 500, `${item.kind} produced non-silent PCM`);
    player.stop();
  }
  console.log('ok  network: radio by tag and YouTube both decode to non-silent PCM through the mixer');
}

(async () => {
  await synthetic();
  await fades();
  await network();
  process.exit(0);
})().catch((e) => { console.error('FAIL', e.message); process.exit(1); });
