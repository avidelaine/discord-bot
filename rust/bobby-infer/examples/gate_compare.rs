//! Gate parity: Rust `WakeScan` against Python `wakescan.WakeScan` on every collected Discord clip.
//!   python examples/gate_ref.py <main checkout>      (writes _training/gate_ref.csv)
//!   cargo run -p bobby-infer --features onnx --example gate_compare -- <main checkout> [gate_ref.csv]
use bobby_infer::listen::WakeScan;
use bobby_infer::wake::Wake;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(std::env::args().nth(1).ok_or("usage: gate_compare <main checkout> [csv]")?);
    let csv = std::env::args().nth(2).map_or(root.join("_training/gate_ref.csv"), PathBuf::from);
    let threshold = 0.10;
    let mut wake = Wake::new(&root.join("models/wakeword"))?;
    let (mut worst, mut mismatches, mut n) = (0f32, 0, 0);
    for line in std::fs::read_to_string(&csv)?.lines() {
        let (path, py) = line.rsplit_once(',').ok_or("bad csv line")?;
        let py: f32 = py.parse()?;
        let mono: Vec<i16> = hound::WavReader::open(path)?.samples::<i16>().collect::<Result<_, _>>()?;
        let raw: Vec<i16> = mono.iter().flat_map(|&s| [s; 6]).collect(); // x3, both channels
        let mut scan = WakeScan::new(&mut wake);
        for end in (1920..raw.len() + 1920).step_by(1920) {
            scan.feed(&raw[..end.min(raw.len())], false);
        }
        let rs = scan.feed(&raw, true);
        let diff = (rs - py).abs();
        let (a, b) = (rs >= threshold, py >= threshold);
        worst = worst.max(diff);
        n += 1;
        if a != b {
            mismatches += 1;
        }
        if diff > 0.01 || a != b {
            println!("{path}: rust {rs:.6} python {py:.6} diff {diff:.6}{}", if a != b { "  GATE MISMATCH" } else { "" });
        }
    }
    println!("{n} clips, max |rust - python| = {worst:.6}, gate decision mismatches at {threshold}: {mismatches}");
    Ok(())
}
