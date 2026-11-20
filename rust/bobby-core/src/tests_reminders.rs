//! Reminders and timers: parsing, set, fire (with an injected "now"), list, cancel, persistence, rights.
//! Driven through a real `Brain` over a fake client; no Discord, mic or GPU.
use crate::actions::reminders::{self as rem, clock, clock_text, delay_secs, fire_due, next_at, MAX_LATE_MS, MAX_PER_USER};
use crate::actions::{self, info};
use crate::brain::{Brain, Heard};
use crate::resolve::{needed, resolve};
use crate::shortcuts::quick;
use crate::store::{temp_dir, Store};
use crate::testing::{fake_client, FakeLlm, Sent};
use crate::types::Role;
use bobby_proto::msg::{BrainMsg, Channel, Context, Member, Voice};
use chrono::{Local, TimeZone};
use serde_json::{json, Value};

const MARIE: (&str, &str, &str) = ("42", "Marie", "Team Prime");
const NOBODY: (&str, &str, &str) = ("x", "Inconnu", "");

fn context(speaker: &str) -> Context {
    let ch = |n: &str| Channel { id: n.into(), name: n.into() };
    let member = |(id, name, role): (&str, &str, &str)| Member {
        id: id.into(),
        name: name.into(),
        username: name.into(),
        global_name: None,
        channel: None,
        roles: if role.is_empty() { vec![] } else { vec![role.into()] },
        bot: false,
    };
    Context { speaker: speaker.into(), members: vec![member(MARIE), member(NOBODY)], voice_channels: vec![ch("Général")], text_channels: vec![ch("dev-bot-test")], bobby_channel: Some("dev-bot-test".into()) }
}

struct Rig {
    brain: Brain<FakeLlm>,
    sent: Sent,
    seen: usize,
}

impl Rig {
    fn new() -> Self {
        let (io, sent) = fake_client(|_| Ok(Value::Null));
        let path = temp_dir("reminders").join("conversations.jsonl");
        Rig { brain: Brain::new(Store::temp(), FakeLlm::default(), io, &path), sent, seen: 0 }
    }

    /// What the brain sent since the last call: (text, lang, to_user) of every `Say`.
    async fn flush(&mut self) -> Vec<(String, String, Option<String>)> {
        tokio::time::sleep(std::time::Duration::from_millis(30)).await; // the fake client records on its own task
        let msgs: Vec<BrainMsg> = self.sent.lock().unwrap()[self.seen..].to_vec();
        self.seen += msgs.len();
        msgs.into_iter().filter_map(|m| if let BrainMsg::Say { text, lang, to_user, .. } = m { Some((text, lang, to_user)) } else { None }).collect()
    }

    async fn hear(&mut self, who: (&str, &str, &str), text: &str, lang: &str) -> Vec<String> {
        let h = Heard { text: text.into(), lang: lang.into(), score: Some(0.9), user: who.0.into(), user_roles: vec![who.2.into()], user_name: who.1.into(), ..Default::default() };
        self.brain.hear(h, &context(who.0)).await;
        self.flush().await.into_iter().map(|s| s.0).collect()
    }

    fn docs(&self) -> Vec<Value> {
        self.brain.store.load_or("reminders", json!([])).as_array().unwrap().clone()
    }

    async fn fire(&mut self, now: i64) -> Vec<(String, String, Option<String>)> {
        fire_due(&self.brain.store, &self.brain.io, now);
        self.flush().await
    }
}

fn due_of(r: &Value) -> i64 {
    r["due"].as_i64().unwrap()
}

// ---------- Parsing ----------

fn parsed(q: &str) -> Value {
    let a = quick(q).unwrap_or_else(|| panic!("no shortcut: {q}"));
    assert_eq!(a.len(), 1, "{q}");
    a[0].clone()
}

