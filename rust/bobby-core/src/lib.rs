//! Bobby's brain: shortcuts, context, rights, actions, storage. Port of `bot/brain.js` and friends.
//! It touches neither Discord nor hardware: every side effect is a `BrainMsg` to a client (see `io`).
pub mod actions;
pub mod context;
pub mod guard;
pub mod io;
pub mod prompt;
pub mod resolve;
pub mod roles;
pub mod shortcuts;
pub mod store;
pub mod types;
pub mod util;
pub mod wake;
pub mod writing;

#[cfg(test)]
pub mod testing;
