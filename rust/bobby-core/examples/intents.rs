//! Bobby's intent test runner for the Rust brain: port of the runner at the bottom of `bot/intent.test.js`,
//! against the real LLM (llama-server). The cases come from `tests/fixtures/intents.json`
//! (regenerate with `node tests/fixtures/extract-intents.js`).
//! Run: `cargo run --example intents -p bobby-core [-- --llm http://127.0.0.1:8080]`
use bobby_core::brain::Brain;
use bobby_core::guard::understand;
use bobby_core::io::Io;
use bobby_core::prompt::unsure;
use bobby_core::resolve::resolve;
use bobby_core::store::Store;
use bobby_core::types::{Chan, Ctx, Person, Role, Step};
use bobby_core::util::{hit, re, BrainError, Hist, Res};
use bobby_infer::HttpLlm;
use serde_json::{json, Map, Value};
use std::sync::Arc;
use std::time::Instant;

/// Port of `bot/test-context.js` (`testing::ctx()` is `cfg(test)`, so it is duplicated here), role super.
fn ctx() -> Ctx {
    let m = |name: &str, channel: &str, id: &str, nicknames: &[&str]| {
        let n: Vec<String> = nicknames.iter().map(|s| s.to_string()).collect();
        Person { id: id.into(), name: name.into(), channel: Some(channel.into()), nicknames: n.clone(), aliases: n, roles: vec![] }
    };
    let chans = |names: &[&str]| names.iter().map(|n| Chan { id: n.to_string(), name: n.to_string() }).collect::<Vec<_>>();
    let members = vec![
        m("Alex", "Général", "Alex", &[]),
        m("Zyklon", "Général", "Zyklon", &["le Grec"]),
        m("Kévin", "Général", "Kévin", &[]),
        m("Marie", "Gaming", "Marie", &[]),
        m("Lucas", "Gaming", "lucas", &[]),
        m("Prime Ilias", "Gaming", "397478613559148545", &[]),
        m("Sorciax", "Gaming", "Sorciax", &["Vinice Lapute"]),
        m("WakandeX", "Gaming", "261531632589078528", &[]),
    ];
    Ctx {
        speaker: members[0].clone(),
        members,
        voice_channels: chans(&["Général", "Gaming", "AFK", "Réunion"]),
        text_channels: chans(&["général", "annonces", "memes", "dev-bot-test"]),
        text_channel: None,
        role: Role::Super,
        history: vec![],
        trouble: vec![],
    }
}

/// What a case produced: an array (no action) or a string (the plan as `fmt`, or "ERR").
#[derive(Clone)]
enum Got {
    NoAction,
    Text(String),
}

impl Got {
    fn json(&self) -> Value {
        match self {
            Got::NoAction => json!([]),
            Got::Text(s) => json!(s),
        }
    }
    fn text(&self) -> &str {
        match self {
            Got::NoAction => "",
            Got::Text(s) => s,
        }
    }
}

/// Adjacent steps of the same type and channel count as one ("mute Zyklon" + "mute Kévin" = "mute Zyklon and Kévin").
fn merge(plan: Vec<Step>) -> Vec<Step> {
    let mut out: Vec<Step> = Vec::new();
    for s in plan {
        if let Some(last) = out.last_mut() {
            if last.kind == s.kind && last.channel == s.channel {
                if let (Some(a), Some(b)) = (last.targets.as_mut(), s.targets.as_ref()) {
                    a.extend(b.iter().cloned());
                    continue;
                }
            }
        }
        out.push(s);
    }
    out
}

fn fmt(plan: Vec<Step>) -> String {
    let parts: Vec<String> = merge(plan)
        .iter()
        .map(|s| {
            let mut names: Vec<&str> = s.targets.iter().flatten().map(|t| t.name.as_str()).collect();
            names.sort();
            let ch = s.channel.as_ref().map(|c| format!(">{}", c.name)).unwrap_or_default();
            format!("{}:{}{ch}", s.kind, names.join(","))
        })
        .collect();
    parts.join(" | ")
}

