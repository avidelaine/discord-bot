//! Port of `bot/actions/todo.js`: to-do lists in the store document "todo": one shared general list plus one
//! personal list per person. Item: { id, text, list: 'general' | memberId, author: { id, name }, at (ISO), done }.
//! Rights: everyone with a role adds to the general list and to their own, and manages their own items
//! (written by them, or on their list); the super admin manages everything.
//! Here: texts, shortcuts, "note que ..." detection, prompt lines, list resolution and the `say` lines.
//! `run` (edit/touch, findItem, cards) is batch R3b.
use crate::resolve::pick;
use crate::store::Store;
use crate::types::{ActionDef, Ctx, Module, Person, QuickFn, Role, SayFn, Step};
use crate::util::{caps, group, hit, re, re_i, replace1, Say};
use fancy_regex::Regex;
use serde_json::{json, Value};
use std::sync::LazyLock;

pub const GENERAL: &str = "general";

/// Old format (plain strings) is read as unsigned items of the general list.
pub fn items(store: &Store) -> Vec<Value> {
    let Value::Array(all) = store.load_or("todo", json!([])) else { return vec![] };
    all.into_iter()
        .enumerate()
        .map(|(i, x)| match x {
            Value::String(text) => json!({ "id": i + 1, "text": text, "list": GENERAL, "author": { "id": null, "name": "?" }, "at": null, "done": false }),
            x => x,
        })
        .collect()
}

static GENERAL_RE: LazyLock<Regex> = LazyLock::new(|| re_i(r"^(?:general|générale?|generale?|commune?|partag[ée]e|shared|la liste|the list|todo|to-?do(?: list)?)$"));
static MINE_RE: LazyLock<Regex> = LazyLock::new(|| re_i(r"^(?:moi|me|mine|my|ma liste|my list|perso|personnelle?|personal|la mienne)$"));
static LIST_PREFIX: LazyLock<Regex> = LazyLock::new(|| re_i(r"^(?:la liste (?:de |d')|ma liste|la liste|list of |the list of )"));

fn is_general(raw: &str) -> bool {
    raw.is_empty() || hit(&GENERAL_RE, raw.trim())
}
fn is_mine(raw: &str) -> bool {
    hit(&MINE_RE, raw.trim())
}

/// Spoken list name -> { id, name }. People come from voice and from authors already in the lists.
/// Deviation from the plan: takes the `Store` too (Node read `items()` from the file).
pub fn list_of(raw: &str, ctx: &Ctx, store: &Store) -> Result<Value, Say> {
    if is_general(raw) {
        return Ok(json!({ "id": GENERAL, "name": null }));
    }
    if is_mine(raw) {
        return Ok(json!({ "id": ctx.speaker.id, "name": ctx.speaker.name }));
    }
    let mut people: Vec<Person> = ctx.members.clone();
    for it in items(store) {
        let id = it["author"]["id"].as_str().unwrap_or("");
        if !id.is_empty() && !people.iter().any(|p| p.id == id) {
            people.push(Person { id: id.to_string(), name: it["author"]["name"].as_str().unwrap_or("").to_string(), ..Default::default() });
        }
    }
    let q = replace1(&LIST_PREFIX, raw, "");
    let p = pick(&q, &people, "person")?;
    Ok(json!({ "id": p.id, "name": p.name }))
}

// ---------- say helpers ----------

/// `s.list || { id: GENERAL }` as (id, name).
fn step_list(s: &Step) -> (String, Option<String>) {
    list_parts(s.list.as_ref())
}

fn list_parts(l: Option<&Value>) -> (String, Option<String>) {
    match l {
        Some(v) if !v.is_null() => (v["id"].as_str().unwrap_or("").to_string(), v["name"].as_str().map(String::from)),
        _ => (GENERAL.to_string(), None),
    }
}

fn list_name(l: (String, Option<String>), s: &Step, lang: &str) -> String {
    let (id, name) = l;
    if id == GENERAL {
        return if lang == "en" { "the general list".into() } else { "la liste générale".into() };
    }
    if id == s.by.id {
        return if lang == "en" { "your list".into() } else { "ta liste".into() };
    }
    let name = name.unwrap_or_else(|| "null".into());
    if lang == "en" { format!("{name}'s list") } else { format!("la liste de {name}") }
}

static SPACES: LazyLock<Regex> = LazyLock::new(|| re(r"\s+"));
static SENTENCES: LazyLock<Regex> = LazyLock::new(|| re(r"(?<=[.?!…])\s+"));

