//! Port of `bot/actions/perms.js`: rights managed by voice, super admin only: blacklist someone, give them
//! back their rights, grant or remove admin. Anyone with a role may ask who has which rights (answered from
//! live state). Nobody, the super admin included, can make or unmake a super admin by voice: that is only
//! SUPER_ADMINS in .env, edited by hand.
//! Texts, shortcuts and `say` lines; `run` (change, rights card) is batch R3b.
use crate::actions::info::abilities;
use crate::types::{ActionDef, Module, QuickFn, Role, SayFn, Step, Who};
use crate::util::{group, re, Say};
use fancy_regex::Regex;
use serde_json::json;

/// What `super_admin` throws (its `run` is R3b).
pub fn super_only_by_hand() -> Say {
    Say::new("Le super admin ne se change pas à la voix : seulement à la main, dans le fichier .env.", "The super admin cannot be changed by voice: only by hand, in the .env file.")
}

/// "X est admin : ...", from the role X has right now and the roles the action modules declare.
/// `role` and `blacklisted` are the live state (`roles.roleOf(...)`, `perms().blacklist.includes(id)`): the
/// `run` of perm_show (R3b) puts them in `step.f` ("role" as its name, "blacklisted" as a bool).
pub fn rights_of(role: Role, blacklisted: bool, n: &str, lang: &str) -> String {
    if lang == "en" {
        if role == Role::Other {
            return if blacklisted { format!("{n} is blacklisted: I don't answer them.") } else { format!("{n} has no role: I don't answer them.") };
        }
        let what = match role {
            Role::Super => "super admin, with every right",
            Role::Admin => "an admin",
            _ => "Prime",
        };
        return format!("{n} is {what}: {}.", abilities(role, lang));
    }
    if role == Role::Other {
        return if blacklisted { format!("{n} est sur liste noire : je ne lui réponds pas.") } else { format!("{n} n'a aucun rôle : je ne lui réponds pas.") };
    }
    let what = match role {
        Role::Super => "super admin, avec tous les droits",
        Role::Admin => "admin",
        _ => "Prime",
    };
    format!("{n} est {what} : {}.", abilities(role, lang))
}

fn say_revoke(_: &Step, n: Option<&str>, lang: &str) -> Option<String> {
    let n = n.unwrap_or("");
    Some(if lang == "en" { format!("{n} can no longer give me orders.") } else { format!("{n} n'a plus le droit de me parler.") })
}

fn say_restore(_: &Step, n: Option<&str>, lang: &str) -> Option<String> {
    let n = n.unwrap_or("");
    Some(if lang == "en" { format!("{n} has their rights back.") } else { format!("{n} retrouve ses droits.") })
}

fn say_admin_add(_: &Step, n: Option<&str>, lang: &str) -> Option<String> {
    let n = n.unwrap_or("");
    Some(if lang == "en" { format!("{n} is now an admin.") } else { format!("{n} est admin.") })
}

fn say_admin_remove(_: &Step, n: Option<&str>, lang: &str) -> Option<String> {
    let n = n.unwrap_or("");
    Some(if lang == "en" { format!("{n} is no longer an admin.") } else { format!("{n} n'est plus admin.") })
}

fn say_show(s: &Step, n: Option<&str>, lang: &str) -> Option<String> {
    let role = s.str("role").and_then(Role::parse).unwrap_or_default();
    Some(rights_of(role, s.flag("blacklisted"), n.unwrap_or(""), lang))
}

fn say_rights_list(s: &Step, _: Option<&str>, lang: &str) -> Option<String> {
    let at = s.str("where").filter(|w| !w.is_empty());
    Some(match (at, lang == "en") {
        (Some(w), true) => format!("I put it in {w}."),
        (Some(w), false) => format!("Je l'ai mis dans {w}."),
        (None, _) => String::new(),
    })
}

fn q(r: Regex, f: QuickFn) -> (Regex, QuickFn) {
    (r, f)
}

