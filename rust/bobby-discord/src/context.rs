//! Builds the protocol `Context` (the old `context(member, guild)` of `bot/brain.js`) from plain inputs,
//! so it is testable without Discord. `from_guild` is the thin serenity adapter.

use bobby_proto::msg::{Channel, Context, Member};

/// One guild member as plain data.
#[derive(Debug, Clone)]
pub struct MemberIn {
    pub id: u64,
    pub username: String,
    pub global_name: Option<String>,
    pub nick: Option<String>,
    pub bot: bool,
    /// Voice channel id, if connected.
    pub channel: Option<u64>,
    /// Role names and ids.
    pub roles: Vec<String>,
}

fn display_name(m: &MemberIn) -> String {
    m.nick.clone().or_else(|| m.global_name.clone()).unwrap_or_else(|| m.username.clone())
}

fn to_member(m: &MemberIn) -> Member {
    Member {
        id: m.id.to_string(),
        name: display_name(m),
        username: m.username.clone(),
        global_name: m.global_name.clone(),
        channel: m.channel.map(|c| c.to_string()),
        roles: m.roles.clone(),
        bot: m.bot,
    }
}

/// Members = non-bot people in voice. The speaker is added with `channel: None` when not in voice: the brain
/// needs their roles for the rights check even when they type `/join` from outside a channel.
pub fn build(
    speaker: u64,
    members: &[MemberIn],
    voice_channels: Vec<Channel>,
    text_channels: Vec<Channel>,
    bobby_channel: Option<String>,
) -> Context {
    let mut out: Vec<Member> = members.iter().filter(|m| !m.bot && m.channel.is_some()).map(to_member).collect();
    if !out.iter().any(|m| m.id == speaker.to_string()) {
        if let Some(m) = members.iter().find(|m| m.id == speaker) {
            out.push(Member { channel: None, ..to_member(m) });
        }
    }
    Context { speaker: speaker.to_string(), members: out, voice_channels, text_channels, bobby_channel }
}

/// Adapter over serenity's cached guild.
pub fn from_guild(guild: &serenity::all::Guild, speaker: u64, bobby_channel: Option<String>) -> Context {
    use serenity::all::ChannelType;
    let members: Vec<MemberIn> = guild
        .members
        .values()
        .map(|m| MemberIn {
            id: m.user.id.get(),
            username: m.user.name.clone(),
            global_name: m.user.global_name.clone(),
            nick: m.nick.clone(),
            bot: m.user.bot,
            channel: guild.voice_states.get(&m.user.id).and_then(|v| v.channel_id).map(|c| c.get()),
            roles: m
                .roles
                .iter()
                .flat_map(|r| guild.roles.get(r).map(|role| role.name.clone()).into_iter().chain([r.get().to_string()]))
                .collect(),
        })
        .collect();
    let chans = |kind: ChannelType| {
        guild
            .channels
            .values()
            .filter(|c| c.kind == kind)
            .map(|c| Channel { id: c.id.get().to_string(), name: c.name.clone() })
            .collect::<Vec<_>>()
    };
    build(speaker, &members, chans(ChannelType::Voice), chans(ChannelType::Text), bobby_channel)
}

/// Words that help STT spell names. Nicknames are brain data and not available here, so the brain may add
/// its own; this covers display names and voice channel names.
pub fn hotwords(ctx: &Context) -> String {
    let words = std::iter::once("Bobby")
        .chain(ctx.members.iter().map(|m| m.name.as_str()))
        .chain(ctx.voice_channels.iter().map(|c| c.name.as_str()))
        .collect::<Vec<_>>()
        .join(" ");
    words.chars().take(400).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(id: u64, name: &str, bot: bool, channel: Option<u64>) -> MemberIn {
        MemberIn { id, username: name.into(), global_name: None, nick: None, bot, channel, roles: vec![format!("role{id}"), id.to_string()] }
    }

    fn fixture() -> Vec<MemberIn> {
        vec![
            MemberIn { nick: Some("Alexou".into()), global_name: Some("Alex G".into()), ..m(1, "alex", false, Some(10)) },
            m(2, "musicbot", true, Some(10)),
            m(3, "idle", false, None),
        ]
    }

    fn chans() -> (Vec<Channel>, Vec<Channel>) {
        (vec![Channel { id: "10".into(), name: "General".into() }], vec![Channel { id: "20".into(), name: "bobby".into() }])
    }

    #[test]
    fn keeps_non_bot_people_in_voice_with_display_names() {
        let (v, t) = chans();
        let ctx = build(1, &fixture(), v, t, Some("20".into()));
        assert_eq!(ctx.members.len(), 1);
        assert_eq!(ctx.members[0].name, "Alexou");
        assert_eq!(ctx.members[0].channel.as_deref(), Some("10"));
        assert_eq!(ctx.members[0].roles, vec!["role1", "1"]);
        assert_eq!(ctx.bobby_channel.as_deref(), Some("20"));
    }

    #[test]
    fn speaker_outside_voice_is_added_without_channel() {
        let (v, t) = chans();
        let ctx = build(3, &fixture(), v, t, None);
        assert_eq!(ctx.members.len(), 2);
        let s = ctx.members.iter().find(|m| m.id == "3").unwrap();
        assert_eq!((s.channel.clone(), s.name.as_str()), (None, "idle"));
        assert_eq!(ctx.speaker, "3");
    }

    #[test]
    fn hotwords_lists_names_and_is_capped() {
        let (v, t) = chans();
        let ctx = build(1, &fixture(), v, t, None);
        assert_eq!(hotwords(&ctx), "Bobby Alexou General");
        let mut long = ctx.clone();
        long.members[0].name = "x".repeat(900);
        assert_eq!(hotwords(&long).chars().count(), 400);
    }
}
