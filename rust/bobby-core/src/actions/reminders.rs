//! Reminders and timers ("rappelle-moi dans 10 minutes de sortir le linge", "mets un minuteur de 5 minutes").
//! Shortcut-only: no line in the LLM prompt or schema (`actions::types()` filters `remind_*`).
//! Document "reminders": [{ id, user, name, kind: "reminder" | "timer", text, due (epoch ms), secs, lang }].
//! Set, list and cancel are Prime actions on the speaker's own reminders; `fire_due` says the due ones aloud.
use super::{Env, UndoOp};
use crate::shortcuts::clean;
use crate::store::Store;
use crate::types::{ActionDef, Module, Role, Step};
use crate::util::{caps, group, hit, re, replace1, BrainError, Res, Say};
use bobby_proto::msg::Voice;
use chrono::{DateTime, Duration, Local, TimeZone, Timelike};
use fancy_regex::Regex;
use serde_json::{json, Value};
use std::sync::{LazyLock, Mutex};

const DOC: &str = "reminders";
/// Max reminders and timers waiting for one person.
/// ponytail: a const, the brain has no settings struct yet; make it a setting the day someone asks for more.
pub const MAX_PER_USER: usize = 10;
/// Farther than this is almost surely a misheard number.
const MAX_SECS: u64 = 30 * 24 * 3600;
/// An overdue reminder (Bobby was off) still fires up to this late, then it is dropped.
pub const MAX_LATE_MS: i64 = 10 * 60 * 1000;
/// Late enough to say so.
const LATE_TAG_MS: i64 = 30 * 1000;

/// Read-modify-write of the document is not atomic in the store: one lock for the ticker and the actions.
static LOCK: Mutex<()> = Mutex::new(());

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

// ---------- Parsing ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    /// Seconds from now.
    In(u64),
    /// A clock time: today, or tomorrow if it has passed.
    At(u32, u32),
}

const NUM: &str = r"\d+(?:[.,]\d+)?|une?|deux|trois|quatre|cinq|six|sept|huit|neuf|dix|quinze|vingt|trente|quarante[- ]cinq|quarante|a|an|one|two|three|four|five|ten|fifteen|twenty|thirty";
const UH: &str = r"h(?:eures?)?|hours?|hrs?";
const UM: &str = r"min(?:ute)?s?|mn";
const US: &str = r"sec(?:onde)?s?|seconds?";
const HALF: &str = r"(?:une? )?demi[- ]heure|(?:a )?half(?: an| a)?[- ]?hour";
const QUARTER: &str = r"(?:un )?quart d'heure|(?:a )?quarter(?: of an| an)? hour";

fn delay_src() -> String {
    format!(r"(?:\d{{1,2}} ?(?:h|heures?) ?\d{{2}}(?: ?min(?:utes?)?)?|{HALF}|{QUARTER}|(?:{NUM}) ?(?:{UH})(?: (?:et|and) (?:demie?|a half|quart))?(?: (?:et |and )?(?:{NUM}) ?(?:{UM}))?|(?:{NUM}) ?(?:{UM})|(?:{NUM}) ?(?:{US}))(?![\p{{L}}\d])")
}

const TIME: &str = r"(?:\d{1,2}(?::\d{2})? ?(?:am|pm)|\d{1,2} ?(?:h|:) ?(?:\d{2})?|midi|minuit)(?![\p{L}\d])";

static WHEN_SRC: LazyLock<String> = LazyLock::new(|| format!(r"((?:dans|in|d'ici) {}|(?:à|a|at|vers) {TIME})", delay_src()));
const TRIG: &str = r"(?:rappelle(?:-moi| moi)|rappelez(?:-moi| moi)|rappeler(?:-moi| moi)?|me rappeler|préviens(?:-moi| moi)|remind me)";
const JOIN: &str = r"(?:(?:de|que|to|that) |d'|qu')?";
const VERB: &str = r"(?:(?:mets|met|lance|démarre|demarre|programme|règle|regle|fais|set|start|launch|create)(?:-moi| moi| me)? )?";

