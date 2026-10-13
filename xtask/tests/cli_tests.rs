use std::process::Command;

#[test]
fn xtask_help_output() {
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("--help")
        .output()
        .expect("Failed to execute xtask --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("alloyOS build, run, and test automation"));
    assert!(stdout.contains("build"));
    assert!(stdout.contains("run"));
    assert!(stdout.contains("test"));
    assert!(stdout.contains("ci"));
}
