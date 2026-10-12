use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use metalos_proto::{InputEvent, InputEventKind, SyscallNo};

use crate::auth::{AgentUiToken, CallerPrincipal, MercuryError, SecurityGate};
use crate::widgets::{AiConsoleView, MercuryWidget};

/// Queues and dispatches `metalos_proto::InputEvent` (keyboard key-down/up with
/// ASCII conversion, pointer motion/clicks) to the active Mercury AI Console.
#[derive(Debug, Clone)]
pub struct InputMultiplexer {
    queue: VecDeque<InputEvent>,
    pub shift_pressed: bool,
    pub pointer_x: i32,
    pub pointer_y: i32,
    pub pointer_buttons: u8,
}

impl Default for InputMultiplexer {
    fn default() -> Self {
        Self::new()
    }
}

impl InputMultiplexer {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            shift_pressed: false,
            pointer_x: 0,
            pointer_y: 0,
            pointer_buttons: 0,
        }
    }

    /// Enqueues a raw hardware `InputEvent` from VirtIO-Input.
    pub fn push_event(&mut self, event: InputEvent) {
        self.queue.push_back(event);
    }

    /// Number of queued input events awaiting dispatch.
    pub fn pending_count(&self) -> usize {
        self.queue.len()
    }

    /// Pops and dispatches all pending events to the active `AiConsoleView`.
    /// Returns the number of events processed.
    pub fn dispatch_to_console(&mut self, console: &mut AiConsoleView) -> usize {
        let mut processed = 0;
        while let Some(ev) = self.queue.pop_front() {
            processed += 1;
            match ev.kind {
                InputEventKind::KeyDown => {
                    if ev.code == 42 || ev.code == 54 {
                        self.shift_pressed = true;
                    } else if let Some(ascii) = Self::keycode_to_ascii(ev.code, self.shift_pressed)
                    {
                        console.push_input_char(ascii);
                    }
                }
                InputEventKind::KeyUp => {
                    if ev.code == 42 || ev.code == 54 {
                        self.shift_pressed = false;
                    }
                }
                InputEventKind::PointerMove => {
                    self.pointer_x = ev.x;
                    self.pointer_y = ev.y;
                }
                InputEventKind::PointerButtonDown => {
                    self.pointer_x = ev.x;
                    self.pointer_y = ev.y;
                    self.pointer_buttons |= 1 << (ev.code & 7);
                }
                InputEventKind::PointerButtonUp => {
                    self.pointer_x = ev.x;
                    self.pointer_y = ev.y;
                    self.pointer_buttons &= !(1 << (ev.code & 7));
                }
            }
        }
        processed
    }

    /// Converts either direct printable ASCII codes (`0x20..=0x7E`, `\n`, `\x08`)
    /// or Linux/VirtIO evdev keycodes into an ASCII `char`.
    pub const fn keycode_to_ascii(code: u16, shift: bool) -> Option<char> {
        match code {
            0x08 | 0x7F | 14 => Some('\x08'), // Backspace (ASCII BS/DEL or evdev KEY_BACKSPACE)
            0x0A | 0x0D | 28 => Some('\n'),   // Enter/Newline (ASCII LF/CR or evdev KEY_ENTER)
            0x20..=0x7E => {
                let c = code as u8 as char;
                if shift && c >= 'a' && c <= 'z' {
                    Some((c as u8 - 32) as char)
                } else {
                    Some(c)
                }
            }
            // Non-ASCII evdev scan codes (< 0x20)
            2 => Some(if shift { '!' } else { '1' }),
            3 => Some(if shift { '@' } else { '2' }),
            4 => Some(if shift { '#' } else { '3' }),
            5 => Some(if shift { '$' } else { '4' }),
            6 => Some(if shift { '%' } else { '5' }),
            7 => Some(if shift { '^' } else { '6' }),
            9 => Some(if shift { '*' } else { '8' }),
            11 => Some(if shift { ')' } else { '0' }),
            16 => Some(if shift { 'Q' } else { 'q' }),
            17 => Some(if shift { 'W' } else { 'w' }),
            18 => Some(if shift { 'E' } else { 'e' }),
            19 => Some(if shift { 'R' } else { 'r' }),
            20 => Some(if shift { 'T' } else { 't' }),
            21 => Some(if shift { 'Y' } else { 'y' }),
            22 => Some(if shift { 'U' } else { 'u' }),
            23 => Some(if shift { 'I' } else { 'i' }),
            24 => Some(if shift { 'O' } else { 'o' }),
            25 => Some(if shift { 'P' } else { 'p' }),
            30 => Some(if shift { 'A' } else { 'a' }),
            31 => Some(if shift { 'S' } else { 's' }),
            _ => None,
        }
    }
}

/// UI commands submitted over the `SYS_SHM` shared-memory IPC ring buffer to `mercuryd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MercuryIpcCommand {
    /// Creates a new surface/window (only permitted for the AI Agent).
    CreateSurface {
        title: String,
        width: u32,
        height: u32,
    },
    /// Replaces the generative UI widget tree displayed alongside the AI console.
    SubmitWidgets {
        widgets: Vec<MercuryWidget>,
    },
    /// Mutates or appends a single generative UI widget.
    UpsertWidget {
        index: usize,
        widget: MercuryWidget,
    },
    /// Streams a chat token chunk into the active `AiConsoleView`.
    StreamChatToken {
        token: String,
    },
    /// Commits the current streaming response line in the `AiConsoleView`.
    FinishStreamingTurn,
    /// Switches the active frontier model badge (`Gemini`, `Claude`, `ChatGPT`).
    SetActiveModel {
        model: String,
    },
}

/// Shared-memory ring simulation (`SyscallNo::ShmGet` / `SyscallNo::ShmAt`)
/// allowing the authenticated AI agent runtime to submit UI trees, streaming
/// chat tokens, and widget updates to `mercuryd`.
pub struct MercuryIpcChannel {
    pub shm_id: u64,
    pub capacity: usize,
    security_gate: SecurityGate,
    ring: VecDeque<MercuryIpcCommand>,
}

impl MercuryIpcChannel {
    pub const DEFAULT_SHM_KEY: u64 = (SyscallNo::ShmGet as u64) << 32 | 0x4D45_5243;

    pub fn new(capacity: usize, expected_token: AgentUiToken) -> Self {
        Self {
            shm_id: Self::DEFAULT_SHM_KEY,
            capacity: capacity.max(1),
            security_gate: SecurityGate::new(expected_token),
            ring: VecDeque::new(),
        }
    }

    /// Submits a UI command into the shared-memory ring buffer after enforcing
    /// the Agent-Exclusive UI Security Gate.
    ///
    /// Any attempt by `CallerPrincipal::DeveloperMcpApp` or `CallerPrincipal::Unauthenticated`
    /// is immediately rejected with `MercuryError::UiCreationForbiddenForApp`.
    pub fn send(
        &mut self,
        principal: &CallerPrincipal,
        token: Option<&AgentUiToken>,
        command: MercuryIpcCommand,
    ) -> Result<(), MercuryError> {
        self.security_gate.authorize(principal, token)?;
        if self.ring.len() >= self.capacity {
            return Err(MercuryError::IpcRingFull);
        }
        self.ring.push_back(command);
        Ok(())
    }

    /// Pops the next authenticated UI command from the shared-memory ring buffer.
    pub fn recv(&mut self) -> Option<MercuryIpcCommand> {
        self.ring.pop_front()
    }

    /// Number of queued commands in the ring buffer.
    pub fn len(&self) -> usize {
        self.ring.len()
    }

    /// Returns `true` if no commands are waiting in the ring buffer.
    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }
}
