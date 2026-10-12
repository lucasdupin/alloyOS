use metalos_boot::memory_map_is_valid;
use metalos_proto::{MemoryRegion, MemoryRegionKind};

#[test]
fn parse_mixed_region_types() {
    let regions = [
        MemoryRegion::new(0x0000, 0x1000, MemoryRegionKind::Bootloader),
        MemoryRegion::new(0x1000, 0x1000, MemoryRegionKind::KernelAndModules),
        MemoryRegion::new(0x2000, 0x2000, MemoryRegionKind::Usable),
        MemoryRegion::new(0x4000, 0x1000, MemoryRegionKind::AcpReclaimable),
        MemoryRegion::new(0x5000, 0x1000, MemoryRegionKind::AcpiNvs),
        MemoryRegion::new(0x6000, 0x1000, MemoryRegionKind::BadMemory),
        MemoryRegion::new(0x7000, 0x1000, MemoryRegionKind::Framebuffer),
        MemoryRegion::new(0x8000, 0x1000, MemoryRegionKind::Reserved),
    ];

    assert!(memory_map_is_valid(&regions));
}

#[test]
fn usable_regions_sorted() {
    let regions = [
        MemoryRegion::new(0x0000, 0x1000, MemoryRegionKind::Bootloader),
        MemoryRegion::new(0x1000, 0x2000, MemoryRegionKind::Usable),
        MemoryRegion::new(0x3000, 0x1000, MemoryRegionKind::Reserved),
        MemoryRegion::new(0x4000, 0x4000, MemoryRegionKind::Usable),
        MemoryRegion::new(0x8000, 0x1000, MemoryRegionKind::AcpReclaimable),
        MemoryRegion::new(0x9000, 0x5000, MemoryRegionKind::Usable),
    ];

    assert!(memory_map_is_valid(&regions));

    let usable_regions: Vec<_> = regions
        .iter()
        .filter(|r| r.kind == MemoryRegionKind::Usable)
        .copied()
        .collect();

    assert_eq!(usable_regions.len(), 3);
    for window in usable_regions.windows(2) {
        assert!(window[0].start < window[1].start);
        assert!(window[0].end() <= window[1].start);
    }
}
