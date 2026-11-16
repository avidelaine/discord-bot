//! LLM-free shortcuts (port of the "LLM-free shortcuts" part of `bot/brain.js`): confirmations, "stop",
//! polite wrappers, `quick`, `cancels` and "réessaie".
use crate::actions::modules;
use crate::util::{caps, group, hit, re, re_i, replace1, replace_all, Hist};
use fancy_regex::Regex;
use serde_json::{json, Value};
use std::sync::LazyLock;

// A confirmation starts with a yes and has no "no" in it, however long ("oui je confirme Bobby je confirme"):
// "non, pas ok" or "ok mais attends" must never confirm a disconnect or a shutdown.
pub static YES: LazyLock<Regex> = LazyLock::new(|| re_i(r"^(?:bobby[\s,.!?:;-]*)?(?:oui|ouais|vas-y|confirme|je confirme|ok|okay|d'accord|go|yes|yeah|yep|sure|confirm)\b"));
pub static NO: LazyLock<Regex> = LazyLock::new(|| re_i(r"\b(?:non|no|pas|nope|don't|annule|cancel|stop|attends|wait)\b"));

pub fn confirms(text: &str) -> bool {
    static END: LazyLock<Regex> = LazyLock::new(|| re(r"[.!?…]+$"));
    let t = replace1(&END, text.trim(), "");
    hit(&YES, &t) && !hit(&NO, &t)
}

/// "Stop" while Bobby speaks (see hear). Whole sentence only: "stop la musique" is a music command.
pub static STOP: LazyLock<Regex> = LazyLock::new(|| {
    re_i(r"^(?:(?:bobby|ok|okay|bon|non|euh|allez|hey)[\s,.!?]*)*(?:stop|arr[eê]te(?:[- ]toi)?|chut|tais[- ]toi|c['’]est bon|[çc]a suffit|merci|shut up|be quiet|quiet|enough|that['’]s enough|thanks|thank you|that['’]s fine)(?:[\s,.!?]*(?:stop|bobby|merci|thanks|c['’]est bon|[çc]a suffit))*[\s,.!?…]*$")
});

// Polite wrappers around a command ("est-ce que tu peux mute Kévin s'il te plaît").
pub static LEAD: LazyLock<Regex> = LazyLock::new(|| re(r"^(?:(?:est-ce que |est ce que )?(?:tu peux|tu pourrais|peux-tu|peux tu|pourrais-tu|pourrais tu)|vas-y|vas y|can you|could you|would you|please)[\s,]+"));
pub static TRAIL: LazyLock<Regex> = LazyLock::new(|| re(r"[\s,]+(?:stp|svp|s'il te pla[iî]t|s'il vous pla[iî]t|please)$"));

pub fn clean(query: &str) -> String {
    static PUNCT: LazyLock<Regex> = LazyLock::new(|| re(r"[.!?…]+"));
    let q = query.to_lowercase().replace(['’', '`'], "'");
    let q = replace_all(&PUNCT, &q, " ");
    let q = q.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut q = q.strip_suffix(',').unwrap_or(&q).to_string();
    loop {
        let prev = q.clone();
        q = replace1(&TRAIL, &replace1(&LEAD, &q, ""), "").trim().to_string();
        if prev == q {
            return q;
        }
    }
}

/// Simple commands without the LLM (saves ~1 s). Anything more complex returns None and goes to the LLM.
pub fn quick(query: &str) -> Option<Vec<Value>> {
    static MANY: LazyLock<Regex> = LazyLock::new(|| re(r" et | puis |sauf| and | then |except|,"));
    static UNDO: LazyLock<Regex> = LazyLock::new(|| re(r"^(?:annule(?: ça)?|undo(?: that)?)$"));
    for m in modules() {
        if let Some(e) = m.early.and_then(|f| f(query)) {
            return Some(e);
        }
    }
    let q = clean(query);
    // several requests, people or exclusions: leave it to the LLM
    if hit(&MANY, &q) {
        return None;
    }
    for m in modules() {
        for (r, make) in &m.quick {
            if let Some(c) = caps(r, &q) {
                let v = make(&c);
                return if v.is_empty() { None } else { Some(v) };
            }
        }
    }
    if hit(&UNDO, &q) {
        return Some(vec![json!({ "type": "undo" })]);
    }
    None
}

// A sentence ending in "annule" / "laisse tomber" cancels the whole request; "annule" alone (or "non annule") is undo.
pub static FILLER: LazyLock<Regex> = LazyLock::new(|| re(r"^(?:non|euh|ah|bon|ok|bobby|en fait|no|oh)$"));