/// JS `str.split(regex)` for a regex that never matches the empty string.
fn split<'a>(r: &Regex, s: &'a str) -> Vec<&'a str> {
    let mut out = vec![];
    let mut last = 0;
    for m in r.find_iter(s).flatten() {
        out.push(&s[last..m.start()]);
        last = m.end();
    }
    out.push(&s[last..]);
    out
}

/// Spoken items are cut to 10 words, the full text is posted in the text channel.
fn short(t: &str) -> String {
    let w = split(&SPACES, t);
    if w.len() > 10 { format!("{}…", w[..10].join(" ")) } else { t.to_string() }
}

fn text_of(it: &Value) -> &str {
    it["text"].as_str().unwrap_or("")
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

fn plural(n: usize) -> &'static str {
    if n > 1 { "s" } else { "" }
}

fn list_of_step(s: &Step, k: &str) -> Vec<Value> {
    s.f.get(k).and_then(Value::as_array).cloned().unwrap_or_default()
}

fn say_add(s: &Step, _: Option<&str>, lang: &str) -> Option<String> {
    let n = list_name(step_list(s), s, lang);
    Some(if lang == "en" { format!("Added to {n}.") } else { format!("C'est noté dans {n}.") })
}

fn say_list(s: &Step, _: Option<&str>, lang: &str) -> Option<String> {
    let en = lang == "en";
    let name = list_name(step_list(s), s, lang);
    let author = s.str("author").filter(|a| !a.is_empty());
    let join = |v: &[Value], sep: &str| v.iter().take(3).map(|it| short(text_of(it))).collect::<Vec<_>>().join(sep);
    if s.flag("shown") {
        let shown = list_of_step(s, "shown");
        let n = shown.len();
        let filter = s.str("filter");
        if en {
            let what = format!("{}item{}", if filter == Some("done") { "done " } else if filter == Some("open") { "open " } else { "" }, plural(n));
            let from = author.map(|a| format!(" from {a}")).unwrap_or_default();
            if n == 0 {
                return Some(format!("No {} in {name}{from}.", what.strip_suffix('s').unwrap_or(&what)));
            }
            return Some(format!("In {name}, {n} {what}{from}: {}{}.", join(&shown, "; "), if n > 3 { "; and more" } else { "" }));
        }
        let what = match filter {
            Some("done") => format!("point{} fait{}", plural(n), plural(n)),
            Some("open") => format!("point{} à faire", plural(n)),
            _ => format!("point{}", plural(n)),
        };
        let of = author.map(|a| format!(" de {a}")).unwrap_or_default();
        if n == 0 {
            // the plural "s" of a word's end is never there when n = 0
            return Some(format!("Aucun {what} dans {name}{of}."));
        }
        return Some(format!("Dans {name}, {n} {what}{of} : {}{}.", join(&shown, " ; "), if n > 3 { " ; et la suite" } else { "" }));
    }
    let open = list_of_step(s, "open");
    let n = open.len();
    let name = cap(&name);
    let at = s.str("where").filter(|w| !w.is_empty());
    if en {
        if n == 0 {
            return Some(format!("{name} is empty."));
        }
        return Some(format!("{name}: {n} item{}. {}{}.{}", plural(n), join(&open, "; "), if n > 3 { "; and more" } else { "" }, at.map(|w| format!(" Everything is in {w}.")).unwrap_or_default()));
    }
    if n == 0 {
        return Some(format!("{name} est vide."));
    }
    Some(format!("{name} : {n} point{}. {}{}.{}", plural(n), join(&open, " ; "), if n > 3 { " ; et la suite" } else { "" }, at.map(|w| format!(" Tout est dans {w}.")).unwrap_or_default()))
}

fn missing(s: &Step, lang: &str) -> String {
    let t = &s.text;
    if lang == "en" { format!("I can't find \"{t}\" in the list.") } else { format!("Je ne trouve pas « {t} » dans la liste.") }
}

fn say_done(s: &Step, _: Option<&str>, lang: &str) -> Option<String> {
    let Some(it) = s.f.get("item").filter(|_| s.flag("item")) else { return Some(missing(s, lang)) };
    Some(if lang == "en" { format!("Done: {}", short(text_of(it))) } else { format!("Fait : {}", short(text_of(it))) })
}

fn say_remove(s: &Step, _: Option<&str>, lang: &str) -> Option<String> {
    let Some(it) = s.f.get("item").filter(|_| s.flag("item")) else { return Some(missing(s, lang)) };
    Some(if lang == "en" { format!("Deleted: {}", short(text_of(it))) } else { format!("Je supprime : {}", short(text_of(it))) })
}

