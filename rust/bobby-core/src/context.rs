//! The server as the brain sees it (port of `context()` and `hotwords()` of `bot/brain.js`), built from the
//! proto `Context` a client sends plus the nicknames the brain stores.
use crate::store::Store;
use crate::types::{Chan, Ctx, Person};
use bobby_proto::msg::{Channel, Context};
use serde_json::{json, Value};

fn chans(l: &[Channel]) -> Vec<Chan> {
    l.iter().map(|c| Chan { id: c.id.clone(), name: c.name.clone() }).collect()
}

/// `role`, `history` and `trouble` are left at their defaults: the caller fills them.
pub fn build(c: &Context, store: &Store) -> Ctx {
    let nicknames = store.load_or("aliases", json!({})); // memberId -> [nickname], set by voice
    let nick = |id: &str| -> Vec<String> {
        nicknames.get(id).and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default()
    };
    let voice_channels = chans(&c.voice_channels);
    let chan_name = |id: &str| voice_channels.iter().find(|v| v.id == id).map(|v| v.name.clone());
    let members: Vec<Person> = c
        .members
        .iter()
        .filter(|m| !m.bot)
        .filter_map(|m| {
            let channel = chan_name(m.channel.as_deref()?)?;
            let n = nick(&m.id);
            let aliases = [Some(m.username.clone()), m.global_name.clone()].into_iter().flatten().chain(n.iter().cloned()).filter(|s| !s.is_empty()).collect();
            Some(Person { id: m.id.clone(), name: m.name.clone(), channel: Some(channel), nicknames: n, aliases, roles: m.roles.clone() })
        })
        .collect();
    // not in voice: name and roles from the member list all the same (perm_show on oneself)
    let speaker = members.iter().find(|m| m.id == c.speaker).cloned().unwrap_or_else(|| {
        let m = c.members.iter().find(|m| m.id == c.speaker);
        Person { id: c.speaker.clone(), name: m.map_or_else(|| c.speaker.clone(), |m| m.name.clone()), roles: m.map(|m| m.roles.clone()).unwrap_or_default(), ..Default::default() }
    });
    let text_channels = chans(&c.text_channels);
    let text_channel = c.bobby_channel.as_ref().and_then(|id| text_channels.iter().find(|t| &t.id == id).cloned());
    Ctx { speaker, members, voice_channels, text_channels, text_channel, ..Default::default() }
}

/// Words Whisper should favour: Bobby, command words, names, nicknames, voice channels (400 chars at most).
pub fn hotwords(ctx: &Ctx) -> String {
    let all: Vec<&str> = ["Bobby", "Discord", "le son", "volume", "musique", "hardstyle"] // command words stay ahead of the 400-char cut
        .into_iter()
        .chain(ctx.members.iter().flat_map(|m| std::iter::once(m.name.as_str()).chain(m.nicknames.iter().map(String::as_str))))
        .chain(ctx.voice_channels.iter().map(|c| c.name.as_str()))
        .collect();
    all.join(" ").chars().take(400).collect() // ponytail: JS slices UTF-16 units; same for every name without emoji
}

#[cfg(test)]
mod tests {
    use super::*;
    use bobby_proto::msg::Member;

    fn mem(id: &str, name: &str, channel: Option<&str>, bot: bool) -> Member {
        Member { id: id.into(), name: name.into(), username: format!("{}_u", name.to_lowercase()), global_name: None, channel: channel.map(Into::into), roles: vec![], bot }
    }

    #[test]
    fn context_from_proto() {
        let s = Store::temp();
        s.save("aliases", &json!({"2": ["le Grec"]})).unwrap();
        let c = Context {
            speaker: "9".into(),
            members: vec![mem("1", "Alex", Some("10"), false), mem("2", "Zyklon", Some("10"), false), mem("3", "Bot", Some("10"), true), Member { roles: vec!["Team Prime".into()], ..mem("9", "Marie", None, false) }],
            voice_channels: vec![Channel { id: "10".into(), name: "Général".into() }],
            text_channels: vec![Channel { id: "20".into(), name: "bobby".into() }],
            bobby_channel: Some("20".into()),
        };
        let ctx = build(&c, &s);
        assert_eq!(ctx.members.len(), 2);
        assert_eq!(ctx.members[1].aliases, vec!["zyklon_u".to_string(), "le Grec".into()]);
        assert_eq!(ctx.members[0].channel.as_deref(), Some("Général"));
        assert_eq!((ctx.speaker.id.as_str(), ctx.speaker.name.as_str(), ctx.speaker.channel.clone()), ("9", "Marie", None));
        assert_eq!(ctx.speaker.roles, vec!["Team Prime".to_string()]); // final review: a speaker out of voice keeps their roles
        assert_eq!(ctx.text_channel.unwrap().name, "bobby");
        assert_eq!(hotwords(&crate::testing::ctx()), "Bobby Discord le son volume musique hardstyle Alex Zyklon le Grec Kévin Marie Lucas Prime Ilias Sorciax Vinice Lapute WakandeX Général Gaming AFK Réunion");
    }
}
