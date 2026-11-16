//! Port of `bot/actions/discord.js`: static texts, action definitions, shortcuts. The `run` functions
//! (voice mute/move, own messages, `whenAlone`) are done by the Discord client now.
use crate::types::{ActionDef, ChanKind, Echo, Module, QuickFn, Role, SayFn, Step, Who};
use crate::util::{group, hit, re, re_i};
use fancy_regex::{Captures, Regex};
use serde_json::{json, Value};
use std::sync::LazyLock;

static ALL: LazyLock<Regex> = LazyLock::new(|| re(r"^(?:tout le monde|everyone|everybody)$"));

fn who(w: &str) -> Value {
    json!([if hit(&ALL, w) { "tous" } else { w }])
}

// "Appelle-moi sur Discord": a bot cannot call. Bobby says so and offers a mention instead (offer: see brain.js).
// Never "appelle-moi le Boss", which gives a nickname.
static CALL: LazyLock<Regex> = LazyLock::new(|| {
    re_i(r"(?:appelle[- ]moi|m'appeler|call me|passe[- ]moi un (?:appel|coup de fil))(?: (?:sur|on|via) discord| en (?:appel|vocal|privé|mp)| au téléphone)|(?:passe|lance|fais)[- ](?:moi |nous )?un appel(?: discord)?")
});

fn call(query: &str) -> Option<Vec<Value>> {
    if hit(&CALL, &query.replace(['’', '`'], "'")) {
        Some(vec![json!({ "type": "ping", "who": ["moi"], "text": "📞 Tu m'as demandé de t'appeler : je ne peux pas, mais me voilà.", "offer": "call" })])
    } else {
        None
    }
}

fn q(r: Regex, f: QuickFn) -> (Regex, QuickFn) {
    (r, f)
}

fn sf(f: SayFn) -> Option<SayFn> {
    Some(f)
}

/// fr or en line.
fn l(lang: &str, fr: String, en: String) -> Option<String> {
    Some(if lang == "en" { en } else { fr })
}

fn js(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => "undefined".into(),
    }
}

// Server mute / deafen (undone by restoring each person's previous state: client side).
fn voice_set(say: SayFn) -> ActionDef {
    // echo: done silently after the ack (see brain.js report)
    ActionDef { role: Role::Admin, who: Who::Yes, echo: Echo::Yes, say: sf(say), ..Default::default() }
}

fn chan_name(s: &Step) -> String {
    s.channel.as_ref().map(|c| c.name.clone()).unwrap_or_default()
}

fn chan_or(s: &Step, other: &str) -> String {
    match &s.channel {
        Some(c) if !c.name.is_empty() => c.name.clone(),
        _ => other.to_string(),
    }
}

