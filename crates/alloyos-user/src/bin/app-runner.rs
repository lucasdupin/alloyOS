#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
extern crate alloc;
#[cfg(target_os = "none")]
#[global_allocator]
static HEAP: liballoy::runtime::ProcessAllocator = liballoy::runtime::ProcessAllocator::new();
#[cfg(target_os = "none")]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    liballoy::runtime::panic_report(info)
}
#[cfg(target_os = "none")]
#[no_mangle]
pub unsafe extern "C" fn _start(heap: u64, length: u64) -> ! {
    unsafe { HEAP.init(heap as usize, length as usize) };
    if unsafe {liballoy::syscall::syscall0(alloyos_proto::process::QUERY_ABI_VERSION)} != alloyos_proto::process::ABI_VERSION as i64 {liballoy::process::exit(72)}
    use alloyos_js::process::*;
    let mut request_buffer = alloc::vec![0;MAX_WIRE_BYTES];
    loop {
        let n = unsafe {
            liballoy::syscall::syscall2(
                REQUEST_SYSCALL,
                request_buffer.as_mut_ptr() as u64,
                request_buffer.len() as u64,
            )
        };
        if n == -11 {
            liballoy::process::yield_now();
            continue;
        }
        if n < 0 || n as usize > request_buffer.len() {
            liballoy::process::exit(2)
        }
        let request = match core::str::from_utf8(&request_buffer[..n as usize]) {
            Ok(s) => s,
            Err(_) => liballoy::process::exit(3),
        };
        let mut host = ProcessHost {
            exchange: |wire: &str| {
                let mut response = alloc::vec![0;MAX_WIRE_BYTES];
                loop {
                    let n = unsafe {
                        liballoy::syscall::syscall3(
                            HOST_SYSCALL,
                            wire.as_ptr() as u64,
                            wire.len() as u64,
                            response.as_mut_ptr() as u64,
                        )
                    };
                    if n == -11 {
                        liballoy::process::yield_now();
                        continue;
                    }
                    if n < 0 {
                        return Err(alloc::format!("Host operation denied ({n})"));
                    }
                    if n as usize > response.len() {
                        return Err("Oversized host response".into());
                    }
                    return alloc::string::String::from_utf8(response[..n as usize].to_vec())
                        .map_err(|_| "Invalid host response".into());
                }
            },
        };
        let (result, status) = match execute_request(request, &mut host) {
            Ok(r) => (r.text, 0),
            Err(e) => (alloc::format!("{e}"), 1),
        };
        unsafe {
            liballoy::syscall::syscall3(
                RESULT_SYSCALL,
                result.as_ptr() as u64,
                result.len() as u64,
                status,
            )
        };
    }
}
#[cfg(not(target_os = "none"))]
fn main() {
    eprintln!("app-runner is an alloyOS EL0 executable");
}
