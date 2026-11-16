//! R5 5.3: in-process text-to-speech, a port of Python piper-tts 1.8.0 (voice.py + phonemize_espeak.py).
//! Phonemes come from espeak-ng (same commit as Piper, built by build.rs, linked statically, Piper's own data),
//! then ids -> the VITS model through ONNX Runtime -> peak-normalised s16 mono WAV, like `synthesize_wav`.
use crate::onnx;
use bobby_proto::infer::{InferError, InferErrorKind, Speak, Wav};
use ort::{environment::Environment, session::Session, value::Tensor};
use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::Path;
use std::sync::{Arc, Mutex};
use unicode_normalization::UnicodeNormalization;

const BOS: &str = "^";
const EOS: &str = "$";
const PAD: &str = "_";

fn bad(msg: impl Into<String>) -> InferError {
    InferError::new(InferErrorKind::Bad, msg)
}

// ---- espeak-ng (the three calls Piper's espeakbridge.c uses) ----

extern "C" {
    fn espeak_Initialize(output: c_int, buflength: c_int, path: *const c_char, options: c_int) -> c_int;
    fn espeak_SetVoiceByName(name: *const c_char) -> c_int;
    fn espeak_TextToPhonemesWithTerminator(textptr: *mut *const c_void, textmode: c_int, phonememode: c_int, terminator: *mut c_int) -> *const c_char;
}

const AUDIO_OUTPUT_SYNCHRONOUS: c_int = 2;
const ESPEAK_CHARS_AUTO: c_int = 0;
const ESPEAK_PHONEMES_IPA: c_int = 0x02;
const CLAUSE_INTONATION_COMMA: c_int = 0x1000;
const CLAUSE_INTONATION_QUESTION: c_int = 0x2000;
const CLAUSE_INTONATION_EXCLAMATION: c_int = 0x3000;
const CLAUSE_TYPE_CLAUSE: c_int = 0x40000;
const CLAUSE_TYPE_SENTENCE: c_int = 0x80000;
const CLAUSE_PERIOD: c_int = 40 | CLAUSE_TYPE_SENTENCE;
const CLAUSE_COMMA: c_int = 20 | CLAUSE_INTONATION_COMMA | CLAUSE_TYPE_CLAUSE;
const CLAUSE_QUESTION: c_int = 40 | CLAUSE_INTONATION_QUESTION | CLAUSE_TYPE_SENTENCE;
const CLAUSE_EXCLAMATION: c_int = 45 | CLAUSE_INTONATION_EXCLAMATION | CLAUSE_TYPE_SENTENCE;
const CLAUSE_COLON: c_int = 30 | CLAUSE_TYPE_CLAUSE;
const CLAUSE_SEMICOLON: c_int = 30 | CLAUSE_INTONATION_COMMA | CLAUSE_TYPE_CLAUSE;

/// espeak-ng keeps global state and is not thread safe: every call into it holds this lock.
static ESPEAK: Mutex<()> = Mutex::new(());

/// espeakbridge.get_phonemes: `(phonemes, terminator, end_of_sentence)` per clause. Caller holds the espeak lock.
fn espeak_clauses(text: &str) -> Vec<(String, &'static str, bool)> {
    let c = CString::new(text.replace('\0', "")).unwrap();
    let mut ptr = c.as_ptr() as *const c_void;
    let mut out = Vec::new();
    while !ptr.is_null() {
        let mut term: c_int = 0;
        // SAFETY: `ptr` points into `c` (alive here); espeak advances it and sets it to NULL at the end.
        let p = unsafe { espeak_TextToPhonemesWithTerminator(&mut ptr, ESPEAK_CHARS_AUTO, ESPEAK_PHONEMES_IPA, &mut term) };
        let phonemes = if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() };
        term &= 0x000F_FFFF;
        let t = match term {
            CLAUSE_PERIOD => ".",
            CLAUSE_QUESTION => "?",
            CLAUSE_EXCLAMATION => "!",
            CLAUSE_COMMA => ",",
            CLAUSE_COLON => ":",
            CLAUSE_SEMICOLON => ";",
            _ => "",
        };
        out.push((phonemes, t, term & CLAUSE_TYPE_SENTENCE == CLAUSE_TYPE_SENTENCE));
    }
    out
}

