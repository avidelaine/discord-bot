//! Replays the real journal through the Rust brain's `intent()`, the twin of tests/fixtures/replay-node.js.
//!   cargo run -p bobby-core --example replay -- [--llm http://127.0.0.1:8080] [--apps apps.txt] [journal.jsonl] > rust.jsonl
//!   cargo run -p bobby-core --example replay -- --compare node.jsonl rust.jsonl
//! One JSON line per journal record that has a `query` and no `confirm`: {i, at, query, via, plan, decision, written}.
//! Never touches data/ or _logs/ (temporary store, empty temporary journal); the journal is only read.
use bobby_core::brain::Brain;
use bobby_core::io::Io;
use bobby_core::journal::{parse_iso_ms, Memory};
use bobby_core::store::{temp_dir, Store};
use bobby_core::types::{Chan, Ctx, Person, Role, Step};
use bobby_infer::HttpLlm;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const SUPER: &str = "261531632589078528";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--compare") {
        match args.as_slice() {
            [_, node, rust] => compare(node, rust),
            _ => die("usage: replay --compare node.jsonl rust.jsonl"),
        }
        return;
    }
    let mut llm = "http://127.0.0.1:8080".to_string();
    let mut journal: Option<PathBuf> = None;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if a == "--llm" {
            llm = it.next().unwrap_or_else(|| die("--llm needs an address"));
        } else if a == "--apps" {
            // the PC app names the client would report (one per line), e.g. dumped from Node's pc.js index
            let f = it.next().unwrap_or_else(|| die("--apps needs a file"));
            let text = std::fs::read_to_string(&f).unwrap_or_else(|e| die(&format!("{f}: {e}")));
            bobby_core::actions::pc::set_apps(text.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect());
        } else {
            journal = Some(PathBuf::from(a));
        }
    }
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(replay(&llm, journal));
}

fn die(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(2);
}

/// The journal is not in git: a worktree (.claude/worktrees/<name>) reads the main checkout's.
fn default_journal() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    [root.join("_logs"), root.join("..").join("..").join("..").join("_logs")]
        .into_iter()
        .map(|d| d.join("conversations.jsonl"))
        .find(|p| p.exists())
        .unwrap_or_else(|| die("journal not found: pass its path"))
}

fn names(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect()
}

fn chans(v: &Value) -> Vec<Chan> {
    names(v).into_iter().map(|n| Chan { id: n.clone(), name: n }).collect()
}

/// Node's `merge` + `fmt` (bot/intent.test.js): adjacent steps of the same type and channel count as one.
fn fmt(plan: &[Step]) -> String {
    // (type, targets, channel, has targets)
    let mut out: Vec<(String, Vec<String>, Option<String>, bool)> = vec![];
    for s in plan {
        let ts: Vec<String> = s.targets.iter().flatten().map(|t| t.name.clone()).collect();
        let ch = s.channel.as_ref().map(|c| c.name.clone());
        match out.last_mut() {
            Some(l) if l.0 == s.kind && l.2 == ch && l.3 && s.targets.is_some() => l.1.extend(ts),
            _ => out.push((s.kind.clone(), ts, ch, s.targets.is_some())),
        }
    }
    let parts: Vec<String> = out
        .into_iter()
        .map(|(kind, mut ts, ch, _)| {
            ts.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16())); // JS default sort: UTF-16 code units
            format!("{kind}:{}{}", ts.join(","), ch.map(|c| format!(">{c}")).unwrap_or_default())
        })
        .collect();
    parts.join(" | ")
}

