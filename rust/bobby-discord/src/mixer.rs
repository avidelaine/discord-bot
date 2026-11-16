//! PCM mixer: a music channel (x volume) plus an overlay channel (Bobby's beep/TTS) summed into one
//! s16 48 kHz stereo stream. Music is ducked while the overlay plays. Port of `bot/music/mixer.js`.
//! No callbacks: they would run while the caller holds the mixer's Mutex and deadlock with the player.
//! The mixer sends `MixEvent`s on a channel instead, and the owner reacts after releasing the lock.

use crate::pcm::{PcmRx, FRAME};
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};
use symphonia::core::io::MediaSource;
use tokio::sync::{mpsc::UnboundedSender, oneshot};

const DUCK: f32 = 0.25; // music gain while the overlay plays
const STEP: f32 = 0.1; // max gain change per frame: smooth ducking, no clicks
const FADE_OUT: f32 = 1.0; // seconds: fade out on stop / skip
const XFADE: f32 = 2.0; // seconds: crossfade between two tracks

fn frames(sec: f32) -> f32 {
    (sec * 50.0).round().max(1.0)
}

#[derive(Debug, PartialEq)]
pub enum MixEvent {
    /// Output must (re)start: nothing was pulling frames (first sound, or after going idle).
    Start,
    /// The music ended by itself (not replaced or stopped). `read` = samples read, 0 = died before any sound.
    MusicEnded { tag: u64, read: usize },
}

#[derive(Clone, Copy)]
pub enum Fade {
    None,
    Out,
    Secs(f32),
}

struct Music {
    rx: PcmRx,
    tag: u64,
}

/// A track ramping to silence; `f` is its current factor, `per` the decrease per frame.
struct Fading {
    rx: PcmRx,
    f: f32,
    per: f32,
}

struct Overlay {
    rx: PcmRx,
    done: oneshot::Sender<()>,
}

pub struct Mixer {
    events: UnboundedSender<MixEvent>,
    pub volume: f32,
    paused: bool,
    music: Option<Music>,
    fading: Option<Fading>,
    over: Option<Overlay>,
    mf: f32, // fade-in factor of the current music
    mf_step: f32,
    gain: f32,    // smoothed music gain
    active: bool, // someone is pulling frames
}

impl Mixer {
    pub fn new(events: UnboundedSender<MixEvent>) -> Mixer {
        Mixer { events, volume: 0.5, paused: false, music: None, fading: None, over: None, mf: 1.0, mf_step: 0.0, gain: 0.0, active: false }
    }

    /// `fade_in`: ramp the new track up over XFADE seconds (crossfade with the one fading out).
    pub fn set_music(&mut self, src: PcmRx, tag: u64, fade_in: bool) {
        self.stop_music(Fade::None);
        self.music = Some(Music { rx: src, tag });
        self.mf = if fade_in { 0.0 } else { 1.0 };
        self.mf_step = 1.0 / frames(XFADE);
        self.ensure();
    }

    /// With a fade the track ramps to 0 in the mixer instead of cutting. A paused mixer just stops.
    /// Returns true when a track is now fading out.
    pub fn stop_music(&mut self, fade: Fade) -> bool {
        let Some(m) = self.music.take() else { return false };
        let secs = match fade {
            Fade::None => return false,
            _ if self.paused => return false,
            Fade::Out => FADE_OUT,
            Fade::Secs(s) => s,
        };
        // A track still fading in fades out from where it is.
        self.fading = Some(Fading { rx: m.rx, f: self.mf, per: 1.0 / frames(secs) });
        true
    }

