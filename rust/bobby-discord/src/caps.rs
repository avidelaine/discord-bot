//! Execution of the brain's caps (stub: only join/leave for now, a later task fills the rest).

use crate::client::Bot;
use bobby_proto::msg::{BrainMsg, Cap, ClientMsg, VolumeOp};
use crate::format;
use serde_json::{json, Value};
use serenity::all::{
    ChannelId, ChannelType, CreateAllowedMentions, CreateMessage, EditMember, EditMessage, GetMessages, Message, MessageId, UserId,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

type Res = Result<Value, String>;

/// Bumped by every Join: a delayed Leave only disconnects if no join happened meanwhile.
static JOIN_GEN: AtomicU64 = AtomicU64::new(0);

/// Discord error text; code 50013 (missing permissions) is prefixed so the brain can tell.
fn dc(e: serenity::Error) -> String {
    use serenity::http::HttpError;
    if let serenity::Error::Http(HttpError::UnsuccessfulRequest(r)) = &e {
        if let Some(t) = code_text(r.error.code, &r.error.message) {
            return t;
        }
    }
    e.to_string()
}

fn code_text(code: isize, message: &str) -> Option<String> {
    (code == 50013).then(|| format!("50013 {message}"))
}

fn uid(s: &str) -> Result<UserId, String> {
    s.parse::<u64>().ok().filter(|n| *n > 0).map(UserId::new).ok_or_else(|| format!("bad user id {s}"))
}

fn cid(s: &str) -> Result<ChannelId, String> {
    s.parse::<u64>().ok().filter(|n| *n > 0).map(ChannelId::new).ok_or_else(|| "bad channel id".to_string())
}

/// Never panics: every Discord error becomes `ok: false` with its text.
pub async fn run(bot: &Arc<Bot>, id: u64, cap: Cap) -> ClientMsg {
    match exec(bot, cap).await {
        Ok(data) => ClientMsg::Result { id, ok: true, data, error: None },
        Err((e, data)) => ClientMsg::Result { id, ok: false, data, error: Some(e) },
    }
}

/// The error carries data too: a music error keeps the player's `{"error": ..}` value.
async fn exec(bot: &Arc<Bot>, cap: Cap) -> Result<Value, (String, Value)> {
    let p = &bot.player;
    let v = match cap {
        Cap::MusicPlay { item, add } => {
            if add {
                p.add(item).await
            } else {
                p.play_now(item, true).await
            }
        }
        Cap::MusicStop => p.stop().await,
        Cap::MusicSkip => p.skip().await,
        Cap::MusicReplay => p.replay().await,
        Cap::MusicPause => p.pause().await,
        Cap::MusicResume => p.resume().await,
        Cap::MusicNow => json!({ "nowPlaying": p.list().await["nowPlaying"] }),
        Cap::MusicQueue => p.list().await,
        Cap::MusicVolume { op } => match op {
            VolumeOp::Get => p.volume().await,
            VolumeOp::Set { level } => p.set_volume(level as i64).await,
            VolumeOp::Delta { delta } if delta >= 0 => p.volume_up(delta as i64).await,
            VolumeOp::Delta { delta } => p.volume_down(-(delta as i64)).await,
            VolumeOp::Mute | VolumeOp::Unmute => return Err(("unsupported".into(), Value::Null)),
        },
        Cap::PcMedia { .. }
        | Cap::PcNow
        | Cap::PcVolume { .. }
        | Cap::PcApps
        | Cap::PcWindows
        | Cap::PcLaunch { .. }
        | Cap::PcClose { .. }
        | Cap::PcLock
        | Cap::PcSleep
        | Cap::PcShutdown
        | Cap::PcGpu => return Err(("not a Discord cap".into(), Value::Null)),
        other => return discord(bot, other).await.map_err(|e| (e, Value::Null)),
    };
    match v.get("error").and_then(Value::as_str) {
        Some(e) => Err((e.to_string(), v.clone())),
        None => Ok(v),
    }
}

async fn discord(bot: &Arc<Bot>, cap: Cap) -> Res {
    let g = bot.guild;
    let h = &bot.http;
    match cap {
        Cap::Mute { users, on } => voice_set(bot, &users, on, true).await,
        Cap::Deafen { users, on } => voice_set(bot, &users, on, false).await,
        Cap::Move { users, channel } => {
            let to = cid(&channel)?;
            let mut prev = serde_json::Map::new();
            let mut follow = false;
            for u in &users {
                let user = uid(u)?;
                let before = bot.cache.guild(g).and_then(|gd| gd.voice_states.get(&user).and_then(|v| v.channel_id));
                prev.insert(u.clone(), before.map_or(Value::Null, |c| json!(c.to_string())));
                g.edit_member(h, user, EditMember::new().voice_channel(to)).await.map_err(dc)?;
                follow |= bot.joiner() == Some(user);
            }
            // Approximation of `io.follow` (bot.js): the client does not know who spoke, so Bobby follows
            // when the person who ran /join is among those moved.
            if follow {
                bot.move_to(to).await;
            }
            Ok(json!({ "prev": prev }))
        }
        Cap::Disconnect { users } => {
            for u in &users {
                g.disconnect_member(h, uid(u)?).await.map_err(dc)?;
            }
            Ok(Value::Null)
        }
        Cap::Join { channel } => {
            let c = cid(&channel)?;
            JOIN_GEN.fetch_add(1, Ordering::SeqCst);
            // The person who just typed /join becomes the joiner (the one Bobby follows).
            let typer = bot.last_join_cmd().filter(|(_, t)| t.elapsed() < Duration::from_secs(10)).map(|(u, _)| u);
            if let Some(u) = typer {
                bot.set_joiner(u);
            }
            if let Some(call) = bot.songbird.get(g) {
                // Keeps the receive handlers and the music running (a fresh join would drop them).
                let prev = bot.move_to(c).await;
                let here = call.lock().await.current_channel().map(|x| x.0.get());
                if prev.is_none() && here != Some(c.get()) {
                    return Err(format!("could not move to channel {channel}"));
                }
                return Ok(json!({ "prev": prev.map_or(Value::Null, |p| json!(p.to_string())) }));
            }
            let j = typer
                .or_else(|| bot.joiner())
                .or_else(|| bot.cfg.super_admins.first().filter(|u| **u > 0).map(|u| UserId::new(*u)))
                .ok_or("no joiner known")?;
            bot.join(c, j).await.map(|_| json!({ "prev": Value::Null }))
        }
        Cap::Leave => {
            // Music stops now, the connection goes 3 s later so the farewell is heard (bot.js `leave`).
            bot.player.stop().await;
            let b = bot.clone();
            let gen = JOIN_GEN.load(Ordering::SeqCst);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(3)).await;
                if JOIN_GEN.load(Ordering::SeqCst) == gen {
                    b.leave().await;
                }
            });
            Ok(Value::Null)
        }
        Cap::Post { channel, text, embeds } => {
            let (ch, name) = text_channel(bot, channel.as_deref())?;
            for o in format::messages(text.as_deref(), &embeds) {
                ch.send_message(h, format::to_serenity(&o)).await.map_err(dc)?;
            }
            Ok(json!({ "channel": name }))
        }
        Cap::Ping { users, channel, text } => {
            let (ch, _) = text_channel(bot, channel.as_deref())?;
            let ids = users.iter().map(|u| uid(u)).collect::<Result<Vec<_>, _>>()?;
            let m = CreateMessage::new().content(ping_content(&users, &text)).allowed_mentions(CreateAllowedMentions::new().users(ids));
            ch.send_message(h, m).await.map_err(dc)?;
            Ok(Value::Null)
        }
        Cap::EditLast { channel, text } => {
            let (ch, _) = text_channel(bot, channel.as_deref())?;
            let found = own(bot, ch, false, 300).await?.into_iter().next();
            if let Some(mut m) = found.clone() {
                m.edit(h, EditMessage::new().content(edit_text(&text)).allowed_mentions(CreateAllowedMentions::new())).await.map_err(dc)?;
            }
            Ok(json!({ "found": found.is_some() }))
        }
        Cap::DeleteLast { channel } => {
            let (ch, _) = text_channel(bot, channel.as_deref())?;
            let found = own(bot, ch, false, 300).await?.into_iter().next();
            if let Some(m) = &found {
                m.delete(h).await.map_err(dc)?;
            }
            Ok(json!({ "found": found.is_some() }))
        }
        Cap::Clean { channel } => {
            let (ch, name) = text_channel(bot, channel.as_deref())?;
            let mine = own(bot, ch, true, 1000).await?;
            let count = mine.len();
            let http = bot.http.clone();
            // One by one in the background (rate limits): the spoken reply does not wait.
            tokio::spawn(async move {
                for m in mine {
                    let _ = m.delete(&http).await;
                }
            });
            Ok(json!({ "count": count, "channel": name }))
        }
        Cap::MemberName { id } => {
            let user = uid(&id)?;
            let cached = bot.cache.guild(g).and_then(|gd| gd.members.get(&user).map(|m| m.display_name().to_string()));
            let name = match cached {
                Some(n) => n,
                None => g.member(h, user).await.map(|m| m.display_name().to_string()).unwrap_or(id),
            };
            Ok(json!({ "name": name }))
        }
        _ => Err("not a Discord cap".into()),
    }
}

