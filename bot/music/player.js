// Music player: queue + transport controls on top of the mixer. Everything returns plain data
// (never throws) so voice commands can describe the result. Items to add/play:
//   { kind: 'radio', style: 'hardstyle' }  or  { kind: 'youtube', query: 'search text or URL' }
// Events: 'track' (nowPlaying) when a track starts, 'end' when the queue ran dry, 'error' (message).
const { EventEmitter } = require('events');
const { decode, radio, youtube } = require('./sources');

const label = (it) => it.title || (it.kind === 'radio' ? `${it.style} radio` : it.query);

class Player extends EventEmitter {
  constructor(mixer) {
    super();
    Object.assign(this, { mixer, queue: [], current: null, vol: 50, paused: false, token: 0 });
    mixer.volume = this.vol / 100;
  }

  nowPlaying() {
    const c = this.current;
    return c && { title: c.title, kind: c.kind, ...(c.station && { station: c.station }) };
  }

  async add(item) {
    if (!this.current) return { started: true, ...(await this.playNow(item)) };
    this.queue.push(item);
    return { started: false, position: this.queue.length, label: label(item) };
  }
  list() { return { nowPlaying: this.nowPlaying(), queue: this.queue.map((it) => ({ kind: it.kind, label: label(it) })) }; }
  clear() { const cleared = this.queue.length; this.queue = []; return { cleared }; }

  // -> nowPlaying() data, or { error }
  async playNow(item) {
    const id = ++this.token;
    this.mixer.stopMusic();
    this.current = null;
    this._setPaused(false);
    try {
      if (item.kind === 'radio') {
        const [first, ...alts] = await radio(item.style);
        item = { ...item, title: first.station, station: first.station, url: first.url, alts, live: true };
      } else if (!item.url) item = { ...item, ...(await youtube(item.query)) };
    } catch (e) { return this._fail(e, id); }
    if (id !== this.token) return { error: 'superseded' };
    return this._start(item);
  }

  _start(item) {
    this.current = item;
    this.mixer.setMusic(decode(item.url, { live: item.live }), (bytes) => this._ended(item, bytes));
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
      const r = await this.playNow(this.queue.shift());
      if (!r.error) return;
    }
    this.emit('end');
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
    this.mixer.stopMusic();
    const stopped = this.nowPlaying();
    this.current = null;
    this._setPaused(false);
    return { stopped, cleared };
  }
  async skip() {
    const skipped = this.nowPlaying();
    if (!skipped) return { error: 'nothing is playing' };
    this.mixer.stopMusic();
    await this._advance();
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
