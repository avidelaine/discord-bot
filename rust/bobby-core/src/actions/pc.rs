//! Port of `bot/actions/pc.js`: static texts, action definitions, shortcuts. The PC client owns the index of
//! games and apps and every side effect (media, volume, launch, close, lock, sleep, shutdown, GPU); the brain
//! only caches the app names the client reports, to decide whether "ouvre X" is really an app.
use crate::types::{ActionDef, Echo, Module, QuickFn, Role, SayFn, Step};
use crate::util::{group, hit, re, replace1, sim};
use fancy_regex::Regex;
use serde_json::{json, Value};
use std::sync::{LazyLock, RwLock};

/// Names of the launchable games and apps, in the client's order (games first), see `Cap::PcApps`.
static APPS: RwLock<Vec<String>> = RwLock::new(Vec::new());

pub fn set_apps(names: Vec<String>) {
    *APPS.write().unwrap_or_else(|e| e.into_inner()) = names;
}

// Best entry for a spoken name; games come first so they win ties, then shorter names.
static ART: LazyLock<Regex> = LazyLock::new(|| re(r"^(?:le |la |les |l'|the |un |une )+|^(?:jeu|game|app|application)\s+"));

fn find(q: &str) -> Option<String> {
    let lower = q.to_lowercase();
    let name = replace1(&ART, &replace1(&ART, &lower, ""), "");
    let apps = APPS.read().unwrap_or_else(|e| e.into_inner());
    let mut best: Option<(f64, usize, &String)> = None;
    for a in apps.iter() {
        let s = sim(&name, a);
        if s < 0.75 {
            continue;
        }
        let len = a.encode_utf16().count();
        // best score, then shorter name; the first of equals stays (stable sort)
        if best.is_none_or(|(bs, bl, _)| s > bs || (s == bs && len < bl)) {
            best = Some((s, len, a));
        }
    }
    best.map(|(_, _, a)| a.clone())
}

fn flag_fail(s: &Step) -> bool {
    s.flag("fail")
}

const OFF: [&str; 2] = ["Je n'arrive pas à joindre le service du PC.", "I can't reach the PC service."];

/// JS template rendering of a value.
fn js(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "undefined".into(),
        other => other.to_string(),
    }
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

// A step that may fail marks s.fail instead of throwing, so Bobby says it out loud.
fn say(s: &Step, lang: &str, f: fn(&Step, &Value, bool) -> String) -> Option<String> {
    let fr = lang != "en";
    if flag_fail(s) {
        return Some(OFF[usize::from(!fr)].to_string());
    }
    let null = Value::Null;
    Some(f(s, s.f.get("res").unwrap_or(&null), fr))
}

fn media(text: &str) -> [&'static str; 2] {
    match text {
        "play" => ["Je relance la musique.", "Resuming."],
        "pause" => ["Je mets en pause.", "Paused."],
        "next" => ["Musique suivante.", "Next track."],
        "previous" => ["Musique précédente.", "Previous track."],
        _ => ["", ""],
    }
}

// "on the PC" / "sur Spotify": explicit target, so Bobby's own Discord player can keep "pause", "skip", "volume".
const ON: &str = "(?: (?:sur|on) (?:le pc|the pc|spotify|youtube music))";
const VOL: &str = "(?:(?:le )?(?:son|volume) (?:du pc|de windows|windows|sur le pc)|the (?:pc|windows) (?:volume|sound))";

fn act(kind: &str, text: &str) -> Vec<Value> {
    vec![json!({ "type": kind, "text": text })]
}

fn q(r: Regex, f: QuickFn) -> (Regex, QuickFn) {
    (r, f)
}

fn sf(f: SayFn) -> Option<SayFn> {
    Some(f)
}

fn sup() -> ActionDef {
    ActionDef { role: Role::Super, ..Default::default() }
}