    pub fn pause(&mut self, on: bool) {
        self.paused = on;
        if !on {
            self.ensure();
        }
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    /// A new overlay replaces the running one. The receiver resolves when it ends, is replaced or stopped.
    pub fn overlay(&mut self, src: PcmRx) -> oneshot::Receiver<()> {
        self.stop_overlay();
        let (done, rx) = oneshot::channel();
        self.over = Some(Overlay { rx: src, done });
        self.ensure();
        rx
    }

    /// Bobby's voice stops now; the music comes back up.
    pub fn stop_overlay(&mut self) {
        if let Some(o) = self.over.take() {
            let _ = o.done.send(());
        }
    }

    pub fn has_overlay(&self) -> bool {
        self.over.is_some()
    }

    /// Nobody pulls frames any more (not in voice, or the call is gone): the next sound sends `Start` again.
    pub fn detach(&mut self) {
        self.active = false;
    }

    /// A new call exists: start pulling if anything is waiting to be heard.
    pub fn restart(&mut self) {
        self.active = false;
        if self.music.is_some() || self.fading.is_some() || self.over.is_some() {
            self.ensure();
        }
    }

    fn ensure(&mut self) {
        if !self.active {
            self.active = true;
            let _ = self.events.send(MixEvent::Start);
        }
    }

    /// The next 20 ms, or None when nothing is left (the output ends; a later set_music/overlay/unpause restarts it).
    pub fn next_frame(&mut self) -> Option<[i16; FRAME]> {
        let playing = !self.paused && self.music.is_some();
        if !playing && self.fading.is_none() && self.over.is_none() {
            self.active = false;
            return None;
        }
        let (mut m, mut o, mut x) = ([0; FRAME], [0; FRAME], [0; FRAME]);
        if let Some(mu) = self.music.as_mut().filter(|_| playing) {
            mu.rx.take(&mut m);
        }
        if let Some(ov) = &mut self.over {
            ov.rx.take(&mut o);
        }
        if let Some(fd) = &mut self.fading {
            fd.rx.take(&mut x);
        }
        let mf0 = self.mf;
        let mf1 = if playing { (mf0 + self.mf_step).min(1.0) } else { mf0 };
        self.mf = mf1;
        let f0 = self.fading.as_ref().map_or(0.0, |f| f.f);
        let f1 = self.fading.as_mut().map_or(0.0, |f| {
            f.f = (f.f - f.per).max(0.0);
            f.f
        });
        let target = self.volume * if self.over.is_some() { DUCK } else { 1.0 };
        let g0 = self.gain;
        let g1 = g0 + (target - g0).clamp(-STEP, STEP);
        self.gain = g1;

        let mut out = [0i16; FRAME];
        for (i, s) in out.iter_mut().enumerate() {
            let t = i as f32 / FRAME as f32;
            let g = g0 + (g1 - g0) * t;
            let v = m[i] as f32 * g * (mf0 + (mf1 - mf0) * t) + x[i] as f32 * g * (f0 + (f1 - f0) * t) + o[i] as f32;
            *s = v.round().clamp(-32768.0, 32767.0) as i16;
        }

        if self.fading.as_ref().is_some_and(|f| f1 == 0.0 || f.rx.ended()) {
            self.fading = None;
        }
        if playing && self.music.as_ref().is_some_and(|m| m.rx.ended()) {
            if let Some(m) = self.music.take() {
                let _ = self.events.send(MixEvent::MusicEnded { tag: m.tag, read: m.rx.read });
            }
        }
        if self.over.as_ref().is_some_and(|o| o.rx.ended()) {
            self.stop_overlay();
        }
        Some(out)
    }
}

/// The mixer as a songbird input (f32 LE interleaved stereo, wrapped by `RawAdapter`). Symphonia's
/// stream may read a little ahead: that buffer adds latency to ducking and stops.
pub fn input(m: Arc<Mutex<Mixer>>) -> songbird::input::Input {
    songbird::input::RawAdapter::new(MixerSource { m, pending: Vec::new(), pos: 0 }, 48000, 2).into()
}

struct MixerSource {
    m: Arc<Mutex<Mixer>>,
    pending: Vec<u8>,
    pos: usize,
}

impl Read for MixerSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.pending.len() {
            let Some(frame) = self.m.lock().unwrap().next_frame() else { return Ok(0) };
            self.pending.clear();
            self.pending.extend(frame.iter().flat_map(|&s| (s as f32 / 32768.0).to_le_bytes()));
            self.pos = 0;
        }
        let n = buf.len().min(self.pending.len() - self.pos);
        buf[..n].copy_from_slice(&self.pending[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

impl Seek for MixerSource {
    fn seek(&mut self, _: SeekFrom) -> io::Result<u64> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "live stream"))
    }
}

impl MediaSource for MixerSource {
    fn is_seekable(&self) -> bool {
        false
    }
    fn byte_len(&self) -> Option<u64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

