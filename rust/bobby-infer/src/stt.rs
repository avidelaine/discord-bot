//! In-process speech-to-text: a port of voice/server.py `_whisper` on top of CTranslate2 (the engine under
//! faster-whisper). Silero VAD -> log-mel -> language detection (fr/en fallback) -> greedy generation with timestamps
//! -> segments split by timestamp tokens -> filters. Same defaults as `WhisperModel.transcribe(beam_size=1,
//! temperature=0, vad_filter=True, condition_on_previous_text=False, hotwords=...)`.
use crate::vad::{collect_chunks, Vad, VadOptions};
use bobby_proto::infer::{InferError, InferErrorKind, Transcribe, Transcript};
use ct2rs::tokenizers::hf::Tokenizer;
use ct2rs_sys::{ComputeType, Config, Device, StorageView, Whisper, WhisperOptions};
use rustfft::{num_complex::Complex, num_traits::Zero, Fft, FftPlanner};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

const N_FFT: usize = 400;
const HOP: usize = 160;
pub const MELS: usize = 128;
const FRAMES: usize = 3000; // 30 s window
const TIME_PER_FRAME: f64 = 0.01;
const TIME_PRECISION: f64 = 0.02;
const INPUT_STRIDE: usize = 2;
const MAX_LENGTH: usize = 448;
const NO_SPEECH_THRESHOLD: f32 = 0.6;
const LOG_PROB_THRESHOLD: f32 = -1.0;
const KEEP_LOGPROB: f32 = -1.5; // `_whisper`: segments below this are dropped
const LANGS: [&str; 2] = ["fr", "en"];

fn bad<E: std::fmt::Display>(e: E) -> InferError {
    InferError::new(InferErrorKind::Bad, e.to_string())
}

// ---------------------------------------------------------------- log-mel

fn hz_to_mel(f: f64) -> f64 {
    if f >= 1000.0 { 15.0 + (f / 1000.0).ln() / (6.4f64.ln() / 27.0) } else { f / (200.0 / 3.0) }
}
fn mel_to_hz(m: f64) -> f64 {
    if m >= 15.0 { 1000.0 * ((m - 15.0) * (6.4f64.ln() / 27.0)).exp() } else { m * 200.0 / 3.0 }
}

/// faster-whisper `FeatureExtractor` (librosa slaney mel filters, 128 bins).
pub struct MelBank {
    fft: Arc<dyn Fft<f64>>,
    win: Vec<f32>,
    bands: Vec<(usize, Vec<f64>)>, // per mel bin: first FFT bin, non-zero weights
}

