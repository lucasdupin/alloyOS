mod image;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "xtask", about = "alloyOS build, run, and test automation")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug, PartialEq, Eq)]
enum Commands {
    /// Builds kernel, userland crates, and the bootable AArch64 kernel image
    Build {
        #[arg(long)]
        release: bool,
        #[arg(long)]
        debug_shell: bool,
    },
    /// Rebuilds and packages protected executables without building the kernel
    Processes {
        #[arg(long)] release: bool,
        /// Package into a separate image without changing a running guest's disk.
        #[arg(long)] disk_image: Option<PathBuf>,
    },
    /// Builds then launches alloyOS in QEMU (aarch64 virt)
    Run {
        #[arg(long)]
        release: bool,
        #[arg(long)]
        debug_shell: bool,
        /// Attach a packaged Forge model to the UI guest
        #[arg(long)]
        model_image: Option<PathBuf>,
        /// Guest RAM; defaults to 16G with a model, otherwise 512M
        #[arg(long)]
        memory: Option<String>,
        /// Cloud credential environment references (never key values)
        #[arg(long)] auth_env: Vec<String>,
        /// Private cloud credential files
        #[arg(long)] auth_file: Vec<PathBuf>,
    },
    /// Launches the host development bridge and its loopback Forge endpoint
    ForgeBridge {
        #[arg(long)] release: bool,
        #[arg(long, default_value = "127.0.0.1:7070")] bind: std::net::SocketAddr,
        #[arg(long, default_value = "16G")] memory: String,
        #[arg(long, default_value = "120")] timeout: u64,
        #[arg(long)] auth_env: Vec<String>,
        #[arg(long)] auth_file: Vec<PathBuf>,
        #[arg(long)] model_image: Option<PathBuf>,
    },
    /// Runs headless E2E QEMU tests
    Test {
        #[arg(long, default_value = "60")]
        timeout: u64,
        #[arg(long)]
        release: bool,
        #[arg(long)]
        disk_image: Option<PathBuf>,
    },
    /// Runs workspace tests followed by headless QEMU E2E tests
    Ci,
    /// Builds the alloyOS POSIX compatibility sysroot (newlib, libc++, pthreads, ICU)
    Sysroot {
        #[arg(long)]
        force: bool,
    },
    /// Fetches WPE WebKit and builds the native AArch64 JavaScriptCore engine
    Jsc {
        #[arg(long)]
        force: bool,
    },
    /// Cross-compiles WebCore C/C++ rendering and parsing dependencies (zlib, png, jpeg, sqlite, xml2, freetype, harfbuzz)
    WebcoreDeps {
        #[arg(long)]
        force: bool,
    },
    /// Cross-compiles and packages native WPE WebKit 2.54.1 for alloyOS AArch64
    Wpe {
        #[arg(long)]
        force: bool,
    },
}

/// Harness for controlling and testing a headless QEMU virtual machine (`qemu-system-aarch64`).
pub struct QemuHarness {
    child: Child,
    stdin: ChildStdin,
    output_buffer: Arc<Mutex<String>>,
    _reader_thread: JoinHandle<()>,
    deadline: Option<Instant>,
}

impl QemuHarness {
    /// Spawns a `qemu-system-aarch64` instance on `-M virt` with stdio connected to PL011 UART0.
    pub fn new(kernel_image: &Path, cobalt_image: &Path) -> Result<Self> {
        let mut child = Command::new("qemu-system-aarch64")
            .arg("-M")
            .arg("virt")
            .arg("-accel")
            .arg("hvf")
            .arg("-cpu")
            .arg("host")
            .arg("-kernel")
            .arg(kernel_image)
            .arg("-drive")
            .arg(format!(
                "file={},if=none,format=raw,id=hd0",
                cobalt_image.display()
            ))
            .arg("-device")
            .arg("virtio-blk-device,drive=hd0")
            .arg("-device")
            .arg(format!("virtio-gpu-device,xres={},yres={}", alloyos_proto::display::PHONE_WIDTH, alloyos_proto::display::PHONE_HEIGHT))
            .arg("-device")
            .arg("virtio-keyboard-device")
        .arg("-device").arg("virtio-rng-device")
            .arg("-device")
            .arg("virtio-tablet-device")
            .arg("-netdev")
            .arg("user,id=net0")
            .arg("-device")
            .arg("virtio-net-device,netdev=net0")
            .arg("-serial")
            .arg("stdio")
            .arg("-display")
            .arg("none")
            .arg("-m")
            .arg("512M")
            .arg("-no-reboot")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| {
                format!(
                    "Failed to spawn qemu-system-aarch64 for kernel {}",
                    kernel_image.display()
                )
            })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("Failed to capture QEMU stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Failed to capture QEMU stdout"))?;

        let output_buffer = Arc::new(Mutex::new(String::new()));
        let buffer_clone = Arc::clone(&output_buffer);

        let reader_thread = thread::spawn(move || {
            use std::io::Read;
            let mut reader = stdout;
            let mut buf = [0u8; 256];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let chunk = String::from_utf8_lossy(&buf[..n]);
                let mut lock = buffer_clone.lock().unwrap();
                lock.push_str(&chunk);
            }
        });

        Ok(Self {
            child,
            stdin,
            output_buffer,
            _reader_thread: reader_thread,
            deadline: None,
        })
    }

    /// Sends a text command to QEMU stdin followed by a newline.
    pub fn send_command(&mut self, cmd: &str) {
        let mut full_cmd = cmd.to_string();
        if !full_cmd.ends_with('\n') {
            full_cmd.push('\n');
        }
        let _ = self.stdin.write_all(full_cmd.as_bytes());
        let _ = self.stdin.flush();
    }

    /// Checks if serial output contains expected pattern within given timeout.
    pub fn expect_output(&mut self, pattern: &str, timeout: Duration) -> Result<()> {
        let start = Instant::now();
        let timeout = self.deadline.map_or(timeout, |deadline| {
            timeout.min(deadline.saturating_duration_since(start))
        });
        while start.elapsed() < timeout {
            {
                let lock = self.output_buffer.lock().unwrap();
                if lock.contains(pattern) {
                    return Ok(());
                }
            }

            if let Some(status) = self.child.try_wait()? {
                let lock = self.output_buffer.lock().unwrap();
                if lock.contains(pattern) {
                    return Ok(());
                }
                return Err(anyhow!(
                    "QEMU exited prematurely with status {} before finding {:?}. Output:\n{}",
                    status,
                    pattern,
                    *lock
                ));
            }

            thread::sleep(Duration::from_millis(50).min(timeout.saturating_sub(start.elapsed())));
        }

        let lock = self.output_buffer.lock().unwrap();
        Err(anyhow!(
            "Timed out after {:?} waiting for {:?}. Output:\n{}",
            timeout,
            pattern,
            *lock
        ))
    }

    /// Waits for QEMU process to exit and returns exit code.
    pub fn wait_for_exit(&mut self, timeout: Duration) -> Result<i32> {
        let start = Instant::now();
        let timeout = self.deadline.map_or(timeout, |deadline| {
            timeout.min(deadline.saturating_duration_since(start))
        });
        while start.elapsed() < timeout {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status.code().unwrap_or(0));
            }
            thread::sleep(Duration::from_millis(50).min(timeout.saturating_sub(start.elapsed())));
        }
        let _ = self.child.kill();
        Err(anyhow!("QEMU did not exit within {:?}", timeout))
    }
}

