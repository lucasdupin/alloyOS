//! Initial protected-process ABI. Handles are granted by the kernel supervisor;
//! unavailable handles return EPERM. Calls are nonblocking (EAGAIN on empty/full).
pub const ABI_VERSION: u64 = 1;
pub const IPC_SEND: u64 = 400;
pub const IPC_RECEIVE: u64 = 401;
pub const QUERY_ABI_VERSION: u64 = 402;
// x0 = local endpoint handle, x1 = buffer, x2 = byte length/capacity.
// Send returns payload length. Receive returns bytes copied, beginning with an
// eight-byte little-endian kernel-derived sender PID followed by the payload.
// An undersized/invalid receive buffer leaves the queued message intact.

/// Report at most 1024 bytes from the caller's private memory before trapping.
/// x0 = message pointer, x1 = length. This grants no descriptor access.
pub const PANIC_REPORT: u64 = 403;

/// Private anonymous page reservation. x0 = nonzero page-aligned byte length.
/// Returns an inaccessible address or a negative error; the supervisor's process
/// page quota includes static memory and all outstanding reservations.
pub const MEMORY_RESERVE: u64 = 404;
/// Commit zeroed private pages or change existing pages' read/write permission.
/// x0 = address, x1 = page-aligned length, x2 = MEMORY_READ or MEMORY_READ_WRITE.
/// Existing committed bytes are preserved; executable memory is unsupported.
pub const MEMORY_COMMIT: u64 = 405;
/// Discard backing and make a reservation interval inaccessible. x0/x1 = range.
/// Recommit returns zeroed memory. The virtual reservation remains charged.
pub const MEMORY_DECOMMIT: u64 = 406;
/// Release exactly one whole reservation (x0 address, x1 length).
pub const MEMORY_RELEASE: u64 = 407;
pub const MEMORY_READ: u64 = 1;
pub const MEMORY_READ_WRITE: u64 = 3;

/// Opted-in native services only. x0 = pointer to 48-byte little-endian descriptor,
/// x1 = 48, x2 = 0. Six u64 fields: entry, argument, stack base, stack length,
/// thread pointer, reserved zero. Stack must be private RW, page-aligned, at least
/// 16 KiB and disjoint from live thread stacks. Returns a unique thread ID.
pub const THREAD_CREATE: u64 = 408;
/// Retire only this thread (x0 = signed i32 status). Process Exit still ends all
/// threads. The entry trampoline must call this rather than return to address 0.
pub const THREAD_EXIT: u64 = 409;
/// Nonblocking join: x0 = child TID, x1 = writable i32 status pointer, x2 = 0.
/// EAGAIN while running, successful reap only after kernel context retirement.
pub const THREAD_JOIN: u64 = 410;
/// Atomically compare a private aligned u32 and sleep if equal. x0 = address,
/// x1 = expected u32, x2 = absolute monotonic ns deadline (0 = no timeout).
/// Returns EAGAIN on mismatch, ETIMEDOUT on deadline, zero after notification.
/// Wakes are advisory: callers must recheck their atomic synchronization word.
pub const WORD_WAIT: u64 = 411;
/// Notify at most x1 waiters on private word address x0, x2 = 0.
/// Wait keys are scoped to process identity; returns number made runnable.
pub const WORD_WAKE: u64 = 412;
/// Return the current unique thread ID; Getpid returns the process identity.
pub const GETTID: u64 = 413;
pub const THREAD_DESCRIPTOR_BYTES: usize = 48;
