//! Persist crash records without storing private UART HTTP envelopes.
use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
};

pub fn record(line: &str) {
    if !line.starts_with("[crash] ") || line.len() > 16 * 1024 {
        return;
    }
    let path = std::env::var_os("ALLOYOS_CRASH_LOG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/alloyos-crash.log"));
    if append(&path, line).is_err() {
        eprintln!("[crash-log] could not persist crash record");
    }
}

fn append(path: &Path, line: &str) -> std::io::Result<()> {
    if !line.starts_with("[crash] ") || line.len() > 16 * 1024 {
        return Ok(());
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(line.as_bytes())?;
    if !line.ends_with('\n') {
        file.write_all(b"\n")?;
    }
    file.flush()
}

#[cfg(test)]
mod tests {
    #[test]
    fn persists_crash_records_without_transport_secrets() {
        let path = std::env::temp_dir().join(format!(
            "alloyos-crash-test-{}-{}.log",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        super::append(&path, "FORGEHTTP1\t1\tpost\tsecret\n").unwrap();
        super::append(&path, "MCPHTTP1\t1\tget\tsecret\n").unwrap();
        assert!(!path.exists());
        super::append(&path, "[crash] kernel-panic example\n").unwrap();
        super::append(&path, "[crash] frame=0 pc=0x1234 kind=return").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[crash] kernel-panic example\n[crash] frame=0 pc=0x1234 kind=return\n"
        );
        std::fs::remove_file(path).unwrap();
    }
}