static SET_A: LazyLock<Regex> = LazyLock::new(|| re(&format!(r"^{TRIG} {} ?{JOIN}(.+)$", &*WHEN_SRC)));
static SET_B: LazyLock<Regex> = LazyLock::new(|| re(&format!(r"^{},? {TRIG} {JOIN}(.+)$", &*WHEN_SRC)));
static SET_C: LazyLock<Regex> = LazyLock::new(|| re(&format!(r"^{TRIG} {JOIN}(.+?) {}$", &*WHEN_SRC)));
static TIMER_A: LazyLock<Regex> = LazyLock::new(|| re(&format!(r"^{VERB}(?:un |le |a |the )?(?:minuteur|timer)(?: (?:de|pour|d'|à|for|of)|d')? ?({})$", delay_src())));
static TIMER_B: LazyLock<Regex> = LazyLock::new(|| re(&format!(r"^{VERB}(?:un |a )({}) (?:minuteur|timer)$", delay_src())));
static LIST: LazyLock<Regex> = LazyLock::new(|| re(r"^(?:(?:quels? (?:sont|est)|quels?|c'est quoi|what are|what's|what is)|(?:montre|dis|donne|affiche|lis|liste|show|list|read|tell|give)(?:-moi| moi| me)?) (?:mes|mon|les|my|the|all my)(?: tous)? (?:rappels?|minuteurs?|timers?|reminders?)(?: en cours| actifs?| pending)?$"));
static CANCEL_ALL: LazyLock<Regex> = LazyLock::new(|| re(r"^(?:annule|supprime|efface|enlève|retire|cancel|delete|remove|clear)(?:-moi| moi)? (?:tous|toutes|all)(?: mes| tes| my| les| the)? (rappels|minuteurs|timers|reminders)$"));
static CANCEL_ONE: LazyLock<Regex> = LazyLock::new(|| re(r"^(?:annule|supprime|efface|enlève|retire|arrête|cancel|delete|remove|stop)(?:-moi| moi)? (?:mon|le|ce|ton|my|the|that)(?: dernier| last)? (rappel|minuteur|timer|reminder)$"));
static WS: LazyLock<Regex> = LazyLock::new(|| re(r"\s+"));

fn num_val(s: &str) -> f64 {
    match s {
        "un" | "une" | "a" | "an" | "one" => 1.0,
        "deux" | "two" => 2.0,
        "trois" | "three" => 3.0,
        "quatre" | "four" => 4.0,
        "cinq" | "five" => 5.0,
        "six" => 6.0,
        "sept" => 7.0,
        "huit" => 8.0,
        "neuf" => 9.0,
        "dix" | "ten" => 10.0,
        "quinze" | "fifteen" => 15.0,
        "vingt" | "twenty" => 20.0,
        "trente" | "thirty" => 30.0,
        "quarante" => 40.0,
        s if s.starts_with("quarante") => 45.0,
        s => s.replace(',', ".").parse().unwrap_or(0.0),
    }
}

