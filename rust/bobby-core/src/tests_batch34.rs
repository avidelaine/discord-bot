//! Port of unit.test.js batch 3 (follow the group, interruptions), batch 3 written (answers in writing) and the
//! part of batch 4 that goes through hear(), a run or the journal.
//!
//! Counts of JS `assert*` call sites in `batch3()`, `batch3Written()` and `batch4()` (108 in all):
//! - ported here: 54 (batch3 18, batch3Written 11, batch4 25)
//! - already ported elsewhere: 45 (guard.rs, writing.rs, journal.rs, prompt.rs, actions/todo.rs)
//! - moved to R4 (Discord client): 9 (whenAlone 698-700; chunks/embed splitting/messages 788, 790, 793, 796, 798, 801)
//!
//! Already ported elsewhere (JS lines of unit.test.js): 325, 326, 328, 329 (journal.rs); 340, 342, 344, 346, 347, 349,
//! 351, 354, 356, 362, 380, 382, 383, 385, 386 (guard.rs); 388, 398-403 (writing.rs); 390-393 (prompt.rs `unsure`);
//! 405, 406, 416, 417, 420 (actions/todo.rs); 742-749, 751, 752 (writing.rs).
//! Adapted: 372 (the ping's text sanitizing and `allowedMentions` live in the Discord client, R4: only the cap the
//! brain sends is checked) and 799 (`format::table`, kept in the core).
use crate::actions::{self, Env, UndoOp};
use crate::brain::{Brain, Heard};
use crate::guard::decide;
use crate::resolve::{needed, resolve};
use crate::shortcuts::{quick, STOP};
use crate::store::Store;
use crate::testing::{caps, ctx, fake_client, FakeLlm, Sent};
use crate::types::{Chan, Ctx, Role, Step};
use crate::util::hit;
use bobby_proto::msg::{BrainMsg, Cap, Channel, ClientMsg, Context, Member, SignalKind};
use serde_json::{json, Value};
use std::path::PathBuf;

const WAKANDEX: &str = "261531632589078528";
const IISAL: &str = "397478613559148545";

fn sup() -> Ctx {
    Ctx { role: Role::Super, ..ctx() }
}

fn with_voice(c: &Ctx, name: &str) -> Ctx {
    let mut c = c.clone();
    c.voice_channels.push(Chan { id: name.into(), name: name.into() });
    c
}

/// Runs one step against a fake client; returns its undo and the caps it sent.
async fn run_one(s: &mut Step, c: &Ctx, store: &Store, answer: impl Fn(&Cap) -> Result<Value, String> + Send + 'static) -> (Vec<UndoOp>, Vec<Cap>) {
    let (io, sent) = fake_client(answer);
    let env = Env { io: &io, store, ctx: c, llm: &FakeLlm::default(), post_to: None };
    let ops = actions::run(s, &env, "fr").await.unwrap();
    (ops, caps(&sent))
}

fn joins(cs: &[Cap]) -> Vec<String> {
    cs.iter().filter_map(|c| if let Cap::Join { channel } = c { Some(channel.clone()) } else { None }).collect()
}

/// `resolve(quick(sentence), ctx)`, first step.
fn step(sentence: &str, c: &Ctx, store: &Store) -> Step {
    resolve(&quick(sentence).unwrap(), c, store).unwrap().remove(0)
}

fn moved(c: &Cap) -> Result<Value, String> {
    match c {
        Cap::Move { users, .. } => Ok(json!({ "prev": users.iter().map(|u| (u.clone(), json!("Général"))).collect::<serde_json::Map<_, _>>() })),
        Cap::Join { .. } => Ok(json!({ "prev": "Général" })),
        _ => Ok(Value::Null),
    }
}

// ---------- Batch 3: Bobby follows the group, interruptions ----------

#[tokio::test]
async fn batch3_follow_the_group() {
    let store = Store::temp();
    let admin = Ctx { role: Role::Admin, ..ctx() };
    // A move that includes the speaker takes Bobby along; a move of other people leaves it where it is
    let (undo, sent) = run_one(&mut step("mets-moi dans Gaming", &admin, &store), &admin, &store, moved).await;
    assert_eq!(joins(&sent), ["Gaming"]);
    // undo brings Bobby back too
    assert!(undo.contains(&UndoOp::Cap(Cap::Join { channel: "Général".into() })), "{undo:?}");
    let (_, sent) = run_one(&mut step("envoie Lucas en AFK", &admin, &store), &admin, &store, moved).await;
    assert!(joins(&sent).is_empty());
    // everyone includes the speaker
    let (_, sent) = run_one(&mut step("déplace tout le monde dans AFK", &admin, &store), &admin, &store, moved).await;
    assert_eq!(joins(&sent), ["AFK"]);
}

