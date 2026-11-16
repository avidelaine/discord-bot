//! Port of `bot/actions/info.js`: static texts, action definitions, shortcuts, and the "what can I do"
//! helpers. `run`, `facts`, `aboutMe` and the capabilities card are batch R3b.
use crate::types::{ActionDef, ChanKind, Echo, Module, Role, Step};
use crate::util::{group, re};
use chrono::{DateTime, Datelike, Local, Timelike};
use serde_json::{json, Value};

pub const FR_DAYS: [&str; 7] = ["lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche"];
pub const FR_MONTHS: [&str; 12] = [
    "janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre", "décembre",
];

/// Each action module describes itself in "help"; the list follows whatever modules exist.
pub fn capabilities(lang: &str) -> String {
    let i = usize::from(lang == "en");
    crate::actions::modules().iter().map(|m| m.help[i]).filter(|h| !h.is_empty()).collect::<Vec<_>>().join(", ")
}

/// Lowest role that can use a module, read-only answers ("info" actions) aside.
pub fn module_role(m: &Module) -> Option<Role> {
    m.actions.iter().filter(|(_, a)| !a.info).map(|(_, a)| a.role).min()
}

/// What a role may do, from the roles the action modules declare: "la musique, les listes de tâches, ..."
pub fn abilities(role: Role, lang: &str) -> String {
    let i = usize::from(lang == "en");
    crate::actions::modules()
        .iter()
        .filter(|m| (!m.short[0].is_empty() || !m.short[1].is_empty()) && module_role(m).is_some_and(|r| r <= role))
        .map(|m| m.short[i])
        .collect::<Vec<_>>()
        .join(", ")
}

/// `now(lang)` of info.js: fr-FR "lundi 16 novembre à 19:05", en-GB "Monday 16 November at 19:05".
fn now(lang: &str) -> String {
    let t: DateTime<Local> = Local::now();
    if lang == "en" {
        t.format("%A %-d %B at %H:%M").to_string()
    } else {
        let d = FR_DAYS[t.weekday().num_days_from_monday() as usize];
        format!("{d} {} {} à {:02}:{:02}", t.day(), FR_MONTHS[t.month0() as usize], t.hour(), t.minute())
    }
}

fn say_time(_: &Step, _: Option<&str>, lang: &str) -> Option<String> {
    Some(if lang == "en" { format!("It's {}.", now("en")) } else { format!("Nous sommes {}.", now("fr")) })
}

fn say_help(_: &Step, _: Option<&str>, lang: &str) -> Option<String> {
    Some(if lang == "en" { format!("I can {}.", capabilities("en")) } else { format!("Je sais {}.", capabilities("fr")) })
}

fn say_come(s: &Step, _: Option<&str>, lang: &str) -> Option<String> {
    let name = s.channel.as_ref().map_or("undefined", |c| c.name.as_str());
    Some(if lang == "en" { format!("Coming to {name}.") } else { format!("J'arrive dans {name}.") })
}

fn say_answer(s: &Step, _: Option<&str>, _: &str) -> Option<String> {
    s.str("answer").map(str::to_string)
}

