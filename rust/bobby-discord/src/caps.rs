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

/// Signal / Say / Hush / Expect (stub: logs only).
pub async fn brain_msg(_bot: &Arc<Bot>, msg: BrainMsg) {
    eprintln!("[caps] not implemented yet: {msg:?}");
}