pub fn cancels(query: &str) -> bool {
    static END: LazyLock<Regex> = LazyLock::new(|| re(r"^(.*?)\s*\b(?:annule|laisse tomber|cancel|never mind|nevermind|forget it)$"));
    let q = clean(query).replace(',', " ");
    let Some(m) = caps(&END, &q) else { return false };
    group(&m, 1).split_whitespace().any(|w| !hit(&FILLER, w))
}

// "Réessaie" / "refais" replays the speaker's own last request, else the conversation's last one.
// "Recommence" stays music replay.
pub static RETRY: LazyLock<Regex> = LazyLock::new(|| re_i(r"^(?:(?:bobby|vas-y|allez)[\s,]+)*(?:réessaie|réessaye|reessaie|essaie encore|essaye encore|refais(?:-le| le| ça)?|try again|retry)[\s.!?]*$"));

pub fn last_of(list: &[Hist]) -> Option<&str> {
    list.iter().rev().find(|h| !h.confirm && !hit(&RETRY, &h.query)).map(|h| h.query.as_str())
}

/// The query to run: the last request when `query` is a "réessaie", else `query` itself (`all` = `mine` in Node's default).
pub fn retry(query: &str, mine: &[Hist], all: &[Hist]) -> String {
    if hit(&RETRY, query) { last_of(mine).or_else(|| last_of(all)).unwrap_or(query).to_string() } else { query.to_string() }
}

