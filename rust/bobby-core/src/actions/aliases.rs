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

// ---------- Running steps ----------

use super::{Env, Post, UndoOp};
use crate::format::{card, table, NICKNAMES};
use crate::util::{BrainError, Res};
use bobby_proto::infer::Understand;
use bobby_proto::msg::{Cap, Embed};
use serde_json::Map;

/// `io.memberName(id)`
pub(super) async fn member_name<L>(env: &Env<'_, L>, id: &str) -> Res<String> {
    let v = env.io.run(Cap::MemberName { id: id.to_string() }).await?;
    Ok(v["name"].as_str().unwrap_or(id).to_string())
}

fn has(list: Option<&Value>, text: &str) -> Option<usize> {
    list.and_then(Value::as_array).and_then(|l| l.iter().position(|x| norm(x.as_str().unwrap_or("")) == norm(text)))
}

/// Person who has nickname s.text: the one named in the sentence, else anyone.
fn owner(all: &Map<String, Value>, s: &Step) -> Option<String> {
    let ids: Vec<String> = match &s.targets {
        Some(t) if !t.is_empty() => vec![t[0].id.clone()],
        _ => all.keys().cloned().collect(),
    };
    ids.into_iter().find(|id| has(all.get(id), &s.text).is_some())
}

/// Runs `f(all, step)` on the stored nicknames; `f` returns the id of the person it changed (none: nickname not
/// found). Undo restores the previous document.
async fn edit<L>(s: &mut Step, env: &Env<'_, L>, f: impl FnOnce(&mut Map<String, Value>, &Step) -> Option<String>) -> Res<Vec<UndoOp>> {
    let prev = env.store.load_or("aliases", json!({}));
    let mut all = prev.as_object().cloned().unwrap_or_default();
    let id = f(&mut all, s);
    s.set("missing", id.is_none());
    if let Some(id) = &id {
        if s.targets.is_none() {
            let name = member_name(env, id).await?;
            s.set("owner", name);
        }
    }
    all.retain(|_, l| l.as_array().is_none_or(|a| !a.is_empty()));
    env.store.save("aliases", &Value::Object(all)).map_err(|msg| BrainError::Failed { msg, no_rights: false })?;
    Ok(vec![UndoOp::Restore { name: "aliases", prev }])
}

/// Nicknames as a card holding an aligned table (person | nicknames), sorted by name; None when nobody has one.
/// People may have left voice: names come from the client.
pub(crate) async fn list_card<L>(env: &Env<'_, L>) -> Res<Option<Embed>> {
    let all = env.store.load_or("aliases", json!({}));
    let mut rows: Vec<Vec<String>> = vec![];
    for (id, l) in all.as_object().into_iter().flatten() {
        let l = l.as_array().cloned().unwrap_or_default();
        if !l.is_empty() {
            rows.push(vec![member_name(env, id).await?, l.iter().map(|x| x.as_str().unwrap_or("")).collect::<Vec<_>>().join(", ")]);
        }
    }
    if rows.is_empty() {
        return Ok(None);
    }
    rows.sort_by(|a, b| norm(&a[0]).cmp(&norm(&b[0])).then_with(|| a[0].cmp(&b[0]))); // localeCompare, close enough
    Ok(Some(card("🏷️ Surnoms", NICKNAMES, Some(&table(&["Personne", "Surnoms"], &rows)), &[], None)))
}