#[test]
fn parses_french_and_english() {
    // (phrase, kind, text, secs, at)
    let cases = [
        ("rappelle-moi dans 10 minutes de sortir le linge", "reminder", "sortir le linge", Some(600), None),
        ("Bobby, rappelle-moi dans 10 minutes de sortir le linge.", "reminder", "sortir le linge", Some(600), None),
        ("rappelle-moi à 18h30 d'appeler maman", "reminder", "appeler maman", None, Some("18:30")),
        ("rappelle-moi à 18:30 d'appeler maman", "reminder", "appeler maman", None, Some("18:30")),
        ("rappelle-moi à 9 h de prendre mon médicament", "reminder", "prendre mon médicament", None, Some("09:00")),
        ("dans une heure rappelle-moi que le four est chaud", "reminder", "le four est chaud", Some(3600), None),
        ("rappelle-moi de sortir le linge dans 10 minutes", "reminder", "sortir le linge", Some(600), None),
        ("rappelle-moi dans une demi-heure d'appeler Paul", "reminder", "appeler paul", Some(1800), None),
        ("rappelle-moi dans 1h30 de relire le mail", "reminder", "relire le mail", Some(5400), None),
        ("rappelle-moi dans une heure et demie de relire le mail", "reminder", "relire le mail", Some(5400), None),
        ("rappelle-moi dans deux heures de faire les courses", "reminder", "faire les courses", Some(7200), None),
        ("tu peux me rappeler dans 5 minutes d'éteindre le gaz s'il te plaît", "reminder", "éteindre le gaz", Some(300), None),
        ("mets un minuteur de 5 minutes", "timer", "", Some(300), None),
        ("minuteur 3 minutes", "timer", "", Some(180), None),
        ("Bobby, lance un minuteur de 45 secondes", "timer", "", Some(45), None),
        ("remind me in 10 minutes to take out the laundry", "reminder", "take out the laundry", Some(600), None),
        ("remind me at 6:30 pm to call mom", "reminder", "call mom", None, Some("18:30")),
        ("remind me to call mom in an hour", "reminder", "call mom", Some(3600), None),
        ("set a timer for 5 minutes", "timer", "", Some(300), None),
        ("start a 5 minute timer", "timer", "", Some(300), None),
    ];
    for (phrase, kind, text, secs, at) in cases {
        let a = parsed(phrase);
        assert_eq!(a["type"], "remind_set", "{phrase}");
        assert_eq!(a["kind"], kind, "{phrase}");
        assert_eq!(a["text"], text, "{phrase}");
        assert_eq!(a["secs"].as_u64(), secs, "{phrase}");
        assert_eq!(a["at"].as_str(), at, "{phrase}");
    }
    assert_eq!(delay_secs("1 h 30"), Some(5400));
    assert_eq!(clock("6:30pm"), Some((18, 30)));
    assert_eq!(clock("12 am"), Some((0, 0)));
    assert_eq!(clock("midi"), Some((12, 0)));
}

#[test]
fn parsing_negatives() {
    for phrase in [
        "rappelle-moi",
        "rappelle-moi de sortir le linge",           // no time: left to the LLM
        "rappelle-moi dans 10 minutes",              // nothing to remind
        "rappelle-moi dans quelques minutes de partir",
        "rappelle-moi à 25h30 d'appeler maman",      // not a time
        "rappelle-moi à 18h72 d'appeler maman",
        "tu te rappelles ce que j'ai dit hier",
        "mets un minuteur",
        "mets un minuteur de 0 minute",
        "mets un minuteur de 5 pommes",
        "set a timer",
        "remind me later",
        "annule le minuteur de la veille",
        "dans 10 minutes",
        "mets de la musique dans 10 minutes",
    ] {
        let a = quick(phrase).unwrap_or_default();
        assert!(!a.iter().any(|s| s["type"].as_str().is_some_and(|t| t.starts_with("remind_"))), "must not be a reminder: \"{phrase}\" -> {a:?}");
    }
    // other modules keep their phrases
    assert_eq!(quick("mets Bohemian Rhapsody").unwrap()[0]["type"], "music_play");
}

