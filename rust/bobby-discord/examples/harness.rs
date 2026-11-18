//! Debug harness for live tests: runs the Discord client against a stub brain.
//!
//! Stop the real Bobby first (same token: two gateways on one token fight each other).
//! Run: `cargo run -p bobby-discord --example harness` (from `rust/`; `BOBBY_ENV` overrides the `.env` path).
//!
//! Stub brain: `/join` joins the typer's voice channel, `/leave` leaves, `/surnoms` answers a fixed text.
//! Every `ClientMsg` is printed as one JSON line; every stdin line that parses as a `BrainMsg` is forwarded,
//! e.g. `{"type":"run","id":1,"cap":{"cap":"leave"}}`.
//!
//! Ready-to-paste lines for the live test of every cap (replace the <PLACEHOLDERS> with real ids; each
//! `Result` comes back as one readable `[result]` line):
//! ```text
//! {"type":"run","id":10,"cap":{"cap":"mute","users":["<USER_ID>"],"on":true}}
//! {"type":"run","id":11,"cap":{"cap":"mute","users":["<USER_ID>"],"on":false}}
//! {"type":"run","id":12,"cap":{"cap":"deafen","users":["<USER_ID>"],"on":true}}
//! {"type":"run","id":13,"cap":{"cap":"deafen","users":["<USER_ID>"],"on":false}}
//! {"type":"run","id":14,"cap":{"cap":"move","users":["<USER_ID>"],"channel":"<VOICE_CHANNEL_ID>"}}
//! {"type":"run","id":15,"cap":{"cap":"disconnect","users":["<USER_ID>"]}}
//! {"type":"run","id":16,"cap":{"cap":"join","channel":"<VOICE_CHANNEL_ID>"}}
//! {"type":"run","id":17,"cap":{"cap":"leave"}}
//! {"type":"run","id":20,"cap":{"cap":"post","channel":null,"text":"hello","embeds":[{"title":"Title","description":"Body","color":255}]}}
//! {"type":"run","id":21,"cap":{"cap":"post","channel":"<TEXT_CHANNEL_ID_OR_NAME>","text":"hi @everyone (must stay inert)","embeds":[]}}
//! {"type":"run","id":22,"cap":{"cap":"ping","users":["<USER_ID>"],"channel":null,"text":"wake up @everyone"}}
//! {"type":"run","id":23,"cap":{"cap":"edit_last","channel":null,"text":"edited"}}
//! {"type":"run","id":24,"cap":{"cap":"delete_last","channel":null}}
//! {"type":"run","id":25,"cap":{"cap":"clean","channel":"<TEXT_CHANNEL_ID_OR_NAME>"}}
//! {"type":"run","id":26,"cap":{"cap":"member_name","id":"<USER_ID>"}}
//! {"type":"run","id":30,"cap":{"cap":"music_play","item":{"kind":"radio","style":"jazz"},"add":false}}
//! {"type":"run","id":31,"cap":{"cap":"music_play","item":{"kind":"youtube","query":"daft punk around the world","endless":"similar","seed":null},"add":true}}
//! {"type":"run","id":32,"cap":{"cap":"music_play","item":{"kind":"style","style":"lofi"},"add":false}}
//! {"type":"run","id":33,"cap":{"cap":"music_now"}}
//! {"type":"run","id":34,"cap":{"cap":"music_queue"}}
//! {"type":"run","id":35,"cap":{"cap":"music_pause"}}
//! {"type":"run","id":36,"cap":{"cap":"music_resume"}}
//! {"type":"run","id":37,"cap":{"cap":"music_skip"}}
//! {"type":"run","id":38,"cap":{"cap":"music_replay"}}
//! {"type":"run","id":39,"cap":{"cap":"music_volume","op":{"op":"set","level":40}}}
//! {"type":"run","id":40,"cap":{"cap":"music_volume","op":{"op":"delta","delta":-10}}}
//! {"type":"run","id":41,"cap":{"cap":"music_volume","op":{"op":"get"}}}
//! {"type":"run","id":42,"cap":{"cap":"music_stop"}}
//! ```

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
            match &msg {
                ClientMsg::Result { id, ok, data, error } => {
                    println!("[result {id}] {} data={data}{}", if *ok { "OK" } else { "FAILED" }, error.as_ref().map_or(String::new(), |e| format!(" error={e}")));
                }
                _ => println!("{}", serde_json::to_string(&msg).unwrap_or_default()),
            }
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
