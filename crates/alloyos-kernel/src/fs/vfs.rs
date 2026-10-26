use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
use crate::fs::devfs::DevFS;
use crate::fs::ramfs::RamFS;

pub use alloyos_fs::vfs::{FileSystem, FsError, Inode, InodeKind};

#[derive(Debug, Clone)]
pub struct FileHandle {
    pub ino: u64,
    pub offset: usize,
    pub kind: InodeKind,
    pub path: String,
}

pub const MAX_FDS: usize = 256;

pub struct FileDescriptorTable {
    pub open_file_table: Vec<Option<FileHandle>>,
}

impl FileDescriptorTable {
    pub fn new() -> Self {
        let mut open_file_table = Vec::with_capacity(MAX_FDS);
        open_file_table.resize(MAX_FDS, None);
        let mut table = Self { open_file_table };

        // Pre-populate stdin (0), stdout (1), stderr (2) as serial CharDevice
        table.open_file_table[0] = Some(FileHandle {
            ino: crate::fs::devfs::INO_DEV_SERIAL,
            offset: 0,
            kind: InodeKind::CharDevice,
            path: String::from("/dev/serial"),
        });
        table.open_file_table[1] = Some(FileHandle {
            ino: crate::fs::devfs::INO_DEV_SERIAL,
            offset: 0,
            kind: InodeKind::CharDevice,
            path: String::from("/dev/serial"),
        });
        table.open_file_table[2] = Some(FileHandle {
            ino: crate::fs::devfs::INO_DEV_SERIAL,
            offset: 0,
            kind: InodeKind::CharDevice,
            path: String::from("/dev/serial"),
        });

        table
    }

    pub fn open(&mut self, handle: FileHandle) -> Result<usize, FsError> {
        for fd in 3..MAX_FDS {
            if self.open_file_table[fd].is_none() {
                self.open_file_table[fd] = Some(handle);
                return Ok(fd);
            }
        }
        Err(FsError::PermissionDenied)
    }

    pub fn close(&mut self, fd: usize) -> Result<(), FsError> {
        if fd >= MAX_FDS || self.open_file_table[fd].is_none() {
            return Err(FsError::BadFileDescriptor);
        }
        self.open_file_table[fd] = None;
        Ok(())
    }

    pub fn get(&self, fd: usize) -> Option<&FileHandle> {
        if fd < MAX_FDS {
            self.open_file_table[fd].as_ref()
        } else {
            None
        }
    }

    pub fn get_mut(&mut self, fd: usize) -> Option<&mut FileHandle> {
        if fd < MAX_FDS {
            self.open_file_table[fd].as_mut()
        } else {
            None
        }
    }
}

pub struct Vfs {
    pub root_fs: Arc<dyn FileSystem>,
    pub dev_fs: Arc<DevFS>,
    pub backend_name: &'static str,
}

impl Vfs {
    pub fn new() -> Self {
        Self::with_root(Arc::new(RamFS::new()), "RamFS")
    }