/// Server mute (`mute`) or deafen; returns each user's state before the change so the brain can undo it.
async fn voice_set(bot: &Arc<Bot>, users: &[String], on: bool, mute: bool) -> Res {
    let mut prev = serde_json::Map::new();
    for u in users {
        let user = uid(u)?;
        let before = bot.cache.guild(bot.guild).and_then(|g| g.voice_states.get(&user).map(|v| if mute { v.mute } else { v.deaf }));
        prev.insert(u.clone(), json!(before.unwrap_or(false)));
        let edit = if mute { EditMember::new().mute(on) } else { EditMember::new().deafen(on) };
        bot.guild.edit_member(&bot.http, user, edit).await.map_err(dc)?;
    }
    Ok(json!({ "prev": prev }))
}

/// Channel by id or name among the guild's text channels; None = Bobby's default one.
fn text_channel(bot: &Bot, key: Option<&str>) -> Result<(ChannelId, String), String> {
    let list: Vec<(u64, String)> = bot
        .cache
        .guild(bot.guild)
        .map(|g| g.channels.values().filter(|c| c.kind == ChannelType::Text).map(|c| (c.id.get(), c.name.clone())).collect())
        .unwrap_or_default();
    let id = match key {
        Some(k) => resolve_channel(&list, k).ok_or_else(|| format!("unknown channel {k}"))?,
        None => bot.default_text().ok_or("no default text channel")?.get(),
    };
    let name = list.iter().find(|(i, _)| *i == id).map_or_else(|| id.to_string(), |(_, n)| n.clone());
    Ok((ChannelId::new(id), name))
}

