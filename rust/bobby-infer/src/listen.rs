//! In-process replacement of the voice service's `/listen` WebSocket (Discord sentences): live wake-word scan while
//! the sentence is spoken, gate at the end, transcription of the sentences meant for Bobby, training clip collection.
//! Port of `voice/server.py::listen` + `voice/wakescan.py`; the scores must match Python's (`examples/gate_compare.rs`).
//!
//! Wake scoring is CPU work (about 1 ms per 80 ms frame): every scoring step runs in `tokio::task::spawn_blocking`
//! (works on any runtime flavour, unlike `block_in_place`), so the async executor never waits on the model.
use crate::collect::collect;
use crate::wake::Wake;
use bobby_proto::infer::{Transcribe, Transcript};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::UnboundedReceiver;

const FRAME: usize = 1280;
const PAD: usize = 8000; // Discord sends no audio during silence: give the model silence on both sides
// wakescan.py constants, in i16 samples (the Python ones are bytes: halve them).
const ALIGN: usize = 6; // 3 stereo frames at 48 kHz = 1 sample at 16 kHz
const CTX: usize = 19200; // 0.2 s already converted, resampled again so the filter has its history
const HOLD: usize = 9600; // 0.1 s kept for the next chunk: the filter needs what comes after too

// ---------- scipy.signal.resample_poly(x, 1, 3) ----------

const HALF: usize = 30; // 10 * max(up, down)

fn i0(x: f64) -> f64 {
    let (mut sum, mut term, q) = (1.0f64, 1.0f64, x * x / 4.0);
    for k in 1..200 {
        term *= q / (k * k) as f64;
        sum += term;
        if term < sum * 1e-18 {
            break;
        }
    }
    sum
}

fn sinc(x: f64) -> f64 {
    if x == 0.0 {
        1.0
    } else {
        let a = std::f64::consts::PI * x;
        a.sin() / a
    }
}

/// `firwin(61, 1/3, window=('kaiser', 5.0))`, what scipy designs for `resample_poly(x, 1, 3)`.
fn taps() -> &'static [f64] {
    static H: std::sync::OnceLock<Vec<f64>> = std::sync::OnceLock::new();
    H.get_or_init(|| {
        let (n, cutoff, beta) = (2 * HALF + 1, 1.0 / 3.0, 5.0);
        let alpha = HALF as f64;
        let mut h: Vec<f64> = (0..n)
            .map(|i| {
                let m = i as f64 - alpha;
                let r = (m / alpha).powi(2);
                let win = i0(beta * (1.0 - r).max(0.0).sqrt()) / i0(beta);
                cutoff * sinc(cutoff * m) * win
            })
            .collect();
        let s: f64 = h.iter().sum();
        h.iter_mut().for_each(|v| *v /= s);
        h
    })
}

/// Exact port of scipy `resample_poly(x, 1, 3)`: output `k` is the filter centred on input sample `3k`
/// (zero outside the signal), `ceil(len / 3)` outputs.
pub fn resample_poly_1_3(x: &[f64]) -> Vec<f64> {
    let h = taps();
    (0..x.len().div_ceil(3))
        .map(|k| {
            let c = 3 * k;
            let lo = c.saturating_sub(HALF);
            let hi = (c + HALF).min(x.len() - 1);
            (lo..=hi).map(|i| h[c + HALF - i] * x[i]).sum()
        })
        .collect()
}

/// s16 interleaved stereo at 48 kHz (decoded Discord audio) -> f32 mono at 16 kHz (`wakescan.to_16k`).
pub fn to_16k(raw: &[i16]) -> Vec<f32> {
    let mono: Vec<f64> = raw.chunks_exact(2).map(|c| (f64::from(c[0]) + f64::from(c[1])) / 2.0 / 32768.0).collect();
    resample_poly_1_3(&mono).into_iter().map(|v| v as f32).collect()
}

// ---------- WakeScan ----------

/// Something that scores 80 ms frames (1280 samples, 16 kHz s16). Implemented by `Wake`; tests use fakes.
pub trait Scorer {
    fn reset(&mut self);
    fn predict(&mut self, frame: &[i16]) -> f32;
}

impl Scorer for Wake {
    fn reset(&mut self) {
        Wake::reset(self)
    }
    fn predict(&mut self, frame: &[i16]) -> f32 {
        Wake::predict(self, frame)
    }
}

