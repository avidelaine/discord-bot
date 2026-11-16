//! Port of `bot/actions/ask.js`: knowledge questions forwarded to Claude Haiku. The Claude CLI spawn
//! (`check`, `cli`, `ask`, `think`) and `run` are batch R3b; here are the texts, the definition and the
//! "Claude is connected" flag the announcement depends on.
use crate::types::{ActionDef, Module, Role, Step};
use std::sync::atomic::{AtomicBool, Ordering};

/// Claude Code logged in; checked at startup and again while it is not.
static READY: AtomicBool = AtomicBool::new(false);

pub fn ready() -> bool {
    READY.load(Ordering::Relaxed)
}

pub fn set_ready(v: bool) {
    READY.store(v, Ordering::Relaxed);
}

pub const OFF: [&str; 2] = ["Claude n'est pas encore branché.", "Claude isn't connected yet."];
pub const DOWN: [&str; 2] = ["Je n'arrive pas à joindre Claude.", "I can't reach Claude right now."];

fn say_answer(s: &Step, _: Option<&str>, _: &str) -> Option<String> {
    s.str("answer").map(str::to_string)
}

pub fn module() -> Module {
    Module {
        name: "ask",
        help: ["poser les questions de culture générale à Claude", "forward general knowledge questions to Claude"],
        short: ["les questions à Claude", "questions to Claude"],
        prompt: vec![r#"- ask : transmettre à Claude, un assistant plus savant, une vraie question de culture générale "text" (reformulée clairement, complète)"#],
        rules: vec![
            r#"- ask seulement pour une vraie question sur le monde (Claude cherche sur le web : météo, actualité, sport : toujours faisable) : culture, actualité, sciences, calcul, explication, recette, conseil général. Jamais pour une question sur toi (Bobby), le serveur, les gens présents, les droits, la conversation en cours, une demande de noter, une remarque ou un avis sur toi : ça, c'est about, todo_add ou une autre action locale."#,
            r#"- Ne réponds toi-même ("actions" vide, réponse courte dans "say") qu'aux salutations et aux politesses."#,
        ],
        examples: vec![
            r#""raconte-moi une blague" -> {"actions":[{"type":"ask","text":"Raconte une blague courte."}],"say":""}"#,
            r#""c'est quoi la capitale de l'Australie" -> {"actions":[{"type":"ask","text":"Quelle est la capitale de l'Australie ?"}],"say":""}"#,
            r#""who won the last world cup" -> {"actions":[{"type":"ask","text":"Who won the last FIFA World Cup?"}],"say":""}"#,
            r#""qui est le meilleur entre toi et Alexa" -> {"actions":[{"type":"about","text":"qui est le meilleur entre toi et Alexa"}],"say":""}"#,
        ],
        fields: vec![],
        early: None,
        quick: vec![],
        prompt_lines: None,
        actions: vec![(
            "ask",
            ActionDef {
                role: Role::Admin,
                text: true,
                // Claude takes a few seconds: say so right away, unless it is not connected.
                announce: Some(|lang| if ready() { Some(if lang == "en" { "Let me ask Claude." } else { "Je demande à Claude." }) } else { None }),
                say: Some(say_answer),
                ..Default::default()
            },
        )],
    }
}

/// R3b: port of the module's `run` functions (one per action type).
pub async fn run<L: bobby_proto::infer::Understand + Sync>(_s: &mut crate::types::Step, _env: &super::Env<'_, L>, _lang: &str) -> crate::util::Res<Vec<super::UndoOp>> {
    Ok(vec![]) // R3b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition() {
        let m = module();
        assert_eq!(m.actions.len(), 1);
        let d = &m.actions[0].1;
        assert!(d.text && d.role == Role::Admin);
        let announce = d.announce.unwrap();
        set_ready(false);
        assert_eq!(announce("fr"), None);
        set_ready(true);
        assert_eq!(announce("en"), Some("Let me ask Claude."));
        assert_eq!(announce("fr"), Some("Je demande à Claude."));
        set_ready(false);
        let mut s = Step::default();
        s.set("answer", "Canberra.");
        assert_eq!((d.say.unwrap())(&s, None, "fr").as_deref(), Some("Canberra."));
    }
}
