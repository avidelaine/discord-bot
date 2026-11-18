//! Voice receive: cuts each speaker's decoded voice into sentences, streams every sentence to the voice
//! service (`/listen`, same protocol as `bot/bot.js` `listen()`), and sends the transcript to the brain.
//!
//! Layers, so most of it is testable without Discord: `Cutter` (pure sentence cutting), `exchange` (one
//! WebSocket conversation), `Watchdog` (pure DAVE-drop detection), and `attach` (the songbird glue).
//!
//! A future tracing subscriber must filter the `songbird::driver::tasks::udp_rx` RTCP warnings
//! ("RTCP decryption failed", about 3 every 4 s, harmless): the crate has none today, so they are not printed.

use crate::{caps, client::Bot};
use bobby_proto::msg::ClientMsg;
use futures_util::{SinkExt, StreamExt};
use serenity::all::UserId;
use songbird::events::{CoreEvent, Event as SbEvent, EventContext, EventHandler as VoiceHandler};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio_tungstenite::tungstenite::Message;

/// What the brain's messages set: who is heard without a wake word, and who Bobby is answering.
#[derive(Default)]
pub struct Listen {
    expect: HashMap<UserId, Instant>,
    /// Set by the Say task: while Bobby answers this person, their sentences skip the wake word gate
    /// (they may say "stop").
    pub speaking_to: Option<UserId>,
}

impl Listen {
    pub fn expect(&mut self, user: UserId, ms: u32) {
        self.expect.insert(user, Instant::now() + Duration::from_millis(ms.into()));
    }

    /// Judged when a sentence STARTS: a long order must not outlive the window (as in `bot.js`).
    pub fn expecting(&self, user: UserId) -> bool {
        self.expect.get(&user).is_some_and(|t| Instant::now() < *t)
    }
}

// ---------- sentence cutter ----------

#[derive(Debug, PartialEq)]
pub enum Event {
    Open(UserId),
    Pcm(UserId, Vec<i16>),
    /// The sentence ended; the `Instant` is when the last voice was heard (the real end of speech).
    Close(UserId, Instant),
}

/// Per user: opens on the first decoded voice, closes after `silence` without any.
pub struct Cutter {
    silence: Duration,
    last_voice: HashMap<UserId, Instant>,
}

impl Cutter {
    pub fn new(silence: Duration) -> Self {
        Self { silence, last_voice: HashMap::new() }
    }

    pub fn voice(&mut self, user: UserId, pcm: Vec<i16>, now: Instant) -> Vec<Event> {
        let mut out = Vec::new();
        if self.last_voice.insert(user, now).is_none() {
            out.push(Event::Open(user));
        }
        out.push(Event::Pcm(user, pcm));
        out
    }

    /// Call every ~100 ms: closes the sentences whose silence is long enough.
    pub fn tick(&mut self, now: Instant) -> Vec<Event> {
        let silence = self.silence;
        let mut out = Vec::new();
        self.last_voice.retain(|user, last| {
            let done = now.duration_since(*last) >= silence;
            if done {
                out.push(Event::Close(*user, *last));
            }
            !done
        });
        out
    }
}

// ---------- watchdog ----------

const DROP_AFTER: Duration = Duration::from_secs(2);
const WARN_EVERY: Duration = Duration::from_secs(60);

/// Detects packets songbird 0.6.1 drops silently.
///
/// In `udp_rx`, an undecryptable DAVE packet (unknown SSRC, session not ready, no key) is a bare `return`:
/// no event, no log. `RtpPacket` fires only AFTER a successful decrypt, so a drop is invisible as such.
/// What we CAN see: (a) Discord says an SSRC is speaking (`SpeakingStateUpdate`, flags not empty) but no
/// `RtpPacket` follows within 2 s; (b) `RtpPacket`s from an SSRC we cannot map to a user (clear Opus
/// silence frames pass, so this shows an SSRC whose `Speaking` we missed). What we CANNOT see: a drop
/// when Discord does not resend `Speaking` for a user already marked speaking, or partial loss.
#[derive(Default)]
pub struct Watchdog {
    /// SSRC -> (user, since): announced as speaking, no packet yet.
    announced: HashMap<u32, (UserId, Instant)>,
    /// SSRC -> since: packets arriving from an SSRC with no user.
    unmapped: HashMap<u32, Instant>,
    warned: HashMap<String, Instant>,
}