    pub fn with_root(root_fs: Arc<dyn FileSystem>, backend_name: &'static str) -> Self {
        let _ = root_fs.create("/bin", InodeKind::Directory);
        let _ = root_fs.create("/dev", InodeKind::Directory);
        let _ = root_fs.create("/proc", InodeKind::Directory);
        let _ = root_fs.create("/etc", InodeKind::Directory);
        let _ = root_fs.create("/var", InodeKind::Directory);
        let _ = root_fs.create("/tmp", InodeKind::Directory);
        let _ = root_fs.create("/scripts", InodeKind::Directory);
        let _ = root_fs.create("/apps", InodeKind::Directory);
        let _ = root_fs.create("/knowledge", InodeKind::Directory);
        let _ = root_fs.create("/knowledge/projects", InodeKind::Directory);
        let _ = root_fs.create("/etc/ssl", InodeKind::Directory);
        let _ = root_fs.create("/etc/ssl/certs", InodeKind::Directory);

        if root_fs.lookup("/etc/ssl/certs/ca-certificates.crt").is_err() {
            if let Ok(ino) = root_fs.create("/etc/ssl/certs/ca-certificates.crt", InodeKind::File) {
                let _ = root_fs.write(
                    ino.ino,
                    0,
                    CA_CERTIFICATES_BUNDLE,
                );
            }
        }

        if root_fs.lookup("/etc/motd").is_err() {
            if let Ok(ino) = root_fs.create("/etc/motd", InodeKind::File) {
                let _ = root_fs.write(
                    ino.ino,
                    0,
                    b"Welcome to alloyOS v0.1.0 (aarch64 POSIX-compliant Rust kernel)\n",
                );
            }
        }
        if root_fs.lookup("/etc/ai.conf").is_err() {
            if let Ok(ino) = root_fs.create("/etc/ai.conf", InodeKind::File) {
                let _ = root_fs.write(
                    ino.ino,
                    0,
                    b"provider=forge\nmodel=gemma-2-2b-it\n",
                );
            }
        }
        if root_fs.lookup("/etc/schedules.conf").is_err() {
            if let Ok(ino) = root_fs.create("/etc/schedules.conf", InodeKind::File) {
                let _ = root_fs.write(
                    ino.ino,
                    0,
                    b"# alloyOS local JavaScript schedule registry\n",
                );
            }
        }
        if root_fs.lookup("/bin/sh").is_err() {
            if let Ok(ino) = root_fs.create("/bin/sh", InodeKind::File) {
                let _ = root_fs.write(ino.ino, 0, b"#!/bin/sh\n");
            }
        }
        if root_fs.lookup("/scripts/heartbeat.js").is_err() {
            if let Ok(ino) = root_fs.create("/scripts/heartbeat.js", InodeKind::File) {
                let _ = root_fs.write(
                    ino.ino,
                    0,
                    b"export function main() { return AlloyOS.sys.status(); }\n",
                );
            }
        }



        let (total_f, used_f, free_f) = crate::mm::frame_allocator::memory_stats();
        let total_kb = if total_f > 0 { total_f * 4 } else { 262144 };
        let free_kb = if total_f > 0 { free_f * 4 } else { 258048 };
        let used_kb = used_f * 4;
        let info = alloc::format!(
            "MemTotal:       {:>8} kB\nMemFree:        {:>8} kB\nMemUsed:        {:>8} kB\n",
            total_kb,
            free_kb,
            used_kb
        );
        if let Ok(ino) = root_fs.lookup("/proc/meminfo").or_else(|_| root_fs.create("/proc/meminfo", InodeKind::File)) {
            let _ = root_fs.write(ino.ino, 0, info.as_bytes());
        }
        if let Ok(ino) = root_fs.lookup("/proc/uptime").or_else(|_| root_fs.create("/proc/uptime", InodeKind::File)) {
            let _ = root_fs.write(ino.ino, 0, b"1.25 1.10\n");
        }
        if let Ok(ino) = root_fs.lookup("/proc/cpuinfo").or_else(|_| root_fs.create("/proc/cpuinfo", InodeKind::File)) {
            let _ = root_fs.write(
                ino.ino,
                0,
                b"Processor\t: AArch64 Processor rev 0 (aarch64)\nFeatures\t: fp asimd evtstrm aes pmull sha1 sha2 crc32\n",
            );
        }
        if let Ok(ino) = root_fs.lookup("/proc/version").or_else(|_| root_fs.create("/proc/version", InodeKind::File)) {
            let _ = root_fs.write(ino.ino, 0, b"alloyOS version 0.1.0 (rustc nightly aarch64)\n");
        }

        Self {
            root_fs,
            dev_fs: Arc::new(DevFS::new()),
            backend_name,
        }
    }

    pub fn lookup(&self, path: &str) -> Result<Inode, FsError> {
        if private_account_path(path) { return Err(FsError::PermissionDenied); }
        let clean_path = path.trim();
        if clean_path.starts_with("/dev/") {
            self.dev_fs.lookup(clean_path)
        } else {
            self.root_fs.lookup(clean_path)
        }
    }

    pub fn create(&self, path: &str, kind: InodeKind) -> Result<Inode, FsError> {
        if private_account_path(path) { return Err(FsError::PermissionDenied); }
        let clean_path = path.trim();
        if clean_path.starts_with("/dev/") {
            self.dev_fs.create(clean_path, kind)
        } else {
            self.root_fs.create(clean_path, kind)
        }
    }

