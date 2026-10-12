use std::io::{BufRead, BufReader, Write};
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
    /// Builds kernel, userland crates, and the bootable BIOS image
    Build {
        #[arg(long)]
        release: bool,
    },
    /// Builds then launches metalOS in QEMU
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

/// Harness for controlling and testing a headless QEMU virtual machine.
pub struct QemuHarness {
    child: Child,
    stdin: ChildStdin,
    output_buffer: Arc<Mutex<String>>,
    _reader_thread: JoinHandle<()>,
}

impl QemuHarness {
    /// Spawns a QEMU instance with stdio connected to serial port.
    pub fn new(disk_image: &Path) -> Result<Self> {
        let mut child = Command::new("qemu-system-x86_64")
            .arg("-drive")
            .arg(format!("format=raw,file={}", disk_image.display()))
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
            .with_context(|| format!("Failed to spawn qemu-system-x86_64 for image {}", disk_image.display()))?;

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
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            while let Ok(n) = reader.read_line(&mut line) {
                if n == 0 {
                    break;
                }
                let mut lock = buffer_clone.lock().unwrap();
                lock.push_str(&line);
                line.clear();
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
        .join("x86_64-unknown-none")
        .join(profile)
        .join("metalos-bios.img")
}

fn build_crates(release: bool) -> Result<PathBuf> {
    let workspace_root = get_workspace_root()?;

    println!("[xtask] Building kernel crate for x86_64-unknown-none...");
    let mut kernel_cmd = Command::new("cargo");
    kernel_cmd
        .current_dir(&workspace_root)
        .arg("build")
        .arg("--package")
        .arg("metalos-kernel")
        .arg("--target")
        .arg("x86_64-unknown-none");
    if release {
        kernel_cmd.arg("--release");
    }
    let status = kernel_cmd.status().context("Failed to run cargo build for kernel")?;
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
    let status = user_cmd.status().context("Failed to run cargo build for user")?;
    if !status.success() {
        return Err(anyhow!("Userland compilation failed with status {}", status));
    }

    let profile = if release { "release" } else { "debug" };
    let kernel_bin = workspace_root
        .join("target")
        .join("x86_64-unknown-none")
        .join(profile)
        .join("metalos-kernel");

    let disk_image = get_disk_image_path(&workspace_root, release);

    if kernel_bin.exists() {
        println!("[xtask] Generating bootable BIOS disk image: {}", disk_image.display());
        let bios_boot = bootloader::BiosBoot::new(&kernel_bin);
        bios_boot.create_disk_image(&disk_image)
            .context("Failed to create BIOS disk image with bootloader")?;
        println!("[xtask] Successfully created BIOS disk image!");
    } else {
        return Err(anyhow!("Kernel binary does not exist at {}", kernel_bin.display()));
    }

    Ok(disk_image)
}

fn run_qemu(release: bool) -> Result<()> {
    let disk_image = build_crates(release)?;
    println!("[xtask] Launching QEMU with image {}...", disk_image.display());
    let status = Command::new("qemu-system-x86_64")
        .arg("-drive")
        .arg(format!("format=raw,file={}", disk_image.display()))
        .arg("-serial")
        .arg("stdio")
        .arg("-display")
        .arg("none")
        .arg("-m")
        .arg("256M")
        .arg("-no-reboot")
        .status()
        .context("Failed to execute qemu-system-x86_64")?;

    if !status.success() {
        return Err(anyhow!("QEMU exited with error: {:?}", status.code()));
    }
    Ok(())
}

fn run_e2e_tests(disk_image: &Path, _overall_timeout: Duration) -> Result<()> {
    println!("[xtask] Starting QEMU E2E test harness for {}...", disk_image.display());
    let mut harness = QemuHarness::new(disk_image)?;

    println!("[xtask] [TEST 1/4] e2e_boot_banner...");
    harness.expect_output("metalOS", Duration::from_secs(10))
        .context("e2e_boot_banner failed")?;
    println!("[xtask] [PASS] e2e_boot_banner");

    println!("[xtask] [TEST 2/4] e2e_shell_prompt...");
    harness.expect_output("metalos:/$", Duration::from_secs(10))
        .context("e2e_shell_prompt failed")?;
    println!("[xtask] [PASS] e2e_shell_prompt");

    println!("[xtask] [TEST 3/4] e2e_uname...");
    harness.send_command("uname -a");
    harness.expect_output("x86_64", Duration::from_secs(5))
        .context("e2e_uname failed")?;
    println!("[xtask] [PASS] e2e_uname");

    println!("[xtask] [TEST 4/4] e2e_shutdown...");
    harness.send_command("shutdown");
    let exit_code = harness.wait_for_exit(Duration::from_secs(5))
        .context("e2e_shutdown failed")?;
    if exit_code != 0 {
        return Err(anyhow!("QEMU shutdown exited with code {}", exit_code));
    }
    println!("[xtask] [PASS] e2e_shutdown");

    println!("[xtask] All E2E tests PASSED!");
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

    println!("[xtask] Step 2: Building metalOS and executing E2E tests...");
    let disk_image = build_crates(false)?;
    run_e2e_tests(&disk_image, Duration::from_secs(60))?;
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Build { release } => {
            let image = build_crates(release)?;
            println!("[xtask] Image built: {}", image.display());
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
            Path::new("/tmp/test_ws/target/x86_64-unknown-none/debug/metalos-bios.img")
        );
        let release_img = get_disk_image_path(root, true);
        assert_eq!(
            release_img,
            Path::new("/tmp/test_ws/target/x86_64-unknown-none/release/metalos-bios.img")
        );
    }
}