#[test]
fn parses_list_and_cancel() {
    assert_eq!(parsed("quels sont mes rappels")["type"], "remind_list");
    assert_eq!(parsed("what are my reminders")["type"], "remind_list");
    let a = parsed("annule mon rappel");
    assert_eq!((a["type"].as_str(), a["kind"].as_str(), a["all"].is_null()), (Some("remind_cancel"), Some("reminder"), true));
    assert_eq!(parsed("annule le minuteur")["kind"], "timer");
    let a = parsed("annule tous mes rappels");
    assert_eq!((a["all"].as_bool(), a["kind"].as_str()), (Some(true), Some("")));
    assert_eq!(parsed("cancel all my reminders")["all"], true);
}

#[test]
fn next_at_is_today_or_tomorrow() {
    let now = Local.with_ymd_and_hms(2026, 11, 20, 17, 0, 0).unwrap();
    assert_eq!(next_at(now, 18, 30).unwrap(), Local.with_ymd_and_hms(2026, 11, 20, 18, 30, 0).unwrap());
    assert_eq!(next_at(now, 9, 0).unwrap(), Local.with_ymd_and_hms(2026, 11, 21, 9, 0, 0).unwrap());
    assert_eq!(next_at(now, 17, 0).unwrap(), Local.with_ymd_and_hms(2026, 11, 21, 17, 0, 0).unwrap());
}

// ---------- Set, fire, list, cancel ----------

#[tokio::test]
async fn set_then_fire_with_an_injected_clock() {
    let mut rig = Rig::new();
    let said = rig.hear(MARIE, "Bobby, rappelle-moi dans 10 minutes de sortir le linge", "fr").await;
    assert_eq!(said.last().map(String::as_str), Some("C'est noté, je te le rappelle dans 10 minutes."), "{said:?}");
    let said = rig.hear(MARIE, "Bobby, mets un minuteur de 5 minutes", "fr").await;
    assert_eq!(said.last().map(String::as_str), Some("Minuteur de 5 minutes lancé."), "{said:?}");
    let docs = rig.docs();
    assert_eq!(docs.len(), 2);
    let (linge, timer) = (&docs[0], &docs[1]);
    assert_eq!((linge["user"].as_str(), linge["name"].as_str(), linge["text"].as_str()), (Some("42"), Some("Marie"), Some("sortir le linge")));
    // nothing is due yet
    assert!(rig.fire(due_of(timer) - 1000).await.is_empty());
    // the timer, on time: said to Marie only
    let out = rig.fire(due_of(timer) + 1000).await;
    assert_eq!(out, vec![("Marie, ton minuteur de 5 minutes est fini.".to_string(), "fr".to_string(), Some("42".to_string()))]);
    assert_eq!(rig.docs().len(), 1);
    // the reminder
    let out = rig.fire(due_of(linge) + 1000).await;
    assert_eq!(out[0].0, "Marie, rappel : sortir le linge.");
    assert!(rig.docs().is_empty());
    assert!(rig.fire(due_of(linge) + 5000).await.is_empty()); // said once
}

#[tokio::test]
async fn english_set_and_fire() {
    let mut rig = Rig::new();
    let said = rig.hear(MARIE, "Bobby, remind me in 10 minutes to take out the laundry", "en").await;
    assert_eq!(said.last().map(String::as_str), Some("Got it, I'll remind you in 10 minutes."), "{said:?}");
    let said = rig.hear(MARIE, "Bobby, set a timer for 5 minutes", "en").await;
    assert_eq!(said.last().map(String::as_str), Some("Timer set for 5 minutes."), "{said:?}");
    let docs = rig.docs();
    let out = rig.fire(due_of(&docs[1]) + 1000).await;
    assert_eq!((out[0].0.as_str(), out[0].1.as_str()), ("Marie, your timer for 5 minutes is done.", "en"));
    let out = rig.fire(due_of(&docs[0]) + 1000).await;
    assert_eq!((out[0].0.as_str(), out[0].1.as_str()), ("Marie, reminder: take out the laundry.", "en"));
}