pub fn module() -> Module {
    Module {
        name: "discord",
        early: Some(call),
        help: [
            "couper ou rétablir le micro et le son, déplacer, ramener ou déconnecter des gens, écrire dans un salon, mentionner quelqu'un, modifier ou supprimer mes propres messages",
            "mute, deafen, move, drag or disconnect people, post in a channel, mention someone, edit or delete my own messages",
        ],
        short: ["la modération et les messages", "moderation and messages"],
        prompt: vec![
            r##"- mute / unmute : couper ou rétablir le micro de "who""##,
            r##"- deafen / undeafen : couper ou rétablir le son (le casque) de "who""##,
            r##"- move : déplacer "who" dans le salon vocal "channel""##,
            r##"- disconnect : déconnecter "who" du vocal"##,
            r##"- message : envoyer "text" dans le salon texte "channel""##,
            r##"- msg_edit : remplacer le texte de ton dernier message écrit par "text" ("channel" facultatif)"##,
            r##"- msg_delete : supprimer ton dernier message écrit ("channel" facultatif)"##,
            r##"- clean : supprimer tous tes propres messages d'un salon texte ("channel" facultatif)"##,
            r##"- ping : mentionner (notifier, arrobase, @, tag, ping) "who" dans le salon texte "channel" (facultatif), avec le message "text" (facultatif)"##,
            r##"- leave : Bobby quitte le salon vocal (aussi "repasse en local", "reviens sur le PC", "déconnecte-toi de Discord")"##,
        ],
        rules: vec![
            r##"- "virer", "dégager" ou "kicker" une personne nommée du vocal, c'est disconnect. Si on dit à Bobby lui-même de partir, c'est leave."##,
            r##"- "dé-mute", "démute" ou "des mute" (mal transcrit), c'est unmute. "drag" ou "draguer" quelqu'un, c'est le déplacer (move) dans le salon de celui qui parle."##,
            r##"- "couper" quelqu'un sans préciser, c'est mute (le micro seulement)."##,
            r##"- "nettoie" un salon ou "supprime les messages" d'un salon : clean (tu ne supprimes que tes propres messages)."##,
            r##"- Bobby, c'est toi : tu ne peux pas te muter ("actions" vide, dis-le dans "say")."##,
            r##"- "ici" désigne le salon vocal de celui qui parle : mets son nom exact dans "channel"."##,
            r##"- Tu ne supprimes que tes propres messages : supprimer ceux des autres ("mes messages", "ceux que j'ai mis") n'est pas faisable."##,
            r##"- Un salon texte n'est jamais la destination d'un move : "arrobase-moi dans X", "ping-moi sur X", c'est ping."##,
            r##"- Tu ne peux pas passer d'appel Discord : propose de mentionner la personne (ping) à la place."##,
        ],
        examples: vec![
            r##""coupe le micro de Kevin" -> {"actions":[{"type":"mute","who":["Kévin"]}],"say":""}"##,
            r##""tout le monde en AFK" -> {"actions":[{"type":"move","who":["tous"],"channel":"AFK"}],"say":""}"##,
            r##""ramène tout le monde ici sauf Marie" -> {"actions":[{"type":"move","who":["tous"],"except":["Marie"],"channel":"Général"}],"say":""}"##,
            r##""mets-moi dans Gaming et rends le son à Zyklon" -> {"actions":[{"type":"move","who":["moi"],"channel":"Gaming"},{"type":"undeafen","who":["Zyklon"]}],"say":""}"##,
            r##""rends le micro à tout le monde sauf Zyklon" -> {"actions":[{"type":"unmute","who":["tous"],"except":["Zyklon"]}],"say":""}"##,
            r##""coupe tout le monde sauf Marie" -> {"actions":[{"type":"mute","who":["tous"],"except":["Marie"]}],"say":""}"##,
            r##""drague Lucas" -> {"actions":[{"type":"move","who":["Lucas"],"channel":"Général"}],"say":""}"##,
            r##""c'est bon tu peux y aller" -> {"actions":[{"type":"leave"}],"say":""}"##,
            r##""move me and Zyklon to Gaming" -> {"actions":[{"type":"move","who":["moi","Zyklon"],"channel":"Gaming"}],"say":""}"##,
            r##""you can leave now" -> {"actions":[{"type":"leave"}],"say":""}"##,
            r##""kick Lucas from voice" -> {"actions":[{"type":"disconnect","who":["Lucas"]}],"say":""}"##,
            r##""fais un clean-up du salon memes" -> {"actions":[{"type":"clean","channel":"memes"}],"say":""}"##,
            r##""corrige ton dernier message en : rendez-vous à 21 h" -> {"actions":[{"type":"msg_edit","text":"rendez-vous à 21 h"}],"say":""}"##,
            r##""arrobase-moi dans annonces pour me rappeler de lancer Apex" -> {"actions":[{"type":"ping","who":["moi"],"channel":"annonces","text":"pense à lancer Apex"}],"say":""}"##,
            r##""supprime les messages que j'ai mis dans memes" -> {"analyse":{"genre":"ordre","faisable":false,"pourquoi":"je ne supprime que mes propres messages"},"actions":[],"say":"Je ne peux supprimer que mes propres messages."}"##,
        ],
        fields: vec![],
        // LLM-free shortcuts. ponytail: a handful of phrasings per language; add the ones that keep showing up in the logs.
        quick: vec![
            q(re(r"^(?:repasse|reviens|retourne|passe)(?: en| sur)? (?:local|le pc|mon pc|l'ordi)$|^(?:déconnecte|deconnecte|quitte)(?:-toi| toi)? (?:de )?discord$"), |_| vec![json!({ "type": "leave" })]),
            q(re(r"^(?:coupe|ferme|mute)(?: le)? micro (?:de|à|a) (.+)$"), |m| vec![json!({ "type": "mute", "who": who(group(m, 1)) })]),
            q(re(r"^(?:dé|de|des|dés) ?-?mute (.+)$"), |m| vec![json!({ "type": "unmute", "who": who(group(m, 1)) })]),
            q(re(r"^mute (.+)$"), |m| vec![json!({ "type": "mute", "who": who(group(m, 1)) })]),
            q(re(r"^(?:rends|remets|rétablis|redonne|rallume)(?: le)? micro (?:de|à|a) (.+)$"), |m| vec![json!({ "type": "unmute", "who": who(group(m, 1)) })]),
            q(re(r"^unmute (.+)$"), |m| vec![json!({ "type": "unmute", "who": who(group(m, 1)) })]),
            q(re(r"^(?:coupe le son (?:de|à|a)|deafen) (.+)$"), |m| vec![json!({ "type": "deafen", "who": who(group(m, 1)) })]),
            q(re(r"^(?:(?:rends|remets|rétablis|redonne) le son (?:de|à|a)|undeafen) (.+)$"), |m| vec![json!({ "type": "undeafen", "who": who(group(m, 1)) })]),
            q(re(r"^(?:déplace|mets|envoie|bouge) (?!la musique |le pc |l'ordi|mon pc |pause |le son |le volume |du |de la |le dernier |le point |ça |ca )(.+?) (?:dans|en|sur|vers) (?:le salon |la salle (?:de )?)?(.+)$"), |m| vec![json!({ "type": "move", "who": who(group(m, 1)), "channel": group(m, 2) })]),
            q(re(r"^mets[- ]moi (?:dans|en) (?:le salon |la salle (?:de )?)?(.+)$"), |m| vec![json!({ "type": "move", "who": ["moi"], "channel": group(m, 1) })]),
            q(re(r"^(?:move|put|send|drag) (.+?) (?:to|in|into) (?:the )?(.+?)(?: channel)?$"), |m| vec![json!({ "type": "move", "who": who(group(m, 1)), "channel": group(m, 2) })]),
            q(re(r"^(?:drag|draguer|drague|ramène) (.+?)(?: ici| here)?$"), |m| vec![json!({ "type": "move", "who": who(group(m, 1)), "channel": "ici" })]),
            q(re(r"^(?:pars|partir|va-t'en|vas-t'en|quitte le (?:salon|vocal)(?: vocal)?|tu peux partir|leave|go away|leave the (?:channel|voice channel))$"), |_| vec![json!({ "type": "leave" })]),
            q(re(r"^(?:nettoie|clean(?: up)?|fais (?:le |un )?(?:clean-?up|ménage)(?: de| du| dans)?)(?: le salon| the channel| salon)? ?#?(.*)$"), clean_quick),
            q(re(r"^(?:supprime|efface|delete) ton dernier message$"), |_| vec![json!({ "type": "msg_delete" })]),
            q(re(r"^(?:kick|vire|dégage|déconnecte) (.+?)(?: du vocal| from voice| out of voice)?$"), |m| vec![json!({ "type": "disconnect", "who": who(group(m, 1)) })]),
        ],
        prompt_lines: None,
        actions: vec![
            ("mute", voice_set(|_, n, lang| l(lang, format!("Je coupe le micro de {}.", n.unwrap_or("")), format!("Muting {}.", n.unwrap_or(""))))),
            ("unmute", voice_set(|_, n, lang| l(lang, format!("Je rétablis le micro de {}.", n.unwrap_or("")), format!("Unmuting {}.", n.unwrap_or(""))))),
            ("deafen", voice_set(|_, n, lang| l(lang, format!("Je coupe le son de {}.", n.unwrap_or("")), format!("Deafening {}.", n.unwrap_or(""))))),
            ("undeafen", voice_set(|_, n, lang| l(lang, format!("Je rétablis le son de {}.", n.unwrap_or("")), format!("Undeafening {}.", n.unwrap_or(""))))),
            (
                "move",
                // A move that includes the speaker ("déplace-nous", "mets-moi", "tout le monde") takes Bobby along (client side).
                ActionDef {
                    role: Role::Admin,
                    who: Who::Yes,
                    channel: ChanKind::Voice,
                    echo: Echo::Yes,
                    say: sf(|s, n, lang| l(lang, format!("Je déplace {} dans {}.", n.unwrap_or(""), chan_name(s)), format!("Moving {} to {}.", n.unwrap_or(""), chan_name(s)))),
                    ..Default::default()
                },
            ),
            (
                "disconnect",
                ActionDef {
                    role: Role::Admin,
                    who: Who::Yes,
                    confirm: true,
                    echo: Echo::Yes,
                    say: sf(|_, n, lang| l(lang, format!("Je déconnecte {}.", n.unwrap_or("")), format!("Disconnecting {}.", n.unwrap_or("")))),
                    ..Default::default()
                },
            ),
            (
                "message",
                ActionDef {
                    role: Role::Admin,
                    channel: ChanKind::Text,
                    text: true,
                    echo: Echo::Yes,
                    say: sf(|s, _, lang| l(lang, format!("J'envoie le message dans {}.", chan_name(s)), format!("Message sent to {}.", chan_name(s)))),
                    ..Default::default()
                },
            ),
            (
                "msg_edit",
                ActionDef {
                    role: Role::Admin,
                    channel: ChanKind::TextOpt,
                    text: true,
                    echo: Echo::If(|s| s.flag("found")),
                    say: sf(|s, _, lang| {
                        let f = s.flag("found");
                        l(lang, if f { "Message modifié.".into() } else { "Je n'ai pas de message à modifier ici.".into() }, if f { "Message edited.".into() } else { "I have no message to edit here.".into() })
                    }),
                    ..Default::default()
                },
            ),
            (
                "msg_delete",
                ActionDef {
                    role: Role::Admin,
                    channel: ChanKind::TextOpt,
                    echo: Echo::If(|s| s.flag("found")),
                    say: sf(|s, _, lang| {
                        let f = s.flag("found");
                        l(lang, if f { "Message supprimé.".into() } else { "Je n'ai pas de message à supprimer ici.".into() }, if f { "Message deleted.".into() } else { "I have no message to delete here.".into() })
                    }),
                    ..Default::default()
                },
            ),
            // Only Bobby's own messages: deleting other people's would need Manage Messages, which Bobby does not have.
            (
                "clean",
                ActionDef {
                    role: Role::Admin,
                    channel: ChanKind::TextOpt,
                    confirm: true,
                    say: sf(|s, _, lang| {
                        let wh = js(s.f.get("where"));
                        let c = js(s.f.get("count"));
                        if s.flag("where") {
                            l(
                                lang,
                                if s.flag("count") { format!("Je supprime mes {c} messages dans {wh}.") } else { format!("Je n'ai aucun message dans {wh}.") },
                                if s.flag("count") { format!("Deleting my {c} messages in {wh}.") } else { format!("I have no messages in {wh}.") },
                            )
                        } else {
                            l(lang, format!("Je nettoie {}.", chan_or(s, "le salon")), format!("Cleaning {}.", chan_or(s, "the channel")))
                        }
                    }),
                    ..Default::default()
                },
            ),
            // Mentions people in a text channel: users only, never @everyone / @here (client side).
            (
                "ping",
                ActionDef {
                    role: Role::Admin, // admin, like posting a message
                    who: Who::Yes,
                    channel: ChanKind::TextOpt,
                    echo: Echo::Yes,
                    say: sf(|s, n, lang| l(lang, format!("Je mentionne {} dans {}.", n.unwrap_or(""), chan_or(s, "le salon")), format!("Mentioning {} in {}.", n.unwrap_or(""), chan_or(s, "the channel")))),
                    ..Default::default()
                },
            ),
            ("leave", ActionDef { role: Role::Admin, say: sf(|_, _, lang| l(lang, "À plus.".into(), "See you.".into())), ..Default::default() }),
        ],
    }
}

