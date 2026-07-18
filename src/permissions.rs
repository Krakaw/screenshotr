//! Screen Recording (TCC) permission handling.

use crate::sys::{CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess};

/// Whether this process holds the Screen Recording grant.
///
/// The result is cached by the OS for the lifetime of the process: once this
/// returns false it keeps returning false even after the user clicks Allow.
/// Only a fresh process observes a newly granted permission.
pub fn has_access() -> bool {
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// Gate startup on the Screen Recording grant, exiting if it is absent.
///
/// Exiting rather than waiting is deliberate: `has_access` is cached per
/// process, so a granted permission is only visible after a restart. launchd
/// (KeepAlive + ThrottleInterval) supplies that restart.
pub fn ensure_access_or_exit() -> ! {
    // If preflight disagrees with the capability probe, the grant is stale:
    // macOS still has a decision on record (so it won't re-prompt) but
    // ScreenCaptureKit returns nothing. Toggling the app off then on is the
    // only fix; a plain restart won't clear it.
    if has_access() {
        log::error!(
            "Screen Recording appears granted but ScreenCaptureKit returns no \
             displays — the grant is stale (usually after the app was updated). \
             Toggle ScreenshotR OFF then ON in System Settings > Privacy & \
             Security > Screen Recording."
        );
    } else {
        log::warn!("Screen Recording permission not granted");
    }

    // Only prompts if TCC has no recorded decision for this app's designated
    // requirement. After a prior denial this returns false silently, so the
    // System Settings pane below is the only recovery path.
    unsafe { CGRequestScreenCaptureAccess() };

    // launchd restarts us every ThrottleInterval until the grant lands, so this
    // runs repeatedly. Opening System Settings each time would reopen the window
    // under the user every few seconds; do it only on the first attempt.
    if !prompted_before() {
        open_settings();
        mark_prompted();
    }

    log::error!(
        "Grant Screen Recording to ScreenshotR in System Settings > Privacy & Security. \
         Exiting so launchd restarts this process with a fresh permission check."
    );
    std::process::exit(1);
}

fn marker_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    std::path::PathBuf::from(home).join(".config/screenshotr/.permission-prompted")
}

fn prompted_before() -> bool {
    marker_path().exists()
}

fn mark_prompted() {
    let path = marker_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, b"");
}

/// Clear the marker so a future permission loss re-opens System Settings once.
pub fn clear_prompt_marker() {
    let _ = std::fs::remove_file(marker_path());
}

fn open_settings() {
    let _ = std::process::Command::new("/usr/bin/open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
        .status();
}
