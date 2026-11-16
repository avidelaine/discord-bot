//! PC speakers: beeps and blocking WAV playback (port of `voice/local.py` play/hush/tone).
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// Room echo: the mic stays ignored this long after the sound ends.
const ECHO: Duration = Duration::from_millis(200);

/// Beep(s) as a mono s16 WAV, one `ms` long sine per frequency, 5 ms fade in/out.
/// `volume` is a percent: amplitude = 0.3 * volume / 100 (30 gives 0.09).
pub fn tone(freqs: &[f32], ms: u32, rate: u32, volume: u8) -> Vec<u8> {
    let amp = 0.3 * volume.min(100) as f64 / 100.0;
    let n = (rate as u64 * ms as u64 / 1000) as usize;
    let mut out = Cursor::new(Vec::new());
    let spec = hound::WavSpec { channels: 1, sample_rate: rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::new(&mut out, spec).expect("wav header");
    for &f in freqs {
        for i in 0..n {
            let t = i as f64 / rate as f64;
            let tr = (n - 1 - i) as f64 / rate as f64;
            let env = (t.min(tr) * 200.0).min(1.0);
            let v = (2.0 * std::f64::consts::PI * f as f64 * t).sin() * env * amp;
            w.write_sample((v * 32767.0) as i16).expect("wav sample");
        }
    }
    w.finalize().expect("wav finalize");
    out.into_inner()
}

/// "nope" = two low notes, anything else = the short "yes" beep.
pub fn beep(kind: &str, volume: u8) -> Vec<u8> {
    if kind == "nope" { tone(&[330.0, 247.0], 140, 22050, volume) } else { tone(&[880.0], 110, 22050, volume) }
}

/// Scales the samples of a 16-bit WAV by `percent` / 100 (100 or an unreadable WAV: returned as is).
pub fn scale_wav(wav: &[u8], percent: u8) -> Vec<u8> {
    if percent == 100 { return wav.to_vec(); }
    let Ok(mut r) = hound::WavReader::new(Cursor::new(wav)) else { return wav.to_vec() };
    let spec = r.spec();
    if spec.sample_format != hound::SampleFormat::Int || spec.bits_per_sample != 16 { return wav.to_vec(); }
    let k = percent as f32 / 100.0;
    let mut out = Cursor::new(Vec::new());
    let Ok(mut w) = hound::WavWriter::new(&mut out, spec) else { return wav.to_vec() };
    for s in r.samples::<i16>().flatten() {
        let _ = w.write_sample((s as f32 * k).round().clamp(-32768.0, 32767.0) as i16);
    }
    if w.finalize().is_err() { return wav.to_vec(); }
    out.into_inner()
}

#[derive(Default)]
pub struct Speaker {
    speaking: AtomicBool,
    /// When the last PlaySound call returned (before the echo sleep); for tests.
    returned_at: Mutex<Option<Instant>>,
    /// Set by `hush`; wakes the waiting `play`.
    stop: (Mutex<bool>, Condvar),
    /// Serializes `play` so overlapping sounds cannot reset `stop`/`speaking` under each other (`hush` skips it).
    gate: Mutex<()>,
    /// Copy of the WAV handed to PlaySound (SND_MEMORY|SND_ASYNC reads it after the call returns);
    /// kept until the next `play` replaces it.
    buf: Mutex<Vec<u8>>,
}

impl Speaker {
    pub fn new() -> Self { Self::default() }

    pub fn is_speaking(&self) -> bool { self.speaking.load(Ordering::SeqCst) }

    /// Plays a WAV, blocking; the mic is ignored meanwhile. Returns false if playback failed.
    pub fn play(&self, wav: &[u8]) -> bool {
        struct Reset<'a>(&'a AtomicBool);
        impl Drop for Reset<'_> {
            fn drop(&mut self) { std::thread::sleep(ECHO); self.0.store(false, Ordering::SeqCst); }
        }
        let _gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
        self.speaking.store(true, Ordering::SeqCst);
        let _reset = Reset(&self.speaking);
        *self.stop.0.lock().unwrap() = false;
        // Previous sound is over (its play waited for it or was hushed), so the buffer can be replaced.
        let mut buf = self.buf.lock().unwrap_or_else(|e| e.into_inner());
        *buf = wav.to_vec();
        let ok = play_start(&buf);
        drop(buf);
        if ok {
            // Async playback (a synchronous PlaySound cannot be interrupted from another thread):
            // wait for the WAV's length, or for `hush`.
            let n = hound::WavReader::new(Cursor::new(wav)).map(|r| r.duration() as f64 / r.spec().sample_rate as f64).unwrap_or(0.0);
            let end = Duration::from_secs_f64(n) + Duration::from_millis(500);
            let _ = self.stop.1.wait_timeout_while(self.stop.0.lock().unwrap(), end, |s| !*s);
        }
        *self.returned_at.lock().unwrap() = Some(Instant::now());
        ok
    }

    /// Stops any sound playing, so a blocking `play` in another thread returns.
    pub fn hush(&self) {
        *self.stop.0.lock().unwrap() = true;
        self.stop.1.notify_all();
        #[cfg(windows)]
        unsafe {
            use windows::core::PCWSTR;
            use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_PURGE};
            let _ = PlaySoundW(PCWSTR::null(), None, SND_ASYNC);
            let _ = PlaySoundW(PCWSTR::null(), None, SND_PURGE);
        }
    }
}

