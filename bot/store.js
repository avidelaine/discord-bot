const fs = require('fs');
const path = require('path');

// ponytail: persistence is a JSON file. A personal assistant keeps a few to-do items;
// move to SQLite the day there are real queries or concurrent writes.
const DIR = process.env.DATA_DIR || path.join(__dirname, '..', 'data');
fs.mkdirSync(DIR, { recursive: true });

const file = (name) => path.join(DIR, `${name}.json`);

function load(name, fallback) {
  try {
    return JSON.parse(fs.readFileSync(file(name), 'utf8'));
  } catch {
    return fallback;
  }
}

function save(name, data) {
  fs.writeFileSync(file(name), JSON.stringify(data, null, 2));
}

module.exports = { load, save };
