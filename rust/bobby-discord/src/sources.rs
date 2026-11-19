//! Where sound comes from (port of `bot/music/sources.js`): decoding of radio / YouTube / TTS audio into the
//! PCM pipe, Radio Browser (web radio by style) and yt-dlp (YouTube search and listings).
//! Decoding is symphonia first (mp3/aac radio streams, WAV from the TTS: no process to spawn, no startup
//! latency) and ffmpeg for everything else (YouTube webm/opus, HLS...).

use std::io::{Read, Seek, SeekFrom};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};
use serde::Serialize;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::pcm::{pipe, PcmRx, PcmTx, RATE};

const UA: &str = "Bobby/1.0";
const RADIO_API: &str = "https://de1.api.radio-browser.info/json/stations";
const YT_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Serialize)]
pub struct Station {
    pub station: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub url: String,
    pub duration: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub id: String,
    pub title: String,
    pub channel: String,
    pub duration: Option<f64>,
}

/// No console window flashing up for each spawned process (Windows only).
fn quiet(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

// ---------------------------------------------------------------------------------------------------------
// Output side: any source format and rate -> s16 48 kHz stereo chunks in the pipe.

/// Resamples (when needed) and sends. `push` / `finish` return false once the receiver is gone.
struct Out {
    tx: PcmTx,
    rs: Option<Fft<f32>>,
    pending: Vec<f32>, // interleaved stereo waiting for a full resampler chunk
}

impl Out {
    fn new(tx: &PcmTx, rate: u32) -> Result<Out, String> {
        let rs = if rate == RATE {
            None
        } else {
            Some(Fft::new(rate as usize, RATE as usize, 1024, 2, FixedSync::Input).map_err(|e| e.to_string())?)
        };
        Ok(Out { tx: tx.clone(), rs, pending: Vec::new() })
    }

    fn push(&mut self, stereo: &[i16]) -> bool {
        if self.rs.is_none() {
            return self.tx.send(stereo.to_vec()).is_ok();
        }
        self.pending.extend(stereo.iter().map(|&s| s as f32 / 32768.0));
        self.drain(false)
    }

    /// Flushes the resampler tail (a partial last chunk).
    fn finish(mut self) -> bool {
        self.drain(true)
    }

    fn drain(&mut self, flush: bool) -> bool {
        let Some(rs) = self.rs.as_mut() else { return true };
        loop {
            let need = rs.input_frames_next();
            let have = self.pending.len() / 2;
            let partial = if have >= need {
                None
            } else if flush && have > 0 {
                self.pending.resize(need * 2, 0.0);
                Some(have)
            } else {
                return true;
            };
            let max = rs.output_frames_max();
            let mut out = vec![0f32; max * 2];
            let done = (|| {
                let inp = InterleavedSlice::new(&self.pending[..need * 2], 2, need).ok()?;
                let mut o = InterleavedSlice::new_mut(&mut out, 2, max).ok()?;
                let idx = partial.map(|p| Indexing { input_offset: 0, output_offset: 0, partial_len: Some(p), active_channels_mask: None });
                rs.process_into_buffer(&inp, &mut o, idx.as_ref()).ok()
            })();
            let Some((n_in, n_out)) = done else { return false };
            let chunk: Vec<i16> = out[..n_out * 2].iter().map(|x| (x * 32768.0).round().clamp(-32768.0, 32767.0) as i16).collect();
            if !chunk.is_empty() && self.tx.send(chunk).is_err() {
                return false;
            }
            if partial.is_some() {
                self.pending.clear();
                return true;
            }
            self.pending.drain(..n_in * 2);
        }
    }
}

/// Interleaved samples of `ch` channels -> interleaved stereo (mono is duplicated, extra channels dropped).
fn to_stereo(s: &[i16], ch: usize) -> Vec<i16> {
    match ch {
        2 => s.to_vec(),
        1 => s.iter().flat_map(|&x| [x, x]).collect(),
        _ => s.chunks_exact(ch).flat_map(|f| [f[0], f[1]]).collect(),
    }
}

/// Decodes any container symphonia knows. Err = could not even open it (the caller may fall back to ffmpeg);
/// Ok(true) = the receiver is gone; Ok(false) = end of stream (or a read error mid-stream).
fn run_symphonia(mss: MediaSourceStream, hint: &Hint, tx: &PcmTx) -> Result<bool, String> {
    let mut fmt = symphonia::default::get_probe()
        .format(hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| e.to_string())?
        .format;
    let track = fmt.default_track().ok_or("no audio track")?;
    let id = track.id;
    let mut dec = symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default()).map_err(|e| e.to_string())?;
    let mut out: Option<Out> = None;
    loop {
        let Ok(pkt) = fmt.next_packet() else { break };
        if pkt.track_id() != id {
            continue;
        }
        let buf = match dec.decode(&pkt) {
            Ok(b) => b,
            Err(SymError::DecodeError(_)) => continue, // a corrupt frame is skipped, not fatal
            Err(_) => break,
        };
        let spec = *buf.spec();
        if out.is_none() {
            out = Some(Out::new(tx, spec.rate)?);
        }
        let mut sb = SampleBuffer::<i16>::new(buf.capacity() as u64, spec);
        sb.copy_interleaved_ref(buf);
        let stereo = to_stereo(sb.samples(), spec.channels.count());
        if !out.as_mut().is_some_and(|o| o.push(&stereo)) {
            return Ok(true);
        }
    }
    Ok(out.is_some_and(|o| !o.finish()))
}

