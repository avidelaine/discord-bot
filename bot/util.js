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

// One call to llama-server with a JSON schema; throws a spoken "brain down" error when it cannot be reached.
const LLM_URL = process.env.LLM_URL || 'http://127.0.0.1:8080';
async function llm(messages, schema, maxTokens = 300) {
  let res;
  try {
    res = await fetch(`${LLM_URL}/v1/chat/completions`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      // a JSON cut short by max_tokens fails to parse: keep "say" short in the prompts
      body: JSON.stringify({ messages, temperature: 0, max_tokens: maxTokens, response_format: { type: 'json_schema', json_schema: { name: 'plan', schema } } }),
    });
  } catch (e) {
    // fetch only throws when the server cannot be reached (ECONNREFUSED: llama-server is down or restarting)
    console.error(`[llm] llama-server unreachable at ${LLM_URL}: ${e.cause?.code || e.message}`);
    throw Object.assign(new Say('Mon cerveau ne répond pas.', "My brain isn't responding."), { code: 'LLM_DOWN' });
  }
  if (!res.ok) throw new Error(`LLM ${res.status}`);
  const text = (await res.json()).choices[0].message.content;
  // a JSON cut by max_tokens (a reply far too long): not understood, never "ça n'a pas marché"
  try { return JSON.parse(text); } catch { console.error(`[llm] invalid JSON: ${text.slice(0, 200)}`); throw Object.assign(new Say("Je n'ai pas compris.", "Sorry, I didn't get that."), { code: 'LLM_JSON' }); }
}

const words = (t) => (t || '').split(/\s+/).filter(Boolean).length;

// The recent conversation as prompt lines, for the local LLM, the "about" answers and Claude:
// who said what to Bobby, what Bobby did (plan) and what it answered. h: { at, who, query, did, reply }.
const ago = (at) => { const m = Math.round((Date.now() - at) / 60_000); return m < 1 ? "à l'instant" : `il y a ${m} min`; };
const talk = (history = []) => history.map((h) => `- ${h.at ? `${ago(h.at)}, ` : ''}${h.who || '?'} : "${h.query}"${h.did ? ` -> fait : ${h.did}` : ''}${h.reply ? ` -> Bobby : "${h.reply}"` : ''}`);

module.exports = { Say, norm, sim, llm, words, talk };
