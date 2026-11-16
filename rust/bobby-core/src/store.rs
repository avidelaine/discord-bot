//! Bobby's saved data (to-do items, nicknames, permissions) in SQLite `data/bobby.db`, mirrored to the
//! old `data/<name>.json` files after every write so the Node brain can take over at any time (rollback).
//! At open, a JSON file that differs from the database wins: Node may have written it since.
//! ponytail: one row per name holding the same JSON document as the file (a few dozen items);
//! real tables the day there are real queries.
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The documents the Node brain keeps in `data/`.
pub const NAMES: [&str; 3] = ["todo", "aliases", "permissions"];

pub struct Store {
    conn: Mutex<Connection>,
    json_dir: PathBuf,
}

impl Store {
    /// Opens (or creates) the database and imports every JSON file of `json_dir` that differs from it.
    pub fn open(db: &Path, json_dir: &Path) -> Result<Store, String> {
        let conn = Connection::open(db).map_err(|e| format!("open {}: {e}", db.display()))?;
        Self::with(conn, json_dir)
    }

    fn with(conn: Connection, json_dir: &Path) -> Result<Store, String> {
        conn.execute_batch("CREATE TABLE IF NOT EXISTS store (name TEXT PRIMARY KEY, json TEXT NOT NULL)").map_err(|e| e.to_string())?;
        std::fs::create_dir_all(json_dir).map_err(|e| e.to_string())?;
        let s = Store { conn: Mutex::new(conn), json_dir: json_dir.to_path_buf() };
        for name in NAMES {
            let Ok(text) = std::fs::read_to_string(s.file(name)) else { continue };
            let Ok(v) = serde_json::from_str::<Value>(&text) else { continue }; // a broken file never erases the database
            if s.load(name).as_ref() != Some(&v) {
                s.put(name, &v)?;
            }
        }
        Ok(s)
    }

    /// Tests: in-memory database, JSON mirror in a fresh temp folder (never the real data/).
    pub fn temp() -> Store {
        Self::with(Connection::open_in_memory().unwrap(), &temp_dir("store")).unwrap()
    }

    pub fn json_dir(&self) -> &Path {
        &self.json_dir
    }

    fn file(&self, name: &str) -> PathBuf {
        self.json_dir.join(format!("{name}.json"))
    }

    pub fn load(&self, name: &str) -> Option<Value> {
        let conn = self.conn.lock().unwrap();
        let text: Option<String> = conn.query_row("SELECT json FROM store WHERE name = ?1", [name], |r| r.get(0)).optional().ok().flatten();
        text.and_then(|t| serde_json::from_str(&t).ok())
    }

    /// `load(name, fallback)` of store.js.
    pub fn load_or(&self, name: &str, fallback: Value) -> Value {
        self.load(name).unwrap_or(fallback)
    }

    fn put(&self, name: &str, data: &Value) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute("INSERT INTO store (name, json) VALUES (?1, ?2) ON CONFLICT(name) DO UPDATE SET json = ?2", params![name, data.to_string()]).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Saves to the database, then mirrors to `<name>.json` exactly as `JSON.stringify(data, null, 2)`.
    pub fn save(&self, name: &str, data: &Value) -> Result<(), String> {
        self.put(name, data)?;
        self.mirror(name, data)
    }

    fn mirror(&self, name: &str, data: &Value) -> Result<(), String> {
        let tmp = self.json_dir.join(format!("{name}.json.tmp"));
        std::fs::write(&tmp, serde_json::to_string_pretty(data).unwrap()).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, self.file(name)).map_err(|e| e.to_string()) // never a half-written file
    }

    /// Writes every stored document back to its JSON file (rollback by hand).
    pub fn export(&self) -> Result<(), String> {
        for name in NAMES {
            if let Some(v) = self.load(name) {
                self.mirror(name, &v)?;
            }
        }
        Ok(())
    }
}

/// A fresh folder under the system temp dir (tests).
pub fn temp_dir(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let d = std::env::temp_dir().join(format!("bobby-core-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The real data/ of the main checkout, read only (copied first).
    fn real_data() -> Option<PathBuf> {
        let m = Path::new(env!("CARGO_MANIFEST_DIR"));
        [m.join("../../data"), m.join("../../../../../data")].into_iter().find(|d| d.join("todo.json").exists()) // main checkout, or a worktree of it
    }

    #[test]
    fn import_then_export_equals_the_originals() {
        let src = real_data().unwrap_or_else(|| {
            // no main checkout (Linux CI): a sample in Node's exact format
            let d = temp_dir("sample");
            std::fs::write(d.join("todo.json"), "[\n  {\n    \"id\": 1,\n    \"text\": \"Mettre des fondus « là »\",\n    \"list\": \"general\",\n    \"author\": {\n      \"id\": \"261531632589078528\",\n      \"name\": \"WakandeX\"\n    },\n    \"done\": false\n  }\n]").unwrap();
            std::fs::write(d.join("aliases.json"), "{\n  \"275277314739142656\": [\n    \"Godmode\"\n  ]\n}").unwrap();
            std::fs::write(d.join("permissions.json"), "{\n  \"blacklist\": [],\n  \"admins\": [\n    \"397478613559148545\"\n  ],\n  \"notAdmins\": []\n}").unwrap();
            d
        });
        let dir = temp_dir("import");
        for n in NAMES {
            std::fs::copy(src.join(format!("{n}.json")), dir.join(format!("{n}.json"))).unwrap();
        }
        let db = dir.join("bobby.db");
        let s = Store::open(&db, &dir).unwrap();
        for n in NAMES {
            std::fs::remove_file(dir.join(format!("{n}.json"))).unwrap();
        }
        drop(s);
        let s = Store::open(&db, &dir).unwrap(); // data now comes from SQLite only
        s.export().unwrap();
        for n in NAMES {
            let a = std::fs::read(src.join(format!("{n}.json"))).unwrap();
            let b = std::fs::read(dir.join(format!("{n}.json"))).unwrap();
            assert_eq!(String::from_utf8_lossy(&a), String::from_utf8_lossy(&b), "{n}.json");
        }
    }

    #[test]
    fn save_mirrors_and_newer_json_wins() {
        let dir = temp_dir("mirror");
        let db = dir.join("bobby.db");
        let s = Store::open(&db, &dir).unwrap();
        assert_eq!(s.load_or("aliases", json!({})), json!({}));
        s.save("aliases", &json!({"1": ["le Grec"]})).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("aliases.json")).unwrap(), "{\n  \"1\": [\n    \"le Grec\"\n  ]\n}");
        drop(s);
        // Node wrote the file meanwhile (rollback, then back to Rust): the file wins
        std::fs::write(dir.join("aliases.json"), r#"{"2": ["Boss"]}"#).unwrap();
        let s = Store::open(&db, &dir).unwrap();
        assert_eq!(s.load("aliases"), Some(json!({"2": ["Boss"]})));
    }
}
