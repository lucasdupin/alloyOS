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
