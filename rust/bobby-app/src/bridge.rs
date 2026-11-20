// The routes the bot used to call on the Python voice service (voice/server.py + local.py + pc.py), served by the app:
// local listening on the PC microphone, speakers, and PC control (media + volume).
use crate::server::origin_ok;
use crate::supervisor::Supervisor;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bobby_audio::{collect::collect, gate::Gate, mic, out, out::Speaker, wake::Wake};
use bobby_infer::{HttpStt, HttpTts, Stt, SttSlot, Tts};
use bobby_proto::infer::{Speak, Transcribe};
use bobby_proto::msg::{MediaCmd, VolumeOp};
use bobby_proto::settings::Config;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Same command words as bot/brain.js COMMAND_WORDS and voice/server.py LOCAL_HOTWORDS.
const LOCAL_HOTWORDS: &str = "Bobby Discord le son volume musique hardstyle";

/// The Rust brain's ear on the PC: (text, lang, wake score, whisper ms) -> whether Bobby now waits for the next
/// sentence without "Bobby". Unset: sentences go to the Node bot (`BOT_URL/local`).
pub type Hook = Arc<dyn Fn(String, String, Option<f32>, u32) -> bool + Send + Sync>;

pub struct Bridge {
    cfg: Config,
    sup: Arc<Supervisor>,
    speaker: Speaker,
    tts: Arc<OnceLock<Tts>>,
    stt: Arc<SttSlot>,
    /// Builds Whisper again after a deep sleep.
    load_stt: Arc<dyn Fn() -> Stt + Send + Sync>,
    /// Set by a deep sleep: only then does `reload_stt` load (the startup load may still be running).
    unloaded: AtomicBool,
    hook: Mutex<Option<Hook>>,
    http: reqwest::Client,
    enabled: AtomicBool,
    /// True while a listener thread exists. On/off decisions and the thread's exit check happen under this lock.
    running: Mutex<bool>,
    /// Wake model, loaded once per Bridge (None inside = no model file, no gate).
    wake: OnceLock<Option<Arc<Mutex<Wake>>>>,
    /// Bobby waits for the next sentence ("Bobby" said alone): no wake word needed until then.
    force_until: Mutex<Instant>,
}

impl Bridge {
    pub fn new(cfg: &Config, sup: Arc<Supervisor>) -> Arc<Self> {
        // Engines load once ("local" = in-process Whisper/Piper, slow: off the main thread); on failure the app still starts on the HTTP engine.
        const FALLBACK: &str = "http://127.0.0.1:3000";
        let models = cfg.models_dir();
        let (stt, tts) = (Arc::new(SttSlot::empty()), Arc::new(OnceLock::new()));
        let (addr, cell, s2) = (cfg.models.stt.clone(), stt.clone(), sup.clone());
        let m2 = models.clone();
        let load = move || {
            Stt::from_addr(&addr, &m2).unwrap_or_else(|e| {
                s2.log("audio", &format!("[stt] {addr} failed to load, falling back to {FALLBACK}: {e}"));
                Stt::Http(HttpStt::new(FALLBACK))
            })
        };
        let load: Arc<dyn Fn() -> Stt + Send + Sync> = Arc::new(load);
        let load_stt = load.clone();
        if bobby_infer::is_local(&cfg.models.stt) {
            std::thread::spawn(move || cell.load(Arc::new(load())));
        } else {
            cell.load(Arc::new(load()));
        }
        let (addr, cell, s2) = (cfg.models.tts.clone(), tts.clone(), sup.clone());
        let load = move || {
            Tts::from_addr(&addr, &models).unwrap_or_else(|e| {
                s2.log("audio", &format!("[tts] {addr} failed to load, falling back to {FALLBACK}: {e}"));
                Tts::Http(HttpTts::new(FALLBACK))
            })
        };
        if bobby_infer::is_local(&cfg.models.tts) {
            std::thread::spawn(move || { let _ = cell.set(load()); });
        } else {
            let _ = cell.set(load());
        }
        Arc::new(Self {
            cfg: cfg.clone(),
            sup,
            speaker: Speaker::new(),
            tts,
            stt,
            load_stt,
            unloaded: AtomicBool::new(false),
            hook: Mutex::new(None),
            http: reqwest::Client::new(),
            enabled: AtomicBool::new(false),
            running: Mutex::new(false),
            wake: OnceLock::new(),
            force_until: Mutex::new(Instant::now()),
        })
    }

