//! Port of `bot/actions/info.js`: static texts, action definitions, shortcuts, and the "what can I do"
//! helpers. `run`, `facts`, `aboutMe` and the capabilities card are batch R3b.
use super::{Env, UndoOp};
use crate::format;
use crate::store::Store;
use crate::types::{ActionDef, ChanKind, Echo, Module, Role, Step};
use crate::util::{group, re, re_i, replace1, talk, BrainError, Res, Say};
use bobby_proto::infer::{ChatMsg, InferError, InferErrorKind, Understand};
use bobby_proto::msg::{Cap, Embed};
use chrono::{DateTime, Datelike, Local, Timelike};
use serde_json::{json, Value};
use std::sync::LazyLock;

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
    format_time(Local::now(), lang)
}

fn format_time(t: DateTime<Local>, lang: &str) -> String {
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


/// "Ce que je sais faire" as an embed, one field per role (what each role adds).
pub fn capabilities_card(lang: &str, store: &Store) -> Embed {
    let en = lang == "en";
    let i = usize::from(en);
    let prime = format!("Prime ({} {})", if en { "role" } else { "rôle" }, crate::roles::people(store).prime_role);
    let names = [
        (Role::Prime, prime),
        (Role::Admin, if en { "Admin (in addition)" } else { "Admin (en plus)" }.to_string()),
        (Role::Super, if en { "Super admin (everything)" } else { "Super admin (tout)" }.to_string()),
    ];
    let fields: Vec<(String, String)> = names
        .iter()
        .map(|(r, name)| {
            let lines: Vec<String> = crate::actions::modules()
                .iter()
                .filter(|m| !m.help[0].is_empty() && module_role(m) == Some(*r))
                .map(|m| format!("• {}", if m.help[i].is_empty() { m.help[0] } else { m.help[i] }))
                .collect();
            (name.clone(), lines.join("\n"))
        })
        .collect();
    let fields: Vec<(&str, &str, bool)> = fields.iter().map(|(n, v)| (n.as_str(), v.as_str(), false)).collect();
    format::card(
        if en { "🤖 What I can do" } else { "🤖 Ce que je sais faire" },
        format::INFO,
        Some(if en { "Say « Bobby, … » in voice." } else { "Dis « Bobby, … » en vocal." }),
        &fields,
        None,
    )
}

fn role_name(r: Role) -> &'static str {
    match r {
        Role::Super => "super admin",
        Role::Admin => "admin",
        Role::Prime => "Prime",
        Role::Other => "sans rôle",
    }
}

/// Everything Bobby knows about itself, rebuilt at each question: nothing here goes stale when rights change.
pub async fn facts<L: Understand + Sync>(s: &Step, env: &Env<'_, L>) -> Res<String> {
    facts_at(s, env, chrono::Utc::now().timestamp_millis()).await
}

