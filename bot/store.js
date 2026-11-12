const fs = require('fs');
const path = require('path');

// ponytail: persistance = fichier JSON. Un assistant perso a une todo de
// quelques items ; on passe à SQLite le jour où il y a des requêtes/concurrence.
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
