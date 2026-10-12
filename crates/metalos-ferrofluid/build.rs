fn main() {
    slint_build::compile_with_config(
        "ui/system-ui.slint",
        slint_build::CompilerConfiguration::new()
            .with_style("fluent-dark".into())
            .with_scale_factor(metalos_proto::display::PHONE_SCALE as f32)
            .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer),
    )
    .expect("compile Ferrofluid System UI");
}
