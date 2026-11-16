//! Music player: queue + transport controls on top of the mixer (port of `bot/music/player.js`).
//! Every public method returns a `serde_json::Value` with the same keys as the JS version and never
//! fails, so the brain's code reads identical results.
//! Endless mode: when the queue runs dry Bobby goes on by itself. `similar` follows the YouTube Mix of the
//! last track (a style is seeded by a search); `artist` plays only songs of the seed.
//! ponytail: player.js was an EventEmitter ('track'/'end'/'error'); nobody listens to it, so it is dropped.
//! Locking: the state lock is never held across a lookup (`.await` on yt-dlp / radio), so `pause`, `list`...
//! answer while a new track is being found, and the old one keeps playing. The mixer lock is brief and sync.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, MutexGuard};

use bobby_proto::msg::{Endless, MusicItem};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::mixer::{Fade, Mixer};
use crate::sources::{decode, listing, radio, youtube, Entry, Station};

/// Crossfade between two tracks, seconds (same as the mixer's own, which is private).
const XFADE: f32 = 2.0;

fn watch(id: &str) -> String {
    format!("https://www.youtube.com/watch?v={id}")
}

/// Letter without its accent (what NFD + stripping the combining marks gives in the JS). Letters that do not
/// decompose (o-slash, ae, ss...) stay as they are, like in JS.
fn base(c: char) -> char {
    const TABLE: [(&str, char); 14] = [
        ("àáâãäåāăą", 'a'), ("çćč", 'c'), ("ď", 'd'), ("èéêëēėęě", 'e'), ("ğ", 'g'), ("ìíîïīį", 'i'),
        ("ñńň", 'n'), ("òóôõöōő", 'o'), ("ř", 'r'), ("śšş", 's'), ("ťţ", 't'), ("ùúûüūůűų", 'u'),
        ("ýÿ", 'y'), ("źżž", 'z'),
    ];
    TABLE.iter().find(|(set, _)| set.contains(c)).map_or(c, |&(_, b)| b)
}

pub fn norm(t: &str) -> String {
    t.to_lowercase().chars().map(base).filter(|c| !('\u{300}'..='\u{36f}').contains(c)).collect()
}

/// A song: not a 2 h mix, a live set or a podcast.
fn song(e: &Entry) -> bool {
    e.duration.is_some_and(|d| (60.0..=600.0).contains(&d))
}

/// Length of a `open ... close` group at the start of `s` (lazy, no line break inside), like `\(.*?\)`.
fn group(s: &[char], open: char, close: char) -> Option<usize> {
    if s.first() != Some(&open) {
        return None;
    }
    let end = s.iter().skip(1).position(|&c| c == close || matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}'))?;
    (s[end + 1] == close).then_some(end + 2)
}

/// "Judas (Hardstyle)" and "JUDAS (HARDSTYLE) [Official Video]" are the same song for the "already played"
/// check. Same as the JS regex `\(.*?\)|\[.*?\]|official|video|clip|lyrics?|audio|hd|4k` then non-alphanumerics -> space.
pub fn key(title: &str) -> String {
    let s: Vec<char> = norm(title).chars().collect();
    let mut out = String::new();
    let mut i = 0;
    'scan: while i < s.len() {
        let rest = &s[i..];
        if let Some(n) = group(rest, '(', ')').or_else(|| group(rest, '[', ']')) {
            i += n;
            continue;
        }
        for w in ["official", "video", "clip", "lyrics", "lyric", "audio", "hd", "4k"] {
            if rest.len() >= w.len() && rest.iter().zip(w.chars()).all(|(&a, b)| a == b) {
                i += w.len();
                continue 'scan;
            }
        }
        out.push(s[i]);
        i += 1;
    }
    let mut words = String::new();
    for c in out.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            words.push(c);
        } else if !words.ends_with(' ') {
            words.push(' ');
        }
    }
    words.trim().to_string()
}

