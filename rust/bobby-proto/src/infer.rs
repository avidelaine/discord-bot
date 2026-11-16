//! The three model interfaces the brain knows: transcribe, understand, speak.
//! Models are called by address (HTTP today); an in-process engine can implement the same traits later.
use serde::{Deserialize, Serialize};
use std::fmt;
use std::future::Future;

/// Speech-to-text result. `ms`: time the engine took.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    pub text: String,
    pub lang: String,
    pub ms: u32,
}

/// A WAV file (RIFF bytes).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Wav(pub Vec<u8>);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMsg {
    pub role: String,
    pub content: String,
}

impl ChatMsg {
    pub fn new(role: &str, content: &str) -> Self {
        Self { role: role.into(), content: content.into() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InferErrorKind {
    /// The service cannot be reached (refused, not started).
    Down,
    /// No answer in time (GPU saturated).
    Timeout,
    /// Reached, but the answer is unusable (HTTP error, invalid JSON, bad audio).
    Bad,
}

#[derive(Debug, Clone)]
pub struct InferError {
    pub kind: InferErrorKind,
    pub msg: String,
}

impl InferError {
    pub fn new(kind: InferErrorKind, msg: impl Into<String>) -> Self {
        Self { kind, msg: msg.into() }
    }
}

impl fmt::Display for InferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.msg)
    }
}

impl std::error::Error for InferError {}

pub trait Transcribe {
    /// `pcm16k`: mono samples at 16 kHz, f32 in [-1, 1]. `hotwords`: space-separated words to favour ("" for none).
    /// Implementations convert to whatever their engine takes.
    fn transcribe(&self, pcm16k: &[f32], hotwords: &str) -> impl Future<Output = Result<Transcript, InferError>> + Send;
}

pub trait Understand {
    /// Chat completion constrained by a JSON schema; returns the parsed JSON object the model produced.
    fn chat(&self, messages: &[ChatMsg], schema: &serde_json::Value, max_tokens: u32) -> impl Future<Output = Result<serde_json::Value, InferError>> + Send;
}

pub trait Speak {
    /// Returns a WAV file. `voice`: "bobby" or "claude".
    fn speak(&self, text: &str, lang: &str, voice: &str) -> impl Future<Output = Result<Wav, InferError>> + Send;
}
