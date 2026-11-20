//! Bobby's brain: shortcuts, context, rights, actions, storage. Port of `bot/brain.js` and friends.
//! It touches neither Discord nor hardware: every side effect is a `BrainMsg` to a client (see `io`).
pub mod actions;
pub mod brain;
pub mod context;
pub mod format;
pub mod guard;
pub mod io;
pub mod journal;
pub mod messenger;
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
#[cfg(test)]
mod tests_batch2;
#[cfg(test)]
mod tests_batch34;
#[cfg(test)]
mod tests_r4;
#[cfg(test)]
mod tests_reminders;
#[cfg(test)]
mod tests_reminders2;
