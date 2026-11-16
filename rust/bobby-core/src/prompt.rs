//! Port of `bot/brain.js` lines 48-201: the LLM schema, the fixed system prompt, the per-request prompt
//! and the call itself. Byte-identical to Node (see tests/fixtures/dump-prompts.js).
use crate::actions::{self, info};
use crate::store::Store;
use crate::types::Ctx;
use crate::util::{talk, BrainError, Hist, Say};
use bobby_proto::infer::{ChatMsg, InferErrorKind, Understand};
use chrono::{DateTime, Datelike, Local, Timelike};
use serde_json::{json, Map, Value};
use std::sync::LazyLock;

/// Reasoning first: what kind of request it is and whether Bobby can do it, then the actions. ~10 more tokens.
pub const KINDS: [&str; 4] = ["ordre", "capacite", "correction", "discussion"];

pub fn schema() -> Value {
    let mut props = Map::new();
    props.insert("type".into(), json!({ "enum": actions::types() }));
    props.insert("who".into(), json!({ "type": "array", "items": { "type": "string" } }));
    props.insert("except".into(), json!({ "type": "array", "items": { "type": "string" } }));
    props.insert("channel".into(), json!({ "type": "string" }));
    props.insert("text".into(), json!({ "type": "string" }));
    for m in actions::modules() {
        for (k, v) in &m.fields {
            props.insert((*k).into(), v.clone());
        }
    }
    json!({
        "type": "object",
        "properties": {
            "analyse": {
                "type": "object",
                "properties": {
                    "genre": { "enum": KINDS },
                    "faisable": { "type": "boolean" },
                    "pourquoi": { "type": "string", "maxLength": 80 },
                },
                "required": ["genre", "faisable"],
            },
            "actions": {
                "type": "array",
                "items": { "type": "object", "properties": props, "required": ["type"] },
            },
            // one short spoken sentence; also keeps the JSON under max_tokens
            "say": { "type": "string", "maxLength": 160 },
            "ecrit": { "type": "boolean" },
        },
        "required": ["analyse", "actions", "say"],
    })
}

const SYS_A: &str = r#"Tu es Bobby, l'assistant vocal d'un serveur Discord. Tu reçois une phrase transcrite à l'oral (parfois avec des fautes), en français ou en anglais, et le contexte du serveur. Tu réponds uniquement en JSON.

Actions possibles (champ "type") :
"#;
const SYS_B: &str = r#"
- undo : annuler la dernière action

Ce que tu sais faire : "#;
const SYS_C: &str = r#".

