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
