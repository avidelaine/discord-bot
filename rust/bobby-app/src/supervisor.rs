// Runs Bobby's parts: spawn, restart 3 s after a crash, stop whole process trees. Port of app/supervisor.js.
// Publishes `status` (snapshot, on any change) and `log` ({part, line, at}) on the bus; every line also goes to _logs/bobby.log.
use crate::config::{Config, Part};
use crate::sleep;
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::broadcast;

pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
pub const RESTART_DELAY: Duration = Duration::from_secs(3);

#[derive(Clone, Debug)]
pub struct Event {
    pub name: &'static str,
    pub data: Value,
}
pub type Bus = broadcast::Sender<Event>;

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[derive(Default)]
struct PartState {
    pid: Option<u32>,
    restarts: u32,
    started_at: Option<i64>,
    /// Deep sleep: the restart loop waits while set (`stop()` clears it, so Bobby starts awake).
    held: bool,
}

#[derive(Default)]
struct State {
    running: bool,
    since: Option<i64>,
    external: bool,
    sleep: sleep::State,
    /// Bumped on every start and stop: a part task from an older run must not restart anything.
    run: u64,
    parts: Vec<PartState>,
}

pub struct Supervisor {
    root: PathBuf,
    logs: PathBuf,
    parts: Vec<Part>,
    state: Mutex<State>,
    log_file: Mutex<Option<std::fs::File>>,
    bus: Bus,
    env: Vec<(String, String)>,
    /// Called with true on start, false on stop: the in-process Rust Bobby follows Bobby's state (see `host`).
    on_run: std::sync::OnceLock<Box<dyn Fn(bool) + Send + Sync>>,
    /// True while Bobby is in the middle of something (set by the host from the brain): a sleep is then refused.
    busy: std::sync::OnceLock<Box<dyn Fn() -> bool + Send + Sync>>,
}

/// Extra env for every part, from the `[run]` switches. Flip the switch back + restart = rollback.
/// audio = rust: the bot sends local mode (/local, /speak, /beep, /hush) and /pc/* to this app instead of Python.
pub fn part_env(cfg: &Config) -> Vec<(String, String)> {
    match cfg.run.audio {
        bobby_proto::settings::AudioImpl::Rust => vec![("LOCAL_URL".into(), format!("http://127.0.0.1:{}", cfg.port))],
        bobby_proto::settings::AudioImpl::Python => vec![],
    }
}

impl Supervisor {
    pub fn new(cfg: &Config, bus: Bus) -> Arc<Self> {
        let logs = cfg.logs();
        let _ = std::fs::create_dir_all(&logs);
        let log_file = std::fs::OpenOptions::new().create(true).append(true).open(logs.join("bobby.log")).ok();
        // The Rust Bobby replaces the bot part (and the voice part once speech runs in process): see `runs_part`.
        let parts: Vec<Part> = cfg.parts.iter().filter(|p| cfg.runs_part(&p.name)).cloned().collect();
        Arc::new(Supervisor {
            root: cfg.root.clone(),
            logs,
            state: Mutex::new(State { parts: parts.iter().map(|_| PartState::default()).collect(), ..Default::default() }),
            parts,
            log_file: Mutex::new(log_file),
            bus,
            env: part_env(cfg),
            on_run: std::sync::OnceLock::new(),
            busy: std::sync::OnceLock::new(),
        })
    }

    pub fn on_run(&self, f: Box<dyn Fn(bool) + Send + Sync>) {
        let _ = self.on_run.set(f);
    }

    pub fn on_busy(&self, f: Box<dyn Fn() -> bool + Send + Sync>) {
        let _ = self.busy.set(f);
    }

    fn run_hook(&self, on: bool) {
        if let Some(f) = self.on_run.get() {
            f(on);
        }
    }

    pub fn part_names(&self) -> Vec<String> {
        self.parts.iter().map(|p| p.name.clone()).collect()
    }

    fn snapshot(&self, s: &State) -> Value {
        json!({
            "running": s.running,
            "since": s.since,
            "sleep": s.sleep.label(),
            "external": !s.running && s.external,
            "parts": self.parts.iter().zip(&s.parts).map(|(d, p)| json!({
                "name": d.name, "pid": p.pid, "up": p.pid.is_some(), "restarts": p.restarts, "startedAt": p.started_at,
            })).collect::<Vec<_>>(),
        })
    }