    /// The TTS slot, shared with the settings page's voice test (empty while a local engine loads).
    pub fn tts_slot(&self) -> Arc<OnceLock<Tts>> {
        self.tts.clone()
    }

    /// Whisper, once loaded: shared with the Rust Discord client so it sits in VRAM once.
    pub fn stt_slot(&self) -> Arc<SttSlot> {
        self.stt.clone()
    }

    /// Deep sleep: frees Whisper's VRAM (the in-process engine only; a remote one is not ours to unload).
    pub fn unload_stt(&self) {
        if bobby_infer::is_local(&self.cfg.models.stt) {
            self.unloaded.store(true, Ordering::SeqCst);
            self.stt.unload();
        }
    }

    /// Loads Whisper again if a deep sleep unloaded it (slow: call off the async threads).
    pub fn reload_stt(&self) {
        if self.unloaded.swap(false, Ordering::SeqCst) {
            self.stt.load(Arc::new((self.load_stt)()));
        }
    }

    pub fn set_hook(&self, hook: Option<Hook>) {
        *self.hook.lock().unwrap_or_else(|e| e.into_inner()) = hook;
    }

    /// The next PC sentence needs no "Bobby" for `ms` (`BrainMsg::Expect`; 0 closes the window).
    pub fn expect(&self, ms: u32) {
        *self.force_until.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now() + Duration::from_millis(ms as u64);
    }

    /// Speaks on the PC speakers and returns once spoken (or hushed). Err: (HTTP code for `/speak`, reason).
    pub async fn say(self: &Arc<Self>, text: &str, lang: &str, voice: &str) -> Result<(), (StatusCode, String)> {
        let Some(tts) = self.tts.get() else {
            self.log("[tts] speech engine still loading, nothing said");
            return Err((StatusCode::SERVICE_UNAVAILABLE, "speech engine still loading".into()));
        };
        match tokio::time::timeout(Duration::from_secs(30), tts.speak(text, lang, voice)).await {
            Err(_) => Err((StatusCode::GATEWAY_TIMEOUT, "tts timed out".into())),
            Ok(Err(e)) => Err((StatusCode::BAD_GATEWAY, e.to_string())),
            Ok(Ok(wav)) => {
                let vol = bobby_proto::settings::load().audio.voice_volume_pc; // read at each use: applies live
                let me = self.clone();
                let _ = tokio::task::spawn_blocking(move || me.speaker.play(&out::scale_wav(&bobby_infer::loudness::normalize(&wav.0), vol))).await;
                Ok(())
            }
        }
    }

    /// "yes" (heard) or "nope" beep on the PC speakers; returns once played.
    pub async fn beep(self: &Arc<Self>, kind: &str) {
        // 100 % = the normalized voice level: build the beep at full amplitude, normalize it, then scale by beep_volume.
        let wav = out::scale_wav(&bobby_infer::loudness::normalize(&out::beep(kind, 100)), bobby_proto::settings::load().audio.beep_volume);
        let me = self.clone();
        let _ = tokio::task::spawn_blocking(move || me.speaker.play(&wav)).await;
    }

    pub fn hush(&self) {
        self.speaker.hush();
    }

    fn log(&self, line: &str) {
        self.sup.log("audio", line);
    }

    /// `/local/on|off`. One dedicated thread owns the microphone (a `Mic` is not Send) and drops it when switched off.
    pub fn set_local(self: &Arc<Self>, on: bool) {
        let mut running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        if on != self.enabled.swap(on, Ordering::SeqCst) {
            self.log(if on { "[local] on: Bobby listens on the PC" } else { "[local] off: Bobby is on Discord" });
        }
        if on && !*running {
            *running = true;
            let me = self.clone();
            std::thread::spawn(move || loop {
                me.listen();
                // Decide under the same lock as set_local: a /local/on that arrived meanwhile keeps the thread going.
                let mut running = me.running.lock().unwrap_or_else(|e| e.into_inner());
                if !me.enabled.load(Ordering::SeqCst) {
                    *running = false;
                    return;
                }
            });
        }
    }