fn kind(it: &MusicItem) -> &'static str {
    match it {
        MusicItem::Radio { .. } => "radio",
        MusicItem::Style { .. } => "style",
        MusicItem::Youtube { .. } => "youtube",
    }
}

fn label(it: &MusicItem) -> String {
    match it {
        MusicItem::Radio { style } => format!("{style} radio"),
        MusicItem::Style { style } => style.clone(),
        MusicItem::Youtube { query, .. } => query.clone(),
    }
}

fn mode_str(m: Endless) -> &'static str {
    match m {
        Endless::Similar => "similar",
        Endless::Artist => "artist",
    }
}

fn clamp_vol(n: i64) -> u8 {
    n.clamp(0, 100) as u8
}

/// Small random index below `n` (n > 0): the picks only need to differ between sessions.
fn rand_below(n: usize) -> usize {
    static C: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    let mut x = t ^ C.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    (x % n as u64) as usize
}

/// A track whose stream is known (JS: the item after `playNow` resolved it).
#[derive(Debug, Clone)]
struct Resolved {
    kind: &'static str, // "radio" | "youtube"
    id: String,
    title: String,
    url: String,
    station: Option<String>,
    alts: Vec<Station>, // radio: the other stations, tried when this stream dies before any sound
    live: bool,
    endless: Option<Endless>,
    seed: Option<String>,
}

enum Src {
    Item(MusicItem),
    Resolved(Resolved),
}

struct Current {
    item: Resolved,
    tag: u64,
}

struct Auto {
    id: u64, // told apart from a later endless session
    mode: Endless,
    seed: String,
    played: HashSet<String>, // ids and normalized title keys
    pool: Vec<Entry>,
    last: String,
}

#[derive(Default)]
struct State {
    queue: VecDeque<MusicItem>,
    current: Option<Current>,
    vol: u8,
    paused: bool,
    token: u64, // bumped by every request: a lookup that finds itself outdated gives up
    tags: u64,
    auto: Option<Auto>,
    autos: u64,
    upcoming: Option<JoinHandle<Option<Resolved>>>,
}

fn drop_upcoming(st: &mut State) {
    if let Some(h) = st.upcoming.take() {
        h.abort();
    }
}

/// Cheap handle: clones share the same player.
#[derive(Clone)]
pub struct Player {
    st: Arc<Mutex<State>>,
    mixer: Arc<StdMutex<Mixer>>,
}

impl Player {
    pub fn new(mixer: Arc<StdMutex<Mixer>>) -> Player {
        let vol = 50;
        mixer.lock().unwrap().volume = vol as f32 / 100.0;
        Player { st: Arc::new(Mutex::new(State { vol, ..Default::default() })), mixer }
    }

