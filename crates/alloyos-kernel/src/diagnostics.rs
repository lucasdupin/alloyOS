//! Bounded, allocation-free AArch64 crash reports. Frame pointers are enabled
//! for bare-metal builds; addresses are symbolicated against the matching ELF.
use crate::drivers::serial::emergency_print as print;
use core::sync::atomic::{AtomicBool, Ordering};
const MAX_FRAMES: usize = 32;
static REPORTING: AtomicBool = AtomicBool::new(false);

/// Only read complete aligned frame records within known stack backing. Corrupt
/// chains, cycles, overflows and excessive depth terminate the walk.
fn walk(
    mut fp: u64,
    low: u64,
    high: u64,
    mut read: impl FnMut(u64) -> Option<(u64, u64)>,
    mut emit: impl FnMut(u64),
) -> &'static str {
    for _ in 0..MAX_FRAMES {
        if fp == 0 {
            return "end";
        }
        if fp & 15 != 0 || fp < low || fp.checked_add(16).is_none_or(|end| end > high) {
            return "invalid-frame";
        }
        let Some((next, lr)) = read(fp) else {
            return "unreadable-frame";
        };
        if lr == 0 {
            return "end";
        }
        emit(lr);
        if next == 0 {
            return "end";
        }
        if next <= fp {
            return "non-increasing-frame";
        }
        fp = next;
    }
    "depth-limit"
}
fn begin() -> bool {
    if REPORTING.swap(true, Ordering::Relaxed) {
        print(format_args!(
            "[crash] nested fault; stack walk suppressed\n"
        ));
        false
    } else {
        true
    }
}
fn frame(index: usize, pc: u64, kind: &str) {
    print(format_args!(
        "[crash] frame={index} pc={pc:#018x} kind={kind}\n"
    ));
}

pub fn panic_report(info: &core::panic::PanicInfo) {
    #[cfg(all(target_arch = "aarch64", target_os = "none"))]
    unsafe {
        core::arch::asm!("msr daifset, #15", options(nostack));
    }
    if !begin() {
        return;
    }
    struct Message {
        line_start: bool,
    }
    impl core::fmt::Write for Message {
        fn write_str(&mut self, text: &str) -> core::fmt::Result {
            for part in text.split_inclusive('\n') {
                if self.line_start {
                    print(format_args!("[crash] "));
                }
                print(format_args!("{part}"));
                self.line_start = part.ends_with('\n');
            }
            Ok(())
        }
    }
    use core::fmt::Write;
    let _ = writeln!(Message { line_start: true }, "kernel-panic {info}");
    #[cfg(all(target_arch = "aarch64", target_os = "none"))]
    {
        let fp: u64;
        unsafe {
            core::arch::asm!("mov {}, x29", out(reg) fp, options(nomem, nostack));
        }
        kernel_trace(fp);
    }
    // Fatal panic: leave the guard set to suppress a fault during reporting.
}

#[cfg(all(target_arch = "aarch64", target_os = "none"))]
fn kernel_trace(fp: u64) {
    unsafe extern "C" {
        static __stack_bottom: u8;
        static __stack_top: u8;
    }
    let task = crate::task::scheduler::try_current_task();
    let bounds = task.as_ref().map(|task| {
        let start = task.kernel_stack.as_ptr() as u64;
        (start, start + task.kernel_stack.len() as u64)
    });
    let boot = (
        core::ptr::addr_of!(__stack_bottom) as u64,
        core::ptr::addr_of!(__stack_top) as u64,
    );
    let (low, high) = bounds
        .filter(|(low, high)| fp >= *low && fp < *high)
        .unwrap_or(boot);
    let mut index = 0;
    let stop = walk(
        fp,
        low,
        high,
        |address| {
            // The walker validated this record against a live kernel stack. No
            // arbitrary virtual pointer from the crashed code is dereferenced.
            unsafe {
                Some((
                    core::ptr::read_volatile(address as *const u64),
                    core::ptr::read_volatile((address + 8) as *const u64),
                ))
            }
        },
        |pc| {
            frame(index, pc, "return");
            index += 1;
        },
    );
    print(format_args!("[crash] stack-end reason={stop}\n"));
}

