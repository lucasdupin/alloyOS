//! QEMU HTTPS relay. Forge supplies requests; this module verifies destinations
//! and executes certificate-verified HTTPS without printing headers or payloads.
use alloyos_forge::{
    cloud::{HttpRequest, HttpResponse, HTTP_BODY_LIMIT, HTTP_FRAME_LIMIT},
    protocol::{decode_hex, encode_hex},
};
use std::{
    io::{Read, Write},
    process::{Command, Stdio},
};

pub fn allowed(request: &HttpRequest) -> bool {
    let valid_url = request.url == "https://api.anthropic.com/v1/messages"
        || request.url == "https://api.openai.com/v1/responses"
        || request
            .url
            .strip_prefix("https://generativelanguage.googleapis.com/v1beta/models/")
            .and_then(|s| s.strip_suffix(":generateContent"))
            .is_some_and(|id| {
                !id.is_empty()
                    && id.len() <= 128
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            });
    valid_url
        && request.body.len() <= HTTP_BODY_LIMIT
        && request.headers.len() <= 4
        && request.headers.iter().all(|(name, value)| {
            matches!(
                name.as_str(),
                "Content-Type"
                    | "Authorization"
                    | "x-api-key"
                    | "x-goog-api-key"
                    | "anthropic-version"
            ) && value.len() <= 4096 + 7
                && !value.bytes().any(|b| b.is_ascii_control())
        })
}
fn quote(value: &str) -> String {
    // curl config's quoted strings interpret these escapes, not a shell.
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
    )
}
fn config(request: &HttpRequest, timeout: std::time::Duration) -> Result<String, String> {
    if !allowed(request) {
        return Err("destination_rejected".into());
    }
    let mut config=format!("url = {}\nrequest = \"POST\"\nproto = \"=https\"\nconnect-timeout = 10\nmax-time = {:.3}\nmax-filesize = {}\nsilent\nwrite-out = \"\\n%{{http_code}}\"\n",quote(&request.url),timeout.as_secs_f64(),HTTP_BODY_LIMIT);
    for (name, value) in &request.headers {
        config.push_str(&format!(
            "header = {}\n",
            quote(&format!("{name}: {value}"))
        ));
    }
    config.push_str(&format!("data = {}\n", quote(&request.body)));
    Ok(config)
}
pub fn post(request: &HttpRequest) -> Result<HttpResponse, String> {
    // Keep the entire relay below HostHttps's 55-second guest deadline.
    let started = std::time::Instant::now();
    retry_post(
        |timeout| post_once(request, timeout),
        || started.elapsed(),
        std::thread::sleep,
    )
}

fn retry_post(
    mut execute: impl FnMut(std::time::Duration) -> Result<HttpResponse, String>,
    elapsed: impl Fn() -> std::time::Duration,
    mut sleep: impl FnMut(std::time::Duration),
) -> Result<HttpResponse, String> {
    use std::time::Duration;
    const BUDGET: Duration = Duration::from_secs(50);
    for attempt in 0..4 {
        let remaining = BUDGET.saturating_sub(elapsed());
        if remaining.is_zero() {
            return Err("https_transport_failed".into());
        }
        let response = execute(remaining.min(Duration::from_secs(45)))?;
        // Only retry explicit temporary HTTP failures, never ambiguous transport
        // failures (which could have completed/billed) or authentication errors.
        if !matches!(response.status, 502 | 503 | 504) || attempt == 3 {
            return Ok(response);
        }
        let jitter = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos() as u64
            % 501;
        let delay = Duration::from_millis((1000 << attempt) + jitter);
        if BUDGET.saturating_sub(elapsed()) <= delay + Duration::from_secs(1) {
            return Ok(response);
        }
        sleep(delay);
    }
    unreachable!()
}