/// Streaming HTTP body as a symphonia source: not seekable, so containers needing a seek (mp4, webm) fail
/// to open and go to ffmpeg.
struct HttpSource(reqwest::blocking::Response);

impl Read for HttpSource {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(b)
    }
}
impl Seek for HttpSource {
    fn seek(&mut self, _: SeekFrom) -> std::io::Result<u64> {
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "stream"))
    }
}
impl MediaSource for HttpSource {
    fn is_seekable(&self) -> bool {
        false
    }
    fn byte_len(&self) -> Option<u64> {
        None
    }
}

fn symphonia_url(url: &str, tx: &PcmTx) -> Result<bool, String> {
    // No overall timeout: a radio never ends.
    let client = reqwest::blocking::Client::builder()
        .user_agent(UA)
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let res = client.get(url).send().and_then(|r| r.error_for_status()).map_err(|e| e.to_string())?;
    let ct = res.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_lowercase();
    if ["webm", "mp4", "mpegurl", "matroska", "ogg"].iter().any(|k| ct.contains(k)) {
        return Err(format!("content-type {ct}"));
    }
    let mut hint = Hint::new();
    if !ct.is_empty() {
        hint.mime_type(ct.split(';').next().unwrap_or(""));
    }
    run_symphonia(MediaSourceStream::new(Box::new(HttpSource(res)), Default::default()), &hint, tx)
}

fn kill_tree(pid: u32) {
    #[cfg(windows)]
    let mut cmd = Command::new("taskkill");
    #[cfg(windows)]
    cmd.args(["/pid", &pid.to_string(), "/T", "/F"]);
    #[cfg(not(windows))]
    let mut cmd = Command::new("kill");
    #[cfg(not(windows))]
    cmd.args(["-9", &pid.to_string()]);
    let _ = quiet(&mut cmd).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
}

