//! Proposed browser-service wire contract (not yet granted by the kernel).
//!
//! Only Ferrofluid should receive the controller endpoint; apps never receive a
//! browser endpoint or pixel-surface capability. The supervisor must enforce
//! this grant: a sender-provided role or PID is not authority. Browser replies
//! must be checked against the current view generation before presentation.
//!
//! All integers are little endian. Packets are bounded to fit process IPC.
//! Navigation checks here are scheme/size framing checks. The engine must parse
//! and canonicalize URLs, enforce HTTPS across redirects, reject credentials,
//! and report its committed URL. Ferrofluid derives trusted origin from that
//! canonical URL; page titles and user-provided URLs are never origin evidence.
//! Frames are BGRA8, top-down, straight alpha; transfer uses BeginFrame followed
//! by ordered FrameChunk packets and EndFrame. No pointers cross this boundary.

pub const VERSION: u16 = 1;
pub const HEADER_BYTES: usize = 28;
pub const MAX_PACKET_BYTES: usize = 1024;
pub const MAX_PAYLOAD_BYTES: usize = MAX_PACKET_BYTES - HEADER_BYTES;
pub const MAX_URL_BYTES: usize = 768;
pub const MAX_TEXT_BYTES: usize = 768;
pub const MAX_DIMENSION: u32 = 4096;
pub const MAX_FRAME_BYTES: u32 = 32 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Truncated,
    TooLarge,
    Version,
    Opcode,
    InvalidPayload,
    InvalidIdentity,
    StaleFrame,
    IncompleteFrame,
}

/// View IDs are assigned by Ferrofluid. A fresh generation is required after
/// destruction or service restart; zero is reserved for invalid identities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct View {
    pub id: u64,
    pub generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum Opcode {
    /// width:u32, height:u32, device scale in thousandths:u32.
    Create = 1,
    Destroy = 2,
    /// Absolute UTF-8 HTTPS URL, or about:blank. Local fixture loading is a
    /// separate service startup option, never an app-supplied file URL.
    Navigate = 3,
    /// Same payload as Create. Dimensions are physical pixels.
    Resize = 4,
    /// Pointer phase:u8 (0 move, 1 down, 2 up, 3 cancel), x:i32, y:i32.
    Pointer = 5,
    /// Horizontal and vertical scroll deltas in physical pixels: i32, i32.
    Scroll = 6,
    /// USB HID usage:u16, down:u8, modifiers:u8 (shift/control/alt/meta bits).
    Key = 7,
    /// Committed UTF-8 text; composition is sent separately.
    CommitText = 8,
    /// UTF-8 preedit text; empty cancels composition. Selection is not yet
    /// supported: the initial implementation places the caret at its end.
    Composition = 9,
    /// focused:u8; losing focus cancels input/composition in the engine.
    Focus = 10,
    Back = 11,
    Forward = 12,
    Reload = 13,
    Stop = 14,
    /// Browser replies below. Created has no payload.
    Created = 128,
    /// Current committed URL (same restrictions as Navigate).
    Url = 129,
    /// UTF-8 page title, untrusted and never a substitute for origin display.
    Title = 130,
    /// 0 idle, 1 loading, 2 failed.
    Loading = 131,
    /// focused:u8, editable:u8. Drives Ferrofluid keyboard visibility.
    Editing = 132,
    /// width, height, stride, damage x/y/width/height: seven u32 values.
    /// Sequence identifies this frame until EndFrame.
    BeginFrame = 133,
    /// offset:u32 followed by pixel bytes. Offsets must be contiguous.
    FrameChunk = 134,
    EndFrame = 135,
    /// Numeric service-defined reason:u32. No process-private diagnostics.
    Crashed = 136,
    Destroyed = 137,
}

impl Opcode {
    pub fn is_reply(self) -> bool {
        (self as u16) >= 128
    }

    fn decode(value: u16) -> Result<Self, Error> {
        Ok(match value {
            1 => Self::Create,
            2 => Self::Destroy,
            3 => Self::Navigate,
            4 => Self::Resize,
            5 => Self::Pointer,
            6 => Self::Scroll,
            7 => Self::Key,
            8 => Self::CommitText,
            9 => Self::Composition,
            10 => Self::Focus,
            11 => Self::Back,
            12 => Self::Forward,
            13 => Self::Reload,
            14 => Self::Stop,
            128 => Self::Created,
            129 => Self::Url,
            130 => Self::Title,
            131 => Self::Loading,
            132 => Self::Editing,
            133 => Self::BeginFrame,
            134 => Self::FrameChunk,
            135 => Self::EndFrame,
            136 => Self::Crashed,
            137 => Self::Destroyed,
            _ => return Err(Error::Opcode),
        })
    }
}

