//! R5 5.1 spike: ct2rs + CUDA on the faster-whisper model: language probs, hotword prompt, scores, latency.
//! cargo run -p bobby-infer --features ct2 --example stt_spike -- <model dir> <16 kHz mono wav>...
use ct2rs::tokenizers::hf::Tokenizer;
use ct2rs_sys::{ComputeType, Config, Device, StorageView, Whisper, WhisperOptions};
use rustfft::{num_complex::Complex, FftPlanner};
use std::time::Instant;

const N_FFT: usize = 400;
const HOP: usize = 160;
const MELS: usize = 128;
const FRAMES: usize = 3000;

fn hz_to_mel(f: f64) -> f64 {
    if f >= 1000.0 { 15.0 + (f / 1000.0).ln() / (6.4f64.ln() / 27.0) } else { f / (200.0 / 3.0) }
}
fn mel_to_hz(m: f64) -> f64 {
    if m >= 15.0 { 1000.0 * ((m - 15.0) * (6.4f64.ln() / 27.0)).exp() } else { m * 200.0 / 3.0 }
}

/// librosa.filters.mel(sr=16000, n_fft=400, n_mels=128), slaney scale and norm (what faster-whisper uses).
fn mel_filters() -> Vec<f64> {
    let nf = N_FFT / 2 + 1;
    let (lo, hi) = (hz_to_mel(0.0), hz_to_mel(8000.0));
    let mel_f: Vec<f64> = (0..MELS + 2).map(|i| mel_to_hz(lo + (hi - lo) * i as f64 / (MELS + 1) as f64)).collect();
    let mut w = vec![0.0; MELS * nf];
    for i in 0..MELS {
        let enorm = 2.0 / (mel_f[i + 2] - mel_f[i]);
        for k in 0..nf {
            let f = 8000.0 * k as f64 / (nf - 1) as f64;
            let lower = (f - mel_f[i]) / (mel_f[i + 1] - mel_f[i]);
            let upper = (mel_f[i + 2] - f) / (mel_f[i + 2] - mel_f[i + 1]);
            w[i * nf + k] = lower.min(upper).max(0.0) * enorm;
        }
    }
    w
}

/// faster-whisper FeatureExtractor: pad 160, centered reflect STFT, |.|^2 minus last frame, log10 mel, then pad/trim to 3000.
fn log_mel(pcm: &[f32]) -> Vec<f32> {
    let mut x: Vec<f32> = pcm.to_vec();
    x.extend(std::iter::repeat(0.0).take(HOP));
    let p = N_FFT / 2;
    let n = x.len();
    let padded: Vec<f32> = (0..n + 2 * p)
        .map(|i| {
            let j = i as isize - p as isize;
            let j = if j < 0 { -j } else if j >= n as isize { 2 * (n as isize - 1) - j } else { j };
            x[j as usize]
        })
        .collect();
    let frames = 1 + (padded.len() - N_FFT) / HOP - 1;
    let win: Vec<f32> = (0..N_FFT).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / N_FFT as f32).cos()).collect();
    let fft = FftPlanner::<f32>::new().plan_fft_forward(N_FFT);
    let filt = mel_filters();
    let nf = N_FFT / 2 + 1;
    let mut spec = vec![0f32; MELS * frames];
    let mut buf = vec![Complex::new(0f32, 0.0); N_FFT];
    for t in 0..frames {
        for i in 0..N_FFT {
            buf[i] = Complex::new(padded[t * HOP + i] * win[i], 0.0);
        }
        fft.process(&mut buf);
        let mag: Vec<f64> = buf[..nf].iter().map(|c| c.norm_sqr() as f64).collect();
        for m in 0..MELS {
            let s: f64 = (0..nf).map(|k| filt[m * nf + k] * mag[k]).sum();
            spec[m * frames + t] = s.max(1e-10).log10() as f32;
        }
    }
    let max = spec.iter().cloned().fold(f32::MIN, f32::max);
    let mut out = vec![0f32; MELS * FRAMES];
    for m in 0..MELS {
        for t in 0..frames.min(FRAMES) {
            out[m * FRAMES + t] = (spec[m * frames + t].max(max - 8.0) + 4.0) / 4.0;
        }
    }
    out
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let cfg = Config { device: Device::CUDA, compute_type: ComputeType::INT8, ..Default::default() };
    let t = Instant::now();
    let w = Whisper::new(&a[1], cfg)?;
    let tok = Tokenizer::new(&a[1])?;
    println!("load {:?}", t.elapsed());
    for f in &a[2..] {
        let wav = std::fs::read(f)?;
        let pcm: Vec<f32> = wav[44..].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect();
        for hot in ["", "Bobby Alexandre"] {
            let t = Instant::now();
            let mut mel = log_mel(&pcm);
            let tm = t.elapsed();
            let sv = StorageView::new(&[1, MELS, FRAMES], &mut mel, Device::CPU)?;
            let enc = w.encode(&sv, false)?;
            let det = w.detect_language(&enc)?;
            let probs: Vec<(String, f32)> = det[0].iter().take(3).map(|d| (d.language.clone(), d.probability)).collect();
            let lang = det[0][0].language.clone();
            let mut prompt: Vec<String> = Vec::new();
            if !hot.is_empty() {
                prompt.push("<|startofprev|>".into());
                prompt.extend(tok.encode(format!(" {hot}").as_str(), false).map_err(|e| anyhow::anyhow!("{e}"))?.get_tokens().iter().cloned());
            }
            prompt.extend(["<|startoftranscript|>".to_string(), lang.clone(), "<|transcribe|>".into()]);
            let opts = WhisperOptions { beam_size: 1, return_scores: true, return_no_speech_prob: true, ..Default::default() };
            let r = w.generate(&enc, &[prompt], &opts)?;
            let toks = &r[0].sequences[0];
            let text_toks: Vec<String> = toks.iter().filter(|t| !t.starts_with("<|")).cloned().collect();
            let text = ct2rs::Tokenizer::decode(&tok, text_toks)?;
            let n = r[0].sequences_ids[0].len() as f32;
            let avg = r[0].scores[0] * n / (n + 1.0);
            println!("{f}\n  hot={hot:?} mel {tm:?} total {:?} langs {probs:?} no_speech {:.3} avg_logprob {avg:.3}\n  {text}", t.elapsed(), r[0].no_speech_prob);
        }
        if let Ok(py) = std::fs::read_to_string(f.replace(".wav", ".txt")) {
            println!("  python: {py}");
        }
    }
    Ok(())
}
