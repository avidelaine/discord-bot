//! openWakeWord streaming scoring (melspectrogram -> embedding -> wake model), a port of
//! `openwakeword.utils.AudioFeatures` + `Model.predict` so that Rust scores match Python's.
use ort::{environment::Environment, session::Session, value::Tensor};
use std::{collections::VecDeque, path::Path};

const FRAME: usize = 1280;
const MEL_BINS: usize = 32;
const EMB_DIM: usize = 96;
const MEL_MAX: usize = 970; // 10 * 97 frames
const FEAT_MAX: usize = 120;

/// Deterministic stand-in for numpy's `randint(-1000, 1000, n)`. `_training/bench.py --csv` uses the same LCG.
pub fn lcg_noise(n: usize) -> Vec<i16> {
    let mut s: u64 = 12345;
    (0..n)
        .map(|_| {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((s >> 33) % 2000) as i16 - 1000
        })
        .collect()
}

fn e<T: std::fmt::Display>(x: T) -> String {
    x.to_string()
}

pub struct Wake {
    _env: Environment,
    mel: Session,
    emb: Session,
    model: Session,
    n_frames: usize,
    raw: VecDeque<i16>,
    mel_buf: Vec<f32>,  // rows * 32
    feat_buf: Vec<f32>, // rows * 96
    noise_feats: Vec<f32>,
    n_pred: usize,
}

impl Wake {
    pub fn new(dir: &Path) -> Result<Self, String> {
        let env = crate::onnx::env(dir)?;
        let load = |f: &str| -> Result<Session, String> {
            Session::builder(&env).map_err(e)?.with_intra_threads(1).map_err(e)?.commit_from_file(dir.join(f)).map_err(e)
        };
        let (mel, emb, model) = (load("melspectrogram.onnx")?, load("embedding_model.onnx")?, load("bobby.onnx")?);
        let n_frames = model
            .inputs()
            .first()
            .and_then(|i| i.dtype().tensor_shape().and_then(|s| s.get(1).copied()))
            .filter(|n| *n > 0)
            .ok_or("wake model input shape")? as usize;
        let mut w = Wake { _env: env, mel, emb, model, n_frames, raw: VecDeque::new(), mel_buf: vec![], feat_buf: vec![], noise_feats: vec![], n_pred: 0 };
        let noise: Vec<f32> = lcg_noise(64000).into_iter().map(f32::from).collect();
        let spec = w.melspec(&noise)?;
        let rows = spec.len() / MEL_BINS;
        let mut batch = vec![];
        let mut i = 0;
        while i + 76 <= rows {
            batch.extend_from_slice(&spec[i * MEL_BINS..(i + 76) * MEL_BINS]);
            i += 8;
        }
        let n = batch.len() / (76 * MEL_BINS);
        w.noise_feats = w.embed(batch, n)?;
        w.reset();
        Ok(w)
    }

    fn melspec(&mut self, x: &[f32]) -> Result<Vec<f32>, String> {
        let t = Tensor::from_array(([1usize, x.len()], x.to_vec())).map_err(e)?;
        let out = self.mel.run(ort::inputs![t]).map_err(e)?;
        let (_, d) = out[0].try_extract_tensor::<f32>().map_err(e)?;
        Ok(d.iter().map(|v| v / 10.0 + 2.0).collect())
    }

    fn embed(&mut self, batch: Vec<f32>, n: usize) -> Result<Vec<f32>, String> {
        let t = Tensor::from_array(([n, 76usize, MEL_BINS, 1usize], batch)).map_err(e)?;
        let out = self.emb.run(ort::inputs![t]).map_err(e)?;
        let (_, d) = out[0].try_extract_tensor::<f32>().map_err(e)?;
        Ok(d.to_vec())
    }

    pub fn reset(&mut self) {
        self.raw.clear();
        self.mel_buf = vec![1.0; 76 * MEL_BINS];
        self.feat_buf = self.noise_feats.clone();
        self.n_pred = 0;
    }

    /// One 80 ms frame (1280 samples). Returns the wake score; the first 5 after a reset are 0.
    pub fn predict(&mut self, frame: &[i16]) -> f32 {
        self.try_predict(frame).unwrap_or(0.0)
    }

    fn try_predict(&mut self, frame: &[i16]) -> Result<f32, String> {
        debug_assert_eq!(frame.len(), FRAME);
        self.raw.extend(frame.iter().copied());
        while self.raw.len() > 160_000 {
            self.raw.pop_front();
        }
        let take = self.raw.len().min(FRAME + 480);
        let x: Vec<f32> = self.raw.iter().skip(self.raw.len() - take).map(|&v| f32::from(v)).collect();
        let spec = self.melspec(&x)?;
        self.mel_buf.extend_from_slice(&spec);
        let rows = self.mel_buf.len() / MEL_BINS;
        if rows > MEL_MAX {
            self.mel_buf.drain(..(rows - MEL_MAX) * MEL_BINS);
        }
        let rows = self.mel_buf.len() / MEL_BINS;
        let win = self.mel_buf[(rows - 76) * MEL_BINS..].to_vec();
        let f = self.embed(win, 1)?;
        self.feat_buf.extend_from_slice(&f);
        let frows = self.feat_buf.len() / EMB_DIM;
        if frows > FEAT_MAX {
            self.feat_buf.drain(..(frows - FEAT_MAX) * EMB_DIM);
        }
        let frows = self.feat_buf.len() / EMB_DIM;
        let n = self.n_frames;
        let inp = self.feat_buf[(frows - n) * EMB_DIM..].to_vec();
        let t = Tensor::from_array(([1usize, n, EMB_DIM], inp)).map_err(e)?;
        let out = self.model.run(ort::inputs![t]).map_err(e)?;
        let (_, d) = out[0].try_extract_tensor::<f32>().map_err(e)?;
        let p = d.first().copied().unwrap_or(0.0);
        self.n_pred += 1;
        Ok(if self.n_pred <= 5 { 0.0 } else { p })
    }

    /// Exact port of `voice/server.py::bobby_score` (16 kHz mono f32 in [-1, 1]).
    pub fn score(&mut self, audio16k: &[f32]) -> f32 {
        let pcm: Vec<i16> = audio16k.iter().map(|v| (v.clamp(-1.0, 1.0) * 32767.0) as i16).collect();
        self.score_i16(&pcm)
    }

    /// Same as `score` for audio that is already 16 kHz mono s16 (what `_training/bench.py` feeds Python).
    pub fn score_i16(&mut self, audio16k: &[i16]) -> f32 {
        let mut pcm = vec![0i16; 8000];
        pcm.extend_from_slice(audio16k);
        pcm.extend(std::iter::repeat_n(0i16, 8000));
        self.reset();
        pcm.chunks_exact(FRAME).map(|c| self.predict(c)).fold(0.0, f32::max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_scores_low_and_is_deterministic() {
        // the repo's models/ (a worktree sits inside the main checkout, so look up the ancestors)
        let Some(dir) = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().map(|d| d.join("models/wakeword")).find(|d| d.join("bobby.onnx").exists()) else {
            println!("wake models not found, skipping");
            return;
        };
        let mut w = Wake::new(&dir).unwrap();
        let s = vec![0.0f32; 16000];
        let (a, b) = (w.score(&s), w.score(&s));
        assert!(a < 0.05, "silence scored {a}");
        assert_eq!(a, b);
    }
}