fn post_once(request: &HttpRequest, timeout: std::time::Duration) -> Result<HttpResponse, String> {
    let mut config = config(request, timeout)?;
    // No redirect following, no insecure TLS flag, no credentials in argv or logs.
    let mut child = Command::new("curl")
        .args(["--disable", "--config", "-"])
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "https_client_unavailable".to_string())?;
    let written = child.stdin.take().unwrap().write_all(config.as_bytes());
    unsafe {
        config.as_bytes_mut().fill(0);
    }
    if written.is_err() {
        let _ = child.kill();
        let _ = child.wait();
        return Err("https_transport_failed".into());
    }
    let mut bytes = Vec::new();
    let read = child
        .stdout
        .take()
        .unwrap()
        .take((HTTP_BODY_LIMIT + 6) as u64)
        .read_to_end(&mut bytes);
    if read.is_err() || bytes.len() > HTTP_BODY_LIMIT + 4 {
        let _ = child.kill();
        let _ = child.wait();
        return Err("https_response_too_large".into());
    }
    let status = child.wait().map_err(|_| "https_transport_failed")?;
    if !status.success() {
        return Err("https_transport_failed".into());
    }
    let output = String::from_utf8(bytes).map_err(|_| "https_invalid_utf8")?;
    let (body, status) = output.rsplit_once('\n').ok_or("https_invalid_response")?;
    let status = status.parse::<u16>().map_err(|_| "https_invalid_status")?;
    Ok(HttpResponse {
        status,
        body: body.into(),
    })
}

/// Handle only the private cloud frame and never expose raw failure details.
pub fn relay(
    line: &str,
    execute: impl FnOnce(&HttpRequest) -> Result<HttpResponse, String>,
) -> Option<String> {
    if !line.starts_with("FORGEHTTP1\t") {
        return None;
    }
    let fields = line
        .trim_end_matches(['\r', '\n'])
        .split('\t')
        .collect::<Vec<_>>();
    let id = fields.get(1)?.parse::<u64>().ok()?;
    let result: Result<String, String> = (|| {
        if line.len() > HTTP_FRAME_LIMIT || fields.len() != 4 || fields[2] != "post" {
            return Err("invalid_request".into());
        }
        let encoded = decode_hex(fields[3]).map_err(|_| "invalid_encoding")?;
        let request: HttpRequest = serde_json::from_str(&encoded).map_err(|_| "invalid_request")?;
        if !allowed(&request) {
            return Err("destination_rejected".into());
        }
        let response = execute(&request)?;
        if response.body.len() > HTTP_BODY_LIMIT {
            return Err("response_too_large".into());
        }
        serde_json::to_string(&response).map_err(|_| "invalid_response".into())
    })();
    Some(match result {
        Ok(body) => format!("FORGEHTTP1\t{id}\tok\t{}\n", encode_hex(&body)),
        Err(_) => format!("FORGEHTTP1\t{id}\terror\thttps_transport_failed\n"),
    })
}