fn clean_quick(m: &Captures) -> Vec<Value> {
    let c = group(m, 1);
    if c.is_empty() {
        vec![json!({ "type": "clean" })]
    } else {
        vec![json!({ "type": "clean", "channel": c })]
    }
}

/// R3b: port of the module's `run` functions (one per action type).
pub async fn run<L: bobby_proto::infer::Understand + Sync>(s: &mut Step, env: &super::Env<'_, L>, _lang: &str) -> crate::util::Res<Vec<super::UndoOp>> {
    use super::UndoOp;
    use bobby_proto::msg::Cap;
    let ids: Vec<String> = s.targets.iter().flatten().map(|t| t.id.clone()).collect();
    // Node's `chan(s, io)`: the step's channel, else Bobby's text channel (None); it ignores the writing mode.
    let chan = s.channel.as_ref().map(|c| c.id.clone());
    match s.kind.as_str() {
        "mute" | "unmute" | "deafen" | "undeafen" => {
            let on = matches!(s.kind.as_str(), "mute" | "deafen");
            let deaf = matches!(s.kind.as_str(), "deafen" | "undeafen");
            let set = |users: Vec<String>, on: bool| if deaf { Cap::Deafen { users, on } } else { Cap::Mute { users, on } };
            let r = env.io.run(set(ids, on)).await?;
            // one undo call per distinct old value
            let mut groups: Vec<(bool, Vec<String>)> = vec![];
            for (id, v) in r["prev"].as_object().into_iter().flatten() {
                let v = v.as_bool().unwrap_or(false);
                match groups.iter_mut().find(|(g, _)| *g == v) {
                    Some((_, users)) => users.push(id.clone()),
                    None => groups.push((v, vec![id.clone()])),
                }
            }
            Ok(groups.into_iter().map(|(v, users)| UndoOp::Cap(set(users, v))).collect())
        }
        "move" => {
            let channel = s.channel.as_ref().map(|c| c.id.clone()).unwrap_or_default();
            let r = env.io.run(Cap::Move { users: ids.clone(), channel: channel.clone() }).await?;
            // a move that includes the speaker ("déplace-nous", "mets-moi", "tout le monde") takes Bobby along
            let back = if ids.contains(&s.by.id) { env.io.run(Cap::Join { channel }).await.ok().and_then(|j| j["prev"].as_str().map(String::from)) } else { None };
            // null = was not in voice: nothing to put back
            let mut groups: Vec<(String, Vec<String>)> = vec![];
            for (id, c) in r["prev"].as_object().into_iter().flatten() {
                let Some(c) = c.as_str() else { continue };
                match groups.iter_mut().find(|(g, _)| g == c) {
                    Some((_, users)) => users.push(id.clone()),
                    None => groups.push((c.to_string(), vec![id.clone()])),
                }
            }
            // ops are undone last to first: people go back first, then Bobby re-joins its previous channel
            let mut undo: Vec<UndoOp> = back.map(|channel| UndoOp::Cap(Cap::Join { channel })).into_iter().collect();
            undo.extend(groups.into_iter().map(|(channel, users)| UndoOp::Cap(Cap::Move { users, channel })));
            Ok(undo)
        }
        "disconnect" => {
            env.io.run(Cap::Disconnect { users: ids }).await?;
            Ok(vec![])
        }
        "message" => {
            env.io.run(Cap::Post { channel: s.channel.as_ref().map(|c| c.id.clone()), text: Some(s.text.clone()), embeds: vec![] }).await?;
            Ok(vec![])
        }
        "msg_edit" => {
            let r = env.io.run(Cap::EditLast { channel: chan, text: s.text.clone() }).await?;
            s.set("found", r["found"].as_bool().unwrap_or(false));
            Ok(vec![])
        }
        "msg_delete" => {
            let r = env.io.run(Cap::DeleteLast { channel: chan }).await?;
            s.set("found", r["found"].as_bool().unwrap_or(false));
            Ok(vec![])
        }
        "clean" => {
            let r = env.io.run(Cap::Clean { channel: chan }).await?;
            s.set("count", r["count"].as_u64().unwrap_or(0));
            s.set("where", r["channel"].as_str().unwrap_or(""));
            Ok(vec![])
        }
        "ping" => {
            env.io.run(Cap::Ping { users: ids, channel: chan, text: s.text.clone() }).await?;
            Ok(vec![])
        }
        "leave" => {
            env.io.run(Cap::Leave).await?;
            Ok(vec![])
        }
        _ => Ok(vec![]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Chan;

    #[test]
    fn quick_regexes_compile_and_match() {
        let m = module();
        assert_eq!(m.quick.len(), 16);
        assert_eq!(m.actions.len(), 12);
        let run = |s: &str| m.quick.iter().find_map(|(r, f)| crate::util::caps(r, s).map(|c| f(&c)));
        assert_eq!(run("coupe le micro de kévin"), Some(vec![json!({"type":"mute","who":["kévin"]})]));
        assert_eq!(run("mute tout le monde"), Some(vec![json!({"type":"mute","who":["tous"]})]));
        assert_eq!(run("mets-moi dans gaming"), Some(vec![json!({"type":"move","who":["moi"],"channel":"gaming"})]));
        assert_eq!(run("nettoie memes"), Some(vec![json!({"type":"clean","channel":"memes"})]));
        assert_eq!(run("nettoie"), Some(vec![json!({"type":"clean"})]));
        assert_eq!(run("pars"), Some(vec![json!({"type":"leave"})]));
        assert_eq!(call("Appelle-moi sur Discord"), Some(vec![json!({"type":"ping","who":["moi"],"text":"📞 Tu m'as demandé de t'appeler : je ne peux pas, mais me voilà.","offer":"call"})]));
        assert_eq!(call("appelle-moi le Boss"), None);
    }

    #[test]
    fn say_lines() {
        let m = module();
        let get = |k: &str| m.actions.iter().find(|(t, _)| *t == k).unwrap().1.say.unwrap();
        let st = Step { channel: Some(Chan { id: "1".into(), name: "Gaming".into() }), ..Default::default() };
        assert_eq!(get("mute")(&st, Some("Kévin et Marie"), "fr").unwrap(), "Je coupe le micro de Kévin et Marie.");
        assert_eq!(get("mute")(&st, Some("Kévin et Marie"), "en").unwrap(), "Muting Kévin et Marie.");
        assert_eq!(get("move")(&st, Some("Kévin et Marie"), "en").unwrap(), "Moving Kévin et Marie to Gaming.");
        let mut c = Step::default();
        assert_eq!(get("clean")(&c, None, "fr").unwrap(), "Je nettoie le salon.");
        c.set("where", "memes");
        c.set("count", 3);
        assert_eq!(get("clean")(&c, None, "fr").unwrap(), "Je supprime mes 3 messages dans memes.");
    }

    use crate::actions::{Env, UndoOp};
    use crate::store::Store;
    use crate::testing::{caps, fake_client, FakeLlm};
    use bobby_proto::msg::Cap;

    /// Resolves, runs and describes `raw` against `io` (the last step's undo is returned).
    async fn exec(raw: Vec<Value>, io: &crate::io::Io) -> (Vec<Step>, Vec<UndoOp>) {
        let (ctx, store, llm) = (crate::testing::ctx(), Store::temp(), FakeLlm::new(vec![]));
        let mut steps = crate::resolve::resolve(&raw, &ctx, &store).unwrap();
        let env = Env { io, store: &store, ctx: &ctx, llm: &llm, post_to: None };
        let mut undo = vec![];
        for s in &mut steps {
            undo = run(s, &env, "fr").await.unwrap();
        }
        (steps, undo)
    }

    fn ids(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[tokio::test]
    async fn mute_undo_restores_each_old_value() {
        let (io, sent) = fake_client(|_| Ok(json!({ "prev": { "Kévin": false, "Marie": true, "Zyklon": false } })));
        let (_, undo) = exec(vec![json!({"type":"mute","who":["Kévin","Marie","Zyklon"]})], &io).await;
        assert_eq!(caps(&sent), vec![Cap::Mute { users: ids(&["Kévin", "Marie", "Zyklon"]), on: true }]);
        assert_eq!(undo.len(), 2);
        assert!(undo.contains(&UndoOp::Cap(Cap::Mute { users: ids(&["Kévin", "Zyklon"]), on: false })));
        assert!(undo.contains(&UndoOp::Cap(Cap::Mute { users: ids(&["Marie"]), on: true })));
        let (_, undo) = exec(vec![json!({"type":"undeafen","who":["Kévin"]})], &io).await;
        assert_eq!(caps(&sent).last().unwrap(), &Cap::Deafen { users: ids(&["Kévin"]), on: false });
        assert!(matches!(&undo[0], UndoOp::Cap(Cap::Deafen { .. })));
    }

    #[tokio::test]
    async fn move_takes_bobby_along_and_undoes() {
        let (io, sent) = fake_client(|c| match c {
            Cap::Move { .. } => Ok(json!({ "prev": { "Alex": "Général", "Zyklon": null } })),
            _ => Ok(json!({ "prev": "Général" })),
        });
        let (_, undo) = exec(vec![json!({"type":"move","who":["moi","Zyklon"],"channel":"Gaming"})], &io).await;
        assert_eq!(caps(&sent), vec![Cap::Move { users: ids(&["Alex", "Zyklon"]), channel: "Gaming".into() }, Cap::Join { channel: "Gaming".into() }]);
        // Zyklon was not in voice (null): nothing to put back for him
        // (undo runs the ops last to first: the move back, then Bobby's join)
        assert_eq!(undo, vec![UndoOp::Cap(Cap::Join { channel: "Général".into() }), UndoOp::Cap(Cap::Move { users: ids(&["Alex"]), channel: "Général".into() })]);
        // without the speaker, Bobby stays
        let (io, sent) = fake_client(|_| Ok(json!({ "prev": { "Marie": "Gaming" } })));
        let (_, undo) = exec(vec![json!({"type":"move","who":["Marie"],"channel":"AFK"})], &io).await;
        assert_eq!(caps(&sent), vec![Cap::Move { users: ids(&["Marie"]), channel: "AFK".into() }]);
        assert_eq!(undo, vec![UndoOp::Cap(Cap::Move { users: ids(&["Marie"]), channel: "Gaming".into() })]);
    }

    #[tokio::test]
    async fn text_actions_send_their_caps() {
        let (io, sent) = fake_client(|c| match c {
            Cap::EditLast { .. } | Cap::DeleteLast { .. } => Ok(json!({ "found": true })),
            Cap::Clean { .. } => Ok(json!({ "count": 3, "channel": "memes" })),
            _ => Ok(Value::Null),
        });
        let (steps, _) = exec(
            vec![
                json!({"type":"message","channel":"annonces","text":"salut"}),
                json!({"type":"msg_edit","text":"nouveau"}),
                json!({"type":"msg_delete","channel":"memes"}),
                json!({"type":"clean","channel":"memes"}),
                json!({"type":"ping","who":["moi"],"text":"debout"}),
                json!({"type":"disconnect","who":["Lucas"]}),
                json!({"type":"leave"}),
            ],
            &io,
        )
        .await;
        assert_eq!(
            caps(&sent),
            vec![
                Cap::Post { channel: Some("annonces".into()), text: Some("salut".into()), embeds: vec![] },
                Cap::EditLast { channel: None, text: "nouveau".into() },
                Cap::DeleteLast { channel: Some("memes".into()) },
                Cap::Clean { channel: Some("memes".into()) },
                Cap::Ping { users: ids(&["Alex"]), channel: None, text: "debout".into() },
                Cap::Disconnect { users: ids(&["lucas"]) },
                Cap::Leave,
            ]
        );
        assert!(steps[1].flag("found") && steps[2].flag("found"));
        assert_eq!((steps[3].str("where"), steps[3].f["count"].as_u64()), (Some("memes"), Some(3)));
    }
}