fn say_move(s: &Step, _: Option<&str>, lang: &str) -> Option<String> {
    let Some(it) = s.f.get("item").filter(|_| s.flag("item")) else { return Some(missing(s, lang)) };
    let to = list_name(list_parts(s.to.as_ref()), s, lang);
    Some(if lang == "en" { format!("Moved \"{}\" to {to}.", short(text_of(it))) } else { format!("Je déplace « {} » dans {to}.", short(text_of(it))) })
}

// ---------- "Note ça" requests, recognized before the LLM (and before the "several requests" guard) ----------

// longest forms first: the regex engine takes the first alternative that fits ("note" would leave the r of "noter")
const VERB: &str = "(?:noter|notes|note|not[ée]|rajouter|rajoutes|rajoute|ajouter|ajoutes|ajoute|inscrire|inscris|retenir|retiens|write down|note down|add|put)";
const LIST: &str = "(?:la|ta|ma|une|the|my|your) (?:liste|to-?do(?: list)?|list)(?: des choses à faire| de tâches| de choses à faire)?";
// \b is ASCII-only in JS: (?<!\p{L}) and (?!\p{L}) also work next to « à » and « noté ».
const NL: &str = r"(?<!\p{L})";
const NA: &str = r"(?!\p{L})";
const POLITE: &str = r"(?:(?:bobby|alors|ok|bon|euh)[\s,]+)*(?:(?:est-ce que |est ce que )?(?:tu peux|tu pourrais|peux-tu|peux tu|pourrais-tu|pourrais tu)\s+|vas-y\s+|can you\s+|could you\s+|please\s+)?";

static ON_LIST: LazyLock<Regex> = LazyLock::new(|| re_i(&format!("{NL}{VERB}{NA}[^.?!]{{0,60}}?{NL}(?:dans|à|a|sur|to|on|in) {LIST}")));
static START: LazyLock<Regex> = LazyLock::new(|| re_i(r"^(?:retiens|note(?:s| bien)?|noter|pour la prochaine fois|il faudrait que tu|il faut que tu retiennes|remember|make a note|note that|for next time)\b"));
static POLITE_RE: LazyLock<Regex> = LazyLock::new(|| re_i(&format!("^{POLITE}")));
static LEAD: LazyLock<Regex> = LazyLock::new(|| {
    re_i(&format!("^{POLITE}(?:me |le |l'|bien )*{VERB}{NA}(?: bien)?(?: (?:ça|ca|this|that))?(?: (?:dans|à|a|sur|to|on|in) {LIST})?(?: que | qu'|, | : |: | that |\\s)?"))
});
static TAIL: LazyLock<Regex> = LazyLock::new(|| re_i(&format!(r"\s*(?:dans|à|a|sur|to|on|in) {LIST}\s*(?:s'il te pla[iî]t|stp|please)?\s*[.?!…]*$")));
static NOT_STRIPPED: LazyLock<Regex> = LazyLock::new(|| re_i(r"^(?:il faudrait|il faut|pour la prochaine)"));
static QUESTION_END: LazyLock<Regex> = LazyLock::new(|| re(r"\s*\?$"));
static JUNK_START: LazyLock<Regex> = LazyLock::new(|| re(r"^[\s,:;-]+"));
static JUNK_END: LazyLock<Regex> = LazyLock::new(|| re(r"\s*[,;:]+$"));
static WHO: LazyLock<Regex> = LazyLock::new(|| re_i(r"\bla liste (?:de |d')([^\s,.?!]+)"));
static MY_LIST: LazyLock<Regex> = LazyLock::new(|| re_i(r"\b(?:ma liste|ma to-?do|ma propre liste|my list|my to-?do)\b"));
static NOT_A_NAME: LazyLock<Regex> = LazyLock::new(|| re_i(r"^(?:choses|tâches|courses)$"));
static LETTER: LazyLock<Regex> = LazyLock::new(|| re(r"\p{L}"));

fn n_words(t: &str) -> usize {
    split(&SPACES, t).into_iter().filter(|w| hit(&LETTER, w)).count()
}

fn is_request(x: &str) -> bool {
    (hit(&ON_LIST, x) || hit(&START, &replace1(&POLITE_RE, x, ""))) && n_words(x) <= 15
}

