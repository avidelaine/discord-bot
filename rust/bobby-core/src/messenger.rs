//! "Va dire à Marie que le repas est prêt": Bobby carries a message to someone and brings the answer back.
//! Shortcut only (no LLM). The target in voice gets the message spoken in their channel; the others get a DM.
//! The flow runs in its own task, so `Brain::hear` returns at once; the target's sentences reach it through
//! two small waiter maps filled by `Brain::hear` (voice) and `Brain::on_client` (DMs).
use crate::io::Io;
use crate::resolve::pick;
use crate::store::Store;
use crate::types::Person;
use crate::util::{caps, hit, re_i, replace1};
use bobby_proto::msg::{Cap, Context, Voice};
use fancy_regex::Regex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;

/// How long Bobby listens, in ms. One struct so tests can shrink them.
#[derive(Debug, Clone, Copy)]
pub struct Timings {
    /// To answer "Tu veux répondre ?" (added to the time the question takes to say).
    pub ask_ms: u64,
    /// To say the reply itself.
    pub reply_ms: u64,
    /// To answer a direct message.
    pub dm_ms: u64,
    /// Speech estimate per character of the question Bobby says before listening.
    pub ms_per_char: u64,
}

impl Default for Timings {
    fn default() -> Self {
        Timings { ask_ms: 8_000, reply_ms: 20_000, dm_ms: 600_000, ms_per_char: 70 }
    }
}

type Waiters = Arc<Mutex<HashMap<String, oneshot::Sender<String>>>>;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the brain keeps for the feature: who is awaited, whether a messenger is out, the timings.
#[derive(Default)]
pub struct Messenger {
    voice: Waiters,
    dm: Waiters,
    busy: Arc<AtomicBool>,
    pub timings: Timings,
}

/// What `start` decided.
#[derive(Debug, PartialEq)]
pub enum Started {
    /// Nothing runs: say this to the sender.
    Refused(String),
    /// The flow runs in its own task and speaks for itself.
    Launched,
}

impl Messenger {
    /// The target's sentence, if the flow waits for it (no wake word needed). True = consumed.
    pub fn deliver_voice(&self, user: &str, text: &str) -> bool {
        lock(&self.voice).remove(user).is_some_and(|tx| tx.send(text.to_string()).is_ok())
    }

    /// The target's direct message, if the flow waits for it. True = consumed.
    pub fn deliver_dm(&self, user: &str, text: &str) -> bool {
        lock(&self.dm).remove(user).is_some_and(|tx| tx.send(text.to_string()).is_ok())
    }

    /// Finds the person, then launches the flow. Needs a tokio runtime.
    pub fn start(&self, io: &Arc<Io>, store: &Store, c: &Context, sender: &str, who: &str, msg: &str) -> Started {
        let target = match find(c, store, sender, who) {
            Ok(p) => p,
            Err(line) => return Started::Refused(line),
        };
        if self.busy.swap(true, Ordering::SeqCst) {
            return Started::Refused("Je suis déjà en train de porter un message.".into());
        }
        let me = c.members.iter().find(|m| m.id == sender);
        let flow = Flow {
            io: io.clone(),
            voice: self.voice.clone(),
            dm: self.dm.clone(),
            t: self.timings,
            _busy: Busy(self.busy.clone()),
            sender: Peer { id: sender.into(), name: me.map_or_else(|| sender.to_string(), |m| m.name.clone()), chan: me.and_then(|m| m.channel.clone()) },
            target: Peer { id: target.id, name: target.name, chan: target.channel },
            msg: msg.into(),
        };
        tokio::spawn(flow.run());
        Started::Launched
    }
}

/// Clears the "one messenger at a time" flag when the flow ends, however it ends.
struct Busy(Arc<AtomicBool>);

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

// ---------- Trigger ----------

/// Where the name ends and the message starts: "que", ":" or ",".
const SEP: &str = r"(?:\s*[:,]\s*|\s+que\s+|\s+qu['’]\s*)";

