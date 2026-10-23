//! Native service thread primitives. These are unavailable to ordinary headless
//! app and display processes unless the supervisor explicitly enables threads.
use alloyos_proto::process::*;

/// Create a thread in the caller's process. The entry receives `argument` in x0
/// and must exit through `exit`; no return address is installed.
///
/// # Safety
/// The caller owns and synchronizes the stack and thread-local block, initializes
/// TLS according to the executable's ABI, and keeps both mapped until join.
/// Entry must be a valid nonreturning AArch64 function with the expected ABI.
pub unsafe fn create(
    entry: u64,
    argument: u64,
    stack_base: u64,
    stack_length: usize,
    thread_pointer: u64,
) -> Result<u64, i64> {
    let mut descriptor = [0u8; THREAD_DESCRIPTOR_BYTES];
    for (index, value) in [
        entry,
        argument,
        stack_base,
        stack_length as u64,
        thread_pointer,
        0,
    ]
    .into_iter()
    .enumerate()
    {
        descriptor[index * 8..index * 8 + 8].copy_from_slice(&value.to_le_bytes());
    }
    let result = unsafe {
        crate::syscall::syscall3(
            THREAD_CREATE,
            descriptor.as_ptr() as u64,
            descriptor.len() as u64,
            0,
        )
    };
    if result < 0 {
        Err(result)
    } else {
        Ok(result as u64)
    }
}

/// Reap a retired child thread. EAGAIN means its stack/TLS are still in use.
pub fn join(tid: u64) -> Result<i32, i64> {
    let mut code = 0i32;
    let result =
        unsafe { crate::syscall::syscall3(THREAD_JOIN, tid, &mut code as *mut i32 as u64, 0) };
    if result == 0 {
        Ok(code)
    } else {
        Err(result)
    }
}

pub fn id() -> u64 {
    unsafe { crate::syscall::syscall0(GETTID) as u64 }
}

/// Compare-and-sleep on a synchronization word. Use atomic loads/stores with
/// suitable ordering around this operation; notification is not a memory fence.
pub fn wait(
    word: &core::sync::atomic::AtomicU32,
    expected: u32,
    deadline_ns: u64,
) -> Result<(), i64> {
    let result = unsafe {
        crate::syscall::syscall3(
            WORD_WAIT,
            word.as_ptr() as u64,
            expected as u64,
            deadline_ns,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(result)
    }
}

pub fn wake(word: &core::sync::atomic::AtomicU32, count: u32) -> Result<usize, i64> {
    let result =
        unsafe { crate::syscall::syscall3(WORD_WAKE, word.as_ptr() as u64, count as u64, 0) };
    if result < 0 {
        Err(result)
    } else {
        Ok(result as usize)
    }
}

/// Exit only the calling thread. Shared process resources survive until the
/// whole process exits; a fault in any thread ends the process.
pub fn exit(code: i32) -> ! {
    unsafe {
        crate::syscall::syscall1(THREAD_EXIT, code as u64);
    }
    loop {
        core::hint::spin_loop();
    }
}
