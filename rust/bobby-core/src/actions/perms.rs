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

// ---------- Running steps ----------

use super::aliases::member_name;
use super::{Env, Post, UndoOp};
use crate::format::{card, RIGHTS};
use crate::roles::{is_super, people, perms, role_of, save_perms, Perms};
use crate::util::{BrainError, Res};
use bobby_proto::infer::Understand;
use bobby_proto::msg::Embed;

fn without(l: &[String], id: &str) -> Vec<String> {
    l.iter().filter(|x| *x != id).cloned().collect()
}
fn with(l: &[String], id: &str) -> Vec<String> {
    let mut v = without(l, id);
    v.push(id.to_string());
    v
}

/// `edit(p, id)` changes the permission lists in place; undo restores the previous document.
fn change<L>(s: &Step, env: &Env<'_, L>, edit: impl FnOnce(&mut Perms, &str)) -> Res<Vec<UndoOp>> {
    let id = s.targets.as_ref().map(|t| t[0].id.clone()).unwrap_or_default();
    if is_super(&id) {
        return Err(Say::new("On ne touche pas aux droits du super admin.", "The super admin's rights can't be changed.").into());
    }
    let mut p = perms(env.store);
    let prev = serde_json::to_value(&p).unwrap();
    edit(&mut p, &id);
    save_perms(env.store, &p).map_err(|msg| BrainError::Failed { msg, no_rights: false })?;
    Ok(vec![UndoOp::Restore { name: "permissions", prev }])
}

/// Who has which rights, as a card: one field per role with its people and what it can do.
async fn rights_card<L>(env: &Env<'_, L>, lang: &str) -> Res<Embed> {
    let p = people(env.store);
    let en = lang == "en";
    let names = |ids: Vec<String>| async move {
        let mut out = vec![];
        for id in &ids {
            out.push(member_name(env, id).await?);
        }
        Ok::<String, BrainError>(if out.is_empty() { (if en { "nobody" } else { "personne" }).to_string() } else { out.join(", ") })
    };
    let supers = format!("{}\n*{}*", names(p.supers.clone()).await?, abilities(Role::Super, lang));
    let admins = format!("{}\n*{}*", names(p.admins.clone()).await?, abilities(Role::Admin, lang));
    let prime = format!("*{}*", abilities(Role::Prime, lang));
    let blacklist = names(p.blacklist.clone()).await?;
    let prime_name = format!("⭐ Prime ({} {})", if en { "role" } else { "rôle" }, p.prime_role);
    Ok(card(
        if en { "🔐 Rights" } else { "🔐 Droits" },
        RIGHTS,
        None,
        &[("👑 Super admin", &supers, false), ("🛡️ Admins", &admins, false), (&prime_name, &prime, false), (if en { "🚫 Blacklisted" } else { "🚫 Liste noire" }, &blacklist, false)],
        Some(if en { "The super admin only changes by hand, in .env." } else { "Le super admin ne change qu'à la main, dans le fichier .env." }),
    ))
}