#[test]
fn batch3_come_alone() {
    let store = Store::temp();
    // "viens", "rejoins-nous", "come here": Bobby alone, Prime is enough
    let prime = Ctx { role: Role::Prime, ..ctx() };
    for (sentence, where_) in [("viens dans Gaming", "Gaming"), ("rejoins-nous", "Général"), ("come here", "Général"), ("come to the AFK channel", "AFK"), ("viens ici", "Général")] {
        let p = resolve(&quick(sentence).unwrap(), &prime, &store).unwrap();
        assert_eq!((p[0].kind.as_str(), p[0].channel.as_ref().unwrap().name.as_str(), needed(&p, None)), ("come", where_, Role::Prime), "{sentence}");
    }
    let p = resolve(&quick("mets-moi dans Gaming").unwrap(), &ctx(), &store).unwrap();
    assert_eq!(needed(&p, None), Role::Admin);
    // LLM without channel: here
    assert_eq!(resolve(&[json!({ "type": "come" })], &prime, &store).unwrap()[0].channel.as_ref().unwrap().name, "Général");
    // "Never alone: follow the /join person, else wait" (`whenAlone`, JS 698-700) moved to R4
}

#[test]
fn batch3_stop_phrases() {
    for t in ["stop", "Stop.", "arrête", "chut !", "tais-toi", "c'est bon", "ok merci", "Bobby, stop", "stop stop", "shut up", "that's enough", "ok thanks"] {
        assert!(hit(&STOP, t), "{t}");
    }
    for t in ["stop la musique", "arrête la radio", "ok", "merci de couper le micro de Kévin", "stop the music"] {
        assert!(!hit(&STOP, t), "{t}");
    }
}

// ---------- Test server and a brain with a fake client, for the tests through hear() ----------

struct Who {
    id: &'static str,
    name: &'static str,
    roles: &'static [&'static str],
}

const SUPER: Who = Who { id: WAKANDEX, name: "WakandeX", roles: &[] };
const KEV: Who = Who { id: "kev", name: "Kévin", roles: &["Team Prime"] };
const NOBODY: Who = Who { id: "x", name: "Inconnu", roles: &[] };
const LUCAS: Who = Who { id: "lucas", name: "Lucas", roles: &["Team Prime"] };
const OTHER: Who = Who { id: "42", name: "Marie", roles: &["Team Prime"] };
const ADMIN: Who = Who { id: IISAL, name: "Ilias", roles: &[] };

const EVERYONE: [&Who; 6] = [&SUPER, &KEV, &NOBODY, &LUCAS, &OTHER, &ADMIN];

/// The server of the JS test: three text channels, voice "Général"; `in_voice` = SUPER and Kévin are in it.
fn context(speaker: &Who, in_voice: bool) -> Context {
    let ch = |n: &str| Channel { id: n.into(), name: n.into() };
    let members = EVERYONE
        .iter()
        .map(|w| Member {
            id: w.id.into(),
            name: w.name.into(),
            username: w.name.into(),
            global_name: None,
            channel: (in_voice && (w.id == WAKANDEX || w.id == "kev")).then(|| "Général".to_string()),
            roles: w.roles.iter().map(|r| r.to_string()).collect(),
            bot: false,
        })
        .collect();
    Context {
        speaker: speaker.id.into(),
        members,
        voice_channels: vec![ch("Général")],
        text_channels: vec![ch("dev-bot-test"), ch("memes"), ch("général")],
        bobby_channel: Some("dev-bot-test".into()),
    }
}

/// What the brain sent during one `go` (the JS fake `io`).
#[derive(Debug, Default)]
struct Out {
    said: Vec<String>,
    sounds: Vec<SignalKind>,
    caps: Vec<Cap>,
    hushed: usize,
}

impl Out {
    fn posts(&self) -> Vec<&Cap> {
        self.caps.iter().filter(|c| matches!(c, Cap::Post { .. })).collect()
    }
    fn embed(&self, n: usize) -> bobby_proto::msg::Embed {
        let Some(Cap::Post { embeds, .. }) = self.posts().get(n).copied() else { panic!("no post {n}: {:?}", self.caps) };
        embeds[0].clone()
    }
}

struct Rig {
    brain: Brain<FakeLlm>,
    sent: Sent,
    seen: usize,
    path: PathBuf,
    in_voice: bool,
}