    pub fn readdir(&self, path: &str) -> Result<Vec<String>, FsError> {
        let clean_path = path.trim();
        if clean_path == "/dev" || clean_path == "/dev/" {
            return self.dev_fs.readdir(0);
        }
        let inode = self.lookup(clean_path)?;
        if inode.kind != InodeKind::Directory {
            return Err(FsError::NotADirectory);
        }
        let raw_entries = self.root_fs.readdir(inode.ino)?;
        let prefix = if clean_path == "/" {
            String::from("")
        } else {
            String::from(clean_path.trim_end_matches('/'))
        };

        let mut formatted = Vec::with_capacity(raw_entries.len());
        for name in raw_entries {
            let child_path = alloc::format!("{}/{}", prefix, name);
            if private_account_path(&child_path) { continue; }
            if let Ok(child_ino) = self.lookup(&child_path) {
                if child_ino.kind == InodeKind::Directory {
                    formatted.push(alloc::format!("{}/", name));
                    continue;
                }
            }
            formatted.push(name);
        }

        if clean_path == "/" {
            // Keep canonical root directories in standard order first
            let canonical = [
                "bin/",
                "dev/",
                "proc/",
                "etc/",
                "var/",
                "tmp/",
                "scripts/",
                "apps/",
                "knowledge/",
            ];
            let mut ordered = Vec::with_capacity(formatted.len());
            for &c in &canonical {
                if formatted.iter().any(|e| e == c) {
                    ordered.push(String::from(c));
                }
            }
            for e in formatted {
                if !canonical.contains(&e.as_str()) {
                    ordered.push(e);
                }
            }
            return Ok(ordered);
        }

        Ok(formatted)
    }

    pub fn read(&self, handle: &mut FileHandle, buf: &mut [u8]) -> Result<usize, FsError> {
        if handle.kind == InodeKind::Directory {
            let entries = self.readdir(&handle.path)?;
            let mut listing = String::new();
            for entry in entries {
                listing.push_str(&entry);
                listing.push('\n');
            }
            let bytes = listing.as_bytes();
            if handle.offset >= bytes.len() {
                return Ok(0);
            }
            let available = bytes.len() - handle.offset;
            let to_read = core::cmp::min(buf.len(), available);
            buf[..to_read].copy_from_slice(&bytes[handle.offset..handle.offset + to_read]);
            handle.offset += to_read;
            return Ok(to_read);
        }

        if handle.path.starts_with("/dev/") {
            let bytes = self.dev_fs.read(handle.ino, handle.offset, buf)?;
            handle.offset += bytes;
            Ok(bytes)
        } else {
            let bytes = self.root_fs.read(handle.ino, handle.offset, buf)?;
            handle.offset += bytes;
            Ok(bytes)
        }
    }

    pub fn write(&self, handle: &mut FileHandle, buf: &[u8]) -> Result<usize, FsError> {
        if handle.path.starts_with("/dev/") {
            let bytes = self.dev_fs.write(handle.ino, handle.offset, buf)?;
            handle.offset += bytes;
            Ok(bytes)
        } else {
            let bytes = self.root_fs.write(handle.ino, handle.offset, buf)?;
            handle.offset += bytes;
            Ok(bytes)
        }
    }

    pub fn unlink(&self, path: &str) -> Result<(), FsError> {
        if private_account_path(path) { return Err(FsError::PermissionDenied); }
        let clean_path = path.trim();
        if clean_path.starts_with("/dev/") {
            self.dev_fs.unlink(clean_path)
        } else {
            self.root_fs.unlink(clean_path)
        }
    }
}

pub static VFS: Mutex<Option<Vfs>> = Mutex::new(None);
pub static GLOBAL_FD_TABLE: Mutex<Option<FileDescriptorTable>> = Mutex::new(None);

pub fn init() -> &'static str {
    let mounted_vfs = try_mount_cobaltfs().unwrap_or_else(Vfs::new);
    let backend = mounted_vfs.backend_name;

    let mut vfs = Mutex::lock(&VFS);
    *vfs = Some(mounted_vfs);

    let mut fd_table = Mutex::lock(&GLOBAL_FD_TABLE);
    *fd_table = Some(FileDescriptorTable::new());

    backend
}

