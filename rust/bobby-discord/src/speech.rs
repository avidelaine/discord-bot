//! Bobby's voice and signals: beep, nope, spoken acks, the speaking ring and TTS replies.
//! Port of the beep/nope/ack/ring/say/hush part of `makeIo` in `bot/bot.js`.
//!
//! The state is plain data in `Speech` (behind a std Mutex in `Bot`); the mixer is locked briefly, never
//! across an `.await`. Lock order is always `Speech` then `Mixer`. The `*_core` functions take the pieces
//! they need instead of `Bot`, so tests can drive them with a bare `Mixer`.

use crate::client::Bot;
use crate::mixer::Mixer;
use crate::pcm::{pipe, PcmRx, FRAME};
use crate::receive::Listen;
use crate::sources::decode_wav;
use bobby_proto::msg::{ClientMsg, SignalKind, Voice};
use serenity::all::UserId;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;

/// Spoken acks, synthesized once by the voice service (retried while it was not up yet).
pub const ACKS: [&str; 5] = ["D'accord.", "Ça marche.", "Je m'en occupe.", "C'est parti.", "Entendu."];

/// 0.4 s of silence after a spoken reply (stereo samples): Discord cuts the end of a stream, this keeps
/// it off the last syllable.
const TAIL: usize = 48_000 * 2 * 4 / 10;

fn round(x: f64) -> i16 {
    (x + 0.5).floor() as i16 // Math.round: halves go up
}

/// 120 ms beep at 880 Hz, 48 kHz stereo, with a short fade-out so it does not click.
pub fn beep_pcm() -> Vec<i16> {
    let n = 5760usize;
    let mut v = Vec::with_capacity(n * 2);
    for i in 0..n {
        let s = round((2.0 * std::f64::consts::PI * 880.0 * i as f64 / 48000.0).sin() * 6000.0 * ((n - i) as f64 / 1500.0).min(1.0));
        v.extend([s, s]);
    }
    v
}

/// "Heard Bobby, did not get it": two soft descending tones (330 then 220 Hz), 250 ms, raised-cosine per tone.
pub fn nope_pcm() -> Vec<i16> {
    let seg = 6000usize;
    let mut v = Vec::with_capacity(seg * 4);
    for hz in [330.0, 220.0] {
        for i in 0..seg {
            let env = (std::f64::consts::PI * i as f64 / seg as f64).sin().powi(2);
            let s = round((2.0 * std::f64::consts::PI * hz * i as f64 / 48000.0).sin() * 4500.0 * env);
            v.extend([s, s]);
        }
    }
    v
}

/// Tiny xorshift: noise and ack choice need no quality, and this avoids a dependency.
fn rnd() -> u64 {
    static S: AtomicU64 = AtomicU64::new(0);
    let mut x = S.load(Ordering::Relaxed);
    if x == 0 {
        x = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64) | 1;
    }
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    S.store(x, Ordering::Relaxed);
    x
}

struct Ring {
    owner: Option<UserId>,
}

#[derive(Default)]
pub struct Speech {
    /// Like `speechGen`: every Say takes the next number; `current` is the one whose voice is playing now.
    gen: u64,
    current: Option<u64>,
    ring: Option<Ring>,
    /// Resolves to true when the last clip ended: a reply that follows waits for it.
    clip: Option<watch::Receiver<bool>>,
    acks: HashMap<&'static str, Arc<Vec<i16>>>,
    last_ack: Option<&'static str>,
    acks_loading: bool,
    /// A `Speaking { user: Some }` went out and its `None` has not.
    told: bool,
}

type MixerRef = Arc<Mutex<Mixer>>;

