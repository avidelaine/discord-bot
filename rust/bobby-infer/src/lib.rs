//! Model calls by address: speech-to-text, LLM, text-to-speech.
//! HTTP clients for today's services (voice/server.py on :3000, llama-server on :8080).
//! Later an in-process engine implements the same traits; the setting value "local" selects it.
use bobby_proto::infer::{ChatMsg, InferError, InferErrorKind, Speak, Transcribe, Transcript, Understand, Wav};
use std::time::Duration;

fn trim(base: &str) -> String {
    base.trim_end_matches('/').to_string()
}

fn map_err(e: reqwest::Error) -> InferError {
    let kind = if e.is_timeout() {
        InferErrorKind::Timeout
    } else if e.is_connect() {
        InferErrorKind::Down
    } else {
        InferErrorKind::Bad
    };
    InferError::new(kind, e.to_string())
}

async fn ok(res: reqwest::Response) -> Result<reqwest::Response, InferError> {
    if res.status().is_success() {
        Ok(res)
    } else {
        Err(InferError::new(InferErrorKind::Bad, format!("HTTP {}", res.status())))
    }
}

/// Percent-encode everything but unreserved characters (the server does `unquote`).
fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// 16 kHz mono f32 -> raw s16le 48 kHz stereo (each sample repeated 3 times, on both channels).
/// This is what `/transcribe` expects (`to_16k` on the server undoes it).
pub fn to_48k_stereo_s16le(pcm16k: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pcm16k.len() * 12);
    for &s in pcm16k {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        let b = v.to_le_bytes();
        for _ in 0..6 {
            out.extend_from_slice(&b);
        }
    }
    out
}

/// Speech-to-text through voice/server.py `POST /transcribe`.
pub struct HttpStt {
    base: String,
    http: reqwest::Client,
}

impl HttpStt {
    pub fn new(base_url: &str) -> Self {
        Self { base: trim(base_url), http: reqwest::Client::new() }
    }
    fn url(&self) -> String {
        format!("{}/transcribe", self.base)
    }
}

impl Transcribe for HttpStt {
    /// Takes 16 kHz mono f32; sends 48 kHz stereo s16le (the server's only input format), wake gate skipped (`x-force: 1`).
    async fn transcribe(&self, pcm16k: &[f32], hotwords: &str) -> Result<Transcript, InferError> {
        let res = self
            .http
            .post(self.url())
            .header("x-force", "1")
            .header("x-hotwords", encode(hotwords))
            .body(to_48k_stereo_s16le(pcm16k))
            .send()
            .await
            .map_err(map_err)?;
        let v: serde_json::Value = ok(res).await?.json().await.map_err(map_err)?;
        Ok(Transcript {
            text: v["text"].as_str().unwrap_or("").to_string(),
            lang: v["lang"].as_str().unwrap_or("").to_string(),
            ms: v["ms"].as_u64().unwrap_or(0) as u32,
        })
    }
}

/// llama-server (OpenAI-compatible) `POST /v1/chat/completions`.
pub struct HttpLlm {
    base: String,
    http: reqwest::Client,
}

impl HttpLlm {
    pub fn new(base_url: &str) -> Self {
        // 20 s: a normal answer takes under 2 s; past that the GPU is saturated, and aborting frees the single slot.
        let http = reqwest::Client::builder().timeout(Duration::from_secs(20)).build().expect("reqwest client");
        Self { base: trim(base_url), http }
    }
    fn url(&self) -> String {
        format!("{}/v1/chat/completions", self.base)
    }
}

impl Understand for HttpLlm {
    async fn chat(&self, messages: &[ChatMsg], schema: &serde_json::Value, max_tokens: u32) -> Result<serde_json::Value, InferError> {
        let msgs: Vec<_> = messages.iter().map(|m| serde_json::json!({"role": m.role, "content": m.content})).collect();
        let body = serde_json::json!({
            "messages": msgs,
            "temperature": 0,
            "max_tokens": max_tokens,
            "response_format": {"type": "json_schema", "json_schema": {"name": "plan", "schema": schema}},
        });
        let res = self.http.post(self.url()).json(&body).send().await.map_err(map_err)?;
        let v: serde_json::Value = ok(res).await?.json().await.map_err(map_err)?;
        let text = v["choices"][0]["message"]["content"].as_str().unwrap_or("");
        serde_json::from_str(text).map_err(|e| InferError::new(InferErrorKind::Bad, format!("invalid JSON from model ({e}): {}", text.chars().take(200).collect::<String>())))
    }
}

/// Text-to-speech through voice/server.py `GET /tts`.
pub struct HttpTts {
    base: String,
    http: reqwest::Client,
}

