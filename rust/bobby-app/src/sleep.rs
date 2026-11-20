//! Deep Sleep: Whisper is unloaded and the LLM is stopped until the next "Bobby". Pure state machine; the host
//! executes the actions (see `host.rs`), the supervisor keeps the state (see `supervisor.rs`).
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum State {
    #[default]
    Awake,
    Asleep,
    Waking,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            State::Awake => "awake",
            State::Asleep => "asleep",
            State::Waking => "waking",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    SleepAsked,
    WakeWord,
    /// The services are back (the LLM answers `/health`).
    Ready,
    /// The LLM is not healthy 60 s after the restart.
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    StopServices,
    SayWaking,
    StartServices,
    SayReady,
    SayFailed,
}

pub const WAKING: &str = "Je me réveille.";
pub const READY: &str = "Je suis là, qu'est-ce qu'il y a ?";
pub const FAILED: &str = "Je n'arrive pas à me réveiller complètement.";

/// ponytail: the sentence spoken together with the waking "Bobby" is dropped (Whisper is gone when it is heard); the user repeats it.
pub fn on(state: State, event: Event) -> (State, Vec<Action>) {
    use {Action::*, Event::*, State::*};
    match (state, event) {
        (Awake, SleepAsked) => (Asleep, vec![StopServices]),
        (Asleep, WakeWord) => (Waking, vec![SayWaking, StartServices]),
        (Waking, Ready) => (Awake, vec![SayReady]),
        (Waking, Failed) => (Awake, vec![SayFailed]),
        (s, _) => (s, vec![]),
    }
}

static HOOK: OnceLock<Box<dyn Fn(Event) + Send + Sync>> = OnceLock::new();

/// Installed once by the host; every source (brain, wake word, tray, API) goes through `fire`.
pub fn set_hook(f: impl Fn(Event) + Send + Sync + 'static) {
    let _ = HOOK.set(Box::new(f));
}

pub fn fire(e: Event) {
    if let Some(f) = HOOK.get() {
        f(e);
    }
}

#[cfg(test)]
mod tests {
    use super::{Action::*, Event::*, State::*, *};

    #[test]
    fn awake_asleep_waking_awake() {
        assert_eq!(on(Awake, SleepAsked), (Asleep, vec![StopServices]));
        assert_eq!(on(Asleep, WakeWord), (Waking, vec![SayWaking, StartServices]));
        assert_eq!(on(Waking, Ready), (Awake, vec![SayReady]));
    }

    #[test]
    fn wake_word_while_waking_or_awake_does_nothing() {
        assert_eq!(on(Waking, WakeWord), (Waking, vec![]));
        assert_eq!(on(Awake, WakeWord), (Awake, vec![]));
    }

    #[test]
    fn second_sleep_does_nothing() {
        assert_eq!(on(Asleep, SleepAsked), (Asleep, vec![]));
        assert_eq!(on(Waking, SleepAsked), (Waking, vec![]));
    }

    #[test]
    fn failure_path_ends_awake() {
        assert_eq!(on(Waking, Failed), (Awake, vec![SayFailed]));
        assert_eq!(on(Asleep, Failed), (Asleep, vec![]));
        assert_eq!(on(Awake, Ready), (Awake, vec![]));
    }
}
