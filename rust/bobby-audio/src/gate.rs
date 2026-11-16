//! Energy gate that cuts the microphone into sentences. Port of `Gate` in voice/local.py, same constants.

pub const RATE: usize = 16000;
pub const BLOCK: usize = 1280; // 80 ms: the wake word model's frame
const SILENCE_S: f64 = 0.6; // end of a sentence
const PREROLL: usize = 4; // blocks kept before the voice starts (the "B" of "Bobby")
const MAX_S: f64 = 15.0;
const MIN_S: f64 = 0.4;

pub struct Gate {
    noise: f64,
    voiced: bool,
    quiet: usize,
    pre: Vec<Vec<f32>>,
    buf: Vec<Vec<f32>>,
}

impl Default for Gate {
    fn default() -> Self {
        Self::new()
    }
}

impl Gate {
    pub fn new() -> Self {
        Self { noise: 0.005, voiced: false, quiet: 0, pre: vec![], buf: vec![] }
    }

    /// Bobby speaks: drop what was heard (the noise floor is kept, like in Python).
    pub fn reset(&mut self) {
        self.voiced = false;
        self.buf.clear();
        self.pre.clear();
    }

    /// Feed one block of 1280 mono 16 kHz samples; returns a whole sentence when one just ended.
    pub fn feed(&mut self, block: &[f32]) -> Option<Vec<f32>> {
        // f32 squares, f64 sum, f32 mean and sqrt: what numpy does, so borderline blocks agree
        let sum: f64 = block.iter().map(|s| (s * s) as f64).sum();
        let rms = ((sum / block.len() as f64) as f32).sqrt() as f64;
        if !self.voiced && rms < self.noise * 3.0 {
            self.noise = 0.98 * self.noise + 0.02 * rms;
        }
        let loud = rms > f64::max(0.01, self.noise * 3.0);
        if !self.voiced {
            self.pre.push(block.to_vec());
            if self.pre.len() > PREROLL {
                self.pre.remove(0);
            }
            if loud {
                self.voiced = true;
                self.quiet = 0;
                self.buf = self.pre.clone();
            }
            return None;
        }
        self.buf.push(block.to_vec());
        self.quiet = if loud { 0 } else { self.quiet + 1 };
        let block_s = |n: usize| n as f64 * BLOCK as f64 / RATE as f64;
        if block_s(self.quiet) >= SILENCE_S || block_s(self.buf.len()) >= MAX_S {
            let audio = self.buf.concat();
            self.reset();
            if audio.len() as f64 / RATE as f64 >= MIN_S {
                return Some(audio);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Same cut points as the Python Gate on the same signal: see tests/make_gate_fixture.py
    #[test]
    fn matches_python_cut_points() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
        let samples: Vec<f32> = hound::WavReader::open(format!("{dir}/gate.wav"))
            .unwrap()
            .into_samples::<i16>()
            .map(|s| s.unwrap() as f32 / 32768.0)
            .collect();
        let expected: Vec<(usize, usize)> = std::fs::read_to_string(format!("{dir}/gate.txt"))
            .unwrap()
            .lines()
            .map(|l| {
                let mut it = l.split_whitespace().map(|v| v.parse().unwrap());
                (it.next().unwrap(), it.next().unwrap())
            })
            .collect();
        let mut gate = Gate::new();
        let got: Vec<(usize, usize)> = samples
            .chunks_exact(BLOCK)
            .enumerate()
            .filter_map(|(k, b)| gate.feed(b).map(|a| (k, a.len())))
            .collect();
        assert!(expected.len() >= 4);
        assert_eq!(got, expected);
    }

    #[test]
    fn reset_keeps_noise_floor_and_drops_sentence() {
        let mut gate = Gate::new();
        let loud = vec![0.3f32; BLOCK];
        assert!(gate.feed(&loud).is_none());
        gate.reset();
        let quiet = vec![0.0f32; BLOCK];
        for _ in 0..20 {
            assert!(gate.feed(&quiet).is_none());
        }
    }
}