pub fn module() -> Module {
    Module {
        name: "pc",
        help: [
            "piloter le PC (musique, volume, lancer et fermer des jeux et applications, verrouiller, veille, éteindre)",
            "control the PC (music, volume, launch and close games and apps, lock, sleep, shut down)",
        ],
        short: ["le PC", "the PC"],
        prompt: vec![
            r##"- pc_media : contrôler la musique du PC (Spotify, YouTube Music, navigateur), "text" vaut play, pause, next ou previous"##,
            r##"- pc_music : dire quelle musique joue sur le PC"##,
            r##"- pc_volume : volume de Windows (le PC, pas Discord), "text" vaut up, down, mute, unmute ou un nombre de 0 à 100"##,
            r##"- pc_launch : lancer le jeu ou l'application nommé dans "text" (Valorant, Steam, Epic, Claude, ChatGPT...)"##,
            r##"- pc_close : fermer le jeu ou l'application nommé dans "text" ("ferme Apex", "quitte Spotify")"##,
            r##"- pc_lock / pc_sleep / pc_shutdown : verrouiller, mettre en veille ou éteindre le PC"##,
            r##"- pc_gpu : température, charge et mémoire de la carte graphique (GPU) du PC, en français ou en anglais ("graphics card")"##,
        ],
        rules: vec![
            r##"- pc_media et pc_volume seulement si on parle du PC, de Windows, de Spotify ou de YouTube Music. "lance" ou "ouvre" suivi d'un jeu ou d'une application, c'est pc_launch. "ferme", "quitte" ou "close" suivi d'un jeu ou d'une application, c'est pc_close, jamais pc_launch."##,
        ],
        examples: vec![
            r##""what is my GPU temperature" -> {"actions":[{"type":"pc_gpu"}],"say":""}"##,
            r##""mets la musique en pause sur le PC" -> {"actions":[{"type":"pc_media","text":"pause"}],"say":""}"##,
            r##""passe à la musique suivante sur Spotify" -> {"actions":[{"type":"pc_media","text":"next"}],"say":""}"##,
            r##""baisse le son du PC à 30" -> {"actions":[{"type":"pc_volume","text":"30"}],"say":""}"##,
            r##""ouvre ChatGPT" -> {"actions":[{"type":"pc_launch","text":"ChatGPT"}],"say":""}"##,
            r##""launch Valorant" -> {"actions":[{"type":"pc_launch","text":"Valorant"}],"say":""}"##,
            r##""ferme Apex Legends" -> {"actions":[{"type":"pc_close","text":"Apex Legends"}],"say":""}"##,
            r##""mets le PC en veille" -> {"actions":[{"type":"pc_sleep"}],"say":""}"##,
            r##""what song is playing on the PC" -> {"actions":[{"type":"pc_music"}],"say":""}"##,
            r##""mute the PC volume" -> {"actions":[{"type":"pc_volume","text":"mute"}],"say":""}"##,
        ],
        fields: vec![],
        early: None,
        quick: vec![
            q(re(&format!(r"^(?:mets? (?:la musique )?en pause|mets? pause|pause(?: la musique| the music)?){ON}$")), |_| act("pc_media", "pause")),
            q(re(&format!(r"^(?:(?:reprends|relance|remets) la musique|resume (?:the )?music){ON}$")), |_| act("pc_media", "play")),
            q(re(&format!(r"^(?:(?:musique|morceau|titre|chanson) suivante?|prochaine (?:musique|chanson)){ON}$|^next (?:song|track){ON}$")), |_| act("pc_media", "next")),
            q(re(&format!(r"^(?:(?:musique|morceau|titre|chanson) précédente?){ON}$|^(?:previous|last) (?:song|track){ON}$")), |_| act("pc_media", "previous")),
            q(re(r"^(?:c'est quoi|quelle est|quel est|qu'est-ce qui (?:joue|passe)) (?:la |le )?(?:musique|morceau|titre|chanson)(?: qui (?:joue|passe))? (?:sur|du) (?:le )?pc$|^what(?:'s| is) (?:the )?(?:music |song |track )?playing (?:on|from) (?:the )?pc$|^what(?:'s| is) (?:the )?(?:music|song|track) (?:on|from) (?:the )?pc$"), |_| act("pc_music", "")),
            q(re(&format!(r"^(monte|augmente|baisse|diminue|descends|turn up|raise|increase|turn down|lower|decrease|reduce) {VOL}$")), |m| {
                act("pc_volume", if matches!(group(m, 1), "monte" | "augmente" | "turn up" | "raise" | "increase") { "up" } else { "down" })
            }),
            q(re(&format!(r"^(?:mets|met|règle|regle) {VOL} (?:à|a|sur) (\d+)(?: ?%| pour cent| percent)?$|^set {VOL} to (\d+)(?: ?%| percent)?$")), |m| {
                act("pc_volume", if group(m, 1).is_empty() { group(m, 2) } else { group(m, 1) })
            }),
            q(re(&format!(r"^(?:coupe|mute|mets en sourdine) {VOL}$")), |_| act("pc_volume", "mute")),
            q(re(&format!(r"^(?:remets|rétablis|réactive|redonne|unmute|démute) {VOL}$")), |_| act("pc_volume", "unmute")),
            // Only taken when the name is really a game or app on this PC, so "lance de la musique" and the like go elsewhere.
            q(re(r"^(?:lance|ouvre|démarre|demarre|launch|open|start) (?!(?:de la |la |une? |some |the )?(?:musique|chanson|music|song|playlist|morceau))(.+)$"), |m| {
                if find(group(m, 1)).is_some() { act("pc_launch", group(m, 1)) } else { vec![] }
            }),
            q(re(r"^(?:ferme|quitte|close|quit|kill) (?!(?:le |la |the )?(?:salon|vocal|channel|voice|musique|music|radio|micro|mic|son)\b)(.+)$"), |m| act("pc_close", group(m, 1))),
            q(re(r"^(?:verrouille|bloque) (?:le pc|l'ordi(?:nateur)?|mon pc|la session)$|^lock (?:the |my )?pc$"), |_| act("pc_lock", "")),
            q(re(r"^mets? (?:le pc|l'ordi(?:nateur)?|mon pc) en (?:veille|sommeil)$|^(?:put|set) (?:the |my )?pc (?:to sleep|in sleep mode)$"), |_| act("pc_sleep", "")),
            q(re(r"^(?:éteins|eteins|arrête|arrete|shut ?down|turn off|shut down) (?:le pc|l'ordi(?:nateur)?|mon pc|the pc|my pc)$"), |_| act("pc_shutdown", "")),
            q(re(r"^(?:c'est quoi |quelle est )?(?:la |l')?(?:température|temperature|charge|état|etat) (?:du|de la) (?:gpu|carte graphique)$|^(?:comment va|how'?s) (?:le|la|the) (?:gpu|carte graphique)$|^(?:gpu|graphics card) (?:status|temp(?:erature)?|usage)$|^how hot is (?:the )?gpu$"), |_| act("pc_gpu", "")),
        ],
        prompt_lines: None,
        actions: vec![
            (
                "pc_media",
                // done silently after the ack (see brain.js report)
                ActionDef { text: true, echo: Echo::If(|s| !flag_fail(s)), say: sf(|s, _, l| say(s, l, |s, _, fr| media(&s.text)[usize::from(!fr)].to_string())), ..sup() },
            ),
            (
                "pc_music",
                ActionDef {
                    say: sf(|s, _, l| {
                        say(s, l, |_, r, fr| {
                            let title = js(&r["title"]);
                            let has_artist = truthy(&r["artist"]);
                            let artist = js(&r["artist"]);
                            let playing = truthy(&r["playing"]);
                            if !truthy(&r["title"]) {
                                return if fr { "Rien ne joue sur le PC.".into() } else { "Nothing is playing on the PC.".into() };
                            }
                            if fr {
                                format!("{} : {title}{}.", if playing { "Sur le PC" } else { "En pause sur le PC" }, if has_artist { format!(", de {artist}") } else { String::new() })
                            } else {
                                format!("{}: {title}{}.", if playing { "On the PC" } else { "Paused on the PC" }, if has_artist { format!(" by {artist}") } else { String::new() })
                            }
                        })
                    }),
                    ..sup()
                },
            ),
            (
                "pc_volume",
                ActionDef {
                    text: true,
                    echo: Echo::If(|s| !flag_fail(s)),
                    say: sf(|s, _, l| {
                        say(s, l, |s, r, fr| {
                            let level = if truthy(&r["level"]) || r["level"] == 0 { js(&r["level"]) } else { s.text.clone() };
                            match (truthy(&r["muted"]), fr) {
                                (true, true) => "Le PC est coupé.".into(),
                                (true, false) => "The PC is muted.".into(),
                                (false, true) => format!("Volume du PC à {level}."),
                                (false, false) => format!("PC volume at {level}."),
                            }
                        })
                    }),
                    ..sup()
                },
            ),
            (
                "pc_launch",
                ActionDef {
                    text: true,
                    say: sf(|s, _, l| {
                        let f = s.str("found").filter(|f| !f.is_empty());
                        Some(match (f, l == "en") {
                            (Some(f), false) => format!("Je lance {f}."),
                            (Some(f), true) => format!("Launching {f}."),
                            (None, false) => format!("Je ne trouve pas « {} » sur le PC.", s.text),
                            (None, true) => format!("I can't find \"{}\" on the PC.", s.text),
                        })
                    }),
                    ..sup()
                },
            ),
            (
                "pc_close",
                ActionDef {
                    text: true,
                    say: sf(|s, _, l| {
                        say(s, l, |s, _, fr| {
                            let f = s.str("found").filter(|f| !f.is_empty());
                            match (f, fr) {
                                (Some(f), true) => format!("Je ferme {f}."),
                                (Some(f), false) => format!("Closing {f}."),
                                (None, true) => format!("Je ne trouve pas « {} » ouvert sur le PC.", s.text),
                                (None, false) => format!("I can't find \"{}\" open on the PC.", s.text),
                            }
                        })
                    }),
                    ..sup()
                },
            ),
            ("pc_lock", ActionDef { confirm: true, say: sf(|_, _, l| Some(if l == "en" { "Locking the PC.".into() } else { "Je verrouille le PC.".into() })), ..sup() }),
            // ponytail: SetSuspendState gives real sleep (rundll32 powrprof would hibernate if hibernation is enabled).
            ("pc_sleep", ActionDef { confirm: true, say: sf(|_, _, l| Some(if l == "en" { "Putting the PC to sleep.".into() } else { "Je mets le PC en veille.".into() })), ..sup() }),
            ("pc_shutdown", ActionDef { confirm: true, say: sf(|_, _, l| Some(if l == "en" { "Shutting down the PC.".into() } else { "J'éteins le PC.".into() })), ..sup() }),
            (
                "pc_gpu",
                ActionDef {
                    say: sf(|s, _, l| {
                        let fr = l != "en";
                        if flag_fail(s) {
                            return Some(if fr { "Je ne peux pas lire la carte graphique.".into() } else { "I can't read the graphics card.".into() });
                        }
                        let null = Value::Null;
                        let r = s.f.get("res").unwrap_or(&null);
                        let (name, temp, load, used, total) = (js(&r["name"]), js(&r["temp"]), js(&r["load"]), js(&r["used"]), js(&r["total"]));
                        Some(if fr {
                            format!("{name} : {temp} degrés, {load} % de charge, {used} Go de mémoire sur {total}.")
                        } else {
                            format!("{name}: {temp} degrees, {load}% load, {used} of {total} GB memory.")
                        })
                    }),
                    ..sup()
                },
            ),
        ],
    }
}