impl Watchdog {
    pub fn speaking(&mut self, ssrc: u32, user: UserId, now: Instant) {
        self.unmapped.remove(&ssrc);
        self.announced.entry(ssrc).or_insert((user, now));
    }

    pub fn packet(&mut self, ssrc: u32, mapped: bool, now: Instant) {
        self.announced.remove(&ssrc);
        if mapped {
            self.unmapped.remove(&ssrc);
        } else {
            self.unmapped.entry(ssrc).or_insert(now);
        }
    }

    /// Warnings due now, at most one per user (or SSRC) per minute.
    pub fn check(&mut self, now: Instant) -> Vec<String> {
        let mut due: Vec<(String, String)> = Vec::new();
        self.announced.retain(|_, (user, since)| {
            let late = now.duration_since(*since) >= DROP_AFTER;
            if late {
                due.push((user.to_string(), format!("<@{user}> was announced as speaking but no packet reached us for 2 s")));
            }
            !late
        });
        self.unmapped.retain(|ssrc, since| {
            let late = now.duration_since(*since) >= DROP_AFTER;
            if late {
                due.push((format!("ssrc {ssrc}"), format!("ssrc {ssrc} sends packets but its user is unknown")));
            }
            !late
        });
        let mut out = Vec::new();
        for (key, why) in due {
            if self.warned.get(&key).is_none_or(|t| now.duration_since(*t) >= WARN_EVERY) {
                self.warned.insert(key.clone(), now);
                out.push(format!("[voice] packets from {key} are being dropped (DAVE): {why}"));
            }
        }
        out
    }
}

// ---------- /listen exchange ----------

/// The gate message of the voice service: `wake` is `None` when the gate was bypassed (`force=1`).
#[derive(Debug, Clone, PartialEq)]
pub struct Gate {
    pub wake: Option<bool>,
    pub score: Option<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Transcript {
    pub text: String,
    pub lang: String,
    pub ms: u32,
}

#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    pub gate: Option<Gate>,
    /// `None` when the gate said this was not for Bobby.
    pub transcript: Option<Transcript>,
}

/// `http://host:port` -> `ws://host:port/listen?force=..&hotwords=..`.
pub fn listen_url(voice_url: &str, force: bool, hotwords: &str) -> String {
    let base = voice_url.trim_end_matches('/');
    let base = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    };
    format!("{base}/listen?force={}&hotwords={}", u8::from(force), percent_encode(hotwords))
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// One sentence over one WebSocket. `pcm` is the decoded sentence (48 kHz stereo); chunks queue in the
/// channel while the socket opens, and the channel closing means the sentence ended (sends 'end').
/// `ring(true)` on `wake`, `ring(false)` on `gate`.
pub async fn exchange(url: &str, mut pcm: UnboundedReceiver<Vec<i16>>, ring: impl Fn(bool)) -> Result<Outcome, String> {
    let (ws, _) = tokio_tungstenite::connect_async(url).await.map_err(|e| format!("connect: {e}"))?;
    let (mut tx, mut rx) = ws.split();
    let mut outcome = Outcome::default();
    let mut ended = false;
    loop {
        tokio::select! {
            chunk = pcm.recv(), if !ended => match chunk {
                Some(samples) => {
                    // s16le, as voice/server.py reads it
                    let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
                    tx.send(Message::Binary(bytes.into())).await.map_err(|e| e.to_string())?;
                }
                None => {
                    ended = true;
                    tx.send(Message::Text("end".into())).await.map_err(|e| e.to_string())?;
                }
            },
            msg = rx.next() => match msg {
                Some(Ok(Message::Text(t))) => {
                    let Ok(m) = serde_json::from_str::<serde_json::Value>(&t) else { continue };
                    match m["type"].as_str() {
                        Some("wake") => ring(true),
                        Some("gate") => {
                            ring(false);
                            let wake = m["wake"].as_bool();
                            outcome.gate = Some(Gate { wake, score: m["score"].as_f64().map(|s| s as f32) });
                            if wake == Some(false) {
                                return Ok(outcome);
                            }
                        }
                        Some("text") => {
                            outcome.transcript = Some(Transcript {
                                text: m["text"].as_str().unwrap_or_default().to_string(),
                                lang: m["lang"].as_str().unwrap_or_default().to_string(),
                                ms: m["ms"].as_u64().unwrap_or(0) as u32,
                            });
                            return Ok(outcome);
                        }
                        _ => {}
                    }
                }
                Some(Ok(Message::Close(_))) | None if outcome.gate.is_some() => return Ok(outcome),
                Some(Ok(Message::Close(_))) | None => return Err("voice service closed the stream".into()),
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(e.to_string()),
            },
        }
    }
}

