const assert = require('assert');
const { findWake } = require('./wake');

// Must trigger (real variants produced by Whisper in French and English)
for (const s of ['Bobby ajoute du lait', 'hey bobbie', 'Boby raconte une blague',
                 'BOBI mets la musique', 'Bobé tu es là', "d'accord Bobby.", 'Dibobby, coupe Kévin', 'Diboby.', 'Heybobby']) {
  assert(findWake(s), `missed: ${s}`);
}

// Must NOT trigger (look-alike words)
for (const s of ['un hobby sympa', 'le lobby de l hotel', 'mon baby dort',
                 'il a un beau body', 'rien a signaler']) {
  assert(findWake(s) === null, `false positive: ${s}`);
}

// The returned token is used to strip the wake word from the request
assert.strictEqual(findWake('Bobby mets la musique'), 'Bobby');

console.log('wake word: ok');