impl Rig {
    fn new(in_voice: bool) -> Self {
        let (io, sent) = fake_client(|c| match c {
            // `io.post` answers the channel it went to
            Cap::Post { channel, .. } => Ok(json!({ "channel": channel.clone().unwrap_or_else(|| "dev-bot-test".into()) })),
            Cap::Mute { .. } | Cap::Deafen { .. } => Ok(json!({ "prev": {} })),
            _ => Ok(Value::Null),
        });
        let path = crate::store::temp_dir("batch34").join("conversations.jsonl");
        Rig { brain: Brain::new(Store::temp(), FakeLlm::default(), io, &path), sent, seen: 0, path, in_voice }
    }

    /// `hear(...)` with a fresh fake io: `talking` = the person Bobby is answering (`io.speaking()`).
    async fn go_with(&mut self, who: &Who, text: &str, acked: bool, talking: Option<&str>) -> Out {
        self.brain.on_client(&ClientMsg::Speaking { user: talking.map(String::from) });
        let h = Heard {
            text: text.into(),
            lang: "fr".into(),
            score: Some(0.9),
            user: who.id.into(),
            user_roles: who.roles.iter().map(|r| r.to_string()).collect(),
            user_name: who.name.into(),
            stt_ms: Some(500),
            acked,
            ..Default::default()
        };
        self.brain.hear(h, &context(who, self.in_voice)).await;
        tokio::time::sleep(std::time::Duration::from_millis(30)).await; // the fake client records on its own task
        let msgs: Vec<BrainMsg> = self.sent.lock().unwrap()[self.seen..].to_vec();
        self.seen += msgs.len();
        let mut out = Out::default();
        for m in msgs {
            match m {
                BrainMsg::Say { text, .. } => out.said.push(text),
                BrainMsg::Signal { kind } => out.sounds.push(kind),
                BrainMsg::Hush => out.hushed += 1,
                BrainMsg::Run { cap, .. } => out.caps.push(cap),
                _ => {}
            }
        }
        out
    }

    async fn go(&mut self, who: &Who, text: &str, acked: bool) -> Out {
        self.go_with(who, text, acked, None).await
    }

    fn journal(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.path).unwrap_or_default().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }
}

// ---------- Batch 3: interrupting Bobby while it speaks ----------

#[tokio::test]
async fn batch3_interrupt() {
    let mut rig = Rig::new(false);
    let lucas = Some("lucas");
    let io = rig.go_with(&OTHER, "Bobby, stop", false, lucas).await;
    assert_eq!((io.hushed, io.said.len()), (0, 0)); // stop from someone else is ignored, silently
    let io = rig.go_with(&ADMIN, "stop", false, lucas).await;
    assert_eq!(io.hushed, 0); // admin needs "Bobby" to stop it
    let io = rig.go_with(&ADMIN, "Bobby, stop !", false, lucas).await;
    assert_eq!(io.hushed, 1);
    let io = rig.go_with(&SUPER, "Bobby arrête", false, lucas).await;
    assert_eq!(io.hushed, 1);
    let io = rig.go_with(&LUCAS, "tais-toi", false, lucas).await;
    assert_eq!((io.hushed, io.said.len()), (1, 0)); // the person it answers: no "Bobby" needed
    let io = rig.go_with(&LUCAS, "stop la musique", false, lucas).await;
    assert_eq!(io.hushed, 0); // the music needs "Bobby, stop la musique"
    let io = rig.go_with(&LUCAS, "stop", false, None).await;
    assert_eq!(io.hushed, 0); // Bobby is not speaking: nothing to interrupt
    let journal = std::fs::read_to_string(&rig.path).unwrap();
    assert!(journal.contains("\"outcome\":\"interrupted\""), "{journal}");
    assert!(journal.contains("\"outcome\":\"stop_ignored\""), "{journal}");
}

// ---------- Batch 3: written answers through hear() ----------

#[tokio::test]
async fn batch3_written_answers() {
    // Through hear(), as Bobby runs it: the content is posted, only a short line is spoken
    let mut rig = Rig::new(false);
    let io = rig.go(&OTHER, "Bobby, dis-moi par écrit ce que tu sais faire", false).await;
    assert_eq!(io.said, ["Je t'envoie ça dans dev-bot-test."]); // "par écrit" posts and does not read
    assert_eq!(io.posts().len(), 1);
    let cap = io.embed(0);
    assert_eq!(cap.fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["Prime (rôle Team Prime)", "Admin (en plus)", "Super admin (tout)"]);
    assert!(cap.fields[0].value.contains("liste générale"), "{}", cap.fields[0].value);
    assert!(!cap.fields[0].value.contains("couper ou rétablir"), "{}", cap.fields[0].value); // moderation stays admin
    let io = rig.go(&OTHER, "Bobby, écris-moi la liste dans memes", false).await;
    assert_eq!((io.said.clone(), io.posts().len()), (vec!["Tu n'as pas le droit de me demander ça.".to_string()], 0)); // outside TEXT_CHANNEL: admin
    let io = rig.go(&SUPER, "Bobby, envoie-moi la liste des droits dans memes", false).await;
    assert_eq!(io.said, ["Je t'envoie ça dans memes."]);
    let Some(Cap::Post { channel, .. }) = io.posts().first().copied().cloned() else { panic!("no post") };
    assert_eq!((channel.as_deref(), io.embed(0).title.as_str()), (Some("memes"), "🔐 Droits"));
    let io = rig.go(&SUPER, "Bobby, quelle heure est-il ?", false).await;
    assert!(io.said[0].starts_with("Nous sommes"), "{:?}", io.said);
    assert_eq!(io.posts().len(), 0); // spoken answers are never echoed in text
}

