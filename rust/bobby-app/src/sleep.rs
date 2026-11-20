//! Deep Sleep: Whisper is unloaded and the LLM is stopped until the next "Bobby". Pure state machine; the host
//! executes the actions (see `host.rs`), the supervisor keeps the state (see `supervisor.rs`).
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

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
    /// Nothing to do for `[sleep] auto_after_min` minutes: sleep without a word.
    Idle,
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
        (Awake, SleepAsked | Idle) => (Asleep, vec![StopServices]),
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

/// When Bobby last had something to do (a sentence, music, speech). Times are seconds on any monotonic clock.
#[derive(Debug, Default)]
pub struct IdleClock {
    last: u64,
    /// When Bobby started speaking, None while silent.
    speaking: Option<u64>,
}

/// A "speaking" flag older than this is a lost end message, not a very long answer.
pub const SPEAKING_STALE_S: u64 = 120;

impl IdleClock {
    pub fn touch(&mut self, now: u64) {
        self.last = now;
    }
    pub fn speaking(&mut self, on: bool, now: u64) {
        self.speaking = on.then_some(now);
        self.last = now;
    }
    fn is_speaking(&self, now: u64) -> bool {
        self.speaking.is_some_and(|t| now.saturating_sub(t) < SPEAKING_STALE_S)
    }
    /// Due once `minutes` full minutes passed without activity; 0 means never; never while Bobby speaks.
    pub fn due(&self, now: u64, minutes: u32) -> bool {
        minutes > 0 && !self.is_speaking(now) && now.saturating_sub(self.last) >= u64::from(minutes) * 60
    }
    /// One tick of the minute ticker: `active` (the brain is busy or music plays) counts as activity.
    pub fn tick(&mut self, now: u64, minutes: u32, active: bool) -> bool {
        if active {
            self.touch(now);
        }
        self.due(now, minutes)
    }
}

/// The ticker only acts when Bobby runs in all-Rust mode: the supervisor is running and the host's brain is up.
pub fn may_tick(supervisor_running: bool, rust_side_up: bool) -> bool {
    supervisor_running && rust_side_up
}

static CLOCK: Mutex<Option<(Instant, IdleClock)>> = Mutex::new(None);

fn with_clock<T>(f: impl FnOnce(u64, &mut IdleClock) -> T) -> T {
    let mut g = CLOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (t0, c) = g.get_or_insert_with(|| (Instant::now(), IdleClock::default()));
    f(t0.elapsed().as_secs(), c)
}

/// Marks activity now (a sentence reached the brain, Bobby woke).
pub fn touch() {
    with_clock(|n, c| c.touch(n));
}

pub fn set_speaking(on: bool) {
    with_clock(|n, c| c.speaking(on, n));
}

/// True when Bobby should go to sleep by himself now.
pub fn idle_tick(minutes: u32, active: bool) -> bool {
    with_clock(|n, c| c.tick(n, minutes, active))
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
    fn idle_while_awake_sleeps_silently() {
        let (st, acts) = on(Awake, Idle);
        assert_eq!((st, &acts), (Asleep, &vec![StopServices]));
        assert!(!acts.iter().any(|a| matches!(a, PlayWake | SayReady | SayFailed)));
    }

    #[test]
    fn idle_while_asleep_or_waking_does_nothing() {
        assert_eq!(on(Asleep, Idle), (Asleep, vec![]));
        assert_eq!(on(Waking, Idle), (Waking, vec![]));
    }

    #[test]
    fn idle_clock_due_exactly_after_n_minutes() {
        let mut c = IdleClock::default();
        c.touch(100);
        assert!(!c.due(100 + 5 * 60 - 1, 5));
        assert!(c.due(100 + 5 * 60, 5));
    }

    #[test]
    fn idle_clock_activity_resets_it() {
        let mut c = IdleClock::default();
        c.touch(0);
        c.touch(250);
        assert!(!c.due(300, 5));
        assert!(c.due(550, 5));
    }

    #[test]
    fn idle_clock_zero_means_never() {
        let c = IdleClock::default();
        assert!(!c.due(1_000_000, 0));
    }

    #[test]
    fn no_auto_sleep_while_speaking() {
        let mut c = IdleClock::default();
        c.speaking(true, 0);
        assert!(!c.due(100, 1)); // a long answer
        c.speaking(false, 100);
        assert!(!c.due(159, 1));
        assert!(c.due(160, 1));
    }

    #[test]
    fn a_lost_speaking_end_goes_stale_after_two_minutes() {
        let mut c = IdleClock::default();
        c.speaking(true, 0);
        assert!(!c.due(SPEAKING_STALE_S - 1, 1));
        assert!(c.due(SPEAKING_STALE_S, 1), "stuck flag must not block sleep forever");
    }

    #[test]
    fn busy_or_music_counts_as_activity_on_every_tick() {
        let mut c = IdleClock::default();
        for t in (0..=3600).step_by(60) {
            assert!(!c.tick(t, 5, true));
        }
        assert!(!c.tick(3600 + 299, 5, false));
        assert!(c.tick(3600 + 300, 5, false));
    }

    #[test]
    fn ticking_needs_the_all_rust_bobby_up() {
        assert!(may_tick(true, true));
        assert!(!may_tick(false, true)); // stopped
        assert!(!may_tick(true, false)); // Node side or brain not up
    }

    #[test]
    fn failure_path_ends_awake() {
        assert_eq!(on(Waking, Failed), (Awake, vec![SayFailed]));
        assert_eq!(on(Asleep, Failed), (Asleep, vec![]));
        assert_eq!(on(Awake, Ready), (Awake, vec![]));
    }
}
