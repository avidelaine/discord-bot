//! Never alone: when Bobby's channel empties it follows the person who typed /join; if they are not in
//! voice, the music pauses and Bobby leaves after 2 minutes unless someone comes back.
//! Port of `checkAlone` / `whenAlone` of `bot/bot.js` and `bot/actions/discord.js`.

use crate::{speech, Bot};
use serenity::all::{ChannelId, UserId};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;

/// A group move fires several voice updates: decide once they settle.
const SETTLE: Duration = Duration::from_millis(1500);
const LEAVE_AFTER: Duration = Duration::from_secs(120);

#[derive(Debug, PartialEq, Eq)]
pub enum Next {
    Stay,
    Follow(ChannelId),
    Wait,
}

/// `voice` holds the non-bot users that are in a voice channel.
pub fn when_alone(bot_channel: ChannelId, joiner: Option<UserId>, voice: &[(UserId, ChannelId)]) -> Next {
    if voice.iter().any(|(_, c)| *c == bot_channel) {
        return Next::Stay;
    }
    match joiner.and_then(|j| voice.iter().find(|(u, _)| *u == j)) {
        Some((_, c)) => Next::Follow(*c),
        None => Next::Wait,
    }
}

#[derive(Default)]
pub struct Alone {
    settle: Option<JoinHandle<()>>,
    leave: Option<JoinHandle<()>>,
    /// Tag of the track paused because Bobby was alone.
    paused: Option<u64>,
}

fn cancel(h: &mut Option<JoinHandle<()>>) {
    if let Some(h) = h.take() {
        h.abort();
    }
}

/// Leave or fresh join: nothing of a previous session may fire later.
pub fn reset(bot: &Bot) {
    let mut a = bot.alone.lock().unwrap();
    cancel(&mut a.settle);
    cancel(&mut a.leave);
    a.paused = None;
}

/// Call on every voice state change of the guild; debounced, then decided from the cache.
pub fn on_voice_update(bot: &Arc<Bot>) {
    let mut a = bot.alone.lock().unwrap();
    cancel(&mut a.settle);
    let b = bot.clone();
    a.settle = Some(tokio::spawn(async move {
        tokio::time::sleep(SETTLE).await;
        check(&b).await;
    }));
}

async fn check(bot: &Arc<Bot>) {
    let Some(call) = bot.songbird.get(bot.guild) else { return };
    let Some(here) = call.lock().await.current_channel().map(|c| ChannelId::new(c.0.get())) else { return };
    let me = bot.cache.current_user().id;
    let voice: Vec<(UserId, ChannelId)> = bot
        .cache
        .guild(bot.guild)
        .map(|g| {
            g.voice_states
                .values()
                .filter_map(|v| {
                    // Only a cached, non-bot member counts as a human (not Bobby himself).
                    let human = v.user_id != me && g.members.get(&v.user_id).is_some_and(|m| !m.user.bot);
                    human.then_some(()).and(v.channel_id).map(|c| (v.user_id, c))
                })
                .collect()
        })
        .unwrap_or_default();
    match when_alone(here, bot.joiner(), &voice) {
        Next::Wait => {
            if bot.alone.lock().unwrap().leave.is_some() {
                return;
            }
            if bot.player.is_playing().await {
                let tag = bot.player.current_tag().await;
                bot.alone.lock().unwrap().paused = tag;
                bot.player.pause().await;
            }
            eprintln!("[voice] alone, nobody to follow: leaving in 2 minutes unless someone comes back");
            let b = bot.clone();
            let h = tokio::spawn(async move {
                tokio::time::sleep(LEAVE_AFTER).await;
                // Detach our own handle first: `leave` -> `reset` would abort this very task mid-way.
                b.alone.lock().unwrap().leave = None;
                b.player.stop().await;
                speech::stop_all(&b);
                b.leave().await;
                eprintln!("[voice] left: alone for 2 minutes");
            });
            bot.alone.lock().unwrap().leave = Some(h);
        }
        next => {
            {
                let mut a = bot.alone.lock().unwrap();
                cancel(&mut a.leave);
            }
            if let Next::Follow(c) = next {
                if bot.move_to(c).await.is_some() {
                    eprintln!("[voice] moved: waiting for DAVE re-handshake");
                }
            }
            // Only the very track we paused, still paused: a pause, stop or new song by someone meanwhile wins.
            let tag = bot.alone.lock().unwrap().paused.take();
            if let Some(t) = tag {
                if bot.player.current_tag().await == Some(t) && !bot.player.is_playing().await {
                    bot.player.resume().await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOT: ChannelId = ChannelId::new(1);
    const OTHER: ChannelId = ChannelId::new(2);
    const ME: UserId = UserId::new(10);

    #[test]
    fn stays_when_someone_is_with_bobby() {
        assert_eq!(when_alone(BOT, Some(ME), &[(UserId::new(11), BOT)]), Next::Stay);
    }

    #[test]
    fn follows_the_joiner() {
        assert_eq!(when_alone(BOT, Some(ME), &[(ME, OTHER), (UserId::new(11), ChannelId::new(3))]), Next::Follow(OTHER));
    }

    #[test]
    fn waits_when_joiner_is_not_in_voice() {
        assert_eq!(when_alone(BOT, Some(ME), &[(UserId::new(11), OTHER)]), Next::Wait);
        assert_eq!(when_alone(BOT, None, &[]), Next::Wait);
    }

    #[test]
    fn joiner_already_in_bobbys_channel() {
        assert_eq!(when_alone(BOT, Some(ME), &[(ME, BOT)]), Next::Stay);
    }
}
