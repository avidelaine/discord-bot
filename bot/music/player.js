// Music player: queue + transport controls on top of the mixer. Everything returns plain data
// (never throws) so voice commands can describe the result. Items to add/play:
//   { kind: 'radio', style: 'hardstyle' }  or  { kind: 'youtube', query: 'search text or URL' }  or  { kind: 'style', style: 'hardstyle' }
// Endless mode (item.endless): when the queue runs dry, Bobby keeps going on its own. 'similar' follows the YouTube Mix of
// the last track (same style, any artist; a style is seeded by a search); 'artist' plays only songs of item.seed.
// Events: 'track' (nowPlaying) when a track starts, 'end' when the queue ran dry, 'error' (message).
const { EventEmitter } = require('events');
const { decode, radio, youtube, listing } = require('./sources');
const { XFADE } = require('./mixer');

const watch = (id) => `https://www.youtube.com/watch?v=${id}`;
const norm = (t = '') => t.normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase();
const song = (e) => e.duration >= 60 && e.duration <= 600; // a song: not a 2 h mix, a live set or a podcast
// "Judas (Hardstyle)" and "JUDAS (HARDSTYLE)" are the same song for the "already played" check
const key = (title) => norm(title).replace(/\(.*?\)|\[.*?\]|official|video|clip|lyrics?|audio|hd|4k/g, '').replace(/[^a-z0-9]+/g, ' ').trim();
const label = (it) => it.title || (it.kind === 'radio' ? `${it.style} radio` : it.query);

class Player extends EventEmitter {
  constructor(mixer) {
    super();
    Object.assign(this, { mixer, queue: [], current: null, vol: 50, paused: false, token: 0 });
    mixer.volume = this.vol / 100;
  }

  nowPlaying() {
    const c = this.current;
    return c && { title: c.title, kind: c.kind, ...(c.station && { station: c.station }), ...(this.auto && { endless: this.auto.mode, seed: this.auto.seed }) };
  }

  async add(item) {
    if (!this.current) return { started: true, ...(await this.playNow(item)) };
    this.queue.push(item);
    return { started: false, position: this.queue.length, label: label(item) };
  }
  list() { return { nowPlaying: this.nowPlaying(), endless: this.auto && { mode: this.auto.mode, seed: this.auto.seed }, queue: this.queue.map((it) => ({ kind: it.kind, label: label(it) })) }; }
  clear() { const cleared = this.queue.length; this.queue = []; return { cleared }; }

  // -> nowPlaying() data, or { error }. fresh: a new request from someone (sets or ends the endless mode);
  // false for the queue and the endless mode's own next tracks.
  async playNow(item, fresh = true) {
    const id = ++this.token;
    try { // the old track keeps playing while the new one is looked up
      if (item.kind === 'style') { // "joue du hardstyle": a real song of that style seeds the endless mix
        const seed = (await listing(`ytsearch15:${item.style}`, 15)).find(song);
        if (!seed) throw new Error(`nothing found on YouTube for "${item.style}"`);
        item = { kind: 'youtube', query: watch(seed.id), endless: 'similar', seed: item.style };
      }
      if (item.kind === 'radio') {
        const [first, ...alts] = await radio(item.style);
        item = { ...item, title: first.station, station: first.station, url: first.url, alts, live: true };
      } else if (!item.url) item = { ...item, ...(await youtube(item.query)) };
    } catch (e) { return this._fail(e, id); }
    if (id !== this.token) return { error: 'superseded' };
    if (fresh) this.auto = item.endless ? { mode: item.endless, seed: item.seed || item.query, played: new Set(), pool: [] } : null;
    this.upcoming = null;
    const cross = this.mixer.stopMusic(XFADE); // old track fades out while the new one fades in
    this._setPaused(false);
    return this._start(item, cross);
  }

  _start(item, cross = false) {
    this.current = item;
    this.mixer.setMusic(decode(item.url, { live: item.live }), (bytes) => this._ended(item, bytes), cross);
    if (this.auto && item.kind === 'youtube') {
      this.auto.played.add(item.id).add(key(item.title));
      this.auto.last = item.id;
      // the next track is found and resolved while this one plays: no gap at the end, "skip" is instant
      if (!this.queue.length) (this.upcoming = this._nextAuto().catch(() => null)).catch(() => {});
    }
    this.emit('track', this.nowPlaying());
    return this.nowPlaying();
  }

