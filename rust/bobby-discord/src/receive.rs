//! Voice receive (stub, filled by a later task): sentence cutting and `Heard` messages.

use crate::client::Bot;
use std::sync::Arc;

/// Registers the receive handlers on a call. Must run BEFORE `join`: Discord sends each present user's
/// SSRC mapping once at connect, and a handler added later misses it.
pub fn attach(_call: &mut songbird::Call, _bot: Arc<Bot>) {}
