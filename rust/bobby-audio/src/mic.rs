//! PC microphone: default input device -> mono -> 16 kHz -> exact 1280-sample blocks.
use crate::gate::{BLOCK, RATE};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use rubato::{Fft, FixedSync, Resampler};

/// Keeps the stream alive; the microphone stops when this is dropped.
pub struct Mic {
    _stream: cpal::Stream,
    failed: Arc<AtomicBool>,
}

impl Mic {
    /// Set by the stream's error callback (device unplugged, driver reset...): the stream is dead.
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::SeqCst)
    }
}

/// Downmix to mono, resample to 16 kHz (state kept across calls), regroup into BLOCK-sample blocks.
pub struct To16k {
    channels: usize,
    resampler: Option<Fft<f32>>, // None when the device is already at 16 kHz
    mono: Vec<f32>,              // mono input waiting for a full resampler chunk
    out: Vec<f32>,               // 16 kHz output waiting for a full block
    buf: Vec<f32>,               // resampler output scratch, allocated once
}

impl To16k {
    pub fn new(in_rate: usize, channels: usize) -> Result<Self, String> {
        let mut buf = vec![];
        let resampler = if in_rate == RATE {
            None
        } else {
            // 10 ms chunks: low latency, and 480 -> 160 is exact for 48 kHz
            let r = Fft::new(in_rate, RATE, in_rate / 100, 1, FixedSync::Input).map_err(|e| e.to_string())?;
            buf = vec![0f32; r.output_frames_max()];
            Some(r)
        };
        Ok(Self { channels, resampler, mono: vec![], out: vec![], buf })
    }

    /// `interleaved` is any number of frames of `channels` samples; `on_block` gets each complete block.
    pub fn push(&mut self, interleaved: &[f32], on_block: &mut impl FnMut(&[f32])) {
        let n = self.channels as f32;
        let mono = interleaved.chunks_exact(self.channels).map(|f| f.iter().sum::<f32>() / n);
        match &mut self.resampler {
            None => self.out.extend(mono),
            Some(r) => {
                self.mono.extend(mono);
                loop {
                    let need = r.input_frames_next();
                    if self.mono.len() < need {
                        break;
                    }
                    let max = self.buf.len();
                    let (input, mut output) = (
                        InterleavedSlice::new(&self.mono[..need], 1, need).unwrap(),
                        InterleavedSlice::new_mut(&mut self.buf, 1, max).unwrap(),
                    );
                    let Ok((_, written)) = r.process_into_buffer(&input, &mut output, None) else {
                        self.mono.clear(); // do not let a failing resampler grow the backlog
                        break;
                    };
                    self.out.extend_from_slice(&self.buf[..written]);
                    self.mono.drain(..need);
                }
            }
        }
        while self.out.len() >= BLOCK {
            on_block(&self.out[..BLOCK]);
            self.out.drain(..BLOCK);
        }
    }
}

pub fn start(mut on_block: impl FnMut(&[f32]) + Send + 'static) -> Result<Mic, String> {
    let device = cpal::default_host().default_input_device().ok_or("no input device")?;
    let name = device.description().map(|d| d.name().to_string()).unwrap_or_default();
    let cfg = device.default_input_config().map_err(|e| e.to_string())?;
    if cfg.sample_format() != cpal::SampleFormat::F32 {
        return Err(format!("unsupported microphone sample format {:?}", cfg.sample_format()));
    }
    let mut conv = To16k::new(cfg.sample_rate() as usize, cfg.channels() as usize)?;
    let failed = Arc::new(AtomicBool::new(false));
    let flag = failed.clone();
    let stream = device
        .build_input_stream::<f32, _, _>(
            cfg.config(),
            move |data, _| conv.push(data, &mut on_block),
            move |e| {
                eprintln!("[local] microphone error: {e}");
                flag.store(true, Ordering::SeqCst);
            },
            None,
        )
        .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    println!("[local] listening on the PC microphone ({name})");
    Ok(Mic { _stream: stream, failed })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn stereo_48k_sine_becomes_16k_blocks() {
        let mut conv = To16k::new(48000, 2).unwrap();
        let sine: Vec<f32> = (0..48000)
            .flat_map(|i| {
                let s = (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48000.0).sin() * 0.5;
                [s, s]
            })
            .collect();
        let mut blocks: Vec<Vec<f32>> = vec![];
        // odd-sized pushes, like a real callback
        for chunk in sine.chunks(2 * 441) {
            conv.push(chunk, &mut |b| blocks.push(b.to_vec()));
        }
        assert!((11..=12).contains(&blocks.len()), "{} blocks", blocks.len());
        assert!(blocks.iter().all(|b| b.len() == BLOCK));
        let mid = &blocks[5];
        assert!((rms(mid) - 0.5 / 2f32.sqrt()).abs() < 0.01, "rms {}", rms(mid));
        let crossings = mid.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        let hz = crossings as f32 / (BLOCK as f32 / RATE as f32);
        assert!((hz - 1000.0).abs() < 30.0, "{hz} Hz");
    }
}
