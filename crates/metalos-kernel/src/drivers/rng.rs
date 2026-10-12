//! Hardware Random Number Generator (RNG) & ChaCha20 CSPRNG backing `/dev/urandom` and `SYS_GETRANDOM`.
//!
//! Combines ARMv8.5-RNG (`RNDR` system register when advertised by `ID_AA64ISAR0_EL1.RNDR`),
//! ARM Generic Timer (`CNTPCT_EL0`) jitter entropy, and a `#![no_std]` 20-round ChaCha20 stream cipher.

use spin::Mutex;

const CHACHA_CONSTANTS: [u32; 4] = [0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574];

pub struct ChaCha20Rng {
    key: [u32; 8],
    counter: u64,
    nonce: [u32; 2],
    seeded: bool,
}

impl ChaCha20Rng {
    pub const fn new() -> Self {
        Self {
            key: [
                0x9e37_79b9,
                0x243f_6a88,
                0x85a3_08d3,
                0x1319_8a2e,
                0x0370_7344,
                0xa409_3822,
                0x299f_31d0,
                0x082e_fa98,
            ],
            counter: 1,
            nonce: [0x6d65_7461, 0x6c4f_5321],
            seeded: false,
        }
    }

    #[inline(always)]
    fn quarter_round(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
        state[a] = state[a].wrapping_add(state[b]);
        state[d] ^= state[a];
        state[d] = state[d].rotate_left(16);

        state[c] = state[c].wrapping_add(state[d]);
        state[b] ^= state[c];
        state[b] = state[b].rotate_left(12);

        state[a] = state[a].wrapping_add(state[b]);
        state[d] ^= state[a];
        state[d] = state[d].rotate_left(8);

        state[c] = state[c].wrapping_add(state[d]);
        state[b] ^= state[c];
        state[b] = state[b].rotate_left(7);
    }

    pub fn reseed(&mut self, hw_seed: [u64; 4]) {
        for (i, word) in hw_seed.iter().enumerate() {
            let lo = *word as u32;
            let hi = (*word >> 32) as u32;
            self.key[i * 2] ^= lo.wrapping_mul(0x85eb_ca6b);
            self.key[i * 2 + 1] ^= hi.wrapping_mul(0xc2b2_ae35);
        }
        self.counter = self.counter.wrapping_add(1);
        self.seeded = true;
    }

    pub fn next_block(&mut self) -> [u8; 64] {
        if !self.seeded {
            let entropy = collect_arch_entropy();
            self.reseed(entropy);
        }

        let mut state = [0u32; 16];
        state[0..4].copy_from_slice(&CHACHA_CONSTANTS);
        state[4..12].copy_from_slice(&self.key);
        state[12] = self.counter as u32;
        state[13] = (self.counter >> 32) as u32;
        state[14] = self.nonce[0];
        state[15] = self.nonce[1];

        let initial = state;

        for _ in 0..10 {
            // Column rounds
            Self::quarter_round(&mut state, 0, 4, 8, 12);
            Self::quarter_round(&mut state, 1, 5, 9, 13);
            Self::quarter_round(&mut state, 2, 6, 10, 14);
            Self::quarter_round(&mut state, 3, 7, 11, 15);
            // Diagonal rounds
            Self::quarter_round(&mut state, 0, 5, 10, 15);
            Self::quarter_round(&mut state, 1, 6, 11, 12);
            Self::quarter_round(&mut state, 2, 7, 8, 13);
            Self::quarter_round(&mut state, 3, 4, 9, 14);
        }

        let mut out = [0u8; 64];
        for i in 0..16 {
            let word = state[i].wrapping_add(initial[i]);
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }

        self.counter = self.counter.wrapping_add(1);
        // Fast-key-erasure style forward security every 16 blocks
        if (self.counter & 0xF) == 0 {
            for i in 0..8 {
                let w = u32::from_le_bytes([
                    out[i * 4],
                    out[i * 4 + 1],
                    out[i * 4 + 2],
                    out[i * 4 + 3],
                ]);
                self.key[i] ^= w;
            }
        }

        out
    }

    pub fn fill_bytes(&mut self, dest: &mut [u8]) {
        let mut offset = 0;
        while offset < dest.len() {
            let block = self.next_block();
            let take = core::cmp::min(64, dest.len() - offset);
            dest[offset..offset + take].copy_from_slice(&block[..take]);
            offset += take;
        }
    }
}

