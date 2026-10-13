//! Opt-in real Gemini generation through the booted guest and production relay.
use alloyos_dev_harness::{credentials, Guest};
use std::{path::PathBuf, process::Command, time::Duration};

#[test]
#[ignore = "requires QEMU HVF, a built kernel, and a configured Gemini key; incurs API usage"]
fn gemini_live_guest_roundtrip() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let kernel = root.join("target/aarch64-unknown-none/debug/alloyos-kernel");
    assert!(kernel.is_file(), "build the kernel first");
    let mut command = Command::new("qemu-system-aarch64");
    command.args([
        "-M",
        "virt",
        "-accel",
        "hvf",
        "-cpu",
        "host",
        "-m",
        "256M",
        "-display",
        "none",
        "-monitor",
        "none",
        "-device",
        "virtio-gpu-device,xres=1206,yres=2622",
        "-serial",
        "stdio",
        "-no-reboot",
        "-kernel",
    ]);
    command.arg(kernel);
    let key_file = root.join(".secrets/GEMINI_API_KEY");
    let auth = if std::env::var_os("GEMINI_API_KEY").is_some() {
        credentials::attach(&mut command, &["GEMINI_API_KEY".into()], &[])
    } else {
        credentials::attach(&mut command, &[], &[key_file])
    }
    .expect("Gemini credentials unavailable");
    let mut guest = Guest::launch(&mut command, Duration::from_secs(65)).unwrap();
    guest.wait_ui_boot().unwrap();
    let status = guest.wait_ready().unwrap();
    assert!(status.contains("gemini-3.8-flash"));
    let prompt = alloyos_forge::protocol::encode_hex("Reply with exactly ALLOY_OK");
    let reply = guest
        .request(&format!("generate\tgemini-3.8-flash\t128\t{prompt}"))
        .expect("live Gemini generation failed");
    assert!(reply.contains("ALLOY_OK"), "unexpected reply");
    drop(guest);
    drop(auth);
}