/// -> [todo_add] with the full text of the note, or None.
fn note(query: &str) -> Option<Vec<Value>> {
    let raw = query.trim();
    if !hit(&ON_LIST, raw) && !hit(&START, &replace1(&POLITE_RE, raw, "")) {
        return None;
    }
    let sentences = split(&SENTENCES, raw);
    let mut text = raw.to_string();
    // "J'aimerais une voix différente pour Claude. Tu peux le noter dans la liste ?": drop the request sentence.
    if sentences.len() > 1 {
        let rest: Vec<&str> = sentences.iter().copied().filter(|x| !is_request(x)).collect();
        if !rest.is_empty() && rest.len() < sentences.len() {
            text = rest.join(" ");
        }
    }
    // "ajoute acheter du pain à ma liste", "note dans la liste que ...": keep only what is to be noted.
    // "il faudrait que tu gardes l'historique ..." is a note as a whole: kept untouched.
    if hit(&LEAD, &text) && !hit(&NOT_STRIPPED, &text) {
        let stripped = replace1(&LEAD, &text, "");
        let stripped = replace1(&TAIL, &stripped, "");
        let stripped = replace1(&QUESTION_END, stripped.trim(), ""); // the request was the question
        if n_words(&stripped) >= 2 {
            text = stripped;
        }
    }
    let text = replace1(&JUNK_START, &text, "");
    let text = replace1(&JUNK_END, &text, "");
    let text = text.trim();
    if n_words(text) < 2 {
        return None;
    }
    let mut c = text.chars();
    let text: String = c.next().unwrap().to_uppercase().chain(c).collect();
    let who = caps(&WHO, raw).map(|c| group(&c, 1).to_string());
    let list = if hit(&MY_LIST, raw) {
        "moi".to_string()
    } else {
        match who {
            Some(w) if !w.is_empty() && !hit(&NOT_A_NAME, &w) => w,
            _ => GENERAL.to_string(),
        }
    };
    Some(vec![json!({ "type": "todo_add", "text": text, "list": list })])
}

