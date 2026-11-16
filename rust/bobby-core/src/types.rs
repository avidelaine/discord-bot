//! The brain's own data: people, channels, the context of one request, plan steps and action definitions.
//! Node steps were free-form objects; here a `Step` has typed common fields plus `f`, a JSON map for
//! everything a module adds (raw LLM fields like "kind"/"mode"/"quiet"/"offer", and run results like
//! "res"/"found"/"count"/"where"/"done"/"card"/"voice").
use crate::store::Store;
use crate::util::Hist;
use fancy_regex::{Captures, Regex};
use serde_json::{Map, Value};

/// Levels: other < prime < admin < super (derive order = `roles.LEVEL`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Role {
    #[default]
    Other,
    Prime,
    Admin,
    Super,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Other => "other",
            Role::Prime => "prime",
            Role::Admin => "admin",
            Role::Super => "super",
        }
    }
    pub fn parse(s: &str) -> Option<Role> {
        Some(match s {
            "other" => Role::Other,
            "prime" => Role::Prime,
            "admin" => Role::Admin,
            "super" => Role::Super,
            _ => return None,
        })
    }
}

/// A person in voice (or the speaker when not in voice). `channel`: voice channel NAME (Node used names).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Person {
    pub id: String,
    pub name: String,
    pub channel: Option<String>,
    /// Nicknames given by voice (store "aliases").
    pub nicknames: Vec<String>,
    /// username, global name, then nicknames: what `pick` also matches.
    pub aliases: Vec<String>,
    /// Discord role ids of the member (`Member.roles`), for `roles::role_of`.
    pub roles: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Chan {
    pub id: String,
    pub name: String,
}

/// Node's `context(member, guild)` + `role`, `history`, `trouble` of one request.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Ctx {
    pub speaker: Person,
    /// People in voice, bots excluded.
    pub members: Vec<Person>,
    pub voice_channels: Vec<Chan>,
    pub text_channels: Vec<Chan>,
    /// Bobby's default text channel (`io.textChannel`).
    pub text_channel: Option<Chan>,
    pub role: Role,
    pub history: Vec<Hist>,
    /// Lines "- who : "query" -> why" (what went wrong lately).
    pub trouble: Vec<String>,
}

/// Who asked (`step.by`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct By {
    pub id: String,
    pub name: String,
    pub role: Role,
}

/// One resolved step of a plan.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Step {
    /// The action type ("mute", "todo_add"...).
    pub kind: String,
    pub text: String,
    pub by: By,
    pub targets: Option<Vec<Person>>,
    pub channel: Option<Chan>,
    /// To-do list resolved by actions/todo (`step.list`, `step.to`).
    pub list: Option<Value>,
    pub to: Option<Value>,
    /// Every other field (see module doc).
    pub f: Map<String, Value>,
}

impl Step {
    pub fn str(&self, k: &str) -> Option<&str> {
        self.f.get(k).and_then(Value::as_str)
    }
    /// JS truthiness of a field.
    pub fn flag(&self, k: &str) -> bool {
        match self.f.get(k) {
            None | Some(Value::Null) => false,
            Some(Value::Bool(b)) => *b,
            Some(Value::Number(n)) => n.as_f64() != Some(0.0),
            Some(Value::String(s)) => !s.is_empty(),
            Some(_) => true,
        }
    }
    pub fn set(&mut self, k: &str, v: impl Into<Value>) {
        self.f.insert(k.to_string(), v.into());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Who {
    #[default]
    No,
    Yes,
    Optional,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChanKind {
    #[default]
    None,
    Voice,
    Text,
    /// 'text?': a text channel only if one was named.
    TextOpt,
}

/// Is the step an echo (done silently after the ack)?
#[derive(Clone, Copy, Default)]
pub enum Echo {
    #[default]
    No,
    Yes,
    If(fn(&Step) -> bool),
}

/// `say: { fr(step, names), en }` as one function: (step, names joined, lang) -> line. None or "" = nothing said.
pub type SayFn = fn(&Step, Option<&str>, &str) -> Option<String>;

/// One action type (`actions: { type: {...} }` of a module). `run` is not here: see `actions::run`.
#[derive(Clone)]
pub struct ActionDef {
    pub role: Role,
    pub who: Who,
    pub one: bool,
    pub channel: ChanKind,
    pub text: bool,
    pub confirm: bool,
    pub list: bool,
    pub to: bool,
    pub info: bool,
    pub saves: bool,
    pub echo: Echo,
    /// Fixes a known LLM confusion on the raw action; None = keep it.
    pub normalize: Option<fn(&Value) -> Option<Value>>,
    /// Said before running (lang -> line).
    pub announce: Option<fn(&str) -> Option<&'static str>>,
    pub say: Option<SayFn>,
}

impl Default for ActionDef {
    /// Node's default role is 'admin' (`ACTIONS[t].role || 'admin'`).
    fn default() -> Self {
        Self { role: Role::Admin, who: Who::No, one: false, channel: ChanKind::None, text: false, confirm: false, list: false, to: false, info: false, saves: false, echo: Echo::No, normalize: None, announce: None, say: None }
    }
}

/// A shortcut: regex on the cleaned sentence -> raw actions (JSON objects like the LLM's).
pub type QuickFn = fn(&Captures) -> Vec<Value>;

/// One action module (one `bot/actions/*.js` file). Order of `actions` = key order of the JS object.
pub struct Module {
    pub name: &'static str,
    /// [fr, en]
    pub help: [&'static str; 2],
    pub short: [&'static str; 2],
    pub prompt: Vec<&'static str>,
    pub rules: Vec<&'static str>,
    pub examples: Vec<&'static str>,
    /// Extra step fields of the LLM schema, in order.
    pub fields: Vec<(&'static str, Value)>,
    /// Checked first, on the raw sentence.
    pub early: Option<fn(&str) -> Option<Vec<Value>>>,
    pub quick: Vec<(Regex, QuickFn)>,
    /// Live context lines added to each LLM request.
    pub prompt_lines: Option<fn(&Ctx, &Store) -> Vec<String>>,
    pub actions: Vec<(&'static str, ActionDef)>,
}

impl Module {
    /// A module with nothing in it yet.
    pub fn empty(name: &'static str) -> Self {
        Self { name, help: ["", ""], short: ["", ""], prompt: vec![], rules: vec![], examples: vec![], fields: vec![], early: None, quick: vec![], prompt_lines: None, actions: vec![] }
    }
}