impl Drop for QemuHarness {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn get_workspace_root() -> Result<PathBuf> {
    // Resolve at runtime: cached binaries may retain a previous checkout path.
    let cwd = std::env::current_dir().context("Could not read current directory")?;
    let executable = std::env::current_exe().context("Could not locate xtask executable")?;
    find_workspace_root(&cwd)
        .or_else(|| executable.parent().and_then(find_workspace_root))
        .ok_or_else(|| anyhow!("Could not locate alloyOS workspace from {} or {}", cwd.display(), executable.display()))
}

fn find_workspace_root(start: &Path) -> Option<PathBuf> {
    start.ancestors().find(|path| {
        path.join("Cargo.toml").is_file()
            && path.join("xtask/Cargo.toml").is_file()
            && path.join("crates/alloyos-kernel/Cargo.toml").is_file()
    }).map(Path::to_path_buf)
}

fn get_disk_image_path(workspace_root: &Path, release: bool) -> PathBuf {
    let profile = if release { "release" } else { "debug" };
    workspace_root
        .join("target")
        .join("aarch64-unknown-none")
        .join(profile)
        .join("alloyos-kernel")
}

fn get_cobaltfs_image_path(workspace_root: &Path, release: bool) -> PathBuf {
    let profile = if release { "release" } else { "debug" };
    workspace_root
        .join("target")
        .join("aarch64-unknown-none")
        .join(profile)
        .join("cobaltfs.img")
}

/// Formats a fresh 128 MiB CobaltFS (ext2-compatible) disk image in pure Rust so
/// no external `mkfs.ext2` binary is required on macOS.
fn ensure_cobaltfs_image(path: &Path) -> Result<()> {
    // Interrupted migration packaging is rolled back before retrying.
    let backup=path.with_extension("pre-process-backup.img");
    let pending=path.with_extension("migration-pending");
    if pending.exists() {
        if !backup.exists() { return Err(anyhow!("migration recovery backup missing")); }
        std::fs::rename(&backup,path)?;
        std::fs::remove_file(&pending)?;
    }
    if path.exists() {
        if std::fs::metadata(path)?.len() >= 512*1024*1024 {
            return image::copy_tree(path, &get_workspace_root()?.join("apps"), "/apps");
        }
        let nonce=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
        if backup.exists() {
            // A completed earlier upgrade may have retained its backup. Keep it
            // recoverable without confusing it with this transaction's rollback.
            let archive = path.with_extension(format!("pre-process-backup-{nonce}.img"));
            std::fs::rename(&backup, &archive)?;
            println!("[xtask] Retained previous migration backup at {}", archive.display());
        }
        let staging=path.with_extension(format!("migration-{}-{nonce}",std::process::id()));
        let replacement=path.with_extension(format!("replacement-{}-{nonce}",std::process::id()));
        let prepared=(|| -> Result<()> {
            image::export_tree(path,&staging)?;
            ensure_cobaltfs_image(&replacement)?;
            image::copy_tree(&replacement,&staging,"/")?;
            image::copy_tree(&replacement,&get_workspace_root()?.join("apps"),"/apps")?;
            std::fs::copy(path,&backup)?;
            std::fs::write(&pending,b"pending executable packaging")?;
            std::fs::rename(&replacement,path)?;
            Ok(())
        })();
        let _=std::fs::remove_dir_all(&staging);
        let _=std::fs::remove_file(&replacement);
        if prepared.is_err() {
            if pending.exists() { let _=std::fs::remove_file(&pending); }
            // Active source is unchanged until the final atomic rename.
            let _=std::fs::remove_file(&backup);
        }
        return prepared;
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    println!(
        "[xtask] Formatting new 512 MiB CobaltFS volume at {}...",
        path.display()
    );

    let img = alloyos_fs::image::development_image();

    std::fs::write(path, &img)
        .with_context(|| format!("Failed to write CobaltFS image to {}", path.display()))?;
    image::copy_tree(path, &get_workspace_root()?.join("apps"), "/apps")
}

fn build_crates(release: bool, debug_shell: bool) -> Result<PathBuf> {
    build_crates_mode(release, debug_shell, false)
}

fn build_crates_mode(release: bool, debug_shell: bool, forge: bool) -> Result<PathBuf> {
    build_crates_mode_with_disk(release, debug_shell, forge, None)
}

fn build_crates_mode_with_disk(
    release: bool,
    debug_shell: bool,
    forge: bool,
    disk: Option<&Path>,
) -> Result<PathBuf> {
    let workspace_root = get_workspace_root()?;

    println!("[xtask] Building kernel crate for aarch64-unknown-none...");
    let mut kernel_cmd = Command::new("cargo");
    kernel_cmd
        .current_dir(&workspace_root)
        .arg("build")
        .arg("--package")
        .arg("alloyos-kernel")
        .arg("--target")
        .arg("aarch64-unknown-none");
    if debug_shell {
        kernel_cmd.args(["--features", "debug-shell"]);
    }
    if forge { kernel_cmd.args(["--features", "forge"]); }
    if release {
        kernel_cmd.arg("--release");
    }
    let status = kernel_cmd
        .status()
        .context("Failed to run cargo build for kernel")?;
    if !status.success() {
        return Err(anyhow!("Kernel compilation failed with status {}", status));
    }

    build_processes(&workspace_root, release)?;
    let kernel_bin = get_disk_image_path(&workspace_root, release);
    if !kernel_bin.exists() {
        return Err(anyhow!(
            "Kernel binary does not exist at {}",
            kernel_bin.display()
        ));
    }
    println!(
        "[xtask] Built bootable AArch64 kernel ELF: {}",
        kernel_bin.display()
    );

    let cobalt_image = disk.map(Path::to_path_buf)
        .unwrap_or_else(|| get_cobaltfs_image_path(&workspace_root, release));
    ensure_cobaltfs_image(&cobalt_image)?;
    package_processes(&workspace_root, release, &cobalt_image)?;

    Ok(kernel_bin)
}

fn build_processes(root: &Path, _release: bool) -> Result<()> {
    for (package, binary, features) in [("alloyos-ferrofluid", "ferrofluid", "executable"), ("alloyos-welder", "welder", "executable"), ("alloyos-user", "app-runner", "")] {
        let mut cmd=Command::new("cargo");cmd.current_dir(root).args(["rustc", "-p",package,"--bin",binary,"--target","aarch64-unknown-none"]);
        if !features.is_empty(){cmd.args(["--features",features]);} cmd.arg("--release");
        if binary=="app-runner" {cmd.args(["--", "-C", "strip=symbols", "-C", "link-arg=-Tcrates/alloyos-ferrofluid/process.ld"]);} else {cmd.args(["--", "-C", "strip=symbols"]);}
        if !cmd.status()?.success(){return Err(anyhow!("{binary} executable build failed"))}
    } Ok(())
}
fn package_processes(root:&Path, release:bool, disk:&Path)->Result<()> {
    let result=package_processes_inner(root,release,disk);
    let pending=disk.with_extension("migration-pending");
    if pending.exists() {
        if result.is_err() {
            let backup=disk.with_extension("pre-process-backup.img");
            std::fs::rename(&backup,disk).with_context(|| "failed to restore migration backup after packaging failure")?;
        }
        std::fs::remove_file(pending)?;
    }
    result
}
fn package_processes_inner(root:&Path, _release:bool, disk:&Path)->Result<()> {
    let profile="release";
    let staging=root.join("target").join("process-package");
    std::fs::create_dir_all(staging.join("system"))?;std::fs::create_dir_all(staging.join("bin"))?;
    for (binary,dir) in [("ferrofluid","system"),("welder","system"),("app-runner","bin")] {
      std::fs::copy(root.join("target/aarch64-unknown-none").join(profile).join(binary),staging.join(dir).join(binary))?;
    }
    let jsc_bin = root.join("build/bin/jsc");
    if jsc_bin.is_file() {
        std::fs::copy(&jsc_bin, staging.join("bin").join("jsc"))?;
    }
    image::copy_tree(disk,&staging,"/")
}

fn attach_cloud_credentials(command: &mut Command, root: &Path, mut env_names: Vec<String>, mut files: Vec<PathBuf>) -> Result<Option<alloyos_dev_harness::credentials::BootCredentials>> {
    // Explicit references take precedence over automatic discovery.
    if env_names.is_empty() && files.is_empty() {
        for name in ["ANTHROPIC_API_KEY", "GEMINI_API_KEY", "OPENAI_API_KEY"] {
            if std::env::var_os(name).is_some() {
                env_names.push(name.into());
            } else {
                let file=root.join(".secrets").join(name);
                if file.is_file() { files.push(file); }
            }
        }
    }
    alloyos_dev_harness::credentials::attach(command,&env_names,&files).map_err(|e|anyhow!(e))
}

fn run_qemu(release: bool, debug_shell: bool, model_image: Option<PathBuf>, memory: Option<String>, auth_env: Vec<String>, auth_file: Vec<PathBuf>) -> Result<()> {
    let forge_mode = model_image.is_some();
    let memory = memory.unwrap_or_else(|| if model_image.is_some() { "16G" } else { "512M" }.into());
    if model_image.is_some() {
        validate_forge_launch(&memory, model_image.as_deref(), 120)?;
    } else if parse_ram(&memory)? < 512u64 << 20 {
        return Err(anyhow!("Standard guest RAM must be at least 512 MiB"));
    }
    let workspace_root = get_workspace_root()?;
    let kernel_image = build_crates_mode(release, debug_shell, model_image.is_some())?;
    let cobalt_image = get_cobaltfs_image_path(&workspace_root, release);
    ensure_cobaltfs_image(&cobalt_image)?;

    println!(
        "[xtask] Launching qemu-system-aarch64 with kernel {} (CobaltFS: {})...",
        kernel_image.display(),
        cobalt_image.display()
    );
    #[cfg(target_os = "macos")]
    // Scale the portrait framebuffer into a resizable desktop window.
    // Native macOS full-screen would move QEMU into a separate Space.
    let display_arg = "cocoa,show-cursor=on,full-screen=off,zoom-to-fit=on";
    #[cfg(not(target_os = "macos"))]
    let display_arg = "default,show-cursor=on";

    let mut qemu = Command::new("qemu-system-aarch64");
    if let Some(image) = &model_image {
        alloyos_dev_harness::model_loader::attach_disk(image, &mut qemu)
            .context("Failed to attach model disk")?;
    }
    #[cfg(target_os = "macos")]
    {
        let helper = workspace_root.join("target/cocoa_window.dylib");
        let compiled = Command::new("clang")
            .args(["-dynamiclib", "-fobjc-arc", "-fblocks", "-framework", "Cocoa"])
            .arg(workspace_root.join("xtask/cocoa_window.m"))
            .arg("-o")
            .arg(&helper)
            .status()
            .context("Failed to build the macOS window sizing helper")?;
        if !compiled.success() {
            return Err(anyhow!("macOS window sizing helper compilation failed"));
        }
        let mut libraries = helper.into_os_string();
        if let Some(existing) = std::env::var_os("DYLD_INSERT_LIBRARIES") {
            libraries.push(":");
            libraries.push(existing);
        }
        qemu.env("DYLD_INSERT_LIBRARIES", libraries);
    }
    let _credentials=attach_cloud_credentials(&mut qemu,&workspace_root,auth_env,auth_file)?;
    qemu.arg("-M")
        .arg("virt")
        .arg("-accel")
        .arg("hvf")
        .arg("-cpu")
        .arg("host")
        .arg("-kernel")
        .arg(&kernel_image)
        .arg("-drive")
        .arg(format!(
            "file={},if=none,format=raw,id=hd0",
            cobalt_image.display()
        ))
        .arg("-device")
        .arg("virtio-blk-device,drive=hd0")
        .arg("-device")
        .arg(format!("virtio-gpu-device,xres={},yres={}", alloyos_proto::display::PHONE_WIDTH, alloyos_proto::display::PHONE_HEIGHT))
        .arg("-device")
        .arg("virtio-keyboard-device")
        .arg("-device").arg("virtio-rng-device")
        .arg("-device")
        .arg("virtio-tablet-device")
        .arg("-netdev")
        .arg("user,id=net0")
        .arg("-device")
        .arg("virtio-net-device,netdev=net0")
        .arg("-serial")
        .arg("stdio")
        .arg("-display")
        .arg(display_arg)
        .arg("-m")
        .arg(memory)
        .arg("-no-reboot")
        .arg("-monitor").arg("none");
    if debug_shell && !forge_mode {
        let status=qemu.status().context("Failed to execute qemu-system-aarch64")?;
        if !status.success() { return Err(anyhow!("QEMU exited with an error")); }
    } else {
        #[cfg(target_os = "macos")]
        if std::env::var_os("ALLOYOS_VOICE_HELPER").is_none() {
            let helper=workspace_root.join("target/alloyos-voice-helper");
            let helper_time=std::fs::metadata(&helper).and_then(|m|m.modified()).ok();
            let stale=["scripts/audio/VoiceHelper.m","scripts/audio/Info.plist","scripts/audio/build.sh"].iter().any(|source| {
                let time=std::fs::metadata(workspace_root.join(source)).and_then(|m|m.modified()).ok();
                helper_time.is_none() || time.zip(helper_time).is_some_and(|(source,built)| source>built)
            });
            if stale {
                let status=Command::new("sh").arg(workspace_root.join("scripts/audio/build.sh")).status();
                if !matches!(status,Ok(s) if s.success()) {eprintln!("Voice helper unavailable; text chat remains available.");}
            }
            std::env::set_var("ALLOYOS_VOICE_HELPER",helper);
        }
        alloyos_dev_harness::https::run_ui(&mut qemu).map_err(|e|anyhow!(e))?;
    }
    Ok(())
}

struct E2eVolume(PathBuf);
impl E2eVolume {
    fn new() -> Result<Self> {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
        let path = std::env::temp_dir().join(format!("alloyos-e2e-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }
}
impl Drop for E2eVolume {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

fn run_e2e_tests(kernel_image: &Path, overall_timeout: Duration) -> Result<()> {
    if overall_timeout.is_zero() { return Err(anyhow!("E2E timeout must be greater than zero")); }
    let started = Instant::now();
    // Keep user settings within this run, but never inherit another run's config.
    // Declare the volume before the harness so QEMU exits before disk cleanup.
    let volume = E2eVolume::new()?;
    let cobalt_image = volume.0.join("cobaltfs.img");
    ensure_cobaltfs_image(&cobalt_image)?;

    println!(
        "[xtask] Starting QEMU AArch64 E2E test harness for {} (CobaltFS: {})...",
        kernel_image.display(),
        cobalt_image.display()
    );
    let mut harness = QemuHarness::new(kernel_image, &cobalt_image)?;
    harness.deadline = Some(started + overall_timeout);

    println!("[xtask] [TEST 1/10] e2e_boot_banner...");
    harness
        .expect_output("alloyOS v0.1.0 booting...", Duration::from_secs(10))
        .context("e2e_boot_banner failed")?;
    println!("[xtask] [PASS] e2e_boot_banner");

    println!("[xtask] [TEST 2/10] e2e_memory_init...");
    harness
        .expect_output(
            "[mm] Frame allocator and 256 MiB kernel heap online.",
            Duration::from_secs(10),
        )
        .context("e2e_memory_init failed")?;
    harness
        .expect_output(
            "[splash] Boot wordmark presented",
            Duration::from_secs(10),
        )
        .context("e2e_splash_init failed")?;
    harness
        .expect_output("[CobaltFS]", Duration::from_secs(10))
        .context("e2e_cobaltfs_mount failed")?;
    harness
        .expect_output(
            "[net] VirtIO-Net + TCP/IP + DNS + TLS 1.3 stack online.",
            Duration::from_secs(10),
        )
        .context("e2e_net_init failed")?;
    harness
        .expect_output(
            "[ferrofluidd] System UI disabled (serial debug shell).",
            Duration::from_secs(10),
        )
        .context("e2e_ferrofluid_init failed")?;
    harness
        .expect_output(
            "[scriptd] Embedded V8/JS runtime & event/time scheduler online.",
            Duration::from_secs(10),
        )
        .context("e2e_scriptd_init failed")?;
    harness
        .expect_output(
            "[mcp] Headless MCP app platform online",
            Duration::from_secs(10),
        )
        .context("e2e_mcp_init failed")?;
    harness
        .expect_output(
            "[welder] Agent runtime online (active=forge, shell-access=disabled).",
            Duration::from_secs(10),
        )
        .context("e2e_ai_init failed")?;
    println!("[xtask] [PASS] e2e_memory_init + CobaltFS + Splash + Net + Ferrofluid + JS + MCP + AI");

    println!("[xtask] [TEST 3/10] e2e_shell_prompt...");
    harness
        .expect_output("alloyos:/$", Duration::from_secs(10))
        .context("e2e_shell_prompt failed")?;
    println!("[xtask] [PASS] e2e_shell_prompt");

    println!("[xtask] [TEST 4/10] e2e_uname...");
    harness.send_command("uname -a");
    harness
        .expect_output("alloyOS alloyos 0.1.0", Duration::from_secs(5))
        .context("e2e_uname failed")?;
    harness
        .expect_output("aarch64", Duration::from_secs(5))
        .context("e2e_uname machine failed")?;
    println!("[xtask] [PASS] e2e_uname");

    println!("[xtask] [TEST 5/10] e2e_ls_and_cd...");
    harness.send_command("ls /");
    harness
        .expect_output("dev/\nproc/", Duration::from_secs(5))
        .context("e2e_ls_root failed")?;
    harness.send_command("ls /etc");
    harness
        .expect_output("motd", Duration::from_secs(5))
        .context("e2e_ls_etc failed")?;
    harness.send_command("ls etc/*");
    harness
        .expect_output("etc/motd", Duration::from_secs(5))
        .context("e2e_ls_etc_glob failed")?;
    harness.send_command("cd /dev");
    harness
        .expect_output("alloyos:/dev$", Duration::from_secs(5))
        .context("e2e_cd_dev_prompt failed")?;
    harness.send_command("pwd && cd ..");
    harness
        .expect_output("/dev\nalloyos:/$", Duration::from_secs(5))
        .context("e2e_pwd_and_cd_parent failed")?;
    println!("[xtask] [PASS] e2e_ls_and_cd");

    println!("[xtask] [TEST 6/10] e2e_echo_redirect_cat...");
    harness.send_command("echo e2e_file_payload > /f && cat /f");
    harness
        .expect_output("e2e_file_payload\n", Duration::from_secs(5))
        .context("e2e_echo_redirect_cat failed")?;
    println!("[xtask] [PASS] e2e_echo_redirect_cat");

    println!("[xtask] [TEST 7/10] e2e_ps_shows_pid1...");
    harness.send_command("ps");
    harness
        .expect_output("1 tty0     00:00:00 sh", Duration::from_secs(5))
        .context("e2e_ps_shows_pid1 failed")?;
    println!("[xtask] [PASS] e2e_ps_shows_pid1");

    println!("[xtask] [TEST 8/10] e2e_mem_command...");
    harness.send_command("mem");
    harness
        .expect_output("MemTotal:", Duration::from_secs(5))
        .context("e2e_mem_command failed")?;
    println!("[xtask] [PASS] e2e_mem_command");

    println!("[xtask] [TEST 9/10] e2e_bench_completes...");
    harness.send_command("bench");
    harness
        .expect_output("VFS open/read/close", Duration::from_secs(10))
        .context("e2e_bench_completes failed")?;
    println!("[xtask] [PASS] e2e_bench_completes");

    println!("[xtask] [TEST 10/10] e2e_shutdown...");
    harness.send_command("shutdown");
    let exit_code = harness
        .wait_for_exit(Duration::from_secs(5))
        .context("e2e_shutdown failed")?;
    if exit_code != 0 {
        return Err(anyhow!("QEMU shutdown exited with code {}", exit_code));
    }
    println!("[xtask] [PASS] e2e_shutdown");

    println!("[xtask] All 10 E2E tests PASSED in {:.2}s!", started.elapsed().as_secs_f64());
    Ok(())
}

fn run_ci() -> Result<()> {
    let workspace_root = get_workspace_root()?;
    println!("[xtask] Step 1: Running cargo test --workspace...");
    let status = Command::new("cargo")
        .current_dir(&workspace_root)
        .arg("test")
        .arg("--workspace")
        .arg("--features")
        .arg("alloyos-keyboard/preview")
        .env_remove("ALLOYOS_UPDATE_SCREENSHOTS")
        .status()
        .context("Failed to run cargo test --workspace")?;

    if !status.success() {
        return Err(anyhow!("cargo test --workspace failed"));
    }

    println!("[xtask] Checking model asset tooling (offline)...");
    let assets = Command::new("python3").current_dir(&workspace_root).args(["-m", "unittest", "discover", "-s", "scripts", "-p", "test_*.py"]).status().context("Failed to run model asset and launcher tests")?;
    if !assets.success() { return Err(anyhow!("model asset tests failed")); }
    println!("[xtask] Step 2: Building alloyOS (aarch64) and executing E2E tests...");
    // The build stage packages executables too; isolate it just like the QEMU
    // stage rather than touching a developer's active CobaltFS volume.
    let volume = E2eVolume::new()?;
    let disk = volume.0.join("cobaltfs.img");
    let kernel_image = build_crates_mode_with_disk(false, true, false, Some(&disk))?;
    run_e2e_tests(&kernel_image, Duration::from_secs(60))?;
    Ok(())
}

fn parse_ram(memory: &str) -> Result<u64> {
    let scale = match memory.as_bytes().last() {
        Some(b'M' | b'm') => 1u64 << 20,
        Some(b'G' | b'g') => 1u64 << 30,
        _ => return Err(anyhow!("RAM must be a positive integer followed by M or G")),
    };
    let digits = &memory[..memory.len() - 1];
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(anyhow!("invalid RAM size"));
    }
    digits.parse::<u64>().ok().and_then(|n| n.checked_mul(scale)).filter(|n| *n > 0)
        .ok_or_else(|| anyhow!("invalid RAM size"))
}

fn validate_forge_launch(memory: &str, image: Option<&Path>, timeout: u64) -> Result<()> {
    if timeout == 0 || timeout > 3600 { return Err(anyhow!("timeout must be 1..3600 seconds")); }
    let bytes = parse_ram(memory)?;
    let minimum = if image.is_some() { 16u64 << 30 } else { 4u64 << 30 };
    if bytes < minimum || bytes > 31u64 << 30 { return Err(anyhow!("Forge RAM must be {}..31 GiB", minimum >> 30)); }
    if let Some(path) = image {
        if path.to_string_lossy().contains(',') { return Err(anyhow!("model image path must not contain commas")); }
        let mut file = std::fs::File::open(path).context("cannot open model image")?;
        let length = file.metadata()?.len();
        let mut header = [0u8; 4096];
        std::io::Read::read_exact(&mut file, &mut header).context("model image header truncated")?;
        if &header[..8] != b"FGMODEL1" { return Err(anyhow!("invalid model image magic")); }
        let weights = u64::from_le_bytes(header[8..16].try_into().unwrap());
        let expected = weights.checked_add(4096).and_then(|n| n.checked_add(511)).map(|n| n & !511).ok_or_else(|| anyhow!("model image length overflow"))?;
        if weights < 24 || length != expected { return Err(anyhow!("model image declared size does not match file size")); }
        if length > bytes - (2u64 << 30) { return Err(anyhow!("model image does not fit guest RAM")); }
        let mut magic = [0u8; 4]; std::io::Read::read_exact(&mut file, &mut magic)?;
        if &magic != b"GGUF" { return Err(anyhow!("model payload is not GGUF")); }
    }
    Ok(())
}

fn resolve_llvm_ar() -> Result<PathBuf> {
    let output = Command::new("rustc").args(["--print", "sysroot"]).output()?;
    let sysroot = String::from_utf8(output.stdout)?.trim().to_string();
    let host_output = Command::new("rustc").arg("-vV").output()?;
    let host_str = String::from_utf8(host_output.stdout)?;
    let host = host_str
        .lines()
        .find(|l| l.starts_with("host: "))
        .map(|l| l["host: ".len()..].trim())
        .ok_or_else(|| anyhow!("could not determine rustc host"))?;
    let ar_path = PathBuf::from(sysroot)
        .join("lib/rustlib")
        .join(host)
        .join("bin/llvm-ar");
    if ar_path.is_file() {
        Ok(ar_path)
    } else {
        Err(anyhow!("llvm-ar not found at {}", ar_path.display()))
    }
}

fn run_sysroot(_force: bool) -> Result<()> {
    let root = get_workspace_root()?;
    println!("[xtask] Ensuring alloyOS POSIX compatibility sysroot...");
    let sysroot_dir = root.join("build/sysroot");
    let script = root.join("scripts/web-engine/build-sysroot.py");
    let newlib_dir = PathBuf::from("/private/tmp/alloyos-newlib-full/sysroot/aarch64-none-elf");

    let mut cmd = Command::new("python3");
    cmd.current_dir(&root).arg(script).arg("--output").arg(&sysroot_dir);
    if newlib_dir.is_dir() {
        cmd.arg("--newlib").arg(&newlib_dir);
    }
    let status = cmd.status().context("Failed to run build-sysroot.py")?;
    if !status.success() {
        return Err(anyhow!("build-sysroot.py failed"));
    }

    let staging_sysroot = PathBuf::from("/private/tmp/web-runtime-newlib-acceptance/sysroot");
    if staging_sysroot.is_dir() {
        let _ = std::fs::remove_dir_all(sysroot_dir.join("include/c++"));
        let _ = Command::new("cp")
            .args(["-R"])
            .arg(staging_sysroot.join("include/c++"))
            .arg(sysroot_dir.join("include/"))
            .status();
        let _ = Command::new("cp")
            .args(["-R"])
            .arg(staging_sysroot.join("include/unicode"))
            .arg(sysroot_dir.join("include/"))
            .status();
        for lib in [
            "libc++.a",
            "libc++abi.a",
            "libicudata.a",
            "libicui18n.a",
            "libicuuc.a",
        ] {
            let src = staging_sysroot.join("lib").join(lib);
            if src.is_file() {
                let _ = std::fs::copy(&src, sysroot_dir.join("lib").join(lib));
            }
        }
    }

    let overlay = root.join("scripts/web-engine/sysroot/include/overlay");
    if overlay.is_dir() {
        for entry in ["alloyos", "sys", "pthread.h", "sched.h", "semaphore.h", "dlfcn.h"] {
            let src = overlay.join(entry);
            let dst = sysroot_dir.join("include").join(entry);
            if src.is_dir() {
                let _ = Command::new("cp")
                    .args(["-R"])
                    .arg(&src)
                    .arg(sysroot_dir.join("include/"))
                    .status();
            } else if src.is_file() {
                let _ = std::fs::copy(&src, &dst);
            }
        }
    }

    println!(
        "[xtask] alloyOS AArch64 sysroot is up to date at {}",
        sysroot_dir.display()
    );
    Ok(())
}

fn run_jsc(force: bool) -> Result<()> {
    let root = get_workspace_root()?;
    run_sysroot(false)?;

    let wpe_source = root.join("build/wpewebkit");
    if !wpe_source.join(".alloyos-patched").is_file() || force {
        println!("[xtask] Fetching and patching WPE WebKit 2.54.1...");
        let status = Command::new("python3")
            .current_dir(&root)
            .arg(root.join("scripts/web-engine/fetch-webkit.py"))
            .status()
            .context("Failed to run fetch-webkit.py")?;
        if !status.success() {
            return Err(anyhow!("fetch-webkit.py failed"));
        }
    }

    println!("[xtask] Configuring and building JavaScriptCore (JSCOnly) for AArch64 alloyOS...");
    let jsc_build = root.join("build/jsc");
    if !jsc_build.join("build.ninja").is_file() || force {
        let status = Command::new("sh")
            .current_dir(&root)
            .arg(root.join("scripts/web-engine/configure-jsc.sh"))
            .status()
            .context("Failed to run configure-jsc.sh")?;
        if !status.success() {
            return Err(anyhow!("configure-jsc.sh failed"));
        }
    }

    let status = Command::new("ninja")
        .current_dir(&jsc_build)
        .arg("jsc")
        .status()
        .context("Failed to run ninja jsc")?;
    if !status.success() {
        return Err(anyhow!("ninja jsc build failed"));
    }

    // Convert thin archives to standalone fat archives in build/sysroot/lib
    let ar = resolve_llvm_ar()?;
    for (name, fat_name) in [
        ("libJavaScriptCore.a", "libJavaScriptCore_fat.a"),
        ("libWTF.a", "libWTF_fat.a"),
        ("libbmalloc.a", "libbmalloc_fat.a"),
    ] {
        let lib_dir = jsc_build.join("lib");
        let thin = lib_dir.join(name);
        if thin.is_file() {
            let list_out = Command::new(&ar)
                .args(["-t"])
                .arg(&thin)
                .output()?;
            let obj_str = String::from_utf8(list_out.stdout)?;
            let mut objs: Vec<PathBuf> = Vec::new();
            for line in obj_str.split_whitespace() {
                let p = lib_dir.join(line);
                if p.is_file() {
                    objs.push(p);
                }
            }
            if name == "libJavaScriptCore.a" {
                let jit_dir = jsc_build.join("Source/JavaScriptCore/CMakeFiles/JavaScriptCoreJIT.dir");
                if jit_dir.is_dir() {
                    let mut stack = vec![jit_dir];
                    while let Some(dir) = stack.pop() {
                        if let Ok(entries) = std::fs::read_dir(dir) {
                            for entry in entries.flatten() {
                                let path = entry.path();
                                if path.is_dir() {
                                    stack.push(path);
                                } else if path.extension().and_then(|s| s.to_str()) == Some("obj") {
                                    objs.push(path);
                                }
                            }
                        }
                    }
                }
            }
            if !objs.is_empty() {
                let fat_path = lib_dir.join(fat_name);
                let _ = std::fs::remove_file(&fat_path);
                for chunk in objs.chunks(50) {
                    let mut cmd = Command::new(&ar);
                    cmd.arg("-rcs").arg(&fat_path);
                    for obj in chunk {
                        cmd.arg(obj);
                    }
                    let status = cmd.status()?;
                    if !status.success() {
                        return Err(anyhow!("llvm-ar failed creating fat archive {}", fat_name));
                    }
                }
                if fat_path.is_file() {
                    let dest = root.join("build/sysroot/lib").join(name);
                    let _ = std::fs::copy(&fat_path, &dest);
                }
            }
        }
    }

    // Install JavaScriptCore C API headers to sysroot
    let jsc_headers_src = jsc_build.join("JavaScriptCore/Headers/JavaScriptCore");
    let jsc_headers_dest = root.join("build/sysroot/include/JavaScriptCore");
    if jsc_headers_src.is_dir() {
        std::fs::create_dir_all(&jsc_headers_dest)?;
        for entry in std::fs::read_dir(&jsc_headers_src)? {
            let entry = entry?;
            if entry.path().extension().and_then(|s| s.to_str()) == Some("h") {
                let _ = std::fs::copy(entry.path(), jsc_headers_dest.join(entry.file_name()));
            }
        }
    }

    // Strip and package binary
    let raw_bin = jsc_build.join("bin/jsc");
    let target_bin = root.join("build/bin/jsc");
    std::fs::create_dir_all(root.join("build/bin"))?;
    let strip_cmd = ar.parent().unwrap().join("llvm-strip");
    if strip_cmd.is_file() && raw_bin.is_file() {
        let _ = Command::new(&strip_cmd)
            .arg(&raw_bin)
            .arg("-o")
            .arg(&target_bin)
            .status();
    } else if raw_bin.is_file() {
        let _ = std::fs::copy(&raw_bin, &target_bin);
    }

    println!(
        "[xtask] Native AArch64 JavaScriptCore engine built and packaged successfully to {}",
        target_bin.display()
    );
    Ok(())
}

fn run_webcore_deps(force: bool) -> Result<()> {
    let root = get_workspace_root()?;
    run_sysroot(false)?;
    let mut cmd = Command::new("python3");
    cmd.current_dir(&root);
    cmd.arg(root.join("scripts/web-engine/build-webcore-deps.py"));
    if force {
        cmd.arg("--force");
    }
    let status = cmd.status().context("Failed to run build-webcore-deps.py")?;
    if !status.success() {
        return Err(anyhow!("build-webcore-deps.py failed"));
    }
    Ok(())
}

fn run_wpe(force: bool) -> Result<()> {
    let root = get_workspace_root()?;
    run_sysroot(false)?;
    run_webcore_deps(false)?;
    let mut cmd = Command::new("python3");
    cmd.current_dir(&root);
    cmd.arg(root.join("scripts/web-engine/build-wpe.py"));
    if force {
        cmd.arg("--force");
    }
    let status = cmd.status().context("Failed to run build-wpe.py")?;
    if !status.success() {
        return Err(anyhow!("build-wpe.py failed"));
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Sysroot { force } => run_sysroot(force),
        Commands::Jsc { force } => run_jsc(force),
        Commands::WebcoreDeps { force } => run_webcore_deps(force),
        Commands::Wpe { force } => run_wpe(force),
        Commands::Build { release, debug_shell } => {
            let image = build_crates(release, debug_shell)?;
            println!("[xtask] Kernel built: {}", image.display());
            Ok(())
        }
        Commands::Processes { release, disk_image } => {
            let root=get_workspace_root()?;build_processes(&root,release)?;
            let disk=disk_image.unwrap_or_else(|| get_cobaltfs_image_path(&root,release));
            ensure_cobaltfs_image(&disk)?;package_processes(&root,release,&disk)
        }
        Commands::Run { release, debug_shell, model_image, memory, auth_env, auth_file } => run_qemu(release, debug_shell, model_image, memory, auth_env, auth_file),
        Commands::Test {
            timeout,
            release,
            disk_image,
        } => {
            let image = match disk_image {
                Some(path) => path,
                None => build_crates(release, true)?,
            };
            run_e2e_tests(&image, Duration::from_secs(timeout))
        }
        Commands::ForgeBridge { release, bind, memory, timeout, auth_env, auth_file, model_image } => {
            if !bind.ip().is_loopback() { return Err(anyhow!("ForgeBridge must bind to loopback")); }
            validate_forge_launch(&memory, model_image.as_deref(), timeout)?;
            let kernel = build_crates_mode(release, false, true)?;
            let root = get_workspace_root()?;
            let disk = get_cobaltfs_image_path(&root, release);
            ensure_cobaltfs_image(&disk)?;
            let mut command = Command::new("qemu-system-aarch64");
            command.args(["-M", "virt", "-accel", "hvf", "-cpu", "host", "-display", "none", "-serial", "stdio", "-monitor", "none", "-no-reboot", "-m"])
                .arg(memory).arg("-kernel").arg(kernel)
                .arg("-drive").arg(format!("file={},if=none,format=raw,id=hd0", disk.display()))
                .args(["-device", "virtio-blk-device,drive=hd0"]);
            if let Some(image) = &model_image {
                alloyos_dev_harness::model_loader::attach_disk(image, &mut command)
                    .context("Failed to attach model disk")?;
            }
            alloyos_dev_harness::install_shutdown_handler();
            let _credentials = attach_cloud_credentials(&mut command, &root, auth_env, auth_file)?;
            let mut guest = alloyos_dev_harness::Guest::launch(&mut command, Duration::from_secs(timeout)).map_err(|e| anyhow!(e))?;
            // A status response confirms the guest transport. Model readiness is reported in its JSON.
            let status = guest.wait_ready().map_err(|e| anyhow!(e))?;
            println!("Forge status: {status}");
            alloyos_dev_harness::serve(bind, guest).map_err(|e| anyhow!(e))
        },
        Commands::Ci => run_ci(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ram_parser_rejects_invalid_and_unicode_sizes_without_panicking() {
        assert_eq!(parse_ram("512M").unwrap(), 512u64 << 20);
        assert_eq!(parse_ram("16g").unwrap(), 16u64 << 30);
        for value in ["", "0M", "-1G", "M", "1💥", "💥M", "18446744073709551615G"] {
            assert!(parse_ram(value).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn parse_build_command() {
        let args = Cli::parse_from(["xtask", "build", "--release"]);
        assert_eq!(args.command, Commands::Build { release: true, debug_shell: false });

        let args_debug = Cli::parse_from(["xtask", "build"]);
        assert_eq!(args_debug.command, Commands::Build { release: false, debug_shell: false });
    }

    #[test]
    fn workspace_root_resolves_runtime_paths() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        assert_eq!(find_workspace_root(root), Some(root.to_path_buf()));
        assert_eq!(find_workspace_root(&root.join("xtask/src")), Some(root.to_path_buf()));
        assert_eq!(find_workspace_root(&root.join("target/debug")), Some(root.to_path_buf()));
        assert_eq!(find_workspace_root(Path::new("/")), None);
    }

    #[test]
    fn forge_launch_checks_ram_and_model_header() {
        for memory in ["", "G", "0G", "3G", "32G", "4MG", "-4G", "999999999999999999999G"] { assert!(validate_forge_launch(memory, None, 120).is_err(), "{memory}"); }
        assert!(validate_forge_launch("4096M", None, 120).is_ok());
        assert!(validate_forge_launch("16G", None, 0).is_err());
        let path = std::env::temp_dir().join(format!("alloyos-welder-image-test-{}", std::process::id()));
        let mut image = vec![0u8; 4608]; image[..8].copy_from_slice(b"FGMODEL1"); image[8..16].copy_from_slice(&24u64.to_le_bytes()); image[4096..4100].copy_from_slice(b"GGUF");
        std::fs::write(&path, &image).unwrap();
        assert!(validate_forge_launch("16G", Some(&path), 120).is_ok());
        assert!(validate_forge_launch("4G", Some(&path), 120).is_err());
        image[8..16].copy_from_slice(&1024u64.to_le_bytes()); std::fs::write(&path, &image).unwrap();
        assert!(validate_forge_launch("16G", Some(&path), 120).is_err());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn parse_welder_uses_references_and_resident_memory() {
        let cli = Cli::parse_from(["xtask", "forge-bridge", "--auth-env", "HF_TOKEN", "--auth-file", "/private/auth.env"]);
        match cli.command {
            Commands::ForgeBridge {memory, auth_env, auth_file, bind, ..} => {
                assert_eq!(memory, "16G");
                assert_eq!(auth_env, vec!["HF_TOKEN"]);
                assert_eq!(auth_file, vec![PathBuf::from("/private/auth.env")]);
                assert!(bind.ip().is_loopback());
            }
            _ => panic!("expected ForgeBridge command"),
        }
    }

    #[test]
    fn parse_run_command() {
        let args = Cli::parse_from(["xtask", "run"]);
        assert_eq!(args.command, Commands::Run { release: false, debug_shell: false, model_image: None, memory: None, auth_env: Vec::new(), auth_file: Vec::new() });
        let debug = Cli::parse_from(["xtask", "run", "--debug-shell"]);
        assert_eq!(debug.command, Commands::Run { release: false, debug_shell: true, model_image: None, memory: None, auth_env: Vec::new(), auth_file: Vec::new() });
        let model = Cli::parse_from(["xtask", "run", "--model-image", "model.img", "--memory", "16G"]);
        assert_eq!(model.command, Commands::Run { release: false, debug_shell: false, model_image: Some(PathBuf::from("model.img")), memory: Some("16G".into()), auth_env: Vec::new(), auth_file: Vec::new() });
    }

    #[test]
    fn parse_test_command_default_timeout() {
        let args = Cli::parse_from(["xtask", "test"]);
        assert_eq!(
            args.command,
            Commands::Test {
                timeout: 60,
                release: false,
                disk_image: None,
            }
        );
    }

    #[test]
    fn parse_test_command_custom_timeout() {
        let args = Cli::parse_from(["xtask", "test", "--timeout", "120", "--release"]);
        assert_eq!(
            args.command,
            Commands::Test {
                timeout: 120,
                release: true,
                disk_image: None,
            }
        );
    }

    #[test]
    fn e2e_deadline_bounds_output_and_exit_waits() {
        let mut child = Command::new("sleep").arg("5")
            .stdin(Stdio::piped()).spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let mut harness = QemuHarness {
            child,
            stdin,
            output_buffer: Arc::new(Mutex::new(String::new())),
            _reader_thread: thread::spawn(|| {}),
            deadline: Some(Instant::now() + Duration::from_millis(20)),
        };
        let started = Instant::now();
        assert!(harness.expect_output("missing", Duration::from_secs(5)).is_err());
        assert!(harness.wait_for_exit(Duration::from_secs(5)).is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn parse_ci_command() {
        let args = Cli::parse_from(["xtask", "ci"]);
        assert_eq!(args.command, Commands::Ci);
        assert!(Cli::try_parse_from(["xtask", "ci", "--full"]).is_err());
    }

    #[test]
    fn disk_image_path_layout() {
        let root = Path::new("/tmp/test_ws");
        let debug_img = get_disk_image_path(root, false);
        assert_eq!(
            debug_img,
            Path::new("/tmp/test_ws/target/aarch64-unknown-none/debug/alloyos-kernel")
        );
        let release_img = get_disk_image_path(root, true);
        assert_eq!(
            release_img,
            Path::new("/tmp/test_ws/target/aarch64-unknown-none/release/alloyos-kernel")
        );
    }
}
