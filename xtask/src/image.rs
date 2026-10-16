//! Copy a filesystem tree into an image before boot. No package catalog is compiled into the OS.
use alloyos_fs::{
    block::{BlockDevice, BlockError},
    cobaltfs::CobaltFS,
    vfs::{FileSystem, InodeKind},
};
use anyhow::{anyhow, Context, Result};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    sync::{Arc, Mutex},
};

struct ImageDevice {
    file: Mutex<File>,
    sectors: u64,
}
impl BlockDevice for ImageDevice {
    fn block_size(&self) -> usize {
        512
    }
    fn block_count(&self) -> u64 {
        self.sectors
    }
    fn read_block(&self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        if buf.len() != 512 {
            return Err(BlockError::InvalidBufferSize);
        }
        if lba >= self.sectors {
            return Err(BlockError::OutOfBounds);
        }
        let mut file = self.file.lock().map_err(|_| BlockError::IoError)?;
        file.seek(SeekFrom::Start(lba * 512))
            .and_then(|_| file.read_exact(buf))
            .map_err(|_| BlockError::IoError)
    }
    fn write_block(&self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        if buf.len() != 512 {
            return Err(BlockError::InvalidBufferSize);
        }
        if lba >= self.sectors {
            return Err(BlockError::OutOfBounds);
        }
        let mut file = self.file.lock().map_err(|_| BlockError::IoError)?;
        file.seek(SeekFrom::Start(lba * 512))
            .and_then(|_| file.write_all(buf))
            .map_err(|_| BlockError::IoError)
    }
}

pub fn copy_tree(image: &Path, source: &Path, destination: &str) -> Result<()> {
    let file = OpenOptions::new().read(true).write(true).open(image)?;
    let sectors = file.metadata()?.len() / 512;
    let device = Arc::new(ImageDevice {
        file: Mutex::new(file),
        sectors,
    });
    let filesystem = CobaltFS::mount(device.clone())
        .map_err(|error| anyhow!("cannot mount image: {error:?}"))?;
    copy_directory(&filesystem, source, destination)?;
    device.file.lock().unwrap().sync_all()?;
    Ok(())
}

fn copy_directory(filesystem: &CobaltFS, source: &Path, destination: &str) -> Result<()> {
    if filesystem.lookup(destination).is_err() {
        filesystem
            .create(destination, InodeKind::Directory)
            .map_err(|error| anyhow!("cannot create {destination}: {error:?}"))?;
    }
    let mut entries: Vec<_> = fs::read_dir(source)
        .with_context(|| format!("cannot read {}", source.display()))?
        .collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let kind = entry.file_type()?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow!("image filenames must be UTF-8"))?;
        let target = format!("{destination}/{name}");
        if kind.is_dir() {
            copy_directory(filesystem, &entry.path(), &target)?;
        } else if kind.is_file() {
            let mut bytes = fs::read(entry.path())?;
            if target == "/apps/search/main.js" {
                let text = String::from_utf8(bytes).context("Search source must be UTF-8")?;
                let key = std::env::var("GEMINI_API_KEY").ok()
                    .filter(|key| !key.trim().is_empty()).or_else(|| {
                        let entry_path = entry.path();
                        let root = entry_path.ancestors().find(|path|
                            path.join("Cargo.toml").is_file() && path.join("xtask").is_dir())?;
                        fs::read_to_string(root.join(".secrets/GEMINI_API_KEY")).ok()
                    }).unwrap_or_default();
                let key = key.trim().strip_prefix("GEMINI_API_KEY=").unwrap_or(key.trim()).trim();
                bytes = inject_search_key(&text, key).into_bytes();
            }
            if let Ok(inode) = filesystem.lookup(&target) {
                let mut existing = vec![0; inode.size];
                if inode.kind == InodeKind::File
                    && filesystem.read(inode.ino, 0, &mut existing).ok() == Some(existing.len())
                    && existing == bytes
                {
                    continue;
                }
                filesystem
                    .unlink(&target)
                    .map_err(|error| anyhow!("cannot replace {target}: {error:?}"))?;
            }
            let inode = filesystem
                .create(&target, InodeKind::File)
                .map_err(|error| anyhow!("cannot create {target}: {error:?}"))?;
            if !bytes.is_empty() {
                let count = filesystem
                    .write(inode.ino, 0, &bytes)
                    .map_err(|error| anyhow!("cannot copy {target}: {error:?}"))?;
                if count != bytes.len() {
                    return Err(anyhow!("incomplete image copy: {target}"));
                }
            }
        } else {
            return Err(anyhow!(
                "unsupported image source entry: {}",
                entry.path().display()
            ));
        }
    }
    Ok(())
}

