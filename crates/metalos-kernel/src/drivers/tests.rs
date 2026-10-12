#[cfg(any(test, feature = "host-tests"))]
pub mod tests {
    use super::*;
    use alloc::vec::Vec;
    use core::fmt::Write;

    pub struct MockWriter {
        pub buffer: Vec<u8>,
    }

    impl Write for MockWriter {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            self.buffer.extend_from_slice(s.as_bytes());
            Ok(())
        }
    }

    #[test]
    fn serial_format_output() {
        let mut writer = MockWriter { buffer: Vec::new() };
        write!(writer, "metalOS v{}", "0.1.0").unwrap();
        let res = core::str::from_utf8(&writer.buffer).unwrap();
        assert_eq!(res, "metalOS v0.1.0");
    }

    #[test]
    fn serial_format_numbers() {
        let mut writer = MockWriter { buffer: Vec::new() };
        write!(writer, "port: {:#x}, dec: {}", 0x3F8, 42).unwrap();
        let res = core::str::from_utf8(&writer.buffer).unwrap();
        assert_eq!(res, "port: 0x3f8, dec: 42");
    }
}