/// `facts` with a given "now" (ms since the epoch) for the conversation's "il y a N min".
async fn facts_at<L: Understand + Sync>(s: &Step, env: &Env<'_, L>, now_ms: i64) -> Res<String> {
    let ctx = env.ctx;
    // a member in voice, else Discord's own name for the id (the id itself if Discord does not know it)
    let name = |id: String| async move {
        if let Some(m) = ctx.members.iter().find(|m| m.id == id && !m.name.is_empty()) {
            return m.name.clone();
        }
        match env.io.run(Cap::MemberName { id: id.clone() }).await {
            Ok(v) => v["name"].as_str().filter(|n| !n.is_empty()).map_or(id, str::to_string),
            Err(_) => id,
        }
    };
    let names = |ids: Vec<String>| {
        let name = &name;
        async move {
            let mut out = vec![];
            for id in ids {
                out.push(name(id).await);
            }
            if out.is_empty() { "personne".to_string() } else { out.join(", ") }
        }
    };
    let p = crate::roles::people(env.store);
    let mut nick = vec![];
    if let Value::Object(nicknames) = env.store.load_or("aliases", json!({})) {
        for (id, l) in nicknames {
            let l: Vec<&str> = l.as_array().map(|a| a.iter().map(|x| x.as_str().unwrap_or("")).collect()).unwrap_or_default();
            if !l.is_empty() {
                nick.push(format!("{} = {}", name(id).await, l.join(", ")));
            }
        }
    }
    let nick = nick.join(" ; ");
    let not_done = |it: &Value| matches!(&it["done"], Value::Null | Value::Bool(false)) || it["done"] == 0 || it["done"] == "";
    let general = crate::actions::todo::items(env.store).iter().filter(|it| not_done(it) && it["list"] == "general").count();
    let mut lines = vec![
        "Tu es Bobby, l'assistant vocal du serveur Discord. Tout tourne en local sur le PC du super admin, gratuitement.".to_string(),
        "Mot d'activation : « Bobby », repéré par un petit modèle openWakeWord entraîné sur des voix de synthèse et 20 enregistrements du super admin. Il laisse passer environ 9 « Bobby » sur 10 : pas encore parfait, on prévoit de l'améliorer avec plus d'enregistrements.".to_string(),
        "Oreilles : Whisper large-v3-turbo (français et anglais). Cerveau : Qwen3-4B en local. Voix : Piper (siwis en français, lessac en anglais). Les questions de culture générale partent à Claude Haiku, qui répond avec une autre voix (tom en français, ryan en anglais). Tout le reste est traité en local.".to_string(),
        format!("Ce que tu sais faire : {}.", capabilities("fr")),
        format!(
            "Rôles : Prime (rôle Discord « {} ») peut : {}. Admin peut en plus : {}. Super admin peut tout : {}. Les autres n'ont aucun droit, tu leur dis que tu n'as pas le droit de leur répondre.",
            p.prime_role,
            abilities(Role::Prime, "fr"),
            abilities(Role::Admin, "fr"),
            abilities(Role::Super, "fr")
        ),
        format!(
            "Super admin : {}. Il ne change qu'à la main dans le fichier .env, jamais à la voix. Admins : {}. Privés de droits (liste noire) : {}.",
            names(p.supers.clone()).await,
            names(p.admins.clone()).await,
            names(p.blacklist.clone()).await
        ),
        format!("Listes de tâches : une liste générale partagée et une liste perso par personne. Chacun ajoute à la générale et à la sienne et gère ses propres points ; le super admin gère tout. Liste générale : {general} points à faire."),
        format!("Surnoms : {}.", if nick.is_empty() { "aucun" } else { &nick }),
        format!("Celui qui te parle : {} ({}).", if s.by.name.is_empty() { "?" } else { &s.by.name }, role_name(s.by.role)),
    ];
    if !ctx.history.is_empty() {
        lines.push("Conversation récente avec toi (tous ceux qui te parlent) :".into());
        lines.extend(talk(&ctx.history, now_ms));
    }
    if ctx.trouble.is_empty() {
        lines.push("Ton journal : aucun échec récent.".into());
    } else {
        lines.push("Ton journal, ce qui a raté ou attend encore (du plus ancien au plus récent) :".into());
        lines.extend(ctx.trouble.iter().cloned());
    }
    Ok(lines.join("\n"))
}

/// util.js `llm`'s error mapping (same as `prompt::ask_llm`).
fn llm_error(e: InferError) -> BrainError {
    match e.kind {
        InferErrorKind::Down => Say::new("Mon cerveau ne répond pas.", "My brain isn't responding.").code("LLM_DOWN").into(),
        InferErrorKind::Timeout => Say::new("Mon cerveau est trop lent, la carte graphique est saturée.", "My brain is too slow, the graphics card is full.").code("LLM_DOWN").into(),
        InferErrorKind::Bad if e.msg.starts_with("HTTP ") => BrainError::failed(format!("LLM {}", e.msg[5..].split_whitespace().next().unwrap_or(""))),
        InferErrorKind::Bad => Say::new("Je n'ai pas compris.", "Sorry, I didn't get that.").code("LLM_JSON").into(),
    }
}