fn try_mount_cobaltfs() -> Option<Vfs> {
    #[cfg(target_os = "none")]
    {
        use crate::drivers::ata::AtaPio;
        use crate::fs::cobaltfs::CobaltFS;

        // Probe ATA Primary Slave (drive 1, attached data disk) first, then Primary Master (drive 0)
        for drive in [1u8, 0u8] {
            if let Some(ata) = AtaPio::probe(drive) {
                let dev: Arc<dyn crate::fs::block::BlockDevice> = Arc::new(ata);
                if let Ok(cobalt) = CobaltFS::mount(dev) {
                    return Some(Vfs::with_root(Arc::new(cobalt), "CobaltFS"));
                }
            }
        }
    }
    None
}

/// The trusted account adapter accesses root_fs directly; public VFS paths never
/// expose private records, even through normalized aliases or ancestor listings.
pub fn private_account_path(path: &str) -> bool {
    let path = alloyos_js::ScriptCapabilities::normalize_path(path);
    path == "/system/accounts" || path.starts_with("/system/accounts/")
}

/// Standard Mozilla Root Certificate Authority PEM bundle for TLS 1.3 verification
pub const CA_CERTIFICATES_BUNDLE: &[u8] = b"\
# Issuer: GTS Root R1\n\
-----BEGIN CERTIFICATE-----\n\
MIIFazCCA1OgAwIBAgIRAIIQz7DSQONZRGPgu2OCiwAwDQYJKoZIhvcNAQELBQAw\n\
TzELMAkGA1UEBhMCVVMxHjAcBgNVBAoTFUdvb2dsZSBUcnVzdCBTZXJ2aWNlczEM\n\
MAoGA1UECxMDZ3RyMRQwEgYDVQQDEwtHVFMgUm9vdCBSMTAeFw0xNjA2MjIwMDAw\n\
MDBaFw0zNjA2MjIwMDAwMDBaME8xCzAJBgNVBAYTAlVTMR4wHAYDVQQKExVHb29n\n\
bGUgVHJ1c3QgU2VydmljZXMxDDAKBgNVBAsTA2d0cjEUMBIGA1UEAxMLR1RTIFJv\n\
b3QgUjEwggIiMA0GCSqGSIb3DQEBAQUAA4ICDwAwggIKAoICAQC2EQ1nmMDaogoY\n\
OWBQeRmxR7EoqWg0dn9kJgi8ytSmGuQMr32bi10OrQEbHdlv54Kq8tx0/U6ljpab\n\
Ap9vglOoYhIlG5Q2h14OwFYunLfq6NVZZQ3B8pmflUQRFAz34evycAO0i45vwg++\n\
SvPUmbQB5AS1OnXrCUIt3ks4aTTMUaw+kulAQUDA+NyTHv++GLx61hWD81UyTurK\n\
LRZZF90DYN70xW+25guNWcCXs2Aq9VKnTIIaxSQgJYdBcgfOI+KU3zsZt83pYAxm\n\
BA2DA/37PWuViYGu0CgojwDQcCxN0KaAGGZBhnLQn6ZNwdZqAZ305UpwidddtlEb\n\
EQFibauJW9xJPLOo8uMY22g9Nd14vU06yY0WoQy260NU8Y5XOOMDSbZV95EDvFE6\n\
IqULaha5TowAssDL+kExNXKEyj8mPXYNFrK53YPFIItBoejpf5yA13kiII8CmiSW\n\
ak37CMY98Z0EBQC+E5jJGLOo6v521eeOpErZRZJZl8W3E4QlWCwWikWhSs9GpgMA\n\
Dg920HuvWiv7DLZHKWD44222e5OzxhjXYb94Bi7rRw5SFiN24i/tHNTnx7i2LVTL\n\
kxTuBmIXqW0bU50gwAqC1BN8b80KeSauKGAswnCAJAzCab5L4drLPO24X8s5ASGQ\n\
-----END CERTIFICATE-----\n\
\n\
# Issuer: DigiCert Global Root G2\n\
-----BEGIN CERTIFICATE-----\n\
MIIDjjCCAnagAwIBAgIQAzrx5qcRqaC7KGSxHQn55zANBgkqhkiG9w0BAQsFADBh\n\
MQswCQYDVQQGEwJVUzEVMBMGA1UEChMMRGlnaUNlcnQgSW5jMRkwFwYDVQQLExB3\n\
d3cuZGlnaWNlcnQuY29tMSAwHgYDVQQDExdEaWdpQ2VydCBHbG9iYWwgUm9vdCBH\n\
MjAeFw0xMzA4MDExMjAwMDBaFw0zODAxMTUxMjAwMDBaMGExCzAJBgNVBAYTAlVT\n\
MRUwEwYDVQQKEwxEaWdpQ2VydCBJbmMxGTAXBgNVBAsTEHd3dy5kaWdpY2VydC5j\n\
b20xIDAeBgNVBAMTF0RpZ2lDZXJ0IEdsb2JhbCBSb290IEcyMIIBIjANBgkqhkiG\n\
9w0BAQEFAAOCAQ8AMIIBCgKCAQEAuzfNNNx7a8mHSSUMmTfZD+Zpkoni0Nu3lGFw\n\
UtGRHXeaLLSpMWg0hEcnl28n4U9GlA7dnRDU5LF3AG68KzGDNXOzWfZ2i0QerRBi\n\
Tr5YYvBPod15xA30HCOzK/ee1dAikRC9znOM3474NiDn4i8U7S/UZjs540WUULxq\n\
30/7U7+cG952W1DnKA1QHcC102G5BQey3242WvNowoSPwZs0H0T4jadbo40laaDC\n\
NJTM9FGtcVao3uWGBKNqRoj15CEo054+27Mh/iSPzxOnK9UzWbGwioZX8enrU8h3\n\
oU5dMpbXU0WC2Ur/gYzgwTN3NuUbiMBJr52GQSmWHXOskL5QdQIDAQAB\n\
-----END CERTIFICATE-----\n\
\n\
# Issuer: ISRG Root X1\n\
-----BEGIN CERTIFICATE-----\n\
MIIFazCCA1OgAwIBAgIRAIIQz7DSQONZRGPgu2OCiwAwDQYJKoZIhvcNAQELBQAw\n\
TzELMAkGA1UEBhMCVVMxKTAnBgNVBAoTIEludGVybmV0IFNlY3VyaXR5IFJlc2Vh\n\
cmNoIEdyb3VwMRUwEwYDVQQDEwxJU1JHIFJvb3QgWDEwHhcNMTUwNjA0MTEwNDM4\n\
WhcNMzUwNjA0MTEwNDM4WjBPMQswCQYDVQQGEwJVUzEpMCcGA1UEChMgSW50ZXJu\n\
ZXQgU2VjdXJpdHkgUmVzZWFyY2ggR3JvdXAxFTATBgNVBAMTDElTUkcgUm9vdCBY\n\
MTCCAiIwDQYJKoZIhvcNAQEBBQADggIPADCCAgoCggIBAK3oJHg0dVbqQbSj2FT5\n\
0HGZX08O06DNJHDjKsR98OGyggqo7NBTgk64BrblTRlt+dH3013tBc5R4ETPPrip\n\
DACRF9kcJbnoP/G9ndYWCC9v4aoViP06fpUX5U70Aj68Z/avAupGIBo5okD8Bat8\n\
Il5VM20rMdaLZQ7JShAZrETvHgpNTutSmVKA877NRPWr5z8aJAymcurSGh+/E12s\n\
SLb2A02v7/9agKqLdEPKejNcRxV4/xzNiC8Z55wZsf8wU5zJNRTxBWk83PMIrLsk\n\
3kgmxPT2byVWGydWVHYvWDW36absDA7RDHXwahFoU89U38ua800Up51M4ZZQZT14\n\
340Ujcd8uoiZSRBz8285tJBRuWCqkyem52qURVDHzxzFVMDtdm8bWJK36y8LNJ73\n\
TUyKDRUca+vSypG4C6nGRZ988p5xWjUUz6A4rlM9wUwLJM2+20cNYgDE1eNu5deY\n\
PhOFZK0vjE6zYTspAOF82ecWNVnwU4OUOuu9FFtRvdDXA90=\n\
-----END CERTIFICATE-----\n";
