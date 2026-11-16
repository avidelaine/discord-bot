//! Media transport controls (SMTC): now playing and play/pause/next/previous.
use bobby_proto::msg::MediaCmd;
use serde_json::{json, Map, Value};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};

const PREFERRED: [&str; 4] = ["brave", "chrome", "music", "spotify"];

/// First session (by PREFERRED order) whose AppUserModelId contains the key.
fn pick(ids: &[String]) -> Option<usize> {
    PREFERRED.iter().find_map(|k| ids.iter().position(|i| i.to_lowercase().contains(k)))
}

fn session() -> Result<Option<Session>, String> {
    let e = |x: windows::core::Error| x.to_string();
    let mgr = Manager::RequestAsync().map_err(e)?.join().map_err(e)?;
    let sessions: Vec<Session> = mgr.GetSessions().map_err(e)?.into_iter().collect();
    let ids: Vec<String> = sessions
        .iter()
        .map(|s| s.SourceAppUserModelId().map(|h| h.to_string()).unwrap_or_default())
        .collect();
    match pick(&ids) {
        Some(i) => Ok(sessions.into_iter().nth(i)),
        None => Ok(mgr.GetCurrentSession().ok()),
    }
}

fn describe(s: Option<&Session>) -> Result<Value, String> {
    let Some(s) = s else { return Ok(json!({"playing": false, "title": "", "artist": ""})) };
    let e = |x: windows::core::Error| x.to_string();
    let info = s.TryGetMediaPropertiesAsync().map_err(e)?.join().map_err(e)?;
    let playing = s.GetPlaybackInfo().and_then(|p| p.PlaybackStatus()).map_err(e)? == Status::Playing;
    Ok(json!({"playing": playing, "title": info.Title().map_err(e)?.to_string(), "artist": info.Artist().map_err(e)?.to_string()}))
}

pub fn now_playing() -> Result<Value, String> {
    describe(session()?.as_ref())
}

pub fn command(cmd: MediaCmd) -> Result<Value, String> {
    let s = session()?.ok_or("no media session")?;
    let e = |x: windows::core::Error| x.to_string();
    let op = match cmd {
        MediaCmd::Play => s.TryPlayAsync(),
        MediaCmd::Pause => s.TryPauseAsync(),
        MediaCmd::Toggle => s.TryTogglePlayPauseAsync(),
        MediaCmd::Next => s.TrySkipNextAsync(),
        MediaCmd::Previous => s.TrySkipPreviousAsync(),
    };
    let ok = op.map_err(e)?.join().map_err(e)?;
    let mut out = Map::new();
    out.insert("ok".into(), json!(ok));
    if let Value::Object(m) = describe(Some(&s))? {
        out.extend(m);
    }
    Ok(Value::Object(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn pick_follows_preference_order() {
        assert_eq!(pick(&v(&["Spotify.exe", "Brave.Browser"])), Some(1));
        assert_eq!(pick(&v(&["Spotify.exe", "chrome"])), Some(1));
        assert_eq!(pick(&v(&["Microsoft.ZuneMusic", "Spotify.exe"])), Some(0));
        assert_eq!(pick(&v(&["vlc"])), None);
        assert_eq!(pick(&[]), None);
    }

    #[test]
    fn now_playing_has_keys() {
        let n = now_playing().unwrap();
        assert!(n["playing"].is_boolean() && n["title"].is_string() && n["artist"].is_string());
        println!("{n}");
    }
}