/// "10 minutes", "une heure et demie", "1h30", "une demi-heure", "5 secondes" -> seconds (None if zero).
pub fn delay_secs(b: &str) -> Option<u64> {
    static HALF_RE: LazyLock<Regex> = LazyLock::new(|| re(&format!("^(?:{HALF})$")));
    static QUARTER_RE: LazyLock<Regex> = LazyLock::new(|| re(&format!("^(?:{QUARTER})$")));
    static HM: LazyLock<Regex> = LazyLock::new(|| re(r"^(\d{1,2}) ?(?:h|heures?) ?(\d{2})"));
    static UNITS: LazyLock<Regex> = LazyLock::new(|| re(&format!(r"({NUM}) ?({UH}|{UM}|{US})(?![\p{{L}}])")));
    static AND_HALF: LazyLock<Regex> = LazyLock::new(|| re(r"(?:et|and) (?:demie?|a half)$"));
    static AND_QUARTER: LazyLock<Regex> = LazyLock::new(|| re(r"et quart$"));
    let secs = if hit(&HALF_RE, b) {
        1800.0
    } else if hit(&QUARTER_RE, b) {
        900.0
    } else if let Some(c) = caps(&HM, b) {
        num_val(group(&c, 1)) * 3600.0 + num_val(group(&c, 2)) * 60.0
    } else {
        let mut t: f64 = UNITS
            .captures_iter(b)
            .flatten()
            .map(|c| {
                let u = group(&c, 2);
                num_val(group(&c, 1)) * if u.starts_with('h') { 3600.0 } else if u.starts_with("mi") || u == "mn" { 60.0 } else { 1.0 }
            })
            .sum();
        if hit(&AND_HALF, b) {
            t += 1800.0;
        } else if hit(&AND_QUARTER, b) {
            t += 900.0;
        }
        t
    };
    (secs >= 1.0).then_some(secs as u64)
}

/// "18h30", "18:30", "18 h", "6 pm", "6:30pm", "midi" -> (hour, minute).
pub fn clock(t: &str) -> Option<(u32, u32)> {
    static AMPM: LazyLock<Regex> = LazyLock::new(|| re(r"^(\d{1,2})(?::(\d{2}))? ?(am|pm)$"));
    static HM24: LazyLock<Regex> = LazyLock::new(|| re(r"^(\d{1,2}) ?(?:h|:) ?(\d{2})?$"));
    let (h, m) = match t {
        "midi" => (12, 0),
        "minuit" => (0, 0),
        _ => {
            if let Some(c) = caps(&AMPM, t) {
                let h: u32 = group(&c, 1).parse().ok()?;
                if !(1..=12).contains(&h) {
                    return None;
                }
                (h % 12 + if group(&c, 3) == "pm" { 12 } else { 0 }, group(&c, 2).parse().unwrap_or(0))
            } else {
                let c = caps(&HM24, t)?;
                (group(&c, 1).parse().ok()?, group(&c, 2).parse().unwrap_or(0))
            }
        }
    };
    (h < 24 && m < 60).then_some((h, m))
}

fn parse_when(w: &str) -> Option<When> {
    let (head, rest) = w.split_once(' ')?;
    match head {
        "dans" | "in" | "d'ici" => delay_secs(rest).map(When::In),
        _ => clock(rest).map(|(h, m)| When::At(h, m)),
    }
}

fn reminder(text: &str, when: When) -> Option<Vec<Value>> {
    let text = replace1(&WS, text.trim(), " ");
    if text.is_empty() {
        return None;
    }
    Some(vec![match when {
        When::In(secs) => json!({ "type": "remind_set", "kind": "reminder", "text": text, "secs": secs }),
        When::At(h, m) => json!({ "type": "remind_set", "kind": "reminder", "text": text, "at": format!("{h:02}:{m:02}") }),
    }])
}

/// The module's shortcuts, tried before every other module's (see `shortcuts::quick`). None = not ours.
fn early(query: &str) -> Option<Vec<Value>> {
    static BOBBY: LazyLock<Regex> = LazyLock::new(|| re(r"^bobby[\s,]+"));
    let q = clean(&replace1(&BOBBY, &query.trim().to_lowercase(), ""));
    if let Some(c) = caps(&SET_A, &q) {
        return reminder(group(&c, 2), parse_when(group(&c, 1))?);
    }
    if let Some(c) = caps(&SET_B, &q) {
        return reminder(group(&c, 2), parse_when(group(&c, 1))?);
    }
    if let Some(c) = caps(&SET_C, &q) {
        return reminder(group(&c, 1), parse_when(group(&c, 2))?);
    }
    for r in [&*TIMER_A, &*TIMER_B] {
        if let Some(c) = caps(r, &q) {
            return Some(vec![json!({ "type": "remind_set", "kind": "timer", "text": "", "secs": delay_secs(group(&c, 1))? })]);
        }
    }
    if hit(&LIST, &q) {
        return Some(vec![json!({ "type": "remind_list" })]);
    }
    if let Some(c) = caps(&CANCEL_ALL, &q) {
        let timers = group(&c, 1).starts_with("minuteur") || group(&c, 1) == "timers";
        return Some(vec![json!({ "type": "remind_cancel", "all": true, "kind": if timers { "timer" } else { "" } })]);
    }
    let c = caps(&CANCEL_ONE, &q)?;
    let timer = matches!(group(&c, 1), "minuteur" | "timer");
    Some(vec![json!({ "type": "remind_cancel", "kind": if timer { "timer" } else { "reminder" } })])
}