impl MelBank {
    pub fn new() -> Self {
        let nf = N_FFT / 2 + 1;
        let (lo, hi) = (hz_to_mel(0.0), hz_to_mel(8000.0));
        let mel_f: Vec<f64> = (0..MELS + 2).map(|i| mel_to_hz(lo + (hi - lo) * i as f64 / (MELS + 1) as f64)).collect();
        let bands = (0..MELS)
            .map(|i| {
                let enorm = 2.0 / (mel_f[i + 2] - mel_f[i]);
                let w: Vec<f64> = (0..nf)
                    .map(|k| {
                        let f = 8000.0 * k as f64 / (nf - 1) as f64;
                        let lower = (f - mel_f[i]) / (mel_f[i + 1] - mel_f[i]);
                        let upper = (mel_f[i + 2] - f) / (mel_f[i + 2] - mel_f[i + 1]);
                        lower.min(upper).max(0.0) * enorm
                    })
                    .collect();
                let first = w.iter().position(|v| *v > 0.0).unwrap_or(0);
                let last = w.iter().rposition(|v| *v > 0.0).unwrap_or(0);
                (first, w[first..=last].to_vec())
            })
            .collect();
        let win = (0..N_FFT).map(|i| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / N_FFT as f64).cos()) as f32).collect();
        Self { fft: FftPlanner::<f64>::new().plan_fft_forward(N_FFT), win, bands }
    }

    /// Log-mel of `pcm` (16 kHz): `(values [mel * frames + t], frames)` with `frames = len / 160 + 1`, not padded to 30 s.
    pub fn compute(&self, pcm: &[f32]) -> (Vec<f32>, usize) {
        let mut x: Vec<f32> = pcm.to_vec();
        x.extend(std::iter::repeat_n(0.0, HOP));
        let p = N_FFT / 2;
        let n = x.len();
        let padded: Vec<f32> = (0..n + 2 * p)
            .map(|i| {
                let j = i as isize - p as isize;
                let j = if j < 0 { -j } else if j >= n as isize { 2 * (n as isize - 1) - j } else { j };
                x[j.clamp(0, n as isize - 1) as usize]
            })
            .collect();
        let frames = (1 + (padded.len() - N_FFT) / HOP).saturating_sub(1);
        let nf = N_FFT / 2 + 1;
        let mut spec = vec![0f32; MELS * frames];
        let mut buf = vec![Complex::zero(); N_FFT];
        let mut mag = vec![0f64; nf];
        for t in 0..frames {
            for i in 0..N_FFT {
                buf[i] = Complex::new((padded[t * HOP + i] * self.win[i]) as f64, 0.0); // product in f32 like numpy, FFT in f64 (exact)
            }
            self.fft.process(&mut buf);
            for k in 0..nf {
                mag[k] = buf[k].norm_sqr();
            }
            for (m, (first, w)) in self.bands.iter().enumerate() {
                let s: f64 = w.iter().zip(&mag[*first..]).map(|(a, b)| a * b).sum();
                spec[m * frames + t] = s.max(1e-10).log10() as f32;
            }
        }
        let max = spec.iter().cloned().fold(f32::MIN, f32::max);
        for v in &mut spec {
            *v = (v.max(max - 8.0) + 4.0) / 4.0;
        }
        (spec, frames)
    }
}

impl Default for MelBank {
    fn default() -> Self {
        Self::new()
    }
}

/// `features[:, seek:seek+size]` zero-padded to 3000 frames (`pad_or_trim`), laid out `[mel][frame]`.
fn window(feats: &[f32], frames: usize, seek: usize, size: usize) -> Vec<f32> {
    let mut out = vec![0f32; MELS * FRAMES];
    let size = size.min(FRAMES);
    for m in 0..MELS {
        out[m * FRAMES..m * FRAMES + size].copy_from_slice(&feats[m * frames + seek..m * frames + seek + size]);
    }
    out
}

// ---------------------------------------------------------------- pure decoding logic

/// One piece of text found by splitting the tokens on timestamps.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub start: f64,
    pub end: f64,
    pub tokens: Vec<u32>,
}

/// faster-whisper `_split_segments_by_timestamps`: returns the pieces, the new `seek` (in mel frames) and
/// whether the tokens ended with a single timestamp.
pub fn split_by_timestamps(tokens: &[u32], ts_begin: u32, time_offset: f64, segment_size: usize, segment_duration: f64, mut seek: usize) -> (Vec<Piece>, usize, bool) {
    let mut pieces = vec![];
    let single_ending = tokens.len() >= 2 && tokens[tokens.len() - 2] < ts_begin && ts_begin <= tokens[tokens.len() - 1];
    let consecutive: Vec<usize> = (1..tokens.len()).filter(|&i| tokens[i] >= ts_begin && tokens[i - 1] >= ts_begin).collect();
    if !consecutive.is_empty() {
        let mut slices = consecutive;
        if single_ending {
            slices.push(tokens.len());
        }
        let mut last = 0;
        for cur in slices {
            let sliced = &tokens[last..cur];
            let s = (sliced[0] - ts_begin) as f64;
            let e = (sliced[sliced.len() - 1] - ts_begin) as f64;
            pieces.push(Piece { start: time_offset + s * TIME_PRECISION, end: time_offset + e * TIME_PRECISION, tokens: sliced.to_vec() });
            last = cur;
        }
        if single_ending {
            seek += segment_size;
        } else {
            seek += (tokens[last - 1] - ts_begin) as usize * INPUT_STRIDE;
        }
    } else {
        let mut duration = segment_duration;
        if let Some(&ts) = tokens.iter().rev().find(|&&t| t >= ts_begin) {
            if ts != ts_begin {
                duration = (ts - ts_begin) as f64 * TIME_PRECISION;
            }
        }
        pieces.push(Piece { start: time_offset, end: time_offset + duration, tokens: tokens.to_vec() });
        seek += segment_size;
    }
    (pieces, seek, single_ending)
}