async fn replay(llm: &str, journal: Option<PathBuf>) {
    let path = journal.unwrap_or_else(default_journal);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| die(&format!("{}: {e}", path.display())));
    let snap: Value = serde_json::from_str(include_str!("../tests/fixtures/replay-ctx.json")).unwrap();

    // The same to-do list as bot/intent.test.js, so the list lines in the prompt are identical on both sides.
    let store = Store::temp();
    let author = json!({ "id": SUPER, "name": "WakandeX" });
    store
        .save(
            "todo",
            &json!([
                { "id": 1, "text": "Mettre des fondus dans la musique", "list": "general", "author": author, "at": "2026-11-12T22:15:08.000Z", "done": false },
                { "id": 2, "text": "Une voix différente pour Claude", "list": "general", "author": author, "at": "2026-11-12T22:24:03.000Z", "done": false },
            ]),
        )
        .unwrap();
    let tmp = temp_dir("replay-journal").join("conversations.jsonl"); // never written to
    let (io, _rx) = Io::new();
    let brain = Brain::new(store, HttpLlm::new(llm), Arc::new(io), tmp);

    let members: Vec<Person> = snap["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| Person { id: m["id"].as_str().unwrap().into(), name: m["name"].as_str().unwrap().into(), channel: m["channel"].as_str().map(String::from), nicknames: vec![], aliases: vec![], roles: vec![] })
        .collect();
    let voice = chans(&snap["voiceChannels"]);
    let text_ch = chans(&snap["textChannels"]);

    let mut memory = Memory::new();
    let mut i = 0;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let rec: Map<String, Value> = serde_json::from_str(line).unwrap_or_else(|e| die(&format!("bad journal line: {e}")));
        let at = rec.get("at").and_then(Value::as_str).and_then(parse_iso_ms).filter(|&t| t != 0).unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
        let query = rec.get("query").and_then(Value::as_str).unwrap_or("");
        let confirm = rec.get("confirm").is_some_and(|c| !matches!(c, Value::Null | Value::Bool(false)));
        if !query.is_empty() && !confirm {
            let id = rec.get("id").and_then(Value::as_str).unwrap_or("");
            let who = rec.get("who").and_then(Value::as_str).unwrap_or("");
            let sp = &snap["speakers"][id];
            let speaker = members.iter().find(|m| m.id == id).cloned().unwrap_or(Person { id: id.into(), name: if who.is_empty() { sp["name"].as_str().unwrap_or(id).into() } else { who.into() }, ..Default::default() });
            let history = memory.conversation(at);
            let ctx = Ctx {
                speaker,
                members: members.clone(),
                voice_channels: voice.clone(),
                text_channels: text_ch.clone(),
                text_channel: None,
                role: Role::parse(sp["role"].as_str().unwrap_or("admin")).unwrap_or(Role::Admin),
                history: history.clone(),
                trouble: memory.trouble(at),
            };
            let lang = rec.get("lang").and_then(Value::as_str).unwrap_or("fr");
            let line = match brain.intent(query, &ctx, lang, &history).await {
                Ok(out) => {
                    let acts = out["actions"].as_array().cloned().unwrap_or_default();
                    let plan = if acts.is_empty() {
                        "(answered)".to_string()
                    } else {
                        match bobby_core::resolve::resolve(&acts, &ctx, &brain.store) {
                            Ok(p) => fmt(&p),
                            Err(e) => format!("ERR {}", e.fr),
                        }
                    };
                    json!({ "via": out["via"], "plan": plan, "decision": out["decision"], "written": out["written"].as_bool().unwrap_or(false) })
                }
                Err(e) => json!({ "via": "", "plan": format!("ERR {e}"), "decision": "", "written": false }),
            };
            let mut o = Map::new();
            o.insert("i".into(), json!(i));
            o.insert("at".into(), rec.get("at").cloned().unwrap_or(Value::Null));
            o.insert("query".into(), json!(query));
            o.extend(line.as_object().unwrap().clone());
            println!("{}", Value::Object(o));
            i += 1;
        }
        memory.remember(&rec, at);
    }
}

fn read(path: &str) -> Vec<Value> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| die(&format!("{path}: {e}")));
    // the brain also logs lines of its own to stdout: only the JSON objects count
    text.lines().filter(|l| l.starts_with('{')).filter_map(|l| serde_json::from_str(l).ok()).collect()
}

fn compare(node: &str, rust: &str) {
    let (n, r) = (read(node), read(rust));
    let key = |v: &Value| format!("{}\u{0}{}\u{0}{}\u{0}{}", v["plan"], v["via"], v["decision"], v["written"]);
    let mut same = 0;
    for (a, b) in n.iter().zip(&r) {
        if key(a) == key(b) {
            same += 1;
        } else {
            let show = |v: &Value| format!("{} [{} / {} / written={}]", v["plan"].as_str().unwrap_or(""), v["via"].as_str().unwrap_or(""), v["decision"].as_str().unwrap_or(""), v["written"]);
            println!("{} | {} | {} | {}", a["i"], a["query"].as_str().unwrap_or(""), show(a), show(b));
        }
    }
    if n.len() != r.len() {
        println!("lengths differ: node {} lines, rust {} lines (only the first {} compared)", n.len(), r.len(), n.len().min(r.len()));
    }
    let total = n.len().max(r.len()).max(1);
    println!("identical: {same}/{total} ({:.1} %)", 100.0 * same as f64 / total as f64);
}
