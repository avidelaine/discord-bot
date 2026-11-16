//! The journal (one JSON line per exchange in `_logs/conversations.jsonl`) and the memory rebuilt from it:
//! the recent conversation and what went wrong lately. Port of the journal part of `bot/brain.js`.
use crate::util::Hist;
use serde_json::{Map, Value};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const HISTORY_MAX: usize = 10;
const HISTORY_MS: i64 = 30 * 60_000;
const NOT_EXCHANGES: [&str; 6] = ["waiting", "ignored", "skipped", "not_allowed", "stop_ignored", "interrupted"];
/// Only what really ran counts as done (a plan offered, refused or awaiting a yes is in the reply, never in "did").
const DID: [&str; 2] = ["done", "confirmed"];
/// What went wrong lately, for "pourquoi ça n'a pas marché ?": Bobby answers from its own journal, never from a guess.
fn trouble_why(outcome: &str) -> Option<&'static str> {
    Some(match outcome {
        "ignored" => "phrase ignorée : pas de « Bobby » dedans, ou dite trop tard après le bip",
        "error" => "action en échec",
        "resolve_error" => "demande non exécutée",
        "forbidden" => "refusé : droits insuffisants",
        "llm_down" => "le cerveau local ne répondait pas",
        "cancelled" => "annulé",
        _ => return None,
    })
}

/// `new Date(ms).toISOString()`.
pub fn iso_now_ms(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms).unwrap_or_default().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// `Date.parse(s)` for ISO dates (None = NaN).
pub fn parse_iso_ms(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp_millis())
}

/// A number that was an f32 (Python/JS float) as JSON.stringify prints it: shortest decimal, integers without ".0".
pub fn num_f32(x: f32) -> Value {
    if !x.is_finite() {
        return Value::Null; // JSON.stringify(NaN) is null
    }
    let f: f64 = x.to_string().parse().unwrap_or(0.0);
    if f.fract() == 0.0 && f.abs() < 1e15 {
        Value::from(f as i64)
    } else {
        Value::from(f)
    }
}

/// Appends records to the journal file.
pub struct Journal {
    pub path: PathBuf,
}

impl Journal {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// One line per record; errors are ignored like Node's `fs.appendFile(.., () => {})`.
    /// The caller builds `rec` with its keys in the order of `hear()` (JSON.stringify keeps insertion order).
    pub fn append(&self, rec: &Map<String, Value>) {
        let Ok(mut line) = serde_json::to_string(rec) else { return };
        line.push('\n');
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

#[derive(Debug, Clone)]
struct Trouble {
    at: i64,
    who: String,
    query: String,
    why: String,
}

#[derive(Debug, Clone)]
struct Running {
    key: u64,
    who: String,
    query: String,
    at: i64,
}

/// The conversation of everyone who talks to Bobby (one voice channel at a time), so "ça", "le dernier", "refais" or
/// "non pas ça" make sense whoever said what before: last 10 exchanges within 30 minutes, with what Bobby did and
/// answered. Rebuilt from the journal at startup, so a restart loses nothing.
#[derive(Debug, Default)]
pub struct Memory {
    convo: Vec<Hist>,
    trouble: Vec<Trouble>,
    /// Requests still being worked on, by caller key.
    running: Vec<Running>,
}

fn s<'a>(rec: &'a Map<String, Value>, k: &str) -> &'a str {
    rec.get(k).and_then(Value::as_str).unwrap_or("")
}

/// JS truthiness of a record field.
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::String(t)) => !t.is_empty(),
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        _ => true,
    }
}

