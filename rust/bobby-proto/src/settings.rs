//! The one settings file (plan rule 4): where Bobby lives, how each part is started, the port, which
//! implementation runs each stage, model addresses. Location: %APPDATA%\Bobbyobby.toml (or BOBBY_CONFIG).
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    /// HTTP + SSE port of the control API and UI (127.0.0.1 only).
    pub port: u16,
    /// Bobby's folder (holds bot/, voice/, _bin/, _logs/).
    pub root: PathBuf,
    /// Start Bobby as soon as the app starts (unless it already runs elsewhere).
    pub start_bobby_on_launch: bool,
    pub parts: Vec<Part>,
    /// Which implementation runs each stage.
    pub run: Run,
    /// Where the models live.
    pub models: Models,
    /// Data paths, relative to `root`.
    pub data: Data,
    /// Where the brain runs: "local" or a ws:// address.
    pub brain: Brain,
    /// Where the Discord client runs: "local" or a ws:// address.
    /// Secrets (the Discord token) stay in the `.env`, never here.
    pub discord: DiscordAt,
    /// PC speaker volumes (editable live from the app).
    pub audio: Audio,
}

/// Volumes of the PC speakers, in percent. Read at each use, so a change applies at once.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Audio {
    /// Beep loudness, percent of the full beep amplitude (0.3): 30 is about -10 dB.
    pub beep_volume: u8,
    /// Bobby's voice on the PC speakers, percent of the normalized level (100 = full).
    #[serde(alias = "voice_volume")]
    pub voice_volume_pc: u8,
    /// Bobby's voice on Discord, percent of the normalized level (applies once Discord runs in Rust).
    pub voice_volume_discord: u8,
}

impl Default for Audio {
    fn default() -> Self {
        Audio { beep_volume: 30, voice_volume_pc: 100, voice_volume_discord: 100 }
    }
}