    pub fn status(&self) -> Value {
        self.snapshot(&self.state.lock().unwrap())
    }

    /// (name, pid) of the parts that are up, for the metrics sampler.
    pub fn pids(&self) -> Vec<(String, u32)> {
        let s = self.state.lock().unwrap();
        self.parts.iter().zip(&s.parts).filter_map(|(d, p)| p.pid.map(|pid| (d.name.clone(), pid))).collect()
    }

    fn changed(&self) {
        let _ = self.bus.send(Event { name: "status", data: self.status() });
    }

    pub fn log(&self, part: &str, line: &str) {
        let at = now_ms();
        if let Some(f) = self.log_file.lock().unwrap().as_mut() {
            let _ = writeln!(f, "{} [{part}] {line}", chrono::Local::now().format("%H:%M:%S"));
        }
        let _ = self.bus.send(Event { name: "log", data: json!({ "part": part, "line": line, "at": at }) });
    }

    /// Called by the sampler: an llama-server.exe we did not start means Bobby runs elsewhere (start.cmd / node start.js).
    pub fn set_external(&self, seen: bool) {
        let mut s = self.state.lock().unwrap();
        let ext = seen && !s.running;
        if ext != s.external {
            s.external = ext;
            drop(s);
            self.changed();
        }
    }

    pub fn is_external(&self) -> bool {
        let s = self.state.lock().unwrap();
        !s.running && s.external
    }

    /// The three user actions, with the same rules as the Node app (shared by the HTTP API and the tray).
    pub fn act(self: &Arc<Self>, what: &str) -> Result<Value, String> {
        match what {
            "stop" => Ok(self.stop()),
            "start" | "restart" if self.is_external() => Err("Bobby was started elsewhere (start.cmd): stop it first.".into()),
            "start" => {
                self.log("bobby", "start from the app");
                self.start()
            }
            "restart" => {
                self.log("bobby", "restart from the app");
                self.stop();
                self.start()
            }
            "sleep" => {
                if !self.state.lock().unwrap().running {
                    return Err("Bobby is not running: nothing to put to sleep.".into());
                }
                if self.busy.get().is_some_and(|f| f()) {
                    return Err("Bobby is busy with something else: try again in a moment.".into());
                }
                sleep::fire(sleep::Event::SleepAsked);
                Ok(self.status())
            }
            "wake" => {
                sleep::fire(sleep::Event::WakeWord);
                Ok(self.status())
            }
            _ => Err(format!("unknown action {what}")),
        }
    }

    pub fn start(self: &Arc<Self>) -> Result<Value, String> {
        let pid_file = self.logs.join("bobby.pid");
        let run = {
            let mut s = self.state.lock().unwrap();
            if s.running {
                return Ok(self.snapshot(&s));
            }
            // One Bobby at a time: a second one would log the bot in twice and fight over the ports.
            if let Some(old) = std::fs::read_to_string(&pid_file).ok().and_then(|t| t.trim().parse::<u32>().ok()) {
                if old != std::process::id() && process_alive(old) {
                    return Err(format!("Bobby is already running (pid {old}). Run stop.cmd first."));
                }
            }
            let _ = std::fs::write(&pid_file, std::process::id().to_string());
            s.running = true;
            s.external = false;
            s.since = Some(now_ms());
            s.run += 1;
            s.sleep = sleep::State::Awake;
            for p in &mut s.parts {
                p.restarts = 0;
            }
            s.run
        };
        for i in 0..self.parts.len() {
            tokio::spawn(self.clone().keep_alive(i, run));
        }
        self.run_hook(true);
        Ok(self.status())
    }

