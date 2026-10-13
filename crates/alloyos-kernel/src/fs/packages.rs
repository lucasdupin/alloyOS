//! Bounded recursive package snapshots taken while the VFS owner lock is held.
use super::vfs::{InodeKind, Vfs, VFS};
use alloc::{
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};
use alloyos_mcp::packages::{PackageChange, PackageService, PackageSnapshot};

const MAX_FILES: usize = 256;
const MAX_BYTES: usize = 2 * 1024 * 1024;

pub fn snapshot(vfs: &Vfs) -> Result<PackageSnapshot, String> {
    let mut result = PackageSnapshot::default();
    let mut bytes = 0;
    collect(vfs, "/apps", 0, &mut bytes, &mut result)?;
    Ok(result)
}

fn collect(
    vfs: &Vfs,
    path: &str,
    depth: usize,
    bytes: &mut usize,
    snapshot: &mut PackageSnapshot,
) -> Result<(), String> {
    if depth > 16 {
        return Err("package nesting exceeds limit".into());
    }
    for name in vfs
        .readdir(path)
        .map_err(|_| format!("cannot scan {}", path))?
    {
        let name = name.trim_end_matches('/');
        if name == "." || name == ".." {
            continue;
        }
        if name.is_empty() || name.contains('/') {
            return Err("invalid package entry".into());
        }
        let child = format!("{}/{}", path, name);
        let inode = vfs
            .lookup(&child)
            .map_err(|_| format!("cannot inspect {}", child))?;
        if inode.kind == InodeKind::Directory {
            if snapshot.directories.len() >= MAX_FILES {
                return Err("package directory count exceeds limit".into());
            }
            snapshot.directories.push(child.clone());
            collect(vfs, &child, depth + 1, bytes, snapshot)?;
        } else if inode.kind == InodeKind::File {
            if snapshot.files.len() >= MAX_FILES || inode.size > MAX_BYTES.saturating_sub(*bytes) {
                return Err("package snapshot exceeds limit".into());
            }
            let mut data = vec![0; inode.size];
            let read = vfs
                .root_fs
                .read(inode.ino, 0, &mut data)
                .map_err(|_| format!("cannot read {}", child))?;
            if read != inode.size {
                return Err(format!("incomplete package file {}", child));
            }
            *bytes += read;
            // Binary assets also participate in revision detection. Text source
            // files must remain UTF-8 for manifest/JavaScript validation.
            let value = String::from_utf8(data.clone()).unwrap_or_else(|_| {
                let mut encoded = String::from("binary:");
                for byte in data {
                    encoded.push_str(&format!("{:02x}", byte));
                }
                encoded
            });
            snapshot.files.insert(child, value);
        }
    }
    Ok(())
}

/// Require two equal snapshots separated by a quiet period before publishing.
/// Polling covers nested writes and package additions/removals without relying
/// on app cooperation. Failed reads preserve the last validated inventory.
#[derive(Default)]
pub struct PackageWatcher {
    pub service: PackageService,
    candidate: Option<PackageSnapshot>,
    settled_at: u64,
    next_poll: u64,
}
impl PackageWatcher {
    pub fn poll(&mut self, now_ms: u64) -> Result<Vec<PackageChange>, String> {
        if now_ms < self.next_poll {
            return Ok(Vec::new());
        }
        self.next_poll = now_ms.saturating_add(250);
        let guard = VFS.lock();
        let vfs = guard
            .as_ref()
            .ok_or_else(|| String::from("VFS unavailable"))?;
        let current = snapshot(vfs)?;
        drop(guard);
        self.observe(now_ms, current)
    }
    /// Readable by Welder through its existing native filesystem tools.
    pub fn publish_inventory(&self) -> Result<(), String> {
        use alloyos_welder::ToolVfsBackend;
        let inventory: Vec<_> = self
            .service
            .inventory()
            .into_iter()
            .map(|status| {
                let diagnostics: Vec<_> = status
                    .diagnostics
                    .iter()
                    .map(|d| serde_json::json!({"path": d.path, "message": d.message}))
                    .collect();
                serde_json::json!({"app_id": status.app_id, "version": status.installed_version,
                "revision": status.revision, "validated_revision": status.validated_revision,
                "ready": status.ready, "diagnostics": diagnostics})
            })
            .collect();
        super::automation::KernelVfsBackend::global().write_file(
            "/proc/app-packages.json",
            &serde_json::json!({"packages": inventory}).to_string(),
        )
    }
    fn observe(
        &mut self,
        now_ms: u64,
        current: PackageSnapshot,
    ) -> Result<Vec<PackageChange>, String> {
        if self.candidate.as_ref() != Some(&current) {
            self.candidate = Some(current);
            self.settled_at = now_ms;
            return Ok(Vec::new());
        }
        if now_ms.saturating_sub(self.settled_at) < PackageService::COALESCE_MS {
            return Ok(Vec::new());
        }
        Ok(self.service.apply_snapshot(current))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_includes_nested_changes_and_waits_for_settling() {
        let vfs = Vfs::new();
        vfs.create("/apps/demo", InodeKind::Directory).unwrap();
        vfs.create("/apps/demo/assets", InodeKind::Directory)
            .unwrap();
        let file = vfs
            .create("/apps/demo/assets/data", InodeKind::File)
            .unwrap();
        vfs.root_fs.write(file.ino, 0, b"one").unwrap();
        let first = snapshot(&vfs).unwrap();
        assert_eq!(first.files["/apps/demo/assets/data"], "one");
        let mut watcher = PackageWatcher::default();
        assert!(watcher.observe(0, first.clone()).unwrap().is_empty());
        assert!(watcher.observe(50, first.clone()).unwrap().is_empty());
        assert_eq!(watcher.observe(250, first).unwrap().len(), 2); // seeded Tasks plus demo
        vfs.root_fs.write(file.ino, 0, b"two").unwrap();
        let second = snapshot(&vfs).unwrap();
        assert!(watcher.observe(500, second.clone()).unwrap().is_empty());
        assert_eq!(watcher.observe(750, second).unwrap().len(), 1);
    }
}