static TRIGGERS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        // va dire à X que M · dis à X que M
        format!(r"^(?:va\s+)?(?:dire|dis)\s+[àa]\s+(?P<who>.+?){SEP}(?P<m>.+)$"),
        // va donner ce message à X : M · donne un message à X, M
        format!(r"^(?:va\s+)?(?:donner|donne)\s+(?:(?:ce|un|le)\s+)?message\s+[àa]\s+(?P<who>.+?){SEP}(?P<m>.+)$"),
        // transmets à X que M · transmets à X : M
        format!(r"^(?:va\s+)?(?:transmets?|transmettre)\s+[àa]\s+(?P<who>.+?){SEP}(?P<m>.+)$"),
        // préviens X que M
        format!(r"^(?:va\s+)?(?:pr[ée]viens|pr[ée]venir)\s+(?P<who>.+?){SEP}(?P<m>.+)$"),
        // tell X that M · tell X: M
        r"^(?:go\s+)?tell\s+(?P<who>.+?)(?:\s*[:,]\s*|\s+that\s+)(?P<m>.+)$".to_string(),
        // go tell X M (the name is one word: nothing marks where it ends)
        r"^go\s+tell\s+(?P<who>\S+)\s+(?P<m>.+)$".to_string(),
    ]
    .iter()
    .map(|p| re_i(p))
    .collect()
});

static TRIM: LazyLock<Regex> = LazyLock::new(|| re_i(r"[\s.!?…]+$"));

/// "va dire à Marie que le repas est prêt" -> ("Marie", "le repas est prêt"). The query has no wake word.
pub fn parse(query: &str) -> Option<(String, String)> {
    let q = query.trim();
    for r in TRIGGERS.iter() {
        let Some(c) = caps(r, q) else { continue };
        let (who, m) = (c["who"].trim().to_string(), replace1(&TRIM, c["m"].trim(), ""));
        let oneself = ["moi", "me", "toi", "you", "us", "nous"];
        if who.is_empty() || m.trim().is_empty() || oneself.contains(&who.to_lowercase().as_str()) {
            return None;
        }
        return Some((who, m));
    }
    None
}

// ---------- The person ----------

/// Everyone but bots and the sender, in voice or not (`Person::channel` holds the voice channel ID here).
fn people(c: &Context, store: &Store, sender: &str) -> Vec<Person> {
    let nicknames = store.load_or("aliases", json!({})); // memberId -> [nickname], as in `context::build`
    c.members
        .iter()
        .filter(|m| !m.bot && m.id != sender)
        .map(|m| {
            let n: Vec<String> = nicknames.get(&m.id).and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
            let aliases = [Some(m.username.clone()), m.global_name.clone()].into_iter().flatten().chain(n.iter().cloned()).filter(|s| !s.is_empty()).collect();
            Person { id: m.id.clone(), name: m.name.clone(), channel: m.channel.clone(), nicknames: n, aliases, roles: m.roles.clone() }
        })
        .collect()
}

/// The member `who` means (same matching as the rest of the brain: `pick`), or the line to say.
pub fn find(c: &Context, store: &Store, sender: &str, who: &str) -> Result<Person, String> {
    let all = people(c, store, sender);
    pick(who, &all, "person").cloned().map_err(|e| match e.fr.split_once("tu veux dire ") {
        Some((_, pair)) => format!("Lequel : {pair}"), // ponytail: reads the pair out of `pick`'s sentence instead of a typed error
        None => format!("Je ne trouve pas {who}."),
    })
}

// ---------- The answer ----------

#[derive(Debug, PartialEq)]
pub enum Answer {
    No,
    /// "oui": wants to answer, nothing said yet.
    Yes,
    Reply(String),
}

static REFUSAL: LazyLock<Regex> = LazyLock::new(|| re_i(r"^(?:non|nan|no|nope|pas maintenant|laisse tomber|ça ira|ca ira|rien)\b"));
static YES_ONLY: LazyLock<Regex> = LazyLock::new(|| re_i(r"^(?:oui|ouais|yes|yeah|yep|ok|okay|d'accord|vas-y|je veux bien)[\s,.!?…]*$"));
static LEAD_YES: LazyLock<Regex> = LazyLock::new(|| re_i(r"^(?:oui|ouais|yes|yeah)[\s,.!?:;…-]*(?:dis[- ]lui\s+(?:que\s+|qu['’]\s*)?)?"));

