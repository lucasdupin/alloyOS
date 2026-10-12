//! VirtIO-Input MMIO Keyboard & Tablet/Mouse Driver (`VIRTIO_DEV_INPUT = 18`).
//!
//! Translates raw `virtio_input_event` structures (`EV_KEY`, `EV_REL`, `EV_ABS`) into
//! `metalos_proto::InputEvent` messages for Mercury (`mercuryd`).

use alloc::collections::VecDeque;
use core::ptr::read_volatile;
use metalos_proto::{InputEvent, InputEventKind};
use spin::Mutex;

use crate::drivers::virtio_blk::{VIRTIO_MMIO_BASE, VIRTIO_MMIO_COUNT, VIRTIO_MMIO_STRIDE};

const VIRTIO_MAGIC: u32 = 0x7472_6976;
pub const VIRTIO_DEV_INPUT: u32 = 18;

const REG_MAGIC: usize = 0x000;
const REG_VERSION: usize = 0x004;
const REG_DEVICE_ID: usize = 0x008;

// Linux evdev event types used by VirtIO-Input
pub const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub const EV_REL: u16 = 0x02;
pub const EV_ABS: u16 = 0x03;

pub const ABS_X: u16 = 0x00;
pub const ABS_Y: u16 = 0x01;
pub const REL_X: u16 = 0x00;
pub const REL_Y: u16 = 0x01;

pub const BTN_MOUSE_LEFT: u16 = 0x110;
pub const BTN_MOUSE_RIGHT: u16 = 0x111;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VirtioInputRawEvent {
    pub type_: u16,
    pub code: u16,
    pub value: u32,
}

pub struct VirtIoInputDevice {
    pub mmio_base: Option<usize>,
    pub pointer_x: i32,
    pub pointer_y: i32,
    queue: VecDeque<InputEvent>,
}

impl VirtIoInputDevice {
    pub const fn new() -> Self {
        Self {
            mmio_base: None,
            pointer_x: 640,
            pointer_y: 400,
            queue: VecDeque::new(),
        }
    }

    pub fn probe() -> Self {
        #[allow(unused_mut)]
        let mut dev = Self::new();
        #[cfg(target_os = "none")]
        unsafe {
            for slot in 0..VIRTIO_MMIO_COUNT {
                let base = VIRTIO_MMIO_BASE + slot * VIRTIO_MMIO_STRIDE;
                let magic = read_volatile((base + REG_MAGIC) as *const u32);
                let version = read_volatile((base + REG_VERSION) as *const u32);
                let dev_id = read_volatile((base + REG_DEVICE_ID) as *const u32);
                if magic == VIRTIO_MAGIC && (version == 1 || version == 2) && dev_id == VIRTIO_DEV_INPUT {
                    dev.mmio_base = Some(base);
                    break;
                }
            }
        }
        let _ = (
            VIRTIO_MMIO_BASE,
            VIRTIO_MMIO_COUNT,
            VIRTIO_MMIO_STRIDE,
            VIRTIO_MAGIC,
            REG_MAGIC,
            REG_VERSION,
            REG_DEVICE_ID,
            read_volatile::<u32>,
        );
        dev
    }

    /// Translate a raw VirtIO-Input event into a `metalos_proto::InputEvent` and queue it.
    pub fn push_raw_event(&mut self, raw: VirtioInputRawEvent) -> Option<InputEvent> {
        match raw.type_ {
            EV_KEY => {
                let is_down = raw.value != 0;
                let kind = if raw.code >= BTN_MOUSE_LEFT {
                    if is_down {
                        InputEventKind::PointerButtonDown
                    } else {
                        InputEventKind::PointerButtonUp
                    }
                } else if is_down {
                    InputEventKind::KeyDown
                } else {
                    InputEventKind::KeyUp
                };
                let ev = InputEvent {
                    kind,
                    code: raw.code,
                    x: self.pointer_x,
                    y: self.pointer_y,
                };
                self.queue.push_back(ev);
                Some(ev)
            }
            EV_REL => {
                let delta = raw.value as i32;
                if raw.code == REL_X {
                    self.pointer_x = (self.pointer_x + delta).clamp(0, 1279);
                } else if raw.code == REL_Y {
                    self.pointer_y = (self.pointer_y + delta).clamp(0, 799);
                }
                let ev = InputEvent {
                    kind: InputEventKind::PointerMove,
                    code: raw.code,
                    x: self.pointer_x,
                    y: self.pointer_y,
                };
                self.queue.push_back(ev);
                Some(ev)
            }
            EV_ABS => {
                // QEMU virtio-tablet reports 0..32767
                let scaled = raw.value as i64;
                if raw.code == ABS_X {
                    self.pointer_x = ((scaled * 1280) / 32768).clamp(0, 1279) as i32;
                } else if raw.code == ABS_Y {
                    self.pointer_y = ((scaled * 800) / 32768).clamp(0, 799) as i32;
                }
                let ev = InputEvent {
                    kind: InputEventKind::PointerMove,
                    code: raw.code,
                    x: self.pointer_x,
                    y: self.pointer_y,
                };
                self.queue.push_back(ev);
                Some(ev)
            }
            _ => None,
        }
    }

    pub fn poll_event(&mut self) -> Option<InputEvent> {
        self.queue.pop_front()
    }
}

pub static VIRTIO_INPUT: Mutex<VirtIoInputDevice> = Mutex::new(VirtIoInputDevice::new());

pub fn init() {
    *VIRTIO_INPUT.lock() = VirtIoInputDevice::probe();
}