#[test]
fn batch3_discord_table() {
    // `format::table` stays in the core; chunks, embed splitting and messages (JS 788-801) moved to R4
    let rows = vec![vec!["1".to_string(), "Zoé".to_string()], vec!["22".to_string(), "x".to_string()]];
    assert_eq!(crate::format::table(&["A", "Nom"], &rows), "```\nA   Nom\n--  ---\n1   Zoé\n22  x\n```");
}

// ---------- Batch 4: wake word, runs of a step ----------

#[test]
fn batch4_wake_word_variant_moby() {
    // "Moby est-ce que tu peux me redéplacer…" was ignored
    assert_eq!(crate::wake::find_wake("Moby est-ce que tu peux me redéplacer dans le channel PRIME ?"), Some("Moby"));
}

#[test]
fn batch4_move_resolves_voice_before_text() {
    let store = Store::temp();
    // "arrobase-moi dans dev-bot-test" moved the user to the voice channel dev-bot: a mention, never a move
    let devbot = with_voice(&sup(), "dev-bot");
    let e = resolve(&[json!({ "type": "move", "who": ["moi"], "channel": "dev-bot-test" })], &devbot, &store).unwrap_err();
    assert!(e.fr.contains("dev-bot-test est un salon texte"), "{}", e.fr);
    let ok = resolve(&[json!({ "type": "move", "who": ["moi"], "channel": "dev-bot" })], &devbot, &store).unwrap();
    assert_eq!(ok[0].channel.as_ref().unwrap().name, "dev-bot");
    // same name as a text channel: voice wins
    let g = resolve(&[json!({ "type": "move", "who": ["moi"], "channel": "Général" })], &sup(), &store).unwrap();
    assert_eq!(g[0].channel.as_ref().unwrap().name, "Général");
}

#[tokio::test]
async fn batch4_volume_over_100_says_the_maximum() {
    let store = Store::temp();
    let c = sup();
    let mut vol = resolve(&[json!({ "type": "music_volume", "text": "200" })], &c, &store).unwrap();
    let (_, sent) = run_one(&mut vol[0], &c, &store, |_| Ok(json!({ "volume": 100 }))).await;
    assert_eq!(sent.len(), 1);
    let say = actions::def("music_volume").and_then(|d| d.say).unwrap();
    assert_eq!(say(&vol[0], None, "fr").unwrap(), "Le maximum c'est 100 %, je le mets à fond.");
}

#[tokio::test]
async fn batch4_ping_mentions_users_only() {
    let store = Store::temp();
    let c = sup();
    let mut ping = resolve(&[json!({ "type": "ping", "who": ["Kévin"], "channel": "memes", "text": "lance Apex @everyone" })], &c, &store).unwrap();
    let (_, sent) = run_one(&mut ping[0], &c, &store, |_| Ok(Value::Null)).await;
    // Dropping "@everyone" and `allowedMentions: { users }` is the Discord client's job (R4); the brain asks for users only.
    let [Cap::Ping { users, channel, .. }] = sent.as_slice() else { panic!("{sent:?}") };
    assert_eq!((users.as_slice(), channel.as_deref()), (["Kévin".to_string()].as_slice(), Some("memes")));
}

#[test]
fn batch4_call_me_on_discord_is_an_offer() {
    // "appelle-moi sur Discord": short answer and an offer of a mention; "appelle-moi le Boss" stays a nickname
    let store = Store::temp();
    let call = quick("est-ce que tu peux m'appeler sur Discord ?").unwrap();
    assert_eq!((call[0]["type"].as_str(), call[0]["offer"].as_str()), (Some("ping"), Some("call")));
    assert_eq!(decide(None, &resolve(&call, &sup(), &store).unwrap()), "offer");
    assert!(quick("appelle-moi le Boss").and_then(|v| v.first().map(|a| a["type"] == "ping")) != Some(true));
}