/// `out.actions.length ? fmt(resolve(out.actions, ctx)) : none`; a resolve error is "ERR" like a JS throw.
fn plan_of(out: &Value, ctx: &Ctx, store: &Store, none: Got) -> Got {
    match out["actions"].as_array().filter(|a| !a.is_empty()) {
        Some(acts) => resolve(acts, ctx, store).map_or(Got::Text("ERR".into()), |p| Got::Text(fmt(p))),
        None => none,
    }
}

/// The history of the JSON file: `ago_ms` becomes an absolute time on the runner's clock.
fn history(v: &Value, now_ms: i64) -> Vec<Hist> {
    let s = |h: &Value, k: &str| h[k].as_str().unwrap_or("").to_string();
    v.as_array()
        .into_iter()
        .flatten()
        .map(|h| Hist {
            at: now_ms - h["ago_ms"].as_i64().unwrap_or(0),
            id: h["id"].as_str().map(String::from),
            who: s(h, "who"),
            query: s(h, "query"),
            did: s(h, "did"),
            confirm: false,
            reply: s(h, "reply"),
        })
        .collect()
}

/// The BATCH4 checks (declarative form of the JS predicates, see extract-intents.js). All keys must hold.
fn check(c: &Value, out: &Value, got: &str) -> bool {
    let truthy = |v: &Value| match v {
        Value::Null | Value::Bool(false) => false,
        Value::String(s) => !s.is_empty(),
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        _ => true,
    };
    c.as_object().into_iter().flatten().all(|(k, want)| match k.as_str() {
        "decision" => out["decision"] == *want,
        "decision_not" => out["decision"] != *want,
        "got" => want.as_str() == Some(got),
        "got_re" => hit(&re(want.as_str().unwrap_or("")), got),
        "written" => truthy(&out["written"]) == want.as_bool().unwrap_or(true),
        "filter" => out["actions"][0]["filter"] == *want,
        other => panic!("unknown check key {other}"),
    })
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let url = args.iter().position(|a| a == "--llm").and_then(|i| args.get(i + 1)).cloned().unwrap_or_else(|| "http://127.0.0.1:8080".into());
    // the PC app names the client would report (one per line), e.g. dumped from Node's pc.js index
    if let Some(f) = args.iter().position(|a| a == "--apps").and_then(|i| args.get(i + 1)) {
        let text = std::fs::read_to_string(f).expect("--apps file");
        bobby_core::actions::pc::set_apps(text.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect());
    }
    let fixtures: Value = serde_json::from_str(include_str!("../tests/fixtures/intents.json")).expect("intents.json");

    // A fixed to-do list, so the list lines in the prompt are the same on every run (never the real data/).
    let dir = std::env::temp_dir().join(format!("bobby-intent-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("todo.json"), fixtures["todo"].to_string()).expect("todo.json");
    let store = Store::open(&dir.join("bobby.db"), &dir).expect("store");
    let (io, _rx) = Io::new();
    let brain = Brain::new(store, HttpLlm::new(&url), Arc::new(io), dir.join("journal.jsonl"));
    let ctx = ctx();
    let rows = |k: &str| fixtures[k].as_array().cloned().unwrap_or_default();
    let s = |v: &Value| v.as_str().unwrap_or("").to_string();

    // Comprehension: `understand` alone.
    let (cases, mut ok, mut unsure_n) = (rows("cases"), 0, 0);
    let mut times: Vec<f64> = Vec::new();
    for c in &cases {
        let (phrase, want) = (s(&c[0]), &c[1]);
        let t0 = Instant::now();
        let got = match understand(&brain.llm, &ctx, &phrase, "fr", &[], &brain.store, chrono::Local::now()).await {
            Ok(out) => {
                if unsure(&out) {
                    unsure_n += 1;
                    println!("unsure (Claude would rethink): \"{phrase}\"");
                }
                plan_of(&out, &ctx, &brain.store, Got::NoAction)
            }
            Err(BrainError::Say(say)) if say.code == Some("LLM_DOWN") => {
                eprintln!("llama-server is not answering on {url}");
                let _ = std::fs::remove_dir_all(&dir);
                std::process::exit(1);
            }
            Err(_) => Got::Text("ERR".into()),
        };
        times.push(t0.elapsed().as_secs_f64() * 1000.0);
        // 'ERR': no action runs; since the analysis step the model may also say it itself ("je ne trouve pas Bernard")
        let pass = match want {
            Value::Array(_) => matches!(got, Got::NoAction),
            _ => got.json() == *want || (want == "ERR" && matches!(got, Got::NoAction)),
        };
        if pass {
            ok += 1;
        } else {
            println!("FAIL  \"{phrase}\"\n   expected {want}\n   got      {}", got.json());
        }
    }
    times.sort_by(|a, b| a.total_cmp(b));
    let med = times[times.len() >> 1].round();
    let max = times.last().copied().unwrap_or(0.0).round();
    println!("\nComprehension: {ok}/{}  ·  LLM latency median {med} ms, max {max} ms  ·  unsure (Claude fallback) {unsure_n}", cases.len());

    // Routing and history: run as Bobby does (shortcut first). "!ask" passes for any local answer, not for an error.
    let (routing, mut rok) = (rows("routing"), 0);
    let ask = re(r"(?:^| )ask:");
    for r in &routing {
        let (phrase, want) = (s(&r[0]), s(&r[2]));
        let h = history(&r[1], now_ms());
        let got = match brain.intent(&phrase, &ctx, "fr", &h).await {
            Ok(out) => plan_of(&out, &ctx, &brain.store, Got::Text("(answered)".into())),
            Err(_) => Got::Text("ERR".into()),
        };
        let g = got.text();
        let pass = if want == "!ask" { g != "ERR" && !hit(&ask, g) } else { g == want };
        if pass {
            rok += 1;
        } else {
            println!("FAIL  routing \"{phrase}\"\n   expected {want}\n   got      {g}");
        }
    }
    // The first entries are ROUTING (no history), the last ones WITH_HISTORY.
    let with_history = routing.iter().filter(|r| r[1].as_array().is_some_and(|h| !h.is_empty())).count();
    let plain = routing.len() - with_history;
    let local = routing.iter().take(plain).filter(|r| r[2] != "ask:").count();
    println!("Routing: {rok}/{} ({local} must stay local, {} must go to Claude, {with_history} with history)", routing.len(), plain - local);

    // Batch 4: the reasoning step and the shared conversation, as hear() decides.
    let (batch4, mut bok) = (rows("batch4"), 0);
    for b in &batch4 {
        let phrase = s(&b[0]);
        let h = history(&b[1], now_ms());
        let res: Res<Value> = brain.intent(&phrase, &ctx, "fr", &h).await;
        let (out, got) = match &res {
            Ok(o) => (Some(o), plan_of(o, &ctx, &brain.store, Got::Text("(answered)".into())).text().to_string()),
            Err(e) => (None, format!("ERR {e}")),
        };
        if out.is_some_and(|o| check(&b[2], o, &got)) {
            bok += 1;
        } else {
            let mut seen = Map::new();
            for k in ["analyse", "decision", "written", "say"] {
                if let Some(v) = out.map(|o| &o[k]).filter(|v| !v.is_null()) {
                    seen.insert(k.into(), v.clone());
                }
            }
            println!("FAIL  batch 4 \"{phrase}\"\n   got {got} · {}", Value::Object(seen));
        }
    }
    println!("Batch 4 (reasoning, conversation): {bok}/{}", batch4.len());
    let rate = |n: usize, d: usize| n as f64 / d as f64 >= 0.9;
    let _ = std::fs::remove_dir_all(&dir);
    std::process::exit(if rate(ok, cases.len()) && rate(rok, routing.len()) && rate(bok, batch4.len()) { 0 } else { 1 });
}
