//! Wake-word training data: every sentence heard is kept as a WAV (+ transcript). Port of `collect` in voice/server.py;
//! the file names and format are what the wake-model training reads, so they must not drift.
use chrono::{Local, Timelike};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_PER_DAY: usize = 600;

fn min_score() -> f32 {
    std::env::var("COLLECT_MIN_SCORE").ok().and_then(|v| v.parse().ok()).unwrap_or(0.003) // 2 turns collection off
}

/// `score` None = forced (no wake word needed). Never fails: a problem is logged and the sentence goes on.
pub fn collect(root: &Path, audio: &[f32], score: Option<f32>, text: Option<&str>, place: &str) {
    if score.is_some_and(|s| s < min_score()) {
        return;
    }
    if let Err(e) = write(root, audio, score, text, place) {
        eprintln!("[collect] {e}");
    }
}

fn write(root: &Path, audio: &[f32], score: Option<f32>, text: Option<&str>, place: &str) -> Result<(), String> {
    let now = Local::now();
    let day = root.join("_training").join("collected").join(now.format("%Y-%m-%d").to_string());
    std::fs::create_dir_all(&day).map_err(|e| e.to_string())?;
    let n = std::fs::read_dir(&day).map_err(|e| e.to_string())?.flatten().filter(|f| f.path().extension().is_some_and(|x| x == "wav")).count();
    if n >= MAX_PER_DAY {
        return Ok(());
    }
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() % 1000);
    let tag = score.map_or("forced".to_string(), |s| format!("{s:.3}"));
    let name = format!("{:02}{:02}{:02}_{ms:03}_{place}_{tag}", now.hour(), now.minute(), now.second());
    let spec = hound::WavSpec { channels: 1, sample_rate: 16000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(day.join(format!("{name}.wav")), spec).map_err(|e| e.to_string())?;
    for s in audio {
        w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16).map_err(|e| e.to_string())?; // truncates like numpy astype
    }
    w.finalize().map_err(|e| e.to_string())?;
    if let Some(t) = text {
        std::fs::write(day.join(format!("{name}.txt")), t).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(root: &Path) -> Vec<String> {
        let day = std::fs::read_dir(root.join("_training/collected")).unwrap().next().unwrap().unwrap().path();
        let mut v: Vec<String> = std::fs::read_dir(day).unwrap().map(|f| f.unwrap().file_name().into_string().unwrap()).collect();
        v.sort();
        v
    }

    #[test]
    fn names_header_and_txt() {
        let root = std::env::temp_dir().join(format!("bobby-collect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let audio = [0.0, 0.5, -0.5, 2.0, -2.0, 0.99999];
        collect(&root, &audio, Some(0.12345), Some("bonjour é"), "pc");
        collect(&root, &audio, None, None, "discord");
        collect(&root, &audio, Some(0.0001), None, "pc"); // below the minimum: skipped
        let f = files(&root);
        assert_eq!(f.len(), 3, "{f:?}"); // 2 wav + 1 txt
        let w = f.iter().find(|n| n.ends_with("_pc_0.123.wav")).expect("scored name");
        assert!(f.iter().any(|n| n.ends_with("_discord_forced.wav")));
        assert!(f.iter().any(|n| n.ends_with("_pc_0.123.txt")));
        let re = |n: &str| n.len() == "HHMMSS_mmm_pc_0.123.wav".len() && n[..6].bytes().all(|b| b.is_ascii_digit()) && &n[6..7] == "_" && n[7..10].bytes().all(|b| b.is_ascii_digit());
        assert!(re(w));
        let day = std::fs::read_dir(root.join("_training/collected")).unwrap().next().unwrap().unwrap().path();
        let bytes = std::fs::read(day.join(w)).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        let mut r = hound::WavReader::new(std::io::Cursor::new(bytes)).unwrap();
        let s = r.spec();
        assert_eq!((s.channels, s.sample_rate, s.bits_per_sample), (1, 16000, 16));
        let got: Vec<i16> = r.samples::<i16>().map(|x| x.unwrap()).collect();
        assert_eq!(got, vec![0, 16383, -16383, 32767, -32767, 32766]);
        assert_eq!(std::fs::read_to_string(day.join(w.replace(".wav", ".txt"))).unwrap(), "bonjour é");
        let _ = std::fs::remove_dir_all(&root);
    }
}
