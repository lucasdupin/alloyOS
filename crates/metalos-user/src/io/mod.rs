//! Input/output primitives for metalOS userland.

use metalos_proto::SyscallNo;

/// Write bytes to a file descriptor.
pub fn write(fd: u32, buf: &[u8]) -> i64 {
    unsafe {
        crate::syscall::syscall3(
            SyscallNo::Write.as_u64(),
            fd as u64,
            buf.as_ptr() as u64,
            buf.len() as u64,
        )
    }
}

/// Read bytes from a file descriptor into the provided buffer.
pub fn read(fd: u32, buf: &mut [u8]) -> i64 {
    unsafe {
        crate::syscall::syscall3(
            SyscallNo::Read.as_u64(),
            fd as u64,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )
    }
}

/// Print a string slice to standard output (fd 1).
pub fn print(s: &str) {
    let _ = write(1, s.as_bytes());
}

/// Print a string slice to standard output followed by a newline.
pub fn println(s: &str) {
    print(s);
    print("\n");
}

/// Print a string slice to standard error (fd 2).
pub fn eprint(s: &str) {
    let _ = write(2, s.as_bytes());
}

/// Print a string slice to standard error followed by a newline.
pub fn eprintln(s: &str) {
    eprint(s);
    eprint("\n");
}

/// A zero-sized writer type targeting standard output.
pub struct Stdout;

impl core::fmt::Write for Stdout {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        print(s);
        Ok(())
    }
}

/// A zero-sized writer type targeting standard error.
pub struct Stderr;

impl core::fmt::Write for Stderr {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        eprint(s);
        Ok(())
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = write!($crate::io::Stdout, $($arg)*);
    }};
}

#[macro_export]
macro_rules! println {
    () => {
        $crate::io::print("\n")
    };
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = writeln!($crate::io::Stdout, $($arg)*);
    }};
}

#[macro_export]
macro_rules! eprint {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = write!($crate::io::Stderr, $($arg)*);
    }};
}

#[macro_export]
macro_rules! eprintln {
    () => {
        $crate::io::eprint("\n")
    };
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = writeln!($crate::io::Stderr, $($arg)*);
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syscall::get_last_syscall;
    use core::fmt::Write;

    #[test]
    fn write_constructs_correct_args() {
        let msg = b"test write payload";
        let res = write(1, msg);
        assert_eq!(res, msg.len() as i64);

        let rec = get_last_syscall().expect("syscall should be recorded");
        assert_eq!(rec.num, SyscallNo::Write.as_u64());
        assert_eq!(rec.a1, 1);
        assert_eq!(rec.a2, msg.as_ptr() as u64);
        assert_eq!(rec.a3, msg.len() as u64);
    }

    #[test]
    fn read_buf_size_passed_correctly() {
        let mut buf = [0u8; 128];
        let _ = read(0, &mut buf);

        let rec = get_last_syscall().expect("syscall should be recorded");
        assert_eq!(rec.num, SyscallNo::Read.as_u64());
        assert_eq!(rec.a1, 0);
        assert_eq!(rec.a2, buf.as_mut_ptr() as u64);
        assert_eq!(rec.a3, 128);
    }

    #[test]
    fn stdout_stderr_fmt_write() {
        let mut out = Stdout;
        assert!(write!(out, "hello {}", 123).is_ok());

        let mut err = Stderr;
        assert!(write!(err, "err {}", 456).is_ok());
    }

    #[test]
    fn print_and_println_calls() {
        print("alpha");
        let rec = get_last_syscall().expect("syscall should be recorded");
        assert_eq!(rec.num, SyscallNo::Write.as_u64());
        assert_eq!(rec.a1, 1);

        println("beta");
        let rec2 = get_last_syscall().expect("syscall should be recorded");
        assert_eq!(rec2.num, SyscallNo::Write.as_u64());
        assert_eq!(rec2.a1, 1);

        eprint("gamma");
        let rec3 = get_last_syscall().expect("syscall should be recorded");
        assert_eq!(rec3.num, SyscallNo::Write.as_u64());
        assert_eq!(rec3.a1, 2);

        eprintln("delta");
        let rec4 = get_last_syscall().expect("syscall should be recorded");
        assert_eq!(rec4.num, SyscallNo::Write.as_u64());
        assert_eq!(rec4.a1, 2);
    }
}
