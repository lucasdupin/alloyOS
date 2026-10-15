//! Live capture launcher. Arguments contain paths only, never credential values.
//! Usage: readme_qemu_capture KERNEL COPIED_DISK QMP_SOCKET [VOICE_HELPER]
use std::{path::PathBuf, process::Command};
fn main() {
    let args: Vec<_> = std::env::args().collect();
    assert!(args.len() >= 4, "expected kernel, copied disk, QMP socket, optional voice helper");
    if let Some(helper) = args.get(4) {
        std::env::set_var("ALLOYOS_VOICE_HELPER", helper);
    }
    let mut qemu = Command::new("qemu-system-aarch64");
    qemu.args([
        "-M", "virt", "-accel", "hvf", "-cpu", "host", "-m", "256M",
        "-display", "none", "-monitor", "none", "-kernel", &args[1],
        "-drive", &format!("file={},if=none,format=raw,id=hd0", args[2]),
        "-device", "virtio-blk-device,drive=hd0",
        "-device", "virtio-gpu-device,xres=1206,yres=2622",
        "-device", "virtio-keyboard-device", "-device", "virtio-tablet-device",
        "-serial", "stdio", "-qmp", &format!("unix:{},server=on,wait=off", args[3]),
        "-snapshot", "-no-reboot",
    ]);
    let mut env_names = Vec::new();
    let mut auth_files = Vec::new();
    for name in ["GEMINI_API_KEY", "OPENAI_API_KEY"] {
        if std::env::var(name).is_ok_and(|value| !value.trim().is_empty()) {
            env_names.push(name.into());
        } else {
            let path = PathBuf::from(".secrets").join(name);
            if path.is_file() { auth_files.push(path); }
        }
    }
    assert!(!env_names.is_empty() || !auth_files.is_empty(), "no real provider credentials available");
    let _auth = alloyos_dev_harness::credentials::attach(&mut qemu, &env_names, &auth_files)
        .expect("private provider credential unavailable");
    alloyos_dev_harness::https::run_ui(&mut qemu).expect("live QEMU bridge failed");
}