// ---------- Texts ----------

fn dur_text(secs: u64, lang: &str) -> String {
    let en = lang == "en";
    let (h, m, s) = (secs / 3600, secs % 3600 / 60, secs % 60);
    let n = |n: u64, fr: &str, en_: &str| format!("{n} {}{}", if en { en_ } else { fr }, if n > 1 { "s" } else { "" });
    match (h, m, s) {
        (0, 0, s) => n(s, "seconde", "second"),
        (0, m, 0) => n(m, "minute", "minute"),
        (h, 0, 0) => n(h, "heure", "hour"),
        (h, m, 0) => format!("{h} h {m:02}"),
        _ => format!("{} {}", n(secs / 60, "minute", "minute"), n(s, "seconde", "second")),
    }
}

fn local(ms: i64) -> DateTime<Local> {
    Local.timestamp_millis_opt(ms).single().unwrap_or_else(Local::now)
}

/// fr "18 h 30" ("18 h" on the hour), en "18:30".
pub fn clock_text(ms: i64, lang: &str) -> String {
    let t = local(ms);
    match (lang == "en", t.minute()) {
        (true, m) => format!("{}:{m:02}", t.hour()),
        (false, 0) => format!("{} h", t.hour()),
        (false, m) => format!("{} h {m:02}", t.hour()),
    }
}

fn plural(n: usize) -> &'static str {
    if n > 1 { "s" } else { "" }
}

fn is_timer(r: &Value) -> bool {
    r["kind"] == "timer"
}

/// What Bobby says when `r` is due. `late`: Bobby was off or busy when it was due.
pub fn fire_text(r: &Value, late: bool) -> String {
    let en = r["lang"] == "en";
    let name = r["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(if en { "Hey" } else { "Hé" });
    let tag = match (late, en) {
        (false, _) => "",
        (true, true) => " (late)",
        (true, false) => " (en retard)",
    };
    let text = r["text"].as_str().unwrap_or("");
    let dur = dur_text(r["secs"].as_u64().unwrap_or(0), if en { "en" } else { "fr" });
    match (is_timer(r), en) {
        (true, true) => format!("{name}, your timer for {dur} is done{tag}."),
        (true, false) => format!("{name}, ton minuteur de {dur} est fini{tag}."),
        (false, true) => format!("{name}, reminder: {text}{tag}."),
        (false, false) => format!("{name}, rappel : {text}{tag}."),
    }
}

// ---------- Store and firing ----------

fn load(store: &Store) -> Vec<Value> {
    match store.load_or(DOC, json!([])) {
        Value::Array(a) => a,
        _ => vec![],
    }
}

fn save(store: &Store, all: Vec<Value>) -> Res<()> {
    store.save(DOC, &Value::Array(all)).map_err(BrainError::failed)
}

fn guard() -> std::sync::MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Says every reminder due at `now` (epoch ms) to its person and removes it. One more than 10 min late is dropped and
/// logged; one more than 30 s late (Bobby was off) is said with "(en retard)". Returns how many were said.
/// A restart loses nothing: this reads the store, and the store is all the state there is.
pub fn fire_due(store: &Store, io: &crate::io::Io, now: i64) -> usize {
    let _g = guard();
    let (due, keep): (Vec<Value>, Vec<Value>) = load(store).into_iter().partition(|r| r["due"].as_i64().is_some_and(|d| d <= now));
    if due.is_empty() {
        return 0;
    }
    let mut said = 0;
    for r in &due {
        let late = now - r["due"].as_i64().unwrap_or(now);
        if late > MAX_LATE_MS {
            eprintln!("[reminders] dropped, {} min late: {r}", late / 60_000);
            continue;
        }
        io.say(&fire_text(r, late > LATE_TAG_MS), r["lang"].as_str().unwrap_or("fr"), Voice::Bobby, r["user"].as_str());
        said += 1;
    }
    if let Err(e) = save(store, keep) {
        eprintln!("[reminders] cannot save: {e}"); // they would fire again next second: better than losing them
    }
    said
}

/// Starts the ticker: `fire_due` every second, the first time at once (the catch-up after a restart).
/// ponytail: one ticker reading one small document each second beats one task per reminder (those must be rebuilt
/// at startup anyway). Limits: a reminder fires up to 1 s late, and the store is read every second, forever.
pub fn spawn_ticker<L: Send + Sync + 'static>(brain: std::sync::Arc<crate::brain::Brain<L>>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut t = tokio::time::interval(std::time::Duration::from_secs(1));
        loop {
            t.tick().await;
            fire_due(&brain.store, &brain.io, now_ms());
        }
    })
}

