//! Execution of the brain's caps (stub: only join/leave for now, a later task fills the rest).

use crate::client::Bot;
use bobby_proto::msg::{BrainMsg, Cap, ClientMsg};
use serenity::all::{ChannelId, UserId};
use std::sync::Arc;

pub async fn run(bot: &Arc<Bot>, id: u64, cap: Cap) -> ClientMsg {
    let res = match cap {
        Cap::Join { channel } => match channel.parse::<u64>() {
            Ok(c) => match bot.joiner().or_else(|| bot.cfg.super_admins.first().map(|u| UserId::new(*u))) {
                Some(j) => bot.join(ChannelId::new(c), j).await,
                None => Err("no joiner known".to_string()),
            },
            Err(_) => Err("bad channel id".to_string()),
        },
        Cap::Leave => {
            bot.leave().await;
            Ok(())
        }
        _ => Err("not implemented yet".to_string()),
    };
    match res {
        Ok(()) => ClientMsg::Result { id, ok: true, data: serde_json::Value::Null, error: None },
        Err(e) => ClientMsg::Result { id, ok: false, data: serde_json::Value::Null, error: Some(e) },
    }
}

/// Lights (or clears) the speaking ring of `owner` (stub: a later task fills it).
pub fn ring(_bot: &Arc<Bot>, _on: bool, _owner: UserId) {}

/// Signal / Say / Hush (stub: logs only); Expect opens the no-wake-word window.
pub async fn brain_msg(bot: &Arc<Bot>, msg: BrainMsg) {
    match msg {
        BrainMsg::Expect { user, ms } => match user.parse::<u64>() {
            Ok(u) => bot.listen.lock().unwrap().expect(UserId::new(u), ms),
            Err(_) => eprintln!("[caps] expect: bad user id {user}"),
        },
        other => eprintln!("[caps] not implemented yet: {other:?}"),
    }
}
