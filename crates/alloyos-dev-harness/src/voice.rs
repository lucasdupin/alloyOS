//! Bounded host device/socket helper. Credentials travel only over private pipes.
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, Command, Stdio},
    sync::mpsc,
};
pub const FRAME_LIMIT: usize = 131072;
#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum HostCommand {
    Start {
        session: u64,
        endpoint: String,
        api_key: String,
    },
    Send {
        session: u64,
        json: String,
    },
    Mute {
        session: u64,
        muted: bool,
    },
    Playback {
        session: u64,
        generation: u64,
        pcm: String,
    },
    Interrupt {
        session: u64,
        generation: u64,
    },
    Stop {
        session: u64,
    },
}
#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum HostEvent {
    Ready { session: u64 },
    Audio { session: u64, pcm: String },
    Message { session: u64, json: String },
    Error { session: u64, message: String },
    Stopped { session: u64 },
}
impl HostEvent {
    pub fn session(&self) -> u64 {
        match self {
            Self::Ready { session }
            | Self::Audio { session, .. }
            | Self::Message { session, .. }
            | Self::Error { session, .. }
            | Self::Stopped { session } => *session,
        }
    }
}
pub struct HostVoice {
    child: Child,
    input: mpsc::SyncSender<Vec<u8>>,
    control: mpsc::SyncSender<Vec<u8>>,
    events: mpsc::Receiver<HostEvent>,
}
impl HostVoice {
    pub fn launch(path: &std::path::Path) -> Result<Self, String> {
        let mut child = Command::new(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "voice helper launch failed")?;
        let mut pipe = child.stdin.take().unwrap();
        let (input, commands) = mpsc::sync_channel::<Vec<u8>>(32);
        let (control, controls) = mpsc::sync_channel::<Vec<u8>>(8);
        std::thread::spawn(move || loop {
            let bytes = match controls.try_recv() {
                Ok(bytes) => bytes,
                Err(_) => match commands.recv_timeout(std::time::Duration::from_millis(2)) {
                    Ok(bytes) => bytes,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => break,
                },
            };
            if pipe.write_all(&bytes).and_then(|_| pipe.flush()).is_err() {
                break;
            }
        });
        let output = child.stdout.take().unwrap();
        let (tx, events) = mpsc::sync_channel(32);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(output);
            loop {
                let mut line = Vec::new();
                if reader
                    .by_ref()
                    .take((FRAME_LIMIT + 1) as u64)
                    .read_until(b'\n', &mut line)
                    .unwrap_or(0)
                    == 0
                    || line.len() > FRAME_LIMIT
                {
                    break;
                }
                let Ok(event) = serde_json::from_slice(&line) else {
                    break;
                };
                if tx.try_send(event).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            input,
            control,
            events,
        })
    }
    pub fn command(&mut self, command: &HostCommand) -> Result<(), String> {
        validate(command)?;
        let mut bytes = serde_json::to_vec(command).map_err(|_| "invalid voice command")?;
        if bytes.len() > FRAME_LIMIT {
            return Err("voice command too large".into());
        }
        bytes.push(b'\n');
        let target = if matches!(
            command,
            HostCommand::Start { .. }
                | HostCommand::Mute { .. }
                | HostCommand::Interrupt { .. }
                | HostCommand::Stop { .. }
        ) {
            &self.control
        } else {
            &self.input
        };
        target
            .try_send(bytes)
            .map_err(|_| "voice helper queue unavailable".into())
    }
    pub fn try_event(&self) -> Result<Option<HostEvent>, String> {
        match self.events.try_recv() {
            Ok(e) => Ok(Some(e)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(_) => Err("voice helper closed".into()),
        }
    }
}
impl Drop for HostVoice {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn validate(c: &HostCommand) -> Result<(), String> {
    match c{
 HostCommand::Start{endpoint,api_key,..}=>{if endpoint!="wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent"||api_key.is_empty()||api_key.len()>4096{return Err("invalid voice endpoint or credentials".into())}},
 HostCommand::Playback{pcm,..}=>{if pcm.len()>65536||pcm.len()%4!=0||!pcm.bytes().all(|b|b.is_ascii_hexdigit()){return Err("invalid playback PCM".into())}},
 HostCommand::Send{json,..}=>{if json.len()>FRAME_LIMIT/2||serde_json::from_str::<serde_json::Value>(json).is_err(){return Err("invalid voice JSON".into())}},_=>{}}
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_frames() {
        assert!(validate(&HostCommand::Playback {
            session: 1,
            generation: 1,
            pcm: "ff".into()
        })
        .is_err());
        assert!(validate(&HostCommand::Playback {
            session: 1,
            generation: 1,
            pcm: "00ff".into()
        })
        .is_ok());
        assert!(validate(&HostCommand::Start {
            session: 1,
            endpoint: "wss://evil.test".into(),
            api_key: "secret".into()
        })
        .is_err());
    }
    #[test]
    fn wire_contract() {
        let e = HostEvent::Ready { session: 7 };
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            r#"{"event":"ready","session":7}"#
        );
        assert!(matches!(
            serde_json::from_str::<HostCommand>(r#"{"op":"mute","session":7,"muted":true}"#)
                .unwrap(),
            HostCommand::Mute { muted: true, .. }
        ));
    }
}

/// Decode only a bounded private UART frame; mismatched session IDs are rejected.
pub fn parse_guest_frame(line: &str) -> Option<(u64, HostCommand)> {
    if line.len() > FRAME_LIMIT * 2 + 64 {
        return None;
    }
    let mut p = line.trim_end_matches(['\r', '\n']).split('\t');
    if p.next()? != "VOICE1" {
        return None;
    }
    let session = p.next()?.parse::<u64>().ok()?;
    let hex = p.next()?;
    if p.next().is_some() || hex.len() % 2 != 0 || hex.len() > FRAME_LIMIT * 2 {
        return None;
    }
    let bytes = hex
        .as_bytes()
        .chunks_exact(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).ok()?, 16).ok())
        .collect::<Option<Vec<_>>>()?;
    let command: HostCommand = serde_json::from_slice(&bytes).ok()?;
    let inner = match &command {
        HostCommand::Start { session, .. }
        | HostCommand::Send { session, .. }
        | HostCommand::Mute { session, .. }
        | HostCommand::Playback { session, .. }
        | HostCommand::Interrupt { session, .. }
        | HostCommand::Stop { session } => *session,
    };
    if inner != session || validate(&command).is_err() {
        return None;
    }
    Some((session, command))
}

#[cfg(all(test, unix))]
mod lifecycle_tests {
    use super::*;
    #[test]
    fn session_prefix_must_match_and_pcm_is_bounded() {
        let json = r#"{"op":"stop","session":2}"#;
        let encoded = alloyos_forge::protocol::encode_hex(json);
        assert!(parse_guest_frame(&format!("VOICE1\t1\t{encoded}\n")).is_none());
        assert!(matches!(
            parse_guest_frame(&format!("VOICE1\t2\t{encoded}\n")),
            Some((2, HostCommand::Stop { session: 2 }))
        ));
        assert!(validate(&HostCommand::Playback {
            session: 2,
            generation: 1,
            pcm: "0000".repeat(16385)
        })
        .is_err());
    }
    #[test]
    fn helper_eof_and_drop_close_process() {
        let mut host = HostVoice::launch(std::path::Path::new("/bin/cat")).unwrap();
        host.command(&HostCommand::Stop { session: 9 }).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        loop {
            match host.try_event() {
                Err(_) => break,
                Ok(None) => {}
                Ok(Some(_)) => panic!("command is not event"),
            };
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}
