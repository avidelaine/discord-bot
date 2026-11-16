//! Port of `bot/brain.js` lines 203-296 and 416-483: the guards on what the LLM picked (each one fixes a miss seen
//! in the journal), the "nothing is noted unless a step saved it" guard, and what to do with an understood plan.
//! A plan is the LLM's JSON `{ analyse, actions, say, ecrit }`, kept as a `serde_json::Value` like Node's object.
use crate::resolve::{is_text, pick};
use crate::store::Store;
use crate::types::{Ctx, Step};
use crate::util::{hit, norm, re, re_i, replace1, replace_all, sim, words, BrainError, Hist};
use crate::writing::find_channel;
use bobby_proto::infer::Understand;
use chrono::{DateTime, Local};
use fancy_regex::Regex;
use serde_json::{json, Map, Value};
use std::sync::LazyLock;

static LEAVE: LazyLock<Regex> = LazyLock::new(|| re_i(r"\b(?:pars|partir|quitte|quitter|leave|go away|va-t'en|vas-t'en|casse-toi|bye|au revoir|à plus|a plus|y aller|go now|you can go|déconnecte-toi|sors|get out)\b"));
// Questions about Bobby, the server, its people or the conversation are answered locally (about), never by Claude.
static ASK_LEAD: LazyLock<Regex> = LazyLock::new(|| re_i(r"^(?:(?:est-ce que |est ce que )?(?:tu peux|tu pourrais|peux-tu|pourrais-tu)(?: me| nous)? (?:dire|expliquer|rappeler|donner)|(?:dis|explique|raconte|donne)[- ]moi|can you tell me|tell me)\s*"));
static SELF: LazyLock<Regex> = LazyLock::new(|| re_i(r"(?:^|[^\p{L}])(?:bobby|toi|tu es|t'es|es-tu|ta|ton|tes|tu (?:penses|crois|trouves|préfères|aimes)|you are|are you|your|yourself|do you think|droits?|permissions?|admins?|surnoms?|nicknames?|liste|to-?do|serveur|server|rights)(?![\p{L}])"));
static BACKREF: LazyLock<Regex> = LazyLock::new(|| re_i(r"^(?:pourquoi|comment ça|comment|qu'est-ce qui|c'était quoi|why|what happened)\b.*\b(?:il|elle|ils|elles|ça|ca|c'était|t'as|it|he|she|they)\b"));
static QUOTES: LazyLock<Regex> = LazyLock::new(|| re(r"[’`]"));

/// "moi" and its English forms: the speaker (`ME` of resolve.js).
const ME: [&str; 5] = ["moi", "me", "je", "myself", "i"];

/// Words of a sentence: `q.split(/[^\p{L}0-9]+/u)`.
fn split_words(q: &str) -> impl Iterator<Item = &str> {
    q.split(|c: char| !(c.is_alphabetic() || c.is_ascii_digit())).filter(|w| !w.is_empty())
}

pub fn about_bobby(query: &str, ctx: &Ctx, history: &[Hist]) -> bool {
    let q = replace_all(&QUOTES, &query.to_lowercase(), "'");
    let q = replace1(&ASK_LEAD, &q, "");
    if hit(&SELF, &q) || (!history.is_empty() && hit(&BACKREF, &q)) {
        return true;
    }
    let names: Vec<String> = ctx.members.iter().flat_map(|m| std::iter::once(&m.name).chain(m.aliases.iter())).map(|n| norm(n)).filter(|n| n.chars().count() >= 4).collect();
    let found = split_words(&q).any(|w| w.chars().count() >= 4 && names.iter().any(|n| sim(w, n) >= 0.85));
    found
}

