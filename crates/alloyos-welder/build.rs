fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("none") {
        println!(
            "cargo:rustc-link-arg-bin=welder=-T{}/process.ld",
            std::env::var("CARGO_MANIFEST_DIR").unwrap()
        );
    }
    println!("cargo:rerun-if-changed=process.ld");
}
