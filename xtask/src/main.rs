use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "xtask", about = "metalOS build, run, and test automation")]
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
    },
    /// Builds then launches metalOS in QEMU (aarch64 virt)
    Run {
        #[arg(long)]
        release: bool,
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
}

/// Harness for controlling and testing a headless QEMU virtual machine (`qemu-system-aarch64`).
pub struct QemuHarness {
    child: Child,
    stdin: ChildStdin,
    output_buffer: Arc<Mutex<String>>,
    _reader_thread: JoinHandle<()>,
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
            .arg("-serial")
            .arg("stdio")
            .arg("-display")
            .arg("none")
            .arg("-m")
            .arg("256M")
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

            thread::sleep(Duration::from_millis(50));
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
        while start.elapsed() < timeout {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status.code().unwrap_or(0));
            }
            thread::sleep(Duration::from_millis(50));
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
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| anyhow!("Could not find workspace root parent directory"))
}

fn get_disk_image_path(workspace_root: &Path, release: bool) -> PathBuf {
    let profile = if release { "release" } else { "debug" };
    workspace_root
        .join("target")
        .join("aarch64-unknown-none")
        .join(profile)
        .join("metalos-kernel")
}

fn get_cobaltfs_image_path(workspace_root: &Path, release: bool) -> PathBuf {
    let profile = if release { "release" } else { "debug" };
    workspace_root
        .join("target")
        .join("aarch64-unknown-none")
        .join(profile)
        .join("cobaltfs.img")
}

/// Formats a fresh 4 MiB CobaltFS (ext2-compatible) disk image in pure Rust so
/// no external `mkfs.ext2` binary is required on macOS.
fn ensure_cobaltfs_image(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    println!(
        "[xtask] Formatting new 4 MiB CobaltFS volume at {}...",
        path.display()
    );

    let bs = 1024usize;
    let total_blocks = 4096u32; // 4 MiB
    let inodes_count = 256u32; // 256 * 128B = 32 KiB = 32 blocks (blocks 5..37)
    let reserved_blocks = 38u32; // 0:boot, 1:SB, 2:BGD, 3:bmap, 4:imap, 5..37:itable, 37:root dir
    let mut img = vec![0u8; bs * total_blocks as usize];

    let w16 = |b: &mut [u8], o: usize, v: u16| {
        let x = v.to_le_bytes();
        b[o] = x[0];
        b[o + 1] = x[1];
    };
    let w32 = |b: &mut [u8], o: usize, v: u32| {
        let x = v.to_le_bytes();
        b[o..o + 4].copy_from_slice(&x);
    };

    // Superblock at offset 1024 (Block 1)
    {
        let sb = &mut img[1024..2048];
        w32(sb, 0, inodes_count);
        w32(sb, 4, total_blocks);
        w32(sb, 12, total_blocks - reserved_blocks);
        w32(sb, 16, inodes_count - 11);
        w32(sb, 20, 1); // first_data_block = 1
        w32(sb, 24, 0); // log_block_size = 0 (1024 B)
        w32(sb, 32, total_blocks);
        w32(sb, 40, inodes_count);
        w16(sb, 56, 0xEF53); // COBALTFS_MAGIC
        w32(sb, 76, 0); // rev 0
    }

    // Block Group Descriptor at Block 2 (offset 2048)
    {
        let bgd = &mut img[2048..2048 + 32];
        w32(bgd, 0, 3); // block_bitmap = 3
        w32(bgd, 4, 4); // inode_bitmap = 4
        w32(bgd, 8, 5); // inode_table = 5
        w16(bgd, 12, (total_blocks - reserved_blocks) as u16);
        w16(bgd, 14, (inodes_count - 11) as u16);
        w16(bgd, 16, 1); // used_dirs_count = 1 (root)
    }

    // Block bitmap at Block 3 (offset 3072): mark blocks 0..reserved_blocks used
    for b in 0..reserved_blocks as usize {
        img[3072 + b / 8] |= 1u8 << (b % 8);
    }

    // Inode bitmap at Block 4 (offset 4096): mark inodes 1..=11 used
    img[4096] = 0xFF;
    img[4097] = 0b0000_0111;

    // Root inode (ino 2 -> index 1 in inode table at Block 5, offset 5120 + 128)
    {
        let root_off = 5120 + 128;
        let ino = &mut img[root_off..root_off + 128];
        w16(ino, 0, 0x4000 | 0o755); // S_IFDIR | 0755
        w32(ino, 4, bs as u32); // size_lo = 1024
        w16(ino, 26, 2); // links_count = 2
        w32(ino, 28, 2); // blocks_512 = 2
        w32(ino, 40, 37); // block[0] = 37
    }

    // Root directory data block at Block 37: "." and ".."
    {
        let dir = &mut img[37 * bs..38 * bs];
        w32(dir, 0, 2);
        w16(dir, 4, 12);
        dir[6] = 1;
        dir[7] = 2; // FT_DIR
        dir[8] = b'.';

        w32(dir, 12, 2);
        w16(dir, 16, (bs - 12) as u16);
        dir[18] = 2;
        dir[19] = 2; // FT_DIR
        dir[20] = b'.';
        dir[21] = b'.';
    }

    std::fs::write(path, &img)
        .with_context(|| format!("Failed to write CobaltFS image to {}", path.display()))?;
    Ok(())
}

