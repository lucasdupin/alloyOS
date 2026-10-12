use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::Mutex;
use crate::fs::vfs::{FileSystem, FsError, Inode, InodeKind};

struct RamNode {
    ino: u64,
    kind: InodeKind,
    data: Vec<u8>,
}

pub struct RamFS {
    nodes: Mutex<BTreeMap<String, RamNode>>,
    ino_map: Mutex<BTreeMap<u64, String>>,
    next_ino: Mutex<u64>,
}

impl RamFS {
    pub fn new() -> Self {
        let mut nodes = BTreeMap::new();
        let mut ino_map = BTreeMap::new();

        // Root directory '/'
        nodes.insert(
            String::from("/"),
            RamNode {
                ino: 1,
                kind: InodeKind::Directory,
                data: Vec::new(),
            },
        );
        ino_map.insert(1, String::from("/"));

        Self {
            nodes: Mutex::new(nodes),
            ino_map: Mutex::new(ino_map),
            next_ino: Mutex::new(2),
        }
    }

    fn normalize_path(path: &str) -> String {
        let trimmed = path.trim();
        if trimmed.is_empty() || trimmed == "/" {
            return String::from("/");
        }

        let mut parts = Vec::new();
        for segment in trimmed.split('/') {
            if segment.is_empty() || segment == "." {
                continue;
            }
            if segment == ".." {
                parts.pop();
            } else {
                parts.push(segment);
            }
        }

        let mut res = String::new();
        for part in parts {
            res.push('/');
            res.push_str(part);
        }

        if res.is_empty() {
            String::from("/")
        } else {
            res
        }
    }
}

impl FileSystem for RamFS {
    fn lookup(&self, path: &str) -> Result<Inode, FsError> {
        let norm = Self::normalize_path(path);
        let nodes = self.nodes.lock();
        let node = nodes.get(&norm).ok_or(FsError::NotFound)?;
        Ok(Inode {
            ino: node.ino,
            kind: node.kind,
            size: node.data.len(),
        })
    }

    fn create(&self, path: &str, kind: InodeKind) -> Result<Inode, FsError> {
        let norm = Self::normalize_path(path);
        let mut nodes = self.nodes.lock();
        if nodes.contains_key(&norm) {
            return Err(FsError::AlreadyExists);
        }

        // Ensure parent directories exist
        if let Some(parent_idx) = norm.rfind('/') {
            let parent_path = if parent_idx == 0 {
                "/"
            } else {
                &norm[..parent_idx]
            };
            if !nodes.contains_key(parent_path) {
                // Auto create parent directory if needed
                let mut next_ino = self.next_ino.lock();
                let parent_ino = *next_ino;
                *next_ino += 1;
                nodes.insert(
                    parent_path.to_string(),
                    RamNode {
                        ino: parent_ino,
                        kind: InodeKind::Directory,
                        data: Vec::new(),
                    },
                );
                self.ino_map.lock().insert(parent_ino, parent_path.to_string());
            }
        }

        let mut next_ino = self.next_ino.lock();
        let ino = *next_ino;
        *next_ino += 1;

        nodes.insert(
            norm.clone(),
            RamNode {
                ino,
                kind,
                data: Vec::new(),
            },
        );
        self.ino_map.lock().insert(ino, norm);

        Ok(Inode { ino, kind, size: 0 })
    }

    fn read(&self, ino: u64, offset: usize, buf: &mut [u8]) -> Result<usize, FsError> {
        let ino_map = self.ino_map.lock();
        let path = ino_map.get(&ino).ok_or(FsError::NotFound)?;

        let nodes = self.nodes.lock();
        let node = nodes.get(path).ok_or(FsError::NotFound)?;
        if node.kind == InodeKind::Directory {
            return Err(FsError::IsADirectory);
        }

        if offset >= node.data.len() {
            return Ok(0);
        }

        let available = node.data.len() - offset;
        let to_read = core::cmp::min(buf.len(), available);
        buf[..to_read].copy_from_slice(&node.data[offset..offset + to_read]);
        Ok(to_read)
    }

    fn write(&self, ino: u64, offset: usize, buf: &[u8]) -> Result<usize, FsError> {
        let ino_map = self.ino_map.lock();
        let path = ino_map.get(&ino).ok_or(FsError::NotFound)?;

        let mut nodes = self.nodes.lock();
        let node = nodes.get_mut(path).ok_or(FsError::NotFound)?;
        if node.kind == InodeKind::Directory {
            return Err(FsError::IsADirectory);
        }

        let required_len = offset + buf.len();
        if required_len > node.data.len() {
            node.data.resize(required_len, 0);
        }

        node.data[offset..offset + buf.len()].copy_from_slice(buf);
        Ok(buf.len())
    }

    fn readdir(&self, ino: u64) -> Result<Vec<String>, FsError> {
        let ino_map = self.ino_map.lock();
        let path = ino_map.get(&ino).ok_or(FsError::NotFound)?;

        let nodes = self.nodes.lock();
        let node = nodes.get(path).ok_or(FsError::NotFound)?;
        if node.kind != InodeKind::Directory {
            return Err(FsError::NotADirectory);
        }

        let mut entries = Vec::new();
        let prefix = if path == "/" {
            String::from("/")
        } else {
            alloc::format!("{}/", path)
        };

        for (item_path, _) in nodes.iter() {
            if item_path != path && item_path.starts_with(&prefix) {
                let rest = &item_path[prefix.len()..];
                if !rest.contains('/') {
                    entries.push(rest.to_string());
                }
            }
        }

        Ok(entries)
    }

    fn unlink(&self, path: &str) -> Result<(), FsError> {
        let norm = Self::normalize_path(path);
        let mut nodes = self.nodes.lock();
        let node = nodes.remove(&norm).ok_or(FsError::NotFound)?;
        self.ino_map.lock().remove(&node.ino);
        Ok(())
    }
}
