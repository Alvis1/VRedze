//! What differs between the systems the player runs on: where settings live,
//! and what OpenXR needs to start. On Linux (Steam Frame) and macOS this comes
//! from the environment; on Android (Meta Quest) `android_main` sets it first.

use std::path::PathBuf;
use std::sync::OnceLock;

pub struct Platform {
    /// Settings: servers, credentials, layouts, resume points.
    pub config_dir: PathBuf,
    #[cfg(target_os = "android")]
    pub android: Android,
}

/// The Java side of the app, for OpenXR and system calls.
#[cfg(target_os = "android")]
pub struct Android {
    /// The process's `JavaVM*`.
    pub vm: *mut std::ffi::c_void,
    /// A global reference to the `android.app.Activity` (NativeActivity).
    pub activity: *mut std::ffi::c_void,
    /// The shared storage root (`/sdcard`): Movies, Download, …
    pub external_storage: PathBuf,
}

// SAFETY: the JavaVM pointer and the activity's global reference are valid
// for the whole process and may be used from any thread attached to the VM.
#[cfg(target_os = "android")]
unsafe impl Send for Android {}
#[cfg(target_os = "android")]
unsafe impl Sync for Android {}

static PLATFORM: OnceLock<Platform> = OnceLock::new();

/// The platform: set by [`init`] at startup on Android, read from the
/// environment elsewhere.
pub fn get() -> &'static Platform {
    PLATFORM.get_or_init(from_environment)
}

/// Sets the platform once, before anything reads it (Android startup).
#[cfg(target_os = "android")]
pub fn init(platform: Platform) {
    if PLATFORM.set(platform).is_err() {
        eprintln!("Platform already set; keeping the first");
    }
}

#[cfg(not(target_os = "android"))]
fn from_environment() -> Platform {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    Platform {
        config_dir: crate::config::settings_dir(&base),
    }
}

#[cfg(target_os = "android")]
fn from_environment() -> Platform {
    panic!("platform::init must run before the platform is used on Android")
}

/// What OpenXR needs to create its loader and instance: the JavaVM and
/// Activity on Android, nothing elsewhere.
#[cfg(target_os = "android")]
pub fn xr_info() -> openxr::AndroidPlatformInfo {
    let android = &get().android;
    // SAFETY: both come from the running NativeActivity (see `Android`).
    unsafe { openxr::AndroidPlatformInfo::new(android.vm, android.activity) }
}

#[cfg(not(target_os = "android"))]
pub fn xr_info() {}