    /// No `bobby.onnx`: no gate (Ok(None)), every sentence is transcribed like the Python service.
    /// A model that exists but fails to load is an error: never run ungated by accident.
    fn wake_model(&self) -> Result<Option<Arc<Mutex<Wake>>>, String> {
        if let Some(w) = self.wake.get() {
            return Ok(w.clone());
        }
        let dir = self.cfg.models_dir().join("wakeword");
        let w = if dir.join("bobby.onnx").exists() { Some(Arc::new(Mutex::new(Wake::new(&dir)?))) } else { None };
        Ok(self.wake.get_or_init(|| w).clone())
    }

    fn listen(self: &Arc<Self>) {
        let wake = match self.wake_model() {
            Ok(w) => w,
            Err(e) => {
                self.enabled.store(false, Ordering::SeqCst);
                return self.log(&format!("[local] wake model failed to load, local mode not started: {e}"));
            }
        };
        let (me, mut gate) = (self.clone(), Gate::new());
        let started = mic::start(move |block| {
            if me.speaker.is_speaking() {
                return gate.reset(); // Bobby talks: ignore the mic (no echo of its own voice)
            }
            if let Some(audio) = gate.feed(block) {
                let (me, wake) = (me.clone(), wake.clone());
                std::thread::spawn(move || me.sentence(audio, wake.as_deref()));
            }
        });
        let _mic = match started {
            Ok(m) => m,
            Err(e) => {
                self.enabled.store(false, Ordering::SeqCst);
                return self.log(&format!("[local] microphone unavailable: {e}"));
            }
        };
        self.log("[local] listening on the PC microphone");
        while self.enabled.load(Ordering::SeqCst) {
            if _mic.failed() {
                // the bot's periodic /local/on resync restarts the listener
                self.enabled.store(false, Ordering::SeqCst);
                return self.log("[local] microphone unavailable: the input stream failed");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        self.log("[local] stopped listening on the PC microphone");
    }

    fn sentence(&self, audio: Vec<f32>, wake: Option<&Mutex<Wake>>) {
        let t = Instant::now();
        let mut score = None;
        if let Some(w) = wake {
            if Instant::now() >= *self.force_until.lock().unwrap_or_else(|e| e.into_inner()) {
                let s = w.lock().unwrap_or_else(|e| e.into_inner()).score(&audio);
                if s < self.cfg.models.wake_threshold {
                    collect(&self.cfg.root, &audio, Some(s), None, "pc");
                    return; // not for Bobby: never transcribed
                }
                score = Some(s);
                bobby_infer::wake_heard();
            }
        }
        let Some(stt) = self.stt.get() else {
            return self.log("[local] speech engine still loading, sentence dropped");
        };
        let tr = match tauri::async_runtime::block_on(tokio::time::timeout(Duration::from_secs(60), stt.transcribe(&audio, LOCAL_HOTWORDS))) {
            Ok(Ok(tr)) => tr,
            Ok(Err(e)) => return self.log(&format!("[local] transcription failed: {e}")),
            Err(_) => return self.log("[local] transcription timed out after 60 s, sentence dropped"),
        };
        collect(&self.cfg.root, &audio, score, Some(&tr.text), "pc");
        let ms = t.elapsed().as_millis();
        self.log(&format!("[local] {ms} ms  {:.1} s  {}  {:?}", audio.len() as f64 / 16000.0, tr.lang, tr.text));
        if tr.text.is_empty() {
            return;
        }
        let hook = self.hook.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(hear) = hook {
            let expect = hear(tr.text, tr.lang, score, ms as u32);
            return self.expect(if expect { 8000 } else { 0 });
        }
        let body = json!({"text": tr.text, "lang": tr.lang, "score": score.map(|s| (s as f64 * 1000.0).round() / 1000.0), "ms": ms});
        let bot = std::env::var("BOT_URL").unwrap_or_else(|_| "http://127.0.0.1:3001".into());
        let reply = tauri::async_runtime::block_on(async {
            let r = self.http.post(format!("{bot}/local")).timeout(Duration::from_secs(120)).json(&body).send().await?;
            r.json::<Value>().await
        });
        match reply {
            Ok(r) => *self.force_until.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now() + Duration::from_secs(if r["expect"] == true { 8 } else { 0 }),
            Err(e) => self.log(&format!("[local] bot not reachable at {bot}: {e}")),
        }
    }
}

type S = State<Arc<Bridge>>;
type Q = Query<HashMap<String, String>>;

fn reply(code: StatusCode, body: Value) -> Response {
    (code, [(header::CACHE_CONTROL, "no-store")], Json(body)).into_response()
}

fn forbidden(h: &HeaderMap) -> Option<Response> {
    (!origin_ok(h.get(header::ORIGIN).and_then(|v| v.to_str().ok()))).then(|| reply(StatusCode::FORBIDDEN, json!({ "error": "forbidden origin" })))
}

/// COM calls run on a blocking thread; `Err(msg)` becomes `code`.
async fn pc(f: impl FnOnce() -> Result<Value, String> + Send + 'static, code: StatusCode) -> Response {
    match tokio::task::spawn_blocking(f).await.unwrap_or_else(|e| Err(e.to_string())) {
        Ok(v) => reply(StatusCode::OK, v),
        Err(e) => reply(code, json!({ "detail": e })),
    }
}

async fn local_h(State(b): S, UrlPath(state): UrlPath<String>, h: HeaderMap) -> Response {
    if let Some(r) = forbidden(&h) { return r; }
    b.set_local(state == "on");
    reply(StatusCode::OK, json!({ "local": b.enabled.load(Ordering::SeqCst) }))
}

async fn speak_h(State(b): S, Query(q): Q, h: HeaderMap) -> Response {
    if let Some(r) = forbidden(&h) { return r; }
    let get = |k: &str, d: &str| q.get(k).cloned().unwrap_or_else(|| d.to_string());
    let text = get("text", "");
    if text.is_empty() {
        return reply(StatusCode::BAD_REQUEST, json!({ "detail": "text is required" }));
    }
    match b.say(&text, &get("lang", "fr"), &get("voice", "bobby")).await {
        Ok(()) => reply(StatusCode::OK, json!({ "ok": true })),
        Err((code, e)) => reply(code, json!({ "error": e })),
    }
}

async fn beep_h(State(b): S, Query(q): Q, h: HeaderMap) -> Response {
    if let Some(r) = forbidden(&h) { return r; }
    b.beep(q.get("kind").map_or("yes", String::as_str)).await;
    reply(StatusCode::OK, json!({ "ok": true }))
}

async fn hush_h(State(b): S, h: HeaderMap) -> Response {
    if let Some(r) = forbidden(&h) { return r; }
    b.speaker.hush();
    reply(StatusCode::OK, json!({ "ok": true }))
}

async fn media_get(h: HeaderMap) -> Response {
    if let Some(r) = forbidden(&h) { return r; }
    pc(bobby_pc::media::now_playing, StatusCode::INTERNAL_SERVER_ERROR).await
}

async fn media_cmd(UrlPath(cmd): UrlPath<String>, h: HeaderMap) -> Response {
    if let Some(r) = forbidden(&h) { return r; }
    let Ok(cmd) = serde_json::from_value::<MediaCmd>(json!(cmd)) else { return reply(StatusCode::BAD_REQUEST, json!({ "detail": "unknown command" })) };
    match tokio::task::spawn_blocking(move || bobby_pc::media::command(cmd)).await.unwrap_or_else(|e| Err(e.to_string())) {
        Ok(v) => reply(StatusCode::OK, v),
        Err(e) if e == "no media session" => reply(StatusCode::NOT_FOUND, json!({ "detail": e })),
        Err(e) => reply(StatusCode::INTERNAL_SERVER_ERROR, json!({ "detail": e })),
    }
}

async fn volume_get(h: HeaderMap) -> Response {
    if let Some(r) = forbidden(&h) { return r; }
    pc(bobby_pc::volume::state, StatusCode::INTERNAL_SERVER_ERROR).await
}

async fn volume_set(Query(q): Q, h: HeaderMap) -> Response {
    if let Some(r) = forbidden(&h) { return r; }
    let int = |k: &str| q.get(k).map(|v| v.trim().parse::<i32>().map_err(|_| k.to_string()));
    let mute = q.get("mute").map(|v| match v.to_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err("mute".to_string()),
    });
    let (Ok(level), Ok(delta), Ok(mute)) = (int("level").transpose(), int("delta").transpose(), mute.transpose()) else {
        return reply(StatusCode::BAD_REQUEST, json!({ "detail": "level and delta are integers, mute a boolean" }));
    };
    pc(
        move || {
            let mut state = None;
            if let Some(l) = level {
                state = Some(bobby_pc::volume::apply(VolumeOp::Set { level: l.clamp(0, 100) as u8 })?);
            } else if let Some(d) = delta {
                state = Some(bobby_pc::volume::apply(VolumeOp::Delta { delta: d.clamp(-100, 100) as i8 })?);
            }
            if let Some(m) = mute {
                state = Some(bobby_pc::volume::apply(if m { VolumeOp::Mute } else { VolumeOp::Unmute })?);
            }
            state.map_or_else(bobby_pc::volume::state, Ok)
        },
        StatusCode::INTERNAL_SERVER_ERROR,
    )
    .await
}

pub fn routes(b: Arc<Bridge>) -> Router {
    Router::new()
        .route("/local/{state}", post(local_h))
        .route("/speak", post(speak_h))
        .route("/beep", post(beep_h))
        .route("/hush", post(hush_h))
        .route("/pc/media", get(media_get))
        .route("/pc/media/{cmd}", post(media_cmd))
        .route("/pc/volume", get(volume_get).post(volume_set))
        .with_state(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn app() -> Router {
        let cfg = Config { root: std::env::temp_dir().join("bobby-bridge-test"), ..Default::default() };
        let (bus, _) = tokio::sync::broadcast::channel(8);
        routes(Bridge::new(&cfg, Supervisor::new(&cfg, bus)))
    }

    async fn call(method: &str, uri: &str, origin: Option<&str>) -> (StatusCode, Value) {
        let mut r = Request::builder().method(method).uri(uri);
        if let Some(o) = origin {
            r = r.header(header::ORIGIN, o);
        }
        let res = app().oneshot(r.body(Body::empty()).unwrap()).await.unwrap();
        let code = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
        (code, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn pc_routes_without_side_effects() {
        let (code, v) = call("GET", "/pc/volume", None).await;
        assert_eq!(code, StatusCode::OK);
        let level = v["level"].as_i64().expect("level");
        assert!(v["muted"].is_boolean());
        // same level again changes nothing audible, but only if the raw volume is a whole percent (else it would snap)
        if bobby_pc::volume::raw_is_whole_percent() {
            let (code, v2) = call("POST", &format!("/pc/volume?level={level}"), None).await;
            assert_eq!(code, StatusCode::OK);
            assert_eq!(v2["level"], v["level"]);
        } else {
            println!("volume is not a whole percent: skipping the same-level POST");
        }
        assert_eq!(call("POST", "/pc/volume?level=abc", None).await.0, StatusCode::BAD_REQUEST);
        assert_eq!(call("POST", "/pc/media/explode", None).await.0, StatusCode::BAD_REQUEST);
        for (m, u) in [("GET", "/pc/volume"), ("POST", "/pc/volume?mute=true"), ("POST", "/hush"), ("POST", "/local/off"), ("POST", "/pc/media/pause")] {
            assert_eq!(call(m, u, Some("https://evil.example")).await.0, StatusCode::FORBIDDEN, "{u}");
        }
        let (code, v) = call("POST", "/local/off", Some("http://127.0.0.1:4100")).await;
        assert_eq!((code, v), (StatusCode::OK, json!({ "local": false })));
    }
}