static GLOBAL_CSPRNG: Mutex<ChaCha20Rng> = Mutex::new(ChaCha20Rng::new());

/// Read hardware entropy from ARMv8.5 `RNDR` (if supported in `ID_AA64ISAR0_EL1`) and `CNTPCT_EL0`.
pub fn collect_arch_entropy() -> [u64; 4] {
    #[cfg(all(target_arch = "aarch64", target_os = "none"))]
    unsafe {
        let isar0: u64;
        core::arch::asm!("mrs {}, id_aa64isar0_el1", out(reg) isar0, options(nomem, nostack, preserves_flags));
        let has_rndr = ((isar0 >> 60) & 0xF) != 0;

        let mut out = [0u64; 4];
        for (i, slot) in out.iter_mut().enumerate() {
            let cnt: u64;
            core::arch::asm!("mrs {}, cntpct_el0", out(reg) cnt, options(nomem, nostack, preserves_flags));
            let mut val = cnt.wrapping_mul(0x9e37_79b9_7f4a_7c15).rotate_left((i * 17 + 13) as u32);
            if has_rndr {
                let rnd: u64;
                // S3_3_C2_C4_0 is RNDR
                core::arch::asm!("mrs {}, s3_3_c2_c4_0", out(reg) rnd, options(nomem, nostack));
                val ^= rnd;
            }
            *slot = val;
        }
        out
    }

    #[cfg(not(all(target_arch = "aarch64", target_os = "none")))]
    {
        static COUNTER: core::sync::atomic::AtomicU64 =
            core::sync::atomic::AtomicU64::new(0x517c_c1b7_2722_0a95);
        let c = COUNTER.fetch_add(0x9e37_79b9_7f4a_7c15, core::sync::atomic::Ordering::Relaxed);
        let addr_entropy = &COUNTER as *const _ as usize as u64;
        [
            c ^ addr_entropy,
            c.rotate_left(17) ^ 0xdead_beef_cafe_babe,
            c.rotate_left(31) ^ 0x0123_4567_89ab_cdef,
            c.rotate_left(47) ^ addr_entropy.wrapping_mul(0xbf58_476d_1ce4_e5b9),
        ]
    }
}

/// Read the monotonic hardware counter in nanoseconds.
pub fn monotonic_time_ns() -> u64 {
    #[cfg(all(target_arch = "aarch64", target_os = "none"))]
    unsafe {
        let cnt: u64;
        let freq: u64;
        // Guest virtual time avoids a physical-counter VM exit under HVF.
        core::arch::asm!("mrs {}, cntvct_el0", out(reg) cnt, options(nomem, nostack, preserves_flags));
        static FREQUENCY: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
        let cached = FREQUENCY.load(core::sync::atomic::Ordering::Relaxed);
        let f = if cached == 0 {
            core::arch::asm!("mrs {}, cntfrq_el0", out(reg) freq, options(nomem, nostack, preserves_flags));
            let f = if freq == 0 { 62_500_000 } else { freq };
            FREQUENCY.store(f, core::sync::atomic::Ordering::Relaxed);
            f
        } else { cached };
        let remainder = cnt % f;
        let nanos = remainder.checked_mul(1_000_000_000).map(|n| n / f)
            .unwrap_or_else(|| ((remainder as u128 * 1_000_000_000u128) / f as u128) as u64);
        (cnt / f).saturating_mul(1_000_000_000).saturating_add(nanos)
    }

    #[cfg(not(all(target_arch = "aarch64", target_os = "none")))]
    {
        static TICKS: core::sync::atomic::AtomicU64 =
            core::sync::atomic::AtomicU64::new(1_000_000_000);
        TICKS.fetch_add(1_000_000, core::sync::atomic::Ordering::Relaxed)
    }
}

/// Fill `dest` with cryptographically strong pseudo-random bytes.
pub fn fill_random_bytes(dest: &mut [u8]) {
    let mut rng = GLOBAL_CSPRNG.lock();
    rng.fill_bytes(dest);
}

/// Return a random `u64` from the kernel CSPRNG.
pub fn next_u64() -> u64 {
    let mut buf = [0u8; 8];
    fill_random_bytes(&mut buf);
    u64::from_le_bytes(buf)
}