impl Memory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Last 10 exchanges within 30 minutes.
    pub fn conversation(&self, now_ms: i64) -> Vec<Hist> {
        let live: Vec<Hist> = self.convo.iter().filter(|h| now_ms - h.at < HISTORY_MS).cloned().collect();
        live[live.len().saturating_sub(HISTORY_MAX)..].to_vec()
    }

    /// Lines for "pourquoi ça n'a pas marché ?": recent failures, then requests still running.
    pub fn trouble(&self, now_ms: i64) -> Vec<String> {
        let mut out: Vec<String> = self
            .trouble
            .iter()
            .filter(|x| now_ms - x.at < HISTORY_MS)
            .map(|x| format!("- {} : \"{}\" -> {}", x.who, x.query, x.why))
            .collect();
        out.extend(self.running.iter().map(|r| {
            // JS Math.round: halves go up
            let secs = ((now_ms - r.at) as f64 / 1000.0 + 0.5).floor() as i64;
            format!("- {} : \"{}\" -> toujours en cours depuis {secs} s, pas encore de résultat", r.who, r.query)
        }));
        out
    }

    /// A request starts (Node: `RUNNING.set(rec, Date.now())`).
    pub fn start(&mut self, key: u64, who: &str, query: &str, now_ms: i64) {
        self.running.push(Running { key, who: who.into(), query: query.into(), at: now_ms });
    }

    /// It is over (Node: `RUNNING.delete(rec)`).
    pub fn finish(&mut self, key: u64) {
        self.running.retain(|r| r.key != key);
    }

    /// Notes a journal record: failures go to `trouble`, real exchanges to the conversation.
    pub fn remember(&mut self, rec: &Map<String, Value>, now_ms: i64) {
        let outcome = s(rec, "outcome");
        let at = parse_iso_ms(s(rec, "at")).filter(|&t| t != 0).unwrap_or(now_ms);
        if let Some(why) = trouble_why(outcome) {
            let query = if s(rec, "query").is_empty() { s(rec, "heard") } else { s(rec, "query") };
            let why = if truthy(rec.get("error")) {
                let err = rec.get("error").and_then(Value::as_str).map_or_else(|| rec["error"].to_string(), String::from);
                format!("{why} ({err})")
            } else {
                why.to_string()
            };
            self.trouble.push(Trouble { at, who: s(rec, "who").into(), query: query.into(), why });
            self.trouble.drain(..self.trouble.len().saturating_sub(8));
        }
        let query = s(rec, "query");
        if query.is_empty() || NOT_EXCHANGES.contains(&outcome) {
            return;
        }
        let mut convo = self.conversation(now_ms);
        convo.push(Hist {
            at,
            id: rec.get("id").and_then(Value::as_str).map(String::from),
            who: s(rec, "who").into(),
            query: query.into(),
            did: if DID.contains(&outcome) { s(rec, "plan").into() } else { String::new() },
            confirm: truthy(rec.get("confirm")),
            // ponytail: JS slice(0, 200) counts UTF-16 units, this counts chars (differs only for emoji)
            reply: s(rec, "reply").chars().take(200).collect(),
        });
        convo.drain(..convo.len().saturating_sub(HISTORY_MAX));
        self.convo = convo;
    }

    /// Rebuilds the memory from the journal's last 256 KB (plenty for 10 exchanges among the "skipped" lines of
    /// other speech). Returns the number of exchanges found.
    pub fn reload(&mut self, path: &Path, now_ms: i64) -> usize {
        self.convo.clear();
        self.trouble.clear();
        let _ = self.read_tail(path, now_ms); // no journal yet: nothing to rebuild
        self.convo.len()
    }

    fn read_tail(&mut self, path: &Path, now_ms: i64) -> std::io::Result<()> {
        let mut f = std::fs::File::open(path)?;
        let size = f.metadata()?.len();
        let n = size.min(256 * 1024);
        f.seek(SeekFrom::Start(size - n))?;
        let mut buf = Vec::with_capacity(n as usize);
        f.take(n).read_to_end(&mut buf)?;
        let text = String::from_utf8_lossy(&buf);
        let skip = usize::from(size > n); // the first line is cut
        for line in text.split('\n').skip(skip) {
            if let Ok(Value::Object(rec)) = serde_json::from_str::<Value>(line) {
                self.remember(&rec, now_ms);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::talk;
    use serde_json::json;

    fn map(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn conversation_from_the_journal() {
        // other speakers included, skipped speech and old lines left out
        let now = 1_800_000_000_000_i64;
        let at = |min: i64| iso_now_ms(now - min * 60_000);
        let dir = crate::store::temp_dir("journal");
        let path = dir.join("conversations.jsonl");
        let recs = [
            json!({ "at": at(40), "id": "a", "who": "WakandeX", "query": "trop vieux", "outcome": "done" }),
            json!({ "at": at(5), "id": "a", "who": "WakandeX", "outcome": "skipped", "wakeScore": 0.01 }),
            json!({ "at": at(4), "id": "a", "who": "WakandeX", "query": "explique dans le chat ce que tu sais faire", "plan": "help", "reply": "Je t'envoie ça dans dev-bot-test.", "outcome": "done" }),
            json!({ "at": at(3), "id": "b", "who": "Prime Ilias", "query": "mute WakandeX", "plan": "mute -> WakandeX", "reply": "", "outcome": "done" }),
        ];
        let body: Vec<String> = recs.iter().map(|r| r.to_string()).collect();
        std::fs::write(&path, body.join("\n") + "\n{\"partial").unwrap();
        let mut m = Memory::new();
        assert_eq!(m.reload(&path, now), 2);
        let conv = m.conversation(now);
        assert_eq!(conv.iter().map(|h| h.who.as_str()).collect::<Vec<_>>(), ["WakandeX", "Prime Ilias"]);
        let lines = talk(&conv, now);
        assert!(lines[0].starts_with("- il y a 4 min, WakandeX : \"explique dans le chat ce que tu sais faire\" -> fait : help -> Bobby : \"Je t'envoie ça"));
        assert!(lines[1].ends_with("Prime Ilias : \"mute WakandeX\" -> fait : mute -> WakandeX"));
    }

    #[test]
    fn record_serializes_like_json_stringify() {
        let mut rec = Map::new();
        rec.insert("at".into(), json!("2026-11-16T18:03:48.059Z"));
        rec.insert("who".into(), json!("Kévin \"K\""));
        rec.insert("id".into(), json!("1"));
        rec.insert("lang".into(), json!("fr"));
        rec.insert("wakeScore".into(), num_f32(0.123));
        rec.insert("heard".into(), json!("Bobby, salut"));
        rec.insert("role".into(), json!("super"));
        rec.insert("query".into(), json!("salut"));
        rec.insert("intent".into(), json!({"via":"llm","analyse":{"genre":"ordre","faisable":true},"actions":[{"type":"music_volume","text":"down"}],"say":""}));
        rec.insert("plan".into(), json!("music_volume \"down\""));
        rec.insert("outcome".into(), json!("done"));
        rec.insert("reply".into(), json!("Ça marche 🎵"));
        rec.insert("timing".into(), json!({"stt":634,"ack":null,"llm":733,"total":2161.5}));
        let expected = r#"{"at":"2026-11-16T18:03:48.059Z","who":"Kévin \"K\"","id":"1","lang":"fr","wakeScore":0.123,"heard":"Bobby, salut","role":"super","query":"salut","intent":{"via":"llm","analyse":{"genre":"ordre","faisable":true},"actions":[{"type":"music_volume","text":"down"}],"say":""},"plan":"music_volume \"down\"","outcome":"done","reply":"Ça marche 🎵","timing":{"stt":634,"ack":null,"llm":733,"total":2161.5}}"#;
        assert_eq!(serde_json::to_string(&rec).unwrap(), expected);
        // appended as one line, parent dir created
        let path = crate::store::temp_dir("journal").join("sub").join("j.jsonl");
        let j = Journal::new(&path);
        j.append(&rec);
        j.append(&rec);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), format!("{expected}\n{expected}\n"));
        // numbers like JS
        assert_eq!(num_f32(0.75).to_string(), "0.75");
        assert_eq!(num_f32(1.0).to_string(), "1");
        assert_eq!(num_f32(0.0).to_string(), "0");
        assert_eq!(num_f32(f32::NAN), Value::Null);
    }

    #[test]
    fn iso_dates() {
        assert_eq!(iso_now_ms(1_794_852_228_059), "2026-11-16T18:03:48.059Z");
        assert_eq!(parse_iso_ms("2026-11-16T18:03:48.059Z"), Some(1_794_852_228_059));
        assert_eq!(parse_iso_ms("pas une date"), None);
        assert_eq!(parse_iso_ms(&iso_now_ms(1_234_567_890_123)), Some(1_234_567_890_123));
    }

    #[test]
    fn trouble_lines() {
        let now = 1_800_000_000_000_i64;
        let mut m = Memory::new();
        m.remember(&map(json!({ "at": iso_now_ms(now - 60_000), "who": "Alex", "query": "mute Kévin", "outcome": "llm_down", "error": "connect refused" })), now);
        m.remember(&map(json!({ "at": iso_now_ms(now - 90 * 60_000), "who": "Alex", "query": "vieux", "outcome": "error" })), now);
        m.remember(&map(json!({ "who": "Bob", "heard": "salut tout le monde", "outcome": "ignored" })), now);
        m.start(7, "Alex", "joue Daft Punk", now - 12_400);
        assert_eq!(m.trouble(now), vec![
            "- Alex : \"mute Kévin\" -> le cerveau local ne répondait pas (connect refused)".to_string(),
            "- Bob : \"salut tout le monde\" -> phrase ignorée : pas de « Bobby » dedans, ou dite trop tard après le bip".to_string(),
            "- Alex : \"joue Daft Punk\" -> toujours en cours depuis 12 s, pas encore de résultat".to_string(),
        ]);
        m.finish(7);
        assert_eq!(m.trouble(now).len(), 2);
        // llm_down with a query is also an exchange; the reply is cut to 200 chars
        assert_eq!(m.conversation(now).len(), 1);
        m.remember(&map(json!({ "at": iso_now_ms(now), "who": "A", "query": "q", "reply": "é".repeat(300), "outcome": "done", "plan": "p", "confirm": true })), now);
        let h = m.conversation(now).pop().unwrap();
        assert_eq!((h.reply.chars().count(), h.confirm, h.did.as_str()), (200, true, "p"));
    }
}
