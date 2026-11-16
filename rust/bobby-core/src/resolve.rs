//! Name resolution (port of the "Name resolution" part of `bot/brain.js`): pick, targets, resolve, isText.
use crate::types::{Chan, Person};
use crate::util::{sim, Say};

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