/// Interactive UI runner with a private serial pipe dedicated to Forge.
/// The serial debug shell uses the ordinary inherited-terminal path instead.
pub fn run_ui(command: &mut Command) -> Result<(), String> {
    crate::install_shutdown_handler();
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|_| "QEMU launch failed")?;
    let mut input = child.stdin.take().unwrap();
    let output = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        use std::io::BufRead;
        let mut reader = std::io::BufReader::new(output);
        loop {
            let mut bytes = Vec::new();
            if reader
                .by_ref()
                .take((HTTP_FRAME_LIMIT + 1) as u64)
                .read_until(b'\n', &mut bytes)
                .unwrap_or(0)
                == 0
            {
                break;
            }
            if bytes.len() > HTTP_FRAME_LIMIT {
                break;
            }
            let Ok(line) = String::from_utf8(bytes) else {
                break;
            };
            if let Some(reply) = relay(&line, post) {
                if input
                    .write_all(reply.as_bytes())
                    .and_then(|_| input.flush())
                    .is_err()
                {
                    break;
                }
            } else if !line.starts_with("FORGEHTTP1") {
                print!("{line}");
            }
        }
    });
    let status = loop {
        if crate::STOP.load(std::sync::atomic::Ordering::Relaxed) {
            let _ = child.kill();
            break child.wait().map_err(|_| "QEMU wait failed")?;
        }
        if let Some(status) = child.try_wait().map_err(|_| "QEMU wait failed")? {
            break status;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    let _ = reader.join();
    if crate::STOP.load(std::sync::atomic::Ordering::Relaxed) {
        return Ok(());
    }
    if status.success() {
        Ok(())
    } else {
        Err("QEMU exited with an error".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> HttpRequest {
        HttpRequest {
            url: "https://api.openai.com/v1/responses".into(),
            headers: vec![("Authorization".into(), "Bearer fixture-secret".into())],
            body: "{\"input\":\"quotes \\\" and Unicode 東京\"}".into(),
        }
    }
    #[test]
    fn transient_failures_recover_with_bounded_backoff() {
        use std::{cell::Cell, time::Duration};
        let elapsed = Cell::new(Duration::ZERO);
        let mut calls = 0;
        let mut delays = Vec::new();
        let result = retry_post(
            |timeout| {
                assert!(timeout <= Duration::from_secs(45));
                calls += 1;
                Ok(HttpResponse {
                    status: if calls < 4 { 503 } else { 200 },
                    body: "ok".into(),
                })
            },
            || elapsed.get(),
            |delay| {
                elapsed.set(elapsed.get() + delay);
                delays.push(delay);
            },
        )
        .unwrap();
        assert_eq!(result.status, 200);
        assert_eq!(calls, 4);
        for (index, delay) in delays.iter().enumerate() {
            let base = Duration::from_secs(1 << index);
            assert!(*delay >= base && *delay <= base + Duration::from_millis(500));
        }
    }

    #[test]
    fn retries_stop_on_exhaustion_permanent_errors_and_deadline() {
        use std::{cell::Cell, time::Duration};
        for (status, expected) in [(503, 4), (401, 1), (403, 1), (400, 1), (429, 1), (200, 1)] {
            let mut calls = 0;
            let result = retry_post(
                |_| {
                    calls += 1;
                    Ok(HttpResponse {
                        status,
                        body: String::new(),
                    })
                },
                || Duration::ZERO,
                |_| {},
            )
            .unwrap();
            assert_eq!(result.status, status);
            assert_eq!(calls, expected);
        }
        let time = Cell::new(Duration::ZERO);
        let result = retry_post(
            |_| {
                time.set(Duration::from_secs(49));
                Ok(HttpResponse {
                    status: 503,
                    body: String::new(),
                })
            },
            || time.get(),
            |_| panic!("deadline exceeded"),
        )
        .unwrap();
        assert_eq!(result.status, 503);
        assert!(retry_post(
            |_| Err("https_transport_failed".into()),
            || Duration::ZERO,
            |_| panic!("ambiguous failure retried")
        )
        .is_err());
    }

    #[test]
    fn rejects_other_hosts_redirects_and_header_injection() {
        for url in [
            "http://api.openai.com/v1/responses",
            "https://api.openai.com.evil/v1/responses",
            "https://api.openai.com/v1/responses?key=secret",
            "https://generativelanguage.googleapis.com/v1beta/models/../evil:generateContent",
        ] {
            let mut request = request();
            request.url = url.into();
            assert!(!allowed(&request));
        }
        let mut r = request();
        r.headers[0].1 = "secret\r\nX-Other: injected".into();
        assert!(!allowed(&r));
    }
    #[test]
    fn relay_roundtrip_and_errors_do_not_leak_keys() {
        let outbound = request();
        let json = serde_json::to_string(&outbound).unwrap();
        let frame = format!("FORGEHTTP1\t42\tpost\t{}\n", encode_hex(&json));
        let reply = relay(&frame, |r| {
            assert_eq!(r.body, outbound.body);
            Ok(HttpResponse {
                status: 200,
                body: "東京".into(),
            })
        })
        .unwrap();
        let fields = reply.trim_end().split('\t').collect::<Vec<_>>();
        assert_eq!(&fields[..3], &["FORGEHTTP1", "42", "ok"]);
        let response: HttpResponse = serde_json::from_str(&decode_hex(fields[3]).unwrap()).unwrap();
        assert_eq!(response.body, "東京");
        let failure = relay(&frame, |_| Err("fixture-secret".into())).unwrap();
        assert!(!failure.contains("fixture-secret"));
        assert!(failure.contains("https_transport_failed"));
    }
    #[test]
    fn curl_uses_private_stdin_config_and_verified_https() {
        let config = config(&request(), std::time::Duration::from_secs(45)).unwrap();
        assert!(config.contains("proto = \"=https\""));
        assert!(config.contains("max-time = 45.000"));
        assert!(!config.contains("insecure"));
        assert!(!config.contains("location"));
    }
}
