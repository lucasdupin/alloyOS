use core::fmt;
use spin::Mutex;
use uart_16550::SerialPort;

pub static SERIAL1: Mutex<SerialPort> = Mutex::new(unsafe { SerialPort::new(0x3F8) });

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
        x86_64::instructions::interrupts::without_interrupts(|| {
            SERIAL1.lock().write_fmt(args).expect("Printing to serial failed");
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