#[tokio::test]
async fn clock_time_reminder() {
    let mut rig = Rig::new();
    let said = rig.hear(MARIE, "Bobby, rappelle-moi à 18h30 d'appeler maman", "fr").await;
    let due = due_of(&rig.docs()[0]);
    let d = Local.timestamp_millis_opt(due).unwrap();
    assert_eq!(d.format("%H:%M").to_string(), "18:30");
    let tomorrow = d.date_naive() != Local::now().date_naive();
    let expect = if tomorrow { "C'est noté, je te le rappelle demain à 18 h 30." } else { "C'est noté, je te le rappelle à 18 h 30." };
    assert_eq!(said.last().map(String::as_str), Some(expect), "{said:?}");
}

#[tokio::test]
async fn list_and_cancel() {
    let mut rig = Rig::new();
    rig.hear(MARIE, "Bobby, rappelle-moi dans 10 minutes de sortir le linge", "fr").await;
    rig.hear(MARIE, "Bobby, mets un minuteur de 5 minutes", "fr").await;
    rig.hear(NOBODY, "Bobby, mets un minuteur de 9 minutes", "fr").await; // no rights: not stored
    let docs = rig.docs();
    let at = |i: usize| clock_text(due_of(&docs[i]), "fr");
    let said = rig.hear(MARIE, "Bobby, quels sont mes rappels", "fr").await;
    assert_eq!(said.last().unwrap(), &format!("Tu as 2 rappels : sortir le linge à {}, minuteur à {}.", at(0), at(1)));
    // "annule mon rappel" cancels the latest reminder, not the timer
    let said = rig.hear(MARIE, "Bobby, annule mon rappel", "fr").await;
    assert_eq!(said.last().unwrap(), "J'ai annulé ton rappel.");
    assert_eq!(rig.docs().len(), 1);
    assert_eq!(rig.hear(MARIE, "Bobby, annule mon rappel", "fr").await.last().unwrap(), "Tu n'as aucun rappel.");
    // the latest timer
    rig.hear(MARIE, "Bobby, minuteur 3 minutes", "fr").await;
    assert_eq!(rig.docs().len(), 2);
    let said = rig.hear(MARIE, "Bobby, annule le minuteur", "fr").await;
    assert_eq!(said.last().unwrap(), "J'ai annulé ton minuteur.");
    assert_eq!(rig.docs().len(), 1);
    assert_eq!(rig.docs()[0]["secs"], 300); // the older timer is still there
    // all of them
    rig.hear(MARIE, "Bobby, rappelle-moi dans 1 heure de dormir", "fr").await;
    let said = rig.hear(MARIE, "Bobby, annule tous mes rappels", "fr").await;
    assert_eq!(said.last().unwrap(), "J'ai annulé tes 2 rappels.");
    assert!(rig.docs().is_empty());
    assert_eq!(rig.hear(MARIE, "Bobby, quels sont mes rappels", "fr").await.last().unwrap(), "Tu n'as aucun rappel.");
}

#[tokio::test]
async fn lists_and_cancels_are_personal() {
    let mut rig = Rig::new();
    rig.hear(MARIE, "Bobby, mets un minuteur de 5 minutes", "fr").await;
    let mut other = rig.docs()[0].clone();
    other["user"] = json!("lucas");
    other["id"] = json!(2);
    let mut all = rig.docs();
    all.push(other);
    rig.brain.store.save("reminders", &Value::Array(all)).unwrap();
    assert!(rig.hear(MARIE, "Bobby, quels sont mes rappels", "fr").await.last().unwrap().starts_with("Tu as 1 rappel :"));
    rig.hear(MARIE, "Bobby, annule tous mes rappels", "fr").await;
    assert_eq!(rig.docs().len(), 1);
    assert_eq!(rig.docs()[0]["user"], "lucas");
}

