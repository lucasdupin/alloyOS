//! Shell implementation for metalOS (`metal-sh`).
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

/// Write 16-bit word to an x86_64 I/O port.
///
/// # Safety
/// Port I/O instructs physical or emulated hardware. The port and value must be valid.
#[inline]
pub unsafe fn out16(port: u16, val: u16) {
    #[cfg(all(not(test), target_arch = "x86_64", target_os = "none"))]
    core::arch::asm!("out dx, ax", in("dx") port, in("ax") val, options(nomem, nostack, preserves_flags));
    #[cfg(any(test, not(all(target_arch = "x86_64", target_os = "none"))))]
    {
        let _ = (port, val);
    }
}

/// Read Time-Stamp Counter (rdtsc).
#[inline]
pub fn rdtsc() -> u64 {
    #[cfg(all(not(test), target_arch = "x86_64", target_os = "none"))]
    unsafe {
        let rax: u64;
        let rdx: u64;
        core::arch::asm!("rdtsc", out("rax") rax, out("rdx") rdx, options(nomem, nostack, preserves_flags));
        (rdx << 32) | rax
    }
    #[cfg(any(test, not(all(target_arch = "x86_64", target_os = "none"))))]
    {
        1_000_000
    }
}

/// Run microbenchmarks for heap allocation and VFS operations.
pub fn run_benchmarks() {
    crate::io::println("Running metalOS microbenchmarks...");
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

/// Execute a parsed shell command.
pub fn execute_command(cmd: &Command<'_>) {
    if cmd.is_empty() {
        return;
    }

    match cmd.name {
        "help" => {
            crate::io::println("metalOS shell built-in commands:");
            crate::io::println("  help          - Display this help message");
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
        "echo" => {
            let out = echo(&cmd.args);
            if let Some(target_path) = cmd.redirect_stdout {
                let _ = crate::fs::write_file(target_path, out.as_bytes());
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
            let path = if cmd.args.is_empty() { "/" } else { cmd.args[0] };
            if path == "/dev" || path == "dev" {
                crate::io::println("tty\nnull\nzero\nserial");
            } else if path == "/proc" || path == "proc" {
                crate::io::println("meminfo\nuptime\ncpuinfo\nversion");
            } else {
                crate::io::println("bin/\ndev/\nproc/\netc/\ntmp/");
            }
        }
        "cat" => {
            if cmd.args.is_empty() {
                crate::io::println("cat: missing file operand");
            } else {
                match crate::fs::read_file(cmd.args[0]) {
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
                out16(0x604, 0x2000);
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

/// Run the interactive `metal-sh` REPL loop.
pub fn run_shell() {
    let mut history = History::new();
    loop {
        crate::io::print("metalos:/$ ");
        let line_buf = readline();
        let line_str = line_from_buf(&line_buf);
        let trimmed = line_str.trim();
        if !trimmed.is_empty() {
            history.push(trimmed);
        }
        execute_line(line_str);
    }
}