    /// Runs part `i` until the run it belongs to ends, restarting it 3 s after each exit.
    async fn keep_alive(self: Arc<Self>, i: usize, run: u64) {
        let def = &self.parts[i];
        loop {
            let mut cmd = tokio::process::Command::new(def.program(&self.root));
            cmd.args(&def.args)
                .current_dir(def.dir(&self.root))
                .env("PYTHONIOENCODING", "utf-8")
                .env("PYTHONUNBUFFERED", "1")
                .envs(self.env.iter().map(|(k, v)| (k, v)))
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .creation_flags(CREATE_NO_WINDOW);
            let code = match cmd.spawn() {
                Ok(mut child) => {
                    let held;
                    {
                        let mut s = self.state.lock().unwrap();
                        if s.run != run {
                            // stopped while spawning
                            drop(s);
                            kill_tree(child.id().unwrap_or(0));
                            return;
                        }
                        s.parts[i].pid = child.id();
                        s.parts[i].started_at = Some(now_ms());
                        // a hold arrived while spawning: it saw no pid to kill (killed below, outside the lock)
                        held = s.parts[i].held;
                    }
                    if held {
                        kill_tree(child.id().unwrap_or(0));
                    }
                    self.changed();
                    let out = tokio::spawn(self.clone().pipe(i, child.stdout.take()));
                    let err = tokio::spawn(self.clone().pipe(i, child.stderr.take()));
                    let status = child.wait().await;
                    let _ = tokio::join!(out, err);
                    status.ok().and_then(|s| s.code()).map_or("?".into(), |c| c.to_string())
                }
                Err(e) => {
                    self.log(&def.name, &format!("cannot start: {e}"));
                    "none".into()
                }
            };
            {
                let mut s = self.state.lock().unwrap();
                if s.run != run || !s.running {
                    return;
                }
                s.parts[i].pid = None;
            }
            self.log(&def.name, &format!("stopped (exit code {code}), restarting in 3 s"));
            self.changed();
            tokio::time::sleep(RESTART_DELAY).await;
            loop {
                {
                    let mut s = self.state.lock().unwrap();
                    if s.run != run || !s.running {
                        return;
                    }
                    if !s.parts[i].held {
                        s.parts[i].restarts += 1;
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(500)).await; // held (deep sleep)
            }
        }
    }

    async fn pipe(self: Arc<Self>, i: usize, stream: Option<impl AsyncRead + Unpin>) {
        let Some(stream) = stream else { return };
        let def = &self.parts[i];
        let mut r = BufReader::new(stream);
        let mut buf = Vec::new();
        while let Ok(n) = r.read_until(b'\n', &mut buf).await {
            if n == 0 {
                break;
            }
            let line = String::from_utf8_lossy(&buf);
            let line = line.trim_end_matches(['\r', '\n']);
            if !line.trim().is_empty() && def.keeps(line) {
                self.log(&def.name, line);
            }
            buf.clear();
        }
    }

    /// Deep sleep: `on` stops the part and keeps it down; `!on` lets the restart loop bring it back.
    pub fn hold(&self, name: &str, on: bool) {
        let pid = {
            let mut s = self.state.lock().unwrap();
            let Some(i) = self.parts.iter().position(|p| p.name == name) else { return };
            s.parts[i].held = on;
            if on { s.parts[i].pid } else { None }
        };
        if let Some(pid) = pid {
            kill_tree(pid);
        }
        self.changed();
    }

    /// Bobby is started (not stopped).
    pub fn running(&self) -> bool {
        self.state.lock().unwrap().running
    }

    /// Where deep sleep stands now.
    pub fn sleep_state(&self) -> sleep::State {
        self.state.lock().unwrap().sleep
    }

    /// Applies a sleep event to the state and returns what the host must do.
    pub fn sleep_step(&self, e: sleep::Event) -> Vec<sleep::Action> {
        let (acts, changed) = {
            let mut s = self.state.lock().unwrap();
            let (next, acts) = sleep::on(s.sleep, e);
            (acts, std::mem::replace(&mut s.sleep, next) != next)
        };
        if changed {
            self.changed();
        }
        acts
    }

    pub fn stop(&self) -> Value {
        let pids: Vec<u32> = {
            let mut s = self.state.lock().unwrap();
            if !s.running {
                if s.external {
                    // started elsewhere (start.cmd): let stop.cmd do it, but keep the Node app server (if any) alive
                    drop(s);
                    let _ = std::process::Command::new("cmd")
                        .args(["/c", "stop.cmd", "keep-app"])
                        .current_dir(&self.root)
                        .creation_flags(CREATE_NO_WINDOW)
                        .status();
                    self.state.lock().unwrap().external = false;
                    self.changed();
                }
                return self.status();
            }
            s.running = false;
            s.since = None;
            s.run += 1;
            s.sleep = sleep::State::Awake;
            s.parts.iter_mut().for_each(|p| p.held = false);
            s.parts.iter_mut().filter_map(|p| { p.started_at = None; p.pid.take() }).collect()
        };
        self.run_hook(false);
        // ponytail: taskkill /T kills each part's whole tree (voice's venv python.exe is only a launcher); sync so Quit can exit after
        for pid in pids {
            kill_tree(pid);
        }
        let pid_file = self.logs.join("bobby.pid");
        if std::fs::read_to_string(&pid_file).ok().and_then(|t| t.trim().parse::<u32>().ok()) == Some(std::process::id()) {
            let _ = std::fs::remove_file(pid_file);
        }
        self.changed();
        self.status()
    }
}

use std::os::windows::process::CommandExt as _;

fn kill_tree(pid: u32) {
    if pid == 0 {
        return;
    }
    let _ = std::process::Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

pub fn process_alive(pid: u32) -> bool {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    let Ok(mut sys) = System::new() else { return false };
    sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[Pid::from_u32(pid)]), true, ProcessRefreshKind::nothing());
    sys.process(Pid::from_u32(pid)).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy(dir: &std::path::Path) -> Config {
        let part = |name: &str, args: &[&str]| Part {
            name: name.into(),
            cmd: "cmd".into(),
            args: args.iter().map(|a| a.to_string()).collect(),
            cwd: None,
            keep: vec![],
        };
        Config {
            port: 0,
            root: dir.to_path_buf(),
            start_bobby_on_launch: false,
            // one part exits at once (restart check), one keeps running (stop check)
            parts: vec![part("short", &["/c", "echo hi"]), part("long", &["/c", "ping -n 60 127.0.0.1 >nul"])],
            ..Config::default()
        }
    }

