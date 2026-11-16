//! The entry point of Bobby's brain: `hear()` and everything around it (port of `bot/brain.js` lines 404-762):
//! the undo registry, what Bobby says, running a plan, pending confirmations, the journal line of every exchange.
//! Node kept module-level maps; they live in `Brain::state` here (one `std::sync::Mutex`, never held across an await).
use crate::actions::{self, ask, Env, Post, UndoOp};
use crate::guard::{decide, guard, note_guard, note_text, understand};
use crate::io::Io;
use crate::journal::{self, num_f32, Journal, Memory};
use crate::prompt::{prompt, system, unsure};
use crate::resolve::{needed, needs_confirm, names, plan_text, resolve};
use crate::roles;
use crate::shortcuts::{cancels, confirms, mine_of, quick, retry, NO, STOP};
use crate::store::Store;
use crate::types::{Chan, Ctx, Echo, Role, Step};
use crate::util::{hit, re_i, replace1, replace_all, BrainError, Hist, Res};
use crate::wake::find_wake;
use crate::writing::{writing, Written, RIGHTS_Q};
use bobby_proto::infer::Understand;
use bobby_proto::msg::{ClientMsg, Context, Embed, SignalKind, Voice};
use chrono::Local;
use fancy_regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Instant;

// ---------- What Bobby says ----------

/// One language of `LINES`.
pub struct Lines {
    pub confirm: &'static str,
    pub cancelled: &'static str,
    pub not_understood: &'static str,
    pub no_rights: &'static str,
    pub failed: &'static str,
    pub not_allowed: &'static str,
    pub forbidden: &'static str,
    pub can_do: &'static str,
    pub want_me: &'static str,
    pub call: &'static str,
    pub thinking: &'static str,
    undo_done: &'static str,
    undo_none: &'static str,
    wrote_pre: &'static str,
    wrote_post: &'static str,
}

impl Lines {
    pub fn undo(&self, done: bool) -> &'static str {
        if done { self.undo_done } else { self.undo_none }
    }
    pub fn wrote(&self, where_: &str) -> String {
        format!("{}{where_}{}", self.wrote_pre, self.wrote_post)
    }
}

pub static LINES_FR: Lines = Lines {
    undo_done: "J'annule.",
    undo_none: "Il n'y a rien à annuler.",
    confirm: "Tu confirmes ?",
    cancelled: "D'accord, j'annule.",
    not_understood: "Je n'ai pas compris.",
    no_rights: "Je n'ai pas les droits Discord pour ça.",
    failed: "Ça n'a pas marché.",
    not_allowed: "Je n'ai pas le droit de te répondre.",
    forbidden: "Tu n'as pas le droit de me demander ça.",
    wrote_pre: "Je t'envoie ça dans ",
    wrote_post: ".",
    can_do: "Oui, je peux.",
    want_me: "Tu veux que je le fasse ?",
    call: "Je ne peux pas passer d'appel, mais je peux te mentionner dans le salon texte. Tu veux ?",
    thinking: "Je réfléchis.",
};

pub static LINES_EN: Lines = Lines {
    undo_done: "Undone.",
    undo_none: "Nothing to undo.",
    confirm: "Confirm?",
    cancelled: "Okay, cancelled.",
    not_understood: "Sorry, I didn't get that.",
    no_rights: "I don't have the Discord permissions for that.",
    failed: "That didn't work.",
    not_allowed: "I'm not allowed to answer you.",
    forbidden: "You're not allowed to ask me that.",
    wrote_pre: "Sent to ",
    wrote_post: ".",
    can_do: "Yes, I can.",
    want_me: "Do you want me to do it?",
    call: "I can't make calls, but I can mention you in the text channel. Want me to?",
    thinking: "Let me think.",
};

/// `LINES[lang === 'en' ? 'en' : 'fr']`.
pub fn lines(lang: &str) -> &'static Lines {
    if lang == "en" { &LINES_EN } else { &LINES_FR }
}

/// Which role each action needs, for the startup log and the README.
pub fn role_table() -> Vec<(&'static str, Role)> {
    actions::types().into_iter().map(|k| (k, actions::def(k).map_or(Role::Admin, |d| d.role))).collect()
}

/// Coworker tone: a step whose action is an echo ("Je déplace WakandeX dans…") stays silent.
fn echoes(s: &Step) -> bool {
    match actions::def(&s.kind).map(|d| &d.echo) {
        Some(Echo::Yes) => true,
        Some(Echo::If(f)) => f(s),
        _ => false,
    }
}

/// Known before running: an order whose action has an echo is worth an ack.
fn silent_kind(plan: &[Step]) -> bool {
    plan.iter().any(|s| matches!(actions::def(&s.kind).map(|d| &d.echo), Some(Echo::Yes | Echo::If(_))))
}

/// Claude's answers are spoken with another voice.
fn voice_of(plan: &[Step]) -> Option<Voice> {
    plan.iter().find(|s| s.flag("voice")).and_then(|s| (s.str("voice") == Some("claude")).then_some(Voice::Claude))
}

/// JS truthiness of a JSON value.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null | Value::Bool(false) => false,
        Value::String(s) => !s.is_empty(),
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        _ => true,
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn ms_since(t: Instant) -> u64 {
    (t.elapsed().as_secs_f64() * 1000.0).round() as u64
}

// ---------- State ----------

/// "Bobby" said alone (`plan: None`), or a pending confirmation / offer.
#[derive(Debug, Clone)]
pub struct Pending {
    pub until_ms: i64,
    pub plan: Option<Vec<Step>>,
    pub w: Option<Written>,
}

