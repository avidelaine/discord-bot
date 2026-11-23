//! Name resolution (port of the "Name resolution" part of `bot/brain.js`): pick, targets, resolve, isText.
use crate::actions::{self, todo};
use crate::store::Store;
use crate::types::{By, Chan, ChanKind, Ctx, Person, Role, Step, Who};
use crate::util::{norm, replace1, sim, Say};
use fancy_regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

/// Anything `pick` can choose among: a name plus aliases.
pub trait Named {
    fn name(&self) -> &str;
    fn aliases(&self) -> &[String] {
        &[]
    }
}

impl Named for Person {
    fn name(&self) -> &str {
        &self.name
    }
    fn aliases(&self) -> &[String] {
        &self.aliases
    }
}

impl Named for Chan {
    fn name(&self) -> &str {
        &self.name
    }
}

/// `WHAT` of brain.js: "person" | "voice" | "text" | anything else a module needs ([fr, en]).
fn what(w: &str) -> [&'static str; 2] {
    match w {
        "person" => ["la personne", "anyone called"],
        "voice" => ["le salon vocal", "a voice channel called"],
        "text" => ["le salon texte", "a text channel called"],
        _ => ["la liste", "a list called"],
    }
}

/// The item whose name or alias best matches `q`, or a spoken error (not found / ambiguous).
/// ponytail: 0.6 threshold and 0.1 gap picked by hand; tune them on the server's real nicknames.
pub fn pick<'a, T: Named>(q: &str, items: &'a [T], kind: &str) -> Result<&'a T, Say> {
    let mut ranked: Vec<(&T, f64)> = items
        .iter()
        .map(|it| (it, std::iter::once(it.name()).chain(it.aliases().iter().map(String::as_str)).map(|c| sim(q, c)).fold(f64::NEG_INFINITY, f64::max)))
        .collect();
    ranked.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap()); // stable, like Array.sort
    let [fr, en] = what(kind);
    let Some(&(a, sa)) = ranked.first() else { return Err(Say::new(format!("Je ne trouve pas {fr} « {q} »."), format!("I can't find {en} \"{q}\"."))) };
    if sa < 0.6 {
        return Err(Say::new(format!("Je ne trouve pas {fr} « {q} »."), format!("I can't find {en} \"{q}\".")));
    }
    if let Some(&(b, sb)) = ranked.get(1) {
        if sb >= 0.6 && sa < 1.0 && sa - sb < 0.1 {
            let (an, bn) = (a.name(), b.name());
            return Err(Say::new(format!("« {q} » : tu veux dire {an} ou {bn} ?"), format!("\"{q}\": do you mean {an} or {bn}?")));
        }
    }
    Ok(a)
}

/// The text channel a name means better than any voice channel, or None ("dev-bot-test" vs the voice "dev-bot").
pub fn is_text<'a>(q: &str, ctx: &'a crate::types::Ctx) -> Option<&'a Chan> {
    let best = |l: &[Chan]| l.iter().map(|c| sim(q, &c.name)).fold(0.0, f64::max);
    let t = best(&ctx.text_channels);
    if !q.is_empty() && t >= 0.8 && t > best(&ctx.voice_channels) { ctx.text_channels.iter().find(|c| sim(q, &c.name) == t) } else { None }
}

pub static ME: [&str; 5] = ["moi", "me", "je", "myself", "i"];
pub static ALL: [&str; 6] = ["tous", "toutlemonde", "toutes", "everyone", "everybody", "all"];
pub static HERE: [&str; 2] = ["ici", "here"];

fn strings(v: &Value, k: &str) -> Vec<String> {
    v.get(k).and_then(Value::as_array).map(|a| a.iter().map(|x| x.as_str().unwrap_or("").to_string()).collect()).unwrap_or_default()
}

