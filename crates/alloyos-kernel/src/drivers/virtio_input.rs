//! VirtIO-Input MMIO Keyboard & Tablet/Mouse Driver (`VIRTIO_DEV_INPUT = 18`).
//!
//! Translates raw `virtio_input_event` structures (`EV_KEY`, `EV_REL`, `EV_ABS`) into
//! `alloyos_proto::InputEvent` messages for Ferrofluid (`ferrofluidd`) as well as ASCII/ANSI
//! byte streams for interactive console input inside the QEMU display window.

use alloc::collections::VecDeque;
use core::ptr::{read_volatile, write_volatile};
#[cfg(target_os = "none")]
use core::sync::atomic::{fence, Ordering};
use alloyos_proto::{InputEvent, InputEventKind};
use spin::Mutex;

use crate::drivers::virtio_blk::{VIRTIO_MMIO_BASE, VIRTIO_MMIO_COUNT, VIRTIO_MMIO_STRIDE};

const VIRTIO_MAGIC: u32 = 0x7472_6976;
pub const VIRTIO_DEV_INPUT: u32 = 18;

const REG_MAGIC: usize = 0x000;
const REG_VERSION: usize = 0x004;
const REG_DEVICE_ID: usize = 0x008;
const REG_DEVICE_FEATURES: usize = 0x010;
const REG_DRIVER_FEATURES: usize = 0x020;
const REG_GUEST_PAGE_SIZE: usize = 0x028; // v1 only
const REG_QUEUE_SEL: usize = 0x030;
const REG_QUEUE_NUM_MAX: usize = 0x034;
const REG_QUEUE_NUM: usize = 0x038;
const REG_QUEUE_ALIGN: usize = 0x03C; // v1 only
const REG_QUEUE_PFN: usize = 0x040; // v1 only
const REG_QUEUE_READY: usize = 0x044; // v2 only
const REG_QUEUE_NOTIFY: usize = 0x050;
const REG_STATUS: usize = 0x070;
const REG_QUEUE_DESC_LOW: usize = 0x080; // v2 only
const REG_QUEUE_DESC_HIGH: usize = 0x084;
const REG_QUEUE_AVAIL_LOW: usize = 0x090; // v2 only
const REG_QUEUE_AVAIL_HIGH: usize = 0x094;
const REG_QUEUE_USED_LOW: usize = 0x0A0; // v2 only
const REG_QUEUE_USED_HIGH: usize = 0x0A4;

const STATUS_ACKNOWLEDGE: u32 = 1;
const STATUS_DRIVER: u32 = 2;
const STATUS_DRIVER_OK: u32 = 4;
const STATUS_FEATURES_OK: u32 = 8;

#[cfg(target_os = "none")]
const QUEUE_SIZE: usize = 16;
const VRING_DESC_F_WRITE: u16 = 2;
#[cfg(target_os = "none")]
const MAX_INPUT_SLOTS: usize = 2;

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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VirtioInputRawEvent {
    pub type_: u16,
    pub code: u16,
    pub value: u32,
}

#[cfg(target_os = "none")]
#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct VirtqDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

#[cfg(target_os = "none")]
#[repr(C, align(2))]
#[derive(Clone, Copy)]
struct VirtqAvail {
    flags: u16,
    idx: u16,
    ring: [u16; QUEUE_SIZE],
    used_event: u16,
}

#[cfg(target_os = "none")]
#[repr(C, align(4))]
#[derive(Clone, Copy)]
struct VirtqUsedElem {
    id: u32,
    len: u32,
}

#[cfg(target_os = "none")]
#[repr(C, align(4))]
#[derive(Clone, Copy)]
struct VirtqUsed {
    flags: u16,
    idx: u16,
    ring: [VirtqUsedElem; QUEUE_SIZE],
    avail_event: u16,
}

#[cfg(target_os = "none")]
#[repr(C, align(4096))]
#[derive(Clone, Copy)]
struct VirtQueueMemory {
    desc: [VirtqDesc; QUEUE_SIZE],
    avail: VirtqAvail,
    _pad0: [u8; 4096 - 256 - core::mem::size_of::<VirtqAvail>()],
    used: VirtqUsed,
}

