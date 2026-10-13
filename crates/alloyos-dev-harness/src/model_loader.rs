//! Attach the model as read-only storage so kernel boot never waits for weights.
use std::{io, path::Path, process::Command};
pub fn attach_disk(image: &Path, command: &mut Command) -> io::Result<()> {
    let image = image.canonicalize()?;
    let path = image.to_string_lossy().replace(',', ",,");
    command
        .arg("-drive")
        .arg(format!(
            "file={path},if=none,format=raw,readonly=on,id=forge-model"
        ))
        .args(["-device", "virtio-blk-device,drive=forge-model"]);
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn model_is_storage_and_never_a_preboot_rom() {
        let image = std::env::current_exe().unwrap();
        let mut command = Command::new("qemu-system-aarch64");
        attach_disk(&image, &mut command).unwrap();
        let args: Vec<_> = command.get_args().map(|s| s.to_string_lossy()).collect();
        assert!(args.iter().any(|arg| arg.contains("readonly=on")));
        assert!(args
            .iter()
            .all(|arg| !arg.contains("loader,") && !arg.contains("force-raw")));
    }
}