/// The people a raw step means, deduplicated (first seen first), exclusions removed.
pub fn targets(step: &Value, ctx: &Ctx) -> Result<Vec<Person>, Say> {
    let is_move = step["type"] == "move";
    let mut out: Vec<Person> = vec![];
    for w in strings(step, "who") {
        let n = norm(&w);
        if ME.contains(&n.as_str()) {
            out.push(ctx.speaker.clone());
        // "tous" (everyone): for a move, everyone in voice on the server; otherwise, the other people in the speaker's channel.
        } else if ALL.contains(&n.as_str()) {
            out.extend(ctx.members.iter().filter(|m| is_move || (m.channel == ctx.speaker.channel && m.id != ctx.speaker.id)).cloned());
        } else {
            out.push(pick(&w, &ctx.members, "person")?.clone());
        }
    }
    let mut ex: Vec<String> = vec![];
    for w in strings(step, "except") {
        ex.push(if ME.contains(&norm(&w).as_str()) { ctx.speaker.id.clone() } else { pick(&w, &ctx.members, "person")?.id.clone() });
    }
    let mut res: Vec<Person> = vec![];
    for p in out {
        if !res.iter().any(|r| r.id == p.id) && !ex.contains(&p.id) {
            res.push(p);
        }
    }
    if res.is_empty() {
        return Err(Say::new("Je ne sais pas sur qui agir.", "I don't know who you mean."));
    }
    Ok(res)
}

/// Raw actions -> steps: names resolved to people and channels, rights and fields checked.
pub fn resolve(raw_actions: &[Value], ctx: &Ctx, store: &Store) -> Result<Vec<Step>, Say> {
    raw_actions
        .iter()
        .map(|raw| {
            // a module may fix a known LLM confusion
            let a = raw["type"].as_str().and_then(actions::def).and_then(|d| d.normalize).and_then(|f| f(raw)).unwrap_or_else(|| raw.clone());
            let Some(def) = a["type"].as_str().and_then(actions::def) else { return Err(Say::new("Je n'ai pas compris.", "Sorry, I didn't get that.")) };
            // the Claude fallback may answer a number ("text": 50): JS kept it truthy
            let text = match &a["text"] { Value::String(t) => t.clone(), Value::Number(n) => n.to_string(), _ => String::new() };
            let mut step = Step { kind: a["type"].as_str().unwrap_or("").to_string(), text, by: By { id: ctx.speaker.id.clone(), name: ctx.speaker.name.clone(), role: ctx.role }, ..Default::default() };
            for (k, v) in a.as_object().into_iter().flatten() {
                // raw fields a def does not resolve stay on the step as in JS ({...a}): alias_rename reads its raw "to"
                let resolved = match k.as_str() {
                    "type" | "who" | "except" | "text" => true,
                    "channel" => def.channel != ChanKind::None,
                    "list" => def.list,
                    "to" => def.to,
                    _ => false,
                };
                if !resolved {
                    step.f.insert(k.clone(), v.clone());
                }
            }
            let str_of = |k: &str| a[k].as_str().unwrap_or("");
            step.list = if def.list && !str_of("list").is_empty() { Some(todo::list_of(str_of("list"), ctx, store)?) } else { None };
            if def.to {
                step.to = Some(todo::list_of(if str_of("to").is_empty() { "general" } else { str_of("to") }, ctx, store)?);
            }
            if def.who == Who::Yes || (def.who == Who::Optional && !strings(&a, "who").is_empty()) {
                step.targets = Some(targets(&a, ctx)?);
            }
            if def.one && step.targets.as_ref().is_some_and(|t| t.len() != 1) {
                return Err(Say::new("Une seule personne à la fois pour ça.", "One person at a time for that."));
            }
            if def.channel == ChanKind::Voice {
                let q = str_of("channel");
                // a text channel's name never resolves to a voice channel ("dev-bot-test" moved people to "dev-bot")
                if let Some(txt) = is_text(q, ctx) {
                    return Err(Say::new(format!("{} est un salon texte, pas un salon vocal.", txt.name), format!("{} is a text channel, not a voice channel.", txt.name)));
                }
                let chan = if HERE.contains(&norm(q).as_str()) { ctx.voice_channels.iter().find(|c| Some(&c.name) == ctx.speaker.channel.as_ref()) } else { Some(pick(q, &ctx.voice_channels, "voice")?) };
                step.channel = Some(chan.ok_or_else(|| Say::new("Tu n'es dans aucun salon vocal.", "You're not in a voice channel."))?.clone());
            }
            if def.channel == ChanKind::Text || (def.channel == ChanKind::TextOpt && !str_of("channel").is_empty()) {
                step.channel = Some(pick(str_of("channel"), &ctx.text_channels, "text")?.clone());
            }
            if def.text && step.text.is_empty() {
                return Err(Say::new("Il me manque le texte.", "I'm missing the text."));
            }
            Ok(step)
        })
        .collect()
}