// "es-tu capable de…", "quelle est ta plage de volume": a question about what Bobby can do, never an order
// ("peux-tu…" stays a polite order). Backs up the model's own "genre".
static CAPABLE: LazyLock<Regex> = LazyLock::new(|| re_i(r"^(?:(?:bobby|alors|et|ok|euh)[\s,]+)*(?:(?:est-ce que |est ce que )?(?:tu es|t'es|tu serais) capable|es-tu capable|serais-tu capable|(?:est-ce que |est ce que )?tu sais (?!faire quoi|quoi)|sais-tu |are you (?:able|capable)|is it possible for you)|\b(?:quelle?s? (?:est|sont) (?:ta|tes) (?:range|plage|limites?|capacités?)|what(?:'s| is) your (?:range|limit))"));
pub static FORM_FIX: LazyLock<Regex> = LazyLock::new(|| re_i(r"j'aurais (?:voulu|préféré|aimé)|tu (?:aurais dû|es censé|étais censé)|je voulais (?:que|plutôt)|plutôt (?:en|sous forme)|i(?:'d| would) have (?:liked|wanted)|i wanted"));
static RESTORE: LazyLock<Regex> = LazyLock::new(|| re_i(r"\b(?:rend|remet|rétabl|retabl|redonn|rallum|réentend|reentend|réactiv|dé-?mute|démute|des mute|de-?mute|unmute|undeafen|back|give|restore)|peut (?:re)?parler|peut (?:ré)?entendre"));
static POLITE_ORDER: LazyLock<Regex> = LazyLock::new(|| re_i(r"^(?:(?:bobby|alors|et|ok|euh)[\s,]+)*(?:(?:est-ce que |est ce que )?(?:tu peux|tu pourrais)|peux-tu|pourrais-tu|can you|could you)\b"));
pub static WRITE_HINT: LazyLock<Regex> = LazyLock::new(|| re_i(r"tableau|par écrit|dans le chat|format|mis(?:e)? en forme|à puces|embed|table|in writing"));
// "Supprime les messages que j'ai mis": Bobby deletes only its own messages (it did confirm, then deleted nothing).
static OTHERS_MSGS: LazyLock<Regex> = LazyLock::new(|| re_i(r"\b(?:mes|ses|leurs|nos|vos) (?:propres )?messages\b|\bmessages? (?:que |qu')(?:j'ai|il a|elle a|ils ont|on a|tu m'as)|\b(?:my|his|her|their|our) (?:own )?messages\b"));
const OTHERS_SAY: [&str; 2] = ["Je ne peux supprimer que mes propres messages, pas ceux des autres.", "I can only delete my own messages, not other people’s."];
// "Arrobase-moi dans dev-bot-test" moved the user to the voice channel dev-bot: a mention is a ping, never a move.
// Only when the destination is a text channel (or none): "déplace Lucas dans Ping Pong" stays a move.
static PING: LazyLock<Regex> = LazyLock::new(|| re_i(r"arrobas|arobas|\bping|\bmentionne|\btag(?:ue)?\b|@"));

static STOP_WORD: LazyLock<Regex> = LazyLock::new(|| re(r"\b(?:stop|arrête|arrete|tais-toi|chut)\b"));
static CLOSE_WORD: LazyLock<Regex> = LazyLock::new(|| re(r"\b(?:ferme|quitte|close|quit|kill)\b"));
static NICKNAME: LazyLock<Regex> = LazyLock::new(|| re(r"surnom|nickname"));
static RIGHTS_STEP: LazyLock<Regex> = LazyLock::new(|| re(r"^(?:admin_add|admin_remove|perm_restore|perm_revoke)$"));
static SUPER_ADMIN: LazyLock<Regex> = LazyLock::new(|| re(r"super[\s-]?admin"));

/// JS truthiness of a JSON value.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// `a.who?.length` truthy.
fn has_who(a: &Value) -> bool {
    match &a["who"] {
        Value::Array(x) => !x.is_empty(),
        Value::String(s) => !s.is_empty(),
        _ => false,
    }
}

fn lang_idx(lang: &str) -> usize {
    usize::from(lang == "en")
}