/// Today at h:m, tomorrow if that is not in the future.
pub fn next_at(now: DateTime<Local>, h: u32, m: u32) -> Option<DateTime<Local>> {
    let d = now.date_naive().and_hms_opt(h, m, 0)?.and_local_timezone(Local).earliest()?;
    Some(if d <= now { d + Duration::days(1) } else { d })
}

// ---------- Actions ----------

fn set<L>(s: &mut Step, env: &Env<'_, L>, lang: &str) -> Res<()> {
    let _g = guard();
    let mut all = load(env.store);
    if all.iter().filter(|r| r["user"] == s.by.id.as_str()).count() >= MAX_PER_USER {
        return Err(Say::new(format!("Tu as déjà {MAX_PER_USER} rappels, annule-en un d'abord."), format!("You already have {MAX_PER_USER} reminders, cancel one first.")).into());
    }
    let en = lang == "en";
    let now = Local::now();
    let (due, line) = if let Some(at) = s.str("at") {
        let (h, m) = clock(at).unwrap_or((0, 0));
        let d = next_at(now, h, m).ok_or_else(|| Say::new("Cette heure n'existe pas.", "That time doesn't exist."))?;
        let ms = d.timestamp_millis();
        let tomorrow = d.date_naive() != now.date_naive();
        let when = match (en, tomorrow) {
            (true, false) => format!("at {}", clock_text(ms, lang)),
            (true, true) => format!("tomorrow at {}", clock_text(ms, lang)),
            (false, false) => format!("à {}", clock_text(ms, lang)),
            (false, true) => format!("demain à {}", clock_text(ms, lang)),
        };
        (ms, when)
    } else {
        let secs = s.f.get("secs").and_then(Value::as_u64).unwrap_or(0);
        if !(1..=MAX_SECS).contains(&secs) {
            return Err(Say::new("C'est trop loin, 30 jours maximum.", "That's too far, 30 days at most.").into());
        }
        let d = dur_text(secs, lang);
        (now.timestamp_millis() + secs as i64 * 1000, if en { format!("in {d}") } else { format!("dans {d}") })
    };
    let timer = s.str("kind") == Some("timer");
    let secs = ((due - now.timestamp_millis()) / 1000).max(0);
    let id = all.iter().map(|r| r["id"].as_i64().unwrap_or(0)).max().unwrap_or(0) + 1;
    all.push(json!({ "id": id, "user": s.by.id, "name": s.by.name, "kind": if timer { "timer" } else { "reminder" }, "text": s.text, "due": due, "secs": secs, "lang": lang }));
    save(env.store, all)?;
    let said = match (timer, en) {
        (true, false) => format!("Minuteur de {} lancé.", dur_text(secs as u64, lang)),
        (true, true) => format!("Timer set for {}.", dur_text(secs as u64, lang)),
        (false, false) => format!("C'est noté, je te le rappelle {line}."),
        (false, true) => format!("Got it, I'll remind you {line}."),
    };
    s.set("line", said);
    Ok(())
}