static QUESTION: LazyLock<fancy_regex::Regex> =
    LazyLock::new(|| re_i(r"\?\s*$|^(?:pourquoi|comment|est-ce|qu(?:'|e |oi|el|i )|où|quand|combien|why|how|what|who|when|where|do |does |did |can |are |is )"));
static OFFER: LazyLock<fancy_regex::Regex> = LazyLock::new(|| re_i(r"\s*(?:Tu veux que je l'ajoute à la liste|Do you want me to add it to the list)\s*\?\s*$"));

/// The system prompt of `about_me`: the facts, then the rules of the answer.
fn about_system(facts: &str, lang: &str) -> String {
    format!(
        "{facts}\n\nRéponds à la question ou à la remarque sur toi en une ou deux phrases courtes, à dire à voix haute, en {}, à partir de ces faits uniquement. Si (et seulement si) on te fait une critique ou une suggestion d'amélioration, pas une question, remercie et termine par « Tu veux que je l'ajoute à la liste ? » (en anglais : « Do you want me to add it to the list? »). Si on te demande pourquoi une demande n'est pas partie, n'a pas marché ou n'a pas eu de réponse, explique-le à partir de ton journal ci-dessus, en mots simples ; si le journal ne l'explique pas, dis que tu ne vois aucune erreur dans ton journal, sans rien inventer. Ne dis jamais que tu as noté ou retenu quelque chose. Réponds en JSON {{\"say\": \"...\"}}.",
        if lang == "en" { "anglais" } else { "français" }
    )
}

async fn about_me<L: Understand + Sync>(s: &Step, env: &Env<'_, L>, lang: &str) -> Res<String> {
    let system = about_system(&facts(s, env).await?, lang);
    let schema = json!({ "type": "object", "properties": { "say": { "type": "string" } }, "required": ["say"] });
    let messages = [ChatMsg::new("system", &system), ChatMsg::new("user", &s.text)];
    let out = env.llm.chat(&messages, &schema, 200).await.map_err(llm_error)?;
    let say = out["say"].as_str().unwrap_or("");
    // ponytail: a question never gets the "add it to the list?" offer, whatever the model thought (2026-11-13).
    let question = crate::util::hit(&QUESTION, s.text.trim());
    Ok(if question { replace1(&OFFER, say, "") } else { say.to_string() })
}