Réfléchis d'abord dans "analyse" :
- "genre" : ordre (le cas normal : on te demande d'agir ou de répondre, y compris toute question sur toi, ton avis, tes fonctionnalités ou le monde ; "peux-tu…", "tu peux…", "est-ce que tu peux…" sont des ordres polis), capacite (seulement quand on demande si tu es capable de faire une action sans te demander de la faire : "es-tu capable de…", "sais-tu + verbe", ou tes limites : "quelle est ta plage de volume"), correction (on corrige ta dernière réponse ou action : "non pas ça", "pas lui", "je voulais dire…", "j'aurais voulu…"), discussion (salutation, politesse, bavardage).
- "faisable" : true dès qu'une action de la liste le fait (Discord, musique, volume, PC, Spotify, GPU, listes, surnoms, droits, questions à Claude…). false seulement si aucune action ne le couvre (passer un appel, supprimer les messages des autres, bannir, crier) : alors "actions" vide, la raison en quelques mots dans "pourquoi", et "say" le dit en une phrase courte, sans jamais choisir une action voisine.
- capacite : "actions" = l'action qui le ferait (elle n'est exécutée qu'après un oui), "say" = la réponse courte ("Oui, je peux.", "Non, …").
- correction : refais la dernière demande de la conversation récente en appliquant la correction (mêmes actions, corrigées).
- La conversation récente (tous ceux qui parlent à Bobby) sert à comprendre "ça", "il", "le dernier", "refais", "pareil pour lui", "non pas ça".
- "ecrit" : true seulement si on veut la réponse écrite dans le salon texte (par écrit, dans le chat, en tableau, en liste mise en forme).

Règles :
- Tout se traite en local par défaut. Questions sur toi, tes capacités, tes réglages, ta voix, ton mot d'activation, les droits, les gens du serveur, la conversation en cours, ou remarque / avis / critique sur toi : action about avec la question complète dans "text".
- "réessaie" ou "try again" : refais la dernière demande de la conversation (mêmes actions).
- "stop" tout seul arrête la musique (music_stop), jamais leave. leave seulement si on dit à Bobby de partir ("pars", "quitte le salon", "tu peux partir").
- "who" contient des noms de personnes, "moi" pour celui qui parle, ou "tous" pour tout le monde dans son salon, même si la phrase est en anglais. "except" liste les personnes exclues.
- Les noms sont souvent mal transcrits : écris le nom du contexte qui ressemble le plus à ce qui a été dit.
- Plusieurs demandes dans une phrase donnent plusieurs actions, dans l'ordre.
"#;
const SYS_D: &str = r#"
- Demande impossible ou incompréhensible : "actions" vide, et "say" le dit en une phrase courte. Ne prétends jamais avoir fait, noté ou retenu quelque chose sans l'action correspondante.
- Quand il y a des actions (hors capacite), "say" reste vide : Bobby s'en occupe sans commenter.
- "say" : une seule phrase courte, comme un collègue (pas de "je suis un assistant vocal"), toujours dans la langue de la phrase : français si on te parle français, anglais si on te parle anglais.

Exemples :
"non annule ça" -> {"analyse":{"genre":"correction","faisable":true},"actions":[{"type":"undo"}],"say":""}
"es-tu capable de couper le micro de Lucas" -> {"analyse":{"genre":"capacite","faisable":true},"actions":[{"type":"mute","who":["Lucas"]}],"say":"Oui, je peux."}
"est-ce que tu peux monter le volume à 300" -> {"analyse":{"genre":"ordre","faisable":true},"actions":[{"type":"music_volume","text":"300"}],"say":""}
"es-tu capable de supprimer mes messages" -> {"analyse":{"genre":"capacite","faisable":false,"pourquoi":"je ne supprime que mes propres messages"},"actions":[],"say":"Non, je ne peux supprimer que mes propres messages."}
"est-ce que tu sais crier" -> {"analyse":{"genre":"capacite","faisable":false,"pourquoi":"ma voix a un volume fixe"},"actions":[],"say":"Non, ma voix garde toujours le même volume."}
"#;

static SYSTEM: LazyLock<String> = LazyLock::new(|| {
    let join = |f: fn(&crate::types::Module) -> &Vec<&'static str>| actions::modules().iter().flat_map(|m| f(m).iter().copied()).collect::<Vec<_>>().join("\n");
    // older examples have no "analyse": the JSON grammar adds it (writing it in each cost 1100 tokens)
    [SYS_A, &join(|m| &m.prompt), SYS_B, &info::capabilities("fr"), SYS_C, &join(|m| &m.rules), SYS_D, &join(|m| &m.examples)].concat()
});

/// Fixed and first: llama-server caches this prefix, so only the tail is processed for each sentence.
/// The prompt stays in French, the language it was tuned and tested in; the model handles English input fine.
pub fn system() -> &'static str {
    &SYSTEM
}

/// `new Date().toLocaleString('fr-FR', { dateStyle: 'full', timeStyle: 'short' })`: "lundi 16 novembre 2026 à 19:05".
pub fn date_line(now: DateTime<Local>) -> String {
    let day = info::FR_DAYS[now.weekday().num_days_from_monday() as usize];
    format!("{day} {} {} {} à {:02}:{:02}", now.day(), info::FR_MONTHS[now.month0() as usize], now.year(), now.hour(), now.minute())
}