impl HttpTts {
    pub fn new(base_url: &str) -> Self {
        Self { base: trim(base_url), http: reqwest::Client::new() }
    }
    fn request(&self, text: &str, lang: &str, voice: &str) -> reqwest::RequestBuilder {
        self.http.get(format!("{}/tts", self.base)).query(&[("text", text), ("lang", lang), ("voice", voice)])
    }
}

impl Speak for HttpTts {
    async fn speak(&self, text: &str, lang: &str, voice: &str) -> Result<Wav, InferError> {
        let res = self.request(text, lang, voice).send().await.map_err(map_err)?;
        let bytes = ok(res).await?.bytes().await.map_err(map_err)?;
        if !bytes.starts_with(b"RIFF") {
            return Err(InferError::new(InferErrorKind::Bad, "reply is not a WAV file"));
        }
        Ok(Wav(bytes.to_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEAD: &str = "http://127.0.0.1:1";

    #[test]
    fn urls() {
        assert_eq!(HttpStt::new("http://x:3000/").url(), "http://x:3000/transcribe");
        assert_eq!(HttpLlm::new("http://x:8080").url(), "http://x:8080/v1/chat/completions");
        let r = HttpTts::new("http://x:3000").request("salut Bobby", "fr", "bobby").build().unwrap();
        assert_eq!(r.url().as_str(), "http://x:3000/tts?text=salut+Bobby&lang=fr&voice=bobby");
    }

    #[test]
    fn encoding_and_pcm() {
        assert_eq!(encode("Bobby é"), "Bobby%20%C3%A9");
        let b = to_48k_stereo_s16le(&[1.0, -1.0]);
        assert_eq!(b.len(), 24);
        assert_eq!(&b[0..2], &32767i16.to_le_bytes());
        assert_eq!(&b[22..24], &(-32767i16).to_le_bytes());
    }

    #[tokio::test]
    async fn refused_port_is_down() {
        let e = HttpStt::new(DEAD).transcribe(&[0.0; 160], "").await.unwrap_err();
        assert_eq!(e.kind, InferErrorKind::Down);
        let e = HttpLlm::new(DEAD).chat(&[ChatMsg::new("user", "hi")], &serde_json::json!({}), 10).await.unwrap_err();
        assert_eq!(e.kind, InferErrorKind::Down);
        let e = HttpTts::new(DEAD).speak("a", "fr", "bobby").await.unwrap_err();
        assert_eq!(e.kind, InferErrorKind::Down);
    }

    /// Minimal WAV decode: 16-bit PCM mono/stereo -> 16 kHz mono f32 (nearest-sample resampling).
    fn decode_wav(w: &[u8]) -> Vec<f32> {
        let (mut pos, mut ch, mut rate) = (12, 1usize, 22050u32);
        while pos + 8 <= w.len() {
            let id = &w[pos..pos + 4];
            let len = u32::from_le_bytes(w[pos + 4..pos + 8].try_into().unwrap()) as usize;
            let body = pos + 8;
            if id == b"fmt " {
                ch = u16::from_le_bytes(w[body + 2..body + 4].try_into().unwrap()) as usize;
                rate = u32::from_le_bytes(w[body + 4..body + 8].try_into().unwrap());
            } else if id == b"data" {
                let end = (body + len).min(w.len());
                let mono: Vec<f32> = w[body..end].chunks_exact(2 * ch).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect();
                let n = (mono.len() as u64 * 16000 / rate as u64) as usize;
                return (0..n).map(|i| mono[(i as u64 * rate as u64 / 16000) as usize]).collect();
            }
            pos = body + len + (len & 1);
        }
        panic!("no data chunk");
    }

    #[tokio::test]
    #[ignore = "needs llama-server on :8080"]
    async fn live_chat() {
        let schema = serde_json::json!({"type": "object", "properties": {"say": {"type": "string"}}, "required": ["say"]});
        let v = HttpLlm::new("http://127.0.0.1:8080").chat(&[ChatMsg::new("user", "Reply with a short greeting in the say field.")], &schema, 400).await.unwrap();
        assert!(v.is_object(), "{v}");
    }

    #[tokio::test]
    #[ignore = "needs the voice service on :3000"]
    async fn live_tts_then_stt() {
        let wav = HttpTts::new("http://127.0.0.1:3000").speak("Bonjour, il fait beau aujourd'hui.", "fr", "bobby").await.unwrap();
        assert!(wav.0.starts_with(b"RIFF"));
        let t = HttpStt::new("http://127.0.0.1:3000").transcribe(&decode_wav(&wav.0), "").await.unwrap();
        println!("{t:?}");
        assert!(!t.text.is_empty());
    }
}
