//! Action modules, one per `bot/actions/*.js` file, in the same (alphabetical) order: the order of the
//! LLM prompt lines, of the schema's action enum and of the shortcuts.
use crate::io::Io;
use crate::store::Store;
use crate::types::{ActionDef, Chan, Ctx, Echo, Module, Role, Step};
use crate::util::Res;
use bobby_proto::infer::Understand;
use bobby_proto::msg::{Cap, Embed};
use serde_json::Value;
use std::sync::LazyLock;

pub mod aliases;
pub mod ask;
pub mod discord;
pub mod info;
pub mod music;
pub mod perms;
pub mod pc;
pub mod todo;

static MODULES: LazyLock<Vec<Module>> = LazyLock::new(|| {
    vec![aliases::module(), ask::module(), discord::module(), info::module(), music::module(), pc::module(), perms::module(), todo::module()]
});

/// `undo` is handled by the brain itself; its role is the one of what it undoes.
static UNDO: LazyLock<ActionDef> = LazyLock::new(|| ActionDef { role: Role::Prime, echo: Echo::If(|s| s.flag("done")), ..Default::default() });

pub fn modules() -> &'static [Module] {
    &MODULES
}

/// `ACTIONS[type]`.
pub fn def(kind: &str) -> Option<&'static ActionDef> {
    if kind == "undo" {
        return Some(&UNDO);
    }
    MODULES.iter().flat_map(|m| m.actions.iter()).find(|(k, _)| *k == kind).map(|(_, d)| d)
}

/// `Object.keys(ACTIONS)`: every action type in module order, then "undo". `voice_volume` is shortcut-only: kept out of the
/// LLM schema so the prompt stays byte-identical to Node.
pub fn types() -> Vec<&'static str> {
    MODULES.iter().flat_map(|m| m.actions.iter().map(|(k, _)| *k)).filter(|k| *k != "voice_volume").chain(["undo"]).collect()
}

// ---------- Running steps (batch R3b) ----------

/// How to undo a step: re-send caps with the old values, or put a stored document back as it was.
/// Node kept closures; plain data here, so an undo survives being moved across tasks.
#[derive(Debug, Clone, PartialEq)]
pub enum UndoOp {
    Cap(Cap),
    Restore { name: &'static str, prev: Value },
    /// Reverts only some items of a list document (so it never erases what someone else added meanwhile):
    /// drops the items with these ids, puts the items back at their place, replaces the ones with the same id.
    Items { name: &'static str, remove_ids: Vec<i64>, put: Vec<(usize, Value)>, replace: Vec<Value> },
}

impl UndoOp {
    /// Applies a store-side op (`Restore`, `Items`); a `Cap` is for the client and does nothing here.
    pub fn apply_to_store(&self, store: &Store) -> Result<(), String> {
        match self {
            UndoOp::Cap(_) => Ok(()),
            UndoOp::Restore { name, prev } => store.save(name, prev),
            UndoOp::Items { name, remove_ids, put, replace } => {
                let mut cur = match store.load_or(name, Value::Array(vec![])) {
                    Value::Array(a) => a,
                    _ => vec![],
                };
                cur.retain(|x| !x["id"].as_i64().is_some_and(|i| remove_ids.contains(&i)));
                for r in replace {
                    if let Some(x) = cur.iter_mut().find(|x| x["id"] == r["id"]) {
                        *x = r.clone();
                    }
                }
                for (i, v) in put {
                    cur.insert((*i).min(cur.len()), v.clone());
                }
                store.save(name, &Value::Array(cur))
            }
        }
    }
}

/// What a step may use while it runs (Node's `io` + the step's `ctx`).
pub struct Env<'a, L> {
    pub io: &'a Io,
    pub store: &'a Store,
    pub ctx: &'a Ctx,
    /// Local LLM, for the "about" answers.
    pub llm: &'a L,
    /// Asked in writing to a given channel (`writingIo`): every post goes there.
    pub post_to: Option<&'a Chan>,
}

/// What `io.post` took: a string or cards.
#[derive(Debug, Clone, PartialEq)]
pub enum Post {
    Text(String),
    Embeds(Vec<Embed>),
}

impl<L> Env<'_, L> {
    /// `io.post(content)`: posts to `post_to` or Bobby's text channel; returns the channel name it went to.
    pub async fn post(&self, content: Post) -> Res<String> {
        let channel = self.post_to.map(|c| c.id.clone());
        let cap = match content {
            Post::Text(t) => Cap::Post { channel, text: Some(t), embeds: vec![] },
            Post::Embeds(e) => Cap::Post { channel, text: None, embeds: e },
        };
        let v = self.io.run(cap).await?;
        Ok(v["channel"].as_str().unwrap_or("").to_string())
    }
}

/// `ACTIONS[s.type].run(s, io, lang)`: runs one step (never "undo", the brain handles it) and returns its undo.
pub async fn run<L: Understand + Sync>(s: &mut Step, env: &Env<'_, L>, lang: &str) -> Res<Vec<UndoOp>> {
    let module = MODULES.iter().find(|m| m.actions.iter().any(|(k, _)| *k == s.kind)).map_or("", |m| m.name);
    match module {
        "aliases" => aliases::run(s, env, lang).await,
        "ask" => ask::run(s, env, lang).await,
        "discord" => discord::run(s, env, lang).await,
        "info" => info::run(s, env, lang).await,
        "music" => music::run(s, env, lang).await,
        "pc" => pc::run(s, env, lang).await,
        "perms" => perms::run(s, env, lang).await,
        "todo" => todo::run(s, env, lang).await,
        _ => Ok(vec![]),
    }
}