/// `avg_logprob` from CTranslate2's score (already divided by the length, length_penalty = 1).
pub fn avg_logprob(score: f32, n_tokens: usize) -> f32 {
    let n = n_tokens as f32;
    score * n / (n + 1.0)
}

/// faster-whisper's no-speech rule: skip the window when `no_speech_prob > 0.6` unless the text is confident enough.
pub fn is_silence(no_speech_prob: f32, avg_logprob: f32) -> bool {
    no_speech_prob > NO_SPEECH_THRESHOLD && avg_logprob <= LOG_PROB_THRESHOLD
}

/// `_whisper`: keep segments with `avg_logprob > -1.5`, join their texts with a space, strip.
pub fn join_segments(segments: &[(String, f32)]) -> String {
    segments.iter().filter(|(_, lp)| *lp > KEEP_LOGPROB).map(|(t, _)| t.as_str()).collect::<Vec<_>>().join(" ").trim().to_string()
}

fn lang_code(token: &str) -> String {
    token.trim_start_matches("<|").trim_end_matches("|>").to_string()
}

// ---------------------------------------------------------------- engine

fn encode_ids(tok: &Tokenizer, text: &str) -> Result<Vec<u32>, InferError> {
    Ok(tok.encode(text, false).map_err(bad)?.get_ids().to_vec())
}

/// faster-whisper `get_suppressed_tokens(tokenizer, [-1])`.
pub fn suppressed_ids(tok: &Tokenizer) -> Result<Vec<i32>, InferError> {
    let mut set = std::collections::BTreeSet::new();
    set.insert(encode_ids(tok, " -")?[0]);
    set.insert(encode_ids(tok, " '")?[0]);
    let mut symbols: Vec<String> = "\"#()*+/:;<=>@[\\]^_`{|}~「」『』".chars().map(String::from).collect();
    symbols.extend("<< >> <<< >>> -- --- -( -[ (' (\" (( )) ((( ))) [[ ]] {{ }} ♪♪ ♪♪♪".split(' ').map(String::from));
    let misc = "♩♪♫♬♭♮♯";
    symbols.extend(misc.chars().map(String::from));
    for s in &symbols {
        let is_misc = misc.contains(s.as_str());
        for text in [s.clone(), format!(" {s}")] {
            let ids = encode_ids(tok, &text)?;
            if !ids.is_empty() && (ids.len() == 1 || is_misc) {
                set.insert(ids[0]);
            }
        }
    }
    for name in ["<|transcribe|>", "<|translate|>", "<|startoftranscript|>", "<|startofprev|>", "<|startoflm|>", "<|nospeech|>", "<|nocaptions|>"] {
        if let Some(i) = tok.token_to_id(name) {
            set.insert(i);
        }
    }
    Ok(set.into_iter().map(|i| i as i32).collect())
}

struct Engine {
    whisper: Whisper,
    tok: Tokenizer,
    vad: Vad,
    mel: MelBank,
    suppress: Vec<i32>,
    eot: u32,
    ts_begin: u32,
}

impl Engine {
    fn id(&self, t: &str) -> Option<u32> {
        self.tok.token_to_id(t)
    }

    fn encode_text(&self, text: &str) -> Result<Vec<u32>, InferError> {
        encode_ids(&self.tok, text)
    }

