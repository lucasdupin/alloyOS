use liballoy::process::UtsName;
use alloyos_proto::SyscallNo;

#[test]
fn write_syscall_number_is_correct() {
    assert_eq!(SyscallNo::Write as u64, 1);
}

#[test]
fn read_syscall_number_is_correct() {
    assert_eq!(SyscallNo::Read as u64, 0);
}

#[test]
fn open_syscall_number_is_correct() {
    assert_eq!(SyscallNo::Open as u64, 2);
}

#[test]
fn uname_struct_field_count() {
    let uts = UtsName::default();
    assert_eq!(uts.sysname.len(), 65);
    assert_eq!(uts.nodename.len(), 65);
    assert_eq!(uts.release.len(), 65);
    assert_eq!(uts.version.len(), 65);
    assert_eq!(uts.machine.len(), 65);
    // 5 fields of [u8; 65] = 325 bytes total
    assert_eq!(core::mem::size_of::<UtsName>(), 5 * 65);
}
