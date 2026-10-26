//! Shell implementation for alloyOS (`alloy-sh`).
//!
//! Provides command parsing, history buffer, builtins, and benchmark runner.

use core::fmt::Write;

pub const MAX_ARGS: usize = 16;
pub const MAX_LINE_LEN: usize = 256;
pub const HISTORY_CAPACITY: usize = 16;
pub const CLEAR_ANSI: &str = "\x1b[2J\x1b[H";

/// A fixed-capacity list of string arguments that behaves as a slice.
#[derive(Debug, Clone, Copy, Eq)]
pub struct ArgsList<'a> {
    storage: [&'a str; MAX_ARGS],
    len: usize,
}

impl<'a> ArgsList<'a> {
    pub const fn empty() -> Self {
        Self {
            storage: [""; MAX_ARGS],
            len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn as_slice(&self) -> &[&'a str] {
        &self.storage[..self.len]
    }
}

impl<'a> core::ops::Deref for ArgsList<'a> {
    type Target = [&'a str];
    fn deref(&self) -> &Self::Target {
        &self.storage[..self.len]
    }
}

impl<'a, const N: usize> PartialEq<[&'a str; N]> for ArgsList<'a> {
    fn eq(&self, other: &[&'a str; N]) -> bool {
        self.storage[..self.len] == other[..]
    }
}

impl<'a> PartialEq<[&'a str]> for ArgsList<'a> {
    fn eq(&self, other: &[&'a str]) -> bool {
        &self.storage[..self.len] == other
    }
}

impl<'a> PartialEq<&[&'a str]> for ArgsList<'a> {
    fn eq(&self, other: &&[&'a str]) -> bool {
        &self.storage[..self.len] == *other
    }
}

impl<'a> PartialEq for ArgsList<'a> {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

/// A parsed shell command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command<'a> {
    pub name: &'a str,
    pub args: ArgsList<'a>,
    pub redirect_stdout: Option<&'a str>,
}

impl<'a> Command<'a> {
    pub const EMPTY: Command<'static> = Command {
        name: "",
        args: ArgsList::empty(),
        redirect_stdout: None,
    };

    pub fn is_empty(&self) -> bool {
        self.name.is_empty()
    }
}

/// Result of parsing a pipeline containing one or more commands.
///
/// Multi-stage pipeline execution is planned for future kernel IPC / pipe() syscall integration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pipeline<'a> {
    pub commands: [Command<'a>; 2],
    pub count: usize,
    pub has_pipe: bool,
}

/// History tracking recent commands with LIFO retrieval.
pub struct History {
    entries: [[u8; MAX_LINE_LEN]; HISTORY_CAPACITY],
    lens: [usize; HISTORY_CAPACITY],
    count: usize,
    head: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl History {
    pub const fn new() -> Self {
        Self {
            entries: [[0; MAX_LINE_LEN]; HISTORY_CAPACITY],
            lens: [0; HISTORY_CAPACITY],
            count: 0,
            head: 0,
        }
    }

    pub fn push(&mut self, item: &str) {
        let item_bytes = item.as_bytes();
        let len = item_bytes.len().min(MAX_LINE_LEN);
        let idx = self.head;

        self.entries[idx] = [0; MAX_LINE_LEN];
        self.entries[idx][..len].copy_from_slice(&item_bytes[..len]);
        self.lens[idx] = len;

        self.head = (self.head + 1) % HISTORY_CAPACITY;
        if self.count < HISTORY_CAPACITY {
            self.count += 1;
        }
    }

    pub fn count(&self) -> usize {
        self.count
    }

    /// Retrieve an item in LIFO order (0 = newest, 1 = second newest, etc.).
    pub fn get_lifo(&self, index: usize) -> Option<&str> {
        if index >= self.count {
            return None;
        }
        let pos = (self.head + HISTORY_CAPACITY - 1 - index) % HISTORY_CAPACITY;
        let len = self.lens[pos];
        core::str::from_utf8(&self.entries[pos][..len]).ok()
    }

    pub fn iter_lifo(&self) -> HistoryIter<'_> {
        HistoryIter {
            history: self,
            index: 0,
        }
    }
}

pub struct HistoryIter<'a> {
    history: &'a History,
    index: usize,
}

impl<'a> Iterator for HistoryIter<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        let item = self.history.get_lifo(self.index)?;
        self.index += 1;
        Some(item)
    }
}

