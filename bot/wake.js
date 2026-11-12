// Tolerant detection of the wake word "bobby" in the transcribed text.
// ponytail: this list IS the tuning knob. Whisper hotwords already fix most spellings;
// these variants catch what Whisper still gets wrong.
// "baby", "hobby" and "lobby" are left out on purpose (false positives).
const VARIANTS = new Set([
  'bobby', 'bobbie', 'bobie', 'bobbi', 'bobi',
  'boby', 'bobbys', 'bobbey', 'bobbee', 'bobe',
]);

const norm = (t) => t.normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase();

// Returns the original token that matches the wake word, or null.
function findWake(text) {
  for (const w of text.match(/\p{L}+/gu) || []) {
    if (VARIANTS.has(norm(w))) return w;
  }
  return null;
}

module.exports = { findWake };