/// What the target said to "Tu veux répondre ?".
pub fn classify(text: &str) -> Answer {
    let t = text.trim();
    if hit(&REFUSAL, t) {
        return Answer::No;
    }
    if hit(&YES_ONLY, t) {
        return Answer::Yes;
    }
    match replace1(&LEAD_YES, t, "").trim() {
        "" => Answer::Yes,
        r => Answer::Reply(r.to_string()),
    }
}

// ---------- The flow ----------

struct Peer {
    id: String,
    name: String,
    /// Voice channel ID, if in voice.
    chan: Option<String>,
}

/// How the exchange ended.
enum End {
    Silent,
    Refused,
    Reply(String),
}

struct Flow {
    io: Arc<Io>,
    voice: Waiters,
    dm: Waiters,
    t: Timings,
    _busy: Busy,
    sender: Peer,
    target: Peer,
    msg: String,
}

impl Flow {
    fn tell(&self, to: &Peer, text: &str) {
        self.io.say(text, "fr", Voice::Bobby, Some(&to.id));
    }

    /// Registers before anything is said, so a fast answer is never lost.
    fn arm(w: &Waiters, user: &str) -> oneshot::Receiver<String> {
        let (tx, rx) = oneshot::channel();
        lock(w).insert(user.into(), tx);
        rx
    }

    async fn listen(w: &Waiters, user: &str, rx: oneshot::Receiver<String>, ms: u64) -> Option<String> {
        match tokio::time::timeout(Duration::from_millis(ms), rx).await {
            Ok(Ok(text)) => Some(text),
            _ => {
                lock(w).remove(user); // ponytail: an answer that lands after the timeout is dropped
                None
            }
        }
    }

    async fn run(self) {
        match self.target.chan.clone() {
            Some(chan) => self.by_voice(&chan).await,
            None => self.by_dm().await,
        }
    }

    async fn by_voice(&self, chan: &str) {
        let (io, t, to) = (&self.io, self.t, &self.target);
        self.tell(&self.sender, "J'y vais.");
        let moved = self.sender.chan.as_deref() != Some(chan);
        let mut paused = false;
        if moved {
            paused = io.run(Cap::MusicPause).await.is_ok(); // errors ignored: maybe nothing plays
            if io.run(Cap::Join { channel: chan.into() }).await.is_err() {
                self.go_back(false, paused).await;
                return self.tell(&self.sender, &format!("Je n'ai pas pu rejoindre {}.", to.name));
            }
        }
        let line = format!("{} te fait passer un message : {}. Tu veux répondre ?", self.sender.name, self.msg);
        let wait = t.ask_ms + line.chars().count() as u64 * t.ms_per_char;
        let rx = Self::arm(&self.voice, &to.id);
        self.tell(to, &line);
        io.expect(&to.id, wait as u32);
        let end = match Self::listen(&self.voice, &to.id, rx, wait).await.map(|a| classify(&a)) {
            None => End::Silent,
            Some(Answer::No) => End::Refused,
            Some(Answer::Reply(r)) => End::Reply(r),
            Some(Answer::Yes) => {
                let rx = Self::arm(&self.voice, &to.id);
                self.tell(to, "Je t'écoute.");
                io.expect(&to.id, t.reply_ms as u32);
                Self::listen(&self.voice, &to.id, rx, t.reply_ms).await.map_or(End::Silent, End::Reply)
            }
        };
        if matches!(end, End::Reply(_)) {
            self.tell(to, "C'est noté, je lui transmets.");
        }
        self.go_back(moved, paused).await;
        let said = match end {
            End::Reply(r) => format!("{} répond : {r}", to.name),
            End::Refused => format!("{} ne veut pas répondre.", to.name),
            End::Silent => format!("{} n'a pas répondu.", to.name),
        };
        self.tell(&self.sender, &said);
    }