pub async fn run<L: Understand + Sync>(s: &mut Step, env: &Env<'_, L>, lang: &str) -> Res<Vec<UndoOp>> {
    match s.kind.as_str() {
        "perm_revoke" => change(s, env, |p, id| p.blacklist = with(&p.blacklist, id)),
        "perm_restore" => change(s, env, |p, id| p.blacklist = without(&p.blacklist, id)),
        "admin_add" => change(s, env, |p, id| {
            p.admins = with(&p.admins, id);
            p.not_admins = without(&p.not_admins, id);
        }),
        "admin_remove" => change(s, env, |p, id| {
            p.admins = without(&p.admins, id);
            p.not_admins = with(&p.not_admins, id);
        }),
        "perm_show" => {
            // the card is posted only when asked in writing
            let t = s.targets.as_ref().map(|t| t[0].clone()).unwrap_or_default();
            let role = role_of(&t.id, &t.roles, env.store);
            let blacklisted = perms(env.store).blacklist.contains(&t.id);
            s.set("role", role.as_str());
            s.set("blacklisted", blacklisted);
            let title = if lang == "en" { format!("🔐 {}'s rights", t.name) } else { format!("🔐 Droits de {}", t.name) };
            let c = card(&title, RIGHTS, Some(&rights_of(role, blacklisted, &t.name, lang)), &[], None);
            s.set("card", serde_json::to_value(vec![c]).unwrap());
            Ok(vec![])
        }
        "rights_list" => {
            let c = rights_card(env, lang).await?;
            let at = env.post(Post::Embeds(vec![c])).await?;
            s.set("where", at);
            Ok(vec![])
        }
        "super_admin" => Err(super_only_by_hand().into()),
        _ => Ok(vec![]),
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

    // ---------- running steps (bot/unit.test.js, "rights", lines 154-172) ----------

    use crate::actions::{run as run_step, Env, UndoOp};
    use crate::resolve::resolve;
    use crate::roles::{perms, role_of};
    use crate::shortcuts::quick;
    use crate::store::Store;
    use crate::testing::{ctx, fake_client, FakeLlm};
    use crate::types::Ctx;

    /// Runs the plan of a sentence; the undo ops of every step.
    async fn play(c: &Ctx, store: &Store, sentence: &str) -> crate::util::Res<Vec<UndoOp>> {
        let (io, _sent) = fake_client(|_| Ok(json!({ "name": "?", "channel": "dev-bot-test" })));
        let env = Env { io: &io, store, ctx: c, llm: &FakeLlm::default(), post_to: None };
        let mut ops = vec![];
        for mut s in resolve(&quick(sentence).unwrap(), c, store).unwrap() {
            ops.extend(run_step(&mut s, &env, "fr").await?);
        }
        Ok(ops)
    }

    /// "annule": puts the documents back.
    fn undo(store: &Store, ops: Vec<UndoOp>) {
        for op in ops.into_iter().rev() {
            op.apply_to_store(store).unwrap();
        }
    }

    #[tokio::test]
    async fn rights_by_voice_and_undo() {
        let store = Store::temp();
        let c = ctx();
        let ilias = c.members.iter().find(|m| m.name == "Prime Ilias").unwrap().id.clone(); // 397478613559148545
        let role = |id: &str, r: &[&str]| role_of(id, &r.iter().map(|x| x.to_string()).collect::<Vec<_>>(), &store);
        play(&c, &store, "enlève les permissions de Ilias").await.unwrap();
        assert_eq!(role(&ilias, &["Team Prime"]), Role::Other);
        play(&c, &store, "redonne les permissions à Ilias").await.unwrap();
        assert_eq!(role(&ilias, &[]), Role::Admin);
        play(&c, &store, "retire l'admin à Ilias").await.unwrap();
        assert_eq!(role(&ilias, &["Team Prime"]), Role::Prime);
        play(&c, &store, "passe Ilias admin").await.unwrap();
        assert_eq!(role(&ilias, &[]), Role::Admin);
        let ops = play(&c, &store, "passe Lucas admin").await.unwrap();
        assert_eq!(role("lucas", &[]), Role::Admin);
        undo(&store, ops); // undo also covers rights
        assert_eq!(role("lucas", &[]), Role::Other);
        match play(&c, &store, "enlève les permissions de WakandeX").await {
            Err(crate::util::BrainError::Say(say)) => assert!(say.fr.contains("super admin")),
            other => panic!("expected a Say, got {other:?}"),
        }
        assert_eq!(role("261531632589078528", &[]), Role::Super);
        assert!(!perms(&store).blacklist.contains(&"261531632589078528".to_string()));
    }

    #[tokio::test]
    async fn show_and_list_and_super() {
        let store = Store::temp();
        let c = ctx();
        let (io, sent) = fake_client(|cap| match cap {
            bobby_proto::msg::Cap::MemberName { id } => Ok(json!({ "name": format!("n-{id}") })),
            _ => Ok(json!({ "channel": "dev-bot-test" })),
        });
        let env = Env { io: &io, store: &store, ctx: &c, llm: &FakeLlm::default(), post_to: None };
        let mut s = resolve(&[json!({ "type": "perm_show", "who": ["Lucas"] })], &c, &store).unwrap().remove(0);
        run_step(&mut s, &env, "fr").await.unwrap();
        let say = crate::actions::def("perm_show").unwrap().say.unwrap();
        assert_eq!(say(&s, Some("Lucas"), "fr").unwrap(), "Lucas n'a aucun rôle : je ne lui réponds pas.");
        assert_eq!(s.f["card"][0]["title"], "🔐 Droits de Lucas");
        assert_eq!(s.f["card"][0]["description"], "Lucas n'a aucun rôle : je ne lui réponds pas.");
        // the Discord roles of any person count, not only the speaker's
        let mut c2 = ctx();
        c2.members.iter_mut().find(|m| m.name == "Lucas").unwrap().roles = vec![crate::roles::prime_role()];
        let mut s = resolve(&[json!({ "type": "perm_show", "who": ["Lucas"] })], &c2, &store).unwrap().remove(0);
        run_step(&mut s, &env, "fr").await.unwrap();
        assert!(s.f["card"][0]["description"].as_str().unwrap().starts_with("Lucas est Prime"), "{}", s.f["card"][0]["description"]);
        let mut s = resolve(&[json!({ "type": "rights_list" })], &c, &store).unwrap().remove(0);
        run_step(&mut s, &env, "en").await.unwrap();
        assert_eq!(s.str("where"), Some("dev-bot-test"));
        let posted = crate::testing::caps(&sent);
        let Some(bobby_proto::msg::Cap::Post { embeds, .. }) = posted.last() else { panic!("no post") };
        assert_eq!(embeds[0].title, "🔐 Rights");
        assert_eq!(embeds[0].fields[0].name, "👑 Super admin");
        assert!(embeds[0].fields[0].value.starts_with("n-261531632589078528\n*"));
        assert_eq!(embeds[0].fields[3].value, "nobody");
        let mut s = resolve(&[json!({ "type": "super_admin" })], &c, &store).unwrap().remove(0);
        assert!(matches!(run_step(&mut s, &env, "fr").await, Err(crate::util::BrainError::Say(_))));
    }
}
