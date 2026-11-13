// Tolerant detection of the wake word "bobby" in the transcribed text.
// ponytail: this list IS the tuning knob. Whisper hotwords already fix most spellings;
// these variants catch what Whisper still gets wrong.
// "baby", "hobby" and "lobby" are left out on purpose (false positives).
const VARIANTS = new Set([
  'bobby', 'bobbie', 'bobie', 'bobbi', 'bobi',
  'boby', 'bobbys', 'bobbey', 'bobbee', 'bobe',
  'moby', // "Moby est-ce que tu peux me redéplacer…" (2026-11-12): the wake model had heard "Bobby"
]);

const norm = (t) => t.normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase();

// Returns the original token that matches the wake word, or null.
function findWake(text) {
  for (const w of text.match(/\p{L}+/gu) || []) {
    const n = norm(w);
    // Also "Bobby" glued to the previous word by Whisper ("Dibobby", "Heybobby"); "hobby"/"lobby" never end in "bobby".
    if (VARIANTS.has(n) || /.bobb?(y|ie|i)$/.test(n)) return w;
  }
  return null;
}

module.exports = { findWake };