// ---------- songbird glue ----------

struct Shared {
    bot: Arc<Bot>,
    inner: Mutex<Inner>,
    next_utterance: AtomicU64,
}

struct Inner {
    cutter: Cutter,
    watchdog: Watchdog,
    ssrc_user: HashMap<u32, UserId>,
    /// Sentence in progress per user: dropping the sender ends the sentence.
    sentences: HashMap<UserId, UnboundedSender<Vec<i16>>>,
    /// Bot status per user, asked once.
    is_bot: HashMap<UserId, bool>,
}

#[derive(Clone)]
struct Rx(Arc<Shared>);

#[serenity::async_trait]
impl VoiceHandler for Rx {
    async fn act(&self, ctx: &EventContext<'_>) -> Option<SbEvent> {
        let now = Instant::now();
        let mut events = Vec::new();
        {
            let mut g = self.0.inner.lock().unwrap();
            match ctx {
                EventContext::SpeakingStateUpdate(s) => {
                    if let Some(u) = s.user_id.filter(|u| u.0 > 0) {
                        let user = UserId::new(u.0);
                        g.ssrc_user.insert(s.ssrc, user);
                        if !s.speaking.is_empty() {
                            g.watchdog.speaking(s.ssrc, user, now);
                        }
                    }
                }
                EventContext::RtpPacket(p) => {
                    let ssrc = p.rtp().get_ssrc();
                    let mapped = g.ssrc_user.contains_key(&ssrc);
                    g.watchdog.packet(ssrc, mapped, now);
                }
                EventContext::VoiceTick(t) => {
                    for (ssrc, data) in &t.speaking {
                        let user = g.ssrc_user.get(ssrc).copied();
                        if let (Some(user), Some(pcm)) = (user, &data.decoded_voice) {
                            events.extend(g.cutter.voice(user, pcm.clone(), now));
                        }
                    }
                }
                _ => {}
            }
        }
        self.0.dispatch(events);
        None
    }
}

impl Shared {
    /// Routes cutter events to the per-sentence tasks.
    fn dispatch(self: &Arc<Self>, events: Vec<Event>) {
        for ev in events {
            match ev {
                Event::Open(user) => {
                    let (tx, rx) = unbounded_channel();
                    self.inner.lock().unwrap().sentences.insert(user, tx);
                    // Judged now, at the start of the sentence.
                    let (expected, force) = {
                        let l = self.bot.listen.lock().unwrap();
                        let e = l.expecting(user);
                        (e, e || l.speaking_to == Some(user))
                    };
                    let me = self.clone();
                    tokio::spawn(async move { me.sentence(user, rx, expected, force).await });
                }
                Event::Pcm(user, pcm) => {
                    if let Some(tx) = self.inner.lock().unwrap().sentences.get(&user) {
                        let _ = tx.send(pcm);
                    }
                }
                // Dropping the sender is the 'end' of the sentence.
                Event::Close(user, _) => {
                    self.inner.lock().unwrap().sentences.remove(&user);
                }
            }
        }
    }

    /// Bots are skipped. Cache first, then one HTTP lookup, remembered.
    async fn is_bot(&self, user: UserId) -> bool {
        let known = self.inner.lock().unwrap().is_bot.get(&user).copied();
        if let Some(b) = known {
            return b;
        }
        let cached = self.bot.cache.guild(self.bot.guild).and_then(|g| g.members.get(&user).map(|m| m.user.bot));
        let bot = match cached {
            Some(b) => b,
            None => self.bot.http.get_user(user).await.map(|u| u.bot).unwrap_or(false),
        };
        self.inner.lock().unwrap().is_bot.insert(user, bot);
        bot
    }

    fn name(&self, user: UserId) -> String {
        self.bot
            .cache
            .guild(self.bot.guild)
            .and_then(|g| g.members.get(&user).map(|m| m.display_name().to_string()))
            .unwrap_or_else(|| user.to_string())
    }

