fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("none") {
        println!("cargo:rustc-link-arg-bin=ferrofluid=-T{}/process.ld", std::env::var("CARGO_MANIFEST_DIR").unwrap());
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