/// Fixes the plan the LLM gave (see the comments on each rule). `out` is `{ analyse, actions, say, ecrit }`.
pub fn guard(out: Value, query: &str, ctx: &Ctx, history: &[Hist], lang: &str) -> Value {
    let q = replace_all(&QUOTES, &query.to_lowercase(), "'");
    let mut out = out;
    // { ...out, analyse: { genre: 'ordre', faisable: true, ...out.analyse } }
    let mut analyse = Map::new();
    analyse.insert("genre".into(), json!("ordre"));
    analyse.insert("faisable".into(), json!(true));
    if let Some(a) = out["analyse"].as_object() {
        analyse.extend(a.clone());
    }
    out["analyse"] = Value::Object(analyse);
    let has_actions = |o: &Value| o["actions"].as_array().is_some_and(|x| !x.is_empty());
    if hit(&CAPABLE, &q) {
        out["analyse"]["genre"] = json!("capacite");
    } else if out["analyse"]["genre"] == "capacite" && has_actions(&out) && hit(&POLITE_ORDER, &q) {
        out["analyse"]["genre"] = json!("ordre"); // "peux-tu…" is an order
    }
    // "j'aurais voulu des tableaux": a correction about the form means the same request, in writing
    if out["analyse"]["genre"] == "correction" && hit(&WRITE_HINT, &q) {
        out["ecrit"] = json!(true);
    }
    // A question about Bobby the model answered by itself ("c'est quoi ton mot d'activation"): the local facts answer it
    // (only what it classed as a capability question: greetings and orders it refused keep their own answer)
    if !has_actions(&out) && out["analyse"]["genre"] == "capacite" && !hit(&CAPABLE, &q) && about_bobby(query, ctx, history) {
        out["actions"] = json!([{ "type": "about", "text": query }]);
        out["say"] = json!("");
        return out;
    }
    let actions: Vec<Value> = out["actions"].as_array().cloned().unwrap_or_default();
    // not feasible: no "nearest action" (volume 200 % does go through: music_volume says the maximum is 100)
    if !truthy(&out["analyse"]["faisable"]) && !actions.iter().any(|a| a["type"] == "music_volume") {
        let say = if truthy(&out["say"]) { out["say"].clone() } else { json!(if lang == "en" { "I can't do that." } else { "Je ne peux pas faire ça." }) };
        out["actions"] = json!([]);
        out["say"] = say;
        return out;
    }
    let mut say = out.get("say").cloned();
    let mut kept: Vec<Value> = Vec::new();
    for a in actions {
        let kind = a["type"].as_str().unwrap_or("").to_string();
        let kind = kind.as_str();
        if (kind == "clean" || kind == "msg_delete") && hit(&OTHERS_MSGS, &q) {
            say = Some(json!(OTHERS_SAY[lang_idx(lang)]));
            continue;
        }
        if (kind == "move" || kind == "come") && hit(&PING, &q) {
            // the text channel the sentence names wins over the LLM's (it picked the voice channel "dev-bot")
            let named = find_channel(query, ctx).0.map(|c| c.name).filter(|n| !n.is_empty()).or_else(|| a["channel"].as_str().and_then(|c| is_text(c, ctx)).map(|c| c.name.clone()).filter(|n| !n.is_empty()));
            if named.is_some() || !truthy(&a["channel"]) {
                let mut p = Map::new();
                p.insert("type".into(), json!("ping"));
                p.insert("who".into(), if has_who(&a) { a["who"].clone() } else { json!(["moi"]) });
                if let Some(n) = named {
                    p.insert("channel".into(), json!(n));
                }
                p.insert("text".into(), json!(""));
                kept.push(Value::Object(p));
                continue;
            }
        }
        if kind == "leave" && !hit(&LEAVE, &q) {
            if hit(&STOP_WORD, &q) {
                kept.push(json!({ "type": "music_stop", "quiet": true }));
            }
            continue;
        }
        if kind == "pc_launch" && hit(&CLOSE_WORD, &q) {
            let mut a = a;
            a["type"] = json!("pc_close");
            kept.push(a);
            continue;
        }
        // "mute-moi micro et casque" read as undeafen: giving back needs a word that gives back
        if (kind == "unmute" || kind == "undeafen") && !hit(&RESTORE, &q) {
            let mut a = a.clone();
            a["type"] = json!(&kind[2..]);
            kept.push(a);
            continue;
        }
        if kind == "alias_list" && !hit(&NICKNAME, &q) {
            kept.push(json!({ "type": "todo_list", "list": "general" }));
            continue;
        }
        if hit(&RIGHTS_STEP, kind) && hit(&SUPER_ADMIN, &q) {
            kept.push(json!({ "type": "super_admin" }));
            continue;
        }
        if kind == "ask" && about_bobby(query, ctx, history) {
            kept.push(json!({ "type": "about", "text": query }));
            continue;
        }
        // "qui est admin" read as perm_show of someone called "admin": a general question about rights
        let known = |w: &Value| {
            let w = w.as_str().unwrap_or("");
            ME.contains(&norm(w).as_str()) || pick(w, &ctx.members, "person").is_ok()
        };
        if kind == "perm_show" && !(has_who(&a) && a["who"].as_array().is_some_and(|x| x.iter().all(known))) {
            kept.push(json!({ "type": "about", "text": query }));
            continue;
        }
        // "renomme Lucas en le Boss" read as renaming the nickname "Lucas": Lucas is a person, so it gives a nickname
        if kind == "alias_rename" && !has_who(&a) && truthy(&a["to"]) && !hit(&NICKNAME, &q) {
            let named = ctx.members.iter().find(|m| split_words(&q).any(|w| w.chars().count() >= 3 && sim(w, &m.name) >= 0.85));
            if let Some(m) = named {
                kept.push(json!({ "type": "alias_add", "who": [m.name], "text": a["to"] }));
                continue;
            }
        }
        // the 4B model shortens long notes ("gérer une to-do list"): keep the whole sentence
        if kind == "todo_add" && words(query) >= 12 && (words(a["text"].as_str().unwrap_or("")) as f64) < 0.6 * words(query) as f64 {
            let mut a = a;
            a["text"] = json!(note_text(query));
            kept.push(a);
            continue;
        }
        kept.push(a);
    }
    out["actions"] = Value::Array(kept);
    if let Some(s) = say {
        out["say"] = s;
    }
    out
}

