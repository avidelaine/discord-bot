//! Tolerant detection of the wake word "bobby" in the transcribed text (port of `bot/wake.js`).
//! ponytail: this list IS the tuning knob. Whisper hotwords already fix most spellings;
//! these variants catch what Whisper still gets wrong. "baby", "hobby", "lobby" are left out on purpose.
use crate::util::{hit, re};
use std::sync::LazyLock;

const VARIANTS: &[&str] = &[
    "bobby", "bobbie", "bobie", "bobbi", "bobi", "boby", "bobbys", "bobbey", "bobbee", "bobe",
    "moby", // "Moby est-ce que tu peux me redéplacer…" (2026-11-12): the wake model had heard "Bobby"
];

static LETTERS: LazyLock<fancy_regex::Regex> = LazyLock::new(|| re(r"\p{L}+"));
// "Bobby" glued to the previous word by Whisper ("Dibobby", "Heybobby"); "hobby"/"lobby" never end in "bobby".
static GLUED: LazyLock<fancy_regex::Regex> = LazyLock::new(|| re(r".bobb?(y|ie|i)$"));

/// Accents off and lower case, letters kept (wake.js's own norm, not util's).
fn fold(t: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    t.nfd().filter(|c| !('\u{300}'..='\u{36f}').contains(c)).collect::<String>().to_lowercase()
}

/// The original token that matches the wake word, or None.
pub fn find_wake(text: &str) -> Option<&str> {
    LETTERS.find_iter(text).flatten().map(|m| m.as_str()).find(|w| {
        let n = fold(w);
        VARIANTS.contains(&n.as_str()) || hit(&GLUED, &n)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Port of bot/wake.test.js (same cases).
    #[test]
    fn wake_word() {
        for s in ["Bobby ajoute du lait", "hey bobbie", "Boby raconte une blague", "BOBI mets la musique", "Bobé tu es là", "d'accord Bobby.", "Dibobby, coupe Kévin", "Diboby.", "Heybobby"] {
            assert!(find_wake(s).is_some(), "missed: {s}");
        }
        for s in ["un hobby sympa", "le lobby de l hotel", "mon baby dort", "il a un beau body", "rien a signaler"] {
            assert_eq!(find_wake(s), None, "false positive: {s}");
        }
        assert_eq!(find_wake("Bobby mets la musique"), Some("Bobby"));
    }
}
