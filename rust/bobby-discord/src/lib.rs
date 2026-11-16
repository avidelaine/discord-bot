//! Discord client of Bobby: connection, voice receive, music, and the execution of the brain's caps.
//! Port of `bot/bot.js`, `bot/music/*`, `bot/actions/discord.js` (Discord side) and `bot/format.js`.
//!
//! Talks to the brain only through `bobby_proto::msg` (`BrainMsg` in, `ClientMsg` out).

pub mod pcm;
pub mod format;
pub mod mixer;
pub mod sources;
pub mod player;
pub mod config;
pub mod context;
pub mod client;
pub mod receive;
pub mod caps;
pub mod alone;