/// The text of the note a to-do module finds in the sentence, else the whole sentence.
pub fn note_text(query: &str) -> String {
    crate::actions::modules()
        .iter()
        .filter_map(|m| m.early.and_then(|f| f(query)))
        .find(|x| x.first().is_some_and(|s| s["type"] == "todo_add"))
        .and_then(|x| x[0]["text"].as_str().filter(|t| !t.is_empty()).map(String::from))
        .unwrap_or_else(|| query.to_string())
}

// ---------- Nothing is "noted" unless a step saved it ----------

static CLAIM: LazyLock<Regex> = LazyLock::new(|| re_i(r"(?:^|[^\p{L}])(?:not[ée]e?s?|retenu|enregistr[ée]e?s?|ajout[ée]e?s?|je (?:le |l'|la )?(?:note|retiens|garde en (?:t[eê]te|mémoire))|bien reçu|c'est dans la liste|noted|saved|i'?ll remember|i(?:'ve| have) (?:added|written))(?![\p{L}])"));
static OFFER: LazyLock<Regex> = LazyLock::new(|| re_i(r"(?:tu veux|veux-tu|vous voulez|voulez-vous) que je (?:l'|le |la )?(?:ajoute|note|rajoute)|want me to (?:add|note|write)"));
const OFFER_LINE: [&str; 2] = ["Tu veux que je l'ajoute à la liste ?", "Do you want me to add it to the list?"];

/// A reply with no saving step that claims to have noted something becomes the offer; an offer waits for a "oui".
/// Only free text is checked (the LLM's own reply, or an "about" answer): Claude's answers, list readouts or
/// song titles may legitimately contain "ajouté" or "saved". Returns (text, offer).
pub fn note_guard(text: &str, plan: &[Step], lang: &str) -> (String, bool) {
    if plan.iter().any(|s| s.kind != "about") {
        return (text.to_string(), false);
    }
    if hit(&OFFER, text) {
        return (text.to_string(), true);
    }
    if hit(&CLAIM, text) {
        return (OFFER_LINE[lang_idx(lang)].to_string(), true);
    }
    (text.to_string(), false)
}

