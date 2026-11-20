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
    /// The LLM is not healthy 180 s after the restart.
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    StopServices,
    /// The recorded "oui", at once (the host falls back to the beep if the clip is missing).
    PlayWake,
    StartServices,
    SayReady,
    SayFailed,
}

pub const READY: &str = "Je suis là.";
pub const FAILED: &str = "Je n'arrive pas à me réveiller complètement.";

/// ponytail: the sentence said together with the waking "Bobby" is dropped (Whisper is gone when it is heard); the user repeats it.
/// Keeping it would mean buffering the PCM across the Whisper reload (and replaying it): not worth it for now.
pub fn on(state: State, event: Event) -> (State, Vec<Action>) {
    use {Action::*, Event::*, State::*};
    match (state, event) {
        (Awake, SleepAsked) => (Asleep, vec![StopServices]),
        (Asleep, WakeWord) => (Waking, vec![PlayWake, StartServices]),
        (Waking, Ready) => (Awake, vec![SayReady]),
        (Waking, Failed) => (Awake, vec![SayFailed]),
        (s, _) => (s, vec![]),
    }
}

/// Whether a heard sentence may reach the brain. While Bobby is not fully awake (Waking: Whisper is back, the LLM is
/// not healthy yet) the LLM path would only answer "Mon cerveau ne répond pas.", so the sentence is dropped, unless it
/// answers a waiting messenger (that flow needs no LLM).
pub fn accept_sentence(state: State, answers_messenger: bool) -> bool {
    state == State::Awake || answers_messenger
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
        assert_eq!(on(Asleep, WakeWord), (Waking, vec![PlayWake, StartServices]));
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
    fn sentences_are_dropped_unless_awake_or_answering_a_messenger() {
        assert!(accept_sentence(Awake, false));
        assert!(!accept_sentence(Waking, false));
        assert!(!accept_sentence(Asleep, false));
        assert!(accept_sentence(Waking, true));
        assert!(accept_sentence(Asleep, true));
    }

    #[test]
    fn failure_path_ends_awake() {
        assert_eq!(on(Waking, Failed), (Awake, vec![SayFailed]));
        assert_eq!(on(Asleep, Failed), (Asleep, vec![]));
        assert_eq!(on(Awake, Ready), (Awake, vec![]));
    }
}