#[derive(Default)]
struct State {
    memory: Memory,
    /// userId -> the role the undone plan needed, and the ops that undo it ("annule" only undoes your own last action).
    undo: HashMap<String, (Role, Vec<UndoOp>)>,
    pending: HashMap<String, Pending>,
    /// userId -> last time Bobby told them it may not answer.
    refused: HashMap<String, i64>,
    /// The user Bobby is answering while it speaks (Node `io.speaking()`).
    speaking: Option<String>,
    next_rec: u64,
}

/// What `hear` is given for one transcribed sentence (Node's `hear({...})` arguments).
#[derive(Debug, Clone, Default)]
pub struct Heard {
    pub text: String,
    pub lang: String,
    pub score: Option<f32>,
    pub user: String,
    /// Discord role names and ids.
    pub user_roles: Vec<String>,
    pub user_name: String,
    pub stt_ms: Option<u32>,
    /// The ack was already played at the wake gate.
    pub acked: bool,
    pub ack_ms: Option<u32>,
    /// The window was open when the sentence started.
    pub expected: bool,
}

pub struct Brain<L> {
    pub store: Store,
    pub llm: L,
    pub io: Arc<Io>,
    pub journal: Journal,
    state: Mutex<State>,
}

/// One `hear` in flight: its journal record, its timing, what was already acked.
struct Req<'a> {
    h: &'a Heard,
    rec: Map<String, Value>,
    t: Map<String, Value>,
    acked: bool,
}

static LEAD_WORDS: LazyLock<Regex> = LazyLock::new(|| re_i(r"^(?:[\s,.:;!?]+|bobby\b)+"));

impl<L: Understand + Sync> Brain<L> {
    /// Reloads the conversation memory from the journal (a restart loses nothing).
    pub fn new(store: Store, llm: L, io: Arc<Io>, journal_path: impl Into<PathBuf>) -> Self {
        let journal = Journal::new(journal_path);
        let mut memory = Memory::new();
        let n = memory.reload(&journal.path, now_ms());
        println!("[brain] {n} past exchange(s) reloaded from the journal");
        Brain { store, llm, io, journal, state: Mutex::new(State { memory, ..Default::default() }) }
    }