/// Blocking: drains a decoder's pipe (it fills from its own thread, an empty read is only an underrun).
fn collect(mut rx: PcmRx) -> Vec<i16> {
    let (mut out, mut f) = (Vec::new(), [0i16; FRAME]);
    loop {
        let n = rx.take(&mut f);
        out.extend_from_slice(&f[..n]);
        if n == 0 {
            if rx.ended() {
                return out;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

// ---------------------------------------------------------------------------------------------------------
// Loudness: every clip reaches the same level on Discord and on the PC.

use bobby_infer::loudness::normalize;

/// `[audio] voice_volume_discord` as a gain, read at each clip so a change applies to the next one.
fn voice_gain() -> f32 {
    bobby_proto::settings::load().audio.voice_volume_discord as f32 / 100.0
}

/// Generated PCM (beep, nope) through the same normalisation as the TTS: wrapped as a 48 kHz stereo WAV.
fn loud(pcm: Vec<i16>) -> Vec<i16> {
    let data: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut wav = Vec::with_capacity(44 + data.len());
    let u32le = |v: u32| v.to_le_bytes();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&u32le(36 + data.len() as u32));
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&u32le(16));
    wav.extend_from_slice(&[1, 0, 2, 0]); // PCM, stereo
    wav.extend_from_slice(&u32le(48_000));
    wav.extend_from_slice(&u32le(48_000 * 4));
    wav.extend_from_slice(&[4, 0, 16, 0]); // block align, bits
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&u32le(data.len() as u32));
    wav.extend_from_slice(&data);
    collect(decode_wav(normalize(&wav)))
}

/// The in-process Piper engine (`VOICE_URL=local`), loaded once by `client::run`.
pub static LOCAL_TTS: std::sync::OnceLock<bobby_infer::Tts> = std::sync::OnceLock::new();

async fn fetch_tts(voice_url: &str, lang: &str, voice: &str, text: &str) -> Result<Vec<i16>, String> {
    let t0 = std::time::Instant::now();
    if bobby_infer::is_local(voice_url) {
        let tts = LOCAL_TTS.get().ok_or("local tts not loaded")?;
        let wav = tokio::time::timeout(Duration::from_secs(20), bobby_proto::infer::Speak::speak(tts, text, lang, voice))
            .await
            .map_err(|_| "local tts timed out".to_string())?
            .map_err(|e| e.to_string())?;
        let first = t0.elapsed().as_millis();
        let out = tokio::task::spawn_blocking(move || collect(decode_wav(normalize(&wav.0)))).await.map_err(|e| e.to_string());
        println!("[timing] tts first byte {first} ms, total {} ms", t0.elapsed().as_millis());
        return out;
    }
    let client = reqwest::Client::builder().timeout(Duration::from_secs(20)).build().map_err(|e| e.to_string())?;
    let res = client
        .get(format!("{}/tts", voice_url.trim_end_matches('/')))
        .query(&[("lang", lang), ("voice", voice), ("text", text)])
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(format!("tts {}", res.status()));
    }
    let first = t0.elapsed().as_millis();
    let bytes = res.bytes().await.map_err(|e| e.to_string())?;
    let out = tokio::task::spawn_blocking(move || collect(decode_wav(normalize(&bytes)))).await.map_err(|e| e.to_string());
    println!("[timing] tts first byte {first} ms, total {} ms", t0.elapsed().as_millis());
    out
}

// ---------------------------------------------------------------------------------------------------------
// Ring

/// An endless +-2 noise source (Discord only shows the ring while audio flows, zeros would not do).
/// The thread ends when the receiver is dropped: the send fails.
fn noise_source() -> (std::thread::JoinHandle<()>, PcmRx) {
    let (tx, rx) = pipe();
    let h = std::thread::spawn(move || loop {
        let chunk: Vec<i16> = (0..FRAME).map(|_| (rnd() % 5) as i16 - 2).collect();
        if tx.send(chunk).is_err() {
            return;
        }
    });
    (h, rx)
}

fn ring_stop_core(mixer: &MixerRef, st: &Mutex<Speech>, owner: Option<UserId>) {
    let mut s = st.lock().unwrap();
    let Some(r) = &s.ring else { return };
    // owner given: only that speaker's ring stops (another speaker's end of sentence leaves it on)
    if owner.is_some() && r.owner != owner {
        return;
    }
    s.ring = None;
    // The ring is the only overlay that can be running while `ring` is set (clip/say stop it first).
    // Dropping the overlay's receiver also ends the producer thread.
    mixer.lock().unwrap().stop_overlay();
}

