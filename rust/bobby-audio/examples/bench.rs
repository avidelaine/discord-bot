//! cargo run -p bobby-audio --example bench -- <models_dir> <csv from `_training/bench.py --csv`>
//! Scores every clip of the CSV with `Wake::score` and compares with the Python scores.
use bobby_audio::wake::Wake;
use std::{collections::BTreeMap, path::Path, time::Instant};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (dir, csv) = (&args[1], &args[2]);
    let mut wake = Wake::new(Path::new(dir)).expect("load models");
    let mut sets: BTreeMap<String, usize> = BTreeMap::new();
    let mut diffs: Vec<(f32, String, f32, f32)> = vec![];
    let (mut frames, mut secs) = (0usize, 0f64);
    for line in std::fs::read_to_string(csv).unwrap().lines() {
        let Some((path, py)) = line.rsplit_once(',') else { continue };
        let py: f32 = py.trim().parse().unwrap();
        let samples: Vec<i16> = hound::WavReader::open(path).unwrap().samples::<i16>().map(|s| s.unwrap()).collect();
        let t = Instant::now();
        let rs = wake.score_i16(&samples);
        secs += t.elapsed().as_secs_f64();
        frames += (samples.len() + 16000) / 1280;
        let p = Path::new(path);
        let key = format!("{}/{}", p.parent().and_then(|d| d.parent()).and_then(|d| d.file_name()).unwrap().to_string_lossy(), p.parent().unwrap().file_name().unwrap().to_string_lossy());
        *sets.entry(key).or_default() += 1;
        diffs.push(((rs - py).abs(), path.to_string(), rs, py));
    }
    for (k, n) in &sets {
        println!("{k}: {n} clips");
    }
    diffs.sort_by(|a, b| b.0.total_cmp(&a.0));
    let max = diffs.first().map_or(0.0, |d| d.0);
    println!("{} clips, max |rust - python| = {max:.6}, {:.2} ms per 1280-frame", diffs.len(), secs * 1000.0 / frames.max(1) as f64);
    println!("{} clips over 0.01", diffs.iter().filter(|d| d.0 > 0.01).count());
    for (d, p, r, y) in diffs.iter().take(5) {
        println!("  {d:.6}  rust {r:.4} python {y:.4}  {p}");
    }
    if max > 0.01 {
        std::process::exit(1);
    }
}