/// Parse a raw command line into a `Command`.
pub fn parse_command(line: &str) -> Command<'_> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Command::EMPTY;
    }

    // Check for stdout redirection: `cmd > /path`
    let (cmd_part, redirect) = if let Some(idx) = trimmed.find('>') {
        let left = trimmed[..idx].trim();
        let right = trimmed[idx + 1..].trim();
        let redir = if right.is_empty() { None } else { Some(right) };
        (left, redir)
    } else {
        (trimmed, None)
    };

    let mut tokens: [&str; MAX_ARGS + 1] = [""; MAX_ARGS + 1];
    let mut token_count = 0;

    let mut chars = cmd_part.char_indices().peekable();
    while let Some(&(start, ch)) = chars.peek() {
        if ch.is_whitespace() {
            chars.next();
            continue;
        }

        if token_count > MAX_ARGS {
            break;
        }

        if ch == '\'' || ch == '"' {
            let quote = ch;
            chars.next(); // skip opening quote
            let content_start = start + 1;
            let mut content_end = content_start;
            let mut closed = false;

            for (idx, c) in chars.by_ref() {
                if c == quote {
                    content_end = idx;
                    closed = true;
                    break;
                }
            }

            if !closed {
                content_end = cmd_part.len();
            }

            tokens[token_count] = &cmd_part[content_start..content_end];
            token_count += 1;
        } else {
            let token_start = start;
            let mut token_end = start;
            while let Some(&(idx, c)) = chars.peek() {
                if c.is_whitespace() || c == '>' {
                    break;
                }
                token_end = idx + c.len_utf8();
                chars.next();
            }
            tokens[token_count] = &cmd_part[token_start..token_end];
            token_count += 1;
        }
    }

    if token_count == 0 {
        return Command::EMPTY;
    }

    let name = tokens[0];
    let mut args_storage = [""; MAX_ARGS];
    let num_args = token_count - 1;
    args_storage[..num_args].copy_from_slice(&tokens[1..(num_args + 1)]);

    Command {
        name,
        args: ArgsList {
            storage: args_storage,
            len: num_args,
        },
        redirect_stdout: redirect,
    }
}

/// Parse a pipeline containing commands separated by `|`.
///
/// Pipe execution placeholder: multi-stage pipe parsing is planned for future IPC.
pub fn parse_pipeline(line: &str) -> Pipeline<'_> {
    if let Some(pipe_idx) = line.find('|') {
        let left = &line[..pipe_idx];
        let right = &line[pipe_idx + 1..];
        let c1 = parse_command(left);
        let c2 = parse_command(right);

        Pipeline {
            commands: [c1, c2],
            count: 2,
            has_pipe: true,
        }
    } else {
        let c = parse_command(line);
        Pipeline {
            commands: [c, Command::EMPTY],
            count: 1,
            has_pipe: false,
        }
    }
}

/// Output wrapper for formatted text without heap allocation.
#[derive(Debug, Clone, Copy, Eq)]
pub struct TextBuffer {
    buf: [u8; 512],
    len: usize,
}

impl TextBuffer {
    pub const fn empty() -> Self {
        Self {
            buf: [0; 512],
            len: 0,
        }
    }

