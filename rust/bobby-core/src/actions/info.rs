//! Port of `bot/actions/info.js`: static texts, action definitions, shortcuts (filled by R3a), run (R3b).
use crate::types::Module;

pub fn module() -> Module {
    Module::empty("info")
}