#[tokio::test]
async fn max_per_person() {
    let mut rig = Rig::new();
    for _ in 0..MAX_PER_USER {
        rig.hear(MARIE, "Bobby, mets un minuteur de 5 minutes", "fr").await;
    }
    assert_eq!(rig.docs().len(), MAX_PER_USER);
    let said = rig.hear(MARIE, "Bobby, mets un minuteur de 5 minutes", "fr").await;
    assert_eq!(said.last().unwrap(), &format!("Tu as déjà {MAX_PER_USER} rappels, annule-en un d'abord."));
    assert_eq!(rig.docs().len(), MAX_PER_USER);
}

// ---------- Persistence ----------

#[tokio::test]
async fn a_restart_fires_overdue_reminders_and_drops_old_ones() {
    let dir = temp_dir("reminders-db");
    let db = dir.join("bobby.db");
    let now = 1_800_000_000_000_i64;
    let r = |id: i64, text: &str, due: i64| json!({ "id": id, "user": "42", "name": "Marie", "kind": "reminder", "text": text, "due": due, "secs": 60, "lang": "fr" });
    {
        let store = Store::open(&db, &dir.join("data")).unwrap();
        let all = vec![r(1, "sortir le linge", now - 5 * 60_000), r(2, "vieux rappel", now - MAX_LATE_MS - 1), r(3, "plus tard", now + 60_000)];
        store.save("reminders", &Value::Array(all)).unwrap();
    } // Bobby stops
    let store = Store::open(&db, &dir.join("data")).unwrap(); // Bobby starts again
    let (io, sent) = fake_client(|_| Ok(Value::Null));
    assert_eq!(fire_due(&store, &io, now), 1);
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    let said: Vec<_> = sent.lock().unwrap().iter().filter_map(|m| if let BrainMsg::Say { text, to_user, voice, .. } = m { Some((text.clone(), to_user.clone(), *voice == Voice::Bobby)) } else { None }).collect();
    assert_eq!(said, vec![("Marie, rappel : sortir le linge (en retard).".to_string(), Some("42".to_string()), true)]);
    // the late one is gone, the old one dropped, the future one kept
    let left = store.load_or("reminders", json!([]));
    assert_eq!(left.as_array().unwrap().iter().map(|x| x["text"].as_str().unwrap()).collect::<Vec<_>>(), ["plus tard"]);
    // a timer a few seconds late is on time, not "en retard"
    let t = json!({ "id": 9, "user": "42", "name": "Marie", "kind": "timer", "text": "", "due": now, "secs": 120, "lang": "fr" });
    assert_eq!(rem::fire_text(&t, false), "Marie, ton minuteur de 2 minutes est fini.");
}

// ---------- Rights and help ----------

#[tokio::test]
async fn needs_the_prime_role() {
    let mut rig = Rig::new();
    let said = rig.hear(NOBODY, "Bobby, rappelle-moi dans 10 minutes de sortir le linge", "fr").await;
    assert!(rig.docs().is_empty(), "{said:?}");
    assert!(!said.iter().any(|s| s.contains("noté")), "{said:?}");
    let ctx = crate::testing::ctx();
    let plan = resolve(&quick("mets un minuteur de 5 minutes").unwrap(), &ctx, &rig.brain.store).unwrap();
    assert_eq!(needed(&plan, None), Role::Prime);
    for q in ["quels sont mes rappels", "annule mon rappel"] {
        assert_eq!(needed(&resolve(&quick(q).unwrap(), &ctx, &rig.brain.store).unwrap(), None), Role::Prime, "{q}");
    }
}

#[test]
fn help_mentions_reminders_and_the_llm_schema_does_not() {
    assert!(info::capabilities("fr").contains("rappels"));
    assert!(info::capabilities("en").contains("reminders"));
    assert!(info::abilities(Role::Prime, "fr").contains("rappels"));
    assert!(!info::abilities(Role::Other, "fr").contains("rappels"));
    assert!(actions::types().iter().all(|t| !t.starts_with("remind_")));
    assert!(actions::def("remind_set").is_some());
}
