//! ONNX Runtime is loaded from a DLL at run time (ort `load-dynamic`): `ORT_DYLIB_PATH`, else `<models>/onnxruntime.dll`.
//! Never the bare name: Windows ships an old onnxruntime.dll in System32 that the DLL search would find.
use ort::environment::Environment;
use std::path::{Path, PathBuf};

/// `model_dir`: a folder inside `models/` (e.g. `models/wakeword`); the DLL sits in its parent.
pub fn dll_path(model_dir: &Path) -> PathBuf {
    match std::env::var("ORT_DYLIB_PATH") {
        Ok(p) if !p.is_empty() => p.into(),
        _ => model_dir.parent().unwrap_or(model_dir).join(if cfg!(windows) { "onnxruntime.dll" } else { "libonnxruntime.so" }),
    }
}

/// Loads the runtime once (later calls reuse it) and builds an environment for sessions.
pub fn env(model_dir: &Path) -> Result<Environment, String> {
    let p = dll_path(model_dir);
    ort::init_from(&p).map_err(|e| format!("ONNX Runtime not loaded from {}: {e}", p.display()))?.build().map_err(|e| e.to_string())
}