#[cfg(windows)]
fn play_start(wav: &[u8]) -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
    unsafe { PlaySoundW(PCWSTR(wav.as_ptr() as *const u16), None, SND_MEMORY | SND_NODEFAULT | SND_ASYNC).as_bool() }
}

#[cfg(not(windows))]
fn play_start(_wav: &[u8]) -> bool { false }

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn samples(wav: &[u8]) -> Vec<i16> {
        hound::WavReader::new(Cursor::new(wav)).unwrap().samples::<i16>().map(|s| s.unwrap()).collect()
    }

    #[test]
    fn tone_header_and_shape() {
        let wav = tone(&[880.0], 110, 22050, 100);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        let r = hound::WavReader::new(Cursor::new(&wav)).unwrap();
        assert_eq!((r.spec().sample_rate, r.spec().channels, r.spec().bits_per_sample), (22050, 1, 16));
        let s = samples(&wav);
        assert_eq!(s.len(), 2425);
        assert!(s[0].abs() < 50 && s[s.len() - 1].abs() < 800, "fade: {} {}", s[0], s[s.len() - 1]);
        assert!(s.iter().any(|v| v.abs() > 8000));
        assert_eq!(samples(&beep("nope", 30)).len(), 2 * 3087);
    }

    #[test]
    fn volume_scales_beep_and_voice() {
        let peak = |w: &[u8]| samples(w).iter().map(|v| v.unsigned_abs()).max().unwrap();
        assert!(peak(&tone(&[880.0], 110, 22050, 10)) < peak(&tone(&[880.0], 110, 22050, 100)) / 5);
        let p30 = peak(&beep("yes", 30)) as f32;
        assert!((p30 - 0.09 * 32767.0).abs() < 100.0, "{p30}");
        let loud = tone(&[880.0], 110, 22050, 100);
        let half = peak(&scale_wav(&loud, 50)) as f32;
        assert!((half - peak(&loud) as f32 / 2.0).abs() <= 1.0);
        assert_eq!(scale_wav(&loud, 100), loud);
        assert_eq!(scale_wav(b"junk", 50), b"junk");
    }

    /// Live check: plays a real (very quiet) sound on the PC speakers, so it is not part of the normal run.
    #[test]
    #[ignore]
    fn hush_stops_play_quickly() {
        let rate = 22050u32;
        let amp = 0.01f32;
        let pcm: Vec<i16> = (0..rate * 3).map(|i| ((i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * amp * 32767.0) as i16).collect();
        let mut cur = Cursor::new(Vec::new());
        let spec = hound::WavSpec { channels: 1, sample_rate: rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::new(&mut cur, spec).unwrap();
        for s in pcm { w.write_sample(s).unwrap(); }
        w.finalize().unwrap();
        let wav = cur.into_inner();

        let sp = Arc::new(Speaker::new());
        let sp2 = sp.clone();
        let started = Instant::now();
        let h = std::thread::spawn(move || {
            let ok = sp2.play(&wav);
            (ok, Instant::now())
        });
        std::thread::sleep(Duration::from_millis(300));
        assert!(sp.is_speaking() || started.elapsed() > Duration::from_millis(400));
        let t_hush = Instant::now();
        sp.hush();
        let (ok, done) = h.join().unwrap();
        assert!(!sp.is_speaking());
        if !ok && started.elapsed() < Duration::from_millis(1000) && sp.returned_at.lock().unwrap().unwrap() < t_hush {
            println!("no audio device: PlaySound returned immediately, timing assert skipped");
            return;
        }
        let latency = sp.returned_at.lock().unwrap().unwrap().saturating_duration_since(t_hush);
        println!("hush -> PlaySound return: {latency:?} (total with echo sleep {:?})", done - t_hush);
        assert!(latency < Duration::from_millis(100), "hush too slow: {latency:?}");
    }
}