    async fn sentence(self: Arc<Self>, user: UserId, pcm: UnboundedReceiver<Vec<i16>>, expected: bool, force: bool) {
        if self.is_bot(user).await {
            return;
        }
        let bot = &self.bot;
        bot.send_context(user);
        let hotwords = bot
            .cache
            .guild(bot.guild)
            .map(|g| crate::context::hotwords(&crate::context::from_guild(&g, user.get(), None)))
            .unwrap_or_else(|| "Bobby".into());
        let url = listen_url(&bot.cfg.voice_url, force, &hotwords);
        let b = bot.clone();
        let res = match tokio::time::timeout(Duration::from_secs(30), exchange(&url, pcm, move |on| caps::ring(&b, on, user))).await {
            Ok(r) => r,
            Err(_) => Err("voice service timed out (30 s)".to_string()),
        };
        caps::ring(bot, false, user);
        match res {
            Ok(Outcome { gate: Some(Gate { wake: Some(false), score }), .. }) => {
                let score = score.map_or("?".into(), |s| format!("{s:.2}"));
                eprintln!("[skipped] {}: no \"Bobby\" in the audio (score {score})", self.name(user));
            }
            Ok(Outcome { gate, transcript: Some(t) }) if !t.text.is_empty() => {
                eprintln!("[heard] {}: \"{}\" ({}, whisper {} ms)", self.name(user), t.text, t.lang, t.ms);
                bot.send(ClientMsg::Heard {
                    utterance: self.next_utterance.fetch_add(1, Ordering::Relaxed),
                    user: user.to_string(),
                    text: t.text,
                    lang: t.lang,
                    score: gate.and_then(|g| g.score),
                    stt_ms: t.ms,
                    expected,
                });
            }
            Ok(_) => {}
            Err(e) if e.starts_with("connect") => eprintln!("[voice] voice service not reachable at {} (is it running?)", bot.cfg.voice_url),
            Err(e) => eprintln!("[voice] {e}"),
        }
    }
}