/// One ring for everyone (one overlay channel); never starts while Bobby speaks or another overlay plays.
fn ring_core(mixer: &MixerRef, st: &Mutex<Speech>, on: bool, owner: Option<UserId>) {
    if !on {
        return ring_stop_core(mixer, st, owner);
    }
    let mut s = st.lock().unwrap();
    let mut m = mixer.lock().unwrap();
    if s.ring.is_some() || s.current.is_some() || m.has_overlay() {
        return;
    }
    let (_, rx) = noise_source();
    s.ring = Some(Ring { owner });
    drop(m.overlay(rx));
}

// ---------------------------------------------------------------------------------------------------------
// Clips and signals

/// Short clip (ack, beep, nope): stops the ring, plays as overlay; a reply that follows waits for its end.
fn clip_core(mixer: &MixerRef, st: &Mutex<Speech>, pcm: Vec<i16>, beep: bool) {
    ring_stop_core(mixer, st, None);
    let done = {
        let mut m = mixer.lock().unwrap();
        m.voice_gain = voice_gain(); // read at each clip: a volume change applies to the next one
        if beep { m.overlay_beep(PcmRx::from_vec(pcm)) } else { m.overlay(PcmRx::from_vec(pcm)) }
    };
    let (tx, rx) = watch::channel(false);
    st.lock().unwrap().clip = Some(rx);
    tokio::spawn(async move {
        let _ = done.await;
        let _ = tx.send(true);
    });
}

/// Spoken "got it". False while the clips are not synthesized yet (the caller falls back to the beep).
fn ack_core(mixer: &MixerRef, st: &Mutex<Speech>) -> bool {
    let pcm = {
        let mut s = st.lock().unwrap();
        let pool: Vec<&'static str> = ACKS.iter().copied().filter(|t| Some(*t) != s.last_ack && s.acks.contains_key(t)).collect();
        if pool.is_empty() {
            return false;
        }
        let t = pool[rnd() as usize % pool.len()];
        s.last_ack = Some(t);
        s.acks[t].clone()
    };
    clip_core(mixer, st, pcm.to_vec(), false);
    true
}

fn signal_core(mixer: &MixerRef, st: &Mutex<Speech>, kind: SignalKind, speaker: Option<UserId>) {
    match kind {
        SignalKind::Ack => {
            if !ack_core(mixer, st) {
                clip_core(mixer, st, loud(beep_pcm()), true);
            }
        }
        SignalKind::Beep => clip_core(mixer, st, loud(beep_pcm()), true),
        SignalKind::Nope => clip_core(mixer, st, loud(nope_pcm()), false),
        SignalKind::Listening => ring_core(mixer, st, true, speaker),
    }
}

/// Fetches the acks still missing; true when all are cached.
async fn load_acks_once(voice_url: &str, st: &Mutex<Speech>) -> bool {
    for t in ACKS {
        if st.lock().unwrap().acks.contains_key(t) {
            continue;
        }
        match fetch_tts(voice_url, "fr", "bobby", t).await {
            Ok(pcm) if !pcm.is_empty() => {
                st.lock().unwrap().acks.insert(t, Arc::new(pcm));
            }
            _ => return false, // service not up yet: try again later
        }
    }
    true
}