pub fn module() -> Module {
    let sup = |who: Who, say: SayFn| ActionDef { role: Role::Super, who, one: true, say: Some(say), ..Default::default() };
    Module {
        name: "perms",
        help: ["gérer les droits de chacun et dire qui a quels droits", "manage everyone's rights and tell who has which rights"],
        short: ["les droits", "the rights"],
        prompt: vec![
            r#"- perm_revoke / perm_restore : enlever ou redonner à "who" le droit de donner des ordres à Bobby"#,
            r#"- admin_add / admin_remove : passer "who" admin, ou lui retirer l'admin"#,
            r#"- perm_show : dire quels droits a "who" (une seule personne)"#,
            r#"- super_admin : nommer quelqu'un super admin, ou retirer le super admin (toujours refusé)"#,
        ],
        rules: vec![
            r#"- "super admin" dans la demande de changement de droits : super_admin, jamais admin_add ni admin_remove."#,
            r#"- perm_show seulement pour une personne nommée. "Qui est admin", "qui a le droit de…" sans personne précise : about."#,
        ],
        examples: vec![
            r#""enlève les permissions de Lucas" -> {"actions":[{"type":"perm_revoke","who":["Lucas"]}],"say":""}"#,
            r#""passe Marie admin" -> {"actions":[{"type":"admin_add","who":["Marie"]}],"say":""}"#,
            r#""explique à Lucas quels sont ses droits" -> {"actions":[{"type":"perm_show","who":["Lucas"]}],"say":""}"#,
            r#""passe Kévin super admin" -> {"actions":[{"type":"super_admin"}],"say":""}"#,
        ],
        fields: vec![],
        early: None,
        quick: vec![
            // JS: /\bsuper[\s-]?admin\b/ + `m.input` starting with a verb, else null. A QuickFn only sees the
            // captures, so the regex also captures that leading verb (group 1) and an empty group declines (null).
            q(
                re(r"^(?:(passe|mets|nomme|fais|rends|retire|enlève|enleve|supprime|make|remove|promote|demote|donne)\b)?[\s\S]*?\bsuper[\s-]?admin\b"),
                |m| if group(m, 1).is_empty() { vec![] } else { vec![json!({ "type": "super_admin" })] },
            ),
            q(re(r"^(?:enlève|enleve|retire|supprime) (?:les |ses |tous les )?(?:permissions|droits) (?:de|à|a|du) (.+)$"), |m| vec![json!({ "type": "perm_revoke", "who": [group(m, 1)] })]),
            q(re(r"^(?:redonne|rends|remets) (?:les |ses )?(?:permissions|droits) (?:de|à|a|au) (.+)$"), |m| vec![json!({ "type": "perm_restore", "who": [group(m, 1)] })]),
            q(re(r"^(?:passe|mets|nomme) (.+?) admin$"), |m| vec![json!({ "type": "admin_add", "who": [group(m, 1)] })]),
            q(re(r"^(?:retire|enlève|enleve) (?:l'|le rôle )?admin (?:de|à|a|au) (.+)$"), |m| vec![json!({ "type": "admin_remove", "who": [group(m, 1)] })]),
            q(
                re(r"^(?:(?:dis|donne)[- ]moi )?(?:quels sont |c'est quoi )?(?:les )?(?:droits|permissions) (?:de |d'|du )(.+)$|^(?:(?:dis|donne)[- ]moi )?quels droits (?:a|as) (.+)$|^what are (.+?)'s (?:rights|permissions)$"),
                |m| {
                    // m[1] || m[2] || m[3]
                    let w = [1, 2, 3].iter().map(|&i| group(m, i)).find(|g| !g.is_empty()).unwrap_or("");
                    vec![json!({ "type": "perm_show", "who": [w] })]
                },
            ),
            q(re(r"^(?:qui a quels droits|(?:liste|montre|affiche|donne)(?:-moi| moi)? (?:les|tous les) droits|(?:(?:dis|donne)[- ]moi )?(?:la )?liste des droits|who has which rights|(?:list|show|give)(?: me)?(?: all)?(?: the)? rights)$"), |_| vec![json!({ "type": "rights_list" })]),
        ],
        prompt_lines: None,
        actions: vec![
            ("perm_revoke", sup(Who::Yes, say_revoke)),
            ("perm_restore", sup(Who::Yes, say_restore)),
            ("admin_add", sup(Who::Yes, say_admin_add)),
            ("admin_remove", sup(Who::Yes, say_admin_remove)),
            ("perm_show", ActionDef { role: Role::Prime, info: true, who: Who::Yes, one: true, say: Some(say_show), ..Default::default() }),
            // Not in the LLM prompt ("qui est admin" stays a spoken "about"): shortcut, or an "about" on rights asked in writing.
            ("rights_list", ActionDef { role: Role::Prime, info: true, say: Some(say_rights_list), ..Default::default() }),
            // whoever asks gets the same answer
            ("super_admin", ActionDef { role: Role::Prime, info: true, ..Default::default() }),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::{caps, hit};

    fn run(m: &Module, s: &str) -> Option<Vec<serde_json::Value>> {
        m.quick.iter().find(|(r, _)| hit(r, s)).map(|(r, f)| f(&caps(r, s).unwrap()))
    }

    #[test]
    fn module_builds_and_shortcuts() {
        let m = module();
        assert_eq!(m.actions.len(), 7);
        assert_eq!(run(&m, "passe kévin super admin"), Some(vec![json!({ "type": "super_admin" })]));
        // JS: the first shortcut matched but declined (null), so quick() returns null for the whole sentence
        assert_eq!(run(&m, "dis-moi les droits du super admin"), Some(vec![]));
        assert_eq!(run(&m, "passe marie admin"), Some(vec![json!({ "type": "admin_add", "who": ["marie"] })]));
        assert_eq!(run(&m, "what are lucas's rights"), Some(vec![json!({ "type": "perm_show", "who": ["lucas"] })]));
        assert_eq!(run(&m, "qui a quels droits"), Some(vec![json!({ "type": "rights_list" })]));
    }

    #[test]
    fn says() {
        let s = Step::default();
        assert_eq!(say_revoke(&s, Some("Lucas"), "fr").unwrap(), "Lucas n'a plus le droit de me parler.");
        assert_eq!(say_admin_add(&s, Some("Marie"), "en").unwrap(), "Marie is now an admin.");
        assert_eq!(rights_of(Role::Other, true, "Lucas", "fr"), "Lucas est sur liste noire : je ne lui réponds pas.");
        assert_eq!(say_rights_list(&s, None, "fr").unwrap(), "");
    }
}
