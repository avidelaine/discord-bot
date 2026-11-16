//! Silero VAD v6 as faster-whisper uses it (`faster_whisper/vad.py`, default `VadOptions`): speech probabilities
//! per 512-sample window, speech chunks (`get_speech_timestamps`) and `collect_chunks`.
use ort::{environment::Environment, session::Session, value::Tensor};
use std::path::Path;

const WINDOW: usize = 512;
const CONTEXT: usize = 64;
const BATCH: usize = 10000;

fn e<T: std::fmt::Display>(x: T) -> String {
    x.to_string()
}

/// faster-whisper defaults (`VadOptions()`): the only values `_whisper` ever uses.
#[derive(Clone, Debug)]
pub struct VadOptions {
    pub threshold: f64,
    pub neg_threshold: Option<f64>,
    pub min_speech_duration_ms: f64,
    pub max_speech_duration_s: f64,
    pub min_silence_duration_ms: f64,
    pub speech_pad_ms: f64,
}

impl Default for VadOptions {
    fn default() -> Self {
        Self { threshold: 0.5, neg_threshold: None, min_speech_duration_ms: 0.0, max_speech_duration_s: f64::INFINITY, min_silence_duration_ms: 2000.0, speech_pad_ms: 400.0 }
    }
}

/// A speech chunk, in samples at 16 kHz.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub start: usize,
    pub end: usize,
}

pub struct Vad {
    _env: Environment,
    session: Session,
}

impl Vad {
    /// `model`: `models/vad/silero_vad_v6.onnx`; `models/onnxruntime.dll` is loaded (see `onnx::env`).
    pub fn new(model: &Path) -> Result<Self, String> {
        let dir = model.parent().ok_or("VAD model path has no folder")?;
        let env = crate::onnx::env(dir)?;
        let session = Session::builder(&env).map_err(e)?.with_intra_threads(1).map_err(e)?.commit_from_file(model).map_err(e)?;
        Ok(Self { _env: env, session })
    }

    /// One probability per 512-sample window of `audio` (zero-padded to a multiple of 512, at least one pad sample).
    pub fn speech_probs(&mut self, audio: &[f32]) -> Result<Vec<f32>, String> {
        let n = (audio.len() / WINDOW + 1) * WINDOW;
        let mut padded = audio.to_vec();
        padded.resize(n, 0.0);
        let rows = n / WINDOW;
        // Python: `context = batched[..., -64:]` is a view and `context[-1] = 0` also zeroes the last 64 samples of the
        // last window itself. Reproduced on purpose.
        for v in &mut padded[n - CONTEXT..] {
            *v = 0.0;
        }
        let mut out = Vec::with_capacity(rows);
        let mut start = 0;
        while start < rows {
            let end = (start + BATCH).min(rows);
            let mut input = Vec::with_capacity((end - start) * (WINDOW + CONTEXT));
            for r in start..end {
                if r == 0 {
                    input.extend(std::iter::repeat_n(0.0f32, CONTEXT)); // rolled-in zeroed context
                } else {
                    input.extend_from_slice(&padded[r * WINDOW - CONTEXT..r * WINDOW]);
                }
                input.extend_from_slice(&padded[r * WINDOW..(r + 1) * WINDOW]);
            }
            let x = Tensor::from_array(([end - start, WINDOW + CONTEXT], input)).map_err(e)?;
            let h = Tensor::from_array(([1usize, 1, 128], vec![0f32; 128])).map_err(e)?;
            let c = Tensor::from_array(([1usize, 1, 128], vec![0f32; 128])).map_err(e)?;
            let res = self.session.run(ort::inputs!["input" => x, "h" => h, "c" => c]).map_err(e)?;
            let (_, d) = res[0].try_extract_tensor::<f32>().map_err(e)?;
            out.extend_from_slice(d);
            start = end;
        }
        Ok(out)
    }

    /// `get_speech_timestamps(audio, VadOptions())`.
    pub fn speech_chunks(&mut self, audio: &[f32], opts: &VadOptions) -> Result<Vec<Chunk>, String> {
        let probs = self.speech_probs(audio)?;
        Ok(chunks_from_probs(&probs, audio.len(), opts))
    }
}

