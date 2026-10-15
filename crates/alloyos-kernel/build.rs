use std::{env, path::PathBuf};

fn main() {
    // The UI verifies this cached first frame in its own test. Kernel/xtask
    // builds must not compile a second copy of Slint and its font atlases.
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let asset = manifest.join("../../assets/boot-wordmark.bin");
    println!("cargo:rerun-if-changed={}", asset.display());
    let bytes = std::fs::read(asset).expect("read cached boot wordmark");
    assert!(bytes.len() >= 16, "boot wordmark header");
    let value = |offset| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
    let (x, y, width, height) = (value(0), value(4), value(8), value(12));
    assert_eq!(bytes.len(), 16 + width * height, "boot wordmark extent");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    std::fs::write(out.join("boot-logo.gray"), &bytes[16..]).unwrap();
    std::fs::write(out.join("boot_logo.rs"), format!(
        "const BOOT_LOGO_WIDTH: usize = {width};\nconst BOOT_LOGO_HEIGHT: usize = {height};\nconst BOOT_LOGO_X: i64 = {x};\nconst BOOT_LOGO_Y: i64 = {y};\nconst BOOT_LOGO: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/boot-logo.gray\"));\n"
    )).unwrap();
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("none")
        && env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("aarch64")
    {
        let script = manifest.join("linker-aarch64.ld");
        println!("cargo:rustc-link-arg=-T{}", script.display());
        println!("cargo:rerun-if-changed={}", script.display());
    }
}
