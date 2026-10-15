//! QEMU HTTPS relay. Forge supplies requests; this module verifies destinations
//! and executes certificate-verified HTTPS without printing headers or payloads.
use alloyos_forge::{
    cloud::{HttpRequest, HttpResponse, HTTP_BODY_LIMIT, HTTP_FRAME_LIMIT, HTTP_REQUEST_BODY_LIMIT},
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
        && request.body.len() <= HTTP_REQUEST_BODY_LIMIT
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

pub fn registry_get(url: &str) -> Result<serde_json::Value, String> {
    if !url.starts_with("https://registry.modelcontextprotocol.io/v0.1/servers?") || url.bytes().any(|byte| byte.is_ascii_control()) { return Err("registry destination rejected".into()); }
    let config = format!("url = {}\nrequest = \"GET\"\nproto = \"=https\"\nconnect-timeout = 5\nmax-time = 10\nretry = 1\nretry-delay = 1\nretry-all-errors\nmax-filesize = 1048576\nsilent\nwrite-out = \"\\n%{{http_code}}\"\n", quote(url));
    let mut child = Command::new("curl").args(["-q", "--config", "-"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|_| "registry HTTP process failed")?;
    let mut stdin = child.stdin.take().ok_or("registry stdin unavailable")?;
    use std::io::Write;
    stdin.write_all(config.as_bytes()).map_err(|_| "registry config write failed")?;
    drop(stdin);
    let mut bytes = Vec::new();
    child.stdout.take().ok_or("registry stdout unavailable")?.take(1024 * 1024 + 5).read_to_end(&mut bytes).map_err(|_| "registry read failed")?;
    if bytes.len() > 1024 * 1024 + 4 { let _ = child.kill(); let _ = child.wait(); return Err("registry response too large".into()); }
    if !child.wait().map_err(|_| "registry wait failed")?.success() { return Err("registry HTTPS failed".into()); }
    let response = String::from_utf8(bytes).map_err(|_| "registry response encoding failed")?;
    let (body, status) = response.rsplit_once('\n').ok_or("registry status missing")?;
    let status: u16 = status.parse().map_err(|_| "invalid registry status")?;
    if std::env::var_os("ALLOYOS_TRACE_E2E").is_some() { eprintln!("[mcp-search] stage=http_response status={status} body_bytes={}", body.len()); }
    Ok(serde_json::json!({"status":status,"headers":[],"body":body}))
}

pub fn registry_relay(line: &str) -> Option<String> {
    if !line.starts_with("MCPHTTP1\t") { return None; }
    let fields: Vec<_> = line.trim_end_matches(['\r', '\n']).split('\t').collect();
    let id = fields.get(1).and_then(|id| id.parse::<u64>().ok()).unwrap_or(0);
    let result = (|| {
        if fields.len() != 4 || fields[2] != "get" || line.len() > HTTP_FRAME_LIMIT { return Err("invalid registry frame".into()); }
        let value: serde_json::Value = serde_json::from_str(&decode_hex(fields[3]).map_err(|_| "invalid registry hex")?).map_err(|_| "invalid registry JSON")?;
        if value["method"] != "GET" || value["body"] != "" || value["headers"].as_array().is_none_or(|headers| !headers.is_empty()) { return Err("invalid registry HTTP method/headers".into()); }
        registry_get(value["url"].as_str().ok_or("missing registry URL")?)
    })();
    Some(match result {
        Ok(response) => format!("MCPHTTP1\t{id}\tok\t{}\n", encode_hex(&response.to_string())),
        Err(_) => format!("MCPHTTP1\t{id}\terror\tregistry_https_failed\n"),
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
        if std::env::var_os("ALLOYOS_TRACE_E2E").is_some() {
            let value: serde_json::Value = serde_json::from_str(&response.body).unwrap_or_default();
            let candidate = &value["candidates"][0];
            let finish = match candidate["finishReason"].as_str() { Some("STOP") => "STOP", Some("MAX_TOKENS") => "MAX_TOKENS", Some("SAFETY") => "SAFETY", _ => "other" };
            let text_bytes: usize = candidate["content"]["parts"].as_array().map(|parts| parts.iter().filter(|part| part["thought"] != true).filter_map(|part| part["text"].as_str()).map(str::len).sum()).unwrap_or(0);
            eprintln!("[api] stage=response status={} bytes={} finish={} text_bytes={} output_tokens={} thought_tokens={}", response.status, response.body.len(), finish, text_bytes, value["usageMetadata"]["candidatesTokenCount"].as_u64().unwrap_or(0), value["usageMetadata"]["thoughtsTokenCount"].as_u64().unwrap_or(0));
        }
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

/// One bounded worker keeps slow HTTPS out of the device/control pump.
fn relay_worker(execute: impl Fn(&str)->Option<String> + Send + 'static)
 -> (std::sync::mpsc::SyncSender<String>, std::sync::mpsc::Receiver<String>) {
    let (requests_tx, requests_rx)=std::sync::mpsc::sync_channel::<String>(4);
    let (replies_tx, replies_rx)=std::sync::mpsc::sync_channel::<String>(4);
    std::thread::spawn(move || { while let Ok(line)=requests_rx.recv() {
        if let Some(reply)=execute(&line) { if replies_tx.send(reply).is_err(){break;} }
    }});
    (requests_tx,replies_rx)
}
fn relay_busy(line:&str)->String {
    let mut fields=line.split('\t'); let prefix=fields.next().unwrap_or("FORGEHTTP1");
    let id=fields.next().and_then(|s|s.parse::<u64>().ok()).unwrap_or(0);
    format!("{prefix}\t{id}\terror\thost_relay_busy\n")
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
    let (lines_tx, lines_rx) = std::sync::mpsc::sync_channel(32);
    let reader = std::thread::spawn(move || {
        use std::io::BufRead;
        let mut reader = std::io::BufReader::new(output);
        loop {
            let mut bytes=Vec::new();
            if reader.by_ref().take((HTTP_FRAME_LIMIT+1) as u64).read_until(b'\n', &mut bytes).unwrap_or(0)==0 || bytes.len()>HTTP_FRAME_LIMIT {break;}
            let Ok(line)=String::from_utf8(bytes) else {break;};
            if lines_tx.send(line).is_err(){break;}
        }
    });
    let helper=std::env::var_os("ALLOYOS_VOICE_HELPER").map(std::path::PathBuf::from)
        .unwrap_or_else(||std::path::PathBuf::from("target/alloyos-voice-helper"));
    let mut voice=crate::voice::HostVoice::launch(&helper).ok();
    let mut active_voice=0;
    let (http_requests,http_replies)=relay_worker(|line|registry_relay(line).or_else(||relay(line,post)));
    let status = loop {
        if crate::STOP.load(std::sync::atomic::Ordering::Relaxed) {
            let _ = child.kill();
            break child.wait().map_err(|_| "QEMU wait failed")?;
        }
        if let Some(status) = child.try_wait().map_err(|_| "QEMU wait failed")? { break status; }
        if let Ok(reply)=http_replies.try_recv() {
            input.write_all(reply.as_bytes()).and_then(|_|input.flush()).map_err(|_|"guest pipe closed")?;
        }
        if let Some(bridge)=voice.as_ref() {
            for _ in 0..16 {
                match bridge.try_event() {
                    Ok(Some(event))=>{if let Some(stage)=event.diagnostic_stage(){eprintln!("[voice {}] {}",event.session(),stage);}
                        if let crate::voice::HostEvent::AudioStats{session,captured_frames,captured_samples,emitted_frames,sent_frames,input_transcripts,conversion_errors,level,muted,provider_ready,..}=&event {
                            eprintln!("[voice {session}] capture={captured_frames}/{captured_samples} emitted={emitted_frames} sent={sent_frames} transcripts={input_transcripts} conversion_errors={conversion_errors} level={level} muted={muted} provider_ready={provider_ready}");
                            // Host diagnostics remain useful during setup, but telemetry must
                            // never occupy the guest UART ahead of setupComplete.
                            if !provider_ready { continue; }
                        }
                        let json=serde_json::to_string(&event).map_err(|_|"invalid voice event")?;
                        let frame=format!("VOICE1\t{}\t{}\n",event.session(),encode_hex(&json));
                        input.write_all(frame.as_bytes()).and_then(|_|input.flush()).map_err(|_|"voice guest pipe closed")?;
                    },Ok(None)=>{break;},Err(_)=>{voice=None;
                        if active_voice!=0 {
                            let event=crate::voice::HostEvent::Error{session:active_voice,message:"voice host disconnected".into()};
                            let json=serde_json::to_string(&event).map_err(|_|"invalid voice event")?;
                            let frame=format!("VOICE1\t{active_voice}\t{}\n",encode_hex(&json));
                            input.write_all(frame.as_bytes()).and_then(|_|input.flush()).map_err(|_|"guest pipe closed")?;
                            active_voice=0;
                        }
                        break;
                    }
                }
            }
        }
        let line=match lines_rx.recv_timeout(std::time::Duration::from_millis(5)) {
            Ok(line)=>line,Err(std::sync::mpsc::RecvTimeoutError::Timeout)=>continue,Err(_)=>continue,
        };
        if line.starts_with("VOICE1\t") {
            if let Some((session,command))=crate::voice::parse_guest_frame(&line) {
                if matches!(&command,crate::voice::HostCommand::Stop{..}) {
                    voice=None;active_voice=0;
                    let event=crate::voice::HostEvent::Stopped{session};
                    let json=serde_json::to_string(&event).map_err(|_|"invalid voice event")?;
                    let frame=format!("VOICE1\t{session}\t{}\n",encode_hex(&json));
                    input.write_all(frame.as_bytes()).and_then(|_|input.flush()).map_err(|_|"guest pipe closed")?;
                    continue;
                }
                if matches!(&command,crate::voice::HostCommand::Start{..}) {
                    active_voice=session;
                    if voice.is_none(){voice=crate::voice::HostVoice::launch(&helper).ok();}
                }
                let result=voice.as_mut().ok_or("voice helper unavailable".to_string()).and_then(|v|v.command(&command));
                if result.is_err() {
                    voice=None;
                    let event=crate::voice::HostEvent::Error{session,message:"voice host unavailable".into()};
                    let json=serde_json::to_string(&event).map_err(|_|"invalid voice error")?;
                    let frame=format!("VOICE1\t{session}\t{}\n",encode_hex(&json));
                    input.write_all(frame.as_bytes()).and_then(|_|input.flush()).map_err(|_|"voice guest pipe closed")?;
                }
            }
            continue;
        }
        crate::crash::record(&line);
        if line.starts_with("FORGEHTTP1\t") || line.starts_with("MCPHTTP1\t") {
            if http_requests.try_send(line.clone()).is_err() {
                let reply=relay_busy(&line);
                input.write_all(reply.as_bytes()).and_then(|_|input.flush()).map_err(|_|"guest pipe closed")?;
            }
        } else {print!("{line}");}
    };
    drop(http_requests);
    drop(http_replies);
    drop(lines_rx);
    drop(voice);
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
    fn registry_relay_rejects_credentials_and_other_destinations() {
        for value in [
            serde_json::json!({"method":"GET","url":"https://example.com/servers?search=gmail","headers":[],"body":""}),
            serde_json::json!({"method":"GET","url":"https://registry.modelcontextprotocol.io/v0.1/servers?search=gmail","headers":[["Authorization","fixture-secret"]],"body":""}),
            serde_json::json!({"method":"POST","url":"https://registry.modelcontextprotocol.io/v0.1/servers?search=gmail","headers":[],"body":"fixture-secret"}),
        ] {
            let frame = format!("MCPHTTP1\t9\tget\t{}\n", encode_hex(&value.to_string()));
            let reply = registry_relay(&frame).unwrap();
            assert_eq!(reply, "MCPHTTP1\t9\terror\tregistry_https_failed\n");
            assert!(!reply.contains("fixture-secret"));
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
    fn relay_preserves_large_context_across_json_and_hex_framing() {
        let mut outbound = request();
        outbound.body = serde_json::json!({"input":"雪\"\n".repeat(30_000)}).to_string();
        assert!(outbound.body.len() > 65536);
        let frame = format!(
            "FORGEHTTP1\t42\tpost\t{}\n",
            encode_hex(&serde_json::to_string(&outbound).unwrap())
        );
        assert!(frame.len() > 262144);
        let reply = relay(&frame, |request| {
            assert_eq!(request.body, outbound.body);
            Ok(HttpResponse {
                status: 200,
                body: "{}".into(),
            })
        })
        .unwrap();
        assert!(reply.starts_with("FORGEHTTP1\t42\tok\t"));
        outbound.body = "x".repeat(HTTP_REQUEST_BODY_LIMIT + 1);
        assert!(!allowed(&outbound));
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

#[cfg(test)]
mod voice_relay_tests {
    use super::*;
    #[test]
    fn slow_http_is_off_thread_and_bounded() {
        let (gate_tx,gate_rx)=std::sync::mpsc::channel();
        let gate=std::sync::Mutex::new(gate_rx);
        let (requests,replies)=relay_worker(move|_|{gate.lock().unwrap().recv().unwrap();Some("done".into())});
        requests.try_send("first".into()).unwrap();
        let deadline=std::time::Instant::now()+std::time::Duration::from_millis(100);
        let mut saturated=false;
        for _ in 0..10 {if requests.try_send("next".into()).is_err(){saturated=true;break;}}
        assert!(saturated);assert!(std::time::Instant::now()<deadline);
        assert!(matches!(replies.try_recv(),Err(std::sync::mpsc::TryRecvError::Empty)));
        gate_tx.send(()).unwrap();assert_eq!(replies.recv_timeout(std::time::Duration::from_secs(1)).unwrap(),"done");
        // Release remaining work before tearing down the worker.
        for _ in 0..4 {let _=gate_tx.send(());}
    }
    #[test]
    fn saturation_reply_does_not_expose_request() {
        assert_eq!(relay_busy("FORGEHTTP1\t7\tpost\tsecret"),"FORGEHTTP1\t7\terror\thost_relay_busy\n");
    }
}
