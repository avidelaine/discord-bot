//! Port of the R4 block of unit.test.js (2026-11-19): broader LLM-free shortcuts (infinitives, relative volume,
//! politeness, "mets X", skip, "va dans <salon>").
use crate::resolve::resolve;
use crate::shortcuts::quick;
use crate::store::Store;
use crate::testing::ctx;
use serde_json::json;

#[test]
fn r4_shortcuts() {
    // (phrase, type, text); text None = not checked
    let cases: Vec<(&str, &str, Option<&str>)> = vec![
        ("baisse le son de 50 %", "music_volume", Some("-50%")), ("divise le son par deux", "music_volume", Some("/2")),
        ("divise par 3 le son de la musique", "music_volume", Some("/3")), ("diminue de 50% le volume de la musique", "music_volume", Some("-50%")),
        ("augmente le son de 20 %", "music_volume", Some("+20%")), ("mets le son à 50% de ce qu'il est", "music_volume", Some("-50%")),
        ("baisse encore un peu le son", "music_volume", Some("down")), ("baisse plus", "music_volume", Some("down")),
        ("moins fort", "music_volume", Some("down")), ("mets ton son plus fort", "music_volume", Some("up")),
        ("monte le son", "music_volume", Some("up")), ("est-ce que tu peux baisser le son ?", "music_volume", Some("down")),
        ("peux-tu mettre le volume sonore de la musique à 5% s'il te plaît ?", "music_volume", Some("5")),
        ("À 10% s'il te plaît", "music_volume", Some("10")), ("Bobby, plus fort", "music_volume", Some("up")),
        ("BOBBY STOP", "music_stop", Some("")), ("skip cette musique-là.", "music_skip", Some("")),
        ("mets Bohemian Rhapsody", "music_play", Some("bohemian rhapsody")),
        ("s'il te plaît, est-ce que tu peux me jouer du hardstyle ?", "music_play", Some("hardstyle")),
        ("peux-tu me mettre de la musique ?", "music_play", Some("pop")),
        ("mets sur pause", "music_pause", Some("")), ("reprends la lecture", "music_resume", Some("")),
        ("quitte Discord", "leave", None), ("déconnecte-toi", "leave", None),
    ];
    for (phrase, kind, text) in cases {
        let a = quick(phrase).unwrap_or_default();
        assert!(a.len() == 1 && a[0]["type"] == kind && text.is_none_or(|t| a[0]["text"] == t), "R4 shortcut: \"{phrase}\" -> {a:?}");
    }
    assert_eq!(quick("va dans Gaming"), Some(vec![json!({ "type": "come", "channel": "gaming" })]));
    assert_eq!(
        quick("est-ce que tu peux me redéplacer dans le channel PRIME ?"),
        Some(vec![json!({ "type": "move", "who": ["moi"], "channel": "prime" })])
    );
    let (ctx, store) = (ctx(), Store::temp());
    let plan = resolve(&quick("va dans Gaming").unwrap(), &ctx, &store).unwrap();
    assert_eq!(plan[0].channel.as_ref().unwrap().name, "Gaming");
    // unknown channel: falls back to the LLM
    assert!(resolve(&quick("va dans Mordor").unwrap(), &ctx, &store).is_err());
}

#[test]
fn r4_left_to_the_llm() {
    for phrase in [
        "tu peux répéter ?", "quelle est ta range de volume sonore ?", "quel est le volume maximum ?", "pourquoi la musique plus fort ?",
        "est-ce que tu peux augmenter ton son ou le baisser encore plus que 200% ?", "j'aimerais jouer à Valorant", "nous déplacer dans PRIME",
        "baisse le son et mets du jazz",
    ] {
        assert_eq!(quick(phrase), None, "R4 no shortcut: \"{phrase}\"");
    }
    let a = quick("mets Marie en pause").unwrap_or_default();
    assert!(!a.iter().any(|s| s["type"] == "music_play"), "{a:?}");
}
