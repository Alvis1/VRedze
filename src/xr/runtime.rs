//! Connects to the OpenXR runtime. On Android (Meta Quest) through the
//! Khronos loader bundled in the APK. Elsewhere through the system loader if
//! there is one; SteamOS on Steam Frame ships no `libopenxr_loader.so.1`, so
//! then we follow the loader spec's runtime discovery and negotiate with the
//! runtime library directly.

use anyhow::Context;
#[cfg(not(target_os = "android"))]
use anyhow::{bail, ensure};
#[cfg(not(target_os = "android"))]
use openxr::sys;
#[cfg(not(target_os = "android"))]
use std::{
    env,
    path::{Path, PathBuf},
};

/// The bundled loader (`lib/arm64-v8a/libopenxr_loader.so`); loading it also
/// initializes it with the JavaVM and Activity (XR_KHR_loader_init_android).
#[cfg(target_os = "android")]
pub fn entry() -> anyhow::Result<openxr::Entry> {
    // SAFETY: the loader shipped in the APK is the Khronos OpenXR loader.
    unsafe { openxr::Entry::load(&crate::platform::xr_info()) }.context("Load the OpenXR loader")
}

/// Returns an entry for the system loader if present, else the active runtime.
#[cfg(not(target_os = "android"))]
pub fn entry() -> anyhow::Result<openxr::Entry> {
    // SAFETY: loading a library runs its initializers; the OpenXR loader and
    // runtimes are trusted system components.
    if let Ok(entry) = unsafe { openxr::Entry::load(&()) } {
        return Ok(entry);
    }
    let manifest = active_runtime_manifest()?;
    let library = runtime_library(&manifest)?;
    unsafe { negotiate(&library) }
        .with_context(|| format!("Load OpenXR runtime {}", library.display()))
}

#[cfg(not(target_os = "android"))]
/// Loader spec (Linux): XR_RUNTIME_JSON, then `active_runtime[.<arch>].json` in
/// $XDG_CONFIG_HOME, $XDG_CONFIG_DIRS and /etc.
fn active_runtime_manifest() -> anyhow::Result<PathBuf> {
    if let Some(path) = env::var_os("XR_RUNTIME_JSON") {
        return Ok(path.into());
    }
    let config_home = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|h| Path::new(&h).join(".config")));
    let config_dirs = env::var("XDG_CONFIG_DIRS").unwrap_or_else(|_| "/etc/xdg".into());
    let mut dirs: Vec<PathBuf> = config_home.into_iter().collect();
    dirs.extend(
        config_dirs
            .split(':')
            .filter(|d| !d.is_empty())
            .map(PathBuf::from),
    );
    dirs.push("/etc".into());
    let arch = env::consts::ARCH;
    for dir in dirs {
        for name in [
            format!("active_runtime.{arch}.json"),
            "active_runtime.json".into(),
        ] {
            let path = dir.join("openxr/1").join(name);
            if path.exists() {
                return Ok(path);
            }
        }
    }
    bail!("No active OpenXR runtime (is SteamVR installed and set as the OpenXR runtime?)")
}

#[cfg(not(target_os = "android"))]
fn runtime_library(manifest: &Path) -> anyhow::Result<PathBuf> {
    // Resolve symlinks first: relative library paths are relative to the real file.
    let manifest = manifest
        .canonicalize()
        .with_context(|| format!("Resolve {}", manifest.display()))?;
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&manifest)?)
        .with_context(|| format!("Parse {}", manifest.display()))?;
    let library = json["runtime"]["library_path"]
        .as_str()
        .context("Runtime manifest lacks runtime.library_path")?;
    let library = Path::new(library);
    Ok(
        if library.is_absolute() || !library.to_string_lossy().contains('/') {
            library.to_path_buf()
        } else {
            manifest.parent().unwrap_or(Path::new("/")).join(library)
        },
    )
}

#[cfg(not(target_os = "android"))]
unsafe fn negotiate(path: &Path) -> anyhow::Result<openxr::Entry> {
    // The runtime must stay loaded for the life of the process.
    let library: &'static libloading::Library =
        Box::leak(Box::new(unsafe { libloading::Library::new(path) }?));
    let negotiate: libloading::Symbol<sys::pfn::NegotiateLoaderRuntimeInterface> =
        unsafe { library.get(b"xrNegotiateLoaderRuntimeInterface\0") }?;
    let info = sys::NegotiateLoaderInfo {
        struct_type: sys::NegotiateLoaderInfo::TYPE,
        struct_version: sys::NegotiateLoaderInfo::VERSION,
        struct_size: size_of::<sys::NegotiateLoaderInfo>(),
        min_interface_version: 1,
        max_interface_version: sys::CURRENT_LOADER_RUNTIME_VERSION as u32,
        min_api_version: sys::Version::new(1, 0, 0),
        max_api_version: sys::Version::new(1, 0x3ff, 0xfff),
    };
    let mut request = sys::NegotiateRuntimeRequest {
        struct_type: sys::NegotiateRuntimeRequest::TYPE,
        struct_version: sys::NegotiateRuntimeRequest::VERSION,
        struct_size: size_of::<sys::NegotiateRuntimeRequest>(),
        runtime_interface_version: 0,
        runtime_api_version: sys::Version::new(0, 0, 0),
        get_instance_proc_addr: None,
    };
    let result = unsafe { negotiate(&info, &mut request) };
    ensure!(
        result == sys::Result::SUCCESS,
        "Runtime negotiation failed: {result:?}"
    );
    let gipa = request
        .get_instance_proc_addr
        .context("Runtime returned no xrGetInstanceProcAddr")?;
    Ok(unsafe { openxr::Entry::from_get_instance_proc_addr(gipa, &()) }?)
}
