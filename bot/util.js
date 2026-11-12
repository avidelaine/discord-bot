// Small helpers shared by the brain and the action modules.

// Error spoken to the user as is, in their language.
class Say extends Error {
  constructor(fr, en) { super(fr); this.en = en; }
  in(lang) { return lang === 'en' ? this.en : this.message; }
}

const norm = (s) => s.normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase().replace(/[^a-z0-9]/g, '');

function lev(a, b) {
  let prev = [...Array(b.length + 1).keys()];
  for (let i = 1; i <= a.length; i++) {
    const cur = [i];
    for (let j = 1; j <= b.length; j++) cur[j] = Math.min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (a[i - 1] === b[j - 1] ? 0 : 1));
    prev = cur;
  }
  return prev[b.length];
}

function sim(q, c) {
  const a = norm(q), b = norm(c);
  if (!a || !b) return 0;
  if (a === b) return 1;
  if (Math.min(a.length, b.length) >= 3 && (a.includes(b) || b.includes(a))) return 0.9;
  return 1 - lev(a, b) / Math.max(a.length, b.length);
}

module.exports = { Say, norm, sim };