    fn st(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// What the client tells the brain outside `hear`: results of `Run`s, and whether Bobby's voice is on.
    pub fn on_client(&self, m: &ClientMsg) {
        match m {
            ClientMsg::Result { id, ok, data, error } => {
                self.io.deliver(*id, *ok, data.clone(), error.clone());
            }
            ClientMsg::Speaking { user } => self.st().speaking.clone_from(user),
            _ => {}
        }
    }

    /// True while Bobby waits for this person's next sentence, which then needs no "Bobby".
    pub fn expecting(&self, user: &str) -> bool {
        self.st().pending.get(user).is_some_and(|p| p.until_ms > now_ms())
    }

    // ---------- Undo registry ----------

    fn undo_role(&self, plan: &[Step]) -> Option<Role> {
        self.st().undo.get(plan.first().map_or("", |s| s.by.id.as_str())).map(|u| u.0)
    }

    /// Highest role the plan needs; undo needs the role of what it undoes.
    fn needed(&self, plan: &[Step]) -> Role {
        needed(plan, self.undo_role(plan))
    }

    // ---------- What Bobby says about a plan ----------

    fn step_say(&self, s: &Step, plan: &[Step], lang: &str) -> Option<String> {
        if s.kind == "undo" {
            let done = if s.f.contains_key("done") { s.flag("done") } else { self.undo_role(plan).is_some() };
            return Some(lines(lang).undo(done).to_string());
        }
        let say = actions::def(&s.kind)?.say?;
        let lang = if lang == "en" { "en" } else { "fr" };
        let n = s.targets.as_ref().map(|t| names(t, lang));
        say(s, n.as_deref(), lang)
    }

    fn describe(&self, plan: &[Step], lang: &str) -> String {
        plan.iter().filter_map(|s| self.step_say(s, plan, lang)).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ")
    }

    /// An order gets a short ack and is then done without a word; answers, errors and anything useful are spoken.
    fn report(&self, plan: &[Step], lang: &str) -> String {
        let spoken: Vec<Step> = plan.iter().filter(|s| !echoes(s)).cloned().collect();
        // step_say's undo lookup needs the whole plan's speaker, which every step shares
        spoken.iter().filter_map(|s| self.step_say(s, plan, lang)).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ")
    }

    // ---------- Execution ----------

    /// Node `run()`: runs every step, then registers what undoes them for this person.
    pub async fn run_plan(&self, plan: &mut [Step], env: &Env<'_, L>, lang: &str) -> Res<()> {
        let mut ops: Vec<UndoOp> = vec![];
        let who = plan.first().map(|s| s.by.id.clone()).unwrap_or_default();
        for s in plan.iter_mut() {
            if s.kind == "undo" {
                let u = self.st().undo.get(&who).cloned();
                s.set("done", u.is_some());
                if let Some((_, ops)) = u {
                    for op in ops.iter().rev() {
                        match op {
                            UndoOp::Cap(cap) => {
                                env.io.run(cap.clone()).await?;
                            }
                            UndoOp::Restore { name, prev } => env.store.save(name, prev).map_err(BrainError::failed)?,
                        }
                    }
                    self.st().undo.remove(&who);
                }
                continue;
            }
            ops.extend(actions::run(s, env, lang).await?);
        }
        if !ops.is_empty() {
            let role = self.needed(plan);
            self.st().undo.insert(who, (role, ops));
        } else if plan.iter().any(|s| s.kind == "undo") {
            self.st().undo.remove(&who);
        }
        Ok(())
    }

    /// After a plan asked in writing: steps that posted by themselves (lists) set `where`; the others post their card,
    /// or what they would have said. Returns the short spoken line.
    async fn post_written(&self, plan: &[Step], env: &Env<'_, L>, lang: &str) -> Res<String> {
        let mut where_ = plan.iter().find(|s| s.flag("where")).and_then(|s| s.str("where")).map(String::from);
        for s in plan.iter().filter(|s| !s.flag("where")) {
            let card = match s.f.get("card") {
                Some(Value::String(t)) if !t.is_empty() => Some(Post::Text(t.clone())),
                Some(c @ Value::Array(_)) => serde_json::from_value::<Vec<Embed>>(c.clone()).ok().map(Post::Embeds),
                _ => None,
            };
            let content = card.or_else(|| self.step_say(s, plan, lang).filter(|t| !t.is_empty()).map(Post::Text));
            if let Some(c) = content {
                where_ = Some(env.post(c).await?);
            }
        }
        Ok(match where_.filter(|w| !w.is_empty()) {
            Some(w) => lines(lang).wrote(&w),
            None => self.describe(plan, lang),
        })
    }

    fn env<'a>(&'a self, ctx: &'a Ctx, post_to: Option<&'a Chan>) -> Env<'a, L> {
        Env { io: &self.io, store: &self.store, ctx, llm: &self.llm, post_to }
    }

    // ---------- Fallback brain ----------

    /// Fallback brain for admins: when the local model is unsure, Claude reads the same prompt and picks among the same
    /// actions (5-10 s). Its answer goes through the same guards; any failure keeps the local answer.
    async fn rethink(&self, out: Value, query: &str, ctx: &Ctx, lang: &str, history: &[Hist]) -> Value {
        let better: Result<Value, String> = async {
            let better = ask::think(system(), &prompt(ctx, query, lang, history, &self.store, Local::now())).await?;
            let ok = better["actions"].as_array().is_some_and(|a| a.iter().all(|x| x["type"].as_str().is_some_and(|t| actions::def(t).is_some())));
            if !ok {
                return Err(format!("bad plan {}", better.to_string().chars().take(200).collect::<String>()));
            }
            // { say: '', ...better, analyse: { genre: 'ordre', faisable: true, ...better.analyse } }
            let mut merged = Map::new();
            merged.insert("say".into(), json!(""));
            for (k, v) in better.as_object().into_iter().flatten() {
                merged.insert(k.clone(), v.clone());
            }
            let mut analyse = Map::new();
            analyse.insert("genre".into(), json!("ordre"));
            analyse.insert("faisable".into(), json!(true));
            for (k, v) in better["analyse"].as_object().into_iter().flatten() {
                analyse.insert(k.clone(), v.clone());
            }
            merged.insert("analyse".into(), Value::Object(analyse));
            let mut g = guard(Value::Object(merged), query, ctx, history, lang);
            g["via"] = json!("claude");
            Ok(g)
        }
        .await;
        better.unwrap_or_else(|e| {
            eprintln!("[claude] fallback brain failed: {e}");
            out
        })
    }

    // ---------- Entry point ----------

    fn set_pending(&self, user: &str, ms: i64, plan: Option<Vec<Step>>, w: Option<Written>) {
        self.st().pending.insert(user.into(), Pending { until_ms: now_ms() + ms, plan, w });
        // the client listens to this person without a wake word meanwhile
        self.io.expect(user, ms as u32);
    }

    /// "Tu veux que je l'ajoute à la liste ?": a "oui" within 15 s adds the whole sentence to the general list.
    fn offer_note(&self, user: &str, query: &str, ctx: &Ctx) -> Res<()> {
        let plan = resolve(&[json!({ "type": "todo_add", "text": note_text(query), "list": "general" })], ctx, &self.store)?;
        self.set_pending(user, 15000, Some(plan), None);
        Ok(())
    }

    /// Nothing to say (e.g. "stop" with no music): stay silent. Not awaited: the reply plays on while the journal is
    /// written; the person may interrupt it.
    fn say(&self, r: &mut Req<'_>, s: &str, voice: Option<Voice>) {
        r.rec.insert("reply".into(), json!(s));
        self.io.say(s, &r.h.lang, voice.unwrap_or(Voice::Bobby), Some(&r.h.user));
    }

    /// Heard "Bobby" but not understood, or not allowed.
    fn fail(&self, r: &mut Req<'_>, s: &str) {
        self.io.signal(SignalKind::Nope);
        self.say(r, s, None);
    }

    /// One short ack per order, before acting: at the wake gate (`acked`), else here.
    fn ack(&self, r: &mut Req<'_>) {
        if !r.acked {
            r.acked = true;
            self.io.signal(SignalKind::Ack);
        }
    }

    /// A plan Bobby proposes instead of running ("Tu veux que je le fasse ?", runs on a "oui").
    fn offer(&self, r: &mut Req<'_>, plan: &[Step], line: &str, w: Option<Written>) {
        let plan = plan.iter().cloned().map(|mut s| { s.f.remove("offer"); s }).collect();
        self.set_pending(&r.h.user, 15000, Some(plan), w);
        r.rec.insert("outcome".into(), json!("offered"));
        self.say(r, line, None);
    }

    fn written(&self, r: &mut Req<'_>, ctx: &Ctx, w: &Written) {
        let name = w.channel.as_ref().map(|c| c.name.clone()).or_else(|| ctx.text_channel.as_ref().map(|c| c.name.clone()));
        r.rec.insert("written".into(), name.map_or(json!(true), Value::from));
        println!("[written] \"{}\" -> {}", w.query, w.channel.as_ref().map_or("default text channel", |c| c.name.as_str()));
    }

    fn ctx_for(&self, c: &Context, role: Role, history: Vec<Hist>) -> Ctx {
        let trouble = self.st().memory.trouble(now_ms());
        Ctx { role, history, trouble, ..crate::context::build(c, &self.store) }
    }

    /// The transcript must still contain "Bobby" (the wake word model upstream is a cheap, lenient filter), so a false
    /// trigger ("beau bilan") costs one transcription and never an action.
    pub async fn hear(&self, h: Heard, c: &Context) {
        let started = Instant::now();
        let now = now_ms();
        let lang = if h.lang.is_empty() { "fr".to_string() } else { h.lang.clone() };
        let h = Heard { lang, ..h };
        let (mut p, talking) = {
            let st = self.st();
            (st.pending.get(&h.user).cloned(), st.speaking.clone())
        };
        // expected: the window was open when the sentence started, even if it closed while the person was talking
        if p.as_ref().is_some_and(|p| now > p.until_ms) && !h.expected {
            p = None;
        }
        let wake = find_wake(&h.text).map(String::from);
        let mut rec = Map::new();
        rec.insert("at".into(), json!(journal::iso_now_ms(now)));
        rec.insert("who".into(), json!(h.user_name));
        rec.insert("id".into(), json!(h.user));
        rec.insert("lang".into(), json!(h.lang));
        rec.insert("wakeScore".into(), h.score.map_or(Value::Null, num_f32));
        rec.insert("heard".into(), json!(h.text));
        let with = |rec: &Map<String, Value>, outcome: &str| {
            let mut r = rec.clone();
            r.insert("outcome".into(), json!(outcome));
            r
        };
        // Bobby is speaking: the person it answers stops it with a plain "stop"; an admin needs "Bobby, stop"; anyone
        // else's "stop" is ignored. The voice stops, never the music. A pending "oui ?" keeps its own rules.
        if let Some(talking) = talking.filter(|_| !p.as_ref().is_some_and(|p| p.plan.is_some()) && hit(&STOP, h.text.trim())) {
            let ok = talking == h.user || (wake.is_some() && roles::allowed(roles::role_of(&h.user, &h.user_roles, &self.store), Role::Admin));
            let mut r = with(&rec, if ok { "interrupted" } else { "stop_ignored" });
            r.insert("answering".into(), json!(talking));
            self.journal.append(&r);
            if !ok {
                return println!("[stop] ignored: {} is not the one Bobby answers", h.user_name);
            }
            self.io.hush();
            return println!("[stop] {} interrupted Bobby", h.user_name);
        }
        if wake.is_none() && p.is_none() {
            self.st().memory.remember(&with(&rec, "ignored"), now);
            self.journal.append(&with(&rec, "ignored"));
            return println!("[ignored] no \"Bobby\" in \"{}\"", h.text);
        }
        let l = lines(&h.lang);
        let role = roles::role_of(&h.user, &h.user_roles, &self.store);
        rec.insert("role".into(), json!(role.as_str()));
        if role == Role::Other {
            self.journal.append(&with(&rec, "not_allowed"));
            println!("[rights] {} ({}) has no role that lets them talk to Bobby", h.user_name, h.user);
            self.io.signal(SignalKind::Nope); // heard, but no rights: the low sound, and the sentence at most every 30 s
            let tell = {
                let mut st = self.st();
                let tell = now - st.refused.get(&h.user).copied().unwrap_or(0) > 30_000;
                if tell {
                    st.refused.insert(h.user.clone(), now);
                }
                tell
            };
            if tell {
                self.io.say(l.not_allowed, &h.lang, Voice::Bobby, None);
            }
            return;
        }
        let key = {
            let mut st = self.st();
            st.pending.remove(&h.user);
            st.next_rec += 1;
            st.next_rec
        };
        let mut t = Map::new();
        if let Some(s) = h.stt_ms {
            t.insert("stt".into(), json!(s));
        }
        t.insert("ack".into(), h.ack_ms.map_or(Value::Null, Value::from));
        let acked = h.acked;
        let mut r = Req { h: &h, rec, t, acked };
        let res = self.body(&mut r, c, p, wake.as_deref(), role, key).await;
        if let Err(e) = res {
            match e {
                BrainError::Say(s) => {
                    r.rec.insert("outcome".into(), json!(if s.code == Some("LLM_DOWN") { "llm_down" } else { "resolve_error" }));
                    r.rec.insert("error".into(), json!(s.fr));
                    println!("[resolve] {}", s.fr);
                    if s.fr == LINES_FR.not_understood { self.fail(&mut r, s.in_(&h.lang)) } else { self.say(&mut r, s.in_(&h.lang), None) }
                }
                BrainError::Failed { msg, no_rights } => {
                    r.rec.insert("outcome".into(), json!("error"));
                    r.rec.insert("error".into(), json!(msg));
                    eprintln!("[bobby] {msg}");
                    self.say(&mut r, if no_rights { l.no_rights } else { l.failed }, None);
                }
            }
        }
        {
            let mut st = self.st();
            st.memory.finish(key);
            st.memory.remember(&r.rec, now_ms());
        }
        r.t.insert("total".into(), json!(u64::from(h.stt_ms.unwrap_or(0)) + ms_since(started)));
        println!("[timing] {}", r.t.iter().filter(|(_, v)| !v.is_null()).map(|(k, v)| format!("{k} {v} ms")).collect::<Vec<_>>().join(" · "));
        r.rec.insert("timing".into(), Value::Object(std::mem::take(&mut r.t)));
        self.journal.append(&r.rec);
    }

    /// The `try` of Node's `hear`: `Ok` = a regular end (the outcome is in the record), `Err` = the `catch`.
    async fn body(&self, r: &mut Req<'_>, c: &Context, mut p: Option<Pending>, wake: Option<&str>, role: Role, key: u64) -> Res<()> {
        let h = r.h;
        let (text, lang, user) = (h.text.as_str(), h.lang.as_str(), h.user.as_str());
        let l = lines(lang);
        let set = |r: &mut Req<'_>, k: &str, v: Value| {
            r.rec.insert(k.into(), v);
        };
        // "Bobby, …" while a yes is awaited: a new order, not a "no". The pending plan is dropped silently.
        if let Some(pp) = p.as_ref().and_then(|p| p.plan.as_ref()) {
            if wake.is_some() && !confirms(text) && !hit(&NO, text) {
                println!("[confirm] dropped \"{}\": new order", plan_text(pp));
                p = None;
            }
        }
        if let Some(Pending { plan: Some(mut plan), w, .. }) = p {
            set(r, "plan", json!(plan_text(&plan)));
            set(r, "query", json!(text));
            set(r, "confirm", json!(true)); // a yes / no: never replayed by "refais"
            if !confirms(text) {
                set(r, "outcome", json!("cancelled"));
                self.say(r, l.cancelled, None);
                return Ok(());
            }
            if !roles::allowed(role, self.needed(&plan)) {
                // rights may have changed meanwhile
                set(r, "outcome", json!("forbidden"));
                self.fail(r, l.forbidden);
                return Ok(());
            }
            set(r, "outcome", json!("confirmed"));
            if silent_kind(&plan) {
                self.ack(r);
            }
            let t0 = Instant::now();
            let history = self.st().memory.conversation(now_ms());
            let ctx = self.ctx_for(c, role, history);
            // asked in writing before the "oui"
            let env = self.env(&ctx, w.as_ref().and_then(|w| w.channel.as_ref()));
            self.run_plan(&mut plan, &env, lang).await?;
            r.t.insert("action".into(), json!(ms_since(t0)));
            if w.is_some() {
                let line = self.post_written(&plan, &env, lang).await?;
                self.say(r, &line, None);
            } else {
                let line = self.report(&plan, lang);
                self.say(r, &line, voice_of(&plan));
            }
            return Ok(());
        }
        // "OK STOP BOBBY BOBBY STOP": the command follows the last of the repeated "Bobby".
        let after = match wake {
            Some(w) => &text[text.find(w).map_or(0, |i| i + w.len())..],
            None => text,
        };
        let history = self.st().memory.conversation(now_ms());
        let mut query = retry(replace1(&LEAD_WORDS, after, "").trim(), &mine_of(&history, user), &history);
        set(r, "query", json!(query));
        println!("[query] \"{query}\" ({lang})");
        static NOT_LETTER: LazyLock<Regex> = LazyLock::new(|| crate::util::re(r"[^\p{L}]"));
        if replace_all(&NOT_LETTER, &query, "").chars().count() < 2 {
            // "Bobby ?" alone: a beep, then the next sentence is the order (no "Bobby" needed).
            self.io.signal(SignalKind::Beep); // the short beep (it also lights Bobby's ring): heard, go on
            self.set_pending(user, 8000, None, None);
            set(r, "outcome", json!("waiting"));
            println!("[waiting] \"Bobby\" alone: next sentence is the command (8 s)");
            return Ok(());
        }
        let ctx = self.ctx_for(c, role, history.clone());
        self.st().memory.start(key, &h.user_name, &query, now_ms());
        // "dis-moi par écrit…": the request without its writing words; the answer is posted, Bobby only says where.
        let mut w: Option<Written> = None;
        if let Some(asked) = writing(&query, &ctx, &mine_of(&history, user)) {
            query = asked.query.clone();
            self.written(r, &ctx, &asked);
            w = Some(asked);
            if query.is_empty() {
                set(r, "outcome", json!("resolve_error"));
                self.fail(r, l.not_understood);
                return Ok(());
            }
            let wc = w.as_ref().and_then(|w| w.channel.as_ref());
            if wc.is_some_and(|ch| Some(&ch.name) != ctx.text_channel.as_ref().map(|t| &t.name)) && !roles::allowed(role, Role::Admin) {
                set(r, "outcome", json!("forbidden"));
                self.fail(r, l.forbidden);
                return Ok(());
            }
        }
        // A song title may end in a cancel word ("joue Nevermind"): playing wins.
        let fast = quick(&query);
        let music = fast.as_ref().is_some_and(|a| a.iter().any(|x| x["type"].as_str().is_some_and(|t| t.starts_with("music_"))));
        if !music && cancels(&query) {
            set(r, "outcome", json!("cancelled_by_speaker"));
            self.say(r, l.cancelled, None);
            return Ok(());
        }
        let mut t0 = Instant::now();
        let mut plan: Option<Vec<Step>> = None;
        let mut out: Option<Value> = None;
        if let Some(fast) = fast {
            // name not found: let the LLM try
            if let Ok(p) = resolve(&fast, &ctx, &self.store) {
                plan = Some(p);
                r.t.insert("shortcut".into(), json!(0));
                set(r, "intent", json!({ "via": "shortcut", "actions": fast }));
                println!("[intent] shortcut: {}", Value::from(fast));
            }
        }
        let mut plan = match plan {
            Some(p) => p,
            None => {
                let mut o = understand(&self.llm, &ctx, &query, lang, &history, &self.store, Local::now()).await?;
                r.t.insert("llm".into(), json!(ms_since(t0)));
                println!("[intent] LLM: {o}");
                if unsure(&o) && roles::allowed(role, Role::Admin) && ask::ready() {
                    self.io.say(l.thinking, lang, Voice::Bobby, Some(user));
                    t0 = Instant::now();
                    o = self.rethink(o, &query, &ctx, lang, &history).await;
                    r.t.insert("claude".into(), json!(ms_since(t0)));
                    println!("[intent] Claude: {o}");
                }
                let mut intent = Map::new();
                intent.insert("via".into(), o.get("via").filter(|v| truthy(v)).cloned().unwrap_or_else(|| json!("llm")));
                for (k, v) in o.as_object().into_iter().flatten() {
                    intent.insert(k.clone(), v.clone());
                }
                set(r, "intent", Value::Object(intent));
                // "j'aurais voulu des tableaux": the model asks for a written answer itself
                if truthy(&o["ecrit"]) && w.is_none() {
                    let x = Written { query: query.clone(), channel: None };
                    self.written(r, &ctx, &x);
                    w = Some(x);
                }
                let acts = o["actions"].as_array().cloned().unwrap_or_default();
                let say = o["say"].as_str().unwrap_or("").to_string();
                if acts.is_empty() {
                    set(r, "outcome", json!("answered"));
                    if let (Some(w), true) = (&w, !say.is_empty()) {
                        let where_ = self.env(&ctx, w.channel.as_ref()).post(Post::Text(say)).await?;
                        self.say(r, &l.wrote(&where_), None);
                        return Ok(());
                    }
                    if say.is_empty() {
                        self.fail(r, l.not_understood);
                        return Ok(());
                    }
                    let (text, offer) = note_guard(&say, &[], lang);
                    if offer {
                        self.offer_note(user, &query, &ctx)?;
                    }
                    self.say(r, &text, None);
                    return Ok(());
                }
                let p = resolve(&acts, &ctx, &self.store)?;
                out = Some(o);
                p
            }
        };
        let post_to = w.as_ref().and_then(|w| w.channel.clone());
        if w.is_some() {
            for s in plan.iter_mut() {
                s.set("written", true);
                if s.kind == "about" && hit(&RIGHTS_Q, &s.text) {
                    s.kind = "rights_list".into();
                }
            }
        }
        let ptext = plan_text(&plan);
        set(r, "plan", json!(ptext));
        println!("[plan] {ptext}");
        let need = self.needed(&plan);
        if !roles::allowed(role, need) {
            set(r, "outcome", json!("forbidden"));
            println!("[rights] {} is {}, \"{ptext}\" needs {}", h.user_name, role.as_str(), need.as_str());
            self.fail(r, l.forbidden);
            return Ok(());
        }
        // "es-tu capable de supprimer tes messages ?" answers and offers, never runs; "appelle-moi" offers a mention.
        if decide(out.as_ref(), &plan) == "offer" {
            let line = if plan.iter().any(|s| s.str("offer") == Some("call")) {
                l.call.to_string()
            } else {
                let said = out.as_ref().and_then(|o| o["say"].as_str()).filter(|s| !s.is_empty()).unwrap_or(l.can_do);
                format!("{said} {}", l.want_me)
            };
            self.offer(r, &plan, &line, w);
            return Ok(());
        }
        if needs_confirm(&plan) {
            self.set_pending(user, 10000, Some(plan.clone()), w);
            set(r, "outcome", json!("confirm_asked"));
            let line = format!("{} {}", self.describe(&plan, lang), l.confirm);
            self.say(r, &line, None);
            return Ok(());
        }
        if silent_kind(&plan) {
            self.ack(r);
        }
        for s in &plan {
            if let Some(a) = actions::def(&s.kind).and_then(|d| d.announce).and_then(|f| f(lang)) {
                self.io.say(a, lang, Voice::Bobby, Some(user));
            }
        }
        t0 = Instant::now();
        let env = self.env(&ctx, post_to.as_ref());
        self.run_plan(&mut plan, &env, lang).await?;
        r.t.insert("action".into(), json!(ms_since(t0)));
        set(r, "outcome", json!("done"));
        if w.is_some() {
            let line = self.post_written(&plan, &env, lang).await?;
            self.say(r, &line, None);
            return Ok(());
        }
        let (text, offer) = note_guard(&self.report(&plan, lang), &plan, lang);
        if offer {
            self.offer_note(user, &query, &ctx)?;
        }
        self.say(r, &text, voice_of(&plan));
        Ok(())
    }

    /// Shortcut or LLM, as `hear` does it, without running anything (tests). Also says whether it would be written,
    /// and what `hear` would do with the plan ('run', 'offer'), or 'answer' when there is no action.
    /// Unlike Node (which throws), an LLM failure comes back as the `Err`.
    pub async fn intent(&self, query: &str, ctx: &Ctx, lang: &str, history: &[Hist]) -> Res<Value> {
        let mine = mine_of(history, &ctx.speaker.id);
        let mut query = retry(query, &mine, history);
        let w = writing(&query, ctx, &mine);
        if let Some(w) = w.as_ref().filter(|w| !w.query.is_empty()) {
            query = w.query.clone();
        }
        if let Some(fast) = quick(&query) {
            // name not found: the LLM tries
            if let Ok(plan) = resolve(&fast, ctx, &self.store) {
                return Ok(json!({ "via": "shortcut", "actions": fast, "say": "", "written": w.is_some(), "decision": decide(None, &plan) }));
            }
        }
        let out = understand(&self.llm, ctx, &query, lang, history, &self.store, Local::now()).await?;
        let decision = match out["actions"].as_array().filter(|a| !a.is_empty()) {
            Some(acts) => resolve(acts, ctx, &self.store).map_or("error", |plan| decide(Some(&out), &plan)),
            None => "answer",
        };
        let mut res = Map::new();
        res.insert("via".into(), json!("llm"));
        for (k, v) in out.as_object().into_iter().flatten() {
            res.insert(k.clone(), v.clone());
        }
        res.insert("written".into(), json!(w.is_some() || truthy(&out["ecrit"])));
        res.insert("decision".into(), json!(decision));
        Ok(Value::Object(res))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{caps, fake_client, FakeLlm, Sent};
    use bobby_proto::infer::{InferError, InferErrorKind};
    use bobby_proto::msg::{BrainMsg, Cap, Channel, Member};

    const ADMIN: &str = "261531632589078528"; // the default super admin

    fn member(id: &str, name: &str, chan: &str) -> Member {
        Member { id: id.into(), name: name.into(), username: format!("{}_u", name.to_lowercase()), global_name: None, channel: Some(chan.into()), roles: vec![], bot: false }
    }

    fn context() -> Context {
        let ch = |n: &str| Channel { id: n.into(), name: n.into() };
        Context {
            speaker: ADMIN.into(),
            members: vec![member(ADMIN, "WakandeX", "Général"), member("kev", "Kévin", "Général"), member("zyk", "Zyklon", "Gaming"), member("u9", "Passant", "Gaming")],
            voice_channels: vec![ch("Général"), ch("Gaming"), ch("AFK")],
            text_channels: vec![ch("général"), ch("annonces")],
            bobby_channel: None,
        }
    }

    struct Rig {
        brain: Brain<FakeLlm>,
        sent: Sent,
        llm: FakeLlm,
        path: PathBuf,
    }

    fn rig(answers: Vec<Value>, answer: impl Fn(&Cap) -> Result<Value, String> + Send + 'static) -> Rig {
        let (io, sent) = fake_client(answer);
        let llm = FakeLlm::new(answers);
        let path = crate::store::temp_dir("brain").join("conversations.jsonl");
        Rig { brain: Brain::new(Store::temp(), llm.clone(), io, &path), sent, llm, path }
    }

    fn heard(text: &str, user: &str) -> Heard {
        Heard { text: text.into(), lang: "fr".into(), score: Some(0.9), user: user.into(), user_name: if user == ADMIN { "WakandeX".into() } else { user.into() }, stt_ms: Some(500), ..Default::default() }
    }

    impl Rig {
        async fn hear(&self, text: &str, user: &str) {
            self.brain.hear(heard(text, user), &context()).await;
            tokio::time::sleep(std::time::Duration::from_millis(20)).await; // the fake client records on its own task
        }
        fn journal(&self) -> Vec<Value> {
            std::fs::read_to_string(&self.path).unwrap_or_default().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
        }
        fn msgs(&self) -> Vec<BrainMsg> {
            self.sent.lock().unwrap().clone()
        }
        fn said(&self) -> Vec<String> {
            self.msgs().into_iter().filter_map(|m| if let BrainMsg::Say { text, .. } = m { Some(text) } else { None }).collect()
        }
        fn signals(&self) -> Vec<SignalKind> {
            self.msgs().into_iter().filter_map(|m| if let BrainMsg::Signal { kind } = m { Some(kind) } else { None }).collect()
        }
        fn last(&self) -> Value {
            self.journal().pop().unwrap()
        }
    }

    fn keys(v: &Value) -> Vec<&str> {
        v.as_object().unwrap().keys().map(String::as_str).collect()
    }

    fn ok(_: &Cap) -> Result<Value, String> {
        Ok(Value::Null)
    }

    #[tokio::test]
    async fn ignored_sentence_is_journaled() {
        let r = rig(vec![], ok);
        r.hear("on se retrouve à huit heures", ADMIN).await;
        assert!(r.msgs().is_empty());
        let j = r.last();
        assert_eq!(j["outcome"], "ignored");
        assert_eq!(keys(&j), ["at", "who", "id", "lang", "wakeScore", "heard", "outcome"]);
        assert_eq!(j["wakeScore"], 0.9);
    }

    #[tokio::test]
    async fn bobby_alone_then_the_order() {
        let r = rig(vec![], ok);
        r.hear("Bobby", ADMIN).await;
        assert_eq!(r.signals(), [SignalKind::Beep]);
        assert!(r.brain.expecting(ADMIN));
        assert_eq!(r.last()["outcome"], "waiting");
        r.hear("mute Kévin", ADMIN).await; // no "Bobby" needed
        assert!(!r.brain.expecting(ADMIN));
        assert_eq!(caps(&r.sent), vec![Cap::Mute { users: vec!["kev".into()], on: true }]);
        assert_eq!(r.signals(), [SignalKind::Beep, SignalKind::Ack]);
        assert!(r.said().is_empty()); // an echo: the ack says it all
        assert_eq!(r.last()["outcome"], "done");
    }

    #[tokio::test]
    async fn confirm_accepted_and_refused() {
        let r = rig(vec![], ok);
        r.hear("Bobby, kick Kévin", ADMIN).await;
        assert_eq!(r.last()["outcome"], "confirm_asked");
        assert_eq!(r.said(), ["Je déconnecte Kévin. Tu confirmes ?"]);
        assert!(caps(&r.sent).is_empty());
        r.hear("Oui", ADMIN).await;
        assert_eq!(caps(&r.sent), vec![Cap::Disconnect { users: vec!["kev".into()] }]);
        let j = r.last();
        assert_eq!(j["outcome"], "confirmed");
        assert_eq!(j["confirm"], true);
        // refused
        let r = rig(vec![], ok);
        r.hear("Bobby, kick Kévin", ADMIN).await;
        r.hear("non", ADMIN).await;
        assert!(caps(&r.sent).is_empty());
        assert_eq!(r.last()["outcome"], "cancelled");
        assert_eq!(r.said().last().unwrap(), "D'accord, j'annule.");
    }

    #[tokio::test]
    async fn offer_accepted() {
        let plan = json!({"analyse": {"genre": "capacite", "faisable": true}, "actions": [{"type": "mute", "who": ["Kévin"]}], "say": ""});
        let r = rig(vec![plan], ok);
        r.hear("Bobby, est-ce que tu serais capable de couper le micro de Kévin ?", ADMIN).await;
        assert_eq!(r.last()["outcome"], "offered");
        assert_eq!(r.said(), ["Oui, je peux. Tu veux que je le fasse ?"]);
        assert!(caps(&r.sent).is_empty());
        r.hear("oui", ADMIN).await;
        assert_eq!(caps(&r.sent), vec![Cap::Mute { users: vec!["kev".into()], on: true }]);
        assert_eq!(r.last()["outcome"], "confirmed");
    }

    #[tokio::test]
    async fn undo_after_mute_sends_the_old_values() {
        let r = rig(vec![json!({"actions": [{"type": "undo"}], "say": ""})], |c| match c {
            Cap::Mute { .. } => Ok(json!({"prev": {"kev": false}})),
            _ => Ok(Value::Null),
        });
        r.hear("Bobby, mute Kévin", ADMIN).await;
        r.hear("Bobby, remets comme avant", ADMIN).await;
        assert_eq!(caps(&r.sent), vec![Cap::Mute { users: vec!["kev".into()], on: true }, Cap::Mute { users: vec!["kev".into()], on: false }]);
        assert!(r.said().is_empty()); // "J'annule." is an echo once done
        assert_eq!(r.signals(), [SignalKind::Ack, SignalKind::Ack]);
        // nothing left to undo
        r.hear("Bobby, remets comme avant", ADMIN).await;
        assert_eq!(r.said(), ["Il n'y a rien à annuler."]);
    }

    #[tokio::test]
    async fn stop_while_speaking_hushes_only_for_the_right_person() {
        let r = rig(vec![], ok);
        r.brain.on_client(&ClientMsg::Speaking { user: Some("zyk".into()) });
        r.hear("stop", "kev").await;
        assert_eq!(r.last()["outcome"], "stop_ignored");
        assert_eq!(r.last()["answering"], "zyk");
        assert!(!r.msgs().contains(&BrainMsg::Hush));
        r.hear("stop", "zyk").await;
        assert_eq!(r.last()["outcome"], "interrupted");
        assert!(r.msgs().contains(&BrainMsg::Hush));
        // an admin needs "Bobby, stop"
        let n = r.msgs().len();
        r.hear("stop", ADMIN).await;
        assert_eq!(r.msgs().len(), n);
        r.hear("Bobby, stop", ADMIN).await;
        assert_eq!(r.msgs().len(), n + 1);
        // no more speaking: "stop" is an ordinary sentence again
        r.brain.on_client(&ClientMsg::Speaking { user: None });
        r.hear("stop", "zyk").await;
        assert_eq!(r.last()["outcome"], "ignored");
    }

    #[tokio::test]
    async fn written_mode_posts_to_the_asked_channel() {
        let r = rig(vec![json!({"analyse": {"genre": "discussion", "faisable": true}, "actions": [], "say": "Bonjour tout le monde !"})], |c| match c {
            Cap::Post { channel, .. } => Ok(json!({"channel": channel})),
            _ => Ok(Value::Null),
        });
        r.hear("Bobby, dis bonjour par écrit dans annonces", ADMIN).await;
        assert_eq!(caps(&r.sent), vec![Cap::Post { channel: Some("annonces".into()), text: Some("Bonjour tout le monde !".into()), embeds: vec![] }]);
        let j = r.last();
        assert_eq!(j["written"], "annonces");
        assert_eq!(j["outcome"], "answered");
        assert_eq!(r.said(), ["Je t'envoie ça dans annonces."]);
    }

    #[tokio::test]
    async fn llm_down_is_said_and_journaled() {
        let r = rig(vec![], ok);
        *r.llm.answers.lock().unwrap() = vec![Err(InferError::new(InferErrorKind::Down, "refused"))];
        r.hear("Bobby, raconte-moi une histoire de dragons", ADMIN).await;
        let j = r.last();
        assert_eq!(j["outcome"], "llm_down");
        assert_eq!(j["error"], "Mon cerveau ne répond pas.");
        assert_eq!(r.said(), ["Mon cerveau ne répond pas."]);
        assert!(!r.signals().contains(&SignalKind::Nope)); // said, not failed
        // same keys, same order as a real journal line of that outcome (_logs/conversations.jsonl)
        let real: Value = serde_json::from_str(r#"{"at":"2026-11-13T10:34:13.309Z","who":"WakandeX","id":"261531632589078528","lang":"fr","wakeScore":0.533,"heard":"Bobby, est-ce que tu peux jouer de la musique relaxante, s'il te plaît ?","role":"super","query":"est-ce que tu peux jouer de la musique relaxante, s'il te plaît ?","outcome":"llm_down","error":"Mon cerveau ne répond pas.","reply":"Mon cerveau ne répond pas.","timing":{"stt":9224,"ack":null,"total":312877}}"#).unwrap();
        assert_eq!(keys(&j), keys(&real));
        assert_eq!(keys(&j["timing"]), keys(&real["timing"]));
    }

    #[tokio::test]
    async fn journal_key_order_of_a_shortcut_and_a_llm_line() {
        let r = rig(vec![json!({"actions": [], "say": "Je ne sais pas."})], ok);
        r.hear("Bobby, mute Kévin", ADMIN).await;
        assert_eq!(keys(&r.last()), ["at", "who", "id", "lang", "wakeScore", "heard", "role", "query", "intent", "plan", "outcome", "reply", "timing"]);
        assert_eq!(keys(&r.last()["timing"]), ["stt", "ack", "shortcut", "action", "total"]);
        assert_eq!(r.last()["intent"], json!({"via": "shortcut", "actions": [{"type": "mute", "who": ["kévin"]}]}));
        r.hear("Bobby, raconte-moi un truc", ADMIN).await;
        assert_eq!(keys(&r.last()), ["at", "who", "id", "lang", "wakeScore", "heard", "role", "query", "intent", "outcome", "reply", "timing"]);
        assert_eq!(keys(&r.last()["timing"]), ["stt", "ack", "llm", "total"]);
        assert_eq!(r.last()["intent"]["via"], "llm");
    }

    #[tokio::test]
    async fn not_allowed_is_told_every_30_seconds() {
        let r = rig(vec![], ok);
        r.hear("Bobby, mute Kévin", "u9").await;
        r.hear("Bobby, mute Kévin", "u9").await;
        assert_eq!(r.said(), ["Je n'ai pas le droit de te répondre."]);
        assert_eq!(r.signals(), [SignalKind::Nope, SignalKind::Nope]);
        assert_eq!(keys(&r.last()), ["at", "who", "id", "lang", "wakeScore", "heard", "role", "outcome"]);
    }
}