// Replace only the declaration, leaving the runtime placeholder check intact.
// Escape the value as a JS string so credentials cannot alter the app source.
fn inject_search_key(source: &str, key: &str) -> String {
    let mut literal = String::from("\"");
    for character in key.chars() {
        match character {
            '\\' => literal.push_str("\\\\"),
            '"' => literal.push_str("\\\""),
            character if character.is_control() || matches!(character, '\u{2028}' | '\u{2029}') => {
                literal.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => literal.push(character),
        }
    }
    literal.push('"');
    source.replacen("\"__GEMINI_API_KEY__\"", &literal, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_key_injection_escapes_and_preserves_runtime_guard() {
        let source = include_str!("../../apps/search/main.js");
        let injected = inject_search_key(source, "fixture\"\\\nkey");
        assert!(injected.contains(r#"const API_KEY = "fixture\"\\\u000akey";"#));
        assert!(injected.contains(r#"API_KEY === "__GEMINI_API_KEY__""#));
        assert!(inject_search_key(source, "").contains(r#"const API_KEY = "";"#));
    }

    #[test]
    fn migration_preserves_empty_dirs_and_rolls_back_failed_process_packaging() {
        let root = std::env::temp_dir().join(format!("alloyos-migration-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("source/empty")).unwrap();
        fs::write(root.join("source/state.bin"), b"preserved").unwrap();
        let image = root.join("volume.img");
        crate::ensure_cobaltfs_image(&image).unwrap();
        copy_tree(&image, &root.join("source"), "/var").unwrap();
        // Simulate an older smaller image while retaining its allocated blocks.
        OpenOptions::new()
            .write(true)
            .open(&image)
            .unwrap()
            .set_len(31 * 1024 * 1024)
            .unwrap();
        let original = fs::read(&image).unwrap();
        // A completed older upgrade's backup must not block the next upgrade
        // or be overwritten by its new rollback image.
        fs::write(image.with_extension("pre-process-backup.img"), b"older retained backup").unwrap();
        crate::ensure_cobaltfs_image(&image).unwrap();
        let archived = fs::read_dir(&root).unwrap().map(|entry| entry.unwrap().path())
            .find(|path| path.file_name().unwrap().to_string_lossy().starts_with("volume.pre-process-backup-"))
            .expect("previous backup is retained");
        assert_eq!(fs::read(archived).unwrap(), b"older retained backup");
        assert_eq!(fs::read(image.with_extension("pre-process-backup.img")).unwrap(), original);
        assert_eq!(fs::metadata(&image).unwrap().len(), 128 * 1024 * 1024);
        let export = root.join("export");
        export_tree(&image, &export).unwrap();
        assert!(export.join("var/empty").is_dir());
        assert_eq!(
            fs::read(export.join("var/state.bin")).unwrap(),
            b"preserved"
        );
        assert!(export_tree(&image, &export).is_err());
        // Missing executable artifacts must restore the exact original image.
        assert!(crate::package_processes(&root, false, &image).is_err());
        assert_eq!(fs::read(&image).unwrap(), original);
        assert!(!image.with_extension("migration-pending").exists());
        // Retry does not collide with stale staging data.
        crate::ensure_cobaltfs_image(&image).unwrap();
        assert_eq!(fs::metadata(&image).unwrap().len(), 128 * 1024 * 1024);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn image_contains_arbitrary_files_before_boot_and_preserves_other_data() {
        let root =
            std::env::temp_dir().join(format!("alloyos-filesystem-image-{}", std::process::id()));
        fs::create_dir_all(root.join("source/arbitrary/assets/empty")).unwrap();
        fs::write(
            root.join("source/arbitrary/arbitrary.mcp"),
            b"name=Fixture\nversion=1\ncapabilities=ExposeMcpTools\n",
        )
        .unwrap();
        fs::write(
            root.join("source/arbitrary/main.js"),
            br#"function callTool(name,args) { return JSON.stringify({ value: 73 }); }"#,
        )
        .unwrap();
        // Cross the direct-block boundary, and include non-UTF-8 bytes.
        let binary = vec![0xfe; 20_000];
        fs::write(root.join("source/arbitrary/assets/data.bin"), &binary).unwrap();
        let image = root.join("volume.img");
        crate::ensure_cobaltfs_image(&image).unwrap();
        copy_tree(&image, &root.join("source"), "/apps").unwrap();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&image)
            .unwrap();
        let sectors = file.metadata().unwrap().len() / 512;
        let filesystem = CobaltFS::mount(Arc::new(ImageDevice {
            file: Mutex::new(file),
            sectors,
        }))
        .unwrap();
        assert!(filesystem.lookup("/apps/arbitrary/assets/empty").is_ok());
        let inode = filesystem
            .lookup("/apps/arbitrary/assets/data.bin")
            .unwrap();
        let mut actual = vec![0; inode.size];
        assert_eq!(
            filesystem.read(inode.ino, 0, &mut actual).unwrap(),
            binary.len()
        );
        assert_eq!(actual, binary);
        filesystem.create("/var", InodeKind::Directory).unwrap();
        let state = filesystem
            .create("/var/state.txt", InodeKind::File)
            .unwrap();
        filesystem.write(state.ino, 0, b"keep").unwrap();
        drop(filesystem);
        fs::write(root.join("source/arbitrary/main.js"), b"updated").unwrap();
        copy_tree(&image, &root.join("source"), "/apps").unwrap();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&image)
            .unwrap();
        let sectors = file.metadata().unwrap().len() / 512;
        let filesystem = CobaltFS::mount(Arc::new(ImageDevice {
            file: Mutex::new(file),
            sectors,
        }))
        .unwrap();
        let inode = filesystem.lookup("/apps/arbitrary/main.js").unwrap();
        assert_eq!(inode.size, 7);
        let mut bytes = [0; 4];
        let state = filesystem.lookup("/var/state.txt").unwrap();
        filesystem.read(state.ino, 0, &mut bytes).unwrap();
        assert_eq!(&bytes, b"keep");
        drop(filesystem);
        fs::remove_dir_all(root).unwrap();
    }
}

/// Preserve a development volume in a fresh destination; never merge stale exports.
pub fn export_tree(image: &Path, destination: &Path) -> Result<()> {
    if destination.exists() {
        return Err(anyhow!(
            "migration export already exists: {}",
            destination.display()
        ));
    }
    let file = OpenOptions::new().read(true).write(true).open(image)?;
    let sectors = file.metadata()?.len() / 512;
    let filesystem = CobaltFS::mount(Arc::new(ImageDevice {
        file: Mutex::new(file),
        sectors,
    }))
    .map_err(|e| anyhow!("mount for migration: {e:?}"))?;
    fn visit(filesystem: &CobaltFS, path: &str, out: &Path) -> Result<()> {
        fs::create_dir(out)?;
        for name in filesystem
            .readdir(filesystem.lookup(path).map_err(|e| anyhow!("{e:?}"))?.ino)
            .map_err(|e| anyhow!("{e:?}"))?
        {
            if name == "." || name == ".." {
                continue;
            }
            if name.contains('/') || name.contains('\\') {
                return Err(anyhow!("invalid migration filename"));
            }
            let target = format!("{}/{}", path.trim_end_matches('/'), name);
            let inode = filesystem.lookup(&target).map_err(|e| anyhow!("{e:?}"))?;
            match inode.kind {
                InodeKind::Directory => visit(filesystem, &target, &out.join(name))?,
                InodeKind::File => {
                    let mut bytes = vec![0; inode.size];
                    let count = filesystem
                        .read(inode.ino, 0, &mut bytes)
                        .map_err(|e| anyhow!("cannot export {target}: {e:?}"))?;
                    if count != bytes.len() {
                        return Err(anyhow!("short migration read: {target}"));
                    }
                    fs::write(out.join(name), bytes)?;
                }
                _ => return Err(anyhow!("unsupported migration inode: {target}")),
            }
        }
        Ok(())
    }
    visit(&filesystem, "/", destination)
}