/// Lines for the LLM prompt, so "le dernier point de la liste" makes sense.
fn prompt_lines(ctx: &Ctx, store: &Store) -> Vec<String> {
    let all: Vec<Value> = items(store).into_iter().filter(|it| !truthy(&it["done"])).collect();
    let fmt = |l: Vec<&Value>| {
        let s = l.iter().enumerate().map(|(i, it)| format!("{}. {}", i + 1, slice80(text_of(it)))).collect::<Vec<_>>().join(" ; ");
        if s.is_empty() { "vide".to_string() } else { s }
    };
    fn last(l: Vec<&Value>, n: usize) -> Vec<&Value> {
        l[l.len().saturating_sub(n)..].to_vec()
    }
    let general: Vec<&Value> = all.iter().filter(|it| it["list"] == GENERAL).collect();
    let mine: Vec<&Value> = all.iter().filter(|it| it["list"].as_str() == Some(ctx.speaker.id.as_str())).collect();
    vec![format!("Liste générale : {}.", fmt(last(general, 8))), format!("Liste perso de {} : {}.", ctx.speaker.name, fmt(last(mine, 5)))]
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// `text.slice(0, 80)`: JS counts UTF-16 units.
fn slice80(t: &str) -> String {
    let u: Vec<u16> = t.encode_utf16().take(80).collect();
    String::from_utf16_lossy(&u)
}

fn q(r: Regex, f: QuickFn) -> (Regex, QuickFn) {
    (r, f)
}

pub fn module() -> Module {
    let prime = |text: bool, to: bool, say: SayFn| ActionDef { role: Role::Prime, text, list: true, to, say: Some(say), ..Default::default() };
    Module {
        name: "todo",
        help: [
            "tenir la liste générale et la liste perso de chacun (ajouter, lire, cocher, supprimer, déplacer)",
            "keep the general list and everyone's personal list (add, read, tick, delete, move)",
        ],
        short: ["les listes de tâches", "the to-do lists"],
        prompt: vec![
            r#"- todo_add : ajouter "text" (le texte complet, jamais résumé) à une liste"#,
            r#"- todo_list : lire une liste (le détail est posté dans le salon texte) ; "filter" = done (seulement ce qui est fait) ou open (ce qui reste à faire), "author" = seulement les points écrits par cette personne"#,
            r#"- todo_done / todo_remove : cocher comme fait, ou supprimer, le point "text" (son numéro, "dernier", ou ses mots)"#,
            r#"- todo_move : déplacer le point "text" vers la liste "to""#,
            r#"- Champ "list" des todo_* : "general" (liste commune, par défaut, aussi "ta liste" ou "la liste" quand on parle de la liste de Bobby), "moi" (ma liste, la liste perso de celui qui parle) ou le nom d'une personne ("la liste d'Ilias")"#,
        ],
        rules: vec![
            r#"- "retiens", "note", "pour la prochaine fois", "il faudrait que tu…", "rajoute dans la liste" : todo_add avec tout le texte. Ne dis jamais que tu as noté ou retenu quelque chose sans action todo_add."#,
            r#"- "ce que tu as retenu", "ta liste", "la to-do list" : todo_list. "le dernier point de la liste" : "text" = "dernier"."#,
            r#"- Demander des droits ("donne-moi tous les droits") ne donne jamais de droits ; si on demande de le noter, c'est un todo_add."#,
        ],
        examples: vec![
            r#""ajoute acheter du pain à ma liste" -> {"actions":[{"type":"todo_add","text":"acheter du pain","list":"moi"}],"say":""}"#,
            r#""montre-moi ce que tu as retenu" -> {"actions":[{"type":"todo_list","list":"general"}],"say":""}"#,
            r#""qu'est-ce qu'il y a sur la liste d'Ilias" -> {"actions":[{"type":"todo_list","list":"Ilias"}],"say":""}"#,
            r#""quels points de la liste ont été ajoutés par Marie" -> {"actions":[{"type":"todo_list","list":"general","author":"Marie"}],"say":""}"#,
            r#""le point 2 est fait" -> {"actions":[{"type":"todo_done","text":"2"}],"say":""}"#,
            r#""retire les courses de ma liste" -> {"actions":[{"type":"todo_remove","text":"les courses","list":"moi"}],"say":""}"#,
            r#""mets le dernier point dans ma liste" -> {"actions":[{"type":"todo_move","text":"dernier","to":"moi"}],"say":""}"#,
        ],
        fields: vec![("list", json!({ "type": "string" })), ("filter", json!({ "enum": ["done", "open"] })), ("author", json!({ "type": "string" }))],
        early: Some(note),
        quick: vec![
            // "est-ce que tu peux me dire ce qui a déjà été fait dans cette liste ?" read the whole list (2026-11-12)
            q(re(r"(?:qu'est-ce qui|ce qui|quels? (?:points?|trucs?|choses?)) (?:a|ont|est|sont) (?:déjà |deja )?(?:été )?(?:fait|coché|terminé|fini)e?s?(?: dans (?:cette|la|ta) (?:liste|to-?do(?: list)?))?$"), |_| vec![json!({ "type": "todo_list", "list": GENERAL, "filter": "done" })]),
            q(re(r"(?:qu'est-ce qui|ce qui|qu'est-ce qu'il) (?:me |nous |te )?reste (?:encore )?à faire(?: dans (?:cette|la|ta) (?:liste|to-?do(?: list)?))?$"), |_| vec![json!({ "type": "todo_list", "list": GENERAL, "filter": "open" })]),
            q(re(r"^(?:montre|montrer|affiche|afficher|lis|lire|donne|donner|dis|dire|liste)(?:-moi| moi)? (?:la liste de )?(?:tout )?ce que tu as (?:retenu|noté)$|^(?:montre|montrer|affiche|afficher|lis|lire|donne|donner)(?:-moi| moi)? (?:ta liste|la liste|la to-?do(?: list)?|ta to-?do(?: list)?)$|^qu'est-ce que tu as (?:retenu|noté)$|^(?:show|read|give)(?: me)? (?:the|your) (?:list|to-?do(?: list)?)$|^what did you (?:note|remember)$"), |_| vec![json!({ "type": "todo_list", "list": GENERAL })]),
            q(re(r"^(?:montre|montrer|affiche|afficher|lis|lire|donne|donner)(?:-moi| moi)? ma (?:liste|to-?do(?: list)?)$|^(?:show|read)(?: me)? my (?:list|to-?do(?: list)?)$"), |_| vec![json!({ "type": "todo_list", "list": "moi" })]),
        ],
        prompt_lines: Some(prompt_lines),
        actions: vec![
            // saves: the only action that may say "noted" (see brain.js)
            ("todo_add", ActionDef { saves: true, ..prime(true, false, say_add) }),
            ("todo_list", ActionDef { role: Role::Prime, list: true, say: Some(say_list), ..Default::default() }),
            ("todo_done", prime(true, false, say_done)),
            ("todo_remove", prime(true, false, say_remove)),
            ("todo_move", prime(true, true, say_move)),
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
    use crate::testing::ctx;

    fn two_items() -> Store {
        let store = Store::temp();
        let a = json!({ "id": "261531632589078528", "name": "WakandeX" });
        store
            .save(
                "todo",
                &json!([
                    { "id": 1, "text": "Mettre des fondus dans la musique", "list": "general", "author": a, "at": "2026-11-12T22:15:08.000Z", "done": false },
                    { "id": 2, "text": "Une voix différente pour Claude", "list": "general", "author": a, "at": "2026-11-12T22:24:03.000Z", "done": false },
                ]),
            )
            .unwrap();
        store
    }

    #[test]
    fn module_builds() {
        let m = module();
        assert_eq!(m.actions.iter().map(|(k, _)| *k).collect::<Vec<_>>(), ["todo_add", "todo_list", "todo_done", "todo_remove", "todo_move"]);
        assert_eq!(m.quick.len(), 4);
        assert!(hit(&m.quick[0].0, "ce qui a déjà été fait dans cette liste"));
        assert_eq!((m.quick[3].1)(&caps(&m.quick[3].0, "montre-moi ma liste").unwrap()), vec![json!({ "type": "todo_list", "list": "moi" })]);
    }

    // Expected values come from `node -e` on bot/actions/todo.js.
    #[test]
    fn early_notes() {
        let n = |s: &str| note(s).unwrap();
        assert_eq!(
            n("j'aimerais que quand Claude me réponde, ça soit une voix différente. Est-ce que tu peux le noter dans la liste ?"),
            vec![json!({ "type": "todo_add", "text": "J'aimerais que quand Claude me réponde, ça soit une voix différente.", "list": "general" })]
        );
        assert_eq!(n("retiens que Lucas préfère le jazz"), vec![json!({ "type": "todo_add", "text": "Lucas préfère le jazz", "list": "general" })]);
        assert_eq!(n("ajoute acheter du pain à ma liste"), vec![json!({ "type": "todo_add", "text": "Acheter du pain", "list": "moi" })]);
        assert_eq!(
            n("tu peux rajouter à la to-do list que Ilias doit avoir absolument tous les droits à partir de maintenant sur le bot ?"),
            vec![json!({ "type": "todo_add", "text": "Ilias doit avoir absolument tous les droits à partir de maintenant sur le bot", "list": "general" })]
        );
        assert_eq!(n("note that we need a new radio"), vec![json!({ "type": "todo_add", "text": "We need a new radio", "list": "general" })]);
        assert!(note("coupe le micro de Kévin").is_none());
    }

    #[test]
    fn prompt_lines_match_node() {
        assert_eq!(
            prompt_lines(&ctx(), &two_items()),
            vec!["Liste générale : 1. Mettre des fondus dans la musique ; 2. Une voix différente pour Claude.", "Liste perso de Alex : vide."]
        );
    }

    #[test]
    fn list_of_cases() {
        let store = two_items();
        let c = ctx();
        assert_eq!(list_of("wakandex", &c, &store).unwrap(), json!({ "id": "261531632589078528", "name": "WakandeX" }));
        assert_eq!(list_of("la liste de Lucas", &c, &store).unwrap(), json!({ "id": "lucas", "name": "Lucas" }));
        assert_eq!(list_of("moi", &c, &store).unwrap(), json!({ "id": "Alex", "name": "Alex" }));
        assert_eq!(list_of("", &c, &store).unwrap(), json!({ "id": "general", "name": null }));
        assert_eq!(list_of("the list", &c, &store).unwrap(), json!({ "id": "general", "name": null }));
    }

    #[test]
    fn says() {
        let mut s = Step { kind: "todo_add".into(), by: crate::types::By { id: "Alex".into(), ..Default::default() }, ..Default::default() };
        assert_eq!(say_add(&s, None, "fr").unwrap(), "C'est noté dans la liste générale.");
        s.list = Some(json!({ "id": "Alex", "name": "Alex" }));
        assert_eq!(say_add(&s, None, "en").unwrap(), "Added to your list.");
        s.list = Some(json!({ "id": "lucas", "name": "Lucas" }));
        s.set("open", json!([{ "text": "a b" }, { "text": "c" }]));
        assert_eq!(say_list(&s, None, "fr").unwrap(), "La liste de Lucas : 2 points. a b ; c.");
        s.text = "pain".into();
        assert_eq!(say_done(&s, None, "fr").unwrap(), "Je ne trouve pas « pain » dans la liste.");
    }
}