#[cfg(target_os = "none")]
impl VirtQueueMemory {
    const fn zeroed() -> Self {
        Self {
            desc: [VirtqDesc {
                addr: 0,
                len: 0,
                flags: 0,
                next: 0,
            }; QUEUE_SIZE],
            avail: VirtqAvail {
                flags: 0,
                idx: 0,
                ring: [0; QUEUE_SIZE],
                used_event: 0,
            },
            _pad0: [0; 4096 - 256 - core::mem::size_of::<VirtqAvail>()],
            used: VirtqUsed {
                flags: 0,
                idx: 0,
                ring: [VirtqUsedElem { id: 0, len: 0 }; QUEUE_SIZE],
                avail_event: 0,
            },
        }
    }
}

#[cfg(target_os = "none")]
static mut INPUT_VIRTQ_MEM: [VirtQueueMemory; MAX_INPUT_SLOTS] =
    [VirtQueueMemory::zeroed(); MAX_INPUT_SLOTS];

#[cfg(target_os = "none")]
static mut INPUT_EVENT_BUFS: [[VirtioInputRawEvent; QUEUE_SIZE]; MAX_INPUT_SLOTS] = [[VirtioInputRawEvent {
    type_: 0,
    code: 0,
    value: 0,
}; QUEUE_SIZE];
    MAX_INPUT_SLOTS];

#[cfg(target_os = "none")]
#[derive(Clone, Copy)]
struct InputMmioSlot {
    base: usize,
    last_used_idx: u16,
}

pub struct VirtIoInputDevice {
    pub mmio_base: Option<usize>,
    pub pointer_x: i32,
    pub pointer_y: i32,
    display_width: i32,
    display_height: i32,
    shift_down: bool,
    caps_lock: bool,
    #[cfg(target_os = "none")]
    slots: [Option<InputMmioSlot>; MAX_INPUT_SLOTS],
    queue: VecDeque<InputEvent>,
    ascii_queue: VecDeque<u8>,
}

impl VirtIoInputDevice {
    pub const fn new() -> Self {
        Self {
            mmio_base: None,
            pointer_x: 640,
            pointer_y: 400,
            display_width: 1280,
            display_height: 800,
            shift_down: false,
            caps_lock: false,
            #[cfg(target_os = "none")]
            slots: [None; MAX_INPUT_SLOTS],
            queue: VecDeque::new(),
            ascii_queue: VecDeque::new(),
        }
    }

    #[inline]
    unsafe fn read_reg(base: usize, offset: usize) -> u32 {
        read_volatile((base + offset) as *const u32)
    }

    #[inline]
    unsafe fn write_reg(base: usize, offset: usize, val: u32) {
        write_volatile((base + offset) as *mut u32, val);
    }

