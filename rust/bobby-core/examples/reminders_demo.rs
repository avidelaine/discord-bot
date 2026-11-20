//! Reminders demo: a real Brain with a fake client, in fast time. Sets a 2-minute timer and a reminder, lists them,
//! cancels the timer, then jumps the clock forward so the reminder fires. Prints every line Bobby says.
//! Run: `cargo run -p bobby-core --example reminders_demo` (no Discord, no mic, no model, nothing real is touched).
use bobby_core::actions::reminders;
use bobby_core::brain::{Brain, Heard};
use bobby_core::io::Io;
use bobby_core::store::{temp_dir, Store};
use bobby_proto::infer::{ChatMsg, InferError, Understand};
use bobby_proto::msg::{BrainMsg, Channel, Context, Member};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// Every phrase here is a shortcut: the LLM is never asked.
struct NoLlm;
impl Understand for NoLlm {
    async fn chat(&self, _: &[ChatMsg], _: &Value, _: u32) -> Result<Value, InferError> {
        Ok(json!({}))
    }
}

type Lines = Arc<Mutex<Vec<String>>>;

async fn say(brain: &Brain<NoLlm>, lines: &Lines, text: &str) {
    println!("Marie : {text}");
    let h = Heard { text: text.into(), lang: "fr".into(), score: Some(0.9), user: "42".into(), user_roles: vec!["Team Prime".into()], user_name: "Marie".into(), ..Default::default() };
    let ch = |n: &str| Channel { id: n.into(), name: n.into() };
    let marie = Member { id: "42".into(), name: "Marie".into(), username: "Marie".into(), global_name: None, channel: None, roles: vec!["Team Prime".into()], bot: false };
    let ctx = Context { speaker: "42".into(), members: vec![marie], voice_channels: vec![ch("Général")], text_channels: vec![ch("dev-bot-test")], bobby_channel: Some("dev-bot-test".into()) };
    brain.hear(h, &ctx).await;
    flush(lines).await;
}

async fn flush(lines: &Lines) {
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    for l in lines.lock().unwrap().drain(..) {
        println!("  Bobby : {l}");
    }
}

#[tokio::main]
async fn main() {
    let (io, mut rx) = Io::new();
    let io = Arc::new(io);
    let lines: Lines = Arc::default();
    let (io2, lines2) = (io.clone(), lines.clone());
    tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            match m {
                BrainMsg::Say { text, to_user, .. } => lines2.lock().unwrap().push(format!("(to {}) {text}", to_user.unwrap_or_else(|| "everyone".into()))),
                BrainMsg::Run { id, .. } => {
                    io2.deliver(id, true, Value::Null, None);
                }
                _ => {}
            }
        }
    });
    let brain = Brain::new(Store::temp(), NoLlm, io, temp_dir("reminders-demo").join("conversations.jsonl"));

    say(&brain, &lines, "Bobby, mets un minuteur de 2 minutes").await;
    say(&brain, &lines, "Bobby, rappelle-moi dans 10 minutes de sortir le linge").await;
    say(&brain, &lines, "Bobby, quels sont mes rappels").await;
    say(&brain, &lines, "Bobby, annule le minuteur").await;
    say(&brain, &lines, "Bobby, quels sont mes rappels").await;

    // Fast time: no waiting, the ticker's own function gets a "now" 10 minutes later.
    println!("-- 10 minutes later --");
    let marie = Member { id: "42".into(), name: "Marie".into(), username: "Marie".into(), global_name: None, channel: Some("Général".into()), roles: vec![], bot: false };
    let in_voice = Context { speaker: "42".into(), members: vec![marie], voice_channels: vec![], text_channels: vec![], bobby_channel: None };
    let n = reminders::fire_due(&brain.store, &brain.io, reminders::now_ms() + 10 * 60_000 + 1_000, Some(&in_voice)).await;
    flush(&lines).await;
    println!("({n} reminder fired)");
    say(&brain, &lines, "Bobby, quels sont mes rappels").await;
}
