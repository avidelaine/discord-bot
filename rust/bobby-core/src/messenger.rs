//! "Va dire à Marie que le repas est prêt": Bobby carries a message to someone and brings the answer back.
//! Shortcut only (no LLM). The target in voice gets the message spoken in their channel; the others get a DM.
//! A loose name match ("Mari" for Marie) is never delivered before the sender says "oui".
//! The flow runs in its own task, so `Brain::hear` returns at once; the answers reach it through two small
//! waiter maps filled by `Brain::hear` (voice) and `Brain::on_client` (DMs, only as a reply to Bobby's message).
//! The flow reads where people are from the latest `Context` the brain cached, never from the order's snapshot.
use crate::io::Io;
use crate::resolve::pick;
use crate::store::Store;
use crate::types::Person;
use crate::util::{caps, hit, norm, re_i, replace1};
use crate::wake::find_wake;
use bobby_proto::msg::{Cap, Context, Voice};
use fancy_regex::Regex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::task::AbortHandle;

/// How long Bobby listens, in ms. One struct so tests can shrink them.
#[derive(Debug, Clone, Copy)]
pub struct Timings {
    /// For the sender's "oui" after a loose name match.
    pub confirm_ms: u64,
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
        Timings { confirm_ms: 10_000, ask_ms: 8_000, reply_ms: 20_000, dm_ms: 600_000, ms_per_char: 70 }
    }
}

type Waiters = Arc<Mutex<HashMap<String, oneshot::Sender<String>>>>;

/// A messenger waiting for the reply to its DM (keyed by messenger id in `DmWaiters`).
struct DmWait {
    target: String,
    /// The id of Bobby's DM, known once Discord answered. Only a reply to it counts.
    msg_id: Option<String>,
    tx: oneshot::Sender<String>,
}

type DmWaiters = Arc<Mutex<HashMap<u64, DmWait>>>;
type Snapshot = Arc<Mutex<Option<Context>>>;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the brain keeps for the feature: who is awaited, whether a messenger is out, the timings.
#[derive(Default)]
pub struct Messenger {
    voice: Waiters,
    dm: DmWaiters,
    busy: Arc<AtomicBool>,
    next: AtomicU64,
    tasks: Mutex<Vec<AbortHandle>>,
    /// The latest server snapshot, shared with the flows.
    ctx: Snapshot,
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
    /// A sentence that calls Bobby ("Bobby, stop") is a command, not the answer: it goes the normal way.
    pub fn deliver_voice(&self, user: &str, text: &str) -> bool {
        if find_wake(text).is_some() {
            return false;
        }
        lock(&self.voice).remove(user).is_some_and(|tx| tx.send(text.to_string()).is_ok())
    }

    /// A direct message from `user`. It is the answer only if it replies to the DM Bobby sent them. True = consumed.
    pub fn deliver_dm(&self, user: &str, text: &str, reply_to: Option<&str>) -> bool {
        let Some(reply_to) = reply_to else { return false };
        let mut dm = lock(&self.dm);
        let Some(id) = dm.iter().find(|(_, w)| w.target == user && w.msg_id.as_deref() == Some(reply_to)).map(|(id, _)| *id) else { return false };
        dm.remove(&id).is_some_and(|w| w.tx.send(text.to_string()).is_ok())
    }