/// Actions the super admin runs without the "Tu confirmes ?" (Alexandre's call: a kick from voice is his to make
/// at once). Everyone else still confirms; a plan touching 3 people or more always confirms, whoever asks.
const SUPER_NO_CONFIRM: [&str; 1] = ["disconnect"];

pub fn needs_confirm(plan: &[Step], role: Role) -> bool {
    let asks = |s: &Step| actions::def(&s.kind).is_some_and(|d| d.confirm) && !(role == Role::Super && SUPER_NO_CONFIRM.contains(&s.kind.as_str()));
    plan.iter().any(asks) || plan.iter().map(|s| s.targets.as_ref().map_or(0, Vec::len)).sum::<usize>() >= 3
}

/// Highest role the plan needs; undo needs the role of what it undoes (`undo_role`, from the undo registry).
pub fn needed(plan: &[Step], undo_role: Option<Role>) -> Role {
    plan.iter()
        .map(|s| if s.kind == "undo" { undo_role.unwrap_or(Role::Prime) } else { actions::def(&s.kind).map_or(Role::Admin, |d| d.role) })
        .fold(Role::Prime, Role::max)
}

pub fn names(ts: &[Person], lang: &str) -> String {
    static LAST: LazyLock<Regex> = LazyLock::new(|| crate::util::re(r", ([^,]*)$"));
    let joined = ts.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", ");
    replace1(&LAST, &joined, if lang == "en" { " and $1" } else { " et $1" })
}

