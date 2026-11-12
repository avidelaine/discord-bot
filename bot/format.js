// Discord formatting: embeds for structured content, monospace tables, and splitting to Discord's limits
// (message 2000 chars; embed: title 256, description 4096, 25 fields, field name 256 / value 1024, 6000 in all;
// 10 embeds per message).
const COLOR = { info: 0x5865f2, todo: 0x57f287, rights: 0xfee75c, nicknames: 0xeb459e, claude: 0xd97757 };

const cut = (s, n) => (s.length > n ? `${s.slice(0, n - 1)}…` : s);

// Splits text at line breaks into pieces of at most `max` chars; a code block cut in two is closed and reopened.
function chunks(text, max = 2000) {
  const out = [];
  const lim = max - 8; // room to close a code block
  let rest = text;
  while (rest) {
    let n = rest.length <= lim ? rest.length : rest.lastIndexOf('\n', lim);
    if (n <= 0) n = lim;
    let piece = rest.slice(0, n);
    rest = rest.slice(n).replace(/^\n/, '');
    if ((piece.match(/```/g) || []).length % 2) { piece += '\n```'; if (rest) rest = `\`\`\`\n${rest}`; } // never reopen an empty rest
    out.push(piece);
  }
  return out;
}

const size = (e) => (e.title || '').length + (e.description || '').length + (e.footer?.text || '').length
  + (e.fields || []).reduce((n, f) => n + f.name.length + f.value.length, 0);

// One logical embed -> as many embeds as Discord's limits need. Long field values continue in extra fields.
function embed({ title, color = COLOR.info, description, fields = [], footer }) {
  const flat = [];
  for (const f of fields) {
    const parts = chunks(f.value || '-', 1024);
    parts.forEach((v, i) => flat.push({ name: cut(i ? `${f.name} (suite)` : f.name, 256), value: v, inline: Boolean(f.inline) }));
  }
  const [desc, ...more] = description ? chunks(description, 4096) : [];
  const out = [{ title: cut(title, 256), color, ...(desc && { description: desc }), fields: [] }];
  for (const d of more) out.push({ title: cut(`${title} (suite)`, 256), color, description: d, fields: [] });
  for (const f of flat) {
    let cur = out.at(-1);
    if (cur.fields.length >= 25 || size(cur) + f.name.length + f.value.length > 5500) {
      cur = { title: cut(`${title} (suite)`, 256), color, fields: [] };
      out.push(cur);
    }
    cur.fields.push(f);
  }
  if (footer) out.at(-1).footer = { text: cut(footer, 400) }; // fields stop at 5500: the footer stays under 6000
  return out;
}

// Monospace table with aligned columns, in a code block.
function table(headers, rows) {
  const all = [headers, ...rows].map((r) => r.map((c) => String(c ?? '')));
  const w = headers.map((_, i) => Math.max(...all.map((r) => r[i].length)));
  const fmt = (r) => r.map((c, i) => c.padEnd(w[i])).join('  ').trimEnd();
  return ['```', fmt(all[0]), w.map((n) => '-'.repeat(n)).join('  '), ...all.slice(1).map(fmt), '```'].join('\n');
}

// What io.post sends: a string, { embeds }, or a list of those -> Discord message payloads within limits.
function messages(content) {
  if (Array.isArray(content)) return content.flatMap(messages);
  if (typeof content === 'string') return chunks(content).map((c) => ({ content: c }));
  const out = [];
  for (const e of content.embeds || []) {
    const cur = out.at(-1);
    if (cur && cur.embeds.length < 10 && cur.embeds.reduce((n, x) => n + size(x), 0) + size(e) <= 6000) cur.embeds.push(e);
    else out.push({ embeds: [e] });
  }
  return out;
}

module.exports = { COLOR, chunks, embed, table, messages };