    fn encode_window(&self, feats: &[f32], frames: usize, seek: usize, size: usize) -> Result<StorageView<'static>, InferError> {
        let mut w = window(feats, frames, seek, size);
        let sv = StorageView::new(&[1, MELS, FRAMES], &mut w, Device::CPU).map_err(bad)?;
        self.whisper.encode(&sv, false).map_err(bad)
    }

    /// `get_prompt` without previous text: `<|startofprev|> hotwords` (if any) then `<|startoftranscript|> <lang> <|transcribe|>`.
    fn prompt(&self, lang: &str, hotwords: &str) -> Result<Vec<String>, InferError> {
        let mut p: Vec<String> = vec![];
        if !hotwords.is_empty() {
            p.push("<|startofprev|>".into());
            let mut ids = self.encode_text(&format!(" {}", hotwords.trim()))?;
            if ids.len() >= MAX_LENGTH / 2 {
                ids.truncate(MAX_LENGTH / 2 - 1);
            }
            for i in ids {
                p.push(self.tok.id_to_token(i).ok_or_else(|| bad("unknown hotword token"))?);
            }
        }
        p.extend(["<|startoftranscript|>".to_string(), format!("<|{lang}|>"), "<|transcribe|>".to_string()]);
        Ok(p)
    }

    /// `generate_segments` (clip_timestamps "0", no word timestamps): the texts with their window's avg_logprob.
    fn decode_windows(&self, feats: &[f32], frames: usize, lang: &str, hotwords: &str) -> Result<Vec<(String, f32)>, InferError> {
        let content = frames - 1;
        let prompt = self.prompt(lang, hotwords)?;
        if std::env::var_os("STT_DEBUG").is_some() {
            eprintln!("[stt] prompt {:?} ids {:?}", prompt, prompt.iter().map(|t| self.id(t)).collect::<Vec<_>>());
        }
        let opts = WhisperOptions { beam_size: 1, return_scores: true, return_no_speech_prob: true, suppress_tokens: self.suppress.clone(), ..Default::default() };
        let mut out = vec![];
        let mut seek = 0usize;
        while seek < content {
            let size = FRAMES.min(content - seek);
            // Always a fresh encode: reusing the output that language detection already saw changed tokens.
            let enc = self.encode_window(feats, frames, seek, size)?;
            let r = self.whisper.generate(&enc, std::slice::from_ref(&prompt), &opts).map_err(bad)?;
            let r = r.first().ok_or_else(|| bad("empty generation result"))?;
            let ids: Vec<u32> = r.sequences_ids.first().map(|v| v.iter().map(|&i| i as u32).collect()).unwrap_or_default();
            let avg = avg_logprob(r.scores.first().copied().unwrap_or(0.0), ids.len());
            if std::env::var_os("STT_DEBUG").is_some() {
                eprintln!("[stt] seek {seek} lang {lang} no_speech {:.3} avg_logprob {avg:.3} tokens {ids:?}", r.no_speech_prob);
            }
            if is_silence(r.no_speech_prob, avg) {
                seek += size;
                continue;
            }
            let previous = seek;
            let (pieces, next, _) = split_by_timestamps(&ids, self.ts_begin, seek as f64 * TIME_PER_FRAME, size, size as f64 * TIME_PER_FRAME, seek);
            seek = if next > previous { next } else { previous + size }; // never loop on a zero-length step
            for p in pieces {
                let text_ids: Vec<u32> = p.tokens.iter().copied().filter(|&t| t < self.eot).collect();
                let text = self.tok.decode(&text_ids, true).map_err(bad)?;
                if p.start == p.end || text.trim().is_empty() {
                    continue;
                }
                out.push((text, avg));
            }
        }
        Ok(out)
    }

    fn run(&mut self, pcm: &[f32], hotwords: &str) -> Result<(String, String), InferError> {
        let empty = || Ok((String::new(), "fr".to_string()));
        if pcm.is_empty() {
            return empty();
        }
        let chunks = self.vad.speech_chunks(pcm, &VadOptions::default()).map_err(bad)?;
        let audio = collect_chunks(pcm, &chunks);
        if audio.len() < HOP {
            return empty();
        }
        if std::env::var_os("STT_DEBUG").is_some() {
            eprintln!("[stt] vad {chunks:?} of {} samples", pcm.len());
        }
        let (feats, frames) = self.mel.compute(&audio);
        if std::env::var_os("STT_DEBUG").is_some() {
            let head: f64 = (0..MELS).flat_map(|m| (0..100.min(frames)).map(move |t| (m, t))).map(|(m, t)| feats[m * frames + t] as f64).sum();
            eprintln!("[stt] mel frames {frames} head100 sum {head:.4} total {:.4}", feats.iter().map(|v| *v as f64).sum::<f64>());
        }
        let size = FRAMES.min(frames - 1);
        let enc = self.encode_window(&feats, frames, 0, size)?;
        let det = self.whisper.detect_language(&enc).map_err(bad)?;
        let probs: Vec<(String, f32)> = det.first().map(|d| d.iter().map(|r| (lang_code(&r.language), r.probability)).collect()).unwrap_or_default();
        let mut lang = probs.first().map(|p| p.0.clone()).unwrap_or_else(|| "fr".into());
        let hot = hotwords;
        if !LANGS.contains(&lang.as_str()) {
            // Rare misdetection: redo in the likelier of fr/en.
            let p = |l: &str| probs.iter().find(|(c, _)| c == l).map(|(_, v)| *v).unwrap_or(0.0);
            lang = if p(LANGS[1]) > p(LANGS[0]) { LANGS[1] } else { LANGS[0] }.to_string();
        }
        let segs = self.decode_windows(&feats, frames, &lang, hot)?;
        Ok((join_segments(&segs), lang))
    }
}

