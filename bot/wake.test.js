const assert = require('assert');
const { findWake } = require('./wake');

// Doit détecter (variantes réelles produites par Whisper FR/EN)
for (const s of ['Bobby ajoute du lait', 'hey bobbie', 'Boby raconte une blague',
                 'BOBI mets la musique', 'Bobé tu es là', "d'accord Bobby."]) {
  assert(findWake(s), `raté: ${s}`);
}

// Ne doit PAS déclencher (faux amis)
for (const s of ['un hobby sympa', 'le lobby de l hotel', 'mon baby dort',
                 'il a un beau body', 'rien a signaler']) {
  assert(findWake(s) === null, `faux positif: ${s}`);
}

// Le token rendu sert à nettoyer la requête
assert.strictEqual(findWake('Bobby mets la musique'), 'Bobby');

console.log('ok');
