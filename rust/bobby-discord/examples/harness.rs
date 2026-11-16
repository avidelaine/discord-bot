//! Debug harness for live tests: runs the Discord client against a stub brain.
//!
//! Stop the real Bobby first (same token: two gateways on one token fight each other).
//! Run: `cargo run -p bobby-discord --example harness` (from `rust/`; `BOBBY_ENV` overrides the `.env` path).
//!
//! Stub brain: `/join` joins the typer's voice channel, `/leave` leaves, `/surnoms` answers a fixed text.
//! Every `ClientMsg` is printed as one JSON line; every stdin line that parses as a `BrainMsg` is forwarded,
//! e.g. `{"type":"run","id":1,"cap":{"cap":"leave"}}`.

use bobby_discord::config::Config;
use bobby_proto::msg::{BrainMsg, Cap, ClientMsg, Context};
use tokio::sync::mpsc::unbounded_channel;

fn reply(id: u64, text: String, ephemeral: bool) -> BrainMsg {
    BrainMsg::Reply { id, text: Some(text), embeds: vec![], ephemeral }
}

/// What the stub brain answers to a message (`ctx` is the latest context received).
fn stub(msg: &ClientMsg, ctx: &mut Context) -> Vec<BrainMsg> {
    match msg {
        ClientMsg::Context(c) => {
            *ctx = c.clone();
            vec![]
        }
        ClientMsg::Command { id, name, user, .. } => match name.as_str() {
            "leave" => vec![BrainMsg::Run { id: 1000 + id, cap: Cap::Leave }, reply(*id, "À plus.".into(), false)],
            "surnoms" => vec![reply(*id, "Personne n'a de surnom.".into(), false)],
            _ => {
                let channel = ctx.members.iter().find(|m| &m.id == user).and_then(|m| m.channel.clone());
                match channel {
                    None => vec![reply(*id, "Rejoins un salon vocal d'abord.".into(), true)],
                    Some(ch) => {
                        let label = ctx.voice_channels.iter().find(|c| c.id == ch).map_or(ch.clone(), |c| c.name.clone());
                        vec![
                            reply(*id, format!("Bobby écoute dans {label}. Dis « Bobby, … »."), false),
                            BrainMsg::Run { id: 1000 + id, cap: Cap::Join { channel: ch } },
                        ]
                    }
                }
            }
        },
        _ => vec![],
    }
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let cfg = Config::load()?;
    let (to_brain, mut from_client) = unbounded_channel::<ClientMsg>();
    let (to_client, from_brain) = unbounded_channel::<BrainMsg>();

    let tx = to_client.clone();
    tokio::spawn(async move {
        let mut ctx = Context::default();
        while let Some(msg) = from_client.recv().await {
            println!("{}", serde_json::to_string(&msg).unwrap_or_default());
            for out in stub(&msg, &mut ctx) {
                let _ = tx.send(out);
            }
        }
    });
    // A plain thread: tokio's async stdin needs the `io-std` feature, which the workspace does not enable.
    std::thread::spawn(move || {
        for line in std::io::stdin().lines().map_while(Result::ok) {
            match serde_json::from_str::<BrainMsg>(&line) {
                Ok(m) => {
                    let _ = to_client.send(m);
                }
                Err(e) => eprintln!("[harness] not a BrainMsg: {e}"),
            }
        }
    });
    bobby_discord::run(cfg, from_brain, to_brain).await
}
