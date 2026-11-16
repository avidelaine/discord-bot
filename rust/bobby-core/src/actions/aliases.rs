//! Port of `bot/actions/aliases.js`: nicknames given by voice ("appelle Zyklon le Grec"), stored in the
//! document "aliases" (memberId -> [nickname]). Everything here is super admin only.
//! Texts, shortcuts, normalize and `say` lines; `run` and the nicknames card are batch R3b.
use crate::types::{ActionDef, Module, QuickFn, Role, SayFn, Step, Who};
use crate::util::{group, norm, re};
use fancy_regex::Regex;
use serde_json::{json, Value};

/// `n || s.owner`
fn who(s: &Step, n: Option<&str>) -> String {
    match n {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => s.str("owner").unwrap_or("").to_string(),
    }
}

fn not_found(s: &Step, lang: &str) -> String {
    let t = &s.text;
    if lang == "en" { format!("I can't find the nickname \"{t}\".") } else { format!("Je ne trouve pas le surnom « {t} ».") }
}

fn say_add(s: &Step, n: Option<&str>, lang: &str) -> Option<String> {
    let (n, t) = (n.unwrap_or(""), &s.text);
    Some(if lang == "en" { format!("{n} can now also be called \"{t}\".") } else { format!("{n} s'appelle aussi « {t} ».") })
}

fn say_remove(s: &Step, n: Option<&str>, lang: &str) -> Option<String> {
    if s.flag("missing") {
        return Some(not_found(s, lang));
    }
    let (w, t) = (who(s, n), &s.text);
    Some(if lang == "en" { format!("Forgetting \"{t}\" for {w}.") } else { format!("J'oublie « {t} » pour {w}.") })
}

fn say_rename(s: &Step, n: Option<&str>, lang: &str) -> Option<String> {
    if s.flag("missing") {
        return Some(not_found(s, lang));
    }
    let (w, t, to) = (who(s, n), &s.text, s.str("to").unwrap_or(""));
    Some(if lang == "en" { format!("\"{t}\" is now \"{to}\" for {w}.") } else { format!("« {t} » devient « {to} » pour {w}.") })
}

fn say_list(s: &Step, _: Option<&str>, lang: &str) -> Option<String> {
    let at = s.str("where").filter(|w| !w.is_empty());
    Some(match (at, lang == "en") {
        (Some(w), true) => format!("I put it in {w}."),
        (Some(w), false) => format!("Je l'ai mis dans {w}."),
        (None, true) => "Nobody has a nickname.".to_string(),
        (None, false) => "Personne n'a de surnom.".to_string(),
    })
}

/// "renomme Lucas en le Boss": the 4B model sometimes answers rename with text = to; that means giving a nickname.
fn normalize_rename(a: &Value) -> Option<Value> {
    let who = a.get("who").filter(|w| w.as_array().is_some_and(|l| !l.is_empty()) || w.as_str().is_some_and(|s| !s.is_empty()))?;
    let (to, text) = (a["to"].as_str().unwrap_or(""), a["text"].as_str().unwrap_or(""));
    if to.is_empty() || norm(to) == norm(text) {
        return Some(json!({ "type": "alias_add", "who": who, "text": if to.is_empty() { text } else { to } }));
    }
    None
}

fn q(r: Regex, f: QuickFn) -> (Regex, QuickFn) {
    (r, f)
}