/// Port switches. Flipping a switch back is the rollback of that port step.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Run {
    pub audio: AudioImpl,
    pub brain: BrainImpl,
    pub discord: DiscordImpl,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AudioImpl {
    #[default]
    Python,
    Rust,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum BrainImpl {
    #[default]
    Node,
    Rust,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum DiscordImpl {
    #[default]
    Node,
    Rust,
}

/// Model addresses (URLs); the value "local" means in-process engines (later).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Models {
    pub llm: String,
    pub stt: String,
    pub tts: String,
    pub wake_threshold: f32,
}

impl Default for Models {
    fn default() -> Self {
        Models {
            llm: "http://127.0.0.1:8080".into(),
            stt: "http://127.0.0.1:3000".into(),
            tts: "http://127.0.0.1:3000".into(),
            wake_threshold: 0.10,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Data {
    pub db: String,
    pub models: String,
}

impl Default for Data {
    fn default() -> Self {
        Data { db: "data/bobby.db".into(), models: "models".into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Brain {
    pub at: String,
}

impl Default for Brain {
    fn default() -> Self {
        Brain { at: "local".into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DiscordAt {
    pub at: String,
}

impl Default for DiscordAt {
    fn default() -> Self {
        DiscordAt { at: "local".into() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Part {
    pub name: String,
    /// Program: a path relative to `root`, an absolute path, or a bare name looked up in PATH ("node").
    pub cmd: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Working directory relative to `root` (default: `root`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Only output lines containing one of these (case-insensitive) go to the log; empty = every line.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keep: Vec<String>,
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

impl Default for Config {
    fn default() -> Self {
        Config {
            port: 4100,
            root: find_root(),
            start_bobby_on_launch: true,
            parts: vec![
                Part {
                    name: "llm".into(),
                    cmd: "_bin/llama/llama-server.exe".into(),
                    args: s(&[
                        "-m", "models/Qwen3-4B-Instruct-2507-Q4_K_M.gguf", "-ngl", "99", "-c", "8192", "-np", "1", "-fa", "on",
                        "-ctk", "q8_0", "-ctv", "q8_0", "--host", "127.0.0.1", "--port", "8080", "--log-file", "_logs/llm.log",
                    ]),
                    cwd: None,
                    // llama-server logs every request; keep what matters
                    keep: s(&["listening", "error", "failed", "out of memory"]),
                },
                Part { name: "voice".into(), cmd: "voice/.venv/Scripts/python.exe".into(), args: s(&["voice/server.py"]), cwd: None, keep: vec![] },
                Part { name: "bot".into(), cmd: "node".into(), args: s(&["bot.js"]), cwd: Some("bot".into()), keep: vec![] },
            ],
            run: Run::default(),
            models: Models::default(),
            data: Data::default(),
            brain: Brain::default(),
            discord: DiscordAt::default(),
            audio: Audio::default(),
        }
    }
}

/// The folder above the executable that holds bot/bot.js (the exe sits in rust/target/release), else the current dir.
pub fn find_root() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    exe.ancestors()
        .find(|d| d.join("bot").join("bot.js").is_file())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

pub fn path() -> PathBuf {
    if let Ok(p) = std::env::var("BOBBY_CONFIG") {
        return p.into();
    }
    PathBuf::from(std::env::var("APPDATA").unwrap_or_else(|_| ".".into())).join("Bobby").join("bobby.toml")
}

pub fn parse(text: &str) -> Result<Config, String> {
    toml::from_str(text).map_err(|e| e.to_string())
}

/// Reads the settings file, creating it with the defaults if it is missing. A broken file falls back to the defaults.
pub fn load() -> Config {
    let p = path();
    match std::fs::read_to_string(&p) {
        Ok(text) => parse(&text).unwrap_or_else(|e| {
            eprintln!("{}: {e}; using defaults", p.display());
            Config::default()
        }),
        Err(_) => {
            let c = Config::default();
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&p, toml::to_string_pretty(&c).unwrap_or_default());
            c
        }
    }
}

impl Config {
    pub fn logs(&self) -> PathBuf {
        self.root.join("_logs")
    }
    pub fn db_path(&self) -> PathBuf {
        self.root.join(&self.data.db)
    }
    pub fn models_dir(&self) -> PathBuf {
        self.root.join(&self.data.models)
    }
    /// Brain and Discord client in Rust, inside the app: one brain for Discord and the PC. Both switches go together;
    /// any other mix runs the Node bot (rollback: set either back to "node").
    pub fn rust_bobby(&self) -> bool {
        self.run.brain == BrainImpl::Rust && self.run.discord == DiscordImpl::Rust
    }
    /// Whether the supervisor starts this part: the Rust Bobby replaces `bot`, and `voice` too once speech is in process.
    pub fn runs_part(&self, name: &str) -> bool {
        let local = |a: &str| a.trim().eq_ignore_ascii_case("local");
        match name {
            "bot" => !self.rust_bobby(),
            "voice" => !(self.rust_bobby() && local(&self.models.stt) && local(&self.models.tts)),
            _ => true,
        }
    }
}

/// `Cap::VoiceVolume` on Discord (`discord: true`) or the PC speakers: changes that volume and saves the settings file.
pub fn voice_volume(discord: bool, op: crate::msg::VolumeOp) -> Result<serde_json::Value, String> {
    use crate::msg::VolumeOp;
    let mut c = load();
    let slot = if discord { &mut c.audio.voice_volume_discord } else { &mut c.audio.voice_volume_pc };
    let now = *slot as i32;
    let level = match op {
        VolumeOp::Get => return Ok(serde_json::json!({ "level": now })),
        VolumeOp::Set { level } => level as i32,
        VolumeOp::Delta { delta } => now + delta as i32,
        VolumeOp::Mute | VolumeOp::Unmute => return Err("unsupported".into()),
    }
    .clamp(0, 100) as u8;
    *slot = level;
    let text = toml::to_string_pretty(&c).map_err(|e| e.to_string())?;
    std::fs::write(path(), text).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "level": level }))
}

impl Part {
    /// `cmd` resolved: paths with a separator are relative to root, bare names stay for PATH lookup.
    pub fn program(&self, root: &Path) -> PathBuf {
        let p = Path::new(&self.cmd);
        if p.is_absolute() || p.components().count() == 1 { p.to_path_buf() } else { root.join(p) }
    }
    pub fn dir(&self, root: &Path) -> PathBuf {
        self.cwd.as_ref().map_or_else(|| root.to_path_buf(), |c| root.join(c))
    }
    pub fn keeps(&self, line: &str) -> bool {
        self.keep.is_empty() || {
            let l = line.to_lowercase();
            self.keep.iter().any(|k| l.contains(&k.to_lowercase()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_bobby_skips_bot_then_voice() {
        let mut c = Config::default();
        assert!(c.runs_part("bot") && c.runs_part("voice") && c.runs_part("llm"));
        c.run.brain = BrainImpl::Rust; // a mix runs Node
        assert!(!c.rust_bobby() && c.runs_part("bot"));
        c.run.discord = DiscordImpl::Rust;
        assert!(c.rust_bobby() && !c.runs_part("bot") && c.runs_part("voice"));
        c.models.stt = "local".into();
        assert!(c.runs_part("voice"), "tts still served by the voice service");
        c.models.tts = "Local".into();
        assert!(!c.runs_part("voice") && c.runs_part("llm"));
    }

    /// Written by the pre-move code (`toml::to_string_pretty(&Config::default())`, root pinned).
    const OLD_TOML: &str = r#"port = 4100
root = "C:/Bobby"
start_bobby_on_launch = true

[[parts]]
name = "llm"
cmd = "_bin/llama/llama-server.exe"
args = [
    "-m",
    "models/Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
    "-ngl",
    "99",
    "-c",
    "8192",
    "-np",
    "1",
    "-fa",
    "on",
    "-ctk",
    "q8_0",
    "-ctv",
    "q8_0",
    "--host",
    "127.0.0.1",
    "--port",
    "8080",
    "--log-file",
    "_logs/llm.log",
]
keep = [
    "listening",
    "error",
    "failed",
    "out of memory",
]

[[parts]]
name = "voice"
cmd = "voice/.venv/Scripts/python.exe"
args = ["voice/server.py"]

[[parts]]
name = "bot"
cmd = "node"
args = ["bot.js"]
cwd = "bot"
"#;

    #[test]
    fn old_file_loads_unchanged() {
        let old = parse(OLD_TOML).unwrap();
        let mut d = Config::default();
        d.root = PathBuf::from("C:/Bobby");
        assert_eq!(old, d);
    }

    #[test]
    fn old_file_gets_default_audio() {
        let c = parse(OLD_TOML).unwrap();
        assert_eq!((c.audio.beep_volume, c.audio.voice_volume_pc, c.audio.voice_volume_discord), (30, 100, 100));
    }

    #[test]
    fn previous_voice_volume_key_still_loads() {
        let c = parse("[audio]\nvoice_volume = 55\n").unwrap();
        assert_eq!((c.audio.voice_volume_pc, c.audio.voice_volume_discord), (55, 100));
    }

    #[test]
    fn only_beep_volume_keeps_other_defaults() {
        let c = parse("[audio]\nbeep_volume = 10\n").unwrap();
        assert_eq!((c.audio.beep_volume, c.audio.voice_volume_pc), (10, 100));
        let mut d = Config::default();
        d.audio.beep_volume = 10;
        assert_eq!(c, d);
    }

    #[test]
    fn only_run_audio_keeps_other_defaults() {
        let c = parse("[run]
audio = 'rust'
").unwrap();
        assert_eq!(c.run.audio, AudioImpl::Rust);
        assert_eq!(c.run.brain, BrainImpl::Node);
        assert_eq!(c.run.discord, DiscordImpl::Node);
        let mut d = Config::default();
        d.run.audio = AudioImpl::Rust;
        assert_eq!(c, d);
    }

    #[test]
    fn new_sections_defaults_and_paths() {
        let c = Config::default();
        assert_eq!(c.models.llm, "http://127.0.0.1:8080");
        assert_eq!(c.models.wake_threshold, 0.10);
        assert_eq!(c.brain.at, "local");
        assert_eq!(c.discord.at, "local");
        let c = parse("root = 'C:/b'
").unwrap();
        assert_eq!(c.db_path(), Path::new("C:/b").join("data/bobby.db"));
        assert!(toml::to_string_pretty(&c).unwrap().contains("audio = \"python\""));
    }

    #[test]
    fn defaults_round_trip_through_toml() {
        let c = Config::default();
        let text = toml::to_string_pretty(&c).unwrap();
        assert_eq!(parse(&text).unwrap(), c);
        assert_eq!(c.parts.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["llm", "voice", "bot"]);
    }

    #[test]
    fn partial_file_keeps_defaults() {
        let c = parse("port = 4101\nroot = 'D:/bobby'\n").unwrap();
        assert_eq!(c.port, 4101);
        assert_eq!(c.root, PathBuf::from("D:/bobby"));
        assert!(c.start_bobby_on_launch);
        assert_eq!(c.parts.len(), 3);
    }

    #[test]
    fn custom_parts_and_resolution() {
        let c = parse("[[parts]]\nname = 'x'\ncmd = 'tools/x.exe'\ncwd = 'sub'\nkeep = ['Ready']\n").unwrap();
        let root = Path::new("C:/b");
        let p = &c.parts[0];
        assert_eq!(c.parts.len(), 1);
        assert_eq!(p.program(root), root.join("tools/x.exe"));
        assert_eq!(p.dir(root), root.join("sub"));
        assert!(p.keeps("server READY now") && !p.keeps("GET /health"));
        let node = Part { name: "n".into(), cmd: "node".into(), args: vec![], cwd: None, keep: vec![] };
        assert_eq!(node.program(root), PathBuf::from("node"));
        assert!(node.keeps("anything"));
    }

    #[test]
    fn broken_file_is_an_error() {
        assert!(parse("port = 'abc'").is_err());
    }
}