    /// True while a flow runs or any waiter (voice or DM) is alive: Bobby is carrying a message.
    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst) || !lock(&self.voice).is_empty() || !lock(&self.dm).is_empty()
    }

    /// True if a flow waits for this person's spoken answer (the sentence needs no "Bobby").
    pub fn has_voice_waiter(&self, user: &str) -> bool {
        lock(&self.voice).contains_key(user)
    }

    /// The latest server snapshot (people's voice channels), as the host's Discord client sent it.
    pub fn set_context(&self, c: &Context) {
        *lock(&self.ctx) = Some(c.clone());
    }

    /// Stop or restart: no waiter survives, no messenger is out, the flows end without a word.
    pub fn clear(&self) {
        for h in lock(&self.tasks).drain(..) {
            h.abort();
        }
        lock(&self.voice).clear();
        lock(&self.dm).clear();
        self.busy.store(false, Ordering::SeqCst);
    }

    /// Finds the person, then launches the flow. Needs a tokio runtime.
    pub fn start(&self, io: &Arc<Io>, store: &Store, c: &Context, sender: &str, who: &str, msg: &str) -> Started {
        let target = match find(c, store, sender, who) {
            Ok(p) => p,
            Err(line) => return Started::Refused(line),
        };
        if lock(&self.dm).values().any(|w| w.target == target.id) {
            return Started::Refused(format!("J'attends déjà une réponse de {}.", target.name));
        }
        if self.busy.swap(true, Ordering::SeqCst) {
            return Started::Refused("Je suis déjà en train de porter un message.".into());
        }
        lock(&self.ctx).get_or_insert_with(|| c.clone()); // seeded once; `set_context` keeps it fresh
        let me = c.members.iter().find(|m| m.id == sender);
        let flow = Flow {
            io: io.clone(),
            voice: self.voice.clone(),
            dm: self.dm.clone(),
            ctx: self.ctx.clone(),
            id: self.next.fetch_add(1, Ordering::Relaxed),
            t: self.timings,
            busy: Mutex::new(Some(Busy(self.busy.clone()))),
            sender: Peer { id: sender.into(), name: me.map_or_else(|| sender.to_string(), |m| m.name.clone()) },
            confirm: !is_exact(who, &target),
            target: Peer { id: target.id, name: target.name },
            msg: msg.into(),
            bobby: c.bobby_channel.clone(),
        };
        let h = tokio::spawn(flow.run());
        let mut tasks = lock(&self.tasks);
        tasks.retain(|t| !t.is_finished());
        tasks.push(h.abort_handle());
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

/// True when `who` is, ignoring case and accents, the person's display name, username, global name or a nickname.
pub fn is_exact(who: &str, p: &Person) -> bool {
    let w = norm(who);
    !w.is_empty() && std::iter::once(&p.name).chain(p.aliases.iter()).any(|n| norm(n) == w)
}

/// The member `who` means (same matching as the rest of the brain: `pick`), or the line to say.
/// Two exact matches are an ambiguity, never a guess.
pub fn find(c: &Context, store: &Store, sender: &str, who: &str) -> Result<Person, String> {
    let all = people(c, store, sender);
    let exact: Vec<&Person> = all.iter().filter(|p| is_exact(who, p)).collect();
    match exact.as_slice() {
        [p] => return Ok((*p).clone()),
        [a, b, ..] => return Err(format!("Lequel : {} ou {} ?", a.name, b.name)),
        [] => {}
    }
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
}

/// How the exchange ended.
enum End {
    Silent,
    Refused,
    /// The target left the voice channel before answering.
    Left,
    Reply(String),
}

struct Flow {
    io: Arc<Io>,
    voice: Waiters,
    dm: DmWaiters,
    /// The latest server snapshot: where people are now, not when the order was given.
    ctx: Snapshot,
    id: u64,
    t: Timings,
    /// Held for a voice flow only; a DM wait lets go of it (see `by_dm`).
    busy: Mutex<Option<Busy>>,
    sender: Peer,
    target: Peer,
    msg: String,
    /// The name matched loosely: the sender must say "oui" first.
    confirm: bool,
    /// The channel Bobby was in when the order came (None = not connected).
    bobby: Option<String>,
}

impl Flow {
    fn tell(&self, to: &Peer, text: &str) {
        self.io.say(text, "fr", Voice::Bobby, Some(&to.id));
    }

    /// The voice channel a member is in right now.
    fn chan_of(&self, id: &str) -> Option<String> {
        let g = lock(&self.ctx);
        let c = g.as_ref().and_then(|c| c.members.iter().find(|m| m.id == id)).and_then(|m| m.channel.clone());
        c
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
            Ok(Err(_)) => None, // replaced by a newer waiter for the same person: leave that one alone
            Err(_) => {
                lock(w).remove(user); // ponytail: an answer that lands after the timeout is dropped
                None
            }
        }
    }

    /// Says it, then lets the estimated speech time pass so the next step does not cut it off.
    async fn say(&self, to: &Peer, text: &str) {
        self.tell(to, text);
        self.speech(text).await;
    }

    async fn speech(&self, text: &str) {
        let ms = text.chars().count() as u64 * self.t.ms_per_char;
        if ms > 0 {
            tokio::time::sleep(Duration::from_millis(ms)).await;
        }
    }

    async fn run(self) {
        let _flight = self.io.enter(); // a message being carried counts as a request in flight: no deep sleep under it
        if self.confirm && !self.confirmed().await {
            return;
        }
        // Where the target is now decides: voice or DM.
        match self.chan_of(&self.target.id) {
            Some(_) => self.by_voice().await,
            None => self.by_dm().await,
        }
    }

    /// A loose match is read back to the sender, who must say "oui" (no wake word) within `confirm_ms`.
    async fn confirmed(&self) -> bool {
        let s = &self.sender;
        self.say(s, &format!("Je porte à {} : « {} » ? Dis oui pour confirmer.", self.target.name, self.msg)).await;
        let rx = Self::arm(&self.voice, &s.id);
        self.io.expect(&s.id, self.t.confirm_ms as u32);
        let yes = matches!(Self::listen(&self.voice, &s.id, rx, self.t.confirm_ms).await.map(|a| classify(&a)), Some(Answer::Yes));
        if !yes {
            self.tell(s, "D'accord, je ne lui dis rien.");
        }
        yes
    }

    async fn by_voice(&self) {
        let (io, t, to) = (&self.io, self.t, &self.target);
        self.say(&self.sender, "J'y vais.").await;
        // Everything below reads people's channels now, not the values from the start.
        let Some(chan) = self.chan_of(&to.id) else {
            return self.tell(&self.sender, &format!("{} est parti(e) avant que je lui parle.", to.name));
        };
        let sender_chan = self.chan_of(&self.sender.id);
        let home = sender_chan.clone().or_else(|| self.bobby.clone()); // where Bobby was (None = not connected)
        let moved = home.as_deref() != Some(chan.as_str());
        let mut paused = false;
        if moved {
            paused = home.is_some() && io.run(Cap::MusicPause).await.is_ok(); // errors ignored: maybe nothing plays
            if io.run(Cap::Join { channel: chan.clone() }).await.is_err() {
                if paused {
                    let _ = io.run(Cap::MusicResume).await; // Bobby never left
                }
                return self.tell(&self.sender, &format!("Je n'ai pas pu rejoindre {}.", to.name));
            }
        }
        let gone = || self.chan_of(&to.id).as_deref() != Some(chan.as_str());
        let silent = || if gone() { End::Left } else { End::Silent };
        let line = format!("{} te fait passer un message : {}. Tu veux répondre ?", self.sender.name, self.msg);
        // Armed only once the line is said: what the target says while Bobby still talks is not the answer.
        self.say(to, &line).await;
        let end = if gone() {
            End::Left
        } else {
            let rx = Self::arm(&self.voice, &to.id);
            io.expect(&to.id, t.ask_ms as u32);
            match Self::listen(&self.voice, &to.id, rx, t.ask_ms).await.map(|a| classify(&a)) {
                None => silent(),
                Some(Answer::No) => End::Refused,
                Some(Answer::Reply(r)) => End::Reply(r),
                Some(Answer::Yes) => {
                    self.say(to, "Je t'écoute.").await;
                    if gone() {
                        End::Left
                    } else {
                        let rx = Self::arm(&self.voice, &to.id);
                        io.expect(&to.id, t.reply_ms as u32);
                        Self::listen(&self.voice, &to.id, rx, t.reply_ms).await.map_or_else(silent, End::Reply)
                    }
                }
            }
        };
        if matches!(end, End::Reply(_)) {
            self.say(to, "C'est noté, je lui transmets.").await;
        }
        self.go_back(moved, paused, home, sender_chan.is_some()).await;
        let said = match end {
            End::Reply(r) => format!("{} répond : {r}", to.name),
            End::Refused => format!("{} ne veut pas répondre.", to.name),
            End::Silent => format!("{} n'a pas répondu.", to.name),
            End::Left => format!("{} est parti(e) avant de répondre.", to.name),
        };
        self.tell(&self.sender, &said);
    }

    /// Bobby returns to the sender's channel as it is now (never to an empty one), or to where it was if the
    /// sender was not in voice, or leaves if it was nowhere. The music resumes only if Bobby paused it and is
    /// back in the channel it paused in.
    async fn go_back(&self, moved: bool, paused: bool, home: Option<String>, sender_in_voice: bool) {
        if !moved {
            return;
        }
        let back = if sender_in_voice { self.chan_of(&self.sender.id) } else { home.clone() };
        let joined = match &back {
            Some(ch) => self.io.run(Cap::Join { channel: ch.clone() }).await.is_ok(),
            None => {
                let _ = self.io.run(Cap::Leave).await;
                false
            }
        };
        if paused && joined && back == home {
            let _ = self.io.run(Cap::MusicResume).await;
        }
    }

    async fn by_dm(&self) {
        let (io, to) = (&self.io, &self.target);
        let (tx, rx) = oneshot::channel();
        lock(&self.dm).insert(self.id, DmWait { target: to.id.clone(), msg_id: None, tx });
        let text = format!("Message de {} : {}\nPour lui répondre, utilise « Répondre » sur ce message.", self.sender.name, self.msg);
        let sent = io.run(Cap::Dm { user: to.id.clone(), text }).await;
        let msg_id = sent.as_ref().ok().and_then(|v| v.get("message_id")?.as_str().map(String::from));
        match (&sent, msg_id) {
            (Err(_), _) => {
                lock(&self.dm).remove(&self.id);
                return self.tell(&self.sender, "Je n'ai pas pu lui écrire.");
            }
            (Ok(_), None) => {
                lock(&self.dm).remove(&self.id); // no message id: no reply could be matched
                return self.tell(&self.sender, &format!("{} n'est pas en vocal, je lui ai écrit en privé.", to.name));
            }
            (Ok(_), Some(id)) => {
                if let Some(w) = lock(&self.dm).get_mut(&self.id) {
                    w.msg_id = Some(id);
                }
            }
        }
        self.tell(&self.sender, &format!("{} n'est pas en vocal, je lui ai écrit en privé.", to.name));
        lock(&self.busy).take(); // the DM is out: other messengers may go while this one waits
        let said = match tokio::time::timeout(Duration::from_millis(self.t.dm_ms), rx).await {
            Ok(Ok(r)) => format!("{} répond : {r}", to.name),
            Ok(Err(_)) => return, // cleared
            Err(_) => {
                lock(&self.dm).remove(&self.id);
                format!("{} n'a pas répondu.", to.name)
            }
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
        Timings { confirm_ms: 2_000, ask_ms: 2_000, reply_ms: 2_000, dm_ms: 2_000, ms_per_char: 0 }
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

    fn ok(c: &Cap) -> Result<Value, String> {
        Ok(if matches!(c, Cap::Dm { .. }) { json!({"message_id": "m1"}) } else { Value::Null })
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
    async fn deep_sleep_is_refused_while_the_messenger_waits_for_an_answer() {
        let r = rig(quick(), ok);
        assert!(!r.brain.messenger.busy());
        r.hear("Bobby, va dire à Marie que le repas est prêt", ADMIN).await;
        r.until("expect:marie").await;
        assert!(r.brain.messenger.has_voice_waiter("marie"));
        assert!(r.brain.busy());
        r.hear("Bobby, dors", ADMIN).await;
        assert!(!r.sent.lock().unwrap().contains(&BrainMsg::DeepSleep), "asleep under a message");
        r.hear("oui, dis-lui que j'arrive", "marie").await;
        r.until("répond :").await;
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
        r.hear("Bobby, dis à Lucas que ça va", "passant").await; // no role: falls through to the normal path
        assert!(!r.trace().iter().any(|l| l.contains("expect:") || l.contains("forbidden")));
        assert!(!r.trace().iter().any(|l| l.contains("Lucas")));
    }

    #[tokio::test]
    async fn dm_path_with_a_reply() {
        let r = rig(quick(), ok);
        r.hear("Bobby, dis à Paul que le repas est prêt", ADMIN).await;
        r.until("pas en vocal").await;
        let t = r.trace();
        assert_eq!(t[0], "run:Dm { user: \"paul\", text: \"Message de WakandeX : le repas est prêt\\nPour lui répondre, utilise « Répondre » sur ce message.\" }");
        assert_eq!(t[1], "say:261531632589078528:Paul n'est pas en vocal, je lui ai écrit en privé.");
        r.brain.on_client(&ClientMsg::Dm { user: "paul".into(), text: "merci, j'arrive".into(), reply_to: Some("m1".into()) });
        r.until("répond :").await;
        assert_eq!(r.trace().last().unwrap(), "say:261531632589078528:Paul répond : merci, j'arrive");
    }

    #[tokio::test]
    async fn dm_path_without_a_reply_and_with_a_failing_cap() {
        let r = rig(Timings { dm_ms: 100, ..quick() }, ok);
        r.hear("Bobby, dis à Paul que le repas est prêt", ADMIN).await;
        r.until("Paul n'a pas répondu.").await;
        assert!(!r.brain.messenger.deliver_dm("paul", "trop tard", Some("m1"))); // dropped after the timeout
        let r = rig(quick(), |c| if matches!(c, Cap::Dm { .. }) { Err("closed DMs".into()) } else { Ok(Value::Null) });
        r.hear("Bobby, dis à Paul que le repas est prêt", ADMIN).await;
        r.until("Je n'ai pas pu lui écrire.").await;
    }

    #[tokio::test]
    async fn wake_word_from_the_target_is_a_command_not_the_answer() {
        let r = rig(quick(), ok);
        r.hear("Bobby, va dire à Marie que le repas est prêt", ADMIN).await;
        r.until("expect:marie").await;
        assert!(!r.brain.messenger.deliver_voice("marie", "Bobby, stop"));
        assert!(!r.brain.messenger.deliver_voice("marie", "Bobby, coupe la musique"));
        assert!(r.brain.messenger.deliver_voice("marie", "oui")); // the waiter is still there
    }

    #[tokio::test]
    async fn dm_wait_does_not_block_another_messenger() {
        let r = rig(quick(), ok);
        r.hear("Bobby, dis à Paul que le repas est prêt", ADMIN).await;
        r.until("pas en vocal").await;
        r.hear("Bobby, dis à Marie que ça va", ADMIN).await;
        r.until("expect:marie").await;
        assert!(!r.trace().iter().any(|l| l.contains("déjà en train")));
        assert!(r.brain.messenger.deliver_dm("paul", "ok", Some("m1"))); // Paul's waiter was kept
    }

    #[tokio::test]
    async fn speech_before_the_line_is_over_is_not_the_answer() {
        let r = rig(Timings { ms_per_char: 8, ..quick() }, ok); // the line is ~80 chars: ~650 ms
        r.hear("Bobby, va dire à Marie que le repas est prêt", ADMIN).await;
        r.until("Tu veux répondre").await;
        assert!(!r.brain.messenger.deliver_voice("marie", "non merci")); // Bobby is still talking
        r.until("expect:marie").await;
        assert!(r.brain.messenger.deliver_voice("marie", "non merci"));
    }

    // ---------- second round ----------

    fn fresh_ctx(f: impl Fn(&mut Context)) -> Context {
        let mut c = context();
        f(&mut c);
        c
    }

    impl Rig {
        async fn hear_in(&self, text: &str, user: &str, c: &Context) {
            let h = Heard { text: text.into(), lang: "fr".into(), user: user.into(), user_name: user.into(), score: Some(0.9), ..Default::default() };
            self.brain.hear(h, c).await;
        }

        /// The Discord client's new snapshot: `id` is now in `chan`.
        fn move_to(&self, id: &str, chan: Option<&str>) {
            let c = fresh_ctx(|c| c.members.iter_mut().filter(|m| m.id == id).for_each(|m| m.channel = chan.map(Into::into)));
            self.brain.on_client(&ClientMsg::Context(c));
        }

        fn has(&self, part: &str) -> bool {
            self.trace().iter().any(|l| l.contains(part))
        }
    }

    #[test]
    fn exact_names_ignore_case_and_accents() {
        let store = Store::temp();
        store.save("aliases", &json!({"lucas": ["le Grec"]})).unwrap();
        let mut c = context();
        c.members.push(member("zoe", "Zoé", None));
        let exact = |who: &str| is_exact(who, &find(&c, &store, ADMIN, who).unwrap());
        assert!(exact("marie") && exact("MARIE") && exact("zoe") && exact("Zoé") && exact("le grec") && exact("lucas_u"));
        assert!(!exact("Mari") && !exact("Maria"));
    }

    #[tokio::test]
    async fn fuzzy_name_waits_for_a_yes_and_no_cancels() {
        let r = rig(quick(), ok);
        r.hear("Bobby, dis à Maria que le repas est prêt", ADMIN).await; // not exactly "Marie"
        r.until(&format!("expect:{ADMIN}")).await;
        assert_eq!(r.trace()[0], format!("say:{ADMIN}:Je porte à Marie : « le repas est prêt » ? Dis oui pour confirmer."));
        assert!(!r.has("run:")); // nothing delivered yet
        r.hear("non", ADMIN).await;
        r.until("je ne lui dis rien").await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!r.has("run:") && !r.has("te fait passer") && !r.has("expect:marie"));
        // and "oui" lets it go
        r.hear("Bobby, dis à Maria que le repas est prêt", ADMIN).await;
        for _ in 0..600 {
            if r.trace().iter().filter(|l| l.starts_with("expect:")).count() == 2 {
                break; // the second confirmation is armed (the first one is already in the trace)
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        r.hear("oui", ADMIN).await;
        r.until("expect:marie").await;
        assert!(r.has("te fait passer un message : le repas est prêt"));
    }

    #[tokio::test]
    async fn fuzzy_name_without_an_answer_is_not_delivered() {
        let r = rig(Timings { confirm_ms: 100, ..quick() }, ok);
        r.hear("Bobby, dis à Maria que le repas est prêt", ADMIN).await;
        r.until("je ne lui dis rien").await;
        assert!(!r.has("run:") && !r.has("te fait passer"));
    }

    #[tokio::test]
    async fn target_leaves_before_bobby_goes() {
        let r = rig(Timings { ms_per_char: 30, ..quick() }, ok);
        r.hear("Bobby, va dire à Marie que le repas est prêt", ADMIN).await;
        r.until("J'y vais").await;
        r.move_to("marie", None);
        r.until("avant que je lui parle").await;
        assert!(!r.has("run:") && !r.has("te fait passer"));
    }

    #[tokio::test]
    async fn target_leaves_while_bobby_waits() {
        let r = rig(Timings { ask_ms: 300, ..quick() }, ok);
        r.hear("Bobby, va dire à Marie que le repas est prêt", ADMIN).await;
        r.until("expect:marie").await;
        r.move_to("marie", None);
        r.until("est parti(e) avant de répondre").await;
        assert!(r.has("run:Join { channel: \"Général\" }") && r.has("run:MusicResume")); // Bobby went back
        assert!(!r.has("n'a pas répondu"));
    }

    #[tokio::test]
    async fn sender_leaves_so_bobby_does_not_join_an_empty_channel() {
        let r = rig(quick(), ok);
        r.hear("Bobby, va dire à Marie que le repas est prêt", ADMIN).await;
        r.until("expect:marie").await;
        r.move_to(ADMIN, None);
        r.hear("non merci", "marie").await;
        r.until("ne veut pas répondre").await;
        assert!(r.has("run:Leave") && !r.has("Join { channel: \"Général\" }") && !r.has("MusicResume"));
    }

    #[tokio::test]
    async fn join_failure_tells_the_sender() {
        let r = rig(quick(), |c| if matches!(c, Cap::Join { .. }) { Err("no permission".into()) } else { Ok(Value::Null) });
        r.hear("Bobby, va dire à Marie que le repas est prêt", ADMIN).await;
        r.until("Je n'ai pas pu rejoindre Marie.").await;
        assert!(!r.has("te fait passer") && !r.has("expect:marie"));
        assert!(r.has("run:MusicResume")); // Bobby never left: the music goes on
    }

    #[tokio::test]
    async fn sender_not_in_voice_bobby_leaves_and_never_resumes() {
        let r = rig(quick(), ok);
        let c = fresh_ctx(|c| c.members[0].channel = None);
        r.hear_in("Bobby, va dire à Marie que le repas est prêt", ADMIN, &c).await;
        r.until("expect:marie").await;
        r.hear("non merci", "marie").await;
        r.until("ne veut pas répondre").await;
        assert!(r.has("run:Join { channel: \"Gaming\" }") && r.has("run:Leave"));
        assert!(!r.has("MusicResume") && !r.has("MusicPause") && !r.has("Join { channel: \"Général\" }"));
    }

    #[tokio::test]
    async fn dm_answer_only_by_reply_to_bobbys_message() {
        let r = rig(quick(), ok);
        r.hear("Bobby, dis à Paul que le repas est prêt", ADMIN).await;
        r.until("pas en vocal").await;
        assert!(r.has("Pour lui répondre, utilise « Répondre » sur ce message."));
        assert!(!r.brain.messenger.deliver_dm("paul", "salut, tu fais quoi ?", None)); // unrelated DM
        assert!(!r.brain.messenger.deliver_dm("paul", "autre", Some("m9"))); // a reply to something else
        assert!(!r.brain.messenger.deliver_dm("lucas", "merci", Some("m1"))); // not the target
        assert!(!r.has("répond :"));
        r.brain.on_client(&ClientMsg::Dm { user: "paul".into(), text: "merci, j'arrive".into(), reply_to: Some("m1".into()) });
        r.until("Paul répond : merci, j'arrive").await;
    }

    #[tokio::test]
    async fn second_messenger_to_the_same_target_is_refused() {
        let r = rig(quick(), ok);
        r.hear("Bobby, dis à Paul que le repas est prêt", ADMIN).await;
        r.until("pas en vocal").await;
        r.hear("Bobby, dis à Paul que c'est urgent", ADMIN).await;
        r.until("J'attends déjà une réponse de Paul.").await;
        assert_eq!(r.trace().iter().filter(|l| l.starts_with("run:Dm")).count(), 1);
        assert!(r.brain.messenger.deliver_dm("paul", "ok", Some("m1"))); // the first one still waits
    }

    #[tokio::test]
    async fn non_admin_cannot_use_the_dm_path() {
        let r = rig(quick(), ok);
        r.hear("Bobby, dis à Paul que le repas est prêt", "passant").await;
        assert!(!r.has("run:Dm") && !r.has("pas en vocal"));
    }

    #[tokio::test]
    async fn clear_drops_waiters_and_ends_flows_quietly() {
        let r = rig(Timings { ask_ms: 100, ..quick() }, ok);
        r.hear("Bobby, va dire à Marie que le repas est prêt", ADMIN).await;
        r.until("expect:marie").await;
        r.hear("Bobby, dis à Paul que ça va", ADMIN).await;
        r.until("déjà en train").await;
        r.brain.messenger.clear();
        assert!(!r.brain.messenger.deliver_voice("marie", "oui"));
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!r.has("n'a pas répondu")); // the flow ended without a word
        r.hear("Bobby, dis à Lucas que ça va", ADMIN).await; // the busy flag is free
        r.until("expect:lucas").await;
        // a waiting DM is dropped too
        let r = rig(quick(), ok);
        r.hear("Bobby, dis à Paul que le repas est prêt", ADMIN).await;
        r.until("pas en vocal").await;
        r.brain.messenger.clear();
        assert!(!r.brain.messenger.deliver_dm("paul", "merci", Some("m1")));
    }
}
