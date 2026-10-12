pub use metalos_proto::{MemoryRegion, MemoryRegionKind};

pub mod bootloader {

    /// Configuration for physical memory mapping.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct PhysicalMemoryConfig {
        pub init: bool,
    }

    /// Bootloader configuration structure.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct BootloaderConfig {
        pub physical_memory: PhysicalMemoryConfig,
        pub kernel_stack_size: u64,
    }

    impl Default for BootloaderConfig {
        fn default() -> Self {
            Self {
                physical_memory: PhysicalMemoryConfig { init: true },
                kernel_stack_size: 128 * 1024, // 128 KiB
            }
        }
    }
}

pub use self::bootloader::{BootloaderConfig, PhysicalMemoryConfig};

/// Creates standard bootloader configuration with mapped physical memory and kernel stack.
pub fn create_boot_config() -> bootloader::BootloaderConfig {
    bootloader::BootloaderConfig::default()
}

/// Validates that memory regions are sorted by start address, non-empty, and non-overlapping.
pub fn memory_map_is_valid(regions: &[metalos_proto::MemoryRegion]) -> bool {
    if regions.is_empty() {
        return false;
    }

    for window in regions.windows(2) {
        let cur = &window[0];
        let next = &window[1];

        // Ensure non-zero length and no u64 overflow
        if cur.len == 0 || cur.start.checked_add(cur.len).is_none() {
            return false;
        }
        if next.len == 0 || next.start.checked_add(next.len).is_none() {
            return false;
        }

        // Must be strictly ordered and non-overlapping
        if cur.start >= next.start || cur.end() > next.start {
            return false;
        }
    }

    // Check last element if windows was empty (i.e. single element)
    if let Some(last) = regions.last() {
        if last.len == 0 || last.start.checked_add(last.len).is_none() {
            return false;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use metalos_proto::{MemoryRegion, MemoryRegionKind};

    #[test]
    fn boot_config_physical_memory_mapped() {
        let config = create_boot_config();
        assert!(config.physical_memory.init);
    }

    #[test]
    fn boot_config_stack_size_nonzero() {
        let config = create_boot_config();
        assert!(config.kernel_stack_size > 0);
    }

    #[test]
    fn memory_map_valid_nonoverlap() {
        let regions = [
            MemoryRegion::new(0x0, 0x1000, MemoryRegionKind::Bootloader),
            MemoryRegion::new(0x1000, 0x2000, MemoryRegionKind::Usable),
            MemoryRegion::new(0x5000, 0x1000, MemoryRegionKind::Reserved),
        ];
        assert!(memory_map_is_valid(&regions));
    }

    #[test]
    fn memory_map_invalid_overlap() {
        let regions = [
            MemoryRegion::new(0x1000, 0x2000, MemoryRegionKind::Usable), // ends at 0x3000
            MemoryRegion::new(0x2000, 0x2000, MemoryRegionKind::Reserved), // starts at 0x2000 -> overlap!
        ];
        assert!(!memory_map_is_valid(&regions));
    }

    #[test]
    fn memory_map_empty_is_invalid() {
        let regions: [MemoryRegion; 0] = [];
        assert!(!memory_map_is_valid(&regions));
    }
}