    pub fn probe() -> Self {
        #[allow(unused_mut)]
        let mut dev = Self::new();
        #[cfg(target_os = "none")]
        unsafe {
            let mut found = 0usize;
            for slot in 0..VIRTIO_MMIO_COUNT {
                if found >= MAX_INPUT_SLOTS {
                    break;
                }
                let base = VIRTIO_MMIO_BASE + slot * VIRTIO_MMIO_STRIDE;
                let magic = Self::read_reg(base, REG_MAGIC);
                let version = Self::read_reg(base, REG_VERSION);
                let dev_id = Self::read_reg(base, REG_DEVICE_ID);
                if magic == VIRTIO_MAGIC
                    && (version == 1 || version == 2)
                    && dev_id == VIRTIO_DEV_INPUT
                {
                    if Self::init_slot(found, base, version) {
                        if dev.mmio_base.is_none() {
                            dev.mmio_base = Some(base);
                        }
                        dev.slots[found] = Some(InputMmioSlot {
                            base,
                            last_used_idx: 0,
                        });
                        found += 1;
                    }
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
            REG_DEVICE_FEATURES,
            REG_DRIVER_FEATURES,
            REG_GUEST_PAGE_SIZE,
            REG_QUEUE_SEL,
            REG_QUEUE_NUM_MAX,
            REG_QUEUE_NUM,
            REG_QUEUE_ALIGN,
            REG_QUEUE_PFN,
            REG_QUEUE_READY,
            REG_QUEUE_NOTIFY,
            REG_STATUS,
            REG_QUEUE_DESC_LOW,
            REG_QUEUE_DESC_HIGH,
            REG_QUEUE_AVAIL_LOW,
            REG_QUEUE_AVAIL_HIGH,
            REG_QUEUE_USED_LOW,
            REG_QUEUE_USED_HIGH,
            STATUS_ACKNOWLEDGE,
            STATUS_DRIVER,
            STATUS_DRIVER_OK,
            STATUS_FEATURES_OK,
            VRING_DESC_F_WRITE,
            Self::read_reg as unsafe fn(usize, usize) -> u32,
            Self::write_reg as unsafe fn(usize, usize, u32),
        );
        dev
    }

    #[cfg(target_os = "none")]
    unsafe fn init_slot(slot_idx: usize, base: usize, version: u32) -> bool {
        Self::write_reg(base, REG_STATUS, 0);
        let mut status = STATUS_ACKNOWLEDGE | STATUS_DRIVER;
        Self::write_reg(base, REG_STATUS, status);

        let _features = Self::read_reg(base, REG_DEVICE_FEATURES);
        Self::write_reg(base, REG_DRIVER_FEATURES, 0);

        if version == 2 {
            status |= STATUS_FEATURES_OK;
            Self::write_reg(base, REG_STATUS, status);
            if (Self::read_reg(base, REG_STATUS) & STATUS_FEATURES_OK) == 0 {
                return false;
            }
        } else {
            Self::write_reg(base, REG_GUEST_PAGE_SIZE, 4096);
        }

        // Queue 0 = eventq
        Self::write_reg(base, REG_QUEUE_SEL, 0);
        let max_q = Self::read_reg(base, REG_QUEUE_NUM_MAX);
        if max_q < QUEUE_SIZE as u32 {
            return false;
        }
        Self::write_reg(base, REG_QUEUE_NUM, QUEUE_SIZE as u32);

        let vq_ptr = &raw mut INPUT_VIRTQ_MEM[slot_idx];
        core::ptr::write_bytes(
            vq_ptr as *mut u8,
            0,
            core::mem::size_of::<VirtQueueMemory>(),
        );

        for i in 0..QUEUE_SIZE {
            INPUT_EVENT_BUFS[slot_idx][i] = VirtioInputRawEvent::default();
            let buf_addr = &raw mut INPUT_EVENT_BUFS[slot_idx][i] as usize as u64;
            (*vq_ptr).desc[i] = VirtqDesc {
                addr: buf_addr,
                len: core::mem::size_of::<VirtioInputRawEvent>() as u32,
                flags: VRING_DESC_F_WRITE,
                next: 0,
            };
            (*vq_ptr).avail.ring[i] = i as u16;
        }
        fence(Ordering::SeqCst);
        write_volatile(&raw mut (*vq_ptr).avail.idx, QUEUE_SIZE as u16);
        fence(Ordering::SeqCst);

        let q_phys = vq_ptr as usize as u64;
        let desc_phys = &raw const (*vq_ptr).desc as usize as u64;
        let avail_phys = &raw const (*vq_ptr).avail as usize as u64;
        let used_phys = &raw const (*vq_ptr).used as usize as u64;

        if version == 1 {
            Self::write_reg(base, REG_QUEUE_ALIGN, 4096);
            Self::write_reg(base, REG_QUEUE_PFN, (q_phys >> 12) as u32);
        } else {
            Self::write_reg(base, REG_QUEUE_DESC_LOW, desc_phys as u32);
            Self::write_reg(base, REG_QUEUE_DESC_HIGH, (desc_phys >> 32) as u32);
            Self::write_reg(base, REG_QUEUE_AVAIL_LOW, avail_phys as u32);
            Self::write_reg(base, REG_QUEUE_AVAIL_HIGH, (avail_phys >> 32) as u32);
            Self::write_reg(base, REG_QUEUE_USED_LOW, used_phys as u32);
            Self::write_reg(base, REG_QUEUE_USED_HIGH, (used_phys >> 32) as u32);
            Self::write_reg(base, REG_QUEUE_READY, 1);
        }

        status |= STATUS_DRIVER_OK;
        Self::write_reg(base, REG_STATUS, status);
        Self::write_reg(base, REG_QUEUE_NOTIFY, 0);
        true
    }

    pub fn poll_hardware(&mut self) {
        #[cfg(target_os = "none")]
        unsafe {
            for slot_idx in 0..MAX_INPUT_SLOTS {
                let Some(mut slot_state) = self.slots[slot_idx] else {
                    continue;
                };
                let vq_ptr = &raw mut INPUT_VIRTQ_MEM[slot_idx];
                loop {
                    fence(Ordering::SeqCst);
                    let used_idx = read_volatile(&raw const (*vq_ptr).used.idx);
                    if slot_state.last_used_idx == used_idx {
                        break;
                    }
                    let ring_idx = (slot_state.last_used_idx as usize) % QUEUE_SIZE;
                    let used_elem = read_volatile(&raw const (*vq_ptr).used.ring[ring_idx]);
                    let desc_idx = (used_elem.id as usize) % QUEUE_SIZE;
                    let raw_ev = read_volatile(&raw const INPUT_EVENT_BUFS[slot_idx][desc_idx]);

                    self.push_raw_event(raw_ev);

                    // Repost descriptor buffer back to avail ring
                    let avail_idx = read_volatile(&raw const (*vq_ptr).avail.idx);
                    write_volatile(
                        &raw mut (*vq_ptr).avail.ring[(avail_idx as usize) % QUEUE_SIZE],
                        desc_idx as u16,
                    );
                    fence(Ordering::SeqCst);
                    write_volatile(&raw mut (*vq_ptr).avail.idx, avail_idx.wrapping_add(1));
                    fence(Ordering::SeqCst);
                    Self::write_reg(slot_state.base, REG_QUEUE_NOTIFY, 0);

                    slot_state.last_used_idx = slot_state.last_used_idx.wrapping_add(1);
                }
                self.slots[slot_idx] = Some(slot_state);
            }
        }
    }

    fn translate_evdev_key_to_ascii(&mut self, code: u16, value: u32) {
        // Track modifier state (KEY_LEFTSHIFT = 42, KEY_RIGHTSHIFT = 54, KEY_CAPSLOCK = 58)
        if code == 42 || code == 54 {
            self.shift_down = value != 0;
            return;
        }
        if code == 58 && value == 1 {
            self.caps_lock = !self.caps_lock;
            return;
        }
        // Only emit characters on key press (1) or autorepeat (2)
        if value != 1 && value != 2 {
            return;
        }

        // Up/Down arrow keys -> ANSI escape sequences for shell history
        if code == 103 {
            self.ascii_queue.push_back(0x1b);
            self.ascii_queue.push_back(b'[');
            self.ascii_queue.push_back(b'A');
            return;
        }
        if code == 108 {
            self.ascii_queue.push_back(0x1b);
            self.ascii_queue.push_back(b'[');
            self.ascii_queue.push_back(b'B');
            return;
        }

        let upper_alpha = self.shift_down ^ self.caps_lock;
        let shifted = self.shift_down;

        let ch: Option<u8> = match code {
            14 | 111 => Some(0x08), // Backspace / Delete
            15 => Some(b'\t'),
            28 | 96 => Some(b'\n'), // Enter / Keypad Enter
            57 => Some(b' '),       // Space
            2 => Some(if shifted { b'!' } else { b'1' }),
            3 => Some(if shifted { b'@' } else { b'2' }),
            4 => Some(if shifted { b'#' } else { b'3' }),
            5 => Some(if shifted { b'$' } else { b'4' }),
            6 => Some(if shifted { b'%' } else { b'5' }),
            7 => Some(if shifted { b'^' } else { b'6' }),
            8 => Some(if shifted { b'&' } else { b'7' }),
            9 => Some(if shifted { b'*' } else { b'8' }),
            10 => Some(if shifted { b'(' } else { b'9' }),
            11 => Some(if shifted { b')' } else { b'0' }),
            12 => Some(if shifted { b'_' } else { b'-' }),
            13 => Some(if shifted { b'+' } else { b'=' }),
            26 => Some(if shifted { b'{' } else { b'[' }),
            27 => Some(if shifted { b'}' } else { b']' }),
            39 => Some(if shifted { b':' } else { b';' }),
            40 => Some(if shifted { b'"' } else { b'\'' }),
            41 => Some(if shifted { b'~' } else { b'`' }),
            43 => Some(if shifted { b'|' } else { b'\\' }),
            51 => Some(if shifted { b'<' } else { b',' }),
            52 => Some(if shifted { b'>' } else { b'.' }),
            53 => Some(if shifted { b'?' } else { b'/' }),
            16 => Some(if upper_alpha { b'Q' } else { b'q' }),
            17 => Some(if upper_alpha { b'W' } else { b'w' }),
            18 => Some(if upper_alpha { b'E' } else { b'e' }),
            19 => Some(if upper_alpha { b'R' } else { b'r' }),
            20 => Some(if upper_alpha { b'T' } else { b't' }),
            21 => Some(if upper_alpha { b'Y' } else { b'y' }),
            22 => Some(if upper_alpha { b'U' } else { b'u' }),
            23 => Some(if upper_alpha { b'I' } else { b'i' }),
            24 => Some(if upper_alpha { b'O' } else { b'o' }),
            25 => Some(if upper_alpha { b'P' } else { b'p' }),
            30 => Some(if upper_alpha { b'A' } else { b'a' }),
            31 => Some(if upper_alpha { b'S' } else { b's' }),
            32 => Some(if upper_alpha { b'D' } else { b'd' }),
            33 => Some(if upper_alpha { b'F' } else { b'f' }),
            34 => Some(if upper_alpha { b'G' } else { b'g' }),
            35 => Some(if upper_alpha { b'H' } else { b'h' }),
            36 => Some(if upper_alpha { b'J' } else { b'j' }),
            37 => Some(if upper_alpha { b'K' } else { b'k' }),
            38 => Some(if upper_alpha { b'L' } else { b'l' }),
            44 => Some(if upper_alpha { b'Z' } else { b'z' }),
            45 => Some(if upper_alpha { b'X' } else { b'x' }),
            46 => Some(if upper_alpha { b'C' } else { b'c' }),
            47 => Some(if upper_alpha { b'V' } else { b'v' }),
            48 => Some(if upper_alpha { b'B' } else { b'b' }),
            49 => Some(if upper_alpha { b'N' } else { b'n' }),
            50 => Some(if upper_alpha { b'M' } else { b'm' }),
            _ => None,
        };

        if let Some(byte) = ch {
            self.ascii_queue.push_back(byte);
        }
    }

    /// Translate a raw VirtIO-Input event into a `alloyos_proto::InputEvent` and queue it.
    pub fn push_raw_event(&mut self, raw: VirtioInputRawEvent) -> Option<InputEvent> {
        match raw.type_ {
            EV_KEY => {
                self.translate_evdev_key_to_ascii(raw.code, raw.value);
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
                    self.pointer_x = self.pointer_x.saturating_add(delta).clamp(0, self.display_width - 1);
                } else if raw.code == REL_Y {
                    self.pointer_y = self.pointer_y.saturating_add(delta).clamp(0, self.display_height - 1);
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
                    self.pointer_x = ((scaled * self.display_width as i64) / 32768).clamp(0, (self.display_width - 1) as i64) as i32;
                } else if raw.code == ABS_Y {
                    self.pointer_y = ((scaled * self.display_height as i64) / 32768).clamp(0, (self.display_height - 1) as i64) as i32;
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
        self.poll_hardware();
        self.queue.pop_front()
    }

    pub fn set_display_size(&mut self, width: u32, height: u32) {
        self.display_width = width.clamp(1, i32::MAX as u32) as i32;
        self.display_height = height.clamp(1, i32::MAX as u32) as i32;
        self.pointer_x = self.pointer_x.clamp(0, self.display_width - 1);
        self.pointer_y = self.pointer_y.clamp(0, self.display_height - 1);
    }

    /// System UI consumes raw evdev events, not the debug shell's ASCII queue.
    pub fn poll_ui_event(&mut self) -> Option<InputEvent> {
        self.poll_hardware();
        self.ascii_queue.clear();
        self.queue.pop_front()
    }

    pub fn poll_ascii_byte(&mut self) -> Option<u8> {
        self.poll_hardware();
        self.ascii_queue.pop_front()
    }
}

pub static VIRTIO_INPUT: Mutex<VirtIoInputDevice> = Mutex::new(VirtIoInputDevice::new());

pub fn init() {
    *VIRTIO_INPUT.lock() = VirtIoInputDevice::probe();
}

pub fn poll_ascii() -> Option<u8> {
    VIRTIO_INPUT.lock().poll_ascii_byte()
}