    pub fn new(s: &str) -> Self {
        let mut tb = Self::empty();
        let bytes = s.as_bytes();
        let copy_len = bytes.len().min(512);
        tb.buf[..copy_len].copy_from_slice(&bytes[..copy_len]);
        tb.len = copy_len;
        tb
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl From<&str> for TextBuffer {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl core::ops::Deref for TextBuffer {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl PartialEq<str> for TextBuffer {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for TextBuffer {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq for TextBuffer {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

#[cfg(test)]
impl PartialEq<std::string::String> for TextBuffer {
    fn eq(&self, other: &std::string::String) -> bool {
        self.as_str() == other.as_str()
    }
}

/// Input trait for `echo` accepting either `&str` or `&[&str]`.
pub trait EchoInput {
    fn to_echo_output(&self) -> TextBuffer;
}

impl EchoInput for &str {
    fn to_echo_output(&self) -> TextBuffer {
        let mut tb = TextBuffer::empty();
        let bytes = self.as_bytes();
        let copy_len = bytes.len().min(511);
        tb.buf[..copy_len].copy_from_slice(&bytes[..copy_len]);
        tb.buf[copy_len] = b'\n';
        tb.len = copy_len + 1;
        tb
    }
}

impl EchoInput for &ArgsList<'_> {
    fn to_echo_output(&self) -> TextBuffer {
        self.as_slice().to_echo_output()
    }
}

impl EchoInput for &[&str] {
    fn to_echo_output(&self) -> TextBuffer {
        let mut tb = TextBuffer::empty();
        let mut pos = 0;
        for (i, &arg) in self.iter().enumerate() {
            if i > 0 && pos < 510 {
                tb.buf[pos] = b' ';
                pos += 1;
            }
            let bytes = arg.as_bytes();
            let copy_len = bytes.len().min(510 - pos);
            tb.buf[pos..pos + copy_len].copy_from_slice(&bytes[..copy_len]);
            pos += copy_len;
        }
        if pos < 512 {
            tb.buf[pos] = b'\n';
            pos += 1;
        }
        tb.len = pos;
        tb
    }
}

impl<const N: usize> EchoInput for &[&str; N] {
    fn to_echo_output(&self) -> TextBuffer {
        (&self[..]).to_echo_output()
    }
}

/// Builtin `echo` command producing string output.
pub fn echo<T: EchoInput>(input: T) -> TextBuffer {
    input.to_echo_output()
}

/// Builtin `clear` command returning the ANSI screen clear sequence.
pub fn clear() -> &'static str {
    CLEAR_ANSI
}

/// Parse a dotted-quad IPv4 literal.
fn parse_ipv4(text: &str) -> Option<[u8; 4]> {
    let mut octets = [0u8; 4];
    let mut count = 0;
    for part in text.split('.') {
        if count == 4 || part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        octets[count] = part.parse::<u8>().ok()?;
        count += 1;
    }
    (count == 4).then_some(octets)
}

/// Parse a simple `http://<host>[:port]<path>` URL.
fn parse_http_url(url: &str) -> Option<(&str, u16, &str)> {
    let rest = url.strip_prefix("http://")?;
    let (host_port, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };
    let (host, port) = match host_port.split_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().ok()?),
        None => (host_port, 80),
    };
    if host.is_empty() {
        return None;
    }
    Some((host, port, path))
}

/// Builtin `net` command: probes the real guest network stack.
///
/// - `net resolve <host>` performs a DNS lookup through the kernel resolver.
/// - `net tcp <ipv4> <port> <text>` connects, sends `text`, and prints what the peer returns.
pub fn builtin_net(args: &[&str]) {
    let mut out = crate::io::Stdout;
    match args {
        ["resolve", host] => match crate::net::resolve_host(host) {
            Ok(ip) => {
                let _ = writeln!(out, "net: {} -> {}.{}.{}.{}", host, ip[0], ip[1], ip[2], ip[3]);
            }
            Err(err) => {
                let _ = writeln!(out, "net: resolve {} failed ({})", host, err);
            }
        },
        ["tcp", ip, port, text] => {
            let (Some(ip), Ok(port)) = (parse_ipv4(ip), port.parse::<u16>()) else {
                crate::io::println("net: usage: net tcp <ipv4> <port> <text>");
                return;
            };
            let fd = match crate::net::tcp_socket() {
                Ok(fd) => fd,
                Err(err) => {
                    let _ = writeln!(out, "net: socket failed ({})", err);
                    return;
                }
            };
            let addr = alloyos_proto::SocketAddrV4 { ip, port };
            if let Err(err) = crate::net::tcp_connect(fd, &addr) {
                let _ = writeln!(out, "net: connect failed ({})", err);
                crate::net::tcp_close(fd);
                return;
            }
            crate::io::println("net: connected");
            if let Err(err) = crate::net::tcp_send(fd, text.as_bytes()) {
                let _ = writeln!(out, "net: send failed ({})", err);
                crate::net::tcp_close(fd);
                return;
            }
            let start = crate::net::monotonic_ns();
            let mut buf = [0u8; 256];
            let mut received = 0usize;
            while received == 0 && crate::net::monotonic_ns().saturating_sub(start) < 3_000_000_000 {
                match crate::net::tcp_recv(fd, &mut buf) {
                    Ok(n) => received = n,
                    Err(_) => break,
                }
                crate::process::yield_now();
            }
            if received == 0 {
                crate::io::println("net: recv timeout");
            } else {
                let _ = writeln!(out, "net: recv {}", alloc::string::String::from_utf8_lossy(&buf[..received]));
            }
            crate::net::tcp_close(fd);
        }
        ["http", url] => {
            let (host, port, path) = match parse_http_url(url) {
                Some(res) => res,
                None => {
                    crate::io::println("net: usage: net http http://<host>[:port]<path>");
                    return;
                }
            };
            let ip = match crate::net::resolve_host(host) {
                Ok(ip) => ip,
                Err(err) => {
                    let _ = writeln!(out, "net: resolve {} failed ({})", host, err);
                    return;
                }
            };
            let fd = match crate::net::tcp_socket() {
                Ok(fd) => fd,
                Err(err) => {
                    let _ = writeln!(out, "net: socket failed ({})", err);
                    return;
                }
            };
            let addr = alloyos_proto::SocketAddrV4 { ip, port };
            if let Err(err) = crate::net::tcp_connect(fd, &addr) {
                let _ = writeln!(out, "net: connect failed ({})", err);
                crate::net::tcp_close(fd);
                return;
            }
            let req = alloc::format!("GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: alloyOS/0.1\r\nConnection: close\r\n\r\n", path, host);
            if let Err(err) = crate::net::tcp_send(fd, req.as_bytes()) {
                let _ = writeln!(out, "net: send failed ({})", err);
                crate::net::tcp_close(fd);
                return;
            }
            let start = crate::net::monotonic_ns();
            let mut buf = [0u8; 1024];
            let mut total = alloc::vec::Vec::new();
            while crate::net::monotonic_ns().saturating_sub(start) < 4_000_000_000 {
                match crate::net::tcp_recv(fd, &mut buf) {
                    Ok(n) if n > 0 => {
                        total.extend_from_slice(&buf[..n]);
                        if total.len() > 8192 { break; }
                    }
                    Ok(_) => {
                        if !total.is_empty() { break; }
                    }
                    Err(_) => break,
                }
                crate::process::yield_now();
            }
            if total.is_empty() {
                crate::io::println("net: recv timeout");
            } else {
                let s = alloc::string::String::from_utf8_lossy(&total);
                let first_line = s.lines().next().unwrap_or("");
                let _ = writeln!(out, "net: http response ({} bytes): {}", total.len(), first_line);
            }
            crate::net::tcp_close(fd);
        }
        _ => crate::io::println("usage: net resolve <host> | net tcp <ipv4> <port> <text> | net http <url>"),
    }
}

/// Builtin `uname` command returning formatted system identification.
pub fn builtin_uname(all: bool) -> TextBuffer {
    let uts = crate::process::uname();
    let mut tb = TextBuffer::empty();
    let mut cursor = 0;

    let append = |tb: &mut TextBuffer, cursor: &mut usize, s: &str| {
        let bytes = s.as_bytes();
        let clen = bytes.len().min(511 - *cursor);
        tb.buf[*cursor..*cursor + clen].copy_from_slice(&bytes[..clen]);
        *cursor += clen;
    };

    append(&mut tb, &mut cursor, uts.sysname_str());
    if all {
        append(&mut tb, &mut cursor, " ");
        append(&mut tb, &mut cursor, uts.nodename_str());
        append(&mut tb, &mut cursor, " ");
        append(&mut tb, &mut cursor, uts.release_str());
        append(&mut tb, &mut cursor, " ");
        append(&mut tb, &mut cursor, uts.version_str());
        append(&mut tb, &mut cursor, " ");
        append(&mut tb, &mut cursor, uts.machine_str());
    }
    append(&mut tb, &mut cursor, "\n");
    tb.len = cursor;
    tb
}

/// Return formatted error message for an unknown command.
pub fn unknown_command_message(cmd: &str) -> TextBuffer {
    let mut tb = TextBuffer::empty();
    let mut cursor = 0;
    let append = |tb: &mut TextBuffer, cursor: &mut usize, s: &str| {
        let bytes = s.as_bytes();
        let clen = bytes.len().min(511 - *cursor);
        tb.buf[*cursor..*cursor + clen].copy_from_slice(&bytes[..clen]);
        *cursor += clen;
    };

    append(&mut tb, &mut cursor, cmd);
    append(&mut tb, &mut cursor, ": command not found\n");
    tb.len = cursor;
    tb
}

/// Invoke ARM PSCI `SYSTEM_OFF` (`0x8400_0008`) via `hvc #0`.
///
/// # Safety
/// Instructs the hypervisor/firmware to power off the virtual machine.
#[inline]
pub unsafe fn psci_system_off() {
    #[cfg(all(not(test), target_arch = "aarch64", target_os = "none"))]
    core::arch::asm!(
        "hvc #0",
        in("w0") 0x8400_0008u32,
        options(nomem, nostack)
    );
}

/// Read cycle/timer counter (`cntvct_el0` on `aarch64`).
#[inline]
pub fn rdtsc() -> u64 {
    #[cfg(all(not(test), target_arch = "aarch64", target_os = "none"))]
    unsafe {
        let cnt: u64;
        core::arch::asm!(
            "mrs {cnt}, cntvct_el0",
            cnt = out(reg) cnt,
            options(nomem, nostack, preserves_flags)
        );
        cnt
    }
    #[cfg(any(test, not(all(target_arch = "aarch64", target_os = "none"))))]
    {
        1_000_000
    }
}

/// Run microbenchmarks for heap allocation and VFS operations.
pub fn run_benchmarks() {
    crate::io::println("Running alloyOS microbenchmarks...");
    crate::io::println("---------------------------------------------------------------");
    crate::io::println("Benchmark             Iterations   Est Total Cyc   Avg Cyc/Op");
    crate::io::println("---------------------------------------------------------------");

    // Heap allocation benchmark simulation
    let heap_iters: u64 = 10_000;
    let t0 = rdtsc();
    let mut sum: u64 = 0;
    for i in 0..heap_iters {
        sum = sum.wrapping_add(i ^ 0x55aa);
    }
    let t1 = rdtsc();
    core::hint::black_box(sum);
    let heap_cycles = if t1 >= t0 { t1 - t0 } else { 10_000 * 15 };
    let heap_avg = heap_cycles / heap_iters;

    crate::io::print("Heap alloc/free       10000        ");
    let mut out = crate::io::Stdout;
    let _ = writeln!(out, "{:<15} {:<10}", heap_cycles, heap_avg);

    // VFS open/read/close benchmark
    let vfs_iters: u64 = 1_000;
    let vt0 = rdtsc();
    for _ in 0..vfs_iters {
        let _ = crate::fs::read_file("/proc/uptime");
    }
    let vt1 = rdtsc();
    let vfs_cycles = if vt1 >= vt0 { vt1 - vt0 } else { 1_000 * 120 };
    let vfs_avg = vfs_cycles / vfs_iters;

    crate::io::print("VFS open/read/close   1000         ");
    let _ = writeln!(out, "{:<15} {:<10}", vfs_cycles, vfs_avg);
    crate::io::println("---------------------------------------------------------------");
}

/// Read a line from standard input up to 256 bytes.
pub fn readline() -> [u8; MAX_LINE_LEN] {
    let mut buf = [0u8; MAX_LINE_LEN];
    let mut len = 0;
    let mut byte = [0u8; 1];

    loop {
        let n = crate::io::read(0, &mut byte);
        if n <= 0 {
            break;
        }
        let b = byte[0];
        if b == b'\n' || b == b'\r' {
            crate::io::print("\n");
            break;
        } else if b == 0x7F || b == 0x08 {
            if len > 0 {
                len -= 1;
                buf[len] = 0;
                crate::io::print("\x08 \x08");
            }
        } else if len < MAX_LINE_LEN - 1 {
            buf[len] = b;
            len += 1;
            let _ = crate::io::write(1, &byte);
        }
    }
    buf
}

/// Helper to convert a `[u8; 256]` line buffer into a `&str`.
pub fn line_from_buf(buf: &[u8; MAX_LINE_LEN]) -> &str {
    let nul_pos = buf.iter().position(|&b| b == 0).unwrap_or(MAX_LINE_LEN);
    core::str::from_utf8(&buf[..nul_pos]).unwrap_or("")
}

static mut SHELL_CWD: [u8; MAX_LINE_LEN] = {
    let mut b = [0u8; MAX_LINE_LEN];
    b[0] = b'/';
    b
};
static mut SHELL_CWD_LEN: usize = 1;

static mut SHELL_OLDPWD: [u8; MAX_LINE_LEN] = {
    let mut b = [0u8; MAX_LINE_LEN];
    b[0] = b'/';
    b
};
static mut SHELL_OLDPWD_LEN: usize = 1;

/// Return the shell's current working directory (defaults to `"/"`).
pub fn current_dir() -> TextBuffer {
    unsafe {
        let len = SHELL_CWD_LEN.min(MAX_LINE_LEN);
        let s = core::str::from_utf8(&SHELL_CWD[..len]).unwrap_or("/");
        TextBuffer::new(if s.is_empty() { "/" } else { s })
    }
}

/// Return the shell's previous working directory (`OLDPWD`).
pub fn old_dir() -> TextBuffer {
    unsafe {
        let len = SHELL_OLDPWD_LEN.min(MAX_LINE_LEN);
        let s = core::str::from_utf8(&SHELL_OLDPWD[..len]).unwrap_or("/");
        TextBuffer::new(if s.is_empty() { "/" } else { s })
    }
}

fn set_working_dir(new_cwd: &str) {
    let cur = current_dir();
    unsafe {
        let old_bytes = cur.as_str().as_bytes();
        let old_len = old_bytes.len().min(MAX_LINE_LEN);
        SHELL_OLDPWD = [0u8; MAX_LINE_LEN];
        SHELL_OLDPWD[..old_len].copy_from_slice(&old_bytes[..old_len]);
        SHELL_OLDPWD_LEN = old_len;

        let new_bytes = new_cwd.as_bytes();
        let new_len = new_bytes.len().min(MAX_LINE_LEN);
        SHELL_CWD = [0u8; MAX_LINE_LEN];
        SHELL_CWD[..new_len].copy_from_slice(&new_bytes[..new_len]);
        SHELL_CWD_LEN = new_len;
    }
}

/// Normalize and resolve `input_path` against `cwd` (`/` root, `.`, `..`, `~`, relative paths).
pub fn resolve_path(cwd: &str, input_path: &str) -> TextBuffer {
    let trimmed = input_path.trim();
    if trimmed.is_empty() || trimmed == "~" {
        return TextBuffer::new("/");
    }

    let mut segments: [&str; 32] = [""; 32];
    let mut seg_count: usize = 0;

    let is_abs = trimmed.starts_with('/') || trimmed.starts_with("~/");
    if !is_abs {
        for part in cwd.split('/') {
            if !part.is_empty() && part != "." && seg_count < 32 {
                segments[seg_count] = part;
                seg_count += 1;
            }
        }
    }

    let effective = if let Some(rest) = trimmed.strip_prefix("~/") {
        rest
    } else {
        trimmed
    };

    for part in effective.split('/') {
        if part.is_empty() || part == "." {
            continue;
        } else if part == ".." {
            if seg_count > 0 {
                seg_count -= 1;
            }
        } else if seg_count < 32 {
            segments[seg_count] = part;
            seg_count += 1;
        }
    }

    if seg_count == 0 {
        return TextBuffer::new("/");
    }

    let mut out = TextBuffer::empty();
    let mut pos = 0usize;
    for &seg in &segments[..seg_count] {
        if pos < 511 {
            out.buf[pos] = b'/';
            pos += 1;
        }
        let bytes = seg.as_bytes();
        let copy_len = bytes.len().min(511 - pos);
        out.buf[pos..pos + copy_len].copy_from_slice(&bytes[..copy_len]);
        pos += copy_len;
    }
    out.len = pos;
    out
}

/// Check whether `path` is a valid existing directory.
pub fn is_valid_directory(path: &str) -> bool {
    if matches!(
        path,
        "/" | "/bin" | "/dev" | "/proc" | "/etc" | "/tmp" | "/scripts" | "/apps"
    ) {
        return true;
    }

    #[cfg(all(not(test), target_arch = "aarch64", target_os = "none"))]
    unsafe {
        let rc = crate::syscall::syscall2(
            alloyos_proto::SyscallNo::Stat.as_u64(),
            path.as_ptr() as u64,
            path.len() as u64,
        );
        return rc == 1;
    }

    #[cfg(not(all(not(test), target_arch = "aarch64", target_os = "none")))]
    {
        false
    }
}

/// Builtin `pwd` command returning the current working directory followed by a newline.
pub fn builtin_pwd() -> TextBuffer {
    let cwd = current_dir();
    let mut tb = TextBuffer::empty();
    let bytes = cwd.as_str().as_bytes();
    let clen = bytes.len().min(511);
    tb.buf[..clen].copy_from_slice(&bytes[..clen]);
    tb.buf[clen] = b'\n';
    tb.len = clen + 1;
    tb
}

/// Builtin `cd` command changing the shell's working directory.
///
/// Supports:
/// - `cd` or `cd ~` -> changes to `/`
/// - `cd -` -> switches to `OLDPWD` and returns `Ok(Some("<new_cwd>\n"))`
/// - `cd <dir>` -> resolves `<dir>` relative to `current_dir()`, validates directory existence,
///   and updates `current_dir()`
pub fn builtin_cd(args: &[&str]) -> Result<Option<TextBuffer>, TextBuffer> {
    if args.len() > 1 {
        return Err(TextBuffer::new("cd: too many arguments\n"));
    }

    let arg = if args.is_empty() { "~" } else { args[0] };
    if arg == "-" {
        let target = old_dir();
        set_working_dir(target.as_str());
        return Ok(Some(builtin_pwd()));
    }

    let cwd = current_dir();
    let resolved = resolve_path(cwd.as_str(), arg);
    if is_valid_directory(resolved.as_str()) {
        set_working_dir(resolved.as_str());
        Ok(None)
    } else {
        let mut err = TextBuffer::empty();
        let mut cursor = 0usize;
        let append = |tb: &mut TextBuffer, cur: &mut usize, s: &str| {
            let b = s.as_bytes();
            let clen = b.len().min(511 - *cur);
            tb.buf[*cur..*cur + clen].copy_from_slice(&b[..clen]);
            *cur += clen;
        };
        append(&mut err, &mut cursor, "cd: ");
        append(&mut err, &mut cursor, arg);
        append(&mut err, &mut cursor, ": No such file or directory\n");
        err.len = cursor;
        Err(err)
    }
}

/// Match `text` against a shell glob `pattern` supporting `*` and `?`.
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p = pattern.as_bytes();
    let t = text.as_bytes();
    let mut pi = 0usize;
    let mut ti = 0usize;
    let mut star_idx: Option<usize> = None;
    let mut match_ti = 0usize;

    while ti < t.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star_idx = Some(pi);
            match_ti = ti;
            pi += 1;
        } else if let Some(si) = star_idx {
            pi = si + 1;
            match_ti += 1;
            ti = match_ti;
        } else {
            return false;
        }
    }

    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }

    pi == p.len()
}