pub fn prompt(ctx: &Ctx, query: &str, lang: &str, history: &[Hist], store: &Store, now: DateTime<Local>) -> String {
    // members grouped by channel, in first-seen order
    let mut by_chan: Vec<(String, Vec<String>)> = vec![];
    for m in &ctx.members {
        let chan = m.channel.clone().unwrap_or_else(|| "undefined".into());
        let who = if m.nicknames.is_empty() { m.name.clone() } else { format!("{} [surnoms : {}]", m.name, m.nicknames.join(", ")) };
        match by_chan.iter_mut().find(|(c, _)| *c == chan) {
            Some((_, v)) => v.push(who),
            None => by_chan.push((chan, vec![who])),
        }
    }
    // JS object key order: integer-like keys ("2", "100") come first, ascending
    let int_key = |c: &str| (!c.is_empty() && c.len() < 10 && c.bytes().all(|b| b.is_ascii_digit()) && (c == "0" || !c.starts_with('0'))).then(|| c.parse::<u64>().unwrap());
    by_chan.sort_by_key(|(c, _)| int_key(c).map_or((1, 0), |n| (0, n))); // stable: the others keep first-seen order
    let voice = by_chan.iter().map(|(c, n)| format!("{c} ({})", n.join(", "))).collect::<Vec<_>>().join(" ; ");
    let names = |v: &[crate::types::Chan]| v.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ");
    let mut lines = vec![
        format!("Celui qui parle : {}, dans le salon vocal {}.", ctx.speaker.name, ctx.speaker.channel.as_deref().filter(|c| !c.is_empty()).unwrap_or("aucun")),
        format!("En vocal : {}.", if voice.is_empty() { "personne" } else { &voice }),
        format!("Salons vocaux : {}.", names(&ctx.voice_channels)),
        format!("Salons texte : {}.", names(&ctx.text_channels)),
        format!("Date : {}.", date_line(now)),
    ];
    for m in actions::modules() {
        if let Some(f) = m.prompt_lines {
            lines.extend(f(ctx, store));
        }
    }
    if !history.is_empty() {
        lines.push("Conversation récente avec Bobby (du plus ancien au plus récent) :".into());
        lines.extend(talk(history, now.timestamp_millis()));
    }
    lines.push(String::new());
    lines.push(format!("Réponds dans \"say\" en {}.", if lang == "en" { "anglais" } else { "français" }));
    lines.push(format!("Phrase : \"{query}\""));
    lines.join("\n")
}

/// The `llm([system, user], SCHEMA, 400)` call of `understand`, with util.js's error mapping.
pub async fn ask_llm<L: Understand>(llm: &L, ctx: &Ctx, query: &str, lang: &str, history: &[Hist], store: &Store, now: DateTime<Local>) -> Result<Value, BrainError> {
    let messages = [ChatMsg::new("system", system()), ChatMsg::new("user", &prompt(ctx, query, lang, history, store, now))];
    llm.chat(&messages, &schema(), 400).await.map_err(|e| match e.kind {
        InferErrorKind::Down => Say::new("Mon cerveau ne répond pas.", "My brain isn't responding.").code("LLM_DOWN").into(),
        InferErrorKind::Timeout => Say::new("Mon cerveau est trop lent, la carte graphique est saturée.", "My brain is too slow, the graphics card is full.").code("LLM_DOWN").into(),
        InferErrorKind::Bad if e.msg.starts_with("HTTP ") => BrainError::failed(format!("LLM {}", e.msg[5..].split_whitespace().next().unwrap_or(""))),
        // a JSON cut by max_tokens (a reply far too long): not understood, never "ça n'a pas marché"
        InferErrorKind::Bad => Say::new("Je n'ai pas compris.", "Sorry, I didn't get that.").code("LLM_JSON").into(),
    })
}