async fn load_acks_core(voice_url: &str, st: &Mutex<Speech>) {
    {
        let mut s = st.lock().unwrap();
        if s.acks_loading {
            return;
        }
        s.acks_loading = true;
    }
    while !load_acks_once(voice_url, st).await {
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
    st.lock().unwrap().acks_loading = false;
}

// ---------------------------------------------------------------------------------------------------------
// Say / hush

fn clear_speech(st: &Mutex<Speech>, listen: &Mutex<Listen>) {
    st.lock().unwrap().current = None;
    listen.lock().unwrap().speaking_to = None;
}

/// Tells the brain Bobby stopped speaking, only if it was told he started.
fn end_speaking(st: &Mutex<Speech>, speaking: &(dyn Fn(Option<String>) + Sync)) {
    let was = std::mem::replace(&mut st.lock().unwrap().told, false);
    if was {
        speaking(None);
    }
}

#[allow(clippy::too_many_arguments)]
async fn say_core(
    mixer: &MixerRef,
    st: &Mutex<Speech>,
    listen: &Mutex<Listen>,
    speaking: &(dyn Fn(Option<String>) + Sync),
    voice_url: &str,
    text: &str,
    lang: &str,
    voice: &str,
    to: Option<UserId>,
) {
    eprintln!("[{voice}] {text}");
    let me = {
        let mut s = st.lock().unwrap();
        s.gen += 1;
        s.current = Some(s.gen);
        s.gen
    };
    listen.lock().unwrap().speaking_to = to;
    let ours = || st.lock().unwrap().current == Some(me);
    let mut pcm = match fetch_tts(voice_url, lang, voice, text).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[tts] {e}");
            if ours() {
                clear_speech(st, listen);
                end_speaking(st, speaking); // a reply this one interrupted may have said Speaking(Some)
            }
            return;
        }
    };
    if !ours() {
        return; // interrupted, or another reply started, while it was synthesized
    }
    let clip = st.lock().unwrap().clip.clone();
    if let Some(mut c) = clip {
        // the ack or the low sound just played ends first (bounded: a clip nobody pulls must not hang the reply)
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), c.wait_for(|v| *v)).await;
    }
    if !ours() {
        return;
    }
    ring_stop_core(mixer, st, None);
    pcm.resize(pcm.len() + TAIL, 0);
    let secs = pcm.len() as u64 / (48_000 * 2) + 5;
    let done = {
        let mut m = mixer.lock().unwrap();
        m.voice_gain = voice_gain(); // read at each clip: a volume change applies to the next one
        m.overlay_reply(PcmRx::from_vec(pcm))
    };
    if let Some(u) = to {
        st.lock().unwrap().told = true;
        speaking(Some(u.to_string()));
    }
    // Clip length + 5 s: a mixer nobody pulls (not in voice) must not hold this reply forever.
    if tokio::time::timeout(Duration::from_secs(secs), done).await.is_err() && ours() {
        mixer.lock().unwrap().stop_overlay();
    }
    if ours() {
        clear_speech(st, listen);
        end_speaking(st, speaking);
    }
}

fn hush_core(mixer: &MixerRef, st: &Mutex<Speech>, listen: &Mutex<Listen>, speaking: &(dyn Fn(Option<String>) + Sync)) {
    end_speaking(st, speaking);
    clear_speech(st, listen);
    ring_stop_core(mixer, st, None);
    mixer.lock().unwrap().stop_overlay();
}

// ---------------------------------------------------------------------------------------------------------
// Public API (Bot)

/// Loads the acks in the background, retrying every 10 s until all are cached. Call once from `ready`.
pub fn spawn_acks(bot: Arc<Bot>) {
    tokio::spawn(async move { load_acks_core(&bot.cfg.voice_url, &bot.speech).await });
}

pub fn signal(bot: &Arc<Bot>, kind: SignalKind) {
    let speaker = bot.listen.lock().unwrap().speaking_to;
    signal_core(&bot.mixer, &bot.speech, kind, speaker);
}

/// Plays a short clip (48 kHz stereo i16) as overlay.
pub fn clip(bot: &Arc<Bot>, pcm: Vec<i16>) {
    clip_core(&bot.mixer, &bot.speech, pcm, false);
}

/// Speaking ring on or off; `owner`: only that speaker's end stops it.
pub fn ring(bot: &Arc<Bot>, on: bool, owner: Option<UserId>) {
    ring_core(&bot.mixer, &bot.speech, on, owner);
}

/// Spawned, so the brain loop is not blocked while the reply is synthesized and played.
pub fn say(bot: &Arc<Bot>, text: String, lang: String, voice: Voice, to_user: Option<String>) {
    let bot = bot.clone();
    let to = to_user.and_then(|u| u.parse::<u64>().ok()).filter(|&n| n > 0).map(UserId::new);
    let v = match voice {
        Voice::Bobby => "bobby",
        Voice::Claude => "claude",
    };
    tokio::spawn(async move {
        let speaking = |user| bot.send(ClientMsg::Speaking { user });
        say_core(&bot.mixer, &bot.speech, &bot.listen, &speaking, &bot.cfg.voice_url, &text, &lang, v, to).await
    });
}

