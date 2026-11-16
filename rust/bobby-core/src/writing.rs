//! Port of `bot/brain.js` "Answers in writing" (lines 485-544, minus `writingIo` and `postWritten`, which are
//! R3b: they touch the io). Also the text channel named anywhere in a sentence (`find_channel`).
use crate::guard::{FORM_FIX, WRITE_HINT};
use crate::types::{Chan, Ctx};
use crate::util::{caps, group, hit, norm, re, re_i, replace1, replace_all, sim, Hist};
use fancy_regex::Regex;
use std::sync::LazyLock;

// "dis-moi par écrit…", "in writing" (anywhere), or "écris-moi…", "envoie-moi…", "send me…" (at the start).
// "écris dans général bonjour" is not one of them: that is the message action.
// Also "dans le chat" anywhere, and "montre-moi…" / "affiche…" at the start ("explique dans le chat ce que tu sais
// faire" posted a plain LLM paragraph instead of the capabilities card).
pub static IN_WRITING: LazyLock<Regex> = LazyLock::new(|| re_i(r"[\s,]*(?:par écrit|à l'écrit|a l'ecrit|in writing|by text|as a message|(?:dans|sur|in|on) (?:le |the )?chat)(?![\p{L}])[\s,]*"));
pub static SHOW: LazyLock<Regex> = LazyLock::new(|| re_i(r"^(?:(?:est-ce que |est ce que )?(?:tu peux|tu pourrais|peux-tu|pourrais-tu)(?: me| nous)? )?(?:montre|affiche|afficher|montrer)(?:[- ]moi|[- ]nous)?(?![\p{L}])"));
// A text channel named anywhere: "dans le chat dev-bot-test", "dans le channel devbot test", "dans général".
static CHANNEL_AT: LazyLock<Regex> = LazyLock::new(|| re_i(r"(?:^|\s)(?:dans|sur|en|in|on|to) (?:le |la |the )?(?:(?:chat|channel|salon|canal)(?: texte)? )?#?((?:[\p{L}\d_-]+)(?: [\p{L}\d_-]+){0,2})"));
static SPACES: LazyLock<Regex> = LazyLock::new(|| re(r"\s+"));

/// The text channel named in `text` and the sentence without it (`{ channel, rest }`; no channel: `text` as is).
pub fn find_channel(text: &str, ctx: &Ctx) -> (Option<Chan>, String) {
    for m in CHANNEL_AT.captures_iter(text).flatten() {
        let (all, g1) = (group(&m, 0), group(&m, 1));
        let ws: Vec<&str> = g1.split(' ').collect();
        // the 1 to 3 words that best match a channel; on a tie the fewest ("DevBotTest ce que" also contains it)
        let mut best: (String, Option<&Chan>, f64) = (String::new(), None, 0.0);
        for i in 0..ws.len() {
            let name = ws[..=i].join(" ");
            if norm(&name).len() < 3 {
                continue;
            }
            for x in &ctx.text_channels {
                let s = sim(&name, &x.name);
                if s > best.2 {
                    best = (name.clone(), Some(x), s);
                }
            }
        }
        if let (true, Some(x)) = (best.2 >= 0.8, best.1) {
            let said = format!("{}{}", &all[..all.len() - g1.len()], best.0); // the preposition and those words
            let rest = text.replacen(&said, " ", 1);
            return (Some(x.clone()), replace_all(&SPACES, &rest, " ").trim().to_string());
        }
    }
    (None, text.to_string())
}

// Never "mets" or "envoie-le" ("mets-moi dans Gaming", "envoie-le en AFK" are moves); "écris-le" repeats the last answer.
static WRITE_ME: LazyLock<Regex> = LazyLock::new(|| re_i(r"^(?:(?:est-ce que |est ce que )?(?:tu peux|tu pourrais|peux-tu|can you|could you|please)[\s,]+)?((?:écris|ecris)-(?:le|la|les|ça|ca)(?:-moi)?|(?:écris|ecris|envoie)[- ](?:moi|nous)|(?:write|send|text)(?: it| that| this)?(?: to)? me|write (?:it|that) down)(?![\p{L}])[\s,:]*"));
static REPEAT: LazyLock<Regex> = LazyLock::new(|| re_i(r"^(?:écris|ecris)-|^write (?:it|that) down")); // no request of its own: the last one, in writing
pub static RIGHTS_Q: LazyLock<Regex> = LazyLock::new(|| re_i(r"droit|admin|permission|rights|liste noire|blacklist"));
static TRAILING: LazyLock<Regex> = LazyLock::new(|| re(r"[.!?…]+$"));
static AIM: LazyLock<Regex> = LazyLock::new(|| re(r"[’`]"));
static MOVE_WORD: LazyLock<Regex> = LazyLock::new(|| re_i(r"^(?:dans|en|sur|vers|to|in|into)\b"));
static SEND_WORD: LazyLock<Regex> = LazyLock::new(|| re_i(r"^(?:write|send|text)"));

/// The request without its writing words ("écris-le" alone repeats the last request) and the channel asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct Written {
    pub query: String,
    pub channel: Option<Chan>,
}