fn build_crates(release: bool) -> Result<PathBuf> {
    let workspace_root = get_workspace_root()?;

    println!("[xtask] Building kernel crate for aarch64-unknown-none...");
    let mut kernel_cmd = Command::new("cargo");
    kernel_cmd
        .current_dir(&workspace_root)
        .arg("build")
        .arg("--package")
        .arg("metalos-kernel")
        .arg("--target")
        .arg("aarch64-unknown-none");
    if release {
        kernel_cmd.arg("--release");
    }
    let status = kernel_cmd
        .status()
        .context("Failed to run cargo build for kernel")?;
    if !status.success() {
        return Err(anyhow!("Kernel compilation failed with status {}", status));
    }

    println!("[xtask] Building userland crate...");
    let mut user_cmd = Command::new("cargo");
    user_cmd
        .current_dir(&workspace_root)
        .arg("build")
        .arg("--package")
        .arg("metalos-user");
    if release {
        user_cmd.arg("--release");
    }
    let status = user_cmd
        .status()
        .context("Failed to run cargo build for user")?;
    if !status.success() {
        return Err(anyhow!("Userland compilation failed with status {}", status));
    }

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

    let cobalt_image = get_cobaltfs_image_path(&workspace_root, release);
    ensure_cobaltfs_image(&cobalt_image)?;

    Ok(kernel_bin)
}

fn run_qemu(release: bool) -> Result<()> {
    let workspace_root = get_workspace_root()?;
    let kernel_image = build_crates(release)?;
    let cobalt_image = get_cobaltfs_image_path(&workspace_root, release);
    ensure_cobaltfs_image(&cobalt_image)?;

    println!(
        "[xtask] Launching qemu-system-aarch64 with kernel {} (CobaltFS: {})...",
        kernel_image.display(),
        cobalt_image.display()
    );
    let status = Command::new("qemu-system-aarch64")
        .arg("-M")
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
        .arg("-serial")
        .arg("stdio")
        .arg("-display")
        .arg("none")
        .arg("-m")
        .arg("256M")
        .arg("-no-reboot")
        .status()
        .context("Failed to execute qemu-system-aarch64")?;

    if !status.success() {
        return Err(anyhow!("QEMU exited with error: {:?}", status.code()));
    }
    Ok(())
}