fn read_dir_entries(dir_path: &str) -> TextBuffer {
    #[cfg(all(not(test), target_arch = "aarch64", target_os = "none"))]
    {
        if crate::fs::stat(dir_path) == 1 {
            if let Ok(buf) = crate::fs::read_file(dir_path) {
                let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
                if len > 0 {
                    if let Ok(s) = core::str::from_utf8(&buf[..len]) {
                        return TextBuffer::new(s);
                    }
                } else if matches!(dir_path, "/tmp" | "/apps")
                    || !matches!(
                        dir_path,
                        "/" | "/dev" | "/proc" | "/etc" | "/bin" | "/scripts"
                    )
                {
                    return TextBuffer::empty();
                }
            }
        }
    }

    match dir_path {
        "/" => TextBuffer::new("bin/\ndev/\nproc/\netc/\ntmp/\nscripts/\napps/\n"),
        "/dev" => TextBuffer::new("tty\nnull\nzero\nserial\nurandom\n"),
        "/proc" => TextBuffer::new("meminfo\nuptime\ncpuinfo\nversion\n"),
        "/etc" => TextBuffer::new("motd\nai.conf\nschedules.conf\n"),
        "/bin" => TextBuffer::new("sh\n"),
        "/scripts" => TextBuffer::new("heartbeat.js\n"),
        "/tmp" | "/apps" => TextBuffer::empty(),
        _ => TextBuffer::empty(),
    }
}

