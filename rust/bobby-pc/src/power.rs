//! Lock, sleep, shutdown. Side effects are recorded instead of performed in dry mode.
use crate::apps::{dry, record, sh};

pub fn lock() {
    if dry() {
        return record(&["LockWorkStation"]);
    }
    unsafe {
        let _ = windows::Win32::System::Shutdown::LockWorkStation();
    }
}

/// Real sleep (not hibernate), like the Node version.
pub fn sleep() {
    if dry() {
        return record(&["SetSuspendState", "false", "false", "false"]);
    }
    unsafe {
        let _ = windows::Win32::System::Power::SetSuspendState(false, false, false);
    }
}

pub fn shutdown() {
    sh("shutdown.exe", &["/s", "/t", "5"]);
}
