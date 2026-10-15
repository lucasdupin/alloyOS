use std::process::Command;
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let live = args.get(5).is_some_and(|arg| arg == "--live-device");
    if !live {
        std::env::set_var("GEMINI_API_KEY", "fixture-not-a-real-key");
    }
    std::env::set_var("ALLOYOS_VOICE_HELPER", &args[4]);
    let mut qemu = Command::new("qemu-system-aarch64");
    qemu.args([
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
        "-kernel",
        &args[1],
        "-drive",
        &format!("file={},if=none,format=raw,id=hd0", args[2]),
        "-device",
        "virtio-blk-device,drive=hd0",
        "-device",
        "virtio-gpu-device,xres=1206,yres=2622",
        "-device",
        "virtio-keyboard-device",
        "-device",
        "virtio-tablet-device",
        "-serial",
        "stdio",
        "-qmp",
        &format!("unix:{},server=on,wait=off", args[3]),
        "-no-reboot",
    ]);
    let auth_files =
        if live && !std::env::var("GEMINI_API_KEY").is_ok_and(|value| !value.trim().is_empty()) {
            vec![std::path::PathBuf::from(".secrets/GEMINI_API_KEY")]
        } else {
            vec![]
        };
    let env_names = if auth_files.is_empty() {
        vec!["GEMINI_API_KEY".into()]
    } else {
        vec![]
    };
    let _auth = alloyos_dev_harness::credentials::attach(&mut qemu, &env_names, &auth_files)
        .expect("private Gemini credential unavailable");
    alloyos_dev_harness::https::run_ui(&mut qemu).unwrap();
}