/// "réessaie", "écris-le": the speaker's own last request.
pub fn mine_of(history: &[Hist], id: &str) -> Vec<Hist> {
    history.iter().filter(|h| h.id.as_deref().is_none_or(|i| i.is_empty() || i == id)).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::pc::set_apps;
    use crate::resolve::resolve;
    use crate::store::Store;
    use crate::testing::ctx;

    fn apps() {
        set_apps(vec!["Valorant".into(), "Claude".into(), "ChatGPT".into()]);
    }

    // ---------- LLM-free shortcuts: what they take must be right, what they leave goes to the LLM (None) ----------
    #[test]
    fn quick_cases() {
        apps();
        let (ctx, store) = (ctx(), Store::temp());
        let fmt = |plan: &[crate::types::Step]| {
            plan.iter()
                .map(|s| {
                    let mut names: Vec<&str> = s.targets.iter().flatten().map(|t| t.name.as_str()).collect();
                    names.sort();
                    format!("{}:{}{}", s.kind, names.join(","), s.channel.as_ref().map(|c| format!(">{}", c.name)).unwrap_or_default())
                })
                .collect::<Vec<_>>()
                .join(" | ")
        };
        let cases: Vec<(&str, Option<&str>)> = vec![
            ("Coupe le micro de Kévin.", Some("mute:Kévin")),
            ("mute ziklone", Some("mute:Zyklon")),
            ("rends le micro à Kevin", Some("unmute:Kévin")),
            ("coupe le son de Marie", Some("deafen:Marie")),
            ("déplace Zyklon dans gaming", Some("move:Zyklon>Gaming")),
            ("envoie Lucas en AFK", Some("move:Lucas>AFK")),
            ("mets-moi dans la salle de réunion", Some("move:Alex>Réunion")),
            ("annule", Some("undo:")),
            ("coupe le micro de Zyklon et Kévin", None),
            ("déplace Kévin dans AFK et coupe son micro", None),
            ("raconte-moi une blague", None),
            ("Mute Zyklon.", Some("mute:Zyklon")),
            ("deafen marie", Some("deafen:Marie")),
            ("move Lucas to the AFK channel", Some("move:Lucas>AFK")),
            ("move me to gaming", Some("move:Alex>Gaming")),
            ("undo that", Some("undo:")),
            ("mute Zyklon and Kevin", None),
            ("mute le grec", Some("mute:Zyklon")),
            // Missed on 2026-11-12 (journal): polite fillers, "dé-mute", "draguer"
            ("est-ce que tu peux kick Sorciax s'il te plaît ?", Some("disconnect:Sorciax")),
            ("des mute Ilias", Some("unmute:Prime Ilias")),
            ("dé-mute WakandeX", Some("unmute:WakandeX")),
            ("démute Kévin", Some("unmute:Kévin")),
            ("peux-tu draguer Sorciax ?", Some("move:Sorciax>Général")),
            ("est-ce que tu peux draguer Vinice ?", Some("move:Sorciax>Général")),
            ("vas-y mute moi", Some("mute:Alex")),
            ("can you mute Prime Ilias please?", Some("mute:Prime Ilias")),
            ("Mute Bobby", None),
            ("quelle heure est-il ?", Some("time:")),
            ("il est quelle heure ?", Some("time:")),
            ("est-ce que tu peux me lister les features qu'on t'a déjà implémentées ?", Some("help:")),
            ("qu'est-ce que tu sais faire", Some("help:")),
            ("est-ce que tu peux me mute micro et casque s'il te plaît ?", None),
            // Nicknames
            ("qui a quel surnom ?", Some("alias_list:")),
            ("who has which nicknames", Some("alias_list:")),
            ("renomme le surnom le Grec en le Spartiate", Some("alias_rename:")),
            ("rename the nickname le grec to the spartan", Some("alias_rename:")),
            ("supprime le surnom le Grec", Some("alias_remove:")),
            ("delete the nickname le grec", Some("alias_remove:")),
            // Rights by voice (super admin)
            ("enlève les permissions de Lucas", Some("perm_revoke:Lucas")),
            ("redonne les permissions à Lucas", Some("perm_restore:Lucas")),
            ("passe Marie admin", Some("admin_add:Marie")),
            ("retire l'admin à Marie", Some("admin_remove:Marie")),
        ];
        for (phrase, want) in cases {
            // name not found: falls back to the LLM
            let got = quick(phrase).and_then(|a| resolve(&a, &ctx, &store).ok()).map(|p| fmt(&p));
            assert_eq!(got.as_deref(), want, "shortcut: \"{phrase}\"");
        }
    }

    // ---------- A sentence ending in "annule" cancels the whole request ----------
    #[test]
    fn cancel() {
        assert!(cancels("est-ce que tu peux mettre un surnom sur Sorciax et que ça... Bobby, Godmode, annule !"));
        assert!(cancels("mute Zyklon, laisse tomber"));
        assert!(cancels("move Lucas to gaming, never mind"));
        assert!(!cancels("annule"));
        assert!(!cancels("non annule"));
        assert!(!cancels("laisse tomber, reviens en arrière"));
        assert!(!cancels("annule ça"));
    }

    // ---------- Confirmation: a short yes, never a sentence that only contains "ok" ----------
    #[test]
    fn confirm() {
        for t in ["oui", "Oui !", "je confirme", "ok vas-y", "yes", "Bobby, oui"] {
            assert!(confirms(t), "{t}");
        }
        for t in ["non, pas ok", "ok oui go mais attends", "tu crois que c'est ok de faire ça demain soir", "non", "oui non annule"] {
            assert!(!confirms(t), "{t}");
        }
    }

    #[test]
    fn pc_shortcuts() {
        apps();
        let cases = [
            ("mets pause sur le PC", "pc_media", "pause"), ("Mets la musique en pause sur Spotify.", "pc_media", "pause"), ("pause sur le PC", "pc_media", "pause"),
            ("pause the music on Spotify", "pc_media", "pause"), ("reprends la musique sur le PC", "pc_media", "play"),
            ("musique suivante sur Spotify", "pc_media", "next"), ("next song on youtube music", "pc_media", "next"), ("musique précédente sur le PC", "pc_media", "previous"),
            ("c'est quoi la musique sur le PC", "pc_music", ""), ("what's playing on the PC", "pc_music", ""),
            ("monte le son du PC", "pc_volume", "up"), ("baisse le volume Windows", "pc_volume", "down"), ("mets le son du PC à 30", "pc_volume", "30"),
            ("set the PC volume to 40 percent", "pc_volume", "40"), ("coupe le son du PC", "pc_volume", "mute"), ("remets le son du PC", "pc_volume", "unmute"),
            ("lance Valorant", "pc_launch", "valorant"), ("ouvre Claude", "pc_launch", "claude"), ("open ChatGPT", "pc_launch", "chatgpt"),
            ("verrouille le PC", "pc_lock", ""), ("mets le PC en veille", "pc_sleep", ""), ("éteins l'ordinateur", "pc_shutdown", ""),
            ("shut down the PC", "pc_shutdown", ""), ("température du GPU", "pc_gpu", ""), ("how hot is the gpu", "pc_gpu", ""),
        ];
        for (phrase, kind, text) in cases {
            let a = quick(phrase).unwrap_or_default();
            assert!(a.len() == 1 && a[0]["type"] == kind && a[0]["text"] == text, "PC shortcut: {phrase} -> {a:?}");
        }
        // Bobby's own Discord player owns these; the PC shortcuts must not take them.
        for phrase in ["pause", "skip", "suivant", "volume 50", "monte le son", "lance de la musique", "play despacito", "ouvre un truc inconnu xyz"] {
            assert!(!quick(phrase).unwrap_or_default().iter().any(|a| a["type"].as_str().unwrap().starts_with("pc_")), "not a PC shortcut: {phrase}");
        }
    }

    #[test]
    fn music_shortcuts() {
        apps();
        let cases = [
            ("pause", "music_pause", ""), ("mets pause", "music_pause", ""), ("mets la musique en pause", "music_pause", ""),
            ("reprends la musique", "music_resume", ""), ("resume", "music_resume", ""), ("skip", "music_skip", ""), ("suivant", "music_skip", ""),
            ("musique suivante", "music_skip", ""), ("next song", "music_skip", ""), ("stop la musique", "music_stop", ""), ("stop", "music_stop", ""),
            ("rejoue", "music_replay", ""), ("monte le son", "music_volume", "up"), ("baisse le volume", "music_volume", "down"),
            ("volume 50", "music_volume", "50"), ("mets le volume à 40", "music_volume", "40"), ("set the volume to 70", "music_volume", "70"),
            ("c'est quoi ce son", "music_now", ""), ("what's this song", "music_now", ""), ("c'est quoi la file d'attente", "music_queue", ""),
            ("lance de la musique", "music_play", "pop"), ("joue Bohemian Rhapsody", "music_play", "bohemian rhapsody"), ("play Daft Punk", "music_play", "daft punk"),
            ("joue du hardstyle", "music_play", "hardstyle"), ("mets une radio jazz", "music_play", "jazz"), ("play some lofi", "music_play", "lofi"),
        ];
        for (phrase, kind, text) in cases {
            let a = quick(phrase).unwrap_or_default();
            assert!(a.len() == 1 && a[0]["type"] == kind && a[0]["text"] == text, "music shortcut: {phrase} -> {a:?}");
        }
        // 2026-11-16: a style plays endlessly from YouTube; "radio" alone means a web radio; "que du X" = only X; "juste X" = X alone
        let qk = |t: &str| {
            let a = quick(t).unwrap();
            let s = |k: &str| a[0][k].as_str().unwrap_or("").to_string();
            (s("text"), s("kind"), s("mode"))
        };
        let want = |a: &str, b: &str, c: &str| (a.to_string(), b.to_string(), c.to_string());
        assert_eq!(qk("joue du hardstyle"), want("hardstyle", "style", ""));
        assert_eq!(qk("mets une radio jazz"), want("jazz", "radio", ""));
        assert_eq!(qk("play some lofi radio"), want("lofi", "radio", ""));
        assert_eq!(qk("joue-moi que du adèle castillon"), want("adèle castillon", "youtube", "artiste"));
        assert_eq!(qk("mets uniquement daft punk"), want("daft punk", "youtube", "artiste"));
        assert_eq!(qk("joue juste bohemian rhapsody"), want("bohemian rhapsody", "youtube", "unique"));
        assert_eq!(quick("joue Bohemian Rhapsody").unwrap()[0]["kind"], "youtube");
        for phrase in ["mets pause sur le PC", "monte le son du PC", "musique suivante sur Spotify", "lance Valorant"] {
            assert!(!quick(phrase).unwrap_or_default().iter().any(|a| a["type"].as_str().unwrap().starts_with("music_")), "not music: {phrase}");
        }
        assert_eq!(quick("mets pause sur le PC").unwrap()[0]["type"], "pc_media");
    }

    #[test]
    fn retry_last_request() {
        let h = |id: Option<&str>, query: &str, confirm: bool| Hist { id: id.map(String::from), query: query.into(), confirm, ..Default::default() };
        let all = vec![h(Some("a"), "mute Zyklon", false), h(Some("b"), "joue du jazz", false), h(Some("b"), "oui", true), h(Some("b"), "réessaie", false)];
        let mine = mine_of(&all, "a");
        assert_eq!(mine.len(), 1);
        assert_eq!(retry("Réessaie !", &mine, &all), "mute Zyklon");
        // nothing of mine: the conversation's last request
        assert_eq!(retry("bobby, refais ça", &mine_of(&all, "z"), &all), "joue du jazz");
        assert_eq!(retry("réessaie", &[], &[]), "réessaie");
        assert_eq!(retry("mute Kévin", &mine, &all), "mute Kévin");
        assert!(hit(&STOP, "Ok, stop, merci !") && !hit(&STOP, "stop la musique"));
    }
}