/// Bobby's own messages, newest first (`own()` of discord.js), paging by 100 up to `max` messages read.
/// Voice echoes are skipped unless `all`.
async fn own(bot: &Bot, ch: ChannelId, all: bool, max: usize) -> Result<Vec<Message>, String> {
    let me = bot.cache.current_user().id.get();
    let mut out = Vec::new();
    let mut before: Option<MessageId> = None;
    let mut seen = 0;
    while seen < max {
        let mut q = GetMessages::new().limit(100);
        if let Some(b) = before {
            q = q.before(b);
        }
        let batch = ch.messages(&bot.http, q).await.map_err(dc)?;
        let Some(last) = batch.last() else { break };
        before = Some(last.id);
        seen += batch.len();
        out.extend(batch.into_iter().filter(|m| is_own(m.author.id.get(), me, &m.content, all)));
    }
    Ok(out)
}

/// Bobby's message, and (unless `all`) not a voice echo ("🗣️ ..." or "🤖 ...").
fn is_own(author: u64, me: u64, content: &str, all: bool) -> bool {
    author == me && (all || !(content.starts_with('\u{1F5E3}') || content.starts_with('\u{1F916}')))
}

/// Removes the `@` of `@everyone` / `@here` (any case) so they stay plain words.
fn neutralise(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('@') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let low: String = after.chars().take(8).collect::<String>().to_ascii_lowercase();
        if !(low.starts_with("everyone") || low.starts_with("here")) {
            out.push('@');
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Edited text: broadcast mentions neutralised, cut to Discord's 2000 chars.
fn edit_text(text: &str) -> String {
    neutralise(text).chars().take(2000).collect()
}

/// `<@id> <@id> text`, trimmed, with the broadcast mentions neutralised.
fn ping_content(users: &[String], text: &str) -> String {
    let m: Vec<String> = users.iter().map(|u| format!("<@{u}>")).collect();
    format!("{} {}", m.join(" "), neutralise(text)).trim().to_string()
}

/// Id match first, then case-insensitive name (a leading `#` is ignored).
fn resolve_channel(list: &[(u64, String)], key: &str) -> Option<u64> {
    let k = key.trim().trim_start_matches('#');
    list.iter()
        .find(|(i, _)| i.to_string() == k)
        .or_else(|| list.iter().find(|(_, n)| n.eq_ignore_ascii_case(k)))
        .map(|(i, _)| *i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutralise_strips_broadcast_mentions() {
        assert_eq!(neutralise("hi @everyone and @Here, @bob @"), "hi everyone and Here, @bob @");
    }

    #[test]
    fn edit_text_is_neutral_and_char_safe() {
        assert_eq!(edit_text("@everyone"), "everyone");
        assert_eq!(edit_text(&"é".repeat(2500)).chars().count(), 2000);
    }

    #[test]
    fn permission_error_code_is_prefixed() {
        assert_eq!(code_text(50013, "Missing Permissions").as_deref(), Some("50013 Missing Permissions"));
        assert_eq!(code_text(10003, "Unknown Channel"), None);
    }

    #[test]
    fn ids_of_zero_are_rejected() {
        assert!(uid("0").is_err() && cid("0").is_err() && uid("x").is_err());
        assert!(uid("5").is_ok());
    }

    #[test]
    fn ping_content_lists_users_first() {
        assert_eq!(ping_content(&["1".into(), "2".into()], "yo @everyone"), "<@1> <@2> yo everyone");
        assert_eq!(ping_content(&["1".into()], ""), "<@1>");
    }

    #[test]
    fn own_filter_skips_echoes_unless_all() {
        assert!(is_own(5, 5, "salut", false));
        assert!(!is_own(5, 5, "\u{1F5E3}\u{FE0F} salut", false));
        assert!(!is_own(5, 5, "\u{1F916} yo", false));
        assert!(is_own(5, 5, "\u{1F5E3}\u{FE0F} salut", true));
        assert!(!is_own(6, 5, "salut", true));
    }

    #[test]
    fn resolve_channel_by_id_or_name() {
        let l = vec![(10, "General".to_string()), (11, "memes".to_string())];
        assert_eq!(resolve_channel(&l, "11"), Some(11));
        assert_eq!(resolve_channel(&l, "#GENERAL"), Some(10));
        assert_eq!(resolve_channel(&l, "nope"), None);
    }
}

/// Lights (or clears) the speaking ring of `owner` (stub: a later task fills it).
pub fn ring(bot: &Arc<Bot>, on: bool, owner: UserId) {
    crate::speech::ring(bot, on, Some(owner));
}

/// Signal / Say / Hush (stub: logs only); Expect opens the no-wake-word window.
pub async fn brain_msg(bot: &Arc<Bot>, msg: BrainMsg) {
    match msg {
        BrainMsg::Expect { user, ms } => match user.parse::<u64>() {
            Ok(u) => bot.listen.lock().unwrap().expect(UserId::new(u), ms),
            Err(_) => eprintln!("[caps] expect: bad user id {user}"),
        },
        BrainMsg::Signal { kind } => crate::speech::signal(bot, kind),
        BrainMsg::Say { text, lang, voice, to_user } => crate::speech::say(bot, text, lang, voice, to_user),
        BrainMsg::Hush => crate::speech::hush(bot),
        other => eprintln!("[caps] not implemented yet: {other:?}"),
    }
}