pub fn exception_report(tf: &crate::arch::idt::TrapFrame) {
    if !begin() {
        return;
    }
    let user = tf.spsr & 15 == 0;
    let task = crate::task::scheduler::try_current_task();
    print(format_args!(
        "[crash] exception domain={} pid={} ec={:#x} esr={:#018x} far={:#018x} spsr={:#018x}\n",
        if user { "user" } else { "kernel" },
        task.as_ref().map(|t| t.pid).unwrap_or(0),
        tf.esr >> 26,
        tf.esr,
        tf.far,
        tf.spsr
    ));
    print(format_args!(
        "[crash] fp={:#018x} lr={:#018x} user-sp={:#018x}\n",
        tf.regs[29], tf.regs[30], tf.user_sp
    ));
    for (index, value) in tf.regs.iter().enumerate() {
        print(format_args!("[crash] x{index}={value:#018x}\n"));
    }
    frame(0, tf.elr, "fault");
    if user {
        if let Some(task) = task {
            if let Some(guard) = task.address_space.try_lock() {
                if let Some(space) = guard.as_ref() {
                    let high = space.stack_top();
                    let low = high.saturating_sub(64 * 4096).max(tf.user_sp);
                    let mut index = 1;
                    let stop = walk(
                        tf.regs[29],
                        low,
                        high,
                        |address| {
                            let mut record = [0; 16];
                            space.copy_in(address, &mut record).ok()?;
                            Some((
                                u64::from_le_bytes(record[..8].try_into().ok()?),
                                u64::from_le_bytes(record[8..].try_into().ok()?),
                            ))
                        },
                        |pc| {
                            frame(index, pc, "return");
                            index += 1;
                        },
                    );
                    print(format_args!("[crash] stack-end reason={stop}\n"));
                    REPORTING.store(false, Ordering::Relaxed);
                    return;
                }
            }
        }
        print(format_args!("[crash] stack-end reason=stack-unavailable\n"));
    } else {
        #[cfg(all(target_arch = "aarch64", target_os = "none"))]
        kernel_trace(tf.regs[29]);
    }
    REPORTING.store(false, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frame_chain_is_bounded_and_cycles_never_read_twice() {
        let mut frames = std::vec::Vec::new();
        assert_eq!(
            walk(
                0x1000,
                0x1000,
                0x1100,
                |fp| Some((fp + 16, fp + 4)),
                |pc| frames.push(pc)
            ),
            "invalid-frame"
        );
        assert_eq!(frames.len(), 16);
        let mut reads = 0;
        assert_eq!(
            walk(
                0x1000,
                0x1000,
                0x1100,
                |fp| {
                    reads += 1;
                    Some((fp, 8))
                },
                |_| {}
            ),
            "non-increasing-frame"
        );
        assert_eq!(reads, 1);
        assert_eq!(
            walk(
                0x1001,
                0x1000,
                0x1100,
                |_| panic!("must not read unaligned pointer"),
                |_| {}
            ),
            "invalid-frame"
        );
        assert_eq!(
            walk(
                u64::MAX - 15,
                0,
                u64::MAX,
                |_| panic!("must not read overflowed record"),
                |_| {}
            ),
            "invalid-frame"
        );
        assert_eq!(
            walk(0x1000, 0x1000, 0x10000, |fp| Some((fp + 16, 4)), |_| {}),
            "depth-limit"
        );
    }
    #[test]
    fn unmapped_user_frames_stop_without_a_dereference() {
        assert_eq!(
            walk(
                0x1000,
                0x1000,
                0x1100,
                |_| None,
                |_| panic!("no frame emitted")
            ),
            "unreadable-frame"
        );
    }
}

#[cfg(feature = "crash-selftest")]
#[inline(never)]
fn crash_probe_leaf() {
    // Prove panic output works even when the crashing path holds SERIAL1.
    let _serial_guard = crate::drivers::serial::SERIAL1.lock();
    panic!("intentional crash diagnostic selftest with serial lock held");
}
#[cfg(feature = "crash-selftest")]
#[inline(never)]
fn crash_probe_parent() {
    core::hint::black_box(crash_probe_leaf as fn())();
    core::hint::black_box(());
}
#[cfg(feature = "crash-selftest")]
#[inline(never)]
pub fn selftest() {
    core::hint::black_box(crash_probe_parent as fn())();
    core::hint::black_box(());
}
