//! Keep the cached kernel splash identical to the first protected UI frame.
#[path = "../examples/support/boot_logo.rs"]
mod boot_logo;
#[test]
fn cached_boot_wordmark_matches_ui() {
    assert_eq!(
        boot_logo::render().as_slice(),
        include_bytes!("../../../assets/boot-wordmark.bin"),
        "regenerate with cargo run -p alloyos-ferrofluid --example generate_boot_logo"
    );
}