    /// Bobby returns to the sender (or leaves), then the music goes on if Bobby stopped it.
    async fn go_back(&self, moved: bool, paused: bool) {
        if moved {
            let _ = match &self.sender.chan {
                Some(ch) => self.io.run(Cap::Join { channel: ch.clone() }).await,
                None => self.io.run(Cap::Leave).await,
            };
        }
        if paused {
            let _ = self.io.run(Cap::MusicResume).await;
        }
    }

    async fn by_dm(&self) {
        let (io, to) = (&self.io, &self.target);
        let rx = Self::arm(&self.dm, &to.id);
        let text = format!("Message de {} : {}\nRéponds-moi ici si tu veux que je lui transmette ta réponse.", self.sender.name, self.msg);
        if io.run(Cap::Dm { user: to.id.clone(), text }).await.is_err() {
            lock(&self.dm).remove(&to.id);
            return self.tell(&self.sender, "Je n'ai pas pu lui écrire.");
        }
        self.tell(&self.sender, &format!("{} n'est pas en vocal, je lui ai écrit en privé.", to.name));
        let said = match Self::listen(&self.dm, &to.id, rx, self.t.dm_ms).await {
            Some(r) => format!("{} répond : {r}", to.name),
            None => format!("{} n'a pas répondu.", to.name),
        };
        self.tell(&self.sender, &said);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::{Brain, Heard};
    use crate::testing::{fake_client, FakeLlm, Sent};
    use bobby_proto::msg::{BrainMsg, Channel, ClientMsg, Member};

    const ADMIN: &str = "261531632589078528"; // the default super admin

    fn member(id: &str, name: &str, chan: Option<&str>) -> Member {
        Member { id: id.into(), name: name.into(), username: format!("{}_u", name.to_lowercase()), global_name: None, channel: chan.map(Into::into), roles: vec![], bot: false }
    }

    fn context() -> Context {
        let ch = |n: &str| Channel { id: n.into(), name: n.into() };
        Context {
            speaker: ADMIN.into(),
            members: vec![
                member(ADMIN, "WakandeX", Some("Général")),
                member("marie", "Marie", Some("Gaming")),
                member("lucas", "Lucas", Some("Gaming")),
                member("paul", "Paul", None),
                Member { bot: true, ..member("bot", "Robot", Some("Gaming")) },
            ],
            voice_channels: vec![ch("Général"), ch("Gaming")],
            text_channels: vec![ch("général")],
            bobby_channel: None,
        }
    }

    #[test]
    fn parse_french_and_english() {
        let ok = |q: &str, who: &str, m: &str| assert_eq!(parse(q), Some((who.to_string(), m.to_string())), "{q}");
        ok("va dire à Marie que le repas est prêt", "Marie", "le repas est prêt");
        ok("Va dire à Marie que le repas est prêt.", "Marie", "le repas est prêt");
        ok("dis à Lucas que j'arrive", "Lucas", "j'arrive");
        ok("dis à Lucas qu'il peut venir", "Lucas", "il peut venir");
        ok("va donner ce message à Marie : le repas est prêt", "Marie", "le repas est prêt");
        ok("va donner un message à Marie, le repas est prêt", "Marie", "le repas est prêt");
        ok("transmets à Lucas que c'est l'heure", "Lucas", "c'est l'heure");
        ok("transmets à Prime Ilias : on commence", "Prime Ilias", "on commence");
        ok("préviens Marie que je suis en retard", "Marie", "je suis en retard");
        ok("tell Marie that dinner is ready", "Marie", "dinner is ready");
        ok("go tell Marie dinner is ready", "Marie", "dinner is ready");
        for q in ["dis à Marie", "dis-moi que ça va", "dis que le repas est prêt", "va dire bonjour", "tell me that it works", "dis à moi que oui", "mets de la musique", "préviens Marie", "va dans Gaming"] {
            assert_eq!(parse(q), None, "{q}");
        }
    }

    #[test]
    fn names_found_nickname_ambiguous_nobody() {
        let store = Store::temp();
        store.save("aliases", &json!({"lucas": ["le Grec"]})).unwrap();
        let mut c = context();
        assert_eq!(find(&c, &store, ADMIN, "marie").unwrap().id, "marie");
        assert_eq!(find(&c, &store, ADMIN, "le Grec").unwrap().id, "lucas");
        assert_eq!(find(&c, &store, ADMIN, "Paul").unwrap().channel, None); // not in voice is fine
        assert_eq!(find(&c, &store, ADMIN, "Zorglub"), Err("Je ne trouve pas Zorglub.".into()));
        assert_eq!(find(&c, &store, ADMIN, "WakandeX"), Err("Je ne trouve pas WakandeX.".into())); // not the sender
        assert_eq!(find(&c, &store, ADMIN, "Robot"), Err("Je ne trouve pas Robot.".into())); // not a bot
        c.members.push(member("marion", "Marion", Some("Gaming")));
        assert_eq!(find(&c, &store, ADMIN, "Mari"), Err("Lequel : Marie ou Marion ?".into()));
    }

    #[test]
    fn classify_answers() {
        assert_eq!(classify("non"), Answer::No);
        assert_eq!(classify("Non merci."), Answer::No);
        assert_eq!(classify("Oui."), Answer::Yes);
        assert_eq!(classify("oui, dis-lui que j'arrive"), Answer::Reply("j'arrive".into()));
        assert_eq!(classify("Oui, je viens dans cinq minutes"), Answer::Reply("je viens dans cinq minutes".into()));
        assert_eq!(classify("je suis en route"), Answer::Reply("je suis en route".into()));
    }

    struct Rig {
        brain: Brain<FakeLlm>,
        sent: Sent,
    }

    fn rig(t: Timings, answer: impl Fn(&Cap) -> Result<Value, String> + Send + 'static) -> Rig {
        let (io, sent) = fake_client(answer);
        let path = crate::store::temp_dir("messenger").join("conversations.jsonl");
        let mut brain = Brain::new(Store::temp(), FakeLlm::new(vec![]), io, path);
        brain.messenger.timings = t;
        Rig { brain, sent }
    }

    fn quick() -> Timings {
        Timings { ask_ms: 2_000, reply_ms: 2_000, dm_ms: 2_000, ms_per_char: 0 }
    }

    impl Rig {
        async fn hear(&self, text: &str, user: &str) {
            let h = Heard { text: text.into(), lang: "fr".into(), user: user.into(), user_name: user.into(), score: Some(0.9), ..Default::default() };
            self.brain.hear(h, &context()).await;
        }

        /// The flow, as a compact trace: "say:to:text", "run:cap", "expect:user".
        fn trace(&self) -> Vec<String> {
            self.sent
                .lock()
                .unwrap()
                .iter()
                .filter_map(|m| match m {
                    BrainMsg::Say { text, to_user, .. } => Some(format!("say:{}:{text}", to_user.clone().unwrap_or_default())),
                    BrainMsg::Run { cap, .. } => Some(format!("run:{cap:?}")),
                    BrainMsg::Expect { user, .. } => Some(format!("expect:{user}")),
                    _ => None,
                })
                .collect()
        }

        /// Waits (at most 3 s) until a trace line contains `part`.
        async fn until(&self, part: &str) {
            for _ in 0..600 {
                if self.trace().iter().any(|l| l.contains(part)) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            panic!("never saw \"{part}\" in {:#?}", self.trace());
        }
    }

    fn ok(_: &Cap) -> Result<Value, String> {
        Ok(Value::Null)
    }

    #[tokio::test]
    async fn voice_flow_end_to_end() {
        let r = rig(quick(), ok);
        r.hear("Bobby, va dire à Marie que le repas est prêt", ADMIN).await;
        r.until("expect:marie").await;
        r.hear("oui, dis-lui que j'arrive", "marie").await; // no wake word: the hook takes it
        r.until("répond :").await;
        assert_eq!(
            r.trace(),
            vec![
                "say:261531632589078528:J'y vais.".to_string(),
                "run:MusicPause".into(),
                "run:Join { channel: \"Gaming\" }".into(),
                "say:marie:WakandeX te fait passer un message : le repas est prêt. Tu veux répondre ?".into(),
                "expect:marie".into(),
                "say:marie:C'est noté, je lui transmets.".into(),
                "run:Join { channel: \"Général\" }".into(),
                "run:MusicResume".into(),
                "say:261531632589078528:Marie répond : j'arrive".into(),
            ]
        );
    }

    #[tokio::test]
    async fn yes_alone_then_the_reply() {
        let r = rig(quick(), ok);
        r.hear("Bobby, dis à Lucas que c'est l'heure", ADMIN).await;
        r.until("expect:lucas").await;
        r.hear("oui", "lucas").await;
        r.until("Je t'écoute").await;
        r.hear("j'arrive dans deux minutes", "lucas").await;
        r.until("répond :").await;
        let t = r.trace();
        assert_eq!(t.iter().filter(|l| l.starts_with("expect:")).count(), 2);
        assert!(t.contains(&"say:lucas:C'est noté, je lui transmets.".to_string()));
        assert_eq!(t.last().unwrap(), "say:261531632589078528:Lucas répond : j'arrive dans deux minutes");
    }

    #[tokio::test]
    async fn voice_timeout_and_one_at_a_time() {
        let r = rig(Timings { ask_ms: 150, ..quick() }, ok);
        r.hear("Bobby, va dire à Marie que le repas est prêt", ADMIN).await;
        r.until("expect:marie").await;
        r.hear("Bobby, dis à Lucas que ça va", ADMIN).await;
        r.until("déjà en train").await;
        r.until("n'a pas répondu").await;
        assert!(!r.trace().iter().any(|l| l.contains("C'est noté")));
        assert!(r.trace().contains(&"run:MusicResume".to_string()));
        // the flag is free again
        r.hear("Bobby, dis à Lucas que ça va", ADMIN).await;
        r.until("expect:lucas").await;
    }

    #[tokio::test]
    async fn refusal_path() {
        let r = rig(quick(), ok);
        r.hear("Bobby, va dire à Marie que le repas est prêt", ADMIN).await;
        r.until("expect:marie").await;
        r.hear("non merci", "marie").await;
        r.until("ne veut pas répondre").await;
        assert!(!r.trace().iter().any(|l| l.contains("C'est noté")));
    }

    #[tokio::test]
    async fn unknown_or_no_rights_say_so() {
        let r = rig(quick(), ok);
        r.hear("Bobby, dis à Zorglub que ça va", ADMIN).await;
        r.until("Je ne trouve pas Zorglub.").await;
        r.hear("Bobby, dis à Lucas que ça va", "passant").await; // no role
        assert!(!r.trace().iter().any(|l| l.contains("expect:")));
    }

    #[tokio::test]
    async fn dm_path_with_a_reply() {
        let r = rig(quick(), ok);
        r.hear("Bobby, dis à Paul que le repas est prêt", ADMIN).await;
        r.until("pas en vocal").await;
        let t = r.trace();
        assert_eq!(t[0], "run:Dm { user: \"paul\", text: \"Message de WakandeX : le repas est prêt\\nRéponds-moi ici si tu veux que je lui transmette ta réponse.\" }");
        assert_eq!(t[1], "say:261531632589078528:Paul n'est pas en vocal, je lui ai écrit en privé.");
        r.brain.on_client(&ClientMsg::Dm { user: "paul".into(), text: "merci, j'arrive".into() });
        r.until("répond :").await;
        assert_eq!(r.trace().last().unwrap(), "say:261531632589078528:Paul répond : merci, j'arrive");
    }

    #[tokio::test]
    async fn dm_path_without_a_reply_and_with_a_failing_cap() {
        let r = rig(Timings { dm_ms: 100, ..quick() }, ok);
        r.hear("Bobby, dis à Paul que le repas est prêt", ADMIN).await;
        r.until("Paul n'a pas répondu.").await;
        assert!(!r.brain.messenger.deliver_dm("paul", "trop tard")); // dropped after the timeout
        let r = rig(quick(), |c| if matches!(c, Cap::Dm { .. }) { Err("closed DMs".into()) } else { Ok(Value::Null) });
        r.hear("Bobby, dis à Paul que le repas est prêt", ADMIN).await;
        r.until("Je n'ai pas pu lui écrire.").await;
    }
}