fn list<L>(s: &mut Step, env: &Env<'_, L>, lang: &str) -> Res<()> {
    let _g = guard();
    let mine: Vec<Value> = load(env.store).into_iter().filter(|r| r["user"] == s.by.id.as_str()).collect();
    let en = lang == "en";
    let items = mine
        .iter()
        .map(|r| {
            let at = clock_text(r["due"].as_i64().unwrap_or(0), lang);
            let what = if is_timer(r) { (if en { "timer" } else { "minuteur" }).to_string() } else { r["text"].as_str().unwrap_or("").to_string() };
            format!("{what} {} {at}", if en { "at" } else { "à" })
        })
        .collect::<Vec<_>>()
        .join(", ");
    let n = mine.len();
    let said = match (n, en) {
        (0, false) => "Tu n'as aucun rappel.".to_string(),
        (0, true) => "You have no reminders.".to_string(),
        (_, false) => format!("Tu as {n} rappel{} : {items}.", plural(n)),
        (_, true) => format!("You have {n} reminder{}: {items}.", plural(n)),
    };
    s.set("line", said);
    Ok(())
}

fn cancel<L>(s: &mut Step, env: &Env<'_, L>, lang: &str) -> Res<()> {
    let _g = guard();
    let all = load(env.store);
    let kind = s.str("kind").unwrap_or("").to_string();
    let mine = |r: &&Value| r["user"] == s.by.id.as_str() && (kind.is_empty() || r["kind"] == kind.as_str());
    let mut ids: Vec<i64> = all.iter().filter(mine).filter_map(|r| r["id"].as_i64()).collect();
    if !s.flag("all") {
        ids = ids.into_iter().max().into_iter().collect(); // the latest one
    }
    let en = lang == "en";
    let n = ids.len();
    let what = match (kind.as_str(), en) {
        ("timer", false) => "minuteur",
        ("timer", true) => "timer",
        (_, false) => "rappel",
        (_, true) => "reminder",
    };
    let said = match (n, en) {
        (0, false) => format!("Tu n'as aucun {what}."),
        (0, true) => format!("You have no {what}."),
        (1, false) => format!("J'ai annulé ton {what}."),
        (1, true) => format!("I cancelled your {what}."),
        (_, false) => format!("J'ai annulé tes {n} {what}s."),
        (_, true) => format!("I cancelled your {n} {what}s."),
    };
    if n > 0 {
        save(env.store, all.into_iter().filter(|r| !r["id"].as_i64().is_some_and(|i| ids.contains(&i))).collect())?;
    }
    s.set("line", said);
    Ok(())
}

pub async fn run<L: Sync>(s: &mut Step, env: &Env<'_, L>, lang: &str) -> Res<Vec<UndoOp>> {
    match s.kind.as_str() {
        "remind_set" => set(s, env, lang)?,
        "remind_list" => list(s, env, lang)?,
        "remind_cancel" => cancel(s, env, lang)?,
        _ => {}
    }
    Ok(vec![]) // no undo: "annule mon rappel" is the way back
}

fn say_line(s: &Step, _: Option<&str>, _: &str) -> Option<String> {
    s.str("line").map(String::from)
}

pub fn module() -> Module {
    let prime = ActionDef { role: Role::Prime, say: Some(say_line), ..Default::default() };
    Module {
        help: ["programmer des rappels et des minuteurs", "set reminders and timers"],
        short: ["les rappels et minuteurs", "reminders and timers"],
        early: Some(early),
        actions: vec![("remind_set", ActionDef { saves: true, ..prime.clone() }), ("remind_list", prime.clone()), ("remind_cancel", prime)],
        ..Module::empty("reminders")
    }
}
