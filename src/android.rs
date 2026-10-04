//! The Meta Quest (Android) entry point. NativeActivity loads this library
//! and android-activity runs `android_main` on a thread of its own.

use crate::platform::{Android, Platform};
use android_activity::{AndroidApp, MainEvent, PollEvent};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    log_to_logcat();
    std::panic::set_hook(Box::new(|info| eprintln!("Panic: {info}")));
    let data = app
        .internal_data_path()
        .unwrap_or_else(|| "/data/local/tmp".into());
    let external_storage = std::env::var_os("EXTERNAL_STORAGE")
        .map(Into::into)
        .unwrap_or_else(|| "/sdcard".into());
    crate::platform::init(Platform {
        config_dir: data.join("just-video"),
        android: Android {
            vm: app.vm_as_ptr(),
            activity: app.activity_as_ptr(),
            external_storage,
        },
    });
    // SAFETY: the VM pointer is the process's JavaVM.
    unsafe { crate::media::android_init(app.vm_as_ptr()) };
    eprintln!(
        "Just Video {} ({}) starting on Android",
        env!("CARGO_PKG_VERSION"),
        crate::ui::browser::BUILD
    );
    // Let the activity finish starting before the (slower) XR setup.
    app.poll_events(Some(Duration::ZERO), |_| {});

    let quit = Arc::new(AtomicBool::new(false));
    let destroyed = quit.clone();
    let lifecycle = app.clone();
    // Android blocks its main thread until lifecycle events are taken, so the
    // frame loop takes them every frame.
    let pump = Box::new(move || {
        lifecycle.poll_events(Some(Duration::ZERO), |event| {
            if let PollEvent::Main(MainEvent::Destroy) = event {
                destroyed.store(true, Ordering::Relaxed);
            }
        });
    });
    let library = crate::library::Library::start(crate::media::default_hw_backend());
    let navigator = crate::ui::navigator::Navigator::new(library);
    let result = crate::xr::app::run(
        Some(navigator),
        None,
        crate::xr::app::AppOptions {
            view: Default::default(),
            play: Default::default(),
            quit,
            pump: Some(pump),
        },
    );
    match result {
        Ok(_) => eprintln!("Just Video stopped"),
        Err(e) => eprintln!("Just Video stopped with an error: {e:#}"),
    }
}

/// Sends stdout and stderr (our eprintln!s and FFmpeg's messages) to logcat,
/// one line at a time, tagged "JustVideo".
fn log_to_logcat() {
    unsafe extern "C" {
        fn __android_log_write(
            priority: i32,
            tag: *const std::ffi::c_char,
            text: *const std::ffi::c_char,
        ) -> i32;
    }
    const INFO: i32 = 4;
    let mut fds = [0; 2];
    // SAFETY: plain pipe/dup2 on descriptors we own.
    unsafe {
        if libc::pipe(fds.as_mut_ptr()) != 0 {
            return;
        }
        libc::dup2(fds[1], 1);
        libc::dup2(fds[1], 2);
    }
    let read_end = fds[0];
    let _ = std::thread::Builder::new()
        .name("logcat".into())
        .spawn(move || {
            use std::io::BufRead;
            use std::os::fd::FromRawFd;
            // SAFETY: the read end of our own pipe, owned by this thread from now on.
            let pipe = unsafe { std::fs::File::from_raw_fd(read_end) };
            let tag = c"JustVideo";
            for line in std::io::BufReader::new(pipe).lines().map_while(Result::ok) {
                let text = std::ffi::CString::new(line.replace('\0', "")).unwrap_or_default();
                // SAFETY: both strings are NUL-terminated.
                unsafe { __android_log_write(INFO, tag.as_ptr(), text.as_ptr()) };
            }
        });
}
