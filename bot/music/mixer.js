// PCM mixer: a music channel (x volume) plus an overlay channel (Bobby's beep/TTS) summed into one
// s16le 48 kHz stereo stream. Music is ducked while the overlay plays. When nothing is playing the output
// stream ends (the AudioPlayer goes idle) and a new one is created on demand through play(readable).
const { Readable } = require('stream');

const FRAME = 3840; // 20 ms of s16le 48 kHz stereo
const DUCK = 0.25; // music gain while the overlay plays
const STEP = 0.1; // max gain change per frame: smooth ducking, no clicks
const FADE_OUT = 1; // seconds: fade out on stop / skip
const XFADE = 2; // seconds: crossfade between two tracks
const frames = (sec) => Math.round(sec * 50);

class Mixer {
  constructor(play, { duck = DUCK } = {}) {
    Object.assign(this, { play, duck, volume: 0.5, paused: false, music: null, fading: null, mf: 1, over: null, out: null, gain: 0 });
  }

  // stream: non-object Readable of raw PCM. onEnd(bytes) fires when it ends by itself (not when replaced/stopped).
  // fadeIn: ramp the new track up over XFADE seconds (crossfade with the one fading out).
  setMusic(stream, onEnd, fadeIn = false) {
    this.stopMusic();
    this.music = channel(stream, onEnd);
    this.mf = fadeIn ? 0 : 1;
    this._mfStep = 1 / frames(XFADE);
    this._ensure();
  }
  // fade: true (FADE_OUT) or a duration in seconds: the track ramps to 0 in the mixer instead of cutting.
  // A paused mixer just stops. Returns true when a track is now fading out.
  stopMusic(fade = false) {
    const m = this.music;
    this.music = null;
    if (!m) return false;
    if (!fade || this.paused) { m.stop(); return false; }
    this.fading?.ch.stop();
    this.fading = { ch: m, f: this.mf, /* a track still fading in fades out from where it is */ per: 1 / frames(fade === true ? FADE_OUT : fade) };
    return true;
  }
  pause(on) { this.paused = on; if (!on) this._ensure(); }
  // ponytail: a new overlay replaces the running one (same as player.play before). Resolves when it ends,
  // is replaced or is stopped.
  overlay(stream) {
    this.over?.finish();
    const p = new Promise((done) => { this.over = channel(stream, done); });
    this._ensure();
    return p;
  }

  // Bobby's voice stops now; the music comes back up.
  stopOverlay() { const o = this.over; this.over = null; o?.finish(); }

  _ensure() {
    if (this.out) return;
    const out = (this.out = new Readable({ highWaterMark: FRAME * 3, read: () => this._pushFrame(out) }));
    this.play(out);
  }

  _pushFrame(out) {
    const music = this.music && !this.paused ? this.music : null;
    const fd = this.fading;
    if (!music && !fd && !this.over) { this.out = null; return out.push(null); }
    const m = music?.take(), o = this.over?.take(), x = fd?.ch.take();
    const mf0 = this.mf, mf1 = (this.mf = music ? Math.min(1, mf0 + this._mfStep) : mf0);
    const f0 = fd?.f, f1 = fd && (fd.f = Math.max(0, fd.f - fd.per));
    const target = this.volume * (this.over ? this.duck : 1);
    const g0 = this.gain, g1 = (this.gain += Math.max(-STEP, Math.min(STEP, target - g0)));
    const buf = Buffer.alloc(FRAME);
    for (let i = 0; i < FRAME; i += 2) {
      const g = g0 + ((g1 - g0) * i) / FRAME;
      const t = i / FRAME;
      const v = (m ? m.readInt16LE(i) * g * (mf0 + (mf1 - mf0) * t) : 0) + (x ? x.readInt16LE(i) * g * (f0 + (f1 - f0) * t) : 0) +
        (o ? o.readInt16LE(i) : 0);
      buf.writeInt16LE(Math.max(-32768, Math.min(32767, Math.round(v))), i);
    }
    if (fd && (f1 === 0 || fd.ch.done)) { fd.ch.stop(); this.fading = null; }
    if (music?.done) { this.music = null; music.finish(); }
    if (this.over?.done) { const ov = this.over; this.over = null; ov.finish(); }
    out.push(buf);
  }
}

// Pulls exact frames from a stream; an underrun (slow network) gives silence, not a stall.
function channel(stream, onEnd) {
  const ch = { done: false, bytes: 0 };
  let ended = false, finished = false;
  stream.once('end', () => { ended = true; });
  stream.once('close', () => { ended = true; });
  stream.on('error', () => { ended = true; });
  ch.take = () => {
    const frame = Buffer.alloc(FRAME);
    const b = stream.destroyed ? null : stream.read(FRAME);
    if (b) { b.copy(frame); ch.bytes += b.length; }
    if (ended && (!b || b.length < FRAME)) ch.done = true;
    return frame;
  };
  ch.finish = () => { if (!finished) { finished = true; stream.destroy(); onEnd?.(ch.bytes); } };
  ch.stop = () => { finished = true; stream.destroy(); onEnd = null; };
  return ch;
}

module.exports = { Mixer, FRAME, FADE_OUT, XFADE };
