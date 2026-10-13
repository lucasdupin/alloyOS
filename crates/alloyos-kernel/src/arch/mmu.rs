//! ARMv8-A (`aarch64`) MMU & Translation Table setup for QEMU `-M virt`.
//!
//! Configures a 39-bit virtual address space (`T0SZ = 25`, 4 KiB granule) where Level 1
//! is the root table pointed to by `TTBR0_EL1`, using 1 GiB block descriptors:
//! - `0x0000_0000..0x4000_0000` (1 GiB): Device-nGnRnE (PL011 UART, GICv2, VirtIO MMIO)
//! - `0x4000_0000..0x8000_0000` (1 GiB): Normal Write-Back Cacheable Inner-Shareable RAM

/// MAIR_EL1 attribute index 0: Device-nGnRnE (`0x00`)
pub const MAIR_ATTR_DEVICE_NGNRNE: u64 = 0x00;
/// MAIR_EL1 attribute index 1: Normal Inner/Outer Write-Back Non-transient RW-Allocate (`0xFF`)
pub const MAIR_ATTR_NORMAL_WB: u64 = 0xFF;
/// Combined `MAIR_EL1` register value.
pub const MAIR_VALUE: u64 = MAIR_ATTR_DEVICE_NGNRNE | (MAIR_ATTR_NORMAL_WB << 8);

/// 1 GiB Block Descriptor flags (`bits[1:0] = 0b01`).
pub const DESC_VALID_BLOCK: u64 = 0b01;
/// Access Flag (`bit[10] = 1`).
pub const DESC_AF: u64 = 1 << 10;
/// Inner Shareable (`bits[9:8] = 0b11`).
pub const DESC_SH_INNER: u64 = 0b11 << 8;
/// Outer Shareable (`bits[9:8] = 0b10`).
pub const DESC_SH_OUTER: u64 = 0b10 << 8;
/// EL1 Read/Write + Execute (`AP[2:1] = 0b00` at `bits[7:6]`).
/// Note: in ARMv8-A, setting EL0 write permission (`0b01`) implicitly forces PXN=1 at EL1!
pub const DESC_AP_RW_EL1: u64 = 0b00 << 6;
/// Privileged Execute-Never (`bit[53]`) & Unprivileged Execute-Never (`bit[54]`).
pub const DESC_XN: u64 = (1 << 53) | (1 << 54);

/// Construct a 1 GiB Level-1 Block Descriptor for Device-nGnRnE MMIO.
pub const fn l1_device_block(phys_base: u64) -> u64 {
    (phys_base & 0x0000_FFFF_C000_0000)
        | DESC_VALID_BLOCK
        | (0 << 2) // AttrIndx = 0 (Device-nGnRnE)
        | DESC_SH_OUTER
        | DESC_AF
        | DESC_XN
}

/// Construct a 1 GiB Level-1 Block Descriptor for Normal Write-Back Inner-Shareable RAM.
pub const fn l1_normal_ram_block(phys_base: u64) -> u64 {
    (phys_base & 0x0000_FFFF_C000_0000)
        | DESC_VALID_BLOCK
        | (1 << 2) // AttrIndx = 1 (Normal WB)
        | DESC_AP_RW_EL1
        | DESC_SH_INNER
        | DESC_AF
}

#[repr(C, align(4096))]
pub struct L1PageTable {
    pub entries: [u64; 512],
}

impl L1PageTable {
    pub const fn new_identity_2gib() -> Self {
        let mut entries = [0u64; 512];
        // 0x0000_0000 .. 0x4000_0000: Device MMIO (UART, GIC, VirtIO)
        entries[0] = l1_device_block(0x0000_0000);
        // 0x4000_0000 .. 0x8000_0000: Normal Cacheable RAM (QEMU -M virt RAM starts at 0x4000_0000)
        // Forge weights are loaded into a reserved region above the ordinary
        // allocator. Cover up to 32 GiB while retaining device attributes below
        // 1 GiB. Unbacked RAM is never handed to the allocator or accessed.
        let mut slot = 1;
        while slot < 32 {
            entries[slot] = l1_normal_ram_block((slot as u64) << 30);
            slot += 1;
        }
        Self { entries }
    }
}

pub static IDENTITY_L1_TABLE: L1PageTable = L1PageTable::new_identity_2gib();

/// Initialize and enable the ARMv8-A MMU and data/instruction caches.
pub fn init() {
    #[cfg(all(target_arch = "aarch64", target_os = "none"))]
    unsafe {
        let table_phys = &raw const IDENTITY_L1_TABLE as u64;
        // TCR_EL1:
        // - T0SZ = 25 (bits[5:0]): 39-bit VA space (512 GiB, starts at L1 with 4 KiB granule)
        // - IRGN0 = 0b01 (bits[9:8]): Normal memory, Inner Write-Back Read-Allocate Write-Allocate Cacheable
        // - ORGN0 = 0b01 (bits[11:10]): Normal memory, Outer Write-Back Read-Allocate Write-Allocate Cacheable
        // - SH0 = 0b11 (bits[13:12]): Inner Shareable
        // - TG0 = 0b00 (bits[15:14]): 4 KiB granule
        // - IPS = 0b010 (bits[34:32]): 40-bit PA space (1 TiB)
        let tcr_val: u64 = 25
            | (0b01 << 8)
            | (0b01 << 10)
            | (0b11 << 12)
            | (0b00 << 14)
            | (0b010u64 << 32);

        core::arch::asm!(
            "msr mair_el1, {mair}",
            "msr tcr_el1, {tcr}",
            "msr ttbr0_el1, {ttbr0}",
            "dsb ish",
            "isb",
            "tlbi vmalle1",
            "dsb ish",
            "isb",
            "mrs {tmp}, sctlr_el1",
            "orr {tmp}, {tmp}, #(1 << 0)",  // SCTLR_EL1.M (Enable MMU)
            "orr {tmp}, {tmp}, #(1 << 2)",  // SCTLR_EL1.C (Enable Data Cache)
            "orr {tmp}, {tmp}, #(1 << 12)", // SCTLR_EL1.I (Enable Instruction Cache)
            "msr sctlr_el1, {tmp}",
            "isb",
            mair = in(reg) MAIR_VALUE,
            tcr = in(reg) tcr_val,
            ttbr0 = in(reg) table_phys,
            tmp = out(reg) _,
            options(nostack)
        );
    }
}
