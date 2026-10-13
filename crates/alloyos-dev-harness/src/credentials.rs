//! Local-only credential references and transient QEMU fw_cfg transport.
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
const LIMIT: usize = 16384;

/// Contains only a private path. Deliberately does not implement Debug.
pub struct BootCredentials {
    directory: PathBuf,
}
impl Drop for BootCredentials {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

/// Files contain NAME=VALUE lines; argv carries references, never values.
pub fn attach(
    command: &mut Command,
    env_names: &[String],
    files: &[PathBuf],
) -> Result<Option<BootCredentials>, String> {
    // QEMU receives only the private fw_cfg payload, never inherited provider keys.
    for name in [
        "ANTHROPIC_API_KEY",
        "GEMINI_API_KEY",
        "OPENAI_API_KEY",
        "HF_TOKEN",
    ] {
        command.env_remove(name);
    }
    let mut entries = Vec::new();
    for name in env_names {
        let value = std::env::var(name)
            .map_err(|_| "credential environment reference unavailable".to_string())?;
        entries.push((name.clone(), value));
    }
    for path in files {
        #[cfg(unix)]
        if fs::metadata(path)
            .map_err(|_| "credential file unavailable")?
            .permissions()
            .mode()
            & 0o077
            != 0
        {
            return Err("credential file must have private permissions (chmod 600)".into());
        }
        let data = fs::read(path).map_err(|_| "credential file unavailable")?;
        if data.len() > LIMIT {
            return Err("credential file too large".into());
        }
        let data = String::from_utf8(data).map_err(|_| "credential file must be UTF-8")?;
        let basename = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if [
            "ANTHROPIC_API_KEY",
            "GEMINI_API_KEY",
            "OPENAI_API_KEY",
            "HF_TOKEN",
        ]
        .contains(&basename)
            && !data.trim().starts_with(&format!("{basename}="))
        {
            entries.push((basename.into(), data.trim().into()));
            continue;
        }
        for line in data.lines().filter(|s| !s.is_empty()) {
            let (name, value) = line
                .split_once('=')
                .ok_or("credential file requires NAME=VALUE lines")?;
            entries.push((name.into(), value.into()));
        }
    }
    if entries.is_empty() {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    for (name, mut value) in entries {
        if name.is_empty()
            || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            || value.is_empty()
            || value.bytes().any(|b| matches!(b, 0 | b'\n' | b'\r'))
        {
            return Err("invalid credential entry".into());
        }
        bytes.extend_from_slice(name.as_bytes());
        bytes.push(b'=');
        bytes.extend_from_slice(value.as_bytes());
        bytes.push(0);
        // Best effort scrub temporary copies; fw_cfg keeps its volatile guest copy.
        unsafe {
            value.as_bytes_mut().fill(0);
        }
        if bytes.len() > LIMIT {
            return Err("credential payload too large".into());
        }
    }
    let directory = std::env::temp_dir().join(format!(
        "alloyos-auth-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    builder.mode(0o700);
    builder
        .create(&directory)
        .map_err(|_| "cannot create private credential directory")?;
    let guard = BootCredentials { directory };
    let path = guard.directory.join("boot-auth");
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    use std::io::Write;
    let result = options.open(&path).and_then(|mut f| f.write_all(&bytes));
    bytes.fill(0);
    result.map_err(|_| "cannot write private boot credentials")?;
    if path.to_string_lossy().contains(',') {
        return Err("unsupported private directory path".into());
    }
    command
        .arg("-fw_cfg")
        .arg(format!("name=opt/alloyos/auth,file={}", path.display()));
    // Do not pass inherited credentials into QEMU's environment.
    for name in env_names {
        command.env_remove(name);
    }
    Ok(Some(guard))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arguments_are_redacted_and_file_removed() {
        let source = std::env::temp_dir().join(format!("alloyos-auth-test-{}", std::process::id()));
        fs::write(&source, b"TEST_TOKEN=fixture-secret\n").unwrap();
        #[cfg(unix)]
        fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
        let mut command = Command::new("qemu");
        let guard = attach(&mut command, &[], std::slice::from_ref(&source))
            .unwrap()
            .unwrap();
        let args = format!("{:?}", command);
        assert!(!args.contains("fixture-secret"));
        let path = guard.directory.join("boot-auth");
        assert_eq!(fs::read(&path).unwrap(), b"TEST_TOKEN=fixture-secret\0");
        drop(guard);
        assert!(!path.exists());
        fs::remove_file(source).unwrap();
    }
    #[test]
    fn setup_key_files_reach_fw_cfg_without_entering_argv() {
        let source =
            std::env::temp_dir().join(format!("alloyos-api-file-test-{}", std::process::id()));
        fs::create_dir_all(&source).unwrap();
        let path = source.join("OPENAI_API_KEY");
        fs::write(&path, b"fixture-secret=value\n").unwrap();
        #[cfg(unix)]
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let mut command = Command::new("qemu");
        let guard = attach(&mut command, &[], &[path]).unwrap().unwrap();
        assert_eq!(
            fs::read(guard.directory.join("boot-auth")).unwrap(),
            b"OPENAI_API_KEY=fixture-secret=value\0"
        );
        assert!(!format!("{:?}", command).contains("fixture-secret"));
        assert!(command
            .get_envs()
            .any(|(name, value)| name == "OPENAI_API_KEY" && value.is_none()));
        drop(guard);
        fs::remove_dir_all(source).unwrap();
    }

    #[test]
    fn empty_credentials_need_no_channel() {
        assert!(attach(&mut Command::new("qemu"), &[], &[])
            .unwrap()
            .is_none());
    }
}