    #[test]
    fn audio_switch_sets_local_url() {
        let mut c = Config { port: 4100, ..Config::default() };
        assert!(part_env(&c).is_empty());
        c.run.audio = bobby_proto::settings::AudioImpl::Rust;
        assert_eq!(part_env(&c), [("LOCAL_URL".to_string(), "http://127.0.0.1:4100".to_string())]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn restarts_crashed_part_and_stops_tree() {
        let dir = std::env::temp_dir().join(format!("bobby-sup-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (bus, mut rx) = broadcast::channel(1024);
        let sup = Supervisor::new(&dummy(&dir), bus);
        sup.start().unwrap();
        tokio::time::sleep(Duration::from_millis(4500)).await;
        let st = sup.status();
        assert_eq!(st["running"], true);
        assert!(st["parts"][0]["restarts"].as_u64().unwrap() >= 1, "short part restarted: {st}");
        let long_pid = st["parts"][1]["pid"].as_u64().unwrap() as u32;
        assert_eq!(st["parts"][1]["restarts"], 0);
        assert!(process_alive(long_pid));
        assert_eq!(std::fs::read_to_string(dir.join("_logs/bobby.pid")).unwrap(), std::process::id().to_string());

        // a second start while running is a no-op; a pid file from another live process blocks a start
        sup.start().unwrap();
        let st = sup.stop();
        assert_eq!(st["running"], false);
        assert!(!process_alive(long_pid), "long part killed");
        assert!(!dir.join("_logs/bobby.pid").exists());
        tokio::time::sleep(Duration::from_millis(3500)).await;
        assert!(sup.pids().is_empty(), "nothing restarted after stop");

        let mut lines = vec![];
        while let Ok(e) = rx.try_recv() {
            if e.name == "log" {
                lines.push(e.data["line"].as_str().unwrap().to_string());
            }
        }
        assert!(lines.iter().any(|l| l == "hi"));
        assert!(lines.iter().any(|l| l.contains("restarting in 3 s")));
        let log = std::fs::read_to_string(dir.join("_logs/bobby.log")).unwrap();
        assert!(log.contains("[short] hi"));

        // pid guard: a live foreign pid in bobby.pid refuses the start
        // killed by kill_tree below; reaping is not needed in a test
        #[allow(clippy::zombie_processes)]
        let other = std::process::Command::new("cmd").args(["/c", "ping -n 5 127.0.0.1 >nul"]).creation_flags(CREATE_NO_WINDOW).spawn().unwrap();
        std::fs::write(dir.join("_logs/bobby.pid"), other.id().to_string()).unwrap();
        assert!(sup.start().unwrap_err().contains("already running"));
        kill_tree(other.id());
        // stale pid file (dead process) is ignored
        assert!(sup.start().is_ok());
        sup.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sleep_refused_while_stopped_and_start_resets_it() {
        let (bus, _rx) = broadcast::channel(16);
        let sup = Supervisor::new(&dummy(&std::env::temp_dir()), bus);
        assert!(sup.act("sleep").unwrap_err().contains("not running"));
        // a stale Asleep (e.g. left by an earlier run) is reset by start()
        sup.state.lock().unwrap().sleep = sleep::State::Asleep;
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        let _g = rt.enter();
        let dir = std::env::temp_dir().join(format!("bobby-sup-sleep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (bus, _rx2) = broadcast::channel(16);
        let sup = Supervisor::new(&dummy(&dir), bus);
        sup.state.lock().unwrap().sleep = sleep::State::Asleep;
        sup.start().unwrap();
        assert_eq!(sup.sleep_state(), sleep::State::Awake);
        sup.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sleep_refused_while_busy() {
        let (bus, _rx) = broadcast::channel(16);
        let sup = Supervisor::new(&dummy(&std::env::temp_dir()), bus);
        sup.state.lock().unwrap().running = true; // no real parts needed
        let busy = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let b = busy.clone();
        sup.on_busy(Box::new(move || b.load(std::sync::atomic::Ordering::SeqCst)));
        assert!(sup.act("sleep").unwrap_err().contains("busy"));
        assert_eq!(sup.sleep_state(), sleep::State::Awake);
        busy.store(false, std::sync::atomic::Ordering::SeqCst);
        // no host hook installed in this test process: the call goes through
        assert!(sup.act("sleep").is_ok());
    }

    #[test]
    fn sleep_step_applies_events_and_returns_actions() {
        let (bus, _rx) = broadcast::channel(16);
        let sup = Supervisor::new(&dummy(&std::env::temp_dir()), bus);
        assert_eq!(sup.sleep_step(sleep::Event::SleepAsked), vec![sleep::Action::StopServices]);
        assert_eq!(sup.sleep_state(), sleep::State::Asleep);
        assert!(sup.sleep_step(sleep::Event::SleepAsked).is_empty());
        assert_eq!(sup.sleep_step(sleep::Event::WakeWord), vec![sleep::Action::PlayWake, sleep::Action::StartServices]);
        assert_eq!(sup.sleep_state(), sleep::State::Waking);
        assert_eq!(sup.sleep_step(sleep::Event::Failed), vec![sleep::Action::SayFailed]);
        assert_eq!(sup.sleep_state(), sleep::State::Awake);
        // the success path: asleep, woken, the LLM answers
        sup.sleep_step(sleep::Event::SleepAsked);
        sup.sleep_step(sleep::Event::WakeWord);
        assert_eq!(sup.sleep_step(sleep::Event::Ready), vec![sleep::Action::SayReady]);
        assert_eq!(sup.sleep_state(), sleep::State::Awake);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn held_part_is_not_restarted_and_stop_clears_hold() {
        let dir = std::env::temp_dir().join(format!("bobby-sup-hold-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (bus, _rx) = broadcast::channel(1024);
        let sup = Supervisor::new(&dummy(&dir), bus);
        sup.start().unwrap();
        sup.hold("short", true); // "short" exits at once; held, it must not come back
        tokio::time::sleep(Duration::from_millis(4500)).await;
        let st = sup.status();
        assert_eq!(st["parts"][0]["restarts"], 0, "held part not restarted: {st}");
        assert!(st["parts"][0]["pid"].is_null());
        sup.hold("short", false);
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(sup.status()["parts"][0]["restarts"].as_u64().unwrap() >= 1, "released part restarts");
        sup.hold("long", true);
        assert!(sup.state.lock().unwrap().parts[1].held);
        sup.stop();
        assert!(sup.state.lock().unwrap().parts.iter().all(|p| !p.held), "stop() clears the holds");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn external_blocks_start_but_not_stop() {
        let (bus, _rx) = broadcast::channel(16);
        let sup = Supervisor::new(&dummy(&std::env::temp_dir()), bus);
        sup.set_external(true);
        assert!(sup.is_external());
        assert!(sup.act("start").unwrap_err().contains("started elsewhere"));
        assert!(sup.act("restart").is_err());
    }
}
