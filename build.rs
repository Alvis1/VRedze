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

fn main() {
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
