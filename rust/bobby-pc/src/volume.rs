//! Master volume of the default render endpoint (Core Audio).
use bobby_proto::msg::VolumeOp;
use serde_json::{json, Value};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};

fn endpoint() -> Result<IAudioEndpointVolume, String> {
    // S_FALSE / RPC_E_CHANGED_MODE just mean COM is already initialised on this thread.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let e = |x: windows::core::Error| x.to_string();
    unsafe {
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(e)?;
        let dev = en.GetDefaultAudioEndpoint(eRender, eConsole).map_err(e)?;
        dev.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None).map_err(e)
    }
}

fn level(v: &IAudioEndpointVolume) -> Result<i32, String> {
    Ok((unsafe { v.GetMasterVolumeLevelScalar() }.map_err(|e| e.to_string())? * 100.0).round() as i32)
}

fn read(v: &IAudioEndpointVolume) -> Result<Value, String> {
    let muted = unsafe { v.GetMute() }.map_err(|e| e.to_string())?.as_bool();
    Ok(json!({"level": level(v)?, "muted": muted}))
}

pub fn state() -> Result<Value, String> {
    read(&endpoint()?)
}

pub fn apply(op: VolumeOp) -> Result<Value, String> {
    let v = endpoint()?;
    let set = |l: i32| unsafe { v.SetMasterVolumeLevelScalar(l.clamp(0, 100) as f32 / 100.0, std::ptr::null()) };
    let mute = |m: bool| unsafe { v.SetMute(m, std::ptr::null()) };
    match op {
        VolumeOp::Get => {}
        VolumeOp::Set { level } => set(level as i32).map_err(|e| e.to_string())?,
        VolumeOp::Delta { delta } => set(level(&v)? + delta as i32).map_err(|e| e.to_string())?,
        VolumeOp::Mute => mute(true).map_err(|e| e.to_string())?,
        VolumeOp::Unmute => mute(false).map_err(|e| e.to_string())?,
    }
    read(&v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_has_keys_and_range() {
        let s = state().unwrap();
        let l = s["level"].as_i64().unwrap();
        assert!((0..=100).contains(&l));
        assert!(s["muted"].is_boolean());
    }

    #[test]
    fn set_to_current_level_is_stable() {
        let cur = state().unwrap()["level"].as_u64().unwrap() as u8;
        let s = apply(VolumeOp::Set { level: cur }).unwrap();
        assert_eq!(s["level"].as_u64().unwrap(), cur as u64);
        assert!(s["muted"].is_boolean());
    }
}
