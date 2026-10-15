use std::env;
use std::path::PathBuf;

fn main() {
    // Use the actual Slint software renderer, fonts, layout and initial liquid
    // state. A separately rasterized SVG cannot guarantee the same pixels.
    use alloyos_ferrofluid::system_ui::FerrofluidSystemUi;
    use alloyos_proto::display::{PHONE_WIDTH, PHONE_HEIGHT, PHONE_SCALE};
    let ui_sources = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("../alloyos-ferrofluid");
    println!("cargo:rerun-if-changed={}", ui_sources.join("ui").display());
    println!("cargo:rerun-if-changed={}", ui_sources.join("src").display());
    let mut ui = FerrofluidSystemUi::new(PHONE_WIDTH, PHONE_HEIGHT, PHONE_SCALE as f32)
        .expect("boot frame renderer");
    ui.set_booting(true);
    ui.render(std::time::Duration::ZERO).expect("initial boot frame");
    let frame = ui.pixels();
    let lit: Vec<_> = frame.iter().enumerate()
        .filter(|(_, pixel)| **pixel != 0xff000000).map(|(i, _)| i).collect();
    assert!(!lit.is_empty(), "boot logo must be visible");
    let x = lit.iter().map(|i| *i % PHONE_WIDTH as usize).min().unwrap();
    let y = lit.iter().map(|i| *i / PHONE_WIDTH as usize).min().unwrap();
    let width = lit.iter().map(|i| *i % PHONE_WIDTH as usize).max().unwrap() - x + 1;
    let height = lit.iter().map(|i| *i / PHONE_WIDTH as usize).max().unwrap() - y + 1;
    let mut pixels = Vec::with_capacity(width * height);
    for row in y..y + height {
        for col in x..x + width {
            let pixel = frame[row * PHONE_WIDTH as usize + col];
            let gray = pixel as u8;
            assert_eq!(pixel, 0xff000000 | gray as u32 * 0x010101);
            pixels.push(gray);
        }
    }
    // Verify the kernel's cropped blit reproduces every pixel of the UI frame.
    for row in 0..PHONE_HEIGHT as usize {
        for col in 0..PHONE_WIDTH as usize {
            let gray = if (y..y + height).contains(&row) && (x..x + width).contains(&col) {
                pixels[(row - y) * width + col - x]
            } else { 0 };
            assert_eq!(frame[row * PHONE_WIDTH as usize + col],
                0xff000000 | gray as u32 * 0x010101, "boot handoff pixel mismatch");
        }
    }
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    std::fs::write(out.join("boot-logo.gray"), pixels).expect("write boot logo bitmap");
    std::fs::write(out.join("boot_logo.rs"), format!(
        "const BOOT_LOGO_WIDTH: usize = {width};\nconst BOOT_LOGO_HEIGHT: usize = {height};\nconst BOOT_LOGO_X: i64 = {x};\nconst BOOT_LOGO_Y: i64 = {y};\nconst BOOT_LOGO: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/boot-logo.gray\"));\n"
    )).expect("write boot logo dimensions");
    println!("cargo:rerun-if-env-changed=CARGO_MANIFEST_DIR");
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();

    if target_os == "none" && target_arch == "aarch64" {
        let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
        let linker_script = manifest_dir.join("linker-aarch64.ld");
        println!("cargo:rustc-link-arg=-T{}", linker_script.display());
        println!("cargo:rerun-if-changed={}", linker_script.display());
    }
}
