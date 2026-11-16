//! Loudness normalization of 16-bit PCM WAVs: voiced RMS brought to `TARGET_DBFS`, then a peak limiter.
use std::io::Cursor;

/// Voiced RMS every clip is brought to.
pub const TARGET_DBFS: f32 = -18.0;
const CEILING_DBFS: f32 = -1.0;

fn read(wav: &[u8]) -> Option<(hound::WavSpec, Vec<i16>)> {
    let mut r = hound::WavReader::new(Cursor::new(wav)).ok()?;
    let spec = r.spec();
    if spec.sample_format != hound::SampleFormat::Int || spec.bits_per_sample != 16 || spec.channels == 0 {
        return None;
    }
    let samples = r.samples::<i16>().collect::<Result<Vec<_>, _>>().ok()?;
    Some((spec, samples))
}

/// RMS (linear, full scale = 1) over the 20 ms frames within 30 dB of the loudest frame. 0 for silence.
fn voiced_rms(spec: &hound::WavSpec, s: &[i16]) -> f32 {
    let frame = ((spec.sample_rate as usize * spec.channels as usize) / 50).max(1);
    let energy: Vec<(f64, usize)> = s
        .chunks(frame)
        .map(|c| (c.iter().map(|&v| (v as f64 / 32768.0).powi(2)).sum::<f64>(), c.len()))
        .collect();
    let loudest = energy.iter().map(|(e, n)| e / *n as f64).fold(0.0, f64::max);
    if loudest <= 0.0 {
        return 0.0;
    }
    let floor = loudest * 1e-3; // -30 dB in power
    let (sum, count) = energy.iter().filter(|(e, n)| e / *n as f64 >= floor).fold((0.0, 0usize), |a, (e, n)| (a.0 + e, a.1 + n));
    (sum / count as f64).sqrt() as f32
}

/// Voiced RMS of a WAV in dBFS (-inf for silence or an unreadable file).
pub fn rms_dbfs(wav: &[u8]) -> f32 {
    match read(wav) {
        Some((spec, s)) => 20.0 * voiced_rms(&spec, &s).log10(),
        None => f32::NEG_INFINITY,
    }
}