    fn dc(value: i16, n: usize) -> PcmRx {
        PcmRx::from_vec(vec![value; FRAME * n])
    }

    fn mixer() -> (Mixer, UnboundedReceiver<MixEvent>) {
        let (tx, rx) = unbounded_channel();
        let mut m = Mixer::new(tx);
        m.volume = 1.0;
        (m, rx)
    }

    /// First sample of each of the next `n` frames, stopping early when the output ends.
    fn pull(m: &mut Mixer, n: usize) -> Vec<i16> {
        (0..n).map_while(|_| m.next_frame().map(|f| f[0])).collect()
    }

    fn near(x: i16, y: f32, tol: f32) -> bool {
        (x as f32 - y).abs() < tol
    }

    #[test]
    fn overlay_ducks_music_then_restores_it() {
        let (mut m, _ev) = mixer();
        m.set_music(dc(8000, 300), 1, false);
        let mut f = pull(&mut m, 60);
        let mut done = m.overlay(dc(2000, 60));
        assert!(m.has_overlay());
        f.extend(pull(&mut m, 1000));
        assert!(near(f[40], 8000.0, 50.0), "music alone at full volume ({})", f[40]);
        let ducked = f.iter().enumerate().position(|(i, &v)| i >= 60 && near(v, 8000.0 * 0.25 + 2000.0, 50.0));
        let ducked = ducked.expect("ducked");
        assert!(ducked < 70, "ducked within 10 frames ({ducked})");
        assert!(f[ducked..ducked + 40].iter().all(|&v| v < 5000), "stays ducked while the overlay plays");
        assert!(near(f[f.len() - 20], 8000.0, 50.0), "music restored");
        assert!(m.next_frame().is_none(), "ends when idle");
        assert!(!m.has_overlay() && done.try_recv().is_ok(), "overlay receiver resolved");
    }

    #[test]
    fn fade_out_is_monotonic_and_reaches_silence() {
        let (mut m, _ev) = mixer();
        m.set_music(dc(8000, 400), 1, false);
        pull(&mut m, 30);
        assert!(m.stop_music(Fade::Out));
        let f = pull(&mut m, 1000);
        let start = f.iter().position(|&v| v < 7900).expect("fade starts");
        assert!(f[f.len() - 1] < 250 && f.len() <= start + (FADE_OUT * 50.0) as usize + 3, "fades within FADE_OUT ({start}..{})", f.len());
        assert!(f[start..].windows(2).all(|w| w[1] <= w[0]), "monotonic decrease");
    }

    #[test]
    fn paused_mixer_stops_plainly() {
        let (mut m, _ev) = mixer();
        m.set_music(dc(8000, 10), 1, false);
        m.pause(true);
        assert!(m.paused());
        assert!(!m.stop_music(Fade::Out) && m.fading.is_none());
    }

    #[test]
    fn crossfade_mixes_both_tracks() {
        let (mut m, _ev) = mixer();
        m.set_music(dc(8000, 400), 1, false);
        pull(&mut m, 30);
        m.stop_music(Fade::Secs(XFADE));
        m.set_music(dc(4000, 120), 2, true);
        let f = pull(&mut m, 1000);
        assert!(f.iter().any(|&v| v > 4600 && v < 7400), "both tracks mixed during the crossfade");
        assert!(f[f.len() - 30..].iter().any(|&v| near(v, 4000.0, 60.0)), "new track reaches full level");
    }

    #[test]
    fn events_start_and_music_ended() {
        let (mut m, mut ev) = mixer();
        m.set_music(dc(100, 5), 7, false);
        m.set_music(dc(100, 5), 8, false); // replaced: no event for tag 7, and Start only once
        pull(&mut m, 1000);
        assert_eq!(ev.try_recv(), Ok(MixEvent::Start));
        assert_eq!(ev.try_recv(), Ok(MixEvent::MusicEnded { tag: 8, read: 5 * FRAME }));
        assert!(ev.try_recv().is_err());
        // idle now: a new track restarts the output, and stopping it by hand is not an end
        m.set_music(dc(100, 5), 9, false);
        assert_eq!(ev.try_recv(), Ok(MixEvent::Start));
        m.stop_music(Fade::None);
        pull(&mut m, 1000);
        assert!(ev.try_recv().is_err());
    }
}
