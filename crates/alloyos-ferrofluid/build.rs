fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "none" {
        println!("cargo:rustc-link-arg-bin=ferrofluid=-T{}/process.ld", std::env::var("CARGO_MANIFEST_DIR").unwrap());
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