pub fn writing(query: &str, ctx: &Ctx, history: &[Hist]) -> Option<Written> {
    if crate::actions::modules().iter().any(|x| x.early.is_some_and(|f| f(query).is_some())) {
        return None; // "note que le rapport est à rendre par écrit" is a note
    }
    // "j'aurais voulu des tableaux" after a request: the same request again, in writing (the 4B model read it as a note)
    if hit(&FORM_FIX, query) && hit(&WRITE_HINT, query) && !history.is_empty() {
        let last = &history[history.len() - 1].query;
        let q = writing(last, ctx, &[]).map(|w| w.query).filter(|q| !q.is_empty()).unwrap_or_else(|| last.clone());
        return Some(Written { query: q, channel: find_channel(query, ctx).0 });
    }
    let q = replace_all(&AIM, query, "'");
    let q = replace1(&TRAILING, &q, "");
    let q = q.trim();
    let m = caps(&WRITE_ME, q);
    if m.is_none() && !hit(&SHOW, q) && !hit(&IN_WRITING, q) {
        return None;
    }
    // "montre-moi la liste" keeps its verb: the shortcuts know it
    let rest = m.as_ref().map_or(q, |m| &q[group(m, 0).len()..]);
    let verb = m.as_ref().map_or("", |m| group(m, 1));
    // "envoie-moi en AFK", "envoie-nous dans Gaming": a move, not an answer in writing
    if m.is_some() && !hit(&REPEAT, verb) && hit(&MOVE_WORD, rest) {
        return None;
    }
    let (channel, rest) = find_channel(rest, ctx);
    let rest = replace1(&IN_WRITING, &rest, " ");
    let rest = replace_all(&SPACES, &rest, " ").trim().to_string();
    if rest.is_empty() {
        if m.is_some() && !hit(&REPEAT, verb) {
            return None;
        }
        return Some(Written { query: history.last().map(|h| h.query.clone()).unwrap_or_default(), channel });
    }
    // "écris-moi la liste" -> "donne-moi la liste", which the shortcuts know
    let query = if m.is_some() { format!("{} {rest}", if hit(&SEND_WORD, verb) { "give me" } else { "donne-moi" }) } else { rest };
    Some(Written { query, channel })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shortcuts::quick;
    use crate::testing;

    /// `W(q, h)` of the JS tests: [query, channel name or null].
    fn w(q: &str, h: &[Hist]) -> Option<(String, Option<String>)> {
        writing(q, &testing::ctx(), h).map(|w| (w.query, w.channel.map(|c| c.name)))
    }
    fn some(q: &str, c: Option<&str>) -> Option<(String, Option<String>)> {
        Some((q.to_string(), c.map(String::from)))
    }
    fn h(query: &str) -> Vec<Hist> {
        vec![Hist { query: query.into(), ..Default::default() }]
    }

    // batch3Written (unit.test.js 741-758, before the hear() part)
    #[test]
    fn written_answers() {
        assert_eq!(w("dis-moi par écrit ce que tu sais faire", &[]), some("dis-moi ce que tu sais faire", None));
        assert_eq!(w("écris-moi la liste", &[]), some("donne-moi la liste", None));
        assert_eq!(w("envoie-moi les surnoms dans memes", &[]), some("donne-moi les surnoms", Some("memes")));
        assert_eq!(w("écris-le dans général", &h("quelle heure est-il")), some("quelle heure est-il", Some("général")));
        assert_eq!(w("what can you do, in writing?", &[]), some("what can you do", None));
        assert_eq!(w("send me the list", &[]), some("give me the list", None));
        assert_eq!(w("écris dans général que la réunion est à 21 h", &[]), None); // the message action, not an answer in writing
        assert_eq!(w("qu'est-ce que tu sais faire", &[]), None);
        // After review: moves and notes are never taken for answers in writing
        for q in ["mets la musique de Daft Punk", "mets-le en pause", "envoie-le en AFK", "mets-moi dans Gaming", "envoie-moi en AFK", "envoie-nous dans Gaming", "écris le message bonjour dans général", "note que le rapport est à rendre par écrit"] {
            assert_eq!(w(q, &[]), None, "{q}");
        }
        for (q, kind) in [("dis-moi ce que tu sais faire", "help"), ("donne-moi la liste", "todo_list"), ("donne-moi les surnoms", "alias_list"), ("qui a quels droits", "rights_list"), ("give me the list", "todo_list"), ("donne-moi les droits de Lucas", "perm_show")] {
            assert_eq!(quick(q).unwrap()[0]["type"], kind, "{q}");
        }
    }

    // batch4 "reasoning guards" (387-389) and "written mode" (397-403)
    #[test]
    fn written_mode() {
        let fix = writing("quand je te demande ça, j'aurais voulu des tableaux, des trucs comme ça", &testing::ctx(), &h("explique dans le chat ce que tu sais faire")).unwrap();
        assert_eq!((fix.query.as_str(), quick(&fix.query).unwrap()[0]["type"].as_str().unwrap()), ("explique ce que tu sais faire", "help"));
        assert_eq!(w("explique dans le chat ce que tu sais faire", &[]), some("explique ce que tu sais faire", None));
        assert_eq!(w("est-ce que tu es capable de m'expliquer dans le chat DevBotTest ce que tu es capable de faire, s'il te plaît ?", &[]), some("est-ce que tu es capable de m'expliquer ce que tu es capable de faire, s'il te plaît", Some("dev-bot-test")));
        assert_eq!(w("montre-moi dans le channel devbot test ta liste", &[]), some("montre-moi ta liste", Some("dev-bot-test")));
        assert_eq!(w("affiche ma liste", &[]), some("affiche ma liste", None));
        assert_eq!(quick("explique ce que tu sais faire").unwrap()[0]["type"], "help");
        assert_eq!(w("écris dans général que la réunion est à 21 h", &[]), None);
    }

    #[test]
    fn channel_in_a_sentence() {
        let ctx = testing::ctx();
        let (c, rest) = find_channel("dis-moi dans le chat dev-bot-test la liste", &ctx);
        assert_eq!((c.unwrap().name, rest.as_str()), ("dev-bot-test".to_string(), "dis-moi la liste"));
        assert_eq!(find_channel("viens dans Gaming", &ctx), (None, "viens dans Gaming".to_string()));
        assert!(hit(&RIGHTS_Q, "qui est admin"));
    }
}
