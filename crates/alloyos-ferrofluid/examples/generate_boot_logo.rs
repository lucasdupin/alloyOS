//! Run only when changing the boot wordmark: cargo run -p alloyos-ferrofluid --example generate_boot_logo
#[path = "support/boot_logo.rs"]
mod boot_logo;
fn main() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/boot-wordmark.bin");
    std::fs::write(path, boot_logo::render()).expect("save boot wordmark");
}
