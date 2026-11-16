//! Small helpers shared by the brain and the action modules (port of `bot/util.js`), plus JS-compatible regexes.
use fancy_regex::{Captures, Regex};
use std::fmt;
use unicode_normalization::UnicodeNormalization;

/// Error spoken to the user as is, in their language. `code`: "LLM_DOWN" / "LLM_JSON" (journal outcome), else None.
#[derive(Debug, Clone, PartialEq)]
pub struct Say {
    pub fr: String,
    pub en: String,
    pub code: Option<&'static str>,
}

impl Say {
    pub fn new(fr: impl Into<String>, en: impl Into<String>) -> Self {
        Self { fr: fr.into(), en: en.into(), code: None }
    }
    pub fn code(mut self, code: &'static str) -> Self {
        self.code = Some(code);
        self
    }
    /// `e.in(lang)` of the Node code.
    pub fn in_(&self, lang: &str) -> &str {
        if lang == "en" { &self.en } else { &self.fr }
    }
}

/// What can go wrong in the brain: a sentence for the user, or a failure ("Ça n'a pas marché").
#[derive(Debug, Clone, PartialEq)]
pub enum BrainError {
    Say(Say),
    /// `no_rights`: Discord refused for lack of permissions (code 50013): "Je n'ai pas les droits Discord pour ça."
    Failed { msg: String, no_rights: bool },
}

impl BrainError {
    pub fn failed(msg: impl Into<String>) -> Self {
        BrainError::Failed { msg: msg.into(), no_rights: false }
    }
}

impl From<Say> for BrainError {
    fn from(s: Say) -> Self {
        BrainError::Say(s)
    }
}

impl fmt::Display for BrainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BrainError::Say(s) => write!(f, "{}", s.fr),
            BrainError::Failed { msg, .. } => write!(f, "{msg}"),
        }
    }
}

pub type Res<T> = Result<T, BrainError>;

/// Accents off, lower case, letters and digits only ("🎮 Gaming" -> "gaming").
pub fn norm(s: &str) -> String {
    s.nfd()
        .filter(|c| !('\u{300}'..='\u{36f}').contains(c))
        .collect::<String>()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        .collect()
}

pub fn lev(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i];
        for j in 1..=b.len() {
            let v = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + usize::from(a[i - 1] != b[j - 1]));
            cur.push(v);
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Similarity in [0, 1] of two names after `norm`: 1 equal, 0.9 one contains the other (3+ chars), else 1 - lev/len.
pub fn sim(q: &str, c: &str) -> f64 {
    let (a, b) = (norm(q), norm(c));
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a == b {
        return 1.0;
    }
    if a.len().min(b.len()) >= 3 && (a.contains(&b) || b.contains(&a)) {
        return 0.9;
    }
    1.0 - lev(&a, &b) as f64 / a.len().max(b.len()) as f64
}

pub fn words(t: &str) -> usize {
    t.split_whitespace().count()
}

/// One past exchange with Bobby (the brain's conversation memory). `at`: ms since the epoch.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Hist {
    pub at: i64,
    /// Speaker id (None: unknown, counts as everyone's).
    pub id: Option<String>,
    pub who: String,
    pub query: String,
    /// What really ran (plan text), "" if nothing.
    pub did: String,
    /// A yes / no to a confirmation: never replayed by "refais".
    pub confirm: bool,
    pub reply: String,
}

fn ago(at: i64, now: i64) -> String {
    let m = ((now - at) as f64 / 60_000.0).round() as i64;
    if m < 1 { "à l'instant".into() } else { format!("il y a {m} min") }
}

/// The recent conversation as prompt lines (`talk` of util.js). `now`: ms since the epoch.
pub fn talk(history: &[Hist], now: i64) -> Vec<String> {
    history
        .iter()
        .map(|h| {
            let mut s = String::from("- ");
            if h.at != 0 {
                s += &format!("{}, ", ago(h.at, now));
            }
            s += &format!("{} : \"{}\"", if h.who.is_empty() { "?" } else { &h.who }, h.query);
            if !h.did.is_empty() {
                s += &format!(" -> fait : {}", h.did);
            }
            if !h.reply.is_empty() {
                s += &format!(" -> Bobby : \"{}\"", h.reply);
            }
            s
        })
        .collect()
}

// ---------- JS-compatible regexes ----------

const WORD: &str = "A-Za-z0-9_";

/// Compiles a JavaScript regex source with JavaScript semantics where they differ from Rust's:
/// `\b`, `\w`, `\d` are ASCII-only in JS (so "é" is NOT a word character: `\bpas` matches inside "dépas"),
/// and `\/` is a plain slash. Write the pattern exactly as in the JS file (raw string). Panics on a bad
/// pattern: they are all static, and every one is covered by a test.
pub fn re(js: &str) -> Regex {
    Regex::new(&translate(js)).unwrap_or_else(|e| panic!("bad regex {js}: {e}"))
}

