//! Port of `bot/actions/ask.js`: knowledge questions forwarded to Claude Haiku. The Claude CLI spawn
//! (`check`, `cli`, `ask`, `think`) and `run` are batch R3b; here are the texts, the definition and the
//! "Claude is connected" flag the announcement depends on.
use super::{Env, UndoOp};
use crate::types::{ActionDef, Module, Role, Step};
use crate::util::{talk, Hist, Res};
use bobby_proto::infer::Understand;
use serde_json::Value;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(60);
/// `windowsHide`: no console window flashing for each call.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

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

fn model() -> String {
    std::env::var("CLAUDE_MODEL").ok().filter(|m| !m.is_empty()).unwrap_or_else(|| "haiku".into())
}

/// `claude auth status` through the shell, like Node's `shell: true` (so `claude.cmd` resolves on Windows).
pub async fn check() -> bool {
    let mut cmd = if cfg!(windows) {
        let mut c = tokio::process::Command::new("cmd");
        c.args(["/C", "claude auth status"]);
        c
    } else {
        let mut c = tokio::process::Command::new("sh");
        c.args(["-c", "claude auth status"]);
        c
    };
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let ok = match cmd.output().await {
        Ok(o) => serde_json::from_slice::<Value>(&o.stdout).ok().is_some_and(|v| v["loggedIn"] == true),
        Err(_) => false,
    };
    set_ready(ok);
    println!("[claude] {}", if ok { "logged in: questions go to Claude" } else { "not logged in: Bobby answers \"Claude n'est pas encore branché\"" });
    ok
}

/// One Claude Code call. No files, no shell, no MCP, no user settings; tools: web search at most.
/// Spawned without a shell, like Node's `spawn('claude', [...])`: the native `claude.exe` is on the PATH.
async fn cli(prompt: &str, system: &str, tools: &str) -> Result<String, String> {
    let dir = std::env::temp_dir().join("bobby-claude"); // empty working folder: nothing to read
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut cmd = tokio::process::Command::new("claude");
    cmd.args(["-p", prompt, "--model", &model(), "--append-system-prompt", system, "--tools", tools]);
    if !tools.is_empty() {
        cmd.args(["--allowedTools", tools]);
    }
    cmd.args(["--setting-sources", "", "--strict-mcp-config", "--no-session-persistence", "--output-format", "text"]);
    cmd.current_dir(dir).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    // the timeout drops the child, which kills it
    let o = tokio::time::timeout(TIMEOUT, cmd.output()).await;
    let o = match o {
        Ok(r) => r.map_err(|e| e.to_string())?,
        Err(_) => return Err("claude exited null: ".into()),
    };
    let out = String::from_utf8_lossy(&o.stdout).trim().to_string();
    let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
    if o.status.success() && !out.is_empty() {
        Ok(out)
    } else {
        let code = o.status.code().map_or("null".to_string(), |c| c.to_string());
        Err(format!("claude exited {code}: {}", (if err.is_empty() { out } else { err }).chars().take(200).collect::<String>()))
    }
}

/// written: the answer is posted, not spoken, so it may be longer and use Discord markdown.
/// history: the recent conversation with Bobby, so "et lui ?" or "explique mieux" make sense to Claude too.
pub async fn ask(question: &str, lang: &str, written: bool, history: &[Hist]) -> Result<String, String> {
    let en = lang == "en";
    let style = if written {
        if en {
            "Answer in English, for a Discord message: complete but concise (under 1500 characters), Discord markdown allowed (bold, bullet lists)."
        } else {
            "Réponds en français, pour un message Discord : complet mais concis (moins de 1500 caractères), markdown Discord autorisé (gras, listes à puces)."
        }
    } else if en {
        "Answer in English, in one or two short spoken sentences, no markdown, no lists."
    } else {
        "Réponds en français, en une ou deux phrases courtes à dire à voix haute, sans markdown ni liste."
    };
    let convo = if history.is_empty() {
        String::new()
    } else {
        format!("Conversation récente avec Bobby dans le salon vocal (contexte seulement) :\n{}\n\nQuestion : ", talk(history, chrono::Utc::now().timestamp_millis()).join("\n"))
    };
    cli(&format!("{convo}{question}"), &format!("You answer for Bobby, a Discord voice assistant. {style}"), "WebSearch").await
}