fn run_e2e_tests(kernel_image: &Path, _overall_timeout: Duration) -> Result<()> {
    let workspace_root = get_workspace_root()?;
    let cobalt_image = workspace_root
        .join("target")
        .join("aarch64-unknown-none")
        .join("debug")
        .join("cobaltfs-e2e.img");
    ensure_cobaltfs_image(&cobalt_image)?;

    println!(
        "[xtask] Starting QEMU AArch64 E2E test harness for {} (CobaltFS: {})...",
        kernel_image.display(),
        cobalt_image.display()
    );
    let mut harness = QemuHarness::new(kernel_image, &cobalt_image)?;

    println!("[xtask] [TEST 1/10] e2e_boot_banner...");
    harness
        .expect_output("metalOS v0.1.0 booting...", Duration::from_secs(10))
        .context("e2e_boot_banner failed")?;
    println!("[xtask] [PASS] e2e_boot_banner");

    println!("[xtask] [TEST 2/10] e2e_memory_init...");
    harness
        .expect_output(
            "[mm] Frame allocator and 4 MiB kernel heap online.",
            Duration::from_secs(10),
        )
        .context("e2e_memory_init failed")?;
    harness
        .expect_output(
            "[splash] Liquid Metal Drip animation active",
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
            "[mercuryd] Mercury display server online",
            Duration::from_secs(10),
        )
        .context("e2e_mercury_init failed")?;
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
            "[ai] Pluggable AI runtime online (active=gemini, shell-access=disabled).",
            Duration::from_secs(10),
        )
        .context("e2e_ai_init failed")?;
    println!("[xtask] [PASS] e2e_memory_init + CobaltFS + Splash + Net + Mercury + JS + MCP + AI");

    println!("[xtask] [TEST 3/10] e2e_shell_prompt...");
    harness
        .expect_output("metalos:/$", Duration::from_secs(10))
        .context("e2e_shell_prompt failed")?;
    println!("[xtask] [PASS] e2e_shell_prompt");

    println!("[xtask] [TEST 4/10] e2e_uname...");
    harness.send_command("uname -a");
    harness
        .expect_output("metalOS metalos 0.1.0", Duration::from_secs(5))
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
        .expect_output("metalos:/dev$", Duration::from_secs(5))
        .context("e2e_cd_dev_prompt failed")?;
    harness.send_command("pwd && cd ..");
    harness
        .expect_output("/dev\nmetalos:/$", Duration::from_secs(5))
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

    println!("[xtask] All 10 E2E tests PASSED!");
    Ok(())
}

fn run_ci() -> Result<()> {
    let workspace_root = get_workspace_root()?;
    println!("[xtask] Step 1: Running cargo test --workspace...");
    let status = Command::new("cargo")
        .current_dir(&workspace_root)
        .arg("test")
        .arg("--workspace")
        .status()
        .context("Failed to run cargo test --workspace")?;

    if !status.success() {
        return Err(anyhow!("cargo test --workspace failed"));
    }

    println!("[xtask] Step 2: Building metalOS (aarch64) and executing E2E tests...");
    let kernel_image = build_crates(false)?;
    run_e2e_tests(&kernel_image, Duration::from_secs(60))?;
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Build { release } => {
            let image = build_crates(release)?;
            println!("[xtask] Kernel built: {}", image.display());
            Ok(())
        }
        Commands::Run { release } => run_qemu(release),
        Commands::Test {
            timeout,
            release,
            disk_image,
        } => {
            let image = match disk_image {
                Some(path) => path,
                None => build_crates(release)?,
            };
            run_e2e_tests(&image, Duration::from_secs(timeout))
        }
        Commands::Ci => run_ci(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_build_command() {
        let args = Cli::parse_from(["xtask", "build", "--release"]);
        assert_eq!(args.command, Commands::Build { release: true });

        let args_debug = Cli::parse_from(["xtask", "build"]);
        assert_eq!(args_debug.command, Commands::Build { release: false });
    }

    #[test]
    fn parse_run_command() {
        let args = Cli::parse_from(["xtask", "run"]);
        assert_eq!(args.command, Commands::Run { release: false });
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
    fn parse_ci_command() {
        let args = Cli::parse_from(["xtask", "ci"]);
        assert_eq!(args.command, Commands::Ci);
    }

    #[test]
    fn disk_image_path_layout() {
        let root = Path::new("/tmp/test_ws");
        let debug_img = get_disk_image_path(root, false);
        assert_eq!(
            debug_img,
            Path::new("/tmp/test_ws/target/aarch64-unknown-none/debug/metalos-kernel")
        );
        let release_img = get_disk_image_path(root, true);
        assert_eq!(
            release_img,
            Path::new("/tmp/test_ws/target/aarch64-unknown-none/release/metalos-kernel")
        );
    }
}
