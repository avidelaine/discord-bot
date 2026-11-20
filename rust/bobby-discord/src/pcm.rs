//! Raw PCM between producers (sources, TTS, beeps) and the mixer: s16 48 kHz stereo interleaved, in chunks.
//! The node streams of `bot/music/mixer.js` become a bounded channel: the producer thread blocks when the
//! mixer is behind (backpressure), and stops when the receiver is dropped (stop / skip / replace).

use std::collections::VecDeque;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError};

pub const RATE: u32 = 48_000;
/// 20 ms of stereo audio, in i16 samples (960 per channel). Same as `FRAME` (3840 bytes) in mixer.js.
pub const FRAME: usize = 1920;

pub type PcmTx = SyncSender<Vec<i16>>;

/// Receiving end. `take` never blocks: an underrun (slow network) gives silence, not a stall.
pub struct PcmRx {
    rx: Receiver<Vec<i16>>,
    buf: VecDeque<i16>,
    closed: bool,
    /// Samples read so far (the "bytes" of mixer.js: 0 means the stream died before any sound).
    pub read: usize,
}

/// A pipe holding about one second of audio before the producer blocks.
pub fn pipe() -> (PcmTx, PcmRx) {
    let (tx, rx) = sync_channel(50);
    (tx, PcmRx { rx, buf: VecDeque::new(), closed: false, read: 0 })
}

impl PcmRx {
    /// Whole clip known in advance (beep, cached ack, test signal).
    pub fn from_vec(v: Vec<i16>) -> PcmRx {
        let (_, rx) = sync_channel(0);
        PcmRx { rx, buf: v.into(), closed: true, read: 0 }
    }

    /// Fills `out` (zeros where nothing is buffered). Returns the samples really read.
    pub fn take(&mut self, out: &mut [i16; FRAME]) -> usize {
        while self.buf.len() < FRAME && !self.closed {
            match self.rx.try_recv() {
                Ok(chunk) => self.buf.extend(chunk),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => self.closed = true,
            }
        }
        let n = self.buf.len().min(FRAME);
        for (o, s) in out.iter_mut().zip(self.buf.drain(..n)) {
            *o = s;
        }
        out[n..].fill(0);
        self.read += n;
        n
    }

    /// The producer is gone and everything was read.
    pub fn ended(&self) -> bool {
        self.closed && self.buf.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_clip_pads_with_silence_then_ends() {
        let mut rx = PcmRx::from_vec(vec![7; FRAME + 10]);
        let mut f = [0; FRAME];
        assert_eq!(rx.take(&mut f), FRAME);
        assert_eq!(rx.take(&mut f), 10);
        assert_eq!((f[9], f[10]), (7, 0));
        assert!(rx.ended());
    }

    #[test]
    fn underrun_is_silence_not_end() {
        let (tx, mut rx) = pipe();
        let mut f = [1; FRAME];
        assert_eq!(rx.take(&mut f), 0);
        assert!(!rx.ended() && f[0] == 0);
        drop(tx);
        rx.take(&mut f);
        assert!(rx.ended());
    }
}