/// Same with the JS `i` flag.
pub fn re_i(js: &str) -> Regex {
    Regex::new(&format!("(?i){}", translate(js))).unwrap_or_else(|e| panic!("bad regex {js}: {e}"))
}

fn translate(js: &str) -> String {
    let mut out = String::new();
    let mut in_class = false;
    let mut it = js.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\\' => {
                let n = it.next().expect("trailing backslash");
                match (n, in_class) {
                    ('b', false) => out += &format!("(?:(?<=[{WORD}])(?![{WORD}])|(?<![{WORD}])(?=[{WORD}]))"),
                    ('B', false) => out += &format!("(?:(?<=[{WORD}])(?=[{WORD}])|(?<![{WORD}])(?![{WORD}]))"),
                    ('w', false) => out += &format!("[{WORD}]"),
                    ('w', true) => out += WORD,
                    ('W', false) => out += &format!("[^{WORD}]"),
                    ('d', false) => out += "[0-9]",
                    ('d', true) => out += "0-9",
                    ('D', false) => out += "[^0-9]",
                    ('/', _) => out.push('/'),
                    _ => {
                        out.push('\\');
                        out.push(n);
                    }
                }
            }
            '[' if !in_class => {
                in_class = true;
                out.push(c);
                // a leading ']' or '^]' is literal in Rust, an empty class in JS: never used in Bobby's patterns
            }
            ']' if in_class => {
                in_class = false;
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

/// `re.test(s)`.
pub fn hit(r: &Regex, s: &str) -> bool {
    r.is_match(s).unwrap_or(false)
}

/// `s.match(re)` (non-global): the captures of the first match.
pub fn caps<'t>(r: &Regex, s: &'t str) -> Option<Captures<'t>> {
    r.captures(s).ok().flatten()
}

/// Capture group `i` as &str ("" when the group did not take part, like a JS `undefined` used as text).
pub fn group<'t>(c: &Captures<'t>, i: usize) -> &'t str {
    c.get(i).map_or("", |m| m.as_str())
}

/// `s.replace(re, with)` for a non-global JS regex: first match only. `$1` etc. work as in JS.
pub fn replace1(r: &Regex, s: &str, with: &str) -> String {
    r.replacen(s, 1, with).into_owned()
}

/// `s.replace(re, with)` for a global (`/g`) JS regex.
pub fn replace_all(r: &Regex, s: &str, with: &str) -> String {
    r.replace_all(s, with).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(norm("🎮 Gaming"), "gaming");
        assert_eq!(norm("Kévin"), "kevin");
        assert_eq!(sim("🎮 Gaming", "gaming"), 1.0);
        assert_eq!(sim("kev du 93", "kev_du_93"), 1.0);
        assert!((sim("ziklone", "Zyklon") - (1.0 - 2.0 / 7.0)).abs() < 1e-9);
        assert_eq!(sim("", "x"), 0.0);
        assert_eq!(lev("kitten", "sitting"), 3);
        assert_eq!(words("  a b  c "), 3);
    }

    #[test]
    fn js_regex_semantics() {
        // JS \b is ASCII: "é" is not a word character
        assert!(hit(&re(r"\bpas\b"), "dépas"));
        assert!(hit(&re(r"\bpas\b"), "je ne veux pas"));
        assert!(!hit(&re(r"\bpas\b"), "passer"));
        assert!(hit(&re_i(r"^Bobby\b"), "BOBBY, viens"));
        assert!(hit(&re(r"[\w-]+$"), "dev-bot"));
        assert!(hit(&re(r"a\/b"), "a/b"));
        assert!(hit(&re_i(r"(?:^|[^\p{L}])noté(?![\p{L}])"), "c'est NOTÉ !"));
        let c = caps(&re(r"^mute (.+)$"), "mute Kévin").unwrap();
        assert_eq!(group(&c, 1), "Kévin");
        assert_eq!(replace1(&re(r"a"), "aaa", "b"), "baa");
        assert_eq!(replace_all(&re(r", ([^,]*)$"), "a, b, c", " et $1"), "a, b et c");
    }

    #[test]
    fn talk_lines() {
        let h = vec![
            Hist { at: 1_000, id: Some("1".into()), who: "Alex".into(), query: "mute Kévin".into(), did: "mute -> Kévin".into(), confirm: false, reply: "".into() },
            Hist { at: 0, id: None, who: "".into(), query: "salut".into(), did: "".into(), confirm: false, reply: "Salut !".into() },
        ];
        assert_eq!(talk(&h, 1_000 + 3 * 60_000), vec![
            "- il y a 3 min, Alex : \"mute Kévin\" -> fait : mute -> Kévin".to_string(),
            "- ? : \"salut\" -> Bobby : \"Salut !\"".to_string(),
        ]);
        assert_eq!(talk(&h[..1], 1_000 + 20_000)[0], "- à l'instant, Alex : \"mute Kévin\" -> fait : mute -> Kévin");
    }
}
