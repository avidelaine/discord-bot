// The all-Rust Bobby inside the app (`[run] brain = "rust"` and `discord = "rust"`): one brain (bobby-core), heard
// from Discord (bobby-discord) and from the PC microphone (bridge), its messages routed by `dest`. It starts and stops
// with Bobby (the supervisor's Start/Stop), and replaces the Node bot and its `BOT_URL/local` endpoint.
use crate::bridge::Bridge;
use crate::supervisor::Supervisor;
use bobby_core::brain::{Brain, Heard};
use bobby_core::io::Io;
use bobby_core::types::Role;
use bobby_core::store::Store;
use bobby_discord::Bot;
use bobby_infer::HttpLlm;
use bobby_proto::infer::Understand;
use crate::sleep::{self, Action, Event};
use bobby_proto::msg::{BrainMsg, Cap, CapKind, ClientMsg, Context, SignalKind, Voice};
use bobby_proto::settings::Config;
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};

#[derive(Debug, PartialEq)]
pub enum Dest {
    Discord,
    Pc,
}

/// Where a brain message goes: Discord caps to the Discord client, PC caps to the PC. Bobby's voice (and the
/// "no Bobby needed" window) goes to Discord while he is in a voice call, else to the PC speakers.
pub fn dest(m: &BrainMsg, in_voice: bool) -> Dest {
    let voice = if in_voice { Dest::Discord } else { Dest::Pc };
    match m {
        BrainMsg::Run { cap, .. } => match cap.kind() {
            CapKind::DiscordVoice | CapKind::DiscordText | CapKind::Music => Dest::Discord,
            CapKind::PcMedia | CapKind::PcVolume | CapKind::PcApps | CapKind::PcPower => Dest::Pc,
            CapKind::Voice => voice,
        },
        BrainMsg::Reply { .. } => Dest::Discord,
        _ => voice,
    }
}