/// Brings the voiced RMS to `TARGET_DBFS` and keeps peaks under -1 dBFS. Unparseable or silent input is returned unchanged.
pub fn normalize(wav: &[u8]) -> Vec<u8> {
    let Some((spec, s)) = read(wav) else { return wav.to_vec() };
    let rms = voiced_rms(&spec, &s);
    if rms <= 0.0 {
        return wav.to_vec();
    }
    let gain = 10f32.powf(TARGET_DBFS / 20.0) / rms;
    let ceiling = 10f32.powf(CEILING_DBFS / 20.0);
    // Limiter: the gain that fits each sample under the ceiling, looked at 5 ms ahead so it is already down when
    // the peak arrives, then released over ~50 ms. The final clamp guarantees the ceiling.
    let need: Vec<f32> = s
        .iter()
        .map(|&v| {
            let a = (v as f32 / 32768.0 * gain).abs();
            if a > ceiling { ceiling / a } else { 1.0 }
        })
        .collect();
    let ahead = (spec.sample_rate as usize / 200) * spec.channels as usize;
    let release = 1.0 / (spec.sample_rate as f32 * 0.05);
    let mut out = Cursor::new(Vec::new());
    let Ok(mut w) = hound::WavWriter::new(&mut out, spec) else { return wav.to_vec() };
    let mut lim = 1.0f32;
    for (i, &v) in s.iter().enumerate() {
        let target = need[i..(i + ahead + 1).min(need.len())].iter().copied().fold(1.0, f32::min);
        lim = if target < lim { target } else { (lim + release).min(target) };
        let y = (v as f32 / 32768.0 * gain * lim).clamp(-ceiling, ceiling);
        if w.write_sample((y * 32768.0).round().clamp(-32768.0, 32767.0) as i16).is_err() {
            return wav.to_vec();
        }
    }
    if w.finalize().is_err() {
        return wav.to_vec();
    }
    out.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(samples: &[i16], channels: u16, rate: u32) -> Vec<u8> {
        let spec = hound::WavSpec { channels, sample_rate: rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut out = Cursor::new(Vec::new());
        let mut w = hound::WavWriter::new(&mut out, spec).unwrap();
        samples.iter().for_each(|&s| w.write_sample(s).unwrap());
        w.finalize().unwrap();
        out.into_inner()
    }

    /// A sine of `peak_db` dBFS peak followed by 1 s of silence, to exercise the voiced-frame selection.
    fn sine(peak_db: f32, rate: u32) -> Vec<u8> {
        let amp = 10f32.powf(peak_db / 20.0) * 32767.0;
        let mut s: Vec<i16> = (0..rate).map(|i| ((i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * amp) as i16).collect();
        s.extend(std::iter::repeat(0).take(rate as usize));
        wav(&s, 1, rate)
    }

    fn peak_db(w: &[u8]) -> f32 {
        let (_, s) = read(w).unwrap();
        20.0 * (s.iter().map(|v| v.unsigned_abs()).max().unwrap() as f32 / 32768.0).log10()
    }

    #[test]
    fn loud_and_quiet_land_on_target() {
        for db in [0.0, -30.0] {
            let out = normalize(&sine(db, 22050));
            let r = rms_dbfs(&out);
            assert!((r - TARGET_DBFS).abs() < 0.5, "{db}: rms {r}");
            assert!(peak_db(&out) <= CEILING_DBFS + 0.1, "{db}: peak {}", peak_db(&out));
        }
    }

    #[test]
    fn silence_stays_silence_and_junk_is_untouched() {
        let z = wav(&vec![0; 4000], 1, 22050);
        assert_eq!(normalize(&z), z);
        assert_eq!(normalize(b"junk"), b"junk");
    }

    #[test]
    fn stereo_beep_shape() {
        // 110 ms 880 Hz beep with 5 ms fades, peak 0.3, stereo at 44.1 kHz
        let rate = 44100u32;
        let n = (rate as usize * 110) / 1000;
        let mut s = Vec::new();
        for i in 0..n {
            let env = ((i.min(n - 1 - i) as f32 / rate as f32) * 200.0).min(1.0);
            let v = ((i as f32 * 880.0 * std::f32::consts::TAU / rate as f32).sin() * env * 0.3 * 32767.0) as i16;
            s.extend([v, v]);
        }
        let out = normalize(&wav(&s, 2, rate));
        assert!((rms_dbfs(&out) - TARGET_DBFS).abs() < 0.5, "{}", rms_dbfs(&out));
        assert!(peak_db(&out) <= CEILING_DBFS + 0.1);
        assert_eq!(hound::WavReader::new(Cursor::new(&out)).unwrap().spec().channels, 2);
    }

    /// Needs the voice service on 127.0.0.1:3000: `cargo test -p bobby-infer -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn four_voices_match_after_normalize() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut levels = Vec::new();
        for (lang, voice) in [("fr", "bobby"), ("en", "bobby"), ("fr", "claude"), ("en", "claude")] {
            let text = if lang == "fr" { "Ceci est le niveau de référence de Bobby." } else { "This is the reference level of Bobby." };
            let bytes = rt.block_on(async {
                reqwest::Client::new()
                    .get("http://127.0.0.1:3000/tts")
                    .query(&[("text", text), ("lang", lang), ("voice", voice)])
                    .send()
                    .await
                    .unwrap()
                    .bytes()
                    .await
                    .unwrap()
                    .to_vec()
            });
            let (before, out) = (rms_dbfs(&bytes), normalize(&bytes));
            let after = rms_dbfs(&out);
            println!("{lang}/{voice}: {before:.1} -> {after:.1} dBFS, peak {:.1}", peak_db(&out));
            levels.push(after);
        }
        let (lo, hi) = levels.iter().fold((f32::MAX, f32::MIN), |a, &v| (a.0.min(v), a.1.max(v)));
        assert!(hi - lo <= 0.5, "spread {}", hi - lo);
    }
}
