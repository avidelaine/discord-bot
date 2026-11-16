//! Port of `bot/test-context.js`: the fake server shared by the tests. Alex speaks from Général.
//! Ids default to the name; channels have their name as id.
use crate::types::{Chan, Ctx, Person, Role};

fn m(name: &str, channel: &str, id: Option<&str>, nicknames: &[&str]) -> Person {
    let n: Vec<String> = nicknames.iter().map(|s| s.to_string()).collect();
    Person { id: id.unwrap_or(name).into(), name: name.into(), channel: Some(channel.into()), nicknames: n.clone(), aliases: n }
}

fn chans(names: &[&str]) -> Vec<Chan> {
    names.iter().map(|n| Chan { id: n.to_string(), name: n.to_string() }).collect()
}

/// The test server; `role` defaults to Other (set it per test, as the JS tests do with `{ ...ctx, role }`).
pub fn ctx() -> Ctx {
    let members = vec![
        m("Alex", "Général", None, &[]),
        m("Zyklon", "Général", Some("Zyklon"), &["le Grec"]),
        m("Kévin", "Général", None, &[]),
        m("Marie", "Gaming", None, &[]),
        m("Lucas", "Gaming", Some("lucas"), &[]),
        m("Prime Ilias", "Gaming", Some("397478613559148545"), &[]),
        m("Sorciax", "Gaming", Some("Sorciax"), &["Vinice Lapute"]),
        m("WakandeX", "Gaming", Some("261531632589078528"), &[]),
    ];
    Ctx {
        speaker: members[0].clone(),
        members,
        voice_channels: chans(&["Général", "Gaming", "AFK", "Réunion"]),
        text_channels: chans(&["général", "annonces", "memes", "dev-bot-test"]),
        text_channel: None,
        role: Role::Other,
        history: vec![],
        trouble: vec![],
    }
}

// ---------- Fake client and fake LLM (tests of running steps and of hear) ----------

use crate::io::Io;
use bobby_proto::infer::{ChatMsg, InferError, Understand};
use bobby_proto::msg::{BrainMsg, Cap};
use serde_json::Value;
use std::sync::{Arc, Mutex};

pub type Sent = Arc<Mutex<Vec<BrainMsg>>>;

/// A client that records every message the brain sends and answers each `Run` with `answer(cap)`
/// (`Err(text)` = a failed cap). Needs a tokio runtime (`#[tokio::test]`).
pub fn fake_client(answer: impl Fn(&Cap) -> Result<Value, String> + Send + 'static) -> (Arc<Io>, Sent) {
    let (io, mut rx) = Io::new();
    let io = Arc::new(io);
    let sent: Sent = Arc::default();
    let (io2, sent2) = (io.clone(), sent.clone());
    tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            if let BrainMsg::Run { id, cap } = &m {
                let r = answer(cap);
                let (ok, data, error) = match r {
                    Ok(v) => (true, v, None),
                    Err(e) => (false, Value::Null, Some(e)),
                };
                sent2.lock().unwrap().push(m.clone());
                io2.deliver(*id, ok, data, error);
            } else {
                sent2.lock().unwrap().push(m);
            }
        }
    });
    (io, sent)
}

/// The caps a fake client received, in order.
pub fn caps(sent: &Sent) -> Vec<Cap> {
    sent.lock().unwrap().iter().filter_map(|m| if let BrainMsg::Run { cap, .. } = m { Some(cap.clone()) } else { None }).collect()
}

/// An LLM that answers from a list, in order (then the last one again), and records the messages it got.
#[derive(Clone, Default)]
pub struct FakeLlm {
    pub answers: Arc<Mutex<Vec<Result<Value, InferError>>>>,
    pub seen: Arc<Mutex<Vec<Vec<ChatMsg>>>>,
}

impl FakeLlm {
    pub fn new(answers: Vec<Value>) -> Self {
        Self { answers: Arc::new(Mutex::new(answers.into_iter().map(Ok).collect())), seen: Arc::default() }
    }
}

impl Understand for FakeLlm {
    async fn chat(&self, messages: &[ChatMsg], _schema: &Value, _max_tokens: u32) -> Result<Value, InferError> {
        self.seen.lock().unwrap().push(messages.to_vec());
        let mut a = self.answers.lock().unwrap();
        if a.len() > 1 { a.remove(0) } else { a.first().cloned().unwrap_or(Ok(serde_json::json!({}))) }
    }
}
