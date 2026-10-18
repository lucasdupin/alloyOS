//! Opt-in production guest UART/HTTPS acceptance, without a model or credentials.
//! Build the kernel with remote-mcp-selftest, then pass kernel + fw_cfg JSON file.
use std::{io::{BufRead,Read,Write},path::PathBuf,process::{Command,Stdio},sync::mpsc,time::{Duration,Instant}};

fn main()->Result<(),Box<dyn std::error::Error>> {
    let mut args=std::env::args_os().skip(1);
    let kernel=PathBuf::from(args.next().ok_or("kernel path required")?);
    let config=PathBuf::from(args.next().ok_or("probe JSON path required")?);
    let json=std::fs::read(&config)?;
    if json.len()>16384 {return Err("probe config exceeds limit".into());}
    let value:serde_json::Value=serde_json::from_slice(&json)?;
    let endpoint=value["endpoint"].as_str().ok_or("endpoint required")?;
    if alloyos_mcp::transport::remote_mcp_authority(endpoint).is_none() {return Err("public hosted HTTPS endpoint required".into());}
    println!("Public MCP endpoint: {endpoint}; no account/model credentials attached");
    let mut guest=Command::new("qemu-system-aarch64")
        .args(["-M","virt,gic-version=3","-accel","hvf","-cpu","host","-m","512M","-display","none","-monitor","none","-serial","stdio","-no-reboot","-kernel"])
        .arg(kernel).arg("-fw_cfg").arg(format!("name=opt/alloyos/remote-probe,file={}",config.display()))
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
    let mut input=guest.stdin.take().ok_or("missing QEMU input")?;
    let output=guest.stdout.take().ok_or("missing QEMU output")?;
    let (tx,rx)=mpsc::sync_channel::<String>(16);
    let reader=std::thread::spawn(move||{
        let mut output=std::io::BufReader::new(output);
        loop {
            let mut bytes=Vec::new();
            if output.by_ref().take((alloyos_forge::cloud::HTTP_FRAME_LIMIT+1)as u64).read_until(b'\n',&mut bytes).unwrap_or(0)==0
                || bytes.len()>alloyos_forge::cloud::HTTP_FRAME_LIMIT {break;}
            let Ok(line)=String::from_utf8(bytes) else {break;};
            if tx.send(line).is_err() {break;}
        }
    });
    let deadline=Instant::now()+Duration::from_secs(180);
    let result=(||->Result<(),Box<dyn std::error::Error>>{
        loop {
            if Instant::now()>deadline {return Err("guest probe timeout".into());}
            if let Some(exit)=guest.try_wait()? {return Err(format!("QEMU exited early: {exit}").into());}
            let line=match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(line)=>line,Err(mpsc::RecvTimeoutError::Timeout)=>continue,Err(_)=>return Err("guest UART disconnected".into()),
            };
            if line.starts_with("MCPHTTP1\t") {
                let reply=alloyos_dev_harness::https::registry_relay(&line).ok_or("invalid MCP relay request")?;
                input.write_all(reply.as_bytes())?;input.flush()?;
            } else if line.starts_with("REMOTE-MCP:") {
                print!("{line}");
                if line.contains("production guest relay PASS") {return Ok(());}
                if line.contains("FAIL") {return Err("production guest MCP probe failed".into());}
            }
        }
    })();
    let _=guest.kill();let _=guest.wait();drop(rx);let _=reader.join();
    result
}
