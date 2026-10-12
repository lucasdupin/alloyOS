//! Real large-file regression. Explicitly fails if the asset or QEMU is missing.
use metalos_dev_harness::model_loader::attach_disk;
use serde_json::{json, Value};
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

fn unique_id() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

struct Guest {
    child: Child,
    input: ChildStdin,
    lines: Receiver<String>,
}
impl Drop for Guest {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Guest {
    fn receive(&self) -> Value {
        let line = self
            .lines
            .recv_timeout(Duration::from_secs(180))
            .expect("QEMU timed out or exited while loading the full model");
        serde_json::from_str(&line).expect("invalid QMP reply")
    }
    fn request(&mut self, command: Value) -> Value {
        writeln!(self.input, "{command}").unwrap();
        self.input.flush().unwrap();
        loop {
            let reply = self.receive();
            if reply.get("event").is_some() {
                continue;
            }
            assert!(reply.get("error").is_none(), "QMP command failed: {reply}");
            return reply;
        }
    }
}

#[test]
#[ignore = "loads the real model: needs QEMU, 16 GiB guest RAM"]
fn full_model_loads_into_qemu_at_contiguous_addresses() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let image = std::env::var_os("METALOS_TEST_MODEL_IMAGE")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join(".local/models/gemma-2-2b-it/model.img"));
    verify_image(image, true);
}

