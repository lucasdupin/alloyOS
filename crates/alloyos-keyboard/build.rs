fn main() {
    println!("cargo:rerun-if-env-changed=CARGO_MANIFEST_DIR");
    let ui_dir =
        std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("ui");
    // Dependents import @alloyos-keyboard without assuming our checkout location.
    println!("cargo:slint_library={}", ui_dir.display());
    println!("cargo:rerun-if-changed=ui");
    if std::env::var_os("CARGO_FEATURE_PREVIEW").is_none() {
        return;
    }
    slint_build::compile_with_config(
        "ui/preview.slint",
        slint_build::CompilerConfiguration::new()
            .with_style("fluent-dark".into())
            .with_scale_factor(alloyos_proto::display::PHONE_SCALE as f32)
            .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer),
    )
    .expect("compile keyboard preview");
}