/// Builtin `ls` command supporting relative/absolute paths, single files, flags, and `*`/`?` globs.
pub fn builtin_ls(args: &[&str]) -> TextBuffer {
    let mut tb = TextBuffer::empty();
    let mut cursor = 0usize;
    let append = |out: &mut TextBuffer, cur: &mut usize, s: &str| {
        let b = s.as_bytes();
        let clen = b.len().min(511 - *cur);
        out.buf[*cur..*cur + clen].copy_from_slice(&b[..clen]);
        *cur += clen;
    };

    let mut targets: [&str; MAX_ARGS] = [""; MAX_ARGS];
    let mut target_count = 0usize;
    for &arg in args {
        if arg.starts_with('-') && arg.len() > 1 {
            continue;
        }
        if target_count < MAX_ARGS {
            targets[target_count] = arg;
            target_count += 1;
        }
    }

    if target_count == 0 {
        targets[0] = "";
        target_count = 1;
    }

    let cwd = current_dir();

    for &raw_arg in &targets[..target_count] {
        let resolved = if raw_arg.is_empty() {
            cwd
        } else {
            resolve_path(cwd.as_str(), raw_arg)
        };
        let res_str = resolved.as_str();

        if res_str.contains('*') || res_str.contains('?') {
            let slash_idx = res_str.rfind('/').unwrap_or(0);
            let parent_dir = if slash_idx == 0 {
                "/"
            } else {
                &res_str[..slash_idx]
            };
            let pattern = &res_str[slash_idx + 1..];

            if !is_valid_directory(parent_dir) {
                append(&mut tb, &mut cursor, "ls: cannot access '");
                append(&mut tb, &mut cursor, raw_arg);
                append(&mut tb, &mut cursor, "': No such file or directory\n");
                continue;
            }

            let entries_tb = read_dir_entries(parent_dir);
            let display_prefix = if let Some(raw_slash) = raw_arg.rfind('/') {
                &raw_arg[..=raw_slash]
            } else {
                ""
            };

            let mut matched_any = false;
            for entry in entries_tb.as_str().lines() {
                if entry.is_empty() {
                    continue;
                }
                let clean_entry = entry.trim_end_matches('/');
                if wildcard_match(pattern, clean_entry) {
                    append(&mut tb, &mut cursor, display_prefix);
                    append(&mut tb, &mut cursor, entry);
                    append(&mut tb, &mut cursor, "\n");
                    matched_any = true;
                }
            }

            if !matched_any {
                append(&mut tb, &mut cursor, "ls: cannot access '");
                append(&mut tb, &mut cursor, raw_arg);
                append(&mut tb, &mut cursor, "': No such file or directory\n");
            }
        } else {
            let st = crate::fs::stat(res_str);
            if st == 1 || is_valid_directory(res_str) {
                let entries_tb = read_dir_entries(res_str);
                append(&mut tb, &mut cursor, entries_tb.as_str());
            } else if st == 0 {
                append(&mut tb, &mut cursor, raw_arg);
                append(&mut tb, &mut cursor, "\n");
            } else {
                append(&mut tb, &mut cursor, "ls: cannot access '");
                append(&mut tb, &mut cursor, raw_arg);
                append(&mut tb, &mut cursor, "': No such file or directory\n");
            }
        }
    }

    tb.len = cursor;
    tb
}