/// Whisper large-v3-turbo on the GPU (int8), one transcription at a time.
#[derive(Clone)]
pub struct LocalStt {
    engine: Arc<Mutex<Engine>>,
}

// SAFETY: the CTranslate2 model and the ONNX session are only touched while holding the Mutex.
unsafe impl Send for Engine {}

impl LocalStt {
    /// `whisper_dir`: a faster-whisper model folder (model.bin, tokenizer.json...); `vad_model_path`: silero_vad_v6.onnx.
    pub fn new(whisper_dir: &Path, vad_model_path: &Path) -> Result<Self, InferError> {
        let vad = Vad::new(vad_model_path).map_err(|e| InferError::new(InferErrorKind::Down, e))?;
        let cfg = Config { device: Device::CUDA, compute_type: ComputeType::INT8, ..Default::default() };
        let whisper = Whisper::new(whisper_dir, cfg).map_err(|e| InferError::new(InferErrorKind::Down, e.to_string()))?;
        let tok = Tokenizer::new(whisper_dir).map_err(bad)?;
        let eot = tok.token_to_id("<|endoftext|>").ok_or_else(|| bad("no <|endoftext|> token"))?;
        let ts_begin = tok.token_to_id("<|0.00|>").ok_or_else(|| bad("no <|0.00|> token"))?;
        let mut engine = Engine { whisper, tok, vad, mel: MelBank::new(), suppress: vec![], eot, ts_begin };
        engine.suppress = suppressed_ids(&engine.tok)?;
        let stt = Self { engine: Arc::new(Mutex::new(engine)) };
        {
            // Warm up the GPU (silence would stop at the VAD, so go straight to the model).
            let e = stt.engine.lock().map_err(|_| bad("STT engine poisoned"))?;
            let (feats, frames) = e.mel.compute(&vec![0.0; 16000]);
            let enc = e.encode_window(&feats, frames, 0, frames - 1)?;
            e.whisper.detect_language(&enc).map_err(bad)?;
        }
        Ok(stt)
    }

    /// Blocking: `(text, lang)` exactly like `_whisper`. Empty audio or no speech: `("", "fr")`.
    pub fn run(&self, pcm16k: &[f32], hotwords: &str) -> Result<(String, String), InferError> {
        self.engine.lock().map_err(|_| bad("STT engine poisoned"))?.run(pcm16k, hotwords)
    }
}