/// Port of the module's `run` functions (one per action type).
pub async fn run<L: Understand + Sync>(s: &mut Step, env: &Env<'_, L>, lang: &str) -> Res<Vec<UndoOp>> {
    match s.kind.as_str() {
        "time" => {}
        // posted only when asked in writing
        "help" => s.set("card", serde_json::to_value(vec![capabilities_card(lang, env.store)]).unwrap_or(Value::Null)),
        "come" => {
            let channel = s.channel.as_ref().ok_or_else(|| BrainError::failed("come without a channel"))?.id.clone();
            let v = env.io.run(Cap::Join { channel }).await?;
            return Ok(match v["prev"].as_str().filter(|p| !p.is_empty()) {
                Some(prev) => vec![UndoOp::Cap(Cap::Join { channel: prev.to_string() })],
                None => vec![],
            });
        }
        "about" => {
            let a = about_me(s, env, lang).await?;
            s.set("answer", a);
        }
        _ => {}
    }
    Ok(vec![])
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

    // ---------- running steps (batch R3b) ----------

    use crate::testing::{caps as sent_caps, fake_client, FakeLlm};
    use crate::types::{By, Chan, Ctx, Person};
    use crate::util::Hist;
    use chrono::TimeZone;
    use std::sync::{Arc, Mutex};

    fn step(kind: &str, text: &str) -> Step {
        Step { kind: kind.into(), text: text.into(), ..Default::default() }
    }

    #[test]
    fn date_and_time_strings() {
        // expected strings from Node: new Date(...).toLocaleString('fr-FR' | 'en-GB', { weekday, day, month: 'long', hour, minute: '2-digit' })
        let t = |y, mo, d, h, mi| Local.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap();
        assert_eq!(format_time(t(2026, 11, 16, 19, 5), "fr"), "lundi 16 novembre à 19:05");
        assert_eq!(format_time(t(2026, 11, 16, 19, 5), "en"), "Monday 16 November at 19:05");
        assert_eq!(format_time(t(2026, 3, 1, 0, 7), "fr"), "dimanche 1 mars à 00:07");
        assert_eq!(format_time(t(2026, 3, 1, 0, 7), "en"), "Sunday 1 March at 00:07");
        assert_eq!(format_time(t(2026, 8, 2, 9, 30), "fr"), "dimanche 2 août à 09:30");
        assert_eq!(format_time(t(2026, 8, 2, 9, 30), "en"), "Sunday 2 August at 09:30");
        assert!(say_time(&Step::default(), None, "fr").unwrap().starts_with("Nous sommes "));
        assert!(say_time(&Step::default(), None, "en").unwrap().starts_with("It's "));
    }

    #[tokio::test]
    async fn help_card_and_say() {
        let (io, _) = fake_client(|_| Ok(json!({})));
        let (store, ctx, llm) = (Store::temp(), crate::testing::ctx(), FakeLlm::new(vec![]));
        let env = Env { io: &io, store: &store, ctx: &ctx, llm: &llm, post_to: None };
        let mut s = step("help", "");
        assert!(run(&mut s, &env, "fr").await.unwrap().is_empty());
        let cards: Vec<Embed> = serde_json::from_value(s.f["card"].clone()).unwrap();
        assert_eq!(cards.len(), 1);
        let c = &cards[0];
        assert_eq!(c.title, "🤖 Ce que je sais faire");
        assert_eq!(c.description, "Dis « Bobby, … » en vocal.");
        assert_eq!(c.color, Some(format::INFO));
        let names: Vec<&str> = c.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, [format!("Prime (rôle {})", crate::roles::prime_role()).as_str(), "Admin (en plus)", "Super admin (tout)"]);
        assert!(c.fields[0].value.contains("• la musique") || c.fields[0].value.contains("• jouer"), "{}", c.fields[0].value);
        assert!(c.fields.iter().all(|f| f.value.split('\n').all(|l| l.starts_with("• "))));
        let en = capabilities_card("en", &store);
        assert_eq!(en.title, "🤖 What I can do");
        assert_eq!(en.fields[1].name, "Admin (in addition)");
        assert!(en.fields[0].value.contains("• tell the time"));
        // "Je sais ... couper" (unit.test.js, "claude off + help")
        let say = (module().actions.iter().find(|(k, _)| *k == "help").unwrap().1.say.unwrap())(&s, None, "fr").unwrap();
        assert!(say.starts_with("Je sais ") && say.contains("couper"), "{say}");
    }

    #[tokio::test]
    async fn come_joins_and_undoes_to_the_previous_channel() {
        let (io, sent) = fake_client(|cap| match cap {
            Cap::Join { channel } if channel == "Général" => Ok(json!({ "prev": "Gaming" })),
            _ => Ok(json!({ "prev": null })),
        });
        let (store, ctx, llm) = (Store::temp(), crate::testing::ctx(), FakeLlm::new(vec![]));
        let env = Env { io: &io, store: &store, ctx: &ctx, llm: &llm, post_to: None };
        let mut s = step("come", "");
        s.channel = Some(Chan { id: "Général".into(), name: "Général".into() });
        assert_eq!(run(&mut s, &env, "fr").await.unwrap(), vec![UndoOp::Cap(Cap::Join { channel: "Gaming".into() })]);
        // not in a channel before: nothing to undo
        s.channel = Some(Chan { id: "AFK".into(), name: "AFK".into() });
        assert!(run(&mut s, &env, "fr").await.unwrap().is_empty());
        assert_eq!(sent_caps(&sent), vec![Cap::Join { channel: "Général".into() }, Cap::Join { channel: "AFK".into() }]);
        // time does nothing
        assert!(run(&mut step("time", ""), &env, "fr").await.unwrap().is_empty());
        assert_eq!(sent_caps(&sent).len(), 2);
    }

    fn person(id: &str, name: &str) -> Person {
        Person { id: id.into(), name: name.into(), ..Default::default() }
    }

    /// Expected lines come from Node: `facts(s)` of bot/actions/info.js with DATA_DIR on temp files holding the same
    /// aliases and to-do list, `Date.now` fixed, and no SUPER_ADMINS / ADMINS / PRIME_ROLE in the environment.
    #[tokio::test]
    async fn facts_match_node() {
        let (io, _) = fake_client(|_| Err("unknown member".into()));
        let store = Store::temp();
        store.save("aliases", &json!({ "Zyklon": ["le Grec", "Zy"], "inconnu-id": ["inconnu"], "Marie": [] })).unwrap();
        let todo = |id, list, done| json!({ "id": id, "text": "x", "list": list, "author": { "id": null, "name": "?" }, "at": null, "done": done });
        store.save("todo", &json!([todo(1, "general", false), todo(2, "general", true), todo(3, "general", false), todo(4, "Alex", false)])).unwrap();
        let now = 1_800_000_000_000;
        let ctx = Ctx {
            members: vec![person("261531632589078528", "WakandeX"), person("Zyklon", "Zyklon"), person("Marie", "Marie")],
            history: vec![
                Hist { at: now - 5 * 60_000, who: "Alex".into(), query: "mets du jazz".into(), did: "music_play".into(), reply: "Jazz FM.".into(), ..Default::default() },
                Hist { query: "salut".into(), ..Default::default() },
            ],
            trouble: vec![r#"- Alex : "ouvre" -> pas compris"#.into()],
            ..Default::default()
        };
        let llm = FakeLlm::new(vec![]);
        let env = Env { io: &io, store: &store, ctx: &ctx, llm: &llm, post_to: None };
        let mut s = step("about", "x");
        s.by = By { id: "a".into(), name: "Alex".into(), role: Role::Prime };
        let got = facts_at(&s, &env, now).await.unwrap();
        let expected: [&[&str]; 2] = [
    &[
        "Tu es Bobby, l'assistant vocal du serveur Discord. Tout tourne en local sur le PC du super admin, gratuitement.",
        "Mot d'activation : « Bobby », repéré par un petit modèle openWakeWord entraîné sur des voix de synthèse et 20 enregistrements du super admin. Il laisse passer environ 9 « Bobby » sur 10 : pas encore parfait, on prévoit de l'améliorer avec plus d'enregistrements.",
        "Oreilles : Whisper large-v3-turbo (français et anglais). Cerveau : Qwen3-4B en local. Voix : Piper (siwis en français, lessac en anglais). Les questions de culture générale partent à Claude Haiku, qui répond avec une autre voix (tom en français, ryan en anglais). Tout le reste est traité en local.",
        "Ce que tu sais faire : retenir, renommer, supprimer et lister les surnoms, poser les questions de culture générale à Claude, couper ou rétablir le micro et le son, déplacer, ramener ou déconnecter des gens, écrire dans un salon, mentionner quelqu'un, modifier ou supprimer mes propres messages, dire l'heure, venir dans ton salon vocal et répondre aux questions sur moi, jouer de la musique en continu (un style, un titre, ou que un artiste), une radio, pause, suivant, file d'attente, volume, piloter le PC (musique, volume, lancer et fermer des jeux et applications, verrouiller, veille, éteindre), gérer les droits de chacun et dire qui a quels droits, tenir la liste générale et la liste perso de chacun (ajouter, lire, cocher, supprimer, déplacer).",
        "Rôles : Prime (rôle Discord « Team Prime ») peut : l'heure, me faire venir et les questions sur moi, la musique, les listes de tâches. Admin peut en plus : les questions à Claude, la modération et les messages, l'heure, me faire venir et les questions sur moi, la musique, les listes de tâches. Super admin peut tout : les surnoms, les questions à Claude, la modération et les messages, l'heure, me faire venir et les questions sur moi, la musique, le PC, les droits, les listes de tâches. Les autres n'ont aucun droit, tu leur dis que tu n'as pas le droit de leur répondre.",
        "Super admin : WakandeX. Il ne change qu'à la main dans le fichier .env, jamais à la voix. Admins : 397478613559148545. Privés de droits (liste noire) : personne.",
        "Listes de tâches : une liste générale partagée et une liste perso par personne. Chacun ajoute à la générale et à la sienne et gère ses propres points ; le super admin gère tout. Liste générale : 2 points à faire.",
        "Surnoms : Zyklon = le Grec, Zy ; inconnu-id = inconnu.",
        "Celui qui te parle : Alex (Prime).",
        "Conversation récente avec toi (tous ceux qui te parlent) :",
        "- il y a 5 min, Alex : \"mets du jazz\" -> fait : music_play -> Bobby : \"Jazz FM.\"",
        "- ? : \"salut\"",
        "Ton journal, ce qui a raté ou attend encore (du plus ancien au plus récent) :",
        "- Alex : \"ouvre\" -> pas compris",
    ],
    &[
        "Tu es Bobby, l'assistant vocal du serveur Discord. Tout tourne en local sur le PC du super admin, gratuitement.",
        "Mot d'activation : « Bobby », repéré par un petit modèle openWakeWord entraîné sur des voix de synthèse et 20 enregistrements du super admin. Il laisse passer environ 9 « Bobby » sur 10 : pas encore parfait, on prévoit de l'améliorer avec plus d'enregistrements.",
        "Oreilles : Whisper large-v3-turbo (français et anglais). Cerveau : Qwen3-4B en local. Voix : Piper (siwis en français, lessac en anglais). Les questions de culture générale partent à Claude Haiku, qui répond avec une autre voix (tom en français, ryan en anglais). Tout le reste est traité en local.",
        "Ce que tu sais faire : retenir, renommer, supprimer et lister les surnoms, poser les questions de culture générale à Claude, couper ou rétablir le micro et le son, déplacer, ramener ou déconnecter des gens, écrire dans un salon, mentionner quelqu'un, modifier ou supprimer mes propres messages, dire l'heure, venir dans ton salon vocal et répondre aux questions sur moi, jouer de la musique en continu (un style, un titre, ou que un artiste), une radio, pause, suivant, file d'attente, volume, piloter le PC (musique, volume, lancer et fermer des jeux et applications, verrouiller, veille, éteindre), gérer les droits de chacun et dire qui a quels droits, tenir la liste générale et la liste perso de chacun (ajouter, lire, cocher, supprimer, déplacer).",
        "Rôles : Prime (rôle Discord « Team Prime ») peut : l'heure, me faire venir et les questions sur moi, la musique, les listes de tâches. Admin peut en plus : les questions à Claude, la modération et les messages, l'heure, me faire venir et les questions sur moi, la musique, les listes de tâches. Super admin peut tout : les surnoms, les questions à Claude, la modération et les messages, l'heure, me faire venir et les questions sur moi, la musique, le PC, les droits, les listes de tâches. Les autres n'ont aucun droit, tu leur dis que tu n'as pas le droit de leur répondre.",
        "Super admin : 261531632589078528. Il ne change qu'à la main dans le fichier .env, jamais à la voix. Admins : 397478613559148545. Privés de droits (liste noire) : personne.",
        "Listes de tâches : une liste générale partagée et une liste perso par personne. Chacun ajoute à la générale et à la sienne et gère ses propres points ; le super admin gère tout. Liste générale : 2 points à faire.",
        "Surnoms : Zyklon = le Grec, Zy ; inconnu-id = inconnu.",
        "Celui qui te parle : ? (sans rôle).",
        "Ton journal : aucun échec récent.",
    ]
        ];
        assert_eq!(got.split('\n').collect::<Vec<_>>(), expected[0]);
        // nobody in voice, no history, no trouble, an unnamed speaker without role
        let empty = Ctx::default();
        let env = Env { io: &io, store: &store, ctx: &empty, llm: &llm, post_to: None };
        let got = facts_at(&step("about", "x"), &env, now).await.unwrap();
        assert_eq!(got.split('\n').collect::<Vec<_>>(), expected[1]);
    }

    #[tokio::test]
    async fn about_sends_the_facts_and_drops_the_offer_on_a_question() {
        let (io, _) = fake_client(|_| Err("unknown member".into()));
        let (store, ctx) = (Store::temp(), crate::testing::ctx());
        let llm = FakeLlm::new(vec![json!({ "say": "Merci ! Tu veux que je l'ajoute à la liste ?" })]);
        let env = Env { io: &io, store: &store, ctx: &ctx, llm: &llm, post_to: None };
        // a critique keeps the offer
        let mut s = step("about", "ta voix est trop lente");
        run(&mut s, &env, "fr").await.unwrap();
        assert_eq!(s.str("answer"), Some("Merci ! Tu veux que je l'ajoute à la liste ?"));
        {
            let seen = llm.seen.lock().unwrap();
            let m = &seen[0];
            assert_eq!((m.len(), m[0].role.as_str(), m[1].role.as_str(), m[1].content.as_str()), (2, "system", "user", "ta voix est trop lente"));
            assert!(m[0].content.starts_with("Tu es Bobby, l'assistant vocal du serveur Discord."));
            assert!(m[0].content.contains("\n\nRéponds à la question ou à la remarque sur toi en une ou deux phrases courtes, à dire à voix haute, en français, à partir"));
            assert!(m[0].content.ends_with(r#"Ne dis jamais que tu as noté ou retenu quelque chose. Réponds en JSON {"say": "..."}."#));
        }
        // a question never gets the offer, whatever the model said
        let mut q = step("about", "pourquoi ta voix est lente ?");
        run(&mut q, &env, "en").await.unwrap();
        assert_eq!(q.str("answer"), Some("Merci !"));
        assert!(llm.seen.lock().unwrap()[1][0].content.contains("à voix haute, en anglais, à partir"));
        // an English question, offer in English
        let llm = FakeLlm::new(vec![json!({ "say": "Good point. Do you want me to add it to the list? " })]);
        let env = Env { io: &io, store: &store, ctx: &ctx, llm: &llm, post_to: None };
        let mut q = step("about", "what is your voice");
        run(&mut q, &env, "en").await.unwrap();
        assert_eq!(q.str("answer"), Some("Good point."));
    }

    #[tokio::test]
    async fn about_maps_llm_errors_like_util_js() {
        use bobby_proto::infer::InferError;
        let (io, _) = fake_client(|_| Ok(json!({})));
        let (store, ctx) = (Store::temp(), crate::testing::ctx());
        let err = |kind, msg: &str| FakeLlm { answers: Arc::new(Mutex::new(vec![Err(InferError::new(kind, msg))])), seen: Arc::default() };
        let run_with = |llm: FakeLlm| {
            let (io, store, ctx) = (io.clone(), &store, &ctx);
            async move {
                let env = Env { io: &io, store, ctx, llm: &llm, post_to: None };
                run(&mut step("about", "t"), &env, "fr").await.unwrap_err()
            }
        };
        let say = |fr: &str, en: &str, code| BrainError::Say(Say::new(fr, en).code(code));
        assert_eq!(run_with(err(InferErrorKind::Down, "refused")).await, say("Mon cerveau ne répond pas.", "My brain isn't responding.", "LLM_DOWN"));
        assert_eq!(run_with(err(InferErrorKind::Bad, "HTTP 500 oops")).await, BrainError::failed("LLM 500"));
        assert_eq!(run_with(err(InferErrorKind::Bad, "invalid JSON")).await, say("Je n'ai pas compris.", "Sorry, I didn't get that.", "LLM_JSON"));
    }
}
