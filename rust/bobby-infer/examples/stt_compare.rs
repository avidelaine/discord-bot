//! R5 5.2: LocalStt against what the Python service transcribed (the .txt next to each collected clip).
//! cargo run --release -p bobby-infer --features ct2 --example stt_compare
use bobby_infer::stt::LocalStt;
use std::path::{Path, PathBuf};
use std::time::Instant;

const ROOT: &str = "C:/Users/alexa/Desktop/Personal Projects/discord-bot";
const WHISPER: &str = "models/whisper_models/models--mobiuslabsgmbh--faster-whisper-large-v3-turbo/snapshots/0a363e9161cbc7ed1431c9597a8ceaf0c4f78fcf";

fn norm(s: &str) -> Vec<String> {
    let t: String = s.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect();
    t.split_whitespace().map(String::from).collect()
}

fn edits(a: &[String], b: &[String]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + usize::from(a[i - 1] != b[j - 1]));
        }
        prev = cur;
    }
    prev[b.len()]
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if v.is_empty() { 0.0 } else if v.len() % 2 == 1 { v[v.len() / 2] } else { (v[v.len() / 2 - 1] + v[v.len() / 2]) / 2.0 }
}

/// Median of the `[stt] N ms` and `[local] N ms` lines of voice.log (Python's latency).
fn python_latency(log: &Path) -> Option<(f64, usize)> {
    let text = std::fs::read_to_string(log).ok()?;
    let mut v = vec![];
    for line in text.lines() {
        for tag in ["[stt] ", "[local] "] {
            if let Some(rest) = line.strip_prefix(tag) {
                if let Some(n) = rest.split(" ms").next().and_then(|n| n.trim().parse::<f64>().ok()) {
                    v.push(n);
                }
            }
        }
    }
    let n = v.len();
    Some((median(&mut v), n))
}

fn main() -> anyhow::Result<()> {
    let root = PathBuf::from(ROOT);
    let stt = LocalStt::new(&root.join(WHISPER), &root.join("models/vad/silero_vad_v6.onnx"))?;
    let mut clips: Vec<PathBuf> = vec![];
    for day in std::fs::read_dir(root.join("_training/collected"))? {
        let day = day?.path();
        if day.is_dir() {
            for f in std::fs::read_dir(&day)? {
                let f = f?.path();
                if f.extension().is_some_and(|e| e == "wav") && f.with_extension("txt").exists() {
                    clips.push(f);
                }
            }
        }
    }
    clips.sort();
    if let Ok(only) = std::env::var("STT_ONLY") {
        clips.retain(|c| only.split(',').any(|o| c.to_string_lossy().contains(o)));
    }
    let (mut errs, mut words, mut diffs, mut times) = (0usize, 0usize, 0usize, vec![]);
    let mut dump = String::new(); // STT_DUMP=<file>: name, hotwords, lang, text per clip (to compare with Python on the same input)
    for (n, f) in clips.iter().enumerate() {
        let mut r = hound::WavReader::open(f)?;
        anyhow::ensure!(r.spec().sample_rate == 16000 && r.spec().channels == 1, "{} is not 16 kHz mono", f.display());
        let pcm: Vec<f32> = r.samples::<i16>().map(|s| s.map(|v| v as f32 / 32768.0)).collect::<Result<_, _>>()?;
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        let hot = if name.contains("_discord_") { "Bobby" } else { "" };
        let t = Instant::now();
        let (text, lang) = stt.run(&pcm, hot)?;
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        if n > 0 {
            times.push(ms);
        }
        dump.push_str(&format!("{}\t{hot}\t{lang}\t{}\n", f.display(), text.replace('\n', " ")));
        let want = std::fs::read_to_string(f.with_extension("txt"))?;
        let (a, b) = (norm(&want), norm(&text));
        let e = edits(&a, &b);
        errs += e;
        words += a.len();
        if e > 0 {
            diffs += 1;
            println!("DIFF {name} ({lang}, {ms:.0} ms, {e} edits)\n  python: {:?}\n  rust:   {:?}", want.trim(), text);
        }
    }
    if let Ok(p) = std::env::var("STT_DUMP") {
        std::fs::write(p, dump)?;
    }
    let mean = times.iter().sum::<f64>() / times.len().max(1) as f64;
    let med = median(&mut times);
    println!("\nclips {}  differing {}  words {}  edits {}  WER {:.2} %", clips.len(), diffs, words, errs, 100.0 * errs as f64 / words.max(1) as f64);
    println!("rust ms (warm-up excluded): median {med:.0}  mean {mean:.0}");
    match python_latency(&root.join("_logs/voice.log")) {
        Some((m, n)) => println!("python ms (voice.log, {n} lines): median {m:.0}"),
        None => println!("python latency: voice.log not readable"),
    }
    Ok(())
}