impl Transcribe for LocalStt {
    async fn transcribe(&self, pcm16k: &[f32], hotwords: &str) -> Result<Transcript, InferError> {
        let me = self.clone();
        let (pcm, hot) = (pcm16k.to_vec(), hotwords.to_string());
        tokio::task::spawn_blocking(move || {
            let t = Instant::now();
            let (text, lang) = me.run(&pcm, &hot)?;
            Ok(Transcript { text, lang, ms: t.elapsed().as_millis() as u32 })
        })
        .await
        .map_err(bad)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TB: u32 = 100;

    #[test]
    fn mel_matches_python() {
        let x: Vec<f32> = (0..8000)
            .map(|i| {
                let i = i as f64;
                (0.3 * (2.0 * std::f64::consts::PI * 440.0 * i / 16000.0).sin() + 0.1 * (2.0 * std::f64::consts::PI * 1234.5 * i / 16000.0).sin()) as f32
            })
            .collect();
        let (m, t) = MelBank::new().compute(&x);
        assert_eq!(t, 51);
        // (mel, frame, value) from faster_whisper FeatureExtractor(feature_size=128)
        for (mi, ti, v) in [(0, 0, 0.8202394843101501), (5, 10, -0.6255688667297363), (20, 25, 1.1764706373214722), (64, 3, -0.6255688667297363), (100, 49, -0.5119251012802124), (127, 50, -0.11640715599060059), (30, 1, 0.2638416886329651), (2, 30, -0.6255688667297363)] {
            let got = m[mi * t + ti] as f64;
            assert!((got - v).abs() < 1e-3, "mel[{mi}][{ti}] = {got}, python {v}");
        }
        let sum: f64 = m.iter().map(|v| *v as f64).sum();
        assert!((sum - -2612.732421875).abs() < 0.5, "sum {sum}");
    }

    #[test]
    fn split_consecutive_timestamps_and_single_ending() {
        let (p, seek, single) = split_by_timestamps(&[100, 5, 6, 110, 110, 7, 120], TB, 0.0, 3000, 30.0, 0);
        assert!(single);
        assert_eq!(seek, 3000);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0], Piece { start: 0.0, end: 0.2, tokens: vec![100, 5, 6, 110] });
        assert_eq!(p[1].tokens, vec![110, 7, 120]);
        assert!((p[1].start - 0.2).abs() < 1e-9 && (p[1].end - 0.4).abs() < 1e-9);
    }

    #[test]
    fn split_consecutive_without_single_ending_moves_seek() {
        let (p, seek, single) = split_by_timestamps(&[100, 5, 110, 110, 7, 8], TB, 1.0, 3000, 30.0, 50);
        assert!(!single);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].tokens, vec![100, 5, 110]);
        assert_eq!(seek, 50 + 10 * 2);
    }

    #[test]
    fn split_without_consecutive_timestamps() {
        let (p, seek, _) = split_by_timestamps(&[5, 6, 105], TB, 2.0, 400, 4.0, 10);
        assert_eq!(seek, 410);
        assert_eq!(p.len(), 1);
        assert!((p[0].start - 2.0).abs() < 1e-9 && (p[0].end - 2.1).abs() < 1e-9);
        let (p, _, _) = split_by_timestamps(&[5, 6], TB, 0.0, 400, 4.0, 0);
        assert!((p[0].end - 4.0).abs() < 1e-9);
    }

    #[test]
    fn logprob_and_silence_rules() {
        assert!((avg_logprob(-0.5, 9) - -0.45).abs() < 1e-6);
        assert!(is_silence(0.7, -1.2));
        assert!(!is_silence(0.7, -0.5)); // confident enough despite no_speech_prob
        assert!(!is_silence(0.5, -2.0)); // below the no-speech threshold: kept at this stage
        let segs = vec![(" Bonjour".to_string(), -0.3), (" bruit".to_string(), -1.6), (" Bobby".to_string(), -1.4)];
        assert_eq!(join_segments(&segs), "Bonjour  Bobby");
        assert_eq!(join_segments(&[]), "");
    }
}
