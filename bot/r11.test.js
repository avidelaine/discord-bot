// Mixer: a beep never cuts a reply in progress. Run: node bot/r11.test.js
const assert = require('assert');
const { PassThrough } = require('stream');
const { Mixer, FRAME } = require('./music/mixer');

const pcm = (frames) => new PassThrough().end(Buffer.alloc(FRAME * frames));
const m = new Mixer(() => {});
let replyEnded = false, beepBytes = null;
m.overlay(pcm(50), { reply: true }).then(() => { replyEnded = true; });
const first = m.over;
m.overlay(pcm(2), { beep: true }).then((b) => { beepBytes = b; });
setImmediate(() => {
  assert.strictEqual(m.over, first, 'beep must not replace the reply');
  assert.strictEqual(replyEnded, false);
  assert.strictEqual(beepBytes, 0, 'beep dropped');
  // without a reply, a beep still replaces the running overlay (the ring)
  const m2 = new Mixer(() => {});
  m2.overlay(pcm(50));
  const ring = m2.over;
  m2.overlay(pcm(2), { beep: true });
  assert.notStrictEqual(m2.over, ring);
  console.log('r11 ok');
  process.exit(0);
});