pub fn module() -> Module {
    let sup = |who: Who, say: SayFn| ActionDef { role: Role::Super, who, one: true, text: true, say: Some(say), ..Default::default() };
    Module {
        name: "aliases",
        help: ["retenir, renommer, supprimer et lister les surnoms", "remember, rename, delete and list nicknames"],
        short: ["les surnoms", "nicknames"],
        prompt: vec![
            r#"- alias_add : donner à "who" (une seule personne) le surnom "text""#,
            r#"- alias_remove : retirer le surnom "text" ("who" facultatif : sans "who", on cherche chez tout le monde)"#,
            r#"- alias_rename : remplacer un surnom existant "text" par "to" ("who" facultatif). Jamais sans les mots "le surnom" (ou "the nickname") dans la phrase."#,
            r#"- alias_list : poster dans le salon texte la liste des surnoms de chaque personne, seulement si la phrase dit "surnom" (ou "nickname")"#,
        ],
        rules: vec![
            r#"- Chaque personne peut avoir des surnoms (listés dans le contexte) : un surnom désigne cette personne. "renommer X en Y" avec une personne X, c'est alias_add ; "renomme le surnom X en Y", c'est alias_rename. "X ne s'appelle plus Y", c'est alias_remove."#,
        ],
        examples: vec![
            r#""appelle Zyklon le Grec" -> {"actions":[{"type":"alias_add","who":["Zyklon"],"text":"le Grec"}],"say":""}"#,
            r#""Kévin ne s'appelle plus Kev" -> {"actions":[{"type":"alias_remove","who":["Kévin"],"text":"Kev"}],"say":""}"#,
            r#""renomme Lucas en le Boss" -> {"actions":[{"type":"alias_add","who":["Lucas"],"text":"le Boss"}],"say":""}"#,
            r#""tu peux renommer Marie en la Reine ?" -> {"actions":[{"type":"alias_add","who":["Marie"],"text":"la Reine"}],"say":""}"#,
            r#""coupe le micro du Rat" (le Rat = surnom de Lucas) -> {"actions":[{"type":"mute","who":["Lucas"]}],"say":""}"#,
            r#""renomme le surnom le Rat en le Spartiate" -> {"actions":[{"type":"alias_rename","text":"le Rat","to":"le Spartiate"}],"say":""}"#,
            r#""delete the nickname the rookie" -> {"actions":[{"type":"alias_remove","text":"the rookie"}],"say":""}"#,
            r#""qui a quel surnom" -> {"actions":[{"type":"alias_list"}],"say":""}"#,
        ],
        // merged into the LLM schema: new nickname of alias_rename
        fields: vec![("to", json!({ "type": "string" }))],
        early: None,
        // ponytail: shortcuts see the lowercased sentence, so nicknames set through them are stored in lowercase.
        quick: vec![
            q(re(r"^(?:qui a quels? surnoms?|(?:liste|montre|affiche|donne)(?:-moi)? (?:les|tous les|la liste des) surnoms|who has which nicknames?|(?:list|show)(?: all)?(?: the)? nicknames)$"), |_| vec![json!({ "type": "alias_list" })]),
            q(re(r"^renommer? (?!le surnom )(.+?) en (.+)$"), |m| vec![json!({ "type": "alias_add", "who": [group(m, 1)], "text": group(m, 2) })]),
            q(re(r"^renomme le surnom (.+?) en (.+)$"), |m| vec![json!({ "type": "alias_rename", "text": group(m, 1), "to": group(m, 2) })]),
            q(re(r"^rename(?: the)? nickname (.+?) (?:to|into) (.+)$"), |m| vec![json!({ "type": "alias_rename", "text": group(m, 1), "to": group(m, 2) })]),
            q(re(r"^(?:supprime|efface|retire|enlève) le surnom (.+)$"), |m| vec![json!({ "type": "alias_remove", "text": group(m, 1) })]),
            q(re(r"^(?:delete|remove) (?:the )?nickname (.+)$"), |m| vec![json!({ "type": "alias_remove", "text": group(m, 1) })]),
        ],
        prompt_lines: None,
        actions: vec![
            ("alias_add", sup(Who::Yes, say_add)),
            ("alias_remove", sup(Who::Optional, say_remove)),
            ("alias_rename", ActionDef { normalize: Some(normalize_rename), ..sup(Who::Optional, say_rename) }),
            ("alias_list", ActionDef { role: Role::Super, say: Some(say_list), ..Default::default() }),
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
    use crate::util::{caps, hit};

    #[test]
    fn module_builds_and_shortcuts() {
        let m = module();
        assert_eq!(m.actions.iter().map(|(k, _)| *k).collect::<Vec<_>>(), ["alias_add", "alias_remove", "alias_rename", "alias_list"]);
        let run = |s: &str| m.quick.iter().find(|(r, _)| hit(r, s)).map(|(r, f)| f(&caps(r, s).unwrap()));
        assert_eq!(run("renomme lucas en le boss"), Some(vec![json!({ "type": "alias_add", "who": ["lucas"], "text": "le boss" })]));
        assert_eq!(run("renomme le surnom le rat en le spartiate"), Some(vec![json!({ "type": "alias_rename", "text": "le rat", "to": "le spartiate" })]));
        assert_eq!(run("qui a quel surnom"), Some(vec![json!({ "type": "alias_list" })]));
    }

    #[test]
    fn normalize_and_say() {
        let a = json!({ "type": "alias_rename", "who": ["Lucas"], "text": "le Boss" });
        assert_eq!(normalize_rename(&a), Some(json!({ "type": "alias_add", "who": ["Lucas"], "text": "le Boss" })));
        assert_eq!(normalize_rename(&json!({ "who": ["Lucas"], "text": "a", "to": "b" })), None);
        let s = Step { text: "le Grec".into(), ..Default::default() };
        assert_eq!(say_add(&s, Some("Zyklon"), "fr").unwrap(), "Zyklon s'appelle aussi « le Grec ».");
        assert_eq!(say_remove(&s, Some("Zyklon"), "en").unwrap(), "Forgetting \"le Grec\" for Zyklon.");
    }
}
