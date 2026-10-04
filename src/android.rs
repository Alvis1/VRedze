//! The Meta Quest (Android) entry point. NativeActivity loads this library
//! and android-activity runs `android_main` on a thread of its own.

use crate::platform::{Android, Platform};
use android_activity::{AndroidApp, MainEvent, PollEvent};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Set by the first `android_main` in this process.
static STARTED: AtomicBool = AtomicBool::new(false);

#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    log_to_logcat();
    std::panic::set_hook(Box::new(|info| eprintln!("Panic: {info}")));
    if STARTED.swap(true, Ordering::SeqCst) {
        // A new activity in a process whose player hasn't ended: its OpenXR
        // loader, FFmpeg JNI state and `platform` belong to the old activity
        // (starting again aborts in the loader). Android starts a fresh
        // process for the activity instead.
        eprintln!("A player is still running in this process; restarting it");
        exit_soon();
    }
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
            if let PollEvent::Main(MainEvent::Destroy) = event
                && !destroyed.swap(true, Ordering::Relaxed)
            {
                // The activity is gone; the frame loop should end within a
                // few frames. If the runtime blocks it (seen in xrEndFrame
                // when the activity is destroyed during start-up), end the
                // process anyway, or the next launch lands in it.
                let _ = std::thread::Builder::new()
                    .name("exit-deadline".into())
                    .spawn(|| {
                        std::thread::sleep(Duration::from_secs(4));
                        eprintln!(
                            "The player didn't stop after the activity was destroyed; exiting"
                        );
                        exit_soon();
                    });
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
    // Android keeps the process after the activity finishes and runs
    // android_main again in it on the next launch, where the OpenXR loader,
    // FFmpeg's JNI state and `platform` still hold the first, finished
    // activity: the app then never opens again. End the process instead, so
    // every launch starts fresh (settings and resume points are written).
    exit_soon();
}

/// Ends the process after giving the logcat thread a moment to pass on the
/// last lines.
fn exit_soon() -> ! {
    std::thread::sleep(Duration::from_millis(200));
    // SAFETY: _exit ends the process without running atexit handlers, which
    // could race the worker threads still running.
    unsafe { libc::_exit(0) }
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