/// Same arguments as sources.js. Returns when the stream ends or the receiver is dropped (ffmpeg is killed).
fn run_ffmpeg(url: &str, _live: bool, tx: &PcmTx) {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-nostdin", "-loglevel", "error"]);
    // all http(s) inputs: YouTube closes idle connections while the pipe is full, a plain EOF would skip the track
    cmd.args(["-reconnect", "1", "-reconnect_streamed", "1", "-reconnect_on_network_error", "1", "-reconnect_delay_max", "5"]);
    cmd.args(["-user_agent", UA, "-i", url, "-vn", "-f", "s16le", "-ar", "48000", "-ac", "2", "pipe:1"]);
    let Ok(mut child): Result<Child, _> = quiet(&mut cmd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() else { return };
    let mut stdout = child.stdout.take().expect("piped");
    if let Some(stderr) = child.stderr.take() {
        std::thread::spawn(move || {
            for line in std::io::BufRead::lines(std::io::BufReader::new(stderr)).map_while(Result::ok) {
                eprintln!("[ffmpeg] {line}");
            }
        });
    }
    let mut buf = [0u8; 8192];
    let mut carry = 0; // bytes of a sample cut by a short read
    while let Ok(n) = stdout.read(&mut buf[carry..]) {
        if n == 0 {
            break;
        }
        let total = carry + n;
        let whole = total - total % 2;
        let samples: Vec<i16> = buf[..whole].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        if tx.send(samples).is_err() {
            break;
        }
        buf.copy_within(whole..total, 0);
        carry = total - whole;
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Radio or YouTube URL -> PCM. A thread decodes into the pipe; dropping the `PcmRx` stops it.
pub fn decode(url: &str, live: bool) -> PcmRx {
    let (tx, rx) = pipe();
    let url = url.to_string();
    std::thread::spawn(move || {
        let mut first = true;
        let mut fails = 0;
        loop {
            match symphonia_url(&url, &tx) {
                Ok(true) => return,
                Ok(false) if !live => return,
                Ok(false) => fails = 0, // a live stream that ended: reconnect, as ffmpeg's -reconnect did
                Err(e) if first => {
                    eprintln!("[sources] symphonia cannot open the stream ({e}), using ffmpeg");
                    return run_ffmpeg(&url, live, &tx);
                }
                Err(_) => {
                    fails += 1;
                    if fails >= 3 {
                        return;
                    }
                }
            }
            first = false;
            std::thread::sleep(Duration::from_secs(1));
        }
    });
    rx
}

/// A WAV in memory (the TTS answer, often 22050 Hz mono) -> PCM.
pub fn decode_wav(bytes: Vec<u8>) -> PcmRx {
    let (tx, rx) = pipe();
    std::thread::spawn(move || {
        let mut hint = Hint::new();
        hint.with_extension("wav");
        let mss = MediaSourceStream::new(Box::new(std::io::Cursor::new(bytes)), Default::default());
        let _ = run_symphonia(mss, &hint, &tx); // a bad WAV just gives an empty stream (read == 0)
    });
    rx
}

// ---------------------------------------------------------------------------------------------------------
// Radio Browser and yt-dlp

/// Stations for a style/genre, best voted first.
pub async fn radio(style: &str) -> Result<Vec<Station>, String> {
    let tag: String = url_encode(&style.trim().to_lowercase());
    let q = "?hidebroken=true&order=votes&reverse=true&limit=10";
    let client = reqwest::Client::builder().user_agent(UA).build().map_err(|e| e.to_string())?;
    for route in ["bytag", "bytagexact", "byname"] {
        let body = client.get(format!("{RADIO_API}/{route}/{tag}{q}")).send().await.map_err(|e| e.to_string())?.text().await.map_err(|e| e.to_string())?;
        let list: Vec<serde_json::Value> = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        let found: Vec<Station> = list
            .iter()
            .filter_map(|s| {
                let url = s["url_resolved"].as_str().filter(|u| !u.is_empty())?;
                Some(Station { station: s["name"].as_str().unwrap_or("").trim().to_string(), url: url.to_string() })
            })
            .collect();
        if !found.is_empty() {
            return Ok(found);
        }
    }
    Err(format!("no radio found for \"{style}\""))
}

/// Percent-encoding like JS `encodeURIComponent` (unreserved characters kept, the rest as UTF-8 bytes).
fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

fn ytdlp_path() -> String {
    if let Ok(p) = std::env::var("YTDLP") {
        return p;
    }
    if cfg!(windows) {
        let root = std::env::var("BOBBY_ROOT").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../..").to_string());
        format!("{root}/_bin/yt-dlp.exe")
    } else {
        "yt-dlp".to_string()
    }
}

/// Runs yt-dlp and returns its stdout. At the timeout the whole process tree is killed: the PyInstaller
/// yt-dlp.exe is a launcher whose child keeps the pipes open, so killing only the launcher hangs forever
/// (the bug sources.js documents, 2026-11-13).
async fn ytdlp(args: Vec<String>) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(ytdlp_path());
        let child = quiet(&mut cmd).args(&args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("yt-dlp failed: {e}"))?;
        let pid = child.id();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = done_tx.send(child.wait_with_output());
        });
        match done_rx.recv_timeout(YT_TIMEOUT) {
            Ok(Ok(o)) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).into_owned()),
            Ok(Ok(o)) => {
                let err = String::from_utf8_lossy(&o.stderr);
                let line = err.lines().find(|l| !l.trim().is_empty()).map(str::to_string).unwrap_or_else(|| o.status.to_string());
                Err(format!("yt-dlp failed: {line}"))
            }
            Ok(Err(e)) => Err(format!("yt-dlp failed: {e}")),
            Err(_) => {
                kill_tree(pid);
                Err("yt-dlp failed: no answer after 20 s".to_string())
            }
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Search text or URL -> best audio stream. `-I 1`: an artist name makes ytsearch1 return a whole shelf of
/// videos (33 s instead of 3.5 s).
pub async fn youtube(query: &str) -> Result<Track, String> {
    let target = if query.starts_with("http://") || query.starts_with("https://") {
        query.to_string()
    } else {
        format!("ytsearch1:{}", query.strip_prefix("ytsearch1:").unwrap_or(query))
    };
    let args = ["-f", "bestaudio/best", "--no-playlist", "-I", "1", "--no-warnings", "-j", &target].map(String::from).to_vec();
    let out = ytdlp(args).await?;
    let j: serde_json::Value = serde_json::from_str(out.lines().next().unwrap_or("")).unwrap_or_default();
    match j["url"].as_str().filter(|u| !u.is_empty()) {
        Some(url) => Ok(Track { id: j["id"].as_str().unwrap_or("").to_string(), title: j["title"].as_str().unwrap_or("").to_string(), url: url.to_string(), duration: j["duration"].as_f64() }),
        None => Err(format!("nothing found on YouTube for \"{query}\"")),
    }
}