pub fn module() -> Module {
    Module {
        name: "info",
        help: ["dire l'heure, venir dans ton salon vocal et répondre aux questions sur moi", "tell the time, come to your voice channel and answer questions about me"],
        short: ["l'heure, me faire venir et les questions sur moi", "the time, calling me over and questions about me"],
        prompt: vec![
            "- time : dire la date et l'heure",
            "- help : dire ce que tu sais faire (lister tes fonctionnalités, ce qu'on t'a implémenté)",
            "- come : Bobby (toi) vient dans le salon vocal \"channel\" (\"ici\" = celui de celui qui parle) ; \"connecte-toi sur Discord\" = come ici",
            "- about : question ou remarque sur Bobby (toi), tes réglages, ta voix, ton mot d'activation, qui a quels droits, les gens du serveur, ou la conversation en cours ; \"text\" = la phrase complète",
        ],
        rules: vec!["- \"viens\", \"rejoins-nous\", \"come here\", \"déplace-toi dans X\" : come (Bobby vient seul). Déplacer les autres : move."],
        examples: vec![
            r#""il est quelle heure" -> {"actions":[{"type":"time"}],"say":""}"#,
            r#""qu'est-ce que tu sais faire" -> {"actions":[{"type":"help"}],"say":""}"#,
            r#""liste-moi tes fonctionnalités" -> {"actions":[{"type":"help"}],"say":""}"#,
            r#""viens dans Gaming" -> {"actions":[{"type":"come","channel":"Gaming"}],"say":""}"#,
            r#""rejoins-nous" -> {"actions":[{"type":"come","channel":"Général"}],"say":""}"#,
            r#""how are you doing" -> {"actions":[],"say":"I'm doing great, thanks!"}"#,
            r#""t'es qui toi" -> {"actions":[{"type":"about","text":"t'es qui toi"}],"say":""}"#,
            r#""est-ce que tu es meilleur que Siri" -> {"actions":[{"type":"about","text":"est-ce que tu es meilleur que Siri"}],"say":""}"#,
            r#""ta voix est bizarre" -> {"actions":[{"type":"about","text":"ta voix est bizarre"}],"say":""}"#,
            r#""pourquoi tu n'as pas compris tout à l'heure" -> {"actions":[{"type":"about","text":"pourquoi tu n'as pas compris tout à l'heure"}],"say":""}"#,
        ],
        fields: vec![],
        early: None,
        quick: vec![
            (re(r"^(?:quelle heure (?:est-il|il est|est il)|il est quelle heure|what time is it|what's the time)$"), |_| vec![json!({ "type": "time" })]),
            // "connecte-toi sur Discord" (said on the PC, local mode): Bobby joins the speaker's voice channel
            (re(r"^(?:connecte-toi|connecte toi|reconnecte-toi|va|passe|rejoins|retourne|reviens|go|connect|join)(?: (?:sur|à|a|on|to))? discord$"), |_| vec![json!({ "type": "come", "channel": "ici" })]),
            (re(r"^(?:viens|ram[eè]ne-toi|rejoins(?:[- ](?:nous|moi))?|come(?: over)?|join (?:us|me))(?: (?:ici|here|là|la))?$"), |_| vec![json!({ "type": "come", "channel": "ici" })]),
            (re(r"^(?:viens|rejoins(?:[- ](?:nous|moi))?|déplace-toi|come|join(?: us| me)?) (?:dans|en|sur|to|in|into|over to) (?:le salon |la salle (?:de )?|the )?(.+?)(?: channel)?$"), |m| vec![json!({ "type": "come", "channel": group(m, 1) })]),
            (re(r"\blist\w* (?:moi )?(?:tes |les |your )?(?:features|fonctionnalit|commandes|capacit|commands)|ce que tu sais faire|que sais-tu faire|tu sais faire quoi|what can you do"), |_| vec![json!({ "type": "help" })]),
        ],
        prompt_lines: None,
        actions: vec![
            ("time", ActionDef { role: Role::Prime, say: Some(say_time), ..Default::default() }),
            ("help", ActionDef { role: Role::Prime, say: Some(say_help), ..Default::default() }),
            // Bobby alone comes to a channel ("viens dans PRIME", "rejoins-nous"); moving other people stays admin.
            ("come", ActionDef {
                role: Role::Prime,
                channel: ChanKind::Voice,
                echo: Echo::Yes, // done silently after the ack (see brain.js report)
                normalize: Some(|a| {
                    if a.get("channel").and_then(Value::as_str).is_some_and(|c| !c.is_empty()) {
                        return None;
                    }
                    let mut o = a.clone();
                    o["channel"] = json!("ici");
                    Some(o)
                }),
                say: Some(say_come),
                ..Default::default()
            }),
            ("about", ActionDef { role: Role::Prime, text: true, say: Some(say_answer), ..Default::default() }),
        ],
    }
}


/// R3b: port of the module's `run` functions (one per action type).
pub async fn run<L: bobby_proto::infer::Understand + Sync>(_s: &mut crate::types::Step, _env: &super::Env<'_, L>, _lang: &str) -> crate::util::Res<Vec<super::UndoOp>> {
    Ok(vec![]) // R3b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::caps;

    fn quick(s: &str) -> Option<Vec<Value>> {
        module().quick.iter().find_map(|(r, f)| caps(r, s).map(|m| f(&m)))
    }

    #[test]
    fn shortcuts() {
        assert_eq!(quick("il est quelle heure"), Some(vec![json!({ "type": "time" })]));
        assert_eq!(quick("connecte-toi sur discord"), Some(vec![json!({ "type": "come", "channel": "ici" })]));
        assert_eq!(quick("rejoins-nous"), Some(vec![json!({ "type": "come", "channel": "ici" })]));
        assert_eq!(quick("viens dans le salon gaming"), Some(vec![json!({ "type": "come", "channel": "gaming" })]));
        assert_eq!(quick("what can you do"), Some(vec![json!({ "type": "help" })]));
        assert_eq!(quick("bonjour"), None);
    }

    #[test]
    fn roles_and_texts() {
        let m = module();
        let come = &m.actions.iter().find(|(k, _)| *k == "come").unwrap().1;
        let n = come.normalize.unwrap();
        assert_eq!(n(&json!({ "type": "come" })), Some(json!({ "type": "come", "channel": "ici" })));
        assert_eq!(n(&json!({ "type": "come", "channel": "Gaming" })), None);
        assert!(abilities(Role::Other, "fr").is_empty());
        assert!(abilities(Role::Prime, "fr").contains("l'heure"));
        assert!(capabilities("en").contains("tell the time"));
    }
}
