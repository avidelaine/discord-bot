//! Action modules, one per `bot/actions/*.js` file, in the same (alphabetical) order: the order of the
//! LLM prompt lines, of the schema's action enum and of the shortcuts.
use crate::types::{ActionDef, Echo, Module, Role};
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

/// `Object.keys(ACTIONS)`: every action type in module order, then "undo".
pub fn types() -> Vec<&'static str> {
    MODULES.iter().flat_map(|m| m.actions.iter().map(|(k, _)| *k)).chain(["undo"]).collect()
}
