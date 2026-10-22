#![no_std]
#![no_main]
#![feature(alloc_error_handler)]
extern crate alloc;
use alloyos_proto::process;
use alloyos_welder::setup_service::{self, Assembly, Operation, Request};
use liballoy::{runtime::ProcessAllocator, syscall::syscall3};
#[global_allocator]
static HEAP: ProcessAllocator = ProcessAllocator::new();
fn call(number: u64, a: u64, b: u64, c: u64) -> i64 {
    unsafe { syscall3(number, a, b, c) }
}
fn yield_now() {
    call(alloyos_proto::SyscallNo::Yield.as_u64(), 0, 0, 0);
}
fn exit(status: i32) -> ! {
    call(alloyos_proto::SyscallNo::Exit.as_u64(), status as u64, 0, 0);
    loop {
        yield_now();
    }
}
#[no_mangle]
#[link_section = ".text._start"]
pub extern "C" fn _start(heap: usize, length: usize) -> ! {
    unsafe { HEAP.init(heap, length) };
    if call(process::QUERY_ABI_VERSION, 0, 0, 0) != process::ABI_VERSION as i64 {
        exit(72);
    }
    let mut input = [0u8; setup_service::MAX_CHUNK + 8];
    let mut assembly = Assembly::default();
    let mut last_id = 0;
    loop {
        let count = call(
            process::IPC_RECEIVE,
            0,
            input.as_mut_ptr() as u64,
            input.len() as u64,
        );
        if count == -11 {
            yield_now();
            continue;
        }
        if count < 8 || count as usize > input.len() {
            exit(73);
        }
        if u64::from_le_bytes(input[..8].try_into().unwrap()) != 0 {
            exit(74);
        }
        let (id, bytes) = match assembly.push(&input[8..count as usize], None) {
            Ok(Some(message)) => message,
            Ok(None) => continue,
            Err(_) => exit(75),
        };
        if id <= last_id {
            exit(76);
        }
        last_id = id;
        let request: Request = match serde_json::from_slice(&bytes) {
            Ok(request) => request,
            Err(_) => exit(77),
        };
        if request.id != id || request.validate().is_err() {
            exit(78);
        }
        if matches!(request.operation, Operation::FaultProbe) {
            // The supervisor alone can send this containment probe. There are
            // no filesystem/account/display capabilities on this process.
            unsafe {
                core::ptr::read_volatile(0x4008_0000 as *const u64);
            }
            exit(79);
        }
        if matches!(request.operation, Operation::HangProbe) {
            loop {
                core::hint::spin_loop();
            }
        }
        let response = setup_service::execute(&request);
        let bytes = match serde_json::to_vec(&response) {
            Ok(bytes) if bytes.len() <= setup_service::MAX_BYTES => bytes,
            _ => exit(80),
        };
        let mut offset = 0;
        while offset < bytes.len() {
            let frame = setup_service::chunk(id, &bytes, offset).unwrap();
            loop {
                let sent = call(
                    process::IPC_SEND,
                    1,
                    frame.as_ptr() as u64,
                    frame.len() as u64,
                );
                if sent == frame.len() as i64 {
                    break;
                }
                if sent != -11 {
                    exit(81);
                }
                yield_now();
            }
            offset += setup_service::chunk_payload_bytes(frame.len());
        }
    }
}
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    let message = b"Welder setup service panic";
    call(
        process::PANIC_REPORT,
        message.as_ptr() as u64,
        message.len() as u64,
        0,
    );
    exit(70);
}
#[alloc_error_handler]
fn alloc_error(_: core::alloc::Layout) -> ! {
    exit(71);
}
