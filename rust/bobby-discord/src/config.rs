//! Settings of the Discord client, read from the same `.env` as the Node bot so both share one token.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

#[derive(Clone)]
pub struct Config {
    pub token: String,
    pub guild_id: u64,
    /// Name or id of Bobby's default text channel (`TEXT_CHANNEL`); empty when unset.
    pub text_channel: String,
    /// Base URL of the voice service (TTS).
    pub voice_url: String,
    /// Silence that ends a sentence (`RECORD_SILENCE_MS`).
    pub silence_ms: u64,
    pub super_admins: Vec<u64>,
    /// Local models (`MODELS_DIR`, default `models` next to the `.env`); used when `VOICE_URL=local`.
    pub models_dir: PathBuf,
    /// Whisper already loaded by the host (the app shares its PC one): used instead of loading a second copy in VRAM.
    pub shared_stt: Option<std::sync::Arc<bobby_infer::SttSlot>>,
    /// Piper already loaded (or loading) by the host: used instead of a second copy of the voices in RAM.
    pub shared_tts: Option<bobby_infer::TtsSlot>,
}

// Custom Debug: a stray `{:?}` in a log must never leak the token.
impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("token", &"<hidden>")
            .field("guild_id", &self.guild_id)
            .field("text_channel", &self.text_channel)
            .field("voice_url", &self.voice_url)
            .field("silence_ms", &self.silence_ms)
            .field("super_admins", &self.super_admins)
            .finish()
    }
}

/// Parses `KEY=value` lines (comments, blank lines and surrounding quotes ignored).
pub fn parse_env(text: &str) -> HashMap<String, String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().trim_matches('"').to_string()))
        .collect()
}

impl Config {
    /// Builds the config from a key lookup (`get` returns None for unset keys).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Config, String> {
        Config::from_lookup_in(get, std::path::Path::new("."))
    }

    /// Like `from_lookup`; `env_dir` is the folder of the `.env` (home of the default `models` folder).
    pub fn from_lookup_in(get: impl Fn(&str) -> Option<String>, env_dir: &std::path::Path) -> Result<Config, String> {
        let need = |k: &str| get(k).filter(|v| !v.is_empty()).ok_or_else(|| format!("{k} is missing"));
        Ok(Config {
            token: need("DISCORD_TOKEN")?,
            guild_id: need("GUILD_ID")?.parse().ok().filter(|&g: &u64| g > 0).ok_or_else(|| "GUILD_ID is not a valid id".to_string())?,
            text_channel: get("TEXT_CHANNEL").unwrap_or_default(),
            voice_url: get("VOICE_URL").filter(|v| !v.is_empty()).unwrap_or_else(|| "http://127.0.0.1:3000".into()),
            silence_ms: get("RECORD_SILENCE_MS").and_then(|v| v.trim().parse().ok()).unwrap_or(600),
            super_admins: get("SUPER_ADMINS")
                .map(|s| s.split(',').filter_map(|x| x.trim().parse::<u64>().ok()).filter(|&n| n > 0).collect())
                .unwrap_or_default(),
            models_dir: get("MODELS_DIR").filter(|v| !v.is_empty()).map_or_else(|| env_dir.join("models"), PathBuf::from),
            shared_stt: None,
            shared_tts: None,
        })
    }

    /// Loads the `.env` (path in `BOBBY_ENV`, else the main checkout's, like the R0 spike); real environment
    /// variables win over the file so a launcher can override one value.
    pub fn load() -> Result<Config, String> {
        let path = std::env::var("BOBBY_ENV")
            .unwrap_or_else(|_| r"C:\Users\alexa\Desktop\Personal Projects\discord-bot\.env".into());
        Config::load_from(&path)
    }

    /// Like `load`, from a given `.env`.
    pub fn load_from(path: &str) -> Result<Config, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
        let file = parse_env(&text);
        let dir = std::path::Path::new(path).parent().unwrap_or(std::path::Path::new(".")).to_path_buf();
        Config::from_lookup_in(|k| std::env::var(k).ok().or_else(|| file.get(k).cloned()), &dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_env_text() {
        let env = parse_env("# c\nDISCORD_TOKEN=\"abc\"\n\nGUILD_ID = 42\nSUPER_ADMINS=1, 2,x\nRECORD_SILENCE_MS=900\n");
        let cfg = Config::from_lookup(|k| env.get(k).cloned()).unwrap();
        assert_eq!((cfg.token.as_str(), cfg.guild_id, cfg.silence_ms), ("abc", 42, 900));
        assert_eq!(cfg.super_admins, vec![1, 2]);
        assert_eq!(cfg.voice_url, "http://127.0.0.1:3000");
        assert_eq!(cfg.models_dir, std::path::Path::new(".").join("models"));
        assert!(!format!("{cfg:?}").contains("abc"));
    }

    #[test]
    fn missing_token_is_an_error() {
        assert!(Config::from_lookup(|_| None).is_err());
    }
}