/// A PC cap (blocking: COM calls, process scans), with the same functions as the bridge's `/pc/*` routes.
pub fn pc_cap(cap: Cap) -> Result<Value, String> {
    match cap {
        Cap::PcMedia { cmd } => bobby_pc::media::command(cmd),
        Cap::PcNow => bobby_pc::media::now_playing(),
        Cap::PcVolume { op } => bobby_pc::volume::apply(op),
        Cap::VoiceVolume { op } => bobby_proto::settings::voice_volume(false, op),
        c => bobby_pc::apps::run(&c).unwrap_or_else(|| Err(format!("not a PC cap: {c:?}"))),
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// One running brain with its two sides.
pub struct Router<L> {
    pub brain: Arc<Brain<L>>,
    /// The Discord client's inbox, None while it is not connected.
    pub discord: Mutex<Option<mpsc::UnboundedSender<BrainMsg>>>,
    /// The latest server snapshot (Discord sends it before each sentence and command).
    pub ctx: Mutex<Context>,
    pub in_voice: Box<dyn Fn() -> bool + Send + Sync>,
    /// The deep-sleep state: sentences are not sent to the brain until Bobby is awake.
    pub sleep_state: Box<dyn Fn() -> sleep::State + Send + Sync>,
    /// PC caps (run on a blocking thread).
    pub pc_run: Box<dyn Fn(Cap) -> Result<Value, String> + Send + Sync>,
    /// Bobby's voice, signals, hush and windows on the PC.
    pub pc_out: Box<dyn Fn(BrainMsg) + Send + Sync>,
}

impl<L: Understand + Send + Sync + 'static> Router<L> {
    fn result(&self, id: u64, r: Result<Value, String>) {
        let m = match r {
            Ok(data) => ClientMsg::Result { id, ok: true, data, error: None },
            Err(e) => ClientMsg::Result { id, ok: false, data: Value::Null, error: Some(e) },
        };
        self.brain.on_client(&m);
    }

    /// Brain -> clients.
    pub async fn route(self: Arc<Self>, mut rx: mpsc::UnboundedReceiver<BrainMsg>) {
        while let Some(m) = rx.recv().await {
            if m == BrainMsg::DeepSleep {
                sleep::fire(Event::SleepAsked);
                continue;
            }
            match dest(&m, (self.in_voice)()) {
                Dest::Discord => {
                    let tx = lock(&self.discord).clone();
                    let sent = match tx {
                        Some(tx) => tx.send(m).map_err(|e| e.0),
                        None => Err(m),
                    };
                    if let Err(m) = sent {
                        if let BrainMsg::Run { id, .. } = m {
                            self.result(id, Err("Discord is not connected".into())); // at once, not after the 30 s timeout
                        }
                    }
                }
                Dest::Pc => match m {
                    BrainMsg::Run { id, cap } => {
                        let me = self.clone();
                        tokio::spawn(async move {
                            let me2 = me.clone();
                            let r = tokio::task::spawn_blocking(move || (me2.pc_run)(cap)).await.unwrap_or_else(|e| Err(e.to_string()));
                            me.result(id, r);
                        });
                    }
                    other => (self.pc_out)(other),
                },
            }
        }
    }

    fn ctx_of(&self, user: &str) -> Context {
        Context { speaker: user.to_string(), ..lock(&self.ctx).clone() }
    }

    /// One transcribed sentence, from Discord or the PC.
    pub async fn hear(&self, user: String, text: String, lang: String, score: Option<f32>, stt_ms: u32, expected: bool) {
        let answers = self.brain.messenger.has_voice_waiter(&user);
        if !sleep::accept_sentence((self.sleep_state)(), answers) {
            return println!("[sleep] not awake yet: dropped a sentence from {user}");
        }
        let c = self.ctx_of(&user);
        let me = c.members.iter().find(|m| m.id == user);
        let (user_roles, user_name) = me.map_or_else(Default::default, |m| (m.roles.clone(), m.name.clone()));
        let h = Heard { text, lang, score, user, user_roles, user_name, stt_ms: Some(stt_ms), expected, ..Default::default() };
        self.brain.hear(h, &c).await;
    }

    /// Discord client -> brain.
    pub async fn serve(self: Arc<Self>, mut rx: mpsc::UnboundedReceiver<ClientMsg>) {
        while let Some(m) = rx.recv().await {
            match m {
                ClientMsg::Context(c) => {
                    self.brain.on_client(&ClientMsg::Context(c.clone())); // the messenger reads people's channels from it
                    *lock(&self.ctx) = c;
                }
                ClientMsg::Heard { user, text, lang, score, stt_ms, expected, .. } => {
                    let me = self.clone();
                    tokio::spawn(async move { me.hear(user, text, lang, score, stt_ms, expected).await });
                }
                ClientMsg::Command { id, name, user, .. } => {
                    let (me, c) = (self.clone(), self.ctx_of(&user));
                    tokio::spawn(async move { me.brain.command(id, &name, &c).await });
                }
                ClientMsg::Result { .. } | ClientMsg::Speaking { .. } | ClientMsg::Dm { .. } => self.brain.on_client(&m),
                ClientMsg::Hello { .. } | ClientMsg::Wake { .. } | ClientMsg::Utterance { .. } => {}
            }
        }
    }
}

// ponytail: 180 s, a cold LLM (model load into VRAM) took over 60 s; one constant, no config knob
const HEALTH_WAIT_S: u32 = 180;

/// Whether the person who said "Bobby" may wake him: whoever may give orders (not `Role::Other`).
/// `None` is the PC microphone, the super admin's own PC: always allowed.
pub fn wake_allowed(role: Option<Role>) -> bool {
    role.map_or(true, |r| bobby_core::roles::allowed(r, Role::Prime))
}

/// The deep-sleep stop, serialised with the wake-up by `gate`: does nothing (false) unless Bobby is still asleep
/// once it holds the gate, so a wake word that came first is not undone.
fn stop_services(gate: &Mutex<()>, state: impl Fn() -> sleep::State, stop: impl FnOnce()) -> bool {
    let _g = lock(gate);
    let asleep = state() == sleep::State::Asleep;
    if asleep {
        stop();
    }
    asleep
}

type Shared<T> = Arc<Mutex<Option<T>>>;

pub struct Host {
    cfg: Config,
    bridge: Arc<Bridge>,
    sup: Arc<Supervisor>,
    stop: Mutex<Option<oneshot::Sender<()>>>,
    /// The previous run, awaited by the next one so two gateways never overlap on a restart.
    last: Shared<tauri::async_runtime::JoinHandle<()>>,
    /// The running brain's router, for Bobby's own lines (deep sleep and wake-up).
    router: Mutex<Option<Arc<Router<HttpLlm>>>>,
    /// Plays the wake acknowledgement where Bobby is heard (Discord mixer or PC speakers); set while the brain runs.
    wake_clip: Mutex<Option<Arc<dyn Fn(Option<Vec<u8>>) + Send + Sync>>>,
}

impl Host {
    pub fn new(cfg: &Config, bridge: Arc<Bridge>, sup: Arc<Supervisor>) -> Arc<Self> {
        capture_stdio(sup.clone());
        let host = Arc::new(Host { cfg: cfg.clone(), bridge, sup, stop: Mutex::new(None), last: Default::default(), router: Default::default(), wake_clip: Default::default() });
        let h = host.clone();
        sleep::set_hook(move |e| h.sleep_event(e));
        let h = host.clone();
        bobby_infer::on_wake(move |who| h.wake(who));
        let h = host.clone();
        host.sup.on_busy(Box::new(move || h.busy()));
        host
    }

    /// The brain is in the middle of something (a request, a confirmation window): no deep sleep now.
    fn busy(&self) -> bool {
        lock(&self.router).as_ref().is_some_and(|r| r.brain.busy())
    }

    /// "Bobby" was heard while asleep: wakes him if that speaker may give orders (`who` None = the PC microphone).
    fn wake(self: &Arc<Self>, who: Option<String>) {
        if let (Some(id), Some(r)) = (who.as_deref(), lock(&self.router).clone()) {
            let roles = lock(&r.ctx).members.iter().find(|m| m.id == id).map(|m| m.roles.clone()).unwrap_or_default();
            if !wake_allowed(Some(bobby_core::roles::role_of(id, &roles, &r.brain.store))) {
                return self.log(&format!("[sleep] {id} may not give orders: not woken"));
            }
        }
        sleep::fire(Event::WakeWord);
    }

    fn sleep_event(self: &Arc<Self>, e: Event) {
        for a in self.sup.sleep_step(e) {
            self.exec(a);
        }
    }

    /// Bobby's own line, through the normal Say path (Piper stays loaded in deep sleep).
    fn say(&self, text: &str) {
        let Some(r) = lock(&self.router).clone() else { return };
        let m = BrainMsg::Say { text: text.into(), lang: "fr".into(), voice: Voice::Bobby, to_user: None };
        match dest(&m, (r.in_voice)()) {
            Dest::Discord => {
                if let Some(tx) = lock(&r.discord).as_ref() {
                    let _ = tx.send(m);
                }
            }
            Dest::Pc => (r.pc_out)(m),
        }
    }

    fn exec(self: &Arc<Self>, a: Action) {
        let h = self.clone();
        match a {
            Action::StopServices => {
                // off the async threads: unloading Whisper and taskkill both block
                std::thread::spawn(move || {
                    let gate = h.bridge.gate();
                    if stop_services(gate, || h.sup.sleep_state(), || {
                        h.bridge.unload_stt();
                        h.sup.hold("llm", true);
                    }) {
                        h.log("[sleep] deep sleep: Whisper unloaded, LLM stopped");
                    }
                });
            }
            Action::PlayWake => {
                // at once, off the caller: the recorded "oui" (the beep if the file is missing)
                let (clip, wav) = (lock(&self.wake_clip).clone(), std::fs::read(self.cfg.root.join("voice").join("oui.wav")).ok());
                if let Some(f) = clip {
                    std::thread::spawn(move || f(wav));
                }
            }
            Action::SayReady => self.say(sleep::READY),
            Action::SayFailed => self.say(sleep::FAILED),
            Action::StartServices => {
                tauri::async_runtime::spawn(async move {
                    let h2 = h.clone();
                    let _ = tauri::async_runtime::spawn_blocking(move || {
                        let _g = lock(h2.bridge.gate()); // after a stop that is still running, never before it
                        h2.bridge.reload_stt();
                        h2.sup.hold("llm", false);
                    })
                    .await;
                    let ok = h.llm_healthy().await;
                    h.sleep_event(if ok { Event::Ready } else { Event::Failed });
                });
            }
        }
    }

    /// The LLM's `/health`, polled for `HEALTH_WAIT_S` seconds.
    async fn llm_healthy(&self) -> bool {
        let base = self.cfg.models.llm.trim_end_matches('/').trim_end_matches("/v1");
        let (url, http) = (format!("{base}/health"), reqwest::Client::new());
        for _ in 0..HEALTH_WAIT_S {
            if http.get(&url).timeout(Duration::from_secs(2)).send().await.is_ok_and(|r| r.status().is_success()) {
                return true;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        false
    }

    fn log(&self, line: &str) {
        self.sup.log("bobby", line);
    }

    pub fn start(self: &Arc<Self>) {
        let b = self.bridge.clone();
        std::thread::spawn(move || {
            let _g = lock(b.gate());
            b.reload_stt()
        }); // a stop during deep sleep left Whisper unloaded
        let mut stop = lock(&self.stop);
        if stop.is_some() {
            return;
        }
        let (tx, rx) = oneshot::channel();
        *stop = Some(tx);
        let (me, prev) = (self.clone(), lock(&self.last).take());
        *lock(&self.last) = Some(tauri::async_runtime::spawn(async move {
            if let Some(p) = prev {
                let _ = p.await;
            }
            me.run(rx).await;
        }));
    }

    pub fn stop(&self) {
        if let Some(tx) = lock(&self.stop).take() {
            let _ = tx.send(());
        }
        *lock(&self.router) = None;
        *lock(&self.wake_clip) = None;
    }

    async fn run(self: Arc<Self>, stop: oneshot::Receiver<()>) {
        let cfg = &self.cfg;
        let env = cfg.root.join(".env");
        bobby_core::roles::load_dotenv(&env); // SUPER_ADMINS, ADMINS, PRIME_ROLE... for the brain
        let store = match Store::open(&cfg.db_path(), &cfg.root.join("data")) {
            Ok(s) => s,
            Err(e) => return self.log(&format!("[brain] cannot open {}: {e}", cfg.db_path().display())),
        };
        let (io, brain_rx) = Io::new();
        let brain = Arc::new(Brain::new(store, HttpLlm::new(&cfg.models.llm), Arc::new(io), cfg.logs().join("conversations.jsonl")));
        tokio::spawn(bobby_core::actions::ask::check());
        tokio::task::spawn_blocking(|| bobby_core::actions::pc::set_apps(bobby_pc::apps::index().iter().map(|a| a.name.clone()).collect()));

        let bot: Shared<Arc<Bot>> = Default::default();
        let (b, bridge) = (bot.clone(), self.bridge.clone());
        let gen = Arc::new(AtomicU64::new(0)); // bumped by Hush: what was queued before is not said
        let (pc_tx, pc_rx) = mpsc::unbounded_channel();
        let g = gen.clone();
        let router = Arc::new(Router {
            brain,
            discord: Mutex::new(None),
            ctx: Default::default(),
            in_voice: Box::new(move || lock(&b).as_ref().is_some_and(|b| b.in_voice())),
            sleep_state: { let sup = self.sup.clone(); Box::new(move || sup.sleep_state()) },
            pc_run: Box::new(pc_cap),
            pc_out: Box::new(move |m| match m {
                BrainMsg::Hush => {
                    g.fetch_add(1, Ordering::SeqCst);
                    bridge.hush();
                }
                BrainMsg::Expect { ms, .. } => bridge.expect(ms),
                m => {
                    let _ = pc_tx.send((g.load(Ordering::SeqCst), m));
                }
            }),
        });
        *lock(&self.router) = Some(router.clone());
        let (b2, br2, r2) = (bot.clone(), self.bridge.clone(), router.clone());
        *lock(&self.wake_clip) = Some(Arc::new(move |wav| match lock(&b2).clone().filter(|b| (r2.in_voice)() && b.in_voice()) {
            Some(b) => bobby_discord::speech::wake_clip(&b, wav),
            None => tauri::async_runtime::block_on(br2.wake_ack(wav)),
        }));
        let (stop_tx, stop_rx) = watch::channel(false);
        let tasks = [
            tokio::spawn(router.clone().route(brain_rx)),
            tokio::spawn(self.clone().speak_on_pc(router.clone(), pc_rx, gen)),
            tokio::spawn(self.clone().local_sync(router.clone())),
        ];
        let discord = tokio::spawn(self.clone().discord(router.clone(), bot.clone(), stop_rx));

        // The PC microphone talks to this brain, as the super admin (it is their PC), while Bobby is not in Discord voice.
        let (r, b) = (router.clone(), bot.clone());
        self.bridge.set_hook(Some(Arc::new(move |text, lang, score, ms| {
            let Some(sup) = bobby_core::roles::supers().into_iter().next() else { return false };
            let bot = lock(&b).clone();
            if bot.as_ref().is_some_and(|b| b.in_voice()) {
                return false; // on Discord: the PC is not listened to
            }
            if let Some(c) = bot.and_then(|b| b.context_for(sup.parse().ok()?)) {
                *lock(&r.ctx) = c; // fresh server, as Node read it from the guild cache
            }
            tauri::async_runtime::block_on(r.hear(sup.clone(), text, lang, score, ms, score.is_none()));
            r.brain.expecting(&sup)
        })));
        self.log("[brain] Rust brain started (Discord + PC)");

        let _ = stop.await; // Stop, Quit, or the Host dropped
        self.bridge.set_hook(None);
        self.bridge.set_local(false);
        let _ = stop_tx.send(true);
        if tokio::time::timeout(Duration::from_secs(10), discord).await.is_err() {
            self.log("[discord] did not stop within 10 s");
        }
        router.brain.messenger.clear(); // no messenger or waiter outlives the brain
        for t in tasks {
            t.abort();
        }
        self.log("[brain] Rust brain stopped");
    }

    /// Runs the Discord client, restarting it 3 s after the gateway ends (like a supervised part), until stopped.
    async fn discord(self: Arc<Self>, router: Arc<Router<HttpLlm>>, bot: Shared<Arc<Bot>>, stop: watch::Receiver<bool>) {
        let local = |a: &str| bobby_infer::is_local(a);
        loop {
            let mut dc = match bobby_discord::config::Config::load_from(&self.cfg.root.join(".env").to_string_lossy()) {
                Ok(c) => c,
                Err(e) => return self.log(&format!("[discord] {e}")),
            };
            // Speech: in process when both engines are local (Whisper shared with the PC), else the voice service.
            let (stt, tts) = (&self.cfg.models.stt, &self.cfg.models.tts);
            dc.voice_url = if local(stt) && local(tts) { "local".into() } else if local(stt) { tts.clone() } else { stt.clone() };
            dc.models_dir = self.cfg.models_dir();
            dc.shared_stt = local(&dc.voice_url).then(|| self.bridge.stt_slot());
            let (to_client, from_brain) = mpsc::unbounded_channel();
            let (to_brain, from_client) = mpsc::unbounded_channel();
            *lock(&router.discord) = Some(to_client);
            let serve = tokio::spawn(router.clone().serve(from_client));
            let (b, mut s) = (bot.clone(), stop.clone());
            let r = bobby_discord::client::run_until(dc, from_brain, to_brain, move |x| *lock(&b) = Some(x), async move {
                let _ = s.wait_for(|v| *v).await;
            })
            .await;
            *lock(&router.discord) = None;
            *lock(&bot) = None;
            serve.abort();
            if *stop.borrow() {
                return;
            }
            self.log(&format!("[discord] client ended ({}), restarting in 3 s", r.err().unwrap_or_else(|| "no error".into())));
            let mut s = stop.clone();
            if tokio::time::timeout(Duration::from_secs(3), s.wait_for(|v| *v)).await.is_ok() {
                return;
            }
        }
    }

    /// Local mode on while Bobby is not in Discord voice (Node's `syncLocal`); `LOCAL_MODE=off` in `.env` disables it.
    async fn local_sync(self: Arc<Self>, router: Arc<Router<HttpLlm>>) {
        let on = std::env::var("LOCAL_MODE").map_or(true, |v| v != "off");
        loop {
            self.bridge.set_local(on && !(router.in_voice)());
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    /// Bobby's voice and signals on the PC speakers, one at a time, in order (Node's `localIo`).
    async fn speak_on_pc(self: Arc<Self>, router: Arc<Router<HttpLlm>>, mut rx: mpsc::UnboundedReceiver<(u64, BrainMsg)>, gen: Arc<AtomicU64>) {
        while let Some((g, m)) = rx.recv().await {
            if g != gen.load(Ordering::SeqCst) {
                continue; // hushed since it was queued
            }
            let said = match m {
                BrainMsg::Say { text, lang, voice, to_user } => {
                    router.brain.on_client(&ClientMsg::Speaking { user: to_user });
                    println!("[{voice:?} on the PC] {text}");
                    let r = self.bridge.say(&text, &lang, if voice == Voice::Claude { "claude" } else { "bobby" }).await;
                    router.brain.on_client(&ClientMsg::Speaking { user: None });
                    r
                }
                BrainMsg::Signal { kind: SignalKind::Ack } => {
                    // ponytail: clock-based pick, the Discord client's no-repeat rule is not worth it here
                    let acks = bobby_discord::speech::ACKS;
                    let i = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos() as usize) % acks.len();
                    self.bridge.say(acks[i], "fr", "bobby").await
                }
                BrainMsg::Signal { kind: SignalKind::Beep } => Ok(self.bridge.beep("yes").await),
                BrainMsg::Signal { kind: SignalKind::Nope } => Ok(self.bridge.beep("nope").await),
                _ => Ok(()), // the listening ring is a Discord thing
            };
            if let Err((_, e)) = said {
                self.log(&format!("[local] speakers: {e}"));
            }
        }
    }
}

/// The brain and the Discord client log with println!/eprintln!, and the app has no console (windows subsystem):
/// both streams go to the supervisor's log (`_logs/bobby.log`, the dashboard), tagged "rust".
fn capture_stdio(sup: Arc<Supervisor>) {
    use std::io::BufRead;
    use std::os::windows::io::IntoRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Console::{SetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE};
    let Ok((reader, writer)) = std::io::pipe() else { return };
    let h = HANDLE(writer.into_raw_handle()); // leaked on purpose: it lives as long as the process
    // SAFETY: a valid pipe handle we own; Rust's stdout/stderr look the std handle up at each write.
    unsafe {
        let _ = SetStdHandle(STD_OUTPUT_HANDLE, h);
        let _ = SetStdHandle(STD_ERROR_HANDLE, h);
    }
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(reader).lines().map_while(Result::ok) {
            if !line.trim().is_empty() {
                sup.log("rust", line.trim_end());
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use bobby_proto::msg::{Channel, Member, MediaCmd, VolumeOp};
    use serde_json::json;

    const SUPER: &str = "261531632589078528"; // the brain's default super admin

    #[test]
    fn dest_by_family_and_voice_call() {
        let run = |cap: Cap| BrainMsg::Run { id: 1, cap };
        let say = BrainMsg::Say { text: "Oui ?".into(), lang: "fr".into(), voice: Voice::Bobby, to_user: None };
        for in_voice in [false, true] {
            assert_eq!(dest(&run(Cap::Mute { users: vec![], on: true }), in_voice), Dest::Discord);
            assert_eq!(dest(&run(Cap::Post { channel: None, text: None, embeds: vec![] }), in_voice), Dest::Discord);
            assert_eq!(dest(&run(Cap::MusicSkip), in_voice), Dest::Discord);
            assert_eq!(dest(&run(Cap::PcMedia { cmd: MediaCmd::Pause }), in_voice), Dest::Pc);
            assert_eq!(dest(&run(Cap::PcVolume { op: VolumeOp::Get }), in_voice), Dest::Pc);
            assert_eq!(dest(&run(Cap::PcShutdown), in_voice), Dest::Pc);
            assert_eq!(dest(&BrainMsg::Reply { id: 1, text: None, embeds: vec![], ephemeral: false }, in_voice), Dest::Discord);
        }
        let voice = |v| if v { Dest::Discord } else { Dest::Pc };
        for v in [false, true] {
            assert_eq!(dest(&say, v), voice(v));
            assert_eq!(dest(&BrainMsg::Signal { kind: SignalKind::Beep }, v), voice(v));
            assert_eq!(dest(&BrainMsg::Hush, v), voice(v));
            assert_eq!(dest(&BrainMsg::Expect { user: SUPER.into(), ms: 8000 }, v), voice(v));
            assert_eq!(dest(&run(Cap::VoiceVolume { op: VolumeOp::Delta { delta: 10 } }), v), voice(v));
        }
    }

    #[test]
    fn who_may_wake_him() {
        assert!(wake_allowed(None), "the PC microphone is the super admin's");
        assert!(wake_allowed(Some(Role::Super)) && wake_allowed(Some(Role::Admin)) && wake_allowed(Some(Role::Prime)));
        assert!(!wake_allowed(Some(Role::Other)), "someone who cannot give orders does not wake him");
    }

    #[test]
    fn stop_waits_for_the_gate_and_yields_to_a_wake_up() {
        let gate = Arc::new(Mutex::new(()));
        let ran = std::cell::Cell::new(0);
        // already awake or waking: nothing is stopped
        for s in [sleep::State::Awake, sleep::State::Waking] {
            assert!(!stop_services(&gate, || s, || ran.set(ran.get() + 1)));
        }
        assert_eq!(ran.get(), 0);
        assert!(stop_services(&gate, || sleep::State::Asleep, || ran.set(1)));
        assert_eq!(ran.get(), 1);
        // a holder of the gate blocks the stop until it lets go
        let guard = lock(&gate);
        let g2 = gate.clone();
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let d2 = done.clone();
        let t = std::thread::spawn(move || {
            stop_services(&g2, || sleep::State::Asleep, || {});
            d2.store(true, Ordering::SeqCst);
        });
        std::thread::sleep(Duration::from_millis(100));
        assert!(!done.load(Ordering::SeqCst));
        drop(guard);
        t.join().unwrap();
        assert!(done.load(Ordering::SeqCst));
    }

    fn member(id: &str, name: &str) -> Member {
        Member { id: id.into(), name: name.into(), username: name.to_lowercase(), global_name: None, channel: Some("1".into()), roles: vec![], bot: false }
    }

    /// The real brain on LLM-free shortcuts, no Discord, mic, speakers nor GPU: a PC order reaches the PC side only,
    /// a moderation order reaches the Discord side only, and the reply goes to the voice that is live.
    #[tokio::test]
    async fn real_brain_orders_reach_the_right_side() {
        bobby_core::actions::pc::set_apps(vec!["Valorant".into()]);
        let journal = bobby_core::store::temp_dir("host-route").join("conversations.jsonl");
        let (io, brain_rx) = Io::new();
        let brain = Arc::new(Brain::new(Store::temp(), HttpLlm::new("http://127.0.0.1:9"), Arc::new(io), journal));
        let (pc_caps, pc_msgs, dc_caps): (Arc<Mutex<Vec<Cap>>>, Arc<Mutex<Vec<BrainMsg>>>, Arc<Mutex<Vec<Cap>>>) = Default::default();
        let (dtx, mut drx) = mpsc::unbounded_channel();
        let (c1, c2) = (pc_caps.clone(), pc_msgs.clone());
        let router = Arc::new(Router {
            brain,
            discord: Mutex::new(Some(dtx)),
            ctx: Mutex::new(Context {
                speaker: SUPER.into(),
                members: vec![member(SUPER, "WakandeX"), member("zyk", "Zyklon")],
                voice_channels: vec![Channel { id: "1".into(), name: "Général".into() }],
                text_channels: vec![],
                bobby_channel: None,
            }),
            in_voice: Box::new(|| false), // local mode: Bobby speaks on the PC
            sleep_state: Box::new(|| sleep::State::Awake),
            pc_run: Box::new(move |cap| {
                lock(&c1).push(cap);
                Ok(json!({ "found": "Valorant" }))
            }),
            pc_out: Box::new(move |m| lock(&c2).push(m)),
        });
        tokio::spawn(router.clone().route(brain_rx));
        let (r, seen) = (router.clone(), dc_caps.clone());
        tokio::spawn(async move {
            // a Discord client that does every cap
            while let Some(m) = drx.recv().await {
                if let BrainMsg::Run { id, cap } = m {
                    lock(&seen).push(cap);
                    r.result(id, Ok(json!({ "prev": { "zyk": false } })));
                }
            }
        });
        router.hear(SUPER.into(), "Bobby, lance Valorant".into(), "fr".into(), Some(0.9), 300, false).await;
        router.hear(SUPER.into(), "Bobby, mute Zyklon".into(), "fr".into(), Some(0.9), 300, false).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        let pc_caps = lock(&pc_caps).clone();
        assert!(matches!(pc_caps.as_slice(), [Cap::PcLaunch { .. }]), "PC side ran {pc_caps:?}");
        let dc_caps = lock(&dc_caps).clone();
        assert_eq!(dc_caps, [Cap::Mute { users: vec!["zyk".into()], on: true }], "Discord side ran");
        let pc_msgs = lock(&pc_msgs).clone();
        assert!(!pc_msgs.is_empty() && pc_msgs.iter().all(|m| matches!(m, BrainMsg::Say { .. } | BrainMsg::Signal { .. })), "PC voice got {pc_msgs:?}");
    }

    #[tokio::test]
    async fn discord_cap_fails_at_once_when_discord_is_down() {
        let (io, brain_rx) = Io::new();
        let io = Arc::new(io);
        let brain = Arc::new(Brain::new(Store::temp(), HttpLlm::new("http://127.0.0.1:9"), io.clone(), bobby_core::store::temp_dir("host-down").join("j.jsonl")));
        let router = Arc::new(Router { brain, discord: Mutex::new(None), ctx: Default::default(), in_voice: Box::new(|| false), sleep_state: Box::new(|| sleep::State::Awake), pc_run: Box::new(pc_cap), pc_out: Box::new(|_| {}) });
        tokio::spawn(router.route(brain_rx));
        let r = tokio::time::timeout(Duration::from_secs(2), io.run(Cap::Leave)).await.expect("answered before the 30 s timeout");
        assert!(r.is_err());
    }
}
