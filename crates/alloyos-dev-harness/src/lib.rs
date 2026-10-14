//! The development harness's bounded, loopback-only development endpoint and guest serial transport.
pub mod credentials;
pub mod crash;
pub mod https;
pub mod voice;
pub mod model_loader;
use serde::Deserialize;
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
const LIMIT: usize = alloyos_forge::cloud::HTTP_REQUEST_BODY_LIMIT;
static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(unix)]
extern "C" fn stop_signal(_: i32) {
    STOP.store(true, std::sync::atomic::Ordering::Relaxed);
}
/// Installs SIGINT/SIGTERM cleanup before launching QEMU so guest and credential guards drop.
pub fn install_shutdown_handler() {
    #[cfg(unix)]
    unsafe {
        unsafe extern "C" {
            fn signal(number: i32, handler: extern "C" fn(i32)) -> usize;
        }
        signal(2, stop_signal);
        signal(15, stop_signal);
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|v| format!("{v:02x}")).collect()
}
fn unhex(s: &str) -> Result<String, String> {
    if s.len() % 2 != 0 || s.len() > LIMIT {
        return Err("invalid response encoding".into());
    }
    let bytes = s
        .as_bytes()
        .chunks_exact(2)
        .map(|p| {
            let h = std::str::from_utf8(p).map_err(|_| "invalid hex")?;
            u8::from_str_radix(h, 16).map_err(|_| "invalid hex")
        })
        .collect::<Result<Vec<_>, _>>()?;
    String::from_utf8(bytes).map_err(|_| "invalid response UTF-8".into())
}
pub struct Guest {
    child: Child,
    input: ChildStdin,
    replies: mpsc::Receiver<String>,
    next_id: u64,
    timeout: Duration,
}
impl Guest {
    pub fn launch(command: &mut Command, timeout: Duration) -> Result<Self, String> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "QEMU launch failed".to_string())?;
        let input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, replies) = mpsc::sync_channel(32);
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = Vec::new();
                match reader
                    .by_ref()
                    .take((alloyos_forge::cloud::HTTP_FRAME_LIMIT + 1) as u64)
                    .read_until(b'\n', &mut line)
                {
                    Ok(0) | Err(_) => break,
                    _ => {}
                }
                if line.len() > alloyos_forge::cloud::HTTP_FRAME_LIMIT {
                    break;
                }
                if let Ok(line) = String::from_utf8(line) {
                    crate::crash::record(&line);
                    if line.starts_with("[crash] ") { eprint!("{line}"); }
                    if (line.starts_with("FORGE1\t") || line.starts_with("FORGEHTTP1\t") || line.starts_with("MCPHTTP1\t") || line.contains("[systemui] Slint window online;")) && tx.try_send(line).is_err() {
                        break;
                    }
                }
            }
        });
        Ok(Self {
            child,
            input,
            replies,
            next_id: 0,
            timeout,
        })
    }
    /// Wait for the UI to initialize its UART polling before sending RPC frames.
    pub fn wait_ui_boot(&mut self) -> Result<(), String> {
        let deadline = Instant::now() + self.timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let line = self.replies.recv_timeout(remaining)
                .map_err(|_| "guest UI boot timeout".to_string())?;
            if line.contains("[systemui] Slint window online;") {
                return Ok(());
            }
        }
    }
    pub fn request(&mut self, operation: &str) -> Result<String, String> {
        self.next_id += 1;
        let prefix = format!("FORGE1\t{}\t", self.next_id);
        writeln!(self.input, "{prefix}{operation}")
            .and_then(|_| self.input.flush())
            .map_err(|_| "guest transport closed".to_string())?;
        let deadline = Instant::now() + self.timeout;
        loop {
            if STOP.load(std::sync::atomic::Ordering::Relaxed) {
                return Err("Development bridge shutting down".into());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("Forge response timeout".into());
            }
            let line = match self
                .replies
                .recv_timeout(remaining.min(Duration::from_millis(100)))
            {
                Ok(line) => line,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => return Err("guest transport closed".into()),
            };
            if let Some(reply) = https::registry_relay(&line).or_else(|| https::relay(&line, https::post)) {
                self.input.write_all(reply.as_bytes()).and_then(|_|self.input.flush()).map_err(|_|"cloud relay closed")?;
                continue;
            }
            if !line.starts_with(&prefix) {
                continue;
            }
            let fields = line
                .trim_end_matches(['\r', '\n'])
                .split('\t')
                .collect::<Vec<_>>();
            return match fields.as_slice() {
                [_, _, "ok", payload] => unhex(payload),
                [_, _, "error", code, payload] => Err(format!("{code}: {}", unhex(payload)?)),
                _ => Err("malformed Forge reply".into()),
            };
        }
    }
    pub fn wait_ready(&mut self) -> Result<String, String> {
        self.request("status")
    }
}
impl Drop for Guest {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Generate {
    prompt: String,
    model: Option<String>,
    max_tokens: Option<u32>,
}
fn operation(method: &str, path: &str, body: &[u8]) -> Result<String, String> {
    match (method, path) {
        ("GET", "/v1/status") => Ok("status".into()),
        ("GET", "/v1/models") => Ok("models".into()),
        ("POST", "/v1/generate") => {
            let req: Generate =
                serde_json::from_slice(body).map_err(|_| "invalid generate JSON".to_string())?;
            if req.prompt.is_empty() {
                return Err("prompt must not be empty".into());
            }
            let tokens = req.max_tokens.unwrap_or(128);
            if tokens == 0 || tokens > 256 {
                return Err("max_tokens must be 1..256".into());
            }
            let model = req.model.as_deref().unwrap_or("-");
            if model.is_empty()
                || model.len() > 128
                || model
                    .chars()
                    .any(|c| !c.is_ascii_alphanumeric() && !"-_.".contains(c))
            {
                return Err("invalid model id".into());
            }
            Ok(format!(
                "generate\t{model}\t{tokens}\t{}",
                hex(req.prompt.as_bytes())
            ))
        }
        _ => Err("unknown endpoint".into()),
    }
}
struct DeadlineReader<'a> {
    stream: &'a mut TcpStream,
    deadline: Instant,
}
impl Read for DeadlineReader<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "request deadline",
            ));
        }
        self.stream.set_read_timeout(Some(remaining))?;
        self.stream.read(bytes)
    }
}
fn read_request(stream: &mut TcpStream) -> Result<(String, String, Vec<u8>), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|_| "socket timeout")?;
    read_request_from(BufReader::new(DeadlineReader {
        stream,
        deadline: Instant::now() + Duration::from_secs(5),
    }))
}
fn read_request_from(mut reader: impl BufRead) -> Result<(String, String, Vec<u8>), String> {
    let mut header_bytes = 0;
    let mut line = String::new();
    reader
        .by_ref()
        .take(8193)
        .read_line(&mut line)
        .map_err(|_| "request read failed")?;
    header_bytes += line.len();
    let first = line
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if first.len() != 3 || first[2] != "HTTP/1.1" {
        return Err("invalid request".into());
    }
    let mut length = 0;
    let mut saw_length = false;
    loop {
        line.clear();
        reader
            .by_ref()
            .take(8193)
            .read_line(&mut line)
            .map_err(|_| "header read failed")?;
        header_bytes += line.len();
        if header_bytes > 8192 || line.is_empty() {
            return Err("invalid headers".into());
        }
        if line == "\r\n" {
            break;
        }
        let (name, value) = line.split_once(':').ok_or("invalid header")?;
        // Browser cross-origin requests and DNS rebinding must not invoke local inference.
        if name.eq_ignore_ascii_case("origin") {
            return Err("browser origins are not permitted".into());
        }
        if name.eq_ignore_ascii_case("host") {
            let host = value.trim();
            let allowed = host == "localhost"
                || host == "127.0.0.1"
                || host == "[::1]"
                || ["localhost:", "127.0.0.1:", "[::1]:"].iter().any(|prefix| {
                    host.strip_prefix(prefix).is_some_and(|port| {
                        !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())
                    })
                });
            if !allowed {
                return Err("Host must be loopback".into());
            }
        }
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err("chunked requests unsupported".into());
        }
        if name.eq_ignore_ascii_case("content-length") {
            if saw_length {
                return Err("duplicate content length".into());
            }
            saw_length = true;
            length = value
                .trim()
                .parse::<usize>()
                .map_err(|_| "invalid content length")?;
            if length > LIMIT {
                return Err("request body too large".into());
            }
        }
    }
    let mut body = vec![0; length];
    reader
        .read_exact(&mut body)
        .map_err(|_| "body read failed")?;
    Ok((first[0].clone(), first[1].clone(), body))
}
fn reply(stream: &mut TcpStream, code: u16, body: &str) {
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let _=write!(stream,"HTTP/1.1 {code} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",if code==200{"OK"}else{"Error"},body.len());
}
/// Requests execute serially. Binding to a non-loopback address is forbidden.
pub fn serve(address: SocketAddr, mut guest: Guest) -> Result<(), String> {
    if !address.ip().is_loopback() {
        return Err("Development bridge must bind to loopback".into());
    }
    let listener = TcpListener::bind(address).map_err(|_| "endpoint bind failed".to_string())?;
    listener
        .set_nonblocking(true)
        .map_err(|_| "endpoint setup failed")?;
    println!("Development bridge listening on http://{address}");
    while !STOP.load(std::sync::atomic::Ordering::Relaxed) {
        let mut stream = match listener.accept() {
            Ok((s, _)) => s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
                continue;
            }
            Err(_) => return Err("endpoint accept failed".into()),
        };
        stream
            .set_nonblocking(false)
            .map_err(|_| "connection setup failed")?;
        let result =
            read_request(&mut stream).and_then(|(m, p, b)| operation(&m, &p, &b).map(|op| (p, op)));
        match result {
            Err(error) => reply(
                &mut stream,
                400,
                &serde_json::json!({"error":error}).to_string(),
            ),
            Ok((path, op)) => match guest.request(&op) {
                Ok(value) => {
                    let body = if path == "/v1/generate" {
                        serde_json::json!({"text":value}).to_string()
                    } else {
                        value
                    };
                    reply(&mut stream, 200, &body)
                }
                Err(error) => reply(
                    &mut stream,
                    503,
                    &serde_json::json!({"error":error}).to_string(),
                ),
            },
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encodes_and_bounds_generation() {
        assert_eq!(
            operation("POST", "/v1/generate", br#"{"prompt":"hi"}"#).unwrap(),
            "generate\t-\t128\t6869"
        );
        assert!(operation(
            "POST",
            "/v1/generate",
            br#"{"prompt":"hi","max_tokens":257}"#
        )
        .is_err());
        assert!(operation(
            "POST",
            "/v1/generate",
            br#"{"prompt":"hi","model":"bad\tname"}"#
        )
        .is_err());
    }
    #[test]
    fn large_generation_context_roundtrips_through_guest_protocol() {
        let prompt = "雪".repeat(10_000);
        let body = serde_json::json!({"prompt":prompt}).to_string();
        let operation = operation("POST", "/v1/generate", body.as_bytes()).unwrap();
        let frame = format!("FORGE1\t1\t{operation}\n");
        let request = alloyos_forge::protocol::Request::parse(&frame).unwrap();
        match request.command {
            alloyos_forge::protocol::Command::Generate(request) => {
                assert_eq!(request.prompt, prompt)
            }
            _ => panic!("expected generation request"),
        }
    }

    #[test]
    fn unicode_roundtrip() {
        assert_eq!(unhex(&hex("Olá".as_bytes())).unwrap(), "Olá");
        assert!(unhex("f").is_err());
        assert!(unhex("ff").is_err());
    }
    #[test]
    fn rejects_browser_origin_and_non_loopback_host() {
        for header in ["Origin: https://example.com", "Host: attacker.example"] {
            let request = format!("GET /v1/status HTTP/1.1\r\n{header}\r\n\r\n");
            assert!(read_request_from(std::io::Cursor::new(request)).is_err());
        }
        let request = b"GET /v1/status HTTP/1.1\r\nHost: 127.0.0.1:7070\r\n\r\n";
        assert!(read_request_from(std::io::Cursor::new(request)).is_ok());
    }
    #[test]
    fn rejects_duplicate_length() {
        let bytes = b"POST /v1/generate HTTP/1.1\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n";
        assert!(read_request_from(std::io::Cursor::new(bytes)).is_err());
    }
}
#[cfg(all(test, unix))]
mod transport_tests {
    use super::*;
    #[test]
    fn serial_correlates_and_propagates_guest_errors() {
        let mut command = Command::new("python3");
        command.args(["-u","-c", "import sys\nfor line in sys.stdin:\n p=line.strip().split('\\t'); print('boot noise',flush=True); print('FORGE1\\t999\\tok\\t6869',flush=True); print('FORGE1\\t'+p[1]+'\\terror\\tbackend_unavailable\\t6e6f206d6f64656c',flush=True)"]);
        let mut guest = Guest::launch(&mut command, Duration::from_secs(2)).unwrap();
        assert_eq!(
            guest.request("status").unwrap_err(),
            "backend_unavailable: no model"
        );
    }
    #[test]
    fn serial_timeout_is_bounded() {
        let mut command = Command::new("python3");
        command.args(["-u", "-c", "import time;time.sleep(60)"]);
        let mut guest = Guest::launch(&mut command, Duration::from_millis(30)).unwrap();
        let start = Instant::now();
        assert!(guest.request("status").is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