/// Registers the receive handlers on a call. Must run BEFORE `join`: Discord sends each present user's
/// SSRC mapping once at connect, and a handler added later misses it.
pub fn attach(call: &mut songbird::Call, bot: Arc<Bot>) {
    let silence = Duration::from_millis(bot.cfg.silence_ms);
    let shared = Arc::new(Shared {
        bot,
        inner: Mutex::new(Inner {
            cutter: Cutter::new(silence),
            watchdog: Watchdog::default(),
            ssrc_user: HashMap::new(),
            sentences: HashMap::new(),
            is_bot: HashMap::new(),
        }),
        next_utterance: AtomicU64::new(1),
    });
    let rx = Rx(shared.clone());
    for ev in [CoreEvent::SpeakingStateUpdate, CoreEvent::VoiceTick, CoreEvent::RtpPacket] {
        call.add_global_event(ev.into(), rx.clone());
    }
    // The ticker only holds a Weak: when the call is dropped (leave, or a fresh join) the handlers go,
    // `Shared` is freed and the ticker stops.
    let weak: Weak<Shared> = Arc::downgrade(&shared);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        loop {
            tick.tick().await;
            let Some(s) = weak.upgrade() else { break };
            let (events, warnings) = {
                let mut g = s.inner.lock().unwrap();
                let now = Instant::now();
                (g.cutter.tick(now), g.watchdog.check(now))
            };
            for w in warnings {
                eprintln!("{w}");
            }
            s.dispatch(events);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    fn u(n: u64) -> UserId {
        UserId::new(n)
    }

    #[test]
    fn cutter_cuts_two_users_independently() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut c = Cutter::new(Duration::from_millis(600));
        assert_eq!(c.voice(u(1), vec![1], ms(0)), vec![Event::Open(u(1)), Event::Pcm(u(1), vec![1])]);
        assert_eq!(c.voice(u(2), vec![2], ms(20)), vec![Event::Open(u(2)), Event::Pcm(u(2), vec![2])]);
        assert_eq!(c.voice(u(1), vec![3], ms(40)), vec![Event::Pcm(u(1), vec![3])]);
        // user 2 spoke last at 20 ms, user 1 at 40 ms: each is cut 600 ms after its own last voice
        assert!(c.tick(ms(619)).is_empty());
        assert_eq!(c.tick(ms(620)), vec![Event::Close(u(2), ms(20))]);
        assert_eq!(c.tick(ms(640)), vec![Event::Close(u(1), ms(40))]);
        assert!(c.tick(ms(2000)).is_empty());
        // a new voice opens a new sentence
        assert_eq!(c.voice(u(1), vec![4], ms(2100))[0], Event::Open(u(1)));
    }

    #[test]
    fn cutter_keeps_a_500ms_gap() {
        let t0 = Instant::now();
        let mut c = Cutter::new(Duration::from_millis(600));
        c.voice(u(1), vec![1], t0);
        assert!(c.tick(t0 + Duration::from_millis(500)).is_empty());
        let ev = c.voice(u(1), vec![2], t0 + Duration::from_millis(500));
        assert_eq!(ev, vec![Event::Pcm(u(1), vec![2])]);
        assert!(c.tick(t0 + Duration::from_millis(1000)).is_empty());
    }

    #[test]
    fn expect_window() {
        let mut l = Listen::default();
        assert!(!l.expecting(u(1)));
        l.expect(u(1), 5000);
        assert!(l.expecting(u(1)) && !l.expecting(u(2)));
        l.expect(u(1), 0);
        assert!(!l.expecting(u(1)));
    }

    #[test]
    fn watchdog_warns_once_a_minute() {
        let t0 = Instant::now();
        let mut w = Watchdog::default();
        w.speaking(7, u(1), t0);
        assert!(w.check(t0 + Duration::from_secs(1)).is_empty());
        let out = w.check(t0 + Duration::from_secs(3));
        assert_eq!(out.len(), 1);
        assert!(out[0].starts_with("[voice] packets from 1 are being dropped (DAVE)"));
        w.speaking(7, u(1), t0 + Duration::from_secs(10));
        assert!(w.check(t0 + Duration::from_secs(20)).is_empty()); // rate limited
        // a packet in time clears the suspicion
        w.speaking(8, u(2), t0);
        w.packet(8, true, t0 + Duration::from_secs(1));
        assert!(w.check(t0 + Duration::from_secs(5)).is_empty());
    }

    #[test]
    fn url_is_ws_and_encoded() {
        assert_eq!(listen_url("http://127.0.0.1:3000/", true, "Bobby Zoé"), "ws://127.0.0.1:3000/listen?force=1&hotwords=Bobby%20Zo%C3%A9");
        assert!(listen_url("https://v.example", false, "").starts_with("wss://v.example/listen?force=0"));
    }

    #[tokio::test]
    async fn exchange_with_fake_service() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (s, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(s).await.unwrap();
            let mut bytes = 0;
            loop {
                match ws.next().await.unwrap().unwrap() {
                    Message::Binary(b) => bytes += b.len(),
                    Message::Text(t) => {
                        assert_eq!(t.as_str(), "end");
                        break;
                    }
                    _ => {}
                }
            }
            for m in [
                r#"{"type":"wake","score":0.7}"#,
                r#"{"type":"gate","wake":true,"score":0.8,"seconds":1.0,"ms":3}"#,
                r#"{"type":"text","text":"coupe le micro","lang":"fr","ms":120}"#,
            ] {
                ws.send(Message::Text(m.into())).await.unwrap();
            }
            bytes
        });
        let (tx, rx) = unbounded_channel();
        // queued before the socket is open, then the channel closes: the sentence ended
        tx.send(vec![1i16; 960]).unwrap();
        tx.send(vec![2i16; 960]).unwrap();
        drop(tx);
        let rings = Mutex::new(Vec::new());
        let url = format!("ws://127.0.0.1:{port}/listen?force=0&hotwords=");
        let out = exchange(&url, rx, |on| rings.lock().unwrap().push(on)).await.unwrap();
        assert_eq!(server.await.unwrap(), 2 * 960 * 2);
        assert_eq!(*rings.lock().unwrap(), vec![true, false]);
        assert_eq!(out.gate, Some(Gate { wake: Some(true), score: Some(0.8) }));
        assert_eq!(out.transcript, Some(Transcript { text: "coupe le micro".into(), lang: "fr".into(), ms: 120 }));
    }

    #[tokio::test]
    async fn exchange_stops_on_gate_miss_and_reports_unreachable() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (s, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(s).await.unwrap();
            while let Some(Ok(Message::Binary(_))) = ws.next().await {}
            ws.send(Message::Text(r#"{"type":"gate","wake":false,"score":0.1}"#.into())).await.unwrap();
        });
        let (tx, rx) = unbounded_channel();
        tx.send(vec![0i16; 10]).unwrap();
        drop(tx);
        let out = exchange(&format!("ws://127.0.0.1:{port}/listen"), rx, |_| {}).await.unwrap();
        assert_eq!(out.gate.unwrap().wake, Some(false));
        assert!(out.transcript.is_none());

        let (_tx, rx) = unbounded_channel();
        let err = exchange("ws://127.0.0.1:1/listen", rx, |_| {}).await.unwrap_err();
        assert!(err.starts_with("connect"));
    }
}