// ---------- Batch 4: through hear() ----------

const CALL: &str = "Je ne peux pas passer d'appel, mais je peux te mentionner dans le salon texte. Tu veux ?";

/// Through hear(): acks, silent actions, offers, the low sound; the conversation goes on across speakers.
#[tokio::test]
async fn batch4_coworker_tone_through_hear() {
    let mut rig = Rig::new(true);
    let io = rig.go(&SUPER, "Bobby, mute Kévin", false).await;
    assert_eq!((io.sounds.clone(), io.said.clone()), (vec![SignalKind::Ack], vec![]));
    assert_eq!(io.caps, [Cap::Mute { users: vec!["kev".into()], on: true }]);
    let io = rig.go(&SUPER, "Bobby, mute Kévin", true).await;
    assert_eq!((io.sounds, io.said), (vec![], vec![])); // acked at the wake gate already: no second ack
    let io = rig.go(&SUPER, "Bobby, quelle heure est-il ?", true).await;
    assert!(io.said[0].starts_with("Nous sommes"), "{:?}", io.said); // an answer is spoken
    let io = rig.go(&SUPER, "Bobby, est-ce que tu peux m'appeler sur Discord ?", true).await;
    assert_eq!(io.said, [CALL]);
    let io = rig.go(&SUPER, "Oui.", false).await;
    // the "oui" runs the offered ping
    let ping_users = io.caps.iter().find_map(|c| if let Cap::Ping { users, .. } = c { Some(users.clone()) } else { None });
    assert_eq!((io.sounds, ping_users), (vec![SignalKind::Ack], Some(vec![WAKANDEX.to_string()])));
    let io = rig.go(&NOBODY, "Bobby, mute Kévin", false).await;
    assert_eq!(io.sounds, [SignalKind::Nope]); // heard, no rights: the low sound
    let io = rig.go(&KEV, "Bobby, mute WakandeX", true).await;
    assert_eq!((io.sounds, io.said), (vec![SignalKind::Nope], vec!["Tu n'as pas le droit de me demander ça.".to_string()]));
    let io = rig.go(&SUPER, "Bobby, explique dans le chat ce que tu sais faire", true).await;
    assert_eq!((io.said.clone(), io.embed(0).title), (vec!["Je t'envoie ça dans dev-bot-test.".to_string()], "🤖 Ce que je sais faire".to_string()));
    // "refais": the speaker's own last request, even after someone else spoke (Kévin really speaks: the JS `remember` is private here)
    rig.go(&KEV, "Bobby, quelle heure est-il", true).await;
    let io = rig.go(&SUPER, "Bobby, refais", true).await;
    assert_eq!((io.posts().len(), io.said), (1, vec!["Je t'envoie ça dans dev-bot-test.".to_string()]));
    let j = rig.journal();
    assert_eq!(j[j.len() - 2..].iter().map(|r| r["who"].as_str().unwrap()).collect::<Vec<_>>(), ["Kévin", "WakandeX"]);
    // After review: only what ran is "done"; a "oui" is never what "refais" replays
    rig.go(&SUPER, "Bobby, est-ce que tu peux m'appeler sur Discord ?", true).await;
    assert_eq!(rig.journal().last().unwrap()["outcome"], "offered"); // offered, not done (JS: `did` is '')
    rig.go(&SUPER, "Oui.", false).await;
    let last = rig.journal().pop().unwrap();
    assert_eq!((last["confirm"].clone(), last["plan"].as_str().is_some_and(|p| p.starts_with("ping"))), (json!(true), true));
    let io = rig.go(&SUPER, "Bobby, refais", true).await;
    assert_eq!(io.said, [CALL]);
    // 2026-11-13: "Bobby ?" while an offer waits is a new order, never "D'accord, j'annule."
    rig.go(&SUPER, "Bobby, est-ce que tu peux m'appeler sur Discord ?", true).await;
    let io = rig.go(&SUPER, "Bobby ?", true).await;
    assert_eq!((io.said, io.sounds), (vec![], vec![SignalKind::Beep])); // a beep, then waits for the command
    let io = rig.go(&SUPER, "quelle heure est-il ?", false).await;
    assert!(io.said[0].starts_with("Nous sommes"), "{:?}", io.said);
    rig.go(&SUPER, "Bobby, est-ce que tu peux m'appeler sur Discord ?", true).await;
    let io = rig.go(&SUPER, "Non.", false).await;
    assert_eq!(io.said, ["D'accord, j'annule."]); // a plain "non" still cancels
}