    fn mx(&self) -> MutexGuard<'_, Mixer> {
        self.mixer.lock().unwrap()
    }

    fn now_playing(st: &State) -> Value {
        let Some(c) = &st.current else { return Value::Null };
        let mut v = json!({ "title": c.item.title, "kind": c.item.kind });
        if let Some(s) = &c.item.station {
            v["station"] = json!(s);
        }
        if let Some(a) = &st.auto {
            v["endless"] = json!(mode_str(a.mode));
            v["seed"] = json!(a.seed);
        }
        v
    }

    /// Pause state and mixer together (the mixer is only touched when it differs: no useless restart).
    fn set_paused(&self, st: &mut State, on: bool) {
        st.paused = on;
        let mut m = self.mx();
        if m.paused() != on {
            m.pause(on);
        }
    }

    pub async fn add(&self, item: MusicItem) -> Value {
        let mut st = self.st.lock().await;
        if st.current.is_none() {
            drop(st);
            let mut r = self.play_now(item, true).await;
            r["started"] = json!(true);
            return r;
        }
        let l = label(&item);
        st.queue.push_back(item);
        json!({ "started": false, "position": st.queue.len(), "label": l })
    }

    pub async fn list(&self) -> Value {
        let st = self.st.lock().await;
        json!({
            "nowPlaying": Self::now_playing(&st),
            "endless": st.auto.as_ref().map(|a| json!({ "mode": mode_str(a.mode), "seed": a.seed })),
            "queue": st.queue.iter().map(|it| json!({ "kind": kind(it), "label": label(it) })).collect::<Vec<_>>(),
        })
    }

    pub async fn clear(&self) -> Value {
        let mut st = self.st.lock().await;
        let cleared = st.queue.len();
        st.queue.clear();
        json!({ "cleared": cleared })
    }

    /// Looks the item up, then crossfades into it. -> nowPlaying data, or `{error}`.
    /// `fresh`: a new request from someone (sets or ends the endless mode); false for the queue.
    pub async fn play_now(&self, item: MusicItem, fresh: bool) -> Value {
        self.play_src(Src::Item(item), fresh).await
    }

    async fn play_src(&self, src: Src, fresh: bool) -> Value {
        let id = {
            let mut st = self.st.lock().await;
            st.token += 1;
            st.token
        };
        // the old track keeps playing while the new one is looked up
        let item = match resolve(src).await {
            Ok(r) => r,
            Err(e) => return json!({ "error": e }),
        };
        let mut st = self.st.lock().await;
        if id != st.token {
            return json!({ "error": "superseded" });
        }
        if fresh {
            st.autos += 1;
            st.auto = item.endless.map(|mode| Auto {
                id: st.autos,
                mode,
                seed: item.seed.clone().unwrap_or_default(),
                played: HashSet::new(),
                pool: Vec::new(),
                last: String::new(),
            });
        }
        drop_upcoming(&mut st);
        let cross = self.mx().stop_music(Fade::Secs(XFADE)); // old track fades out while the new one fades in
        self.set_paused(&mut st, false);
        self.start(&mut st, item, cross)
    }

    fn start(&self, st: &mut State, item: Resolved, cross: bool) -> Value {
        st.tags += 1;
        let tag = st.tags;
        self.mx().set_music(decode(&item.url, item.live), tag, cross);
        let youtube = item.kind == "youtube";
        let (id, title) = (item.id.clone(), item.title.clone());
        st.current = Some(Current { item, tag });
        if youtube && st.auto.is_some() {
            let has_queue = !st.queue.is_empty();
            if let Some(a) = st.auto.as_mut() {
                a.played.insert(id.clone());
                a.played.insert(key(&title));
                a.last = id;
            }
            // the next track is found and resolved while this one plays: no gap at the end, "skip" is instant
            if !has_queue {
                let me = self.clone();
                st.upcoming = Some(tokio::spawn(async move { me.next_auto().await }));
            }
        }
        Self::now_playing(st)
    }

    /// The client's event loop calls this on `MixEvent::MusicEnded`. A tag that is not the current track's is
    /// stale (replaced or stopped meanwhile) and ignored.
    pub async fn on_ended(&self, tag: u64, read: usize) {
        {
            let mut st = self.st.lock().await;
            let Some(c) = st.current.as_ref().filter(|c| c.tag == tag) else { return };
            let mut item = c.item.clone();
            // a radio stream that died before any sound: try the next station of the list
            if read == 0 && !item.alts.is_empty() {
                let next = item.alts.remove(0);
                item.title = next.station.clone();
                item.station = Some(next.station);
                item.url = next.url;
                self.start(&mut st, item, false);
                return;
            }
        }
        self.advance().await;
    }

    /// Next of the queue, else of the endless mode, else nothing (current stays empty).
    async fn advance(&self) {
        self.st.lock().await.current = None;
        loop {
            let next = self.st.lock().await.queue.pop_front();
            let Some(it) = next else { break };
            if self.play_src(Src::Item(it), false).await.get("error").is_none() {
                return;
            }
        }
        for _ in 0..3 {
            let (auto_id, up) = {
                let mut st = self.st.lock().await;
                let Some(id) = st.auto.as_ref().map(|a| a.id) else { return };
                (id, st.upcoming.take())
            };
            let next = match up {
                Some(h) => h.await.ok().flatten(),
                None => self.next_auto().await,
            };
            {
                let st = self.st.lock().await;
                if st.auto.as_ref().map(|a| a.id) != Some(auto_id) || st.current.is_some() {
                    return; // stopped or replaced meanwhile
                }
            }
            if let Some(n) = next {
                if self.play_src(Src::Resolved(n), false).await.get("error").is_none() {
                    return;
                }
            }
        }
    }

    /// Next track of the endless mode, stream already resolved. ponytail: a fresh listing each time the pool
    /// runs out (2 s, hidden behind the current track); the mix drifts slowly from the seed, like a radio would.
    async fn next_auto(&self) -> Option<Resolved> {
        let (auto_id, mode, seed, last, mut played, mut pool) = {
            let st = self.st.lock().await;
            let a = st.auto.as_ref()?;
            (a.id, a.mode, a.seed.clone(), a.last.clone(), a.played.clone(), a.pool.clone())
        };
        let fresh = |e: &Entry, played: &HashSet<String>| song(e) && !played.contains(&e.id) && !played.contains(&key(&e.title));
        let mut cleared = false;
        if !pool.iter().any(|e| fresh(e, &played)) {
            let list = if mode == Endless::Artist {
                let s = norm(&seed);
                let all = listing(&format!("ytsearch30:{seed}"), 30).await.ok()?;
                all.into_iter().filter(|e| norm(&format!("{} {}", e.title, e.channel)).contains(&s)).collect()
            } else {
                listing(&format!("{}&list=RD{last}", watch(&last)), 25).await.ok()?
            };
            if mode == Endless::Artist && !list.iter().any(|e| fresh(e, &played)) {
                played.clear(); // every song of the artist played: start over
                cleared = true;
            }
            pool = list;
        }
        let cands: Vec<usize> = (0..pool.len()).filter(|&i| fresh(&pool[i], &played)).collect();
        // the mix lists the closest tracks first: pick among the first few so two sessions don't sound the same
        let pick = (!cands.is_empty()).then(|| cands[rand_below(if mode == Endless::Artist { cands.len() } else { cands.len().min(5) })]);
        let entry = pick.map(|i| pool.remove(i));
        {
            let mut st = self.st.lock().await;
            if let Some(a) = st.auto.as_mut().filter(|a| a.id == auto_id) {
                a.pool = pool;
                if cleared {
                    a.played.clear();
                }
            }
        }
        let e = entry?;
        let t = youtube(&watch(&e.id)).await.ok()?;
        Some(Resolved { kind: "youtube", id: t.id, title: t.title, url: t.url, station: None, alts: vec![], live: false, endless: Some(mode), seed: Some(seed) })
    }

    // ponytail: pausing a live radio just stops reading it; it may resume a bit behind live.
    pub async fn pause(&self) -> Value {
        let mut st = self.st.lock().await;
        if st.current.is_none() {
            return json!({ "error": "nothing is playing" });
        }
        self.set_paused(&mut st, true);
        json!({ "paused": true, "nowPlaying": Self::now_playing(&st) })
    }

    pub async fn resume(&self) -> Value {
        let mut st = self.st.lock().await;
        if st.current.is_none() {
            return json!({ "error": "nothing is playing" });
        }
        self.set_paused(&mut st, false);
        json!({ "paused": false, "nowPlaying": Self::now_playing(&st) })
    }

    /// Stops playback (fade out) and empties the queue.
    pub async fn stop(&self) -> Value {
        let mut st = self.st.lock().await;
        let cleared = st.queue.len();
        st.queue.clear();
        st.token += 1;
        st.auto = None;
        drop_upcoming(&mut st);
        self.mx().stop_music(Fade::Out);
        let stopped = Self::now_playing(&st);
        st.current = None;
        self.set_paused(&mut st, false);
        json!({ "stopped": stopped, "cleared": cleared })
    }

    pub async fn skip(&self) -> Value {
        let skipped = Self::now_playing(&*self.st.lock().await);
        if skipped.is_null() {
            return json!({ "error": "nothing is playing" });
        }
        self.advance().await; // play_src crossfades into the next track
        let st = self.st.lock().await;
        if st.current.is_none() {
            self.mx().stop_music(Fade::Out); // nothing next: fade out
        }
        json!({ "skipped": skipped, "nowPlaying": Self::now_playing(&st) })
    }

    /// Restarts the current track from the beginning (radio: reconnects).
    pub async fn replay(&self) -> Value {
        let mut st = self.st.lock().await;
        let Some(item) = st.current.as_ref().map(|c| c.item.clone()) else { return json!({ "error": "nothing is playing" }) };
        self.mx().stop_music(Fade::None);
        self.set_paused(&mut st, false);
        let now = self.start(&mut st, item, false);
        json!({ "nowPlaying": now })
    }

    pub async fn volume(&self) -> Value {
        json!({ "volume": self.st.lock().await.vol })
    }

    pub async fn set_volume(&self, n: i64) -> Value {
        let mut st = self.st.lock().await;
        st.vol = clamp_vol(n);
        self.mx().volume = st.vol as f32 / 100.0;
        json!({ "volume": st.vol })
    }

    pub async fn volume_up(&self, step: i64) -> Value {
        let v = self.st.lock().await.vol as i64;
        self.set_volume(v + step).await
    }

    pub async fn volume_down(&self, step: i64) -> Value {
        let v = self.st.lock().await.vol as i64;
        self.set_volume(v - step).await
    }

    /// Something is loaded and not paused.
    pub async fn is_playing(&self) -> bool {
        let st = self.st.lock().await;
        st.current.is_some() && !st.paused
    }

    pub async fn current_tag(&self) -> Option<u64> {
        self.st.lock().await.current.as_ref().map(|c| c.tag)
    }
}