/// Video list without resolving streams (about 2 s): a search ("ytsearch30:...") or a YouTube Mix.
pub async fn listing(target: &str, n: usize) -> Result<Vec<Entry>, String> {
    let range = format!("1:{n}");
    let args = ["--flat-playlist", "-J", "--no-warnings", "-I", &range, target].map(String::from).to_vec();
    let out = ytdlp(args).await?;
    let j: serde_json::Value = serde_json::from_str(&out).unwrap_or_default();
    let text = |e: &serde_json::Value, k: &str| e[k].as_str().map(str::to_string);
    Ok(j["entries"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|e| Entry {
                    id: text(e, "id").unwrap_or_default(),
                    title: text(e, "title").unwrap_or_default(),
                    channel: text(e, "channel").or_else(|| text(e, "uploader")).unwrap_or_default(),
                    duration: e["duration"].as_f64(),
                })
                .collect()
        })
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pcm::FRAME;

    /// 16-bit PCM WAV with the 44-byte header written by hand.
    fn wav(rate: u32, ch: u16, samples: &[i16]) -> Vec<u8> {
        let data = (samples.len() * 2) as u32;
        let mut v = Vec::new();
        v.extend(b"RIFF");
        v.extend((36 + data).to_le_bytes());
        v.extend(b"WAVEfmt ");
        v.extend(16u32.to_le_bytes());
        v.extend(1u16.to_le_bytes()); // PCM
        v.extend(ch.to_le_bytes());
        v.extend(rate.to_le_bytes());
        v.extend((rate * ch as u32 * 2).to_le_bytes());
        v.extend((ch * 2).to_le_bytes());
        v.extend(16u16.to_le_bytes());
        v.extend(b"data");
        v.extend(data.to_le_bytes());
        v.extend(samples.iter().flat_map(|s| s.to_le_bytes()));
        v
    }

    /// Reads until the stream ends, or `max` samples are in (None = to the end).
    fn collect(mut rx: PcmRx, max: Option<usize>) -> Vec<i16> {
        let (mut all, mut f) = (Vec::new(), [0i16; FRAME]);
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while !rx.ended() && max.map_or(true, |m| all.len() < m) && std::time::Instant::now() < deadline {
            let n = rx.take(&mut f);
            all.extend_from_slice(&f[..n]);
            if n == 0 {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        all
    }

    fn peak(v: &[i16]) -> i32 {
        v.iter().map(|&s| (s as i32).abs()).max().unwrap_or(0)
    }

    #[test]
    fn wav_48k_stereo_is_passed_through_unchanged() {
        let s: Vec<i16> = (0..9600).map(|i| ((i * 7) % 20000 - 10000) as i16).collect();
        assert_eq!(collect(decode_wav(wav(48000, 2, &s)), None), s);
    }

    #[test]
    fn wav_22k_mono_becomes_48k_stereo() {
        let mono: Vec<i16> = (0..22050).map(|i| ((i as f32 * 440.0 * std::f32::consts::TAU / 22050.0).sin() * 12000.0) as i16).collect();
        let out = collect(decode_wav(wav(22050, 1, &mono)), None);
        assert!((94080..=97920).contains(&out.len()), "len {}", out.len());
        assert!(peak(&out) > 8000, "peak {}", peak(&out));
        assert!(out.chunks_exact(2).all(|f| f[0] == f[1]), "mono duplicated to both channels");
    }

    #[test]
    fn dropping_the_receiver_stops_the_thread() {
        let rx = decode_wav(wav(48000, 2, &vec![5; 480_000]));
        drop(rx); // must not hang or panic: the producer's send fails and it exits
    }

    #[test]
    fn url_encoding_matches_js() {
        assert_eq!(url_encode("drum & bass é"), "drum%20%26%20bass%20%C3%A9");
    }

    fn ytdlp_env() {
        std::env::set_var("YTDLP", "C:/Users/alexa/Desktop/Personal Projects/discord-bot/_bin/yt-dlp.exe");
    }

    #[tokio::test]
    #[ignore = "network"]
    async fn network_radio_decodes() {
        let stations = radio("jazz").await.unwrap();
        println!("radio: {} ({})", stations[0].station, stations[0].url);
        let pcm = collect(decode(&stations[0].url, true), Some(5 * 96000));
        println!("radio peak {}", peak(&pcm));
        assert!(peak(&pcm) > 500);
    }

    #[tokio::test]
    #[ignore = "network"]
    async fn network_youtube_decodes() {
        ytdlp_env();
        let t = youtube("ytsearch1:lofi hip hop").await.unwrap();
        println!("youtube: {} ({})", t.title, t.id);
        let pcm = collect(decode(&t.url, false), Some(5 * 96000));
        println!("youtube peak {}", peak(&pcm));
        assert!(peak(&pcm) > 500);
    }

    #[tokio::test]
    #[ignore = "network"]
    async fn network_listing_has_ids() {
        ytdlp_env();
        let l = listing("ytsearch5:daft punk", 5).await.unwrap();
        assert!(!l.is_empty() && l.iter().all(|e| !e.id.is_empty()), "{l:?}");
    }
}
