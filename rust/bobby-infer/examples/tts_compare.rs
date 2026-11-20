//! Compares the Rust phoneme ids with the Python reference (`examples/piper_ref.py`), then writes two sample WAVs.
//! Run from `rust/`: `cargo run --release -p bobby-infer --features piper --example tts_compare [root]`
//! `root` (or BOBBY_ROOT): the checkout holding `models/` and `_training/`.
use bobby_infer::tts::LocalTts;
use std::path::PathBuf;
use std::time::Instant;

fn rms_and_secs(wav: &[u8]) -> (f64, f64) {
    let rate = u32::from_le_bytes(wav[24..28].try_into().unwrap()) as f64;
    let pcm: Vec<f64> = wav[44..].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f64 / 32768.0).collect();
    let rms = (pcm.iter().map(|x| x * x).sum::<f64>() / pcm.len().max(1) as f64).sqrt();
    (rms, pcm.len() as f64 / rate)
}

fn main() -> anyhow::Result<()> {
    let root: PathBuf = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("BOBBY_ROOT").ok())
        .unwrap_or_else(|| "C:/Users/alexa/Desktop/Personal Projects/discord-bot".into())
        .into();
    let tts = LocalTts::new(&root.join("models/piper"), &root.join("models/espeak-ng-data")).map_err(|e| anyhow::anyhow!("{e}"))?;
    let refs: Vec<serde_json::Value> = serde_json::from_str(&std::fs::read_to_string(root.join("_training/piper_ref.json"))?)?;
    let (mut same, mut dur_bad) = (0, 0);
    for r in &refs {
        let (text, lang, voice) = (r["text"].as_str().unwrap(), r["lang"].as_str().unwrap(), r["voice"].as_str().unwrap());
        let want: Vec<Vec<i64>> = r["ids"].as_array().unwrap().iter().map(|s| s.as_array().unwrap().iter().map(|x| x.as_i64().unwrap()).collect()).collect();
        let got = tts.phoneme_ids(text, lang, voice);
        if got == want {
            same += 1;
        } else {
            println!("MISMATCH [{lang}/{voice}] {text}\n  python: {want:?}\n  rust:   {got:?}");
        }
        let t = Instant::now();
        let wav = tts.wav(text, lang, voice).map_err(|e| anyhow::anyhow!("{e}"))?;
        let ms = t.elapsed().as_millis();
        let (rms, secs) = rms_and_secs(&wav);
        let py_secs = r["samples"].as_f64().unwrap() / r["rate"].as_f64().unwrap();
        let ratio = secs / py_secs;
        if !(0.75..=1.25).contains(&ratio) {
            dur_bad += 1;
        }
        println!("  {lang} {secs:.2}s (python {py_secs:.2}s, x{ratio:.2}) rms {rms:.3}  rust {ms} ms, python {} ms", r["ms"]);
    }
    println!("{same}/{} sentences with identical phoneme ids; {dur_bad} durations off by more than 25 %", refs.len());
    for (lang, text) in [
        ("fr", "Bonjour, je suis Bobby. Il est 18h30, et tout est prêt pour la soirée !"),
        ("en", "Hello, I'm Bobby. It's six thirty, and everything is ready for tonight!"),
    ] {
        let wav = tts.wav(text, lang, "bobby").map_err(|e| anyhow::anyhow!("{e}"))?;
        let p = root.join(format!("_training/piper_sample_{lang}.wav"));
        std::fs::write(&p, &wav)?;
        let (rms, secs) = rms_and_secs(&wav);
        println!("{} : {secs:.2} s, rms {rms:.3}", p.display());
    }
    Ok(())
}