pub async fn run<L: Understand + Sync>(s: &mut Step, env: &Env<'_, L>, _lang: &str) -> Res<Vec<UndoOp>> {
    match s.kind.as_str() {
        "alias_add" => {
            edit(s, env, |all, s| {
                let id = s.targets.as_ref()?[0].id.clone();
                let e = all.entry(id.clone()).or_insert_with(|| json!([]));
                if has(Some(e), &s.text).is_none() {
                    e.as_array_mut()?.push(json!(s.text));
                }
                Some(id)
            })
            .await
        }
        "alias_remove" => {
            edit(s, env, |all, s| {
                let id = owner(all, s)?;
                let i = has(all.get(&id), &s.text)?;
                all.get_mut(&id)?.as_array_mut()?.remove(i);
                Some(id)
            })
            .await
        }
        "alias_rename" => {
            edit(s, env, |all, s| {
                let to = s.str("to").filter(|t| !t.is_empty())?;
                let id = owner(all, s)?;
                let i = has(all.get(&id), &s.text)?;
                all.get_mut(&id)?.as_array_mut()?[i] = json!(to);
                Some(id)
            })
            .await
        }
        "alias_list" => {
            let at: Value = match list_card(env).await? {
                Some(c) => env.post(Post::Embeds(vec![c])).await?.into(),
                None => Value::Null,
            };
            s.set("where", at);
            Ok(vec![])
        }
        _ => Ok(vec![]),
    }
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

    // ---------- running steps (bot/unit.test.js, "Undo covers nicknames ...", lines 174-204) ----------

    use crate::actions::{run as run_step, Env, UndoOp};
    use crate::resolve::{names, resolve};
    use crate::shortcuts::quick;
    use crate::store::Store;
    use crate::testing::{caps as sent_caps, ctx, fake_client, FakeLlm};
    use bobby_proto::msg::Cap;

    /// `nio`: names of people out of voice, and a text channel to post in.
    fn answer(cap: &Cap) -> Result<Value, String> {
        match cap {
            Cap::MemberName { id } => Ok(json!({ "name": match id.as_str() { "b" => "Zoé", "a" => "Ana", x => x } })),
            Cap::Post { .. } => Ok(json!({ "channel": "dev-bot-test" })),
            _ => Ok(Value::Null),
        }
    }

    /// `describe(plan, lang)`: the `say` of each step.
    fn describe(plan: &[Step], lang: &str) -> String {
        plan.iter()
            .filter_map(|s| crate::actions::def(&s.kind).and_then(|d| d.say).and_then(|say| say(s, s.targets.as_ref().map(|t| names(t, lang)).as_deref(), lang)))
            .collect::<Vec<_>>()
            .join(" ")
    }

    async fn go(store: &Store, plan: &mut [Step]) -> Vec<UndoOp> {
        let (io, _) = fake_client(answer);
        let c = ctx();
        let env = Env { io: &io, store, ctx: &c, llm: &FakeLlm::default(), post_to: None };
        let mut ops = vec![];
        for s in plan {
            ops.extend(run_step(s, &env, "fr").await.unwrap());
        }
        ops
    }

    fn undo(store: &Store, ops: Vec<UndoOp>) {
        for op in ops.into_iter().rev() {
            if let UndoOp::Restore { name, prev } = op {
                store.save(name, &prev).unwrap();
            }
        }
    }

    fn plan(store: &Store, sentence: &str) -> Vec<Step> {
        resolve(&quick(sentence).unwrap(), &ctx(), store).unwrap()
    }

    #[tokio::test]
    async fn nicknames_undo_list_rename_delete() {
        let store = Store::temp();
        let c = ctx();
        let mut add = resolve(&[json!({ "type": "alias_add", "who": ["Kévin"], "text": "le Boss" })], &c, &store).unwrap();
        let ops = go(&store, &mut add).await;
        assert_eq!(store.load("aliases").unwrap()["Kévin"], json!(["le Boss"]));
        undo(&store, ops);
        assert_eq!(store.load("aliases").unwrap().get("Kévin"), None);
        // list (sorted by name, people out of voice included), rename, delete, each undoable
        store.save("aliases", &json!({ "b": ["le Grec", "Zo"], "a": ["la Reine"], "c": [] })).unwrap();
        let (io, sent) = fake_client(answer);
        let env = Env { io: &io, store: &store, ctx: &c, llm: &FakeLlm::default(), post_to: None };
        let mut lst = plan(&store, "qui a quel surnom");
        run_step(&mut lst[0], &env, "fr").await.unwrap();
        let posted = sent_caps(&sent);
        let Some(Cap::Post { embeds, .. }) = posted.last() else { panic!("no post") };
        assert_eq!(embeds[0].description, "```\nPersonne  Surnoms\n--------  -----------\nAna       la Reine\nZoé       le Grec, Zo\n```"); // aligned table in an embed
        assert_eq!(embeds[0].title, "🏷️ Surnoms");
        assert_eq!(describe(&lst, "fr"), "Je l'ai mis dans dev-bot-test.");
        assert_eq!(describe(&lst, "en"), "I put it in dev-bot-test.");
        let mut rn = plan(&store, "renomme le surnom le grec en le Spartiate");
        let ops = go(&store, &mut rn).await;
        assert_eq!(store.load("aliases").unwrap()["b"], json!(["le spartiate", "Zo"])); // shortcuts lowercase the sentence
        assert_eq!(describe(&rn, "fr"), "« le grec » devient « le spartiate » pour Zoé.");
        undo(&store, ops);
        assert_eq!(store.load("aliases").unwrap()["b"], json!(["le Grec", "Zo"]));
        let mut del = plan(&store, "supprime le surnom la reine");
        let ops = go(&store, &mut del).await;
        assert_eq!(store.load("aliases").unwrap().get("a"), None);
        assert_eq!(describe(&del, "fr"), "J'oublie « la reine » pour Ana.");
        undo(&store, ops);
        assert_eq!(store.load("aliases").unwrap()["a"], json!(["la Reine"]));
        let mut nope = plan(&store, "supprime le surnom inconnu");
        go(&store, &mut nope).await;
        assert_eq!(describe(&nope, "fr"), "Je ne trouve pas le surnom « inconnu ».");
        // with "who": only that person is searched
        let mut one = resolve(&[json!({ "type": "alias_remove", "who": ["Zyklon"], "text": "le Grec" })], &c, &store).unwrap();
        go(&store, &mut one).await;
        assert_eq!(one[0].f["missing"], json!(true));
    }
}