/// Test helpers shared with the music tests: Node's `brain.js` `clean()` + `quick()` over discord, music and pc.
#[cfg(test)]
pub(crate) mod testkit {
    use super::*;
    use crate::actions::{discord, music};

    static MODS: LazyLock<Vec<Module>> = LazyLock::new(|| vec![discord::module(), music::module(), module()]);
    static LEAD: LazyLock<Regex> = LazyLock::new(|| re(r"^(?:(?:est-ce que |est ce que )?(?:tu peux|tu pourrais|peux-tu|peux tu|pourrais-tu|pourrais tu)|vas-y|vas y|can you|could you|would you|please)[\s,]+"));
    static TRAIL: LazyLock<Regex> = LazyLock::new(|| re(r"[\s,]+(?:stp|svp|s'il te pla[iî]t|s'il vous pla[iî]t|please)$"));
    static PUNCT: LazyLock<Regex> = LazyLock::new(|| re(r"[.!?…]+"));
    static MANY: LazyLock<Regex> = LazyLock::new(|| re(r" et | puis |sauf| and | then |except|,"));

    pub fn set_test_apps() {
        set_apps(vec!["Valorant".into(), "Claude".into(), "ChatGPT".into()]);
    }

    pub fn clean(query: &str) -> String {
        let q = query.to_lowercase().replace(['’', '`'], "'");
        let q = crate::util::replace_all(&PUNCT, &q, " ");
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

    pub fn quick(query: &str) -> Option<Vec<Value>> {
        for m in MODS.iter() {
            if let Some(e) = m.early.and_then(|f| f(query)) {
                return Some(e);
            }
        }
        let q = clean(query);
        if hit(&MANY, &q) {
            return None;
        }
        for m in MODS.iter() {
            for (r, make) in &m.quick {
                if let Some(c) = crate::util::caps(r, &q) {
                    let v = make(&c);
                    return if v.is_empty() { None } else { Some(v) };
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::{quick, set_test_apps};
    use super::*;

    #[test]
    fn quick_regexes_compile() {
        let m = module();
        assert_eq!(m.quick.len(), 15);
        assert_eq!(m.actions.len(), 9);
    }

    #[test]
    fn pc_cases() {
        set_test_apps();
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
        for t in ["pc_lock", "pc_sleep", "pc_shutdown"] {
            assert!(module().actions.iter().find(|(k, _)| *k == t).unwrap().1.confirm, "{t} needs confirm");
        }
        assert!(module().actions.iter().all(|(_, d)| d.role == Role::Super));
    }

    #[test]
    fn say_lines() {
        let m = module();
        let get = |k: &str| m.actions.iter().find(|(t, _)| *t == k).unwrap().1.say.unwrap();
        let mut s = Step { text: "pause".into(), ..Default::default() };
        assert_eq!(get("pc_media")(&s, None, "fr").unwrap(), "Je mets en pause.");
        assert_eq!(get("pc_media")(&s, None, "en").unwrap(), "Paused.");
        s.set("res", json!({ "title": "Song", "artist": "Band", "playing": true }));
        assert_eq!(get("pc_music")(&s, None, "fr").unwrap(), "Sur le PC : Song, de Band.");
        assert_eq!(get("pc_music")(&s, None, "en").unwrap(), "On the PC: Song by Band.");
        s.set("fail", true);
        assert_eq!(get("pc_volume")(&s, None, "en").unwrap(), "I can't reach the PC service.");
    }
}
