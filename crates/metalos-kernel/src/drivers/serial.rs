//! ARM PrimeCell PL011 UART driver (`0x0900_0000` on QEMU `-M virt`).

use core::fmt;
#[cfg(all(target_arch = "aarch64", target_os = "none"))]
use core::ptr::{read_volatile, write_volatile};
use spin::Mutex;

/// Base MMIO address of UART0 (PL011) on QEMU `virt`.
pub const PL011_BASE: usize = 0x0900_0000;

#[allow(dead_code)]
const UARTDR: usize = 0x00;
#[allow(dead_code)]
const UARTFR: usize = 0x18;
#[allow(dead_code)]
const UARTLCR_H: usize = 0x2C;
#[allow(dead_code)]
const UARTCR: usize = 0x30;
#[allow(dead_code)]
const UARTIMSC: usize = 0x38;

#[allow(dead_code)]
const FR_RXFE: u32 = 1 << 4; // Receive FIFO empty
#[allow(dead_code)]
const FR_TXFF: u32 = 1 << 5; // Transmit FIFO full

pub struct Pl011Uart {
    base: usize,
}

impl Pl011Uart {
    pub const unsafe fn new(base: usize) -> Self {
        Self { base }
    }

    pub fn init(&mut self) {
        #[cfg(all(target_arch = "aarch64", target_os = "none"))]
        unsafe {
            // Disable UART before configuration
            write_volatile((self.base + UARTCR) as *mut u32, 0);
            // Enable 8-bit word length and FIFOs (FEN = bit 4, WLEN 8-bit = bits 6:5)
            write_volatile((self.base + UARTLCR_H) as *mut u32, (1 << 4) | (0b11 << 5));
            // Mask all interrupts (polled mode for early console/shell)
            write_volatile((self.base + UARTIMSC) as *mut u32, 0);
            // Enable UART (bit 0), TXE (bit 8), RXE (bit 9)
            write_volatile(
                (self.base + UARTCR) as *mut u32,
                (1 << 0) | (1 << 8) | (1 << 9),
            );
        }
    }

    pub fn send(&mut self, byte: u8) {
        #[cfg(all(target_arch = "aarch64", target_os = "none"))]
        unsafe {
            while (read_volatile((self.base + UARTFR) as *const u32) & FR_TXFF) != 0 {
                core::hint::spin_loop();
            }
            write_volatile((self.base + UARTDR) as *mut u32, byte as u32);
        }
        #[cfg(not(all(target_arch = "aarch64", target_os = "none")))]
        {
            let _ = self.base;
            let _ = byte;
        }
    }

    pub fn receive(&mut self) -> u8 {
        #[cfg(all(target_arch = "aarch64", target_os = "none"))]
        unsafe {
            while (read_volatile((self.base + UARTFR) as *const u32) & FR_RXFE) != 0 {
                core::hint::spin_loop();
            }
            (read_volatile((self.base + UARTDR) as *const u32) & 0xFF) as u8
        }
        #[cfg(not(all(target_arch = "aarch64", target_os = "none")))]
        {
            0
        }
    }
}

impl fmt::Write for Pl011Uart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            self.send(byte);
        }
        Ok(())
    }
}

pub static SERIAL1: Mutex<Pl011Uart> = Mutex::new(unsafe { Pl011Uart::new(PL011_BASE) });

pub fn init() {
    #[cfg(target_os = "none")]
    {
        SERIAL1.lock().init();
    }
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    #[cfg(target_os = "none")]
    {
        use core::fmt::Write;
        crate::arch::without_interrupts(|| {
            SERIAL1
                .lock()
                .write_fmt(args)
                .expect("Printing to PL011 UART failed");
        });
    }
    #[cfg(not(target_os = "none"))]
    {
        std::print!("{}", args);
    }
}

#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::drivers::serial::_print(format_args!($($arg)*));
    };
}

#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($($arg:tt)*) => ($crate::serial_print!("{}\n", format_args!($($arg)*)));
}
