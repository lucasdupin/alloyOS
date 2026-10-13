fn main() {
    println!("cargo:rerun-if-env-changed=CARGO_MANIFEST_DIR");
    // Bitmap fonts don't rescale at runtime: embed every physical-pixel size
    // traversed by the 12px/17px labels' 7.5% tap expansion.
    let scale = alloyos_proto::display::PHONE_SCALE as f32;
    let mut sizes = Vec::new();
    for base in [12.0_f32, 17.0] {
        for physical in (base * scale) as u32..=(base * 1.075 * scale).ceil() as u32 {
            sizes.push(((physical as f32 + 0.01) / scale).to_string());
        }
    }
    let sizes = sizes.join(",");
    println!("cargo:font_sizes={sizes}");
    let ui_dir =
        std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("ui");
    // Dependents import @alloyos-keyboard without assuming our checkout location.
    println!("cargo:slint_library={}", ui_dir.display());
    println!("cargo:rerun-if-changed=ui");
    if std::env::var_os("CARGO_FEATURE_PREVIEW").is_none() {
        return;
    }
    let existing = std::env::var("SLINT_FONT_SIZES").unwrap_or_default();
    std::env::set_var(
        "SLINT_FONT_SIZES",
        if existing.is_empty() {
            sizes
        } else {
            format!("{existing},{sizes}")
        },
    );
    slint_build::compile_with_config(
        "ui/preview.slint",
        slint_build::CompilerConfiguration::new()
            .with_style("fluent-dark".into())
            .with_scale_factor(alloyos_proto::display::PHONE_SCALE as f32)
            .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer),
    )
    .expect("compile keyboard preview");
}