  _ended(item, bytes) {
    if (this.current !== item) return;
    // a radio stream that died before any sound: try the next station of the list
    if (item.alts?.length && !bytes) {
      const [next, ...alts] = item.alts;
      return this._start({ ...item, title: next.station, station: next.station, url: next.url, alts });
    }
    this._advance();
  }

  async _advance() {
    this.current = null;
    while (this.queue.length) {
      const r = await this.playNow(this.queue.shift(), false);
      if (!r.error) return;
    }
    for (let tries = 0; this.auto && tries < 3; tries++) {
      const auto = this.auto, next = await (this.upcoming || this._nextAuto().catch(() => null));
      this.upcoming = null;
      if (this.auto !== auto || this.current) return; // stopped or replaced meanwhile
      if (next && !(await this.playNow(next, false)).error) return;
    }
    this.emit('end');
  }

  // Next track of the endless mode, stream already resolved. ponytail: a fresh listing each time the pool runs
  // out (2 s, hidden behind the current track); the mix drifts slowly from the seed, like a radio would.
  async _nextAuto() {
    const a = this.auto;
    const fresh = (e) => song(e) && !a.played.has(e.id) && !a.played.has(key(e.title));
    if (!a.pool.filter(fresh).length) {
      const list = a.mode === 'artist'
        ? (await listing(`ytsearch30:${a.seed}`, 30)).filter((e) => norm(`${e.title} ${e.channel}`).includes(norm(a.seed)))
        : await listing(`${watch(a.last)}&list=RD${a.last}`, 25);
      if (a.mode === 'artist' && !list.some(fresh)) a.played.clear(); // every song of the artist played: start over
      a.pool = list;
    }
    const pool = a.pool.filter(fresh);
    if (!pool.length) return null;
    // the mix lists the closest tracks first: pick among the first few so two sessions don't sound the same
    const e = a.mode === 'artist' ? pool[Math.floor(Math.random() * pool.length)] : pool[Math.floor(Math.random() * Math.min(5, pool.length))];
    a.pool = a.pool.filter((x) => x !== e);
    return { kind: 'youtube', query: e.title, ...(await youtube(watch(e.id))) };
  }

  _fail(e, id) {
    if (id === this.token && this.listenerCount('error')) this.emit('error', e.message);
    return { error: e.message };
  }

  _setPaused(on) { this.paused = on; this.mixer.pause(on); }
  // ponytail: pausing a live radio just stops reading it; it may resume a bit behind live.
  pause() {
    if (!this.current) return { error: 'nothing is playing' };
    this._setPaused(true);
    return { paused: true, nowPlaying: this.nowPlaying() };
  }
  resume() {
    if (!this.current) return { error: 'nothing is playing' };
    this._setPaused(false);
    return { paused: false, nowPlaying: this.nowPlaying() };
  }
  // Stops playback and empties the queue.
  stop() {
    const { cleared } = this.clear();
    this.token++;
    this.auto = null;
    this.upcoming = null;
    this.mixer.stopMusic(true);
    const stopped = this.nowPlaying();
    this.current = null;
    this._setPaused(false);
    return { stopped, cleared };
  }
  async skip() {
    const skipped = this.nowPlaying();
    if (!skipped) return { error: 'nothing is playing' };
    await this._advance(); // playNow crossfades into the next track
    if (!this.current) this.mixer.stopMusic(true); // nothing next: fade out
    return { skipped, nowPlaying: this.nowPlaying() };
  }
  // Restart the current track from the beginning (for radio: reconnect).
  replay() {
    if (!this.current) return { error: 'nothing is playing' };
    this.mixer.stopMusic();
    this._setPaused(false);
    return { nowPlaying: this._start(this.current) };
  }

  volume() { return { volume: this.vol }; }
  setVolume(n) {
    this.vol = Math.max(0, Math.min(100, Math.round(n)));
    this.mixer.volume = this.vol / 100;
    return this.volume();
  }
  volumeUp(step = 10) { return this.setVolume(this.vol + step); }
  volumeDown(step = 10) { return this.setVolume(this.vol - step); }
}

module.exports = { Player };