/// Item -> track with a stream URL. Style: a real song of that style seeds the endless mix.
async fn resolve(src: Src) -> Result<Resolved, String> {
    let item = match src {
        Src::Resolved(r) => return Ok(r),
        Src::Item(i) => i,
    };
    let (query, endless, seed) = match item {
        MusicItem::Style { style } => {
            let seed = listing(&format!("ytsearch15:{style}"), 15).await?.into_iter().find(song);
            let seed = seed.ok_or_else(|| format!("nothing found on YouTube for \"{style}\""))?;
            (watch(&seed.id), Some(Endless::Similar), Some(style))
        }
        MusicItem::Radio { style } => {
            let mut stations = radio(&style).await?;
            if stations.is_empty() {
                return Err(format!("no radio found for \"{style}\""));
            }
            let first = stations.remove(0);
            return Ok(Resolved { kind: "radio", id: String::new(), title: first.station.clone(), url: first.url, station: Some(first.station), alts: stations, live: true, endless: None, seed: None });
        }
        MusicItem::Youtube { query, endless, seed } => (query, endless, seed),
    };
    let t = youtube(&query).await?;
    let seed = seed.filter(|s| !s.is_empty()).unwrap_or(query);
    Ok(Resolved { kind: "youtube", id: t.id, title: t.title, url: t.url, station: None, alts: vec![], live: false, endless, seed: Some(seed) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mixer::MixEvent;
    use tokio::sync::mpsc::unbounded_channel;

    fn idle() -> Player {
        let (tx, _rx) = unbounded_channel();
        Player::new(Arc::new(StdMutex::new(Mixer::new(tx))))
    }

    fn fake(title: &str) -> Resolved {
        Resolved { kind: "youtube", id: "x".into(), title: title.into(), url: "http://127.0.0.1:1/none".into(), station: None, alts: vec![], live: false, endless: None, seed: None }
    }

    #[test]
    fn key_ignores_case_brackets_and_noise() {
        assert_eq!(key("Judas (Hardstyle)"), key("JUDAS (HARDSTYLE) [Official Video]"));
        assert_eq!(key("Judas (Hardstyle)"), "judas");
        assert_eq!(key("Adèle - Lyrics Video HD"), "adele");
        assert_ne!(key("Judas"), key("Judas Priest"));
    }

    #[test]
    fn norm_strips_accents() {
        assert_eq!(norm("Adèle Castillon ÉCOLE"), "adele castillon ecole");
        assert_eq!(norm("Beyoncé"), "beyonce");
    }

    #[test]
    fn song_is_a_normal_track() {
        let e = |d| Entry { id: "a".into(), title: "t".into(), channel: "c".into(), duration: d };
        assert!(song(&e(Some(200.0))) && !song(&e(Some(7200.0))) && !song(&e(Some(30.0))) && !song(&e(None)));
    }

    #[test]
    fn labels() {
        assert_eq!(label(&MusicItem::Radio { style: "jazz".into() }), "jazz radio");
        assert_eq!(label(&MusicItem::Youtube { query: "daft punk".into(), endless: None, seed: None }), "daft punk");
    }

    #[test]
    fn volume_is_clamped() {
        assert_eq!((clamp_vol(-5), clamp_vol(40), clamp_vol(250)), (0, 40, 100));
    }

    #[tokio::test]
    async fn idle_player_answers_without_failing() {
        let p = idle();
        for r in [p.pause().await, p.resume().await, p.skip().await, p.replay().await] {
            assert_eq!(r, json!({ "error": "nothing is playing" }));
        }
        assert_eq!(p.stop().await, json!({ "stopped": null, "cleared": 0 }));
        assert_eq!(p.list().await, json!({ "nowPlaying": null, "endless": null, "queue": [] }));
        assert!(!p.is_playing().await && p.current_tag().await.is_none());
    }

    #[tokio::test]
    async fn volume_up_down_and_mixer_gain() {
        let p = idle();
        assert_eq!(p.volume().await, json!({ "volume": 50 }));
        assert_eq!(p.volume_up(10).await, json!({ "volume": 60 }));
        assert_eq!(p.volume_down(70).await, json!({ "volume": 0 }));
        assert_eq!(p.set_volume(250).await, json!({ "volume": 100 }));
        assert_eq!(p.mixer.lock().unwrap().volume, 1.0);
    }

    #[tokio::test]
    async fn add_queues_while_something_plays_and_transport_works() {
        let p = idle();
        p.st.lock().await.current = Some(Current { item: fake("Song"), tag: 1 });
        let r = p.add(MusicItem::Radio { style: "jazz".into() }).await;
        assert_eq!(r, json!({ "started": false, "position": 1, "label": "jazz radio" }));
        assert_eq!(p.list().await["queue"], json!([{ "kind": "radio", "label": "jazz radio" }]));
        assert_eq!(p.pause().await, json!({ "paused": true, "nowPlaying": { "title": "Song", "kind": "youtube" } }));
        assert!(!p.is_playing().await);
        assert_eq!(p.resume().await["paused"], json!(false));
        assert!(p.is_playing().await && p.current_tag().await == Some(1));
        assert_eq!(p.clear().await, json!({ "cleared": 1 }));
        assert_eq!(p.stop().await["stopped"]["title"], json!("Song"));
        assert!(p.current_tag().await.is_none());
    }

    #[tokio::test]
    async fn stale_tag_is_ignored_and_dead_radio_falls_back() {
        let p = idle();
        let mut r = fake("Station A");
        r.kind = "radio";
        r.station = Some("Station A".into());
        r.alts = vec![Station { station: "Station B".into(), url: "http://127.0.0.1:1/b".into() }];
        p.st.lock().await.current = Some(Current { item: r, tag: 5 });
        p.on_ended(4, 0).await; // stale
        assert_eq!(p.current_tag().await, Some(5));
        p.on_ended(5, 100).await; // played some sound, no alternates used: advance, nothing next
        assert!(p.current_tag().await.is_none());
        let mut r = fake("Station A");
        r.kind = "radio";
        r.station = Some("Station A".into());
        r.alts = vec![Station { station: "Station B".into(), url: "http://127.0.0.1:1/b".into() }];
        p.st.lock().await.current = Some(Current { item: r, tag: 9 });
        p.on_ended(9, 0).await; // died before any sound: next station
        let now = p.list().await["nowPlaying"].clone();
        assert_eq!(now["station"], json!("Station B"));
        assert_ne!(p.current_tag().await, Some(9));
    }

    // ---- network tests: cargo test -p bobby-discord --lib player -- --ignored --test-threads=1 ----

    fn rig() -> (Player, Arc<StdMutex<Mixer>>) {
        std::env::set_var("YTDLP", "C:/Users/alexa/Desktop/Personal Projects/discord-bot/_bin/yt-dlp.exe");
        let (tx, mut rx) = unbounded_channel();
        let mixer = Arc::new(StdMutex::new(Mixer::new(tx)));
        let p = Player::new(mixer.clone());
        let q = p.clone();
        tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                if let MixEvent::MusicEnded { tag, read } = ev {
                    q.on_ended(tag, read).await;
                }
            }
        });
        (p, mixer)
    }

    /// Pulls ~`n` frames at about real time and returns the loudest sample.
    async fn pump(m: &Arc<StdMutex<Mixer>>, n: usize) -> i32 {
        let mut peak = 0;
        for _ in 0..n {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            if let Some(f) = m.lock().unwrap().next_frame() {
                peak = peak.max(f.iter().map(|&s| (s as i32).abs()).max().unwrap_or(0));
            }
        }
        peak
    }

    #[tokio::test]
    #[ignore = "network"]
    async fn network_radio_and_youtube_play_non_silent_pcm() {
        let (p, m) = rig();
        for item in [MusicItem::Radio { style: "jazz".into() }, MusicItem::Youtube { query: "ytsearch1:lofi hip hop".into(), endless: None, seed: None }] {
            let np = p.play_now(item, true).await;
            assert!(np.get("error").is_none(), "{np}");
            let peak = pump(&m, 250).await;
            println!("{}: \"{}\" peak {peak}", np["kind"], np["title"]);
            assert!(peak > 500, "non-silent PCM");
            p.stop().await;
        }
    }

    #[tokio::test]
    #[ignore = "network"]
    async fn network_endless_style_and_artist_go_on_by_themselves() {
        let (p, _m) = rig();
        let artist = MusicItem::Youtube { query: "Adèle Castillon".into(), endless: Some(Endless::Artist), seed: Some("Adèle Castillon".into()) };
        for (item, on_artist) in [(MusicItem::Style { style: "hardstyle".into() }, false), (artist, true)] {
            let first = p.play_now(item, true).await;
            assert!(first.get("error").is_none() && first.get("endless").is_some(), "endless starts: {first}");
            let mut titles = vec![first["title"].as_str().unwrap().to_string()];
            for i in 0..2 {
                let r = p.skip().await;
                assert!(r["nowPlaying"].is_object(), "skip {} found a next track: {r}", i + 1);
                titles.push(r["nowPlaying"]["title"].as_str().unwrap().to_string());
            }
            println!("{titles:?}");
            assert_eq!(titles.iter().collect::<HashSet<_>>().len(), 3, "no repeat: {titles:?}");
            if on_artist {
                assert!(titles.iter().all(|t| norm(t).contains("adele castillon") || norm(t).contains("videoclub")), "stays on the artist: {titles:?}");
            }
            p.stop().await;
        }
    }
}
