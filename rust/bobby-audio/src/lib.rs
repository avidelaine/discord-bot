//! PC microphone, wake word, speakers.
pub mod gate;
pub mod mic;
pub mod out;
// The wake word and clip collection moved to bobby-infer (portable: the Discord path needs them too).
pub use bobby_infer::{collect, wake};