pub fn plan_text(plan: &[Step]) -> String {
    plan.iter()
        .map(|x| {
            let mut s = x.kind.clone();
            if let Some(t) = &x.targets {
                s += &format!(" -> {}", t.iter().map(|m| m.name.as_str()).collect::<Vec<_>>().join(", "));
            }
            if let Some(c) = &x.channel {
                s += &format!(" @ {}", c.name);
            }
            if !x.text.is_empty() {
                s += &format!(" \"{}\"", x.text);
            }
            s
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- Rights ----------
    fn plan(sentence: &str) -> Vec<Step> {
        crate::actions::pc::set_apps(vec!["Valorant".into(), "Claude".into(), "ChatGPT".into()]);
        resolve(&crate::shortcuts::quick(sentence).unwrap(), &crate::testing::ctx(), &Store::temp()).unwrap()
    }

    #[test]
    fn rights() {
        use crate::roles::allowed;
        assert_eq!(needed(&plan("mute Zyklon"), None), Role::Admin);
        assert_eq!(needed(&plan("quelle heure est-il"), None), Role::Prime);
        assert_eq!(needed(&plan("passe Marie admin"), None), Role::Super);
        assert!(allowed(Role::Prime, needed(&plan("quelle heure est-il"), None)));
        assert!(!allowed(Role::Prime, needed(&plan("mute Zyklon"), None)));
        assert!(!allowed(Role::Admin, needed(&plan("passe Marie admin"), None)));
        for sentence in ["qui a quel surnom", "renomme le surnom le grec en le boss", "supprime le surnom le grec"] {
            let n = needed(&plan(sentence), None);
            assert_eq!(n, Role::Super);
            assert!(!allowed(Role::Prime, n) && !allowed(Role::Admin, n));
            assert!(allowed(Role::Super, n));
        }
        let raw = [serde_json::json!({ "type": "alias_add", "who": ["Kévin"], "text": "x" })];
        assert_eq!(needed(&resolve(&raw, &crate::testing::ctx(), &Store::temp()).unwrap(), None), Role::Super);
        // undo needs the role of what it undoes
        let undo = plan("annule");
        assert_eq!(needed(&undo, None), Role::Prime);
        assert_eq!(needed(&undo, Some(Role::Admin)), Role::Admin);
    }

    // ---------- PC control: rights, confirmations ----------
    #[test]
    fn pc_rights() {
        use crate::roles::allowed;
        for phrase in ["mets pause sur le PC", "monte le son du PC", "lance Valorant", "mets le PC en veille", "éteins le PC", "verrouille le PC", "température du GPU"] {
            let n = needed(&plan(phrase), None);
            assert_eq!(n, Role::Super);
            assert!(!allowed(Role::Admin, n) && !allowed(Role::Prime, n) && allowed(Role::Super, n));
        }
        for (t, phrase) in [("pc_lock", "verrouille le PC"), ("pc_sleep", "mets le PC en veille"), ("pc_shutdown", "éteins le PC")] {
            assert!(crate::actions::def(t).unwrap().confirm, "{t} needs confirm");
            assert!(needs_confirm(&plan(phrase), Role::Super));
        }
        for t in ["pc_media", "pc_volume", "pc_launch", "pc_gpu"] {
            assert!(!crate::actions::def(t).unwrap().confirm);
        }
    }

    #[test]
    fn music_rights() {
        use crate::roles::allowed;
        for phrase in ["pause", "joue Despacito", "volume 20"] {
            let n = needed(&plan(phrase), None);
            assert_eq!(n, Role::Prime);
            assert!(allowed(Role::Prime, n) && allowed(Role::Admin, n) && allowed(Role::Super, n));
            assert!(!allowed(Role::Other, n));
        }
    }

    #[test]
    fn targets_and_text() {
        let (ctx, store) = (crate::testing::ctx(), Store::temp());
        let t = |v: Value| targets(&v, &ctx).map(|p| p.into_iter().map(|x| x.name).collect::<Vec<_>>());
        assert_eq!(t(serde_json::json!({ "type": "mute", "who": ["Zyklon", "zyklon", "moi"] })).unwrap(), vec!["Zyklon", "Alex"]);
        assert_eq!(t(serde_json::json!({ "type": "mute", "who": ["tous"], "except": ["Kévin"] })).unwrap(), vec!["Zyklon"]);
        assert!(t(serde_json::json!({ "type": "mute", "who": ["moi"], "except": ["moi"] })).is_err());
        assert_eq!(resolve(&[serde_json::json!({ "type": "nope" })], &ctx, &store).unwrap_err().fr, "Je n'ai pas compris.");
        let p = plan("mets-moi dans la salle de réunion");
        assert_eq!(plan_text(&p), "move -> Alex @ Réunion");
        assert_eq!(names(&p[0].targets.clone().unwrap(), "fr"), "Alex");
        let two = vec![Person { name: "A".into(), ..Default::default() }, Person { name: "B".into(), ..Default::default() }, Person { name: "C".into(), ..Default::default() }];
        assert_eq!(names(&two, "fr"), "A, B et C");
        assert_eq!(names(&two, "en"), "A, B and C");
    }

    // "Name resolution" of bot/unit.test.js (the sim case is in util).
    #[test]
    fn pick_names() {
        let p = |name: &str, aliases: &[&str]| Person { name: name.into(), aliases: aliases.iter().map(|s| s.to_string()).collect(), ..Default::default() };
        let roster = vec![p("Zyklon", &[]), p("Kévin", &["kev_du_93"]), p("Marie", &[]), p("Mario", &[])];
        assert_eq!(pick("ziklone", &roster, "person").unwrap().name, "Zyklon");
        assert_eq!(pick("kevin", &roster, "person").unwrap().name, "Kévin");
        assert_eq!(pick("kev du 93", &roster, "person").unwrap().name, "Kévin");
        assert!(pick("Mari", &roster, "person").unwrap_err().fr.contains("tu veux dire"));
        assert!(pick("Bernard", &roster, "person").unwrap_err().fr.contains("Je ne trouve pas"));
    }

    // review of R3a: raw fields a def does not resolve stay on the step (alias_rename's "to"), numbers become text
    #[test]
    fn raw_fields_kept() {
        let st = Store::temp();
        let p = resolve(&[serde_json::json!({"type": "alias_rename", "text": "le rat", "to": "le spartiate"})], &crate::testing::ctx(), &st).unwrap();
        assert_eq!(p[0].str("to"), Some("le spartiate"));
        let p = resolve(&[serde_json::json!({"type": "music_volume", "text": 50})], &crate::testing::ctx(), &st).unwrap();
        assert_eq!(p[0].text, "50");
    }
}