/// Fallback brain (admins): when the local model is unsure, Claude reads the same prompt and picks among the
/// same actions. -> the parsed JSON plan, or an error.
pub async fn think(system: &str, prompt: &str) -> Result<Value, String> {
    let out = cli(prompt, &format!("{system}\n\nRéponds uniquement avec l'objet JSON, sans texte autour ni bloc de code."), "").await?;
    plan_from(&out)
}

/// From the first `{` to the last `}` of Claude's answer, parsed.
fn plan_from(out: &str) -> Result<Value, String> {
    match (out.find('{'), out.rfind('}')) {
        (Some(a), Some(b)) if a <= b => serde_json::from_str(&out[a..=b]).map_err(|e| e.to_string()),
        _ => Err("no JSON object in Claude's answer".into()),
    }
}

/// Port of the module's `run` functions (one per action type).
pub async fn run<L: Understand + Sync>(s: &mut Step, env: &Env<'_, L>, lang: &str) -> Res<Vec<UndoOp>> {
    let i = usize::from(lang == "en");
    if !ready() {
        s.set("answer", OFF[i]);
        // re-check for next time, without waiting
        #[cfg(not(test))]
        tokio::spawn(check());
        return Ok(vec![]);
    }
    let t0 = std::time::Instant::now();
    let written = s.flag("written");
    match ask(&s.text, lang, written, &env.ctx.history).await {
        Ok(answer) => {
            s.set("voice", "claude");
            if written {
                s.set("card", format!("❓ **{}**\n\n{answer}", s.text));
            }
            s.set("answer", answer);
        }
        Err(e) => {
            eprintln!("[claude] {e}");
            s.set("answer", DOWN[i]);
        }
    }
    println!("[claude] {} ms: {}", t0.elapsed().as_millis(), s.str("answer").unwrap_or(""));
    Ok(vec![])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

    // The ready flag is global: tests that set it do not run at the same time.
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// "Claude not logged in" (unit.test.js): immediate answer, no wait, never the real CLI.
    #[tokio::test]
    async fn not_logged_in_answers_at_once() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (io, sent) = crate::testing::fake_client(|_| Ok(serde_json::json!({})));
        let (store, ctx, llm) = (crate::store::Store::temp(), crate::testing::ctx(), crate::testing::FakeLlm::new(vec![]));
        let env = Env { io: &io, store: &store, ctx: &ctx, llm: &llm, post_to: None };
        set_ready(false);
        let mut q = Step { kind: "ask".into(), text: "Quelle est la capitale du Congo ?".into(), ..Default::default() };
        assert!(run(&mut q, &env, "fr").await.unwrap().is_empty());
        let say = module().actions[0].1.say.unwrap();
        assert_eq!(say(&q, None, "fr").as_deref(), Some("Claude n'est pas encore branché."));
        assert_eq!(q.str("voice"), None);
        run(&mut q, &env, "en").await.unwrap();
        assert_eq!(q.str("answer"), Some("Claude isn't connected yet."));
        assert!(crate::testing::caps(&sent).is_empty());
    }

    #[test]
    fn plan_is_the_json_object_in_the_answer() {
        assert_eq!(plan_from("Voici : {\"actions\": []} ok").unwrap(), serde_json::json!({ "actions": [] }));
        assert_eq!(plan_from("{\"a\": {\"b\": 1}}").unwrap()["a"]["b"], 1);
        assert!(plan_from("pas de json").is_err());
        assert!(plan_from("} {").is_err());
        assert!(plan_from("{oops}").is_err());
    }
}