/// The state machine of `get_speech_timestamps`, on precomputed probabilities.
pub fn chunks_from_probs(probs: &[f32], audio_len: usize, o: &VadOptions) -> Vec<Chunk> {
    let sr = 16000.0;
    let thr = o.threshold;
    let neg = o.neg_threshold.unwrap_or((thr - 0.15).max(0.01));
    let min_speech = sr * o.min_speech_duration_ms / 1000.0;
    let pad = sr * o.speech_pad_ms / 1000.0;
    let max_speech = sr * o.max_speech_duration_s - WINDOW as f64 - 2.0 * pad;
    let min_silence = sr * o.min_silence_duration_ms / 1000.0;
    let min_silence_at_max = sr * 98.0 / 1000.0;

    let mut triggered = false;
    let mut speeches: Vec<(i64, i64)> = vec![];
    let mut cur: Option<i64> = None; // start of the speech in progress
    let (mut temp_end, mut prev_end, mut next_start) = (0i64, 0i64, 0i64);

    for (i, &p) in probs.iter().enumerate() {
        let p = p as f64;
        let pos = (WINDOW * i) as i64;
        if p >= thr && temp_end != 0 {
            temp_end = 0;
            if next_start < prev_end {
                next_start = pos;
            }
        }
        if p >= thr && !triggered {
            triggered = true;
            cur = Some(pos);
            continue;
        }
        if triggered && (pos - cur.unwrap_or(0)) as f64 > max_speech {
            if prev_end != 0 {
                speeches.push((cur.unwrap_or(0), prev_end));
                cur = None;
                if next_start < prev_end {
                    triggered = false;
                } else {
                    cur = Some(next_start);
                }
                prev_end = 0;
                next_start = 0;
                temp_end = 0;
            } else {
                speeches.push((cur.unwrap_or(0), pos));
                cur = None;
                prev_end = 0;
                next_start = 0;
                temp_end = 0;
                triggered = false;
                continue;
            }
        }
        if p < neg && triggered {
            if temp_end == 0 {
                temp_end = pos;
            }
            if (pos - temp_end) as f64 > min_silence_at_max {
                prev_end = temp_end;
            }
            if ((pos - temp_end) as f64) < min_silence {
                continue;
            }
            if (temp_end - cur.unwrap_or(0)) as f64 > min_speech {
                speeches.push((cur.unwrap_or(0), temp_end));
            }
            cur = None;
            prev_end = 0;
            next_start = 0;
            temp_end = 0;
            triggered = false;
        }
    }
    if let Some(start) = cur {
        if (audio_len as i64 - start) as f64 > min_speech {
            speeches.push((start, audio_len as i64));
        }
    }

    let pad_i = pad as i64;
    let len = audio_len as i64;
    for i in 0..speeches.len() {
        if i == 0 {
            speeches[i].0 = (speeches[i].0 - pad_i).max(0);
        }
        if i + 1 != speeches.len() {
            let silence = speeches[i + 1].0 - speeches[i].1;
            if (silence as f64) < 2.0 * pad {
                speeches[i].1 += silence.div_euclid(2);
                speeches[i + 1].0 = (speeches[i + 1].0 - silence.div_euclid(2)).max(0);
            } else {
                speeches[i].1 = len.min(speeches[i].1 + pad_i);
                speeches[i + 1].0 = (speeches[i + 1].0 - pad_i).max(0);
            }
        } else {
            speeches[i].1 = len.min(speeches[i].1 + pad_i);
        }
    }
    speeches.into_iter().map(|(s, en)| Chunk { start: s as usize, end: en as usize }).collect()
}

/// `collect_chunks` with no maximum duration: the speech chunks glued together.
pub fn collect_chunks(audio: &[f32], chunks: &[Chunk]) -> Vec<f32> {
    let mut out = Vec::new();
    for c in chunks {
        out.extend_from_slice(&audio[c.start.min(audio.len())..c.end.min(audio.len())]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const ROOT: &str = "C:/Users/alexa/Desktop/Personal Projects/discord-bot";

    fn read_wav(p: &Path) -> Vec<f32> {
        let mut r = hound::WavReader::open(p).unwrap();
        r.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect()
    }

    #[test]
    fn chunks_padding_and_merge() {
        // Speech, then silence longer than min_silence (ends the chunk), then speech again: padding is applied and clamped.
        let mut probs = vec![0.9f32; 10];
        probs.extend(vec![0.0; 100]);
        probs.extend(vec![0.9f32; 5]);
        let len = (probs.len() + 1) * 512;
        let c = chunks_from_probs(&probs, len, &VadOptions::default());
        assert_eq!(c, vec![Chunk { start: 0, end: 5120 + 6400 }, Chunk { start: 110 * 512 - 6400, end: len }]);
        assert!(chunks_from_probs(&[0.1; 20], 20 * 512, &VadOptions::default()).is_empty());
        assert_eq!(collect_chunks(&[1.0, 2.0, 3.0, 4.0], &[Chunk { start: 1, end: 3 }]), vec![2.0, 3.0]);
    }

    #[test]
    fn matches_python_speech_timestamps() {
        let model = PathBuf::from(ROOT).join("models/vad/silero_vad_v6.onnx");
        let clips = [("_training/collected/2026-11-16/204948_215_discord_0.015.wav", 297920, (256, 297920)), ("_training/collected/2026-11-16/181946_141_pc_0.890.wav", 28160, (0, 28160))];
        if !model.exists() || !clips.iter().all(|c| PathBuf::from(ROOT).join(c.0).exists()) {
            println!("skipped: VAD model or collected clips missing");
            return;
        }
        let mut vad = Vad::new(&model).unwrap();
        for (rel, n, (s, en)) in clips {
            let audio = read_wav(&PathBuf::from(ROOT).join(rel));
            assert_eq!(audio.len(), n);
            let got = vad.speech_chunks(&audio, &VadOptions::default()).unwrap();
            assert_eq!(got, vec![Chunk { start: s, end: en }], "{rel}");
        }
    }
}