/// Read-only answers: asked as a capability question ("es-tu capable de me dire l'heure ?"), they are just done.
const READONLY: [&str; 12] = ["time", "help", "about", "ask", "todo_list", "music_now", "music_queue", "perm_show", "rights_list", "alias_list", "pc_music", "pc_gpu"];

/// What to do with an understood plan: "run" it, or "offer" it ("Tu veux que je le fasse ?", runs on a "oui").
pub fn decide(out: Option<&Value>, plan: &[Step]) -> &'static str {
    let capacity = out.is_some_and(|o| o["analyse"]["genre"] == "capacite");
    if plan.iter().any(|s| s.flag("offer")) || (capacity && !plan.iter().all(|s| READONLY.contains(&s.kind.as_str()))) {
        "offer"
    } else {
        "run"
    }
}

/// `understand`: the LLM's plan for the sentence, through the guards.
pub async fn understand<L: Understand>(llm: &L, ctx: &Ctx, query: &str, lang: &str, history: &[Hist], store: &Store, now: DateTime<Local>) -> Result<Value, BrainError> {
    let out = crate::prompt::ask_llm(llm, ctx, query, lang, history, store, now).await?;
    Ok(guard(out, query, ctx, history, lang))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use crate::types::{Chan, Role};
    use chrono::TimeZone;

    fn sup() -> Ctx {
        Ctx { role: Role::Super, ..testing::ctx() }
    }

    /// `out(actions, analyse, say)` of the JS test.
    fn out(actions: Value, analyse: Value, say: &str) -> Value {
        let mut a = json!({ "genre": "ordre", "faisable": true, "sur": true });
        a.as_object_mut().unwrap().extend(analyse.as_object().unwrap().clone());
        json!({ "analyse": a, "actions": actions, "say": say })
    }

    fn g(o: Value, q: &str) -> Value {
        guard(o, q, &sup(), &[], "fr")
    }

    /// What `resolve(actions)` gives `decide` and `note_guard`: only the action types and the "offer" flag matter.
    fn plan(kinds: &[&str]) -> Vec<Step> {
        kinds.iter().map(|k| Step { kind: k.to_string(), ..Default::default() }).collect()
    }

    fn types(r: &Value) -> Vec<String> {
        r["actions"].as_array().unwrap().iter().map(|a| a["type"].as_str().unwrap().to_string()).collect()
    }

    // batch4 "reasoning guards" (unit.test.js 336-392); the resolve/run/report parts are R3b, quick() is in writing.rs
    #[test]
    fn reasoning_guards() {
        let mut r = g(out(json!([{ "type": "clean", "channel": "memes" }]), json!({}), ""), "es-tu capable de supprimer mes messages à moi ?");
        assert_eq!((r["actions"].clone(), r["say"].clone()), (json!([]), json!("Je ne peux supprimer que mes propres messages, pas ceux des autres.")));
        r = g(out(json!([{ "type": "clean", "channel": "memes" }]), json!({}), ""), "est-ce que tu peux supprimer aussi tous les messages que j'ai mis dans memes ?");
        assert_eq!(r["actions"], json!([])); // it asked to confirm, then deleted nothing
        r = g(out(json!([{ "type": "clean", "channel": "memes" }]), json!({}), ""), "est-ce que tu peux clean le channel memes de tous les messages que tu envoyais ?");
        assert_eq!(r["actions"][0]["type"], "clean"); // its own messages: fine
        r = g(out(json!([{ "type": "music_volume", "text": "100" }]), json!({}), ""), "quelle est ta range de volume sonore que tu peux atteindre ?");
        assert_eq!(r["analyse"]["genre"], "capacite");
        assert_eq!(decide(Some(&r), &plan(&["music_volume"])), "offer"); // set the volume to 100 before
        r = g(out(json!([{ "type": "help" }]), json!({}), ""), "est-ce que tu es capable de m'expliquer ce que tu sais faire ?");
        assert_eq!(decide(Some(&r), &plan(&["help"])), "run"); // a read-only answer is just given
        r = g(out(json!([{ "type": "mute", "who": ["Kévin"] }]), json!({}), ""), "peux-tu mute Kévin ?");
        assert_eq!(decide(Some(&r), &plan(&["mute"])), "run"); // "peux-tu" stays a polite order
        // Not feasible: no nearest action; "volume à 200 %" goes through and says the maximum is 100
        r = g(out(json!([{ "type": "clean" }]), json!({ "faisable": false, "pourquoi": "x" }), "Je ne peux pas."), "fais un truc impossible");
        assert_eq!((r["actions"].clone(), r["say"].clone()), (json!([]), json!("Je ne peux pas.")));
        r = g(out(json!([{ "type": "music_volume", "text": "200" }]), json!({ "faisable": false }), ""), "augmente le son de la musique à 200 %");
        assert_eq!(r["actions"].as_array().unwrap().len(), 1);
        // "arrobase-moi dans dev-bot-test" moved the user to the voice channel dev-bot: a mention, never a move
        r = g(out(json!([{ "type": "move", "who": ["moi"], "channel": "dev-bot-test" }]), json!({}), ""), "est-ce que tu peux me arrobas dans devbot test pour me demander de lancer Apex ?");
        assert_eq!(types(&r), ["ping"]);
        let mut devbot = sup();
        devbot.voice_channels.push(Chan { id: "dev-bot".into(), name: "dev-bot".into() });
        // "appelle-moi sur Discord": an offer of a mention (quick() sets offer "call"; the flag is set by hand here)
        let mut call = plan(&["ping"]);
        call[0].set("offer", "call");
        assert_eq!(decide(None, &call), "offer");
        // After review: "Ping Pong" stays a move; the text channel named in the sentence beats the LLM's voice channel
        let mut pp = devbot.clone();
        pp.voice_channels.push(Chan { id: "Ping Pong".into(), name: "Ping Pong".into() });
        let moved = guard(out(json!([{ "type": "move", "who": ["Lucas"], "channel": "Ping Pong" }]), json!({}), ""), "déplace Lucas dans Ping Pong", &pp, &[], "fr");
        assert_eq!(moved["actions"][0]["type"], "move");
        r = guard(out(json!([{ "type": "move", "who": ["moi"], "channel": "dev-bot" }]), json!({}), ""), "est-ce que tu peux me arrobas dans devbot test pour me demander de lancer Apex ?", &devbot, &[], "fr");
        assert_eq!((r["actions"][0]["type"].clone(), r["actions"][0]["channel"].clone()), (json!("ping"), json!("dev-bot-test")));
        assert_ne!(g(out(json!([{ "type": "music_play", "text": "métal" }]), json!({}), ""), "tu sais quoi, mets du métal")["analyse"]["genre"], "capacite");
        // "mute-moi micro et casque" read as undeafen; "j'aurais voulu des tableaux" redoes the last request in writing
        let both = g(out(json!([{ "type": "mute", "who": ["moi"] }, { "type": "undeafen", "who": ["moi"] }]), json!({}), ""), "est-ce que tu peux me mute micro et casque s'il te plaît ?");
        assert_eq!(types(&both), ["mute", "deafen"]);
        assert_eq!(g(out(json!([{ "type": "undeafen", "who": ["Marie"] }]), json!({}), ""), "Marie peut réentendre")["actions"][0]["type"], "undeafen");
    }

    #[test]
    fn note_guards() {
        // Never a fake "noté": a reply claiming it saved something, with no saving step, becomes the offer
        let nothing: Vec<Step> = vec![];
        assert_eq!(note_guard("J'ai bien noté : on va vouloir mettre des fondus.", &nothing, "fr"), ("Tu veux que je l'ajoute à la liste ?".to_string(), true));
        assert_eq!(note_guard("Noted, I will remember that.", &nothing, "en"), ("Do you want me to add it to the list?".to_string(), true));
        assert_eq!(note_guard("Merci ! Tu veux que je l'ajoute à la liste ?", &nothing, "fr"), ("Merci ! Tu veux que je l'ajoute à la liste ?".to_string(), true));
        assert_eq!(note_guard("Il est 23 h.", &nothing, "fr"), ("Il est 23 h.".to_string(), false));
        assert_eq!(note_guard("C'est noté dans la liste générale.", &plan(&["todo_add"]), "fr"), ("C'est noté dans la liste générale.".to_string(), false));
        // review fixes: a step that is not "about" (an "ask" answer here) is never rewritten
        assert!(!note_guard("Les ingrédients sont ajoutés un par un.", &plan(&["ask"]), "fr").1);
    }

    #[test]
    fn llm_guards() {
        let ctx = testing::ctx();
        let gg = |actions: Value, q: &str, h: &[Hist]| guard(json!({ "actions": actions, "say": "" }), q, &ctx, h, "fr")["actions"].clone();
        assert_eq!(gg(json!([{ "type": "leave" }]), "BOBBY STOP", &[]), json!([{ "type": "music_stop", "quiet": true }]));
        assert_eq!(gg(json!([{ "type": "leave" }]), "quitte le salon", &[]), json!([{ "type": "leave" }]));
        assert_eq!(gg(json!([{ "type": "pc_launch", "text": "Apex Legends" }]), "ferme Apex Legends", &[]), json!([{ "type": "pc_close", "text": "Apex Legends" }]));
        assert_eq!(gg(json!([{ "type": "alias_list" }]), "montre-moi la liste de tout ce que tu as retenu.", &[]), json!([{ "type": "todo_list", "list": "general" }]));
        assert_eq!(gg(json!([{ "type": "admin_add", "who": ["Marie"] }]), "passe Marie super admin", &[]), json!([{ "type": "super_admin" }]));
        for q in ["est-ce que tu penses que tu es meilleur que Iris ?", "est-ce que tu peux expliquer à Ilias en une phrase, c'est quoi les permissions qu'il y a sur toi ?", "je trouve que ta détection n'est pas encore parfaite sur ton mode d'activation."] {
            assert_eq!(gg(json!([{ "type": "ask", "text": q }]), q, &[])[0]["type"], "about", "{q}");
        }
        let h = [Hist { query: "lance Apex".into(), reply: "Je lance Apex Legends.".into(), ..Default::default() }];
        assert_eq!(gg(json!([{ "type": "ask", "text": "x" }]), "Pourquoi il était perdu ?", &h)[0]["type"], "about");
        for q in ["c'est quoi la capitale de l'Australie", "est-ce que tu peux me dire la population de la République démocratique du Congo ?", "raconte-moi une blague", "pourquoi le ciel est bleu"] {
            assert!(!about_bobby(q, &ctx, &[]), "world question stays with Claude: \"{q}\"");
        }
        let long = "est-ce que tu peux noter dans la liste des choses à faire ? Il faut que tu aies absolument en connaissance, un beau contexte, que ce soit sur toi";
        let t = gg(json!([{ "type": "todo_add", "text": "gérer une to-do list" }]), long, &[])[0]["text"].as_str().unwrap().to_string();
        assert!(t.starts_with("Il faut que tu aies absolument en connaissance"), "{t}");
    }

    #[test]
    fn decide_offers() {
        let o = |genre: &str| json!({ "analyse": { "genre": genre } });
        assert_eq!(decide(Some(&o("capacite")), &plan(&["mute"])), "offer");
        assert_eq!(decide(Some(&o("capacite")), &plan(&["time", "help"])), "run");
        assert_eq!(decide(Some(&o("ordre")), &plan(&["mute"])), "run");
        assert_eq!(decide(None, &plan(&["mute"])), "run");
    }

    struct Fake;
    impl Understand for Fake {
        async fn chat(&self, _: &[bobby_proto::infer::ChatMsg], _: &Value, _: u32) -> Result<Value, bobby_proto::infer::InferError> {
            Ok(json!({ "actions": [{ "type": "leave" }], "say": "" }))
        }
    }

    // `understand` = the LLM call, then `guard`
    #[tokio::test]
    async fn understand_guards_the_answer() {
        let now = Local.with_ymd_and_hms(2026, 11, 16, 19, 5, 0).unwrap();
        let r = understand(&Fake, &sup(), "mets du métal", "fr", &[], &Store::temp(), now).await.unwrap();
        assert_eq!(r["actions"], json!([]));
        assert_eq!(r["analyse"]["genre"], "ordre");
    }
}
