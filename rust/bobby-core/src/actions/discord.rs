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
pub async fn run<L: bobby_proto::infer::Understand + Sync>(_s: &mut crate::types::Step, _env: &super::Env<'_, L>, _lang: &str) -> crate::util::Res<Vec<super::UndoOp>> {
    Ok(vec![]) // R3b
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
}