pub fn hush(bot: &Arc<Bot>) {
    hush_core(&bot.mixer, &bot.speech, &bot.listen, &|user| bot.send(ClientMsg::Speaking { user }));
}

/// Leave / disconnect: speech cleared, ring and overlay stopped.
pub fn stop_all(bot: &Arc<Bot>) {
    hush_core(&bot.mixer, &bot.speech, &bot.listen, &|user| bot.send(ClientMsg::Speaking { user }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mixer::MixEvent;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

    fn mixer() -> (MixerRef, UnboundedReceiver<MixEvent>) {
        let (tx, rx) = unbounded_channel();
        (Arc::new(Mutex::new(Mixer::new(tx))), rx)
    }

    #[test]
    fn beep_matches_js_formula() {
        let b = beep_pcm();
        assert_eq!(b.len(), 5760 * 2);
        assert_eq!((b[0], b[1]), (0, 0));
        // i = 1: sin(2*pi*880/48000) * 6000, full envelope
        let want = ((2.0 * std::f64::consts::PI * 880.0 / 48000.0).sin() * 6000.0).round() as i16;
        assert_eq!((b[2], b[3]), (want, want));
        assert_eq!(*b.last().unwrap(), round((2.0 * std::f64::consts::PI * 880.0 * 5759.0 / 48000.0).sin() * 6000.0 / 1500.0));
        assert!(b.iter().all(|s| s.abs() <= 6000));
    }

    #[test]
    fn nope_matches_js_formula() {
        let n = nope_pcm();
        assert_eq!(n.len(), 12000 * 2);
        assert_eq!(n[0], 0);
        let i = 1500usize; // quarter of the first tone: envelope sin^2(pi/4) = 0.5
        let want = ((2.0 * std::f64::consts::PI * 330.0 * i as f64 / 48000.0).sin() * 4500.0 * 0.5).round() as i16;
        assert!((n[i * 2] - want).abs() <= 1);
        assert_eq!(n[6000 * 2], 0, "second tone starts at zero too");
    }

    #[test]
    fn ring_producer_stops_when_receiver_dropped() {
        let (h, mut rx) = noise_source();
        let mut f = [9i16; FRAME];
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(rx.take(&mut f), FRAME);
        assert!(f.iter().all(|s| s.abs() <= 2) && f.iter().any(|&s| s != 0));
        drop(rx);
        h.join().unwrap(); // would hang if the producer ignored the failed send
    }

    #[tokio::test]
    async fn ring_rules() {
        let (m, _ev) = mixer();
        let st = Mutex::new(Speech::default());
        let (a, b) = (UserId::new(1), UserId::new(2));
        ring_core(&m, &st, true, Some(a));
        assert!(m.lock().unwrap().has_overlay());
        ring_core(&m, &st, false, Some(b)); // another speaker's end: stays
        assert!(m.lock().unwrap().has_overlay());
        ring_core(&m, &st, false, Some(a));
        assert!(!m.lock().unwrap().has_overlay() && st.lock().unwrap().ring.is_none());
        st.lock().unwrap().current = Some(1); // Bobby speaks: no ring
        ring_core(&m, &st, true, None);
        assert!(!m.lock().unwrap().has_overlay());
    }

    fn tiny_wav() -> Vec<u8> {
        let data: Vec<u8> = (0..2205i16).flat_map(|i| ((i % 100) * 50).to_le_bytes()).collect(); // 0.1 s, 22050 Hz mono
        let mut w = b"RIFF".to_vec();
        w.extend((36 + data.len() as u32).to_le_bytes());
        w.extend(b"WAVEfmt ");
        w.extend(16u32.to_le_bytes());
        w.extend([1, 0, 1, 0]); // PCM, mono
        w.extend(22050u32.to_le_bytes());
        w.extend(44100u32.to_le_bytes());
        w.extend([2, 0, 16, 0]);
        w.extend(b"data");
        w.extend((data.len() as u32).to_le_bytes());
        w.extend(data);
        w
    }

    async fn fake_tts() -> String {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else { return };
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let _ = s.read(&mut buf).await;
                    let w = tiny_wav();
                    let head = format!("HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", w.len());
                    let _ = s.write_all(head.as_bytes()).await;
                    let _ = s.write_all(&w).await;
                });
            }
        });
        url
    }

    #[tokio::test]
    async fn say_plays_overlay_with_tail_then_clears_state() {
        let url = fake_tts().await;
        let (m, _ev) = mixer();
        let st = Arc::new(Mutex::new(Speech::default()));
        let listen = Arc::new(Mutex::new(Listen::default()));
        let task = {
            let (m, st, listen) = (m.clone(), st.clone(), listen.clone());
            tokio::spawn(async move { say_core(&m, &st, &listen, &|_| {}, &url, "Salut", "fr", "bobby", Some(UserId::new(5))).await })
        };
        for _ in 0..200 {
            if m.lock().unwrap().has_overlay() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(m.lock().unwrap().has_overlay(), "overlay started");
        assert_eq!(listen.lock().unwrap().speaking_to, Some(UserId::new(5)));
        let mut frames = 0;
        while m.lock().unwrap().next_frame().is_some() {
            frames += 1;
        }
        // 0.1 s of speech plus the 0.4 s tail (25 frames of 20 ms)
        assert!(frames >= 24, "tail included ({frames})");
        tokio::time::timeout(Duration::from_secs(5), task).await.unwrap().unwrap();
        assert!(listen.lock().unwrap().speaking_to.is_none() && st.lock().unwrap().current.is_none());
    }

    #[tokio::test]
    async fn say_error_is_logged_and_clears_state() {
        let (m, _ev) = mixer();
        let st = Mutex::new(Speech::default());
        let listen = Mutex::new(Listen::default());
        say_core(&m, &st, &listen, &|_| {}, "http://127.0.0.1:1", "x", "fr", "bobby", Some(UserId::new(1))).await;
        assert!(listen.lock().unwrap().speaking_to.is_none() && !m.lock().unwrap().has_overlay());
    }

    #[tokio::test]
    async fn ack_falls_back_and_never_repeats() {
        let (m, _ev) = mixer();
        let st = Mutex::new(Speech::default());
        assert!(!ack_core(&m, &st));
        {
            let mut s = st.lock().unwrap();
            for t in ACKS {
                s.acks.insert(t, Arc::new(vec![1; 100]));
            }
        }
        let mut last = None;
        for _ in 0..20 {
            assert!(ack_core(&m, &st));
            let now = st.lock().unwrap().last_ack;
            assert_ne!(now, last);
            last = now;
        }
    }

    #[test]
    fn loud_round_trips_generated_clips_through_wav() {
        let b = beep_pcm();
        assert_eq!(loud(b.clone()).len(), b.len()); // same length, only the level changes
    }

    /// Same loudness as the PC: a normalized clip leaves the mixer (voice gain 100 %, no music) at the RMS
    /// the PC plays it at (the PC path is `normalize` then its own volume, 100 % by default).
    #[test]
    fn mixer_output_keeps_the_normalized_rms() {
        let tone: Vec<i16> = (0..48_000).flat_map(|i| {
            let v = ((i as f32 * 0.05).sin() * 3000.0) as i16;
            [v, v]
        }).collect();
        let clip = loud(tone);
        let rms = |x: &[i16]| (x.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut m = crate::mixer::Mixer::new(tx);
        drop(m.overlay(PcmRx::from_vec(clip.clone())));
        let mut out = Vec::new();
        while let Some(f) = m.next_frame() {
            out.extend_from_slice(&f);
        }
        let (a, b) = (rms(&clip), rms(&out[..clip.len()]));
        assert!((a - b).abs() / a < 0.01, "mixer {b} vs clip {a}");
    }
}