fn verify_image(image: PathBuf, expect_ready: bool) {
    let mut source =
        File::open(&image).expect("prepare the local model image before running this test");
    let length = source.metadata().unwrap().len();
    assert!(
        length > 2 * 1024 * 1024 * 1024,
        "fixture must exercise the large-file regression"
    );
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let kernel = std::env::var_os("METALOS_TEST_KERNEL")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target/aarch64-unknown-none/debug/metalos-kernel"));
    assert!(
        kernel.exists(),
        "build the kernel with --features forge before running this test"
    );
    let serial = SyntheticImage(
        std::env::temp_dir().join(format!("metalos-model-serial-{}.log", unique_id())),
    );
    let mut command = Command::new("qemu-system-aarch64");
    command.args([
        "-M", "virt", "-m", "16G", "-display", "none", "-monitor", "none", "-qmp", "stdio",
    ]);
    #[cfg(target_os = "macos")]
    command.args(["-accel", "hvf", "-cpu", "host"]);
    #[cfg(not(target_os = "macos"))]
    command.args(["-accel", "tcg", "-cpu", "cortex-a57"]);
    attach_disk(&image, &mut command).unwrap();
    command
        .arg("-kernel")
        .arg(kernel)
        .arg("-serial")
        .arg(format!("file:{}", serial.0.display()))
        .args(["-device", "virtio-gpu-device,xres=1206,yres=2622"]);
    let fs_source = root.join("target/aarch64-unknown-none/debug/cobaltfs.img");
    let fs_image = SyntheticImage(
        std::env::temp_dir().join(format!("metalos-model-root-{}.img", unique_id())),
    );
    let has_root = fs_source.exists();
    if has_root {
        fs::copy(fs_source, &fs_image.0).unwrap();
        command
            .arg("-drive")
            .arg(format!(
                "file={},if=none,format=raw,id=root",
                fs_image.0.display()
            ))
            .args(["-device", "virtio-blk-device,drive=root"]);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    eprintln!("Starting QEMU and waiting for machine initialization");
    let launch_time = Instant::now();
    let mut child = command.spawn().expect("QEMU must be installed");
    let input = child.stdin.take().unwrap();
    let output = child.stdout.take().unwrap();
    let (tx, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            let Ok(line) = line else {
                break;
            };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut guest = Guest {
        child,
        input,
        lines,
    };
    assert!(guest.receive().get("QMP").is_some());
    guest.request(json!({"execute": "qmp_capabilities"}));
    // Unlike a greeting, this response requires completed machine initialization.
    let status = guest.request(json!({"execute": "query-status"}));
    assert_eq!(status["return"]["running"], true);
    let splash_deadline = Duration::from_secs(5);
    loop {
        let log = fs::read_to_string(&serial.0).unwrap_or_default();
        if log.contains("[splash] Static metalOS logotype visible.") {
            assert!(
                launch_time.elapsed() < splash_deadline,
                "boot screen was delayed by model loading"
            );
            break;
        }
        assert!(
            launch_time.elapsed() < splash_deadline,
            "boot screen did not appear within 5 seconds: {log}"
        );
        assert!(
            guest.child.try_wait().unwrap().is_none(),
            "QEMU exited before splash"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    eprintln!("Boot screen visible after {:?}", launch_time.elapsed());
    loop {
        let log = fs::read_to_string(&serial.0).unwrap_or_default();
        if log.contains("[forge] Model initialization complete.") {
            if has_root {
                assert!(
                    log.contains("[CobaltFS]"),
                    "root FS incorrectly selected the model disk: {log}"
                );
            }
            let transfer = log
                .lines()
                .find(|line| line.contains("[forge] Model disk transfer complete"))
                .unwrap();
            let frames: u64 = transfer
                .split(", ")
                .nth(1)
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap()
                .parse()
                .unwrap();
            assert!(frames > 20, "UI did not keep rendering during model load");
            eprintln!(
                "Model initialized after {:?}; UI advanced {} frames",
                launch_time.elapsed(),
                frames
            );
            assert_eq!(
                log.contains("[forge] Local model ready."),
                expect_ready,
                "unexpected model readiness: {log}"
            );
            let splash = log
                .find("[splash] Static metalOS logotype visible.")
                .unwrap();
            let loading = log
                .find("[forge] Model disk read started after splash.")
                .unwrap();
            assert!(splash < loading, "model I/O started before splash");
            assert!(
                log.contains("[systemui] Slint window online;"),
                "UI loop never started"
            );
            break;
        }
        assert!(
            launch_time.elapsed() < Duration::from_secs(180),
            "model transfer stalled: {log}"
        );
        assert!(
            guest.child.try_wait().unwrap().is_none(),
            "QEMU exited during model transfer: {log}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    eprintln!("QEMU initialized; checking model bytes in guest RAM");
    let dump = std::env::temp_dir().join(format!("metalos-model-check-{}.bin", unique_id()));
    // Check header, sampled boundaries, and the final bytes in actual guest RAM.
    let chunk = 256 * 1024 * 1024;
    let mut offsets = vec![0, length - 16];
    for boundary in (chunk..length).step_by(chunk as usize) {
        offsets.push(boundary - 8);
    }
    for offset in offsets {
        guest.request(json!({"execute": "pmemsave", "arguments": {
            "val": 0x8000_0000u64 + offset, "size": 16, "filename": dump
        }}));
        let actual = fs::read(&dump).unwrap();
        fs::remove_file(&dump).unwrap();
        source.seek(SeekFrom::Start(offset)).unwrap();
        let mut expected = [0; 16];
        source.read_exact(&mut expected).unwrap();
        assert_eq!(actual, expected, "model bytes differ at offset {offset}");
    }
    // Release QEMU before removing test assets.
    drop(guest);
}

struct SyntheticImage(PathBuf);
impl Drop for SyntheticImage {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
#[test]
#[ignore = "requires QEMU and disk space for a >2 GiB regression fixture"]
fn large_synthetic_image_loads_into_qemu() {
    let image = SyntheticImage(
        std::env::temp_dir().join(format!("metalos-large-fixture-{}.img", std::process::id())),
    );
    let length = 2u64 * 1024 * 1024 * 1024 + 512;
    let mut file = File::create(&image.0).unwrap();
    file.set_len(length).unwrap();
    let mut header = [0u8; 4096];
    header[..8].copy_from_slice(b"FGMODEL1");
    header[8..16].copy_from_slice(&(length - 4096).to_le_bytes());
    header[48..61].copy_from_slice(b"gemma-2-2b-it");
    file.write_all(&header).unwrap();
    for boundary in (256 * 1024 * 1024..length).step_by(256 * 1024 * 1024) {
        file.seek(SeekFrom::Start(boundary - 8)).unwrap();
        file.write_all(b"boundary-marker!").unwrap();
    }
    file.seek(SeekFrom::Start(length - 16)).unwrap();
    file.write_all(b"final-model-data").unwrap();
    file.flush().unwrap();
    verify_image(image.0.clone(), false);
}