/// Header: version:u16, opcode:u16, view:u64, generation:u64,
/// sequence:u32, payload length:u32. Sequence is supplied by the sender;
/// frame packets share a sequence. It is not an authorization credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Packet<'a> {
    pub opcode: Opcode,
    pub view: View,
    pub sequence: u32,
    pub payload: &'a [u8],
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn text_valid(bytes: &[u8], limit: usize) -> bool {
    bytes.len() <= limit && core::str::from_utf8(bytes).is_ok() && !bytes.iter().any(|b| *b == 0)
}

fn url_valid(bytes: &[u8]) -> bool {
    text_valid(bytes, MAX_URL_BYTES)
        && !bytes.iter().any(|b| *b <= 0x20 || *b == 0x7f)
        && (bytes == b"about:blank" || (bytes.starts_with(b"https://") && bytes.len() > 8))
}

fn viewport_valid(bytes: &[u8]) -> bool {
    if bytes.len() != 12 {
        return false;
    }
    let width = u32_at(bytes, 0);
    let height = u32_at(bytes, 4);
    let scale = u32_at(bytes, 8);
    width > 0
        && height > 0
        && width <= MAX_DIMENSION
        && height <= MAX_DIMENSION
        && width as u64 * height as u64 * 4 <= MAX_FRAME_BYTES as u64
        && (250..=8000).contains(&scale)
}

impl<'a> Packet<'a> {
    pub fn validate(&self) -> Result<(), Error> {
        if self.view.id == 0 || self.view.generation == 0 {
            return Err(Error::InvalidIdentity);
        }
        let p = self.payload;
        if p.len() > MAX_PAYLOAD_BYTES {
            return Err(Error::TooLarge);
        }
        let valid = match self.opcode {
            Opcode::Create | Opcode::Resize => viewport_valid(p),
            Opcode::Navigate | Opcode::Url => url_valid(p),
            Opcode::CommitText | Opcode::Composition | Opcode::Title => {
                text_valid(p, MAX_TEXT_BYTES)
            }
            Opcode::Pointer => p.len() == 9 && p[0] <= 3,
            Opcode::Scroll => p.len() == 8,
            Opcode::Key => p.len() == 4 && p[2] <= 1 && p[3] & !15 == 0,
            Opcode::Focus => p.len() == 1 && p[0] <= 1,
            Opcode::Loading => p.len() == 1 && p[0] <= 2,
            Opcode::Editing => p.len() == 2 && p[0] <= 1 && p[1] <= 1 && (p[0] == 1 || p[1] == 0),
            Opcode::BeginFrame => FrameInfo::decode(p).is_ok(),
            Opcode::FrameChunk => p.len() > 4,
            Opcode::Crashed => p.len() == 4,
            _ => p.is_empty(),
        };
        if valid {
            Ok(())
        } else {
            Err(Error::InvalidPayload)
        }
    }

    pub fn decode(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() < HEADER_BYTES {
            return Err(Error::Truncated);
        }
        if bytes.len() > MAX_PACKET_BYTES {
            return Err(Error::TooLarge);
        }
        if u16::from_le_bytes([bytes[0], bytes[1]]) != VERSION {
            return Err(Error::Version);
        }
        let packet = Self {
            opcode: Opcode::decode(u16::from_le_bytes([bytes[2], bytes[3]]))?,
            view: View {
                id: u64::from_le_bytes(bytes[4..12].try_into().unwrap()),
                generation: u64::from_le_bytes(bytes[12..20].try_into().unwrap()),
            },
            sequence: u32_at(bytes, 20),
            payload: &bytes[HEADER_BYTES..],
        };
        if u32_at(bytes, 24) as usize != packet.payload.len() {
            return Err(Error::InvalidPayload);
        }
        packet.validate()?;
        Ok(packet)
    }

