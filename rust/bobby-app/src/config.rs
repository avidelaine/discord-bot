// The one settings file (plan rule 4): where Bobby lives, how each part is started, the port.
// Location: %APPDATA%\Bobby\bobby.toml (or the BOBBY_CONFIG env var). Written with today's values on first run.
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
        }
    }
}

/// The folder above the executable that holds bot/bot.js (the exe sits in rust/target/release), else the current dir.
fn find_root() -> PathBuf {
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