impl<M: Scorer + ?Sized> Scorer for &mut M {
    fn reset(&mut self) {
        (**self).reset()
    }
    fn predict(&mut self, frame: &[i16]) -> f32 {
        (**self).predict(frame)
    }
}

impl Scorer for Box<dyn Scorer + Send> {
    fn reset(&mut self) {
        (**self).reset()
    }
    fn predict(&mut self, frame: &[i16]) -> f32 {
        (**self).predict(frame)
    }
}

/// Wake word scoring fed while the sentence is still being spoken: the same score as scanning the whole sentence
/// at once (silence before and after, 1280-sample frames, highest score). Only the new audio is converted at each
/// call, so the cost stays linear in the sentence length.
pub struct WakeScan<M> {
    model: M,
    pub score: f32,
    done: usize,   // samples of `raw` already converted
    buf: Vec<i16>, // 16 kHz samples not fed to the model yet
}

impl<M: Scorer> WakeScan<M> {
    pub fn new(mut model: M) -> Self {
        model.reset();
        Self { model, score: 0.0, done: 0, buf: vec![0; PAD] }
    }

    /// `raw`: the whole sentence so far (48 kHz stereo s16, interleaved). `last`: the sentence is over.
    pub fn feed(&mut self, raw: &[i16], last: bool) -> f32 {
        let mut end = if last { raw.len() } else { raw.len().saturating_sub(HOLD) };
        end -= end % ALIGN;
        if end > self.done {
            let mut start = self.done.saturating_sub(CTX);
            start -= start % ALIGN;
            let new = to_16k(&raw[start..end]);
            self.buf.extend(new[(self.done - start) / ALIGN..].iter().map(|v| (v.clamp(-1.0, 1.0) * 32767.0) as i16));
            self.done = end;
        }
        if last {
            self.buf.resize(self.buf.len() + PAD, 0);
        }
        let n = self.buf.len() / FRAME * FRAME;
        for frame in self.buf[..n].chunks_exact(FRAME) {
            self.score = self.score.max(self.model.predict(frame));
        }
        self.buf.drain(..n);
        self.score
    }

    pub fn into_model(self) -> M {
        self.model
    }
}

// ---------- Listener ----------

/// The gate message of the voice service: `wake` is `None` when the gate was bypassed (`force`, or no wake model).
#[derive(Debug, Clone, PartialEq)]
pub struct Gate {
    pub wake: Option<bool>,
    pub score: Option<f32>,
}

#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    pub gate: Option<Gate>,
    /// `None` when the gate said this was not for Bobby.
    pub transcript: Option<Transcript>,
}

/// `WAKE_THRESHOLD` (default 0.10), like voice/server.py.
pub fn wake_threshold() -> f32 {
    std::env::var("WAKE_THRESHOLD").ok().and_then(|v| v.parse().ok()).unwrap_or(0.10)
}

fn round3(v: f32) -> f32 {
    (v * 1000.0).round() / 1000.0
}

pub type Model = Box<dyn Scorer + Send>;
pub type Factory = Arc<dyn Fn() -> Result<Model, String> + Send + Sync>;
type Scan = WakeScan<Model>;

pub struct Listener<T> {
    stt: Arc<T>,
    threshold: f32,
    collect_root: Option<PathBuf>,
    pool: Mutex<Vec<Model>>,  // a Wake keeps state: each live sentence needs its own, idle ones wait here
    factory: Option<Factory>, // None: no wake model, nothing is gated
}

