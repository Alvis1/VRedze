/// Compiles the WGSL video shader to SPIR-V (pure Rust, works when cross-compiling).
fn compile_shader() {
    let source_path = "src/xr/video.wgsl";
    println!("cargo:rerun-if-changed={source_path}");
    let source = std::fs::read_to_string(source_path).unwrap();
    let module = naga::front::wgsl::parse_str(&source)
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::IMMEDIATES,
    )
    .validate(&module)
    .unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
    let mut options = naga::back::spv::Options {
        lang_version: (1, 0),
        ..Default::default()
    };
    // The shader is written for Vulkan clip space (y down); don't let naga
    // apply WebGPU's y flip.
    options
        .flags
        .remove(naga::back::spv::WriterFlags::ADJUST_COORDINATE_SPACE);
    let words = naga::back::spv::write_vec(&module, &info, &options, None).unwrap();
    let out = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("video.spv");
    std::fs::write(
        out,
        words
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect::<Vec<u8>>(),
    )
    .unwrap();
}

/// "abc1234+ 2026-09-26 17:40": commit (+ when there are local changes) and
/// UTC build time, shown in the app so installs can be told apart.
fn build_id() -> String {
    for path in ["src", "native", "build.rs", ".git/HEAD", ".git/index"] {
        println!("cargo:rerun-if-changed={path}");
    }
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let commit = git(&["rev-parse", "--short=7", "HEAD"]).unwrap_or_else(|| "dev".into());
    let dirty =
        git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    // Civil date from days since 1970 (Howard Hinnant's algorithm).
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{commit}{} {year}-{month:02}-{day:02} {:02}:{:02}",
        if dirty { "+" } else { "" },
        rem / 3600,
        rem / 60 % 60
    )
}

fn main() {
    println!("cargo:rustc-env=JUST_VIDEO_BUILD={}", build_id());
    compile_shader();
    for source in [
        "native/decode.c",
        "native/decode.h",
        "native/media.c",
        "native/media.h",
    ] {
        println!("cargo:rerun-if-changed={source}");
    }
    if std::env::var_os("CARGO_FEATURE_DECODE").is_none() {
        return;
    }
    let mut build = cc::Build::new();
    build
        .file("native/decode.c")
        .file("native/media.c")
        .flag_if_supported("-std=c11");
    println!("cargo:rerun-if-env-changed=JUST_VIDEO_STATIC_FFMPEG");
    // The Steam Frame build bundles its own FFmpeg + dav1d (scripts/build-frame-media.sh).
    let statik = std::env::var_os("JUST_VIDEO_STATIC_FFMPEG").is_some();
    for library in ["libavformat", "libavcodec", "libswresample", "libavutil"] {
        let found = pkg_config::Config::new()
            .statik(statik)
            .probe(library)
            .expect(
                "Install FFmpeg development headers or set PKG_CONFIG_PATH to a target sysroot",
            );
        for include in found.include_paths {
            build.include(include);
        }
    }
    build.compile("just_video_native");
}