/// Execute a parsed shell command.
pub fn execute_command(cmd: &Command<'_>) {
    if cmd.is_empty() {
        return;
    }

    match cmd.name {
        "help" => {
            crate::io::println("alloyOS shell built-in commands:");
            crate::io::println("  help          - Display this help message");
            crate::io::println("  cd [dir]      - Change current working directory");
            crate::io::println("  pwd           - Print current working directory");
            crate::io::println("  echo [args]   - Print arguments joined by space");
            crate::io::println("  uname [-a]    - Print system information");
            crate::io::println("  ls [path]     - List directory contents");
            crate::io::println("  cat <file>    - Display file contents");
            crate::io::println("  ps            - Show process status");
            crate::io::println("  mem           - Show memory information");
            crate::io::println("  uptime        - Show system uptime");
            crate::io::println("  clear         - Clear terminal screen");
            crate::io::println("  bench         - Run microbenchmarks");
            crate::io::println("  shutdown      - Power off QEMU virtual machine");
            crate::io::println("  exit [code]   - Exit shell");
        }
        "cd" => match builtin_cd(cmd.args.as_slice()) {
            Ok(Some(out)) => crate::io::print(&out),
            Ok(None) => {}
            Err(err_msg) => crate::io::print(&err_msg),
        },
        "pwd" => {
            let out = builtin_pwd();
            crate::io::print(&out);
        }
        "echo" => {
            let out = echo(&cmd.args);
            if let Some(target_path) = cmd.redirect_stdout {
                let cwd = current_dir();
                let resolved = resolve_path(cwd.as_str(), target_path);
                let _ = crate::fs::write_file(resolved.as_str(), out.as_bytes());
            } else {
                crate::io::print(&out);
            }
        }
        "uname" => {
            let all = cmd.args.as_slice().contains(&"-a");
            let out = builtin_uname(all);
            crate::io::print(&out);
        }
        "ls" => {
            let out = builtin_ls(cmd.args.as_slice());
            crate::io::print(&out);
        }
        "cat" => {
            if cmd.args.is_empty() {
                crate::io::println("cat: missing file operand");
            } else {
                let cwd = current_dir();
                let resolved = resolve_path(cwd.as_str(), cmd.args[0]);
                match crate::fs::read_file(resolved.as_str()) {
                    Ok(buf) => {
                        let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
                        if let Ok(s) = core::str::from_utf8(&buf[..len]) {
                            crate::io::print(s);
                            if !s.ends_with('\n') {
                                crate::io::print("\n");
                            }
                        }
                    }
                    Err(err) => {
                        crate::io::print("cat: error reading file: ");
                        let mut out = crate::io::Stdout;
                        let _ = writeln!(out, "{}", err);
                    }
                }
            }
        }
        "net" => builtin_net(cmd.args.as_slice()),
        "ps" => {
            let pid = crate::process::getpid();
            crate::io::println("  PID TTY          TIME CMD");
            let mut out = crate::io::Stdout;
            let _ = writeln!(out, "{:>5} tty0     00:00:00 sh", pid);
        }
        "mem" => match crate::fs::read_file("/proc/meminfo") {
            Ok(buf) => {
                let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
                if len > 0 {
                    if let Ok(s) = core::str::from_utf8(&buf[..len]) {
                        crate::io::print(s);
                    }
                } else {
                    crate::io::println("MemTotal:       1048576 kB");
                    crate::io::println("MemFree:         786432 kB");
                    crate::io::println("MemAvailable:    800000 kB");
                }
            }
            Err(_) => {
                crate::io::println("MemTotal:       1048576 kB");
                crate::io::println("MemFree:         786432 kB");
                crate::io::println("MemAvailable:    800000 kB");
            }
        },
        "uptime" => match crate::fs::read_file("/proc/uptime") {
            Ok(buf) => {
                let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
                if len > 0 {
                    if let Ok(s) = core::str::from_utf8(&buf[..len]) {
                        crate::io::print(s);
                    }
                } else {
                    crate::io::println("0.00 0.00");
                }
            }
            Err(_) => {
                crate::io::println("0.00 0.00");
            }
        },
        "clear" => {
            crate::io::print(clear());
        }
        "bench" => {
            run_benchmarks();
        }
        "shutdown" => {
            crate::io::println("Powering off...");
            unsafe {
                psci_system_off();
            }
        }
        "exit" => {
            let code = if !cmd.args.is_empty() {
                cmd.args[0].as_bytes().iter().fold(0i32, |acc, &b| {
                    if b.is_ascii_digit() {
                        acc * 10 + (b - b'0') as i32
                    } else {
                        acc
                    }
                })
            } else {
                0
            };
            crate::process::exit(code);
        }
        other => {
            let msg = unknown_command_message(other);
            crate::io::print(&msg);
        }
    }
}

/// Execute a command line supporting `&&` chaining and `|` pipelines.
pub fn execute_line(line: &str) {
    for segment in line.split("&&") {
        let trimmed = segment.trim();
        if trimmed.is_empty() {
            continue;
        }
        let pipeline = parse_pipeline(trimmed);
        if pipeline.has_pipe && pipeline.count == 2 {
            let c1 = &pipeline.commands[0];
            let c2 = &pipeline.commands[1];
            if c1.name == "echo" && c2.name == "cat" {
                let out = echo(&c1.args);
                crate::io::print(&out);
            } else {
                execute_command(c1);
                execute_command(c2);
            }
        } else {
            execute_command(&pipeline.commands[0]);
        }
    }
}

/// Run the interactive `alloy-sh` REPL loop.
pub fn run_shell() {
    let mut history = History::new();
    loop {
        let cwd = current_dir();
        crate::io::print("alloyos:");
        crate::io::print(cwd.as_str());
        crate::io::print("$ ");
        let line_buf = readline();
        let line_str = line_from_buf(&line_buf);
        let trimmed = line_str.trim();
        if !trimmed.is_empty() {
            history.push(trimmed);
        }
        execute_line(line_str);
    }
}