impl<T: Transcribe + Send + Sync + 'static> Listener<T> {
    /// `wake_dir`: the folder with `melspectrogram.onnx`, `embedding_model.onnx`, `bobby.onnx` (None or unloadable:
    /// no gate, like a voice service without wake models). `collect_root`: project root for `_training/collected`.
    pub fn new(stt: Arc<T>, wake_dir: Option<&Path>, threshold: f32, collect_root: Option<PathBuf>) -> Self {
        let mut me = Self::with_scorer(stt, None, threshold, collect_root);
        if let Some(dir) = wake_dir {
            let dir = dir.to_path_buf();
            match Wake::new(&dir) {
                Ok(w) => {
                    me.pool.get_mut().unwrap().push(Box::new(w));
                    me.factory = Some(Arc::new(move || Wake::new(&dir).map(|w| Box::new(w) as Model)));
                }
                Err(e) => eprintln!("[listen] no wake model, sentences are not gated: {e}"),
            }
        }
        me
    }

    /// Same with any scorer factory (tests).
    pub fn with_scorer(stt: Arc<T>, factory: Option<Factory>, threshold: f32, collect_root: Option<PathBuf>) -> Self {
        Self { stt, threshold, collect_root, pool: Mutex::new(vec![]), factory }
    }

    async fn model(&self) -> Result<Model, String> {
        if let Some(m) = self.pool.lock().unwrap().pop() {
            return Ok(m);
        }
        let make = self.factory.clone().ok_or("no wake model")?;
        tokio::task::spawn_blocking(move || make()).await.map_err(|e| e.to_string())?
    }

    /// One sentence. `pcm`: decoded audio chunks (48 kHz stereo s16, interleaved); the channel closing means the
    /// sentence ended. `ring(true)` when the wake word is heard mid-sentence, `ring(false)` at the gate.
    pub async fn listen(&self, mut pcm: UnboundedReceiver<Vec<i16>>, force: bool, hotwords: &str, ring: impl Fn(bool)) -> Result<Outcome, String> {
        let mut scan: Option<Scan> = if self.factory.is_some() && !force { Some(WakeScan::new(self.model().await?)) } else { None };
        let res = self.run(&mut pcm, &mut scan, hotwords, &ring).await;
        if let Some(s) = scan {
            self.pool.lock().unwrap().push(s.into_model());
        }
        res
    }

    async fn run(&self, pcm: &mut UnboundedReceiver<Vec<i16>>, scan: &mut Option<Scan>, hotwords: &str, ring: &impl Fn(bool)) -> Result<Outcome, String> {
        let mut raw: Vec<i16> = vec![];
        let mut woke = false;
        while let Some(chunk) = pcm.recv().await {
            raw.extend(chunk);
            if woke || scan.is_none() {
                continue;
            }
            let (mut s, r) = (scan.take().unwrap(), std::mem::take(&mut raw));
            let (s, r, score) = tokio::task::spawn_blocking(move || {
                let score = s.feed(&r, false);
                (s, r, score)
            })
            .await
            .map_err(|e| e.to_string())?;
            (*scan, raw) = (Some(s), r);
            if score >= self.threshold {
                woke = true; // the ring can light up while the person is still talking
                ring(true);
            }
        }
        // the sentence is over: last scoring step, and the 16 kHz conversion of the whole audio
        let (s, r) = (scan.take(), raw);
        let (s, audio, score) = tokio::task::spawn_blocking(move || {
            let audio = if r.len() * 2 >= 4 { to_16k(&r) } else { vec![] };
            match s {
                Some(mut s) => {
                    let score = round3(s.feed(&r, true));
                    (Some(s), audio, Some(score))
                }
                None => (None, audio, None),
            }
        })
        .await
        .map_err(|e| e.to_string())?;
        *scan = s;
        let passed = score.map(|s| s >= self.threshold);
        ring(false);
        let mut outcome = Outcome { gate: Some(Gate { wake: passed, score }), transcript: None };
        if let Some(wake) = passed {
            eprintln!("[wake] {} (score {:.2})", if wake { "Bobby heard" } else { "no Bobby" }, score.unwrap_or(0.0));
        }
        if passed == Some(false) {
            self.keep(audio, score, None); // a near-miss (collect keeps only score >= COLLECT_MIN_SCORE)
            return Ok(outcome);
        }
        if audio.is_empty() {
            // nothing was said (forced sentence of zero length): no engine call, no clip
            outcome.transcript = Some(Transcript { text: String::new(), lang: String::new(), ms: 0 });
            return Ok(outcome);
        }
        let t = self.stt.transcribe(&audio, hotwords).await.map_err(|e| e.to_string())?;
        eprintln!("[stt] {} ms  {:.1} s of audio  {}  {:?}", t.ms, audio.len() as f32 / 16000.0, t.lang, t.text);
        self.keep(audio, score, Some(t.text.clone()));
        outcome.transcript = Some(t);
        Ok(outcome)
    }

    /// Training clip (file write on the blocking pool, never fails the sentence).
    fn keep(&self, audio: Vec<f32>, score: Option<f32>, text: Option<String>) {
        if let Some(root) = self.collect_root.clone() {
            tokio::task::spawn_blocking(move || collect(&root, &audio, score, text.as_deref(), "discord"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bobby_proto::infer::InferError;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn resample_matches_scipy() {
        let x: Vec<f64> = (0..300).map(|n| ((n * 7919) % 2001) as f64 / 1000.0 - 1.0 + 0.5 * (0.05 * n as f64).sin()).collect();
        let want: Vec<f64> = include_str!("../testdata/resample_ref.txt").lines().map(|l| l.parse().unwrap()).collect();
        let got = resample_poly_1_3(&x);
        assert_eq!(got.len(), want.len());
        let worst = got.iter().zip(&want).map(|(a, b)| (a - b).abs()).fold(0.0, f64::max);
        assert!(worst <= 1e-5, "max abs diff {worst}");
        println!("resample max abs diff {worst:e}");
    }

    #[test]
    fn to_16k_lengths() {
        assert_eq!(to_16k(&[]).len(), 0);
        assert_eq!(to_16k(&vec![0; 2 * 301]).len(), 101);
    }

    /// Scores a frame by its loudest sample (0..1).
    struct Peak;
    impl Scorer for Peak {
        fn reset(&mut self) {}
        fn predict(&mut self, frame: &[i16]) -> f32 {
            frame.iter().map(|v| v.unsigned_abs()).max().unwrap_or(0) as f32 / 32768.0
        }
    }

    #[test]
    fn live_scan_equals_whole_sentence_scan() {
        let raw: Vec<i16> = (0..2 * 48000).map(|i| if (i / 2) % 7000 < 100 { 9000 } else { ((i * 31) % 200) as i16 - 100 }).collect();
        let mut live = WakeScan::new(Peak);
        for end in (1920..=raw.len()).step_by(1920) {
            live.feed(&raw[..end], false);
        }
        let live = live.feed(&raw, true);
        let whole = WakeScan::new(Peak).feed(&raw, true);
        assert_eq!(live, whole);
    }

    struct Fake {
        hotwords: Mutex<Vec<String>>,
        calls: AtomicUsize,
    }
    impl Transcribe for Fake {
        async fn transcribe(&self, pcm16k: &[f32], hotwords: &str) -> Result<Transcript, InferError> {
            assert!(!pcm16k.is_empty());
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.hotwords.lock().unwrap().push(hotwords.to_string());
            Ok(Transcript { text: "bonjour Bobby".into(), lang: "fr".into(), ms: 7 })
        }
    }

    fn fake() -> Arc<Fake> {
        Arc::new(Fake { hotwords: Mutex::new(vec![]), calls: AtomicUsize::new(0) })
    }

    fn listener(stt: &Arc<Fake>, root: Option<PathBuf>) -> Listener<Fake> {
        Listener::with_scorer(stt.clone(), Some(Arc::new(|| Ok(Box::new(Peak) as Model))), 0.10, root)
    }

    /// 1 s of constant stereo audio at `level` (peak 0..1), sent as 20 ms chunks.
    fn sentence(level: f32, seconds: usize) -> UnboundedReceiver<Vec<i16>> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let v = (level * 32767.0) as i16;
        for _ in 0..50 * seconds {
            tx.send(vec![v; 1920]).unwrap();
        }
        rx
    }

    async fn run(l: &Listener<Fake>, rx: UnboundedReceiver<Vec<i16>>, force: bool) -> (Outcome, Vec<bool>) {
        let rings = Mutex::new(vec![]);
        let out = l.listen(rx, force, "Bobby Alex", |on| rings.lock().unwrap().push(on)).await.unwrap();
        (out, rings.into_inner().unwrap())
    }

    fn collected(root: &Path) -> Vec<String> {
        let Ok(days) = std::fs::read_dir(root.join("_training/collected")) else { return vec![] };
        let mut v: Vec<String> = days.flatten().flat_map(|d| std::fs::read_dir(d.path()).unwrap().flatten().map(|f| f.file_name().into_string().unwrap())).collect();
        v.sort();
        v
    }

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("bobby-listen-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    async fn settle() {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await; // collect runs on the blocking pool
    }

    #[tokio::test]
    async fn forced_sentence_is_transcribed_and_collected() {
        let (stt, root) = (fake(), tmp("forced"));
        let l = listener(&stt, Some(root.clone()));
        let (out, rings) = run(&l, sentence(0.0, 1), true).await;
        assert_eq!(out.gate, Some(Gate { wake: None, score: None }));
        assert_eq!(out.transcript.unwrap().text, "bonjour Bobby");
        assert_eq!(rings, [false]);
        assert_eq!(*stt.hotwords.lock().unwrap(), ["Bobby Alex"]);
        settle().await;
        let f = collected(&root);
        assert!(f.iter().any(|n| n.ends_with("_discord_forced.wav")) && f.iter().any(|n| n.ends_with("_discord_forced.txt")), "{f:?}");
    }

    #[tokio::test]
    async fn gate_fail_skips_transcription() {
        let (stt, root) = (fake(), tmp("fail"));
        let l = listener(&stt, Some(root.clone()));
        let (out, rings) = run(&l, sentence(0.02, 1), false).await; // score 0.02: below 0.10, above the collect minimum
        let g = out.gate.unwrap(); // the resampling filter overshoots a little on a step: 0.02 reads 0.022
        assert_eq!(g.wake, Some(false));
        let s = g.score.unwrap();
        assert!((0.02..0.03).contains(&s), "{s}");
        assert!(out.transcript.is_none());
        assert_eq!(rings, [false]);
        assert_eq!(stt.calls.load(Ordering::SeqCst), 0);
        settle().await;
        let f = collected(&root);
        assert!(f.len() == 1 && f[0].ends_with(&format!("_discord_{s:.3}.wav")), "near-miss without text: {f:?}");
    }

    #[tokio::test]
    async fn gate_pass_rings_early_then_transcribes() {
        let (stt, root) = (fake(), tmp("pass"));
        let l = listener(&stt, Some(root.clone()));
        let (out, rings) = run(&l, sentence(0.5, 1), false).await;
        let g = out.gate.clone().unwrap();
        assert_eq!(g.wake, Some(true));
        let s = g.score.unwrap();
        assert!((0.5..0.6).contains(&s), "{s}");
        assert_eq!(out.transcript.unwrap().lang, "fr");
        assert_eq!(rings, [true, false]);
        assert_eq!(stt.calls.load(Ordering::SeqCst), 1);
        settle().await;
        let f = collected(&root);
        assert!(f.iter().any(|n| n.ends_with(&format!("_discord_{s:.3}.wav"))) && f.iter().any(|n| n.ends_with(&format!("_discord_{s:.3}.txt"))), "{f:?}");
        // the model went back to the pool and is reused
        assert_eq!(l.pool.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn empty_audio() {
        let stt = fake();
        let l = listener(&stt, None);
        let (out, _) = run(&l, sentence(0.0, 0), false).await;
        assert_eq!(out.gate.unwrap().wake, Some(false)); // nothing but silence: the gate says no
        let (out, _) = run(&l, sentence(0.0, 0), true).await;
        assert_eq!(out.transcript.unwrap().text, "");
        assert_eq!(stt.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn no_wake_model_means_no_gate() {
        let stt = fake();
        let l = Listener::with_scorer(stt.clone(), None, 0.10, None);
        let (out, rings) = run(&l, sentence(0.0, 1), false).await;
        assert_eq!(out.gate, Some(Gate { wake: None, score: None }));
        assert!(out.transcript.is_some());
        assert_eq!(rings, [false]);
    }

    fn models() -> Option<PathBuf> {
        let dir = std::env::var("BOBBY_MODELS").map(PathBuf::from).unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models"));
        dir.join("wakeword/bobby.onnx").exists().then(|| dir.join("wakeword"))
    }

    #[tokio::test]
    async fn real_wake_model_rejects_silence() {
        let Some(dir) = models() else { return println!("skipped: wake models not found (set BOBBY_MODELS)") };
        let stt = fake();
        let l = Listener::new(stt.clone(), Some(&dir), 0.10, None);
        let (out, rings) = run(&l, sentence(0.0, 1), false).await;
        assert_eq!(out.gate.unwrap().wake, Some(false));
        assert_eq!(rings, [false]);
        assert_eq!(stt.calls.load(Ordering::SeqCst), 0);
    }
}
