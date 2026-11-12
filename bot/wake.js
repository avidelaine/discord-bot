// Détection tolérante du wake word "bobby" sur le texte transcrit.
// ponytail: cette liste EST le bouton de réglage. hotwords côté STT corrige déjà
// l'orthographe ; ces variantes rattrapent ce que Whisper écrit encore de travers.
// On évite volontairement "baby/hobby/lobby" (faux positifs).
const VARIANTS = new Set([
  'bobby', 'bobbie', 'bobie', 'bobbi', 'bobi',
  'boby', 'bobbys', 'bobbey', 'bobbee', 'bobe',
]);

const norm = (t) => t.normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase();

// Retourne le token original qui matche le wake word, sinon null.
function findWake(text) {
  for (const w of text.match(/\p{L}+/gu) || []) {
    if (VARIANTS.has(norm(w))) return w;
  }
  return null;
}

module.exports = { findWake };