/// The 4B model is lost: an order it calls feasible, with no action and nothing to say.
pub fn unsure(out: &Value) -> bool {
    let a = &out["analyse"];
    let truthy = |v: &Value| match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::String(s) => !s.is_empty(),
        _ => true,
    };
    a["genre"] == "ordre" && truthy(&a["faisable"]) && out["actions"].as_array().is_none_or(|x| x.is_empty()) && !truthy(&out["say"])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use crate::types::{Person, Role};
    use bobby_proto::infer::{InferError, InferErrorKind};
    use chrono::TimeZone;

    /// 2026-11-16 19:05 local time (the fixtures were made with TZ=Europe/Paris, where this is 18:05Z).
    fn now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 11, 16, 19, 5, 0).unwrap()
    }

    /// The fixed to-do list of `bot/intent.test.js`.
    fn store() -> Store {
        let s = Store::temp();
        let author = json!({ "id": "261531632589078528", "name": "WakandeX" });
        s.save("todo", &json!([
            { "id": 1, "text": "Mettre des fondus dans la musique", "list": "general", "author": author, "at": "2026-11-12T22:15:08.000Z", "done": false },
            { "id": 2, "text": "Une voix différente pour Claude", "list": "general", "author": author, "at": "2026-11-12T22:24:03.000Z", "done": false },
        ])).unwrap();
        s
    }

    fn ctx() -> Ctx {
        Ctx { role: Role::Super, ..testing::ctx() }
    }

    fn history() -> Vec<Hist> {
        let at = now().timestamp_millis();
        vec![
            Hist { at: at - 5 * 60_000, id: Some("Alex".into()), who: "Alex".into(), query: "coupe le micro de Kévin".into(), did: "mute Kévin".into(), ..Default::default() },
            Hist { at: at - 60_000, id: Some("Kévin".into()), who: "Kévin".into(), query: "et Marie aussi".into(), reply: "Désolé, je ne peux pas.".into(), ..Default::default() },
        ]
    }

    #[test]
    fn date_line_is_fr_full_short() {
        assert_eq!(date_line(now()), "lundi 16 novembre 2026 à 19:05");
        assert_eq!(date_line(Local.with_ymd_and_hms(2026, 3, 1, 7, 9, 0).unwrap()), "dimanche 1 mars 2026 à 07:09");
    }

    struct Fake(InferErrorKind, &'static str);
    impl Understand for Fake {
        async fn chat(&self, _: &[ChatMsg], _: &Value, _: u32) -> Result<Value, InferError> {
            Err(InferError::new(self.0, self.1))
        }
    }

    #[tokio::test]
    async fn llm_errors_are_mapped() {
        let (c, s) = (ctx(), store());
        let run = |k, m| {
            let (c, s) = (c.clone(), &s);
            async move { ask_llm(&Fake(k, m), &c, "x", "fr", &[], s, now()).await.unwrap_err() }
        };
        let say = |fr: &str, en: &str, code| BrainError::Say(Say::new(fr, en).code(code));
        assert_eq!(run(InferErrorKind::Down, "refused").await, say("Mon cerveau ne répond pas.", "My brain isn't responding.", "LLM_DOWN"));
        assert_eq!(run(InferErrorKind::Timeout, "").await, say("Mon cerveau est trop lent, la carte graphique est saturée.", "My brain is too slow, the graphics card is full.", "LLM_DOWN"));
        assert_eq!(run(InferErrorKind::Bad, "HTTP 500 Internal Server Error").await, BrainError::failed("LLM 500"));
        assert_eq!(run(InferErrorKind::Bad, "invalid JSON").await, say("Je n'ai pas compris.", "Sorry, I didn't get that.", "LLM_JSON"));
    }

    #[test]
    fn unsure_is_a_lost_order() {
        let o = |genre, faisable, actions: Value, say| json!({ "analyse": { "genre": genre, "faisable": faisable }, "actions": actions, "say": say });
        assert!(unsure(&o("ordre", true, json!([]), "")));
        assert!(!unsure(&o("ordre", true, json!([]), "Salut")));
        assert!(!unsure(&o("ordre", true, json!([{ "type": "time" }]), "")));
        assert!(!unsure(&o("ordre", false, json!([]), "")));
        assert!(!unsure(&o("discussion", true, json!([]), "")));
    }


    #[test]
    fn system_matches_node() {
        assert_eq!(system(), include_str!("../tests/fixtures/system.txt"));
    }

    #[test]
    fn schema_matches_node() {
        assert_eq!(serde_json::to_string(&schema()).unwrap(), include_str!("../tests/fixtures/schema.json"));
    }

    #[test]
    fn prompts_match_node() {
        let s = store();
        let outsider = Person { id: "visiteur".into(), name: "Visiteur".into(), ..Default::default() };
        let c = ctx();
        let cases: [(Ctx, &str, &str, Vec<Hist>, &str); 5] = [
            (c.clone(), "coupe le micro de Kévin", "fr", vec![], include_str!("../tests/fixtures/prompt-1.txt")),
            (Ctx { speaker: outsider, ..c.clone() }, "viens dans Gaming", "fr", vec![], include_str!("../tests/fixtures/prompt-2.txt")),
            (c.clone(), "refais pareil pour Marie", "fr", history(), include_str!("../tests/fixtures/prompt-3.txt")),
            (c.clone(), "mute Kevin", "en", vec![], include_str!("../tests/fixtures/prompt-4.txt")),
            (c, "écris \"c'est l'été à Noël\" dans annonces, ça va être génial", "fr", vec![], include_str!("../tests/fixtures/prompt-5.txt")),
        ];
        for (i, (ctx, q, lang, h, want)) in cases.iter().enumerate() {
            assert_eq!(prompt(ctx, q, lang, h, &s, now()), *want, "prompt-{}", i + 1);
        }
    }
}
