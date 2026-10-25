fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "none" {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let repo_root = std::path::Path::new(&manifest_dir).parent().unwrap().parent().unwrap();
        let sysroot = repo_root.join("build/sysroot");

        let llvm_ar = std::env::var("LLVM_AR").unwrap_or_else(|_| {
            let rustc_out = std::process::Command::new("rustc")
                .arg("--print")
                .arg("sysroot")
                .output()
                .ok();
            if let Some(out) = rustc_out {
                let rust_sysroot = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let candidate = std::path::Path::new(&rust_sysroot)
                    .join("lib/rustlib/aarch64-apple-darwin/bin/llvm-ar");
                if candidate.is_file() {
                    return candidate.to_string_lossy().to_string();
                }
            }
            "llvm-ar".to_string()
        });

        println!("cargo:rerun-if-changed=src/webview/wpe_bridge.cpp");
        println!("cargo:rerun-if-changed=src/webview/wpe_bridge.h");

        cc::Build::new()
            .compiler("clang++")
            .archiver(&llvm_ar)
            .cpp(true)
            .file("src/webview/wpe_bridge.cpp")
            .flag("-std=c++20")
            .flag("-ffreestanding")
            .flag("-fno-exceptions")
            .flag("-fno-rtti")
            .flag("-Wno-unused-parameter")
            .flag(&format!("--sysroot={}", sysroot.display()))
            .include(sysroot.join("include/c++/v1"))
            .include(sysroot.join("include"))
            .include(sysroot.join("include/glib-2.0"))
            .include(sysroot.join("lib/glib-2.0/include"))
            .include(sysroot.join("include/libsoup-3.0"))
            .include(sysroot.join("include/wpe-2.0"))
            .include(sysroot.join("include/wpe-platform-2.0"))
            .include(sysroot.join("include/WebKit"))
            .define("BUILDING_WPE__", "1")
            .define("ENABLE_WPE_PLATFORM", "1")
            .define("__alloyos__", "1")
            .define("_POSIX_C_SOURCE", "200809L")
            .compile("wpe_bridge");

        println!("cargo:rustc-link-search=native={}", sysroot.join("lib").display());
        println!("cargo:rustc-link-arg-bin=ferrofluid=-T{}/process.ld", manifest_dir);

        let libs = [
            "WPEWebKit-2.0", "WebCore", "JavaScriptCore", "WTF", "bmalloc",
            "Skia", "PAL", "SysProfCapture", "wpe-1.0", "epoxy", "xkbcommon",
            "fontconfig", "freetype", "harfbuzz", "harfbuzz-icu", "xml2", "sqlite3",
            "webp", "webpdemux", "webpmux", "psl", "expat", "tasn1",
            "soup-3.0", "gio-2.0", "gobject-2.0", "gmodule-2.0", "glib-2.0",
            "nghttp2", "pcre2-8", "sharpyuv", "gcrypt", "gpg-error", "ffi", "intl",
            "png", "jpeg", "z", "icui18n", "icuuc", "icudata",
            "alloyos_cxxrt", "alloyos_posix", "c++", "c++abi", "c", "m", "compiler_rt",
        ];
        for lib in libs {
            println!("cargo:rustc-link-lib=static={}", lib);
        }
    } else if target_os == "macos" {
        println!("cargo:rerun-if-changed=src/webview/webkit_bridge.m");
        println!("cargo:rerun-if-changed=src/webview/webkit_bridge.h");
        cc::Build::new()
            .file("src/webview/webkit_bridge.m")
            .flag("-fobjc-arc")
            .flag("-Wno-unused-parameter")
            .compile("webkit_bridge");

        println!("cargo:rustc-link-lib=framework=WebKit");
        println!("cargo:rustc-link-lib=framework=Cocoa");
    }

    slint_build::compile_with_config(
        "ui/system-ui.slint",
        slint_build::CompilerConfiguration::new()
            .with_library_paths(
                [(
                    "alloyos-keyboard".into(),
                    std::env::var("DEP_ALLOYOS_KEYBOARD_SLINT_LIBRARY")
                        .expect("keyboard UI library")
                        .into(),
                )]
                .into(),
            )
            .with_style("fluent-dark".into())
            .with_scale_factor(alloyos_proto::display::PHONE_SCALE as f32)
            .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer),
    )
    .expect("compile Ferrofluid System UI");
}