    pub fn encode(&self, output: &mut [u8]) -> Result<usize, Error> {
        self.validate()?;
        let len = HEADER_BYTES + self.payload.len();
        if output.len() < len {
            return Err(Error::Truncated);
        }
        output[..2].copy_from_slice(&VERSION.to_le_bytes());
        output[2..4].copy_from_slice(&(self.opcode as u16).to_le_bytes());
        output[4..12].copy_from_slice(&self.view.id.to_le_bytes());
        output[12..20].copy_from_slice(&self.view.generation.to_le_bytes());
        output[20..24].copy_from_slice(&self.sequence.to_le_bytes());
        output[24..28].copy_from_slice(&(self.payload.len() as u32).to_le_bytes());
        output[28..len].copy_from_slice(self.payload);
        Ok(len)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameInfo {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub damage: crate::DirtyRect,
}

impl FrameInfo {
    pub fn decode(p: &[u8]) -> Result<Self, Error> {
        if p.len() != 28 {
            return Err(Error::InvalidPayload);
        }
        let frame = Self {
            width: u32_at(p, 0),
            height: u32_at(p, 4),
            stride: u32_at(p, 8),
            damage: crate::DirtyRect::new(
                u32_at(p, 12),
                u32_at(p, 16),
                u32_at(p, 20),
                u32_at(p, 24),
            ),
        };
        frame.byte_len()?;
        Ok(frame)
    }

    /// Full backing frame is transferred; damage only limits presentation work.
    pub fn byte_len(&self) -> Result<u32, Error> {
        let d = self.damage;
        let len = self.stride as u64 * self.height as u64;
        if self.width == 0
            || self.height == 0
            || self.width > MAX_DIMENSION
            || self.height > MAX_DIMENSION
            || self.stride < self.width * 4
            || self.stride % 4 != 0
            || len > MAX_FRAME_BYTES as u64
            || d.width == 0
            || d.height == 0
            || d.x as u64 + d.width as u64 > self.width as u64
            || d.y as u64 + d.height as u64 > self.height as u64
        {
            return Err(Error::InvalidPayload);
        }
        Ok(len as u32)
    }
}

/// Allocation-free transfer checker. The consumer owns a bounded staging buffer
/// and publishes it only after `finish` succeeds. Drop this state on resize,
/// crash, destroy, or replacement BeginFrame. Reject replies from older views.
#[derive(Debug)]
pub struct FrameTransfer {
    view: View,
    sequence: u32,
    expected: u32,
    received: u32,
}

impl FrameTransfer {
    pub fn begin(packet: &Packet<'_>, current_view: View) -> Result<Self, Error> {
        packet.validate()?;
        if packet.view != current_view {
            return Err(Error::StaleFrame);
        }
        if packet.opcode != Opcode::BeginFrame {
            return Err(Error::Opcode);
        }
        Ok(Self {
            view: packet.view,
            sequence: packet.sequence,
            expected: FrameInfo::decode(packet.payload)?.byte_len()?,
            received: 0,
        })
    }

    /// Returns offset and bytes to copy only after validation. Update staging
    /// storage synchronously before processing the next packet.
    pub fn chunk<'a>(&mut self, packet: &Packet<'a>) -> Result<(u32, &'a [u8]), Error> {
        packet.validate()?;
        self.matches(packet)?;
        if packet.opcode != Opcode::FrameChunk {
            return Err(Error::Opcode);
        }
        let offset = u32_at(packet.payload, 0);
        let data = &packet.payload[4..];
        let end = offset as u64 + data.len() as u64;
        if offset != self.received || end > self.expected as u64 {
            return Err(Error::InvalidPayload);
        }
        self.received = end as u32;
        Ok((offset, data))
    }

    pub fn finish(self, packet: &Packet<'_>) -> Result<(), Error> {
        packet.validate()?;
        self.matches(packet)?;
        if packet.opcode != Opcode::EndFrame {
            return Err(Error::Opcode);
        }
        if self.received != self.expected {
            return Err(Error::IncompleteFrame);
        }
        Ok(())
    }

    fn matches(&self, packet: &Packet<'_>) -> Result<(), Error> {
        if packet.view != self.view || packet.sequence != self.sequence {
            Err(Error::StaleFrame)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const VIEW: View = View {
        id: 1,
        generation: 2,
    };
    fn packet(opcode: Opcode, payload: &[u8]) -> Packet<'_> {
        Packet {
            opcode,
            view: VIEW,
            sequence: 42,
            payload,
        }
    }
    fn frame_bytes() -> [u8; 28] {
        let mut bytes = [0; 28];
        for (index, value) in [2u32, 1, 8, 0, 0, 2, 1].into_iter().enumerate() {
            bytes[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn wire_roundtrip_and_untrusted_header_rejection() {
        let original = packet(Opcode::Navigate, b"https://example.org/form");
        let mut buffer = [0; MAX_PACKET_BYTES];
        let len = original.encode(&mut buffer).unwrap();
        assert_eq!(Packet::decode(&buffer[..len]), Ok(original));
        for prefix in 0..len {
            assert!(Packet::decode(&buffer[..prefix]).is_err());
        }
        buffer[0] = 0;
        assert_eq!(Packet::decode(&buffer[..len]), Err(Error::Version));
        buffer[0] = VERSION as u8;
        buffer[2..4].copy_from_slice(&u16::MAX.to_le_bytes());
        assert_eq!(Packet::decode(&buffer[..len]), Err(Error::Opcode));
    }

    #[test]
    fn rejects_unsafe_or_unbounded_navigation_and_text() {
        for url in [
            b"file:///secret".as_slice(),
            b"javascript:alert(1)",
            b"http://example.org",
            b"https://",
            b"https://a\nHost:b",
            b"https://a\0",
        ] {
            assert_eq!(
                packet(Opcode::Navigate, url).validate(),
                Err(Error::InvalidPayload)
            );
        }
        assert!(packet(Opcode::Navigate, b"about:blank").validate().is_ok());
        assert!(packet(Opcode::Title, &[0xff]).validate().is_err());
        assert!(packet(Opcode::Composition, b"").validate().is_ok());
        assert!(packet(Opcode::CommitText, &[b'x'; MAX_TEXT_BYTES + 1])
            .validate()
            .is_err());
        assert_eq!(
            packet(Opcode::FrameChunk, &[0; MAX_PAYLOAD_BYTES + 1]).validate(),
            Err(Error::TooLarge)
        );
    }

    #[test]
    fn rejects_invalid_dimensions_stride_and_overflowing_damage() {
        let valid = FrameInfo::decode(&frame_bytes()).unwrap();
        assert_eq!(valid.byte_len(), Ok(8));
        let mut frame = valid;
        frame.stride = u32::MAX;
        assert!(frame.byte_len().is_err());
        frame = valid;
        frame.damage.x = u32::MAX;
        assert!(frame.byte_len().is_err());
        frame = valid;
        frame.width = MAX_DIMENSION + 1;
        assert!(frame.byte_len().is_err());
        frame = valid;
        frame.stride = 4;
        assert!(frame.byte_len().is_err());
        let mut viewport = [0u8; 12];
        viewport[..4].copy_from_slice(&1206u32.to_le_bytes());
        viewport[4..8].copy_from_slice(&2622u32.to_le_bytes());
        viewport[8..].copy_from_slice(&3000u32.to_le_bytes());
        assert!(packet(Opcode::Create, &viewport).validate().is_ok());
        viewport[8..].fill(0);
        assert!(packet(Opcode::Resize, &viewport).validate().is_err());
    }

    #[test]
    fn publishes_only_complete_ordered_current_generation_frames() {
        let metadata = frame_bytes();
        let begin = packet(Opcode::BeginFrame, &metadata);
        let mut transfer = FrameTransfer::begin(&begin, VIEW).unwrap();
        let chunk = packet(Opcode::FrameChunk, &[0, 0, 0, 0, 1, 2, 3, 4]);
        assert_eq!(transfer.chunk(&chunk).unwrap(), (0, &[1, 2, 3, 4][..]));
        assert!(transfer.chunk(&chunk).is_err()); // duplicate chunk
        let mut stale = packet(Opcode::FrameChunk, &[4, 0, 0, 0, 5, 6, 7, 8]);
        stale.view.generation -= 1;
        assert_eq!(transfer.chunk(&stale), Err(Error::StaleFrame));
        stale.view = VIEW;
        stale.sequence += 1;
        assert_eq!(transfer.chunk(&stale), Err(Error::StaleFrame));
        stale.sequence -= 1;
        assert!(transfer.chunk(&stale).is_ok());
        assert_eq!(transfer.finish(&packet(Opcode::EndFrame, &[])), Ok(()));
        let incomplete = FrameTransfer::begin(&begin, VIEW).unwrap();
        assert_eq!(
            incomplete.finish(&packet(Opcode::EndFrame, &[])),
            Err(Error::IncompleteFrame)
        );
        assert!(FrameTransfer::begin(
            &begin,
            View {
                id: 1,
                generation: 3
            }
        )
        .is_err());
    }

    #[test]
    fn validates_input_and_focus_contract() {
        assert!(packet(Opcode::Pointer, &[4; 9]).validate().is_err());
        assert!(packet(Opcode::Key, &[1, 0, 1, 0x80]).validate().is_err());
        assert!(packet(Opcode::Editing, &[0, 1]).validate().is_err());
        assert!(packet(Opcode::Focus, &[2]).validate().is_err());
        let invalid = Packet {
            view: View {
                id: 0,
                generation: 1,
            },
            ..packet(Opcode::Destroy, &[])
        };
        assert_eq!(invalid.validate(), Err(Error::InvalidIdentity));
        assert!(Opcode::BeginFrame.is_reply());
        assert!(!Opcode::Navigate.is_reply());
    }
}