fn select_voice(name: &str) -> Result<(), String> {
    let c = CString::new(name).map_err(|e| e.to_string())?;
    // SAFETY: valid C string; espeak initialised in `LocalTts::new`.
    if unsafe { espeak_SetVoiceByName(c.as_ptr()) } != 0 {
        return Err(format!("espeak-ng cannot set voice {name}"));
    }
    Ok(())
}

/// `re.sub(r"\([^)]+\)", "", s)`: drops the "(en)" language-switch flags.
fn strip_lang_flags(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '(' {
            if let Some(off) = chars[i + 1..].iter().position(|&c| c == ')') {
                if off > 0 {
                    i += off + 2;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn merge_vowel_clusters(phones: Vec<String>, clusters: &[Vec<String>]) -> Vec<String> {
    let max_len = clusters.iter().map(Vec::len).max().unwrap_or(0);
    let mut out = Vec::new();
    let mut i = 0;
    while i < phones.len() {
        let mut hit = None;
        for n in (2..=max_len.min(phones.len() - i)).rev() {
            if clusters.iter().any(|c| c.as_slice() == &phones[i..i + n]) {
                hit = Some(n);
                break;
            }
        }
        match hit {
            Some(n) => {
                out.push(phones[i..i + n].concat());
                i += n;
            }
            None => {
                out.push(phones[i].clone());
                i += 1;
            }
        }
    }
    out
}

/// EspeakPhonemizer.phonemize body: clauses -> one phoneme list per sentence.
fn group_sentences(clauses: Vec<(String, &str, bool)>, vowel_clusters: &[Vec<String>]) -> Vec<Vec<String>> {
    let merge = |s: Vec<String>| if vowel_clusters.is_empty() { s } else { merge_vowel_clusters(s, vowel_clusters) };
    let mut all = Vec::new();
    let mut sentence: Vec<String> = Vec::new();
    for (phonemes, terminator, end) in clauses {
        let mut s = strip_lang_flags(&phonemes);
        s.push_str(terminator);
        if matches!(terminator, "," | ":" | ";") {
            s.push(' ');
        }
        sentence.extend(s.nfd().map(String::from));
        if end {
            all.push(merge(std::mem::take(&mut sentence)));
        }
    }
    if !sentence.is_empty() {
        all.push(merge(sentence));
    }
    all
}

// ---- voices ----

struct Voice {
    id_map: HashMap<String, Vec<i64>>,
    espeak_voice: String,
    sample_rate: u32,
    noise_scale: f32,
    length_scale: f32,
    noise_w: f32,
    num_speakers: i64,
    default_speaker: i64,
    vowel_clusters: Vec<Vec<String>>,
    session: Mutex<Session>,
}

impl Voice {
    fn load(env: &Environment, onnx_path: &Path) -> Result<Self, InferError> {
        let cfg = format!("{}.json", onnx_path.display());
        let json = std::fs::read_to_string(&cfg).map_err(|e| bad(format!("{cfg}: {e}")))?;
        let v: serde_json::Value = serde_json::from_str(&json).map_err(|e| bad(format!("{cfg}: {e}")))?;
        let id_map = v["phoneme_id_map"]
            .as_object()
            .ok_or_else(|| bad("config without phoneme_id_map"))?
            .iter()
            .map(|(k, ids)| (k.clone(), ids.as_array().map(|a| a.iter().filter_map(|x| x.as_i64()).collect()).unwrap_or_default()))
            .collect();
        let f = |p: &str, d: f64| v["inference"][p].as_f64().unwrap_or(d) as f32;
        let vowel_clusters = v["vowel_clusters"]
            .as_array()
            .map(|a| a.iter().map(|c| c.as_array().map(|c| c.iter().filter_map(|s| s.as_str().map(String::from)).collect()).unwrap_or_default()).collect())
            .unwrap_or_default();
        let session = Session::builder(env)
            .and_then(|mut b| b.commit_from_file(onnx_path))
            .map_err(|e| bad(format!("{}: {e}", onnx_path.display())))?;
        Ok(Self {
            id_map,
            espeak_voice: v["espeak"]["voice"].as_str().ok_or_else(|| bad("config without espeak.voice"))?.to_string(),
            sample_rate: v["audio"]["sample_rate"].as_u64().ok_or_else(|| bad("config without audio.sample_rate"))? as u32,
            noise_scale: f("noise_scale", 0.667),
            length_scale: f("length_scale", 1.0),
            noise_w: f("noise_w", 0.8),
            num_speakers: v["num_speakers"].as_i64().unwrap_or(1),
            default_speaker: v["default_speaker_id"].as_i64().unwrap_or(0),
            vowel_clusters,
            session: Mutex::new(session),
        })
    }

    /// One VITS run: ids -> float audio in [-1, 1] (not normalised).
    fn run(&self, ids: &[i64]) -> Result<Vec<f32>, InferError> {
        let e = |e: ort::Error| bad(e.to_string());
        let n = ids.len();
        let input = Tensor::from_array(([1usize, n], ids.to_vec())).map_err(e)?;
        let lens = Tensor::from_array(([1usize], vec![n as i64])).map_err(e)?;
        let scales = Tensor::from_array(([3usize], vec![self.noise_scale, self.length_scale, self.noise_w])).map_err(e)?;
        let mut session = self.session.lock().map_err(|_| bad("session poisoned"))?;
        let out = if self.num_speakers > 1 {
            let sid = Tensor::from_array(([1usize], vec![self.default_speaker])).map_err(e)?;
            session.run(ort::inputs!["input" => input, "input_lengths" => lens, "scales" => scales, "sid" => sid]).map_err(e)?
        } else {
            session.run(ort::inputs!["input" => input, "input_lengths" => lens, "scales" => scales]).map_err(e)?
        };
        let (_, data) = out[0].try_extract_tensor::<f32>().map_err(e)?;
        Ok(data.to_vec())
    }
}

/// `phonemes_to_ids`: BOS, PAD, then each known phoneme followed by PAD (unknown ones are skipped), EOS.
pub fn phonemes_to_ids(phonemes: &[String], id_map: &HashMap<String, Vec<i64>>) -> Vec<i64> {
    let get = |k: &str| id_map.get(k).cloned().unwrap_or_default();
    let mut ids = get(BOS);
    ids.extend(get(PAD));
    for p in phonemes {
        if let Some(v) = id_map.get(p) {
            ids.extend(v);
            ids.extend(get(PAD));
        }
    }
    ids.extend(get(EOS));
    ids
}

/// Float audio -> s16 exactly like Piper: peak-normalise per sentence (normalize_audio), clip, `* 32767`, truncate.
fn to_i16(mut audio: Vec<f32>) -> Vec<i16> {
    let max = audio.iter().fold(0f32, |m, x| m.max(x.abs()));
    if max < 1e-8 {
        audio.iter_mut().for_each(|x| *x = 0.0);
    } else {
        audio.iter_mut().for_each(|x| *x /= max);
    }
    audio.iter().map(|x| (x.clamp(-1.0, 1.0) * 32767.0).clamp(-32767.0, 32767.0) as i16).collect()
}

/// The 44-byte-header PCM WAV that Python's `wave` module writes (mono, 16 bit).
pub fn wav_bytes(rate: u32, pcm: &[i16]) -> Vec<u8> {
    let data = (pcm.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + pcm.len() * 2);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data.to_le_bytes());
    w.extend(pcm.iter().flat_map(|s| s.to_le_bytes()));
    w
}

/// `PiperVoice.phonemize` for espeak voices: `[[ raw phonemes ]]` blocks pass through, the rest goes to espeak.
fn phonemize(text: &str, voice: &Voice) -> Vec<Vec<String>> {
    // re.split(r"(\[\[.*?\]\])", text): alternating text / block parts
    let mut parts: Vec<&str> = Vec::new();
    let mut rest = text;
    while let Some(a) = rest.find("[[") {
        match rest[a + 2..].find("]]") {
            Some(b) => {
                parts.push(&rest[..a]);
                parts.push(&rest[a..a + 2 + b + 2]);
                rest = &rest[a + 2 + b + 2..];
            }
            None => break,
        }
    }
    parts.push(rest);
    let mut phonemes: Vec<Vec<String>> = Vec::new();
    let mut prev_raw = false;
    for (i, part) in parts.iter().enumerate() {
        if part.starts_with("[[") {
            prev_raw = true;
            if phonemes.is_empty() {
                phonemes.push(Vec::new());
            }
            if i > 0 && parts[i - 1].ends_with(' ') {
                phonemes.last_mut().unwrap().push(" ".into());
            }
            phonemes.last_mut().unwrap().extend(part[2..part.len() - 2].trim().chars().map(String::from));
            if i + 1 < parts.len() && parts[i + 1].starts_with(' ') {
                phonemes.last_mut().unwrap().push(" ".into());
            }
            continue;
        }
        let mut sentences = {
            let _g = ESPEAK.lock().unwrap_or_else(|p| p.into_inner());
            if let Err(e) = select_voice(&voice.espeak_voice) {
                eprintln!("piper: {e}");
                return Vec::new();
            }
            group_sentences(espeak_clauses(part), &voice.vowel_clusters)
        };
        if prev_raw && !sentences.is_empty() {
            let first = sentences.remove(0);
            phonemes.last_mut().unwrap().extend(first);
        }
        phonemes.extend(sentences);
        prev_raw = false;
    }
    if phonemes.last().is_some_and(Vec::is_empty) {
        phonemes.pop();
    }
    phonemes
}

struct Inner {
    voices: HashMap<(&'static str, &'static str), Voice>,
    _env: Environment,
}

impl Inner {
    fn voice(&self, lang: &str, voice: &str) -> &Voice {
        let who = if voice == "claude" { "claude" } else { "bobby" };
        let lang = if lang == "en" { "en" } else { "fr" }; // unknown language: French, like server.py
        &self.voices[&(who, lang)]
    }

    fn ids(&self, text: &str, lang: &str, voice: &str) -> Vec<Vec<i64>> {
        let v = self.voice(lang, voice);
        phonemize(text, v).iter().filter(|p| !p.is_empty()).map(|p| phonemes_to_ids(p, &v.id_map)).collect()
    }

    fn wav(&self, text: &str, lang: &str, voice: &str) -> Result<Vec<u8>, InferError> {
        let v = self.voice(lang, voice);
        let mut pcm = Vec::new();
        for ids in self.ids(text, lang, voice) {
            pcm.extend(to_i16(v.run(&ids)?));
        }
        Ok(wav_bytes(v.sample_rate, &pcm))
    }
}

/// Piper voices in this process: bobby = fr siwis / en lessac, claude = fr tom / en ryan (as voice/server.py).
#[derive(Clone)]
pub struct LocalTts {
    inner: Arc<Inner>,
}

impl LocalTts {
    /// `piper_dir`: `models/piper` (voices; `onnxruntime.dll` in its parent). `espeak_data_dir`: Piper's `espeak-ng-data`.
    pub fn new(piper_dir: &Path, espeak_data_dir: &Path) -> Result<Self, InferError> {
        let env = onnx::env(piper_dir).map_err(bad)?;
        {
            let _g = ESPEAK.lock().unwrap_or_else(|p| p.into_inner());
            let data = CString::new(espeak_data_dir.to_string_lossy().as_bytes()).map_err(|e| bad(e.to_string()))?;
            // SAFETY: valid C string; synchronous mode, no audio output.
            if unsafe { espeak_Initialize(AUDIO_OUTPUT_SYNCHRONOUS, 0, data.as_ptr(), 0) } < 0 {
                return Err(bad(format!("espeak-ng cannot initialise with {}", espeak_data_dir.display())));
            }
        }
        let mut voices = HashMap::new();
        for (who, lang, file) in [
            ("bobby", "fr", "fr_FR-siwis-medium"),
            ("bobby", "en", "en_US-lessac-medium"),
            ("claude", "fr", "fr_FR-tom-medium"),
            ("claude", "en", "en_US-ryan-medium"),
        ] {
            voices.insert((who, lang), Voice::load(&env, &piper_dir.join(format!("{file}.onnx")))?);
        }
        Ok(Self { inner: Arc::new(Inner { voices, _env: env }) })
    }

    /// Phoneme ids per sentence, exactly as Piper computes them (empty sentences dropped).
    pub fn phoneme_ids(&self, text: &str, lang: &str, voice: &str) -> Vec<Vec<i64>> {
        self.inner.ids(text, lang, voice)
    }

    /// Blocking synthesis: a WAV file (mono s16, sample rate of the voice), like `synthesize_wav`.
    pub fn wav(&self, text: &str, lang: &str, voice: &str) -> Result<Vec<u8>, InferError> {
        self.inner.wav(text, lang, voice)
    }
}

impl Speak for LocalTts {
    async fn speak(&self, text: &str, lang: &str, voice: &str) -> Result<Wav, InferError> {
        let inner = self.inner.clone();
        let (t, l, v) = (text.to_string(), lang.to_string(), voice.to_string());
        tokio::task::spawn_blocking(move || inner.wav(&t, &l, &v)).await.map_err(|e| bad(format!("tts task: {e}")))?.map(Wav)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn map() -> HashMap<String, Vec<i64>> {
        [("_", vec![0]), ("^", vec![1]), ("$", vec![2]), (" ", vec![3]), ("a", vec![14]), ("aɪ", vec![161])].into_iter().map(|(k, v)| (k.to_string(), v)).collect()
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn ids_have_bos_pad_eos_and_skip_unknown() {
        let ids = phonemes_to_ids(&s(&["a", " ", "?", "aɪ"]), &map());
        assert_eq!(ids, vec![1, 0, 14, 0, 3, 0, 161, 0, 2]);
        assert_eq!(phonemes_to_ids(&[], &map()), vec![1, 0, 2]);
    }

    #[test]
    fn clauses_group_into_sentences() {
        let clauses = vec![("bɔ̃".to_string(), ",", false), ("ʒuʁ(en)".to_string(), ".", true), ("sa".to_string(), "", false)];
        let g = group_sentences(clauses, &[]);
        // NFD keeps the combining tilde apart; "," gets a trailing space; the "(en)" flag is dropped
        assert_eq!(g[0], s(&["b", "ɔ", "\u{303}", ",", " ", "ʒ", "u", "ʁ", "."]));
        assert_eq!(g[1], s(&["s", "a"]));
        assert_eq!(strip_lang_flags("a(en)b()c(d"), "ab()c(d");
        assert_eq!(merge_vowel_clusters(s(&["a", "ɪ", "b"]), &[s(&["a", "ɪ"])]), s(&["aɪ", "b"]));
    }

    #[test]
    fn pcm_conversion_normalises_and_truncates() {
        assert_eq!(to_i16(vec![0.25, -0.5, 0.1]), vec![16383, -32767, 6553]);
        assert_eq!(to_i16(vec![0.0; 3]), vec![0; 3]);
    }

    #[test]
    fn wav_header() {
        let w = wav_bytes(22050, &[1, -2, 3]);
        assert_eq!(w.len(), 44 + 6);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(w[4..8].try_into().unwrap()), 36 + 6);
        assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(u16::from_le_bytes(w[22..24].try_into().unwrap()), 1); // mono
        assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), 22050);
        assert_eq!(u32::from_le_bytes(w[28..32].try_into().unwrap()), 44100);
        assert_eq!(&w[36..40], b"data");
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 6);
        assert_eq!(&w[44..46], &1i16.to_le_bytes());
    }

    fn data_dir() -> Option<PathBuf> {
        let m = Path::new(env!("CARGO_MANIFEST_DIR"));
        [std::env::var_os("BOBBY_MODELS").map(PathBuf::from), Some(m.join("../../models")), Some(m.join("../../../../../models"))]
            .into_iter()
            .flatten()
            .map(|p| p.join("espeak-ng-data"))
            .find(|p| p.join("phondata").exists())
    }

    #[test]
    fn espeak_phonemizes_french() {
        let Some(dir) = data_dir() else {
            println!("skipped: models/espeak-ng-data not found");
            return;
        };
        let c = CString::new(dir.to_string_lossy().as_bytes()).unwrap();
        let _g = ESPEAK.lock().unwrap_or_else(|p| p.into_inner());
        assert!(unsafe { espeak_Initialize(AUDIO_OUTPUT_SYNCHRONOUS, 0, c.as_ptr(), 0) } >= 0);
        select_voice("fr").unwrap();
        let sentences = group_sentences(espeak_clauses("Bonjour. Ça va ?"), &[]);
        assert_eq!(sentences.len(), 2);
        assert_eq!(sentences[0].last().unwrap(), ".");
        assert_eq!(sentences[1].last().unwrap(), "?");
        assert!(sentences[0].len() > 4);
    }
}
