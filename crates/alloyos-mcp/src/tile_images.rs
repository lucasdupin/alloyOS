//! Tile images are referenced once and loaded independently of app text/actions.
use alloc::{string::String, vec::Vec};
use serde::{Deserialize, Serialize};

pub const MAX_IMAGE_BYTES: usize = 128 * 1024;
pub const MAX_IMAGE_CACHE_BYTES: usize = 128 * 1024;
pub const MAX_IMAGES: usize = 8;
pub const MAX_IMAGE_DIMENSION: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TileImage {
    pub src: String,
    #[serde(default)]
    pub alt: String,
}
impl TileImage {
    pub fn valid_for(&self, tile_id: &str) -> bool {
        if self.src.len() > 2048 || self.alt.len() > 1024 { return false; }
        if image_url_host(&self.src).is_some() { return true; }
        let Some((folder, _)) = tile_id.split_once('.') else { return false; };
        if folder.is_empty() || !folder.bytes().all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)) { return false; }
        let prefix = alloc::format!("/apps/{folder}/");
        self.src.strip_prefix(&prefix).is_some_and(|path| {
            !path.is_empty() && path.split('/').all(|p| !p.is_empty() && p != "." && p != "..")
                && !path.bytes().any(|b| b.is_ascii_control() || b == b'\\')
        })
    }
}

/// Direct HTTPS images only; no credentials, ports, IP literals, or fragments.
/// The host relay additionally validates and pins DNS to a public address.
pub fn image_url_host(url: &str) -> Option<&str> {
    if url.len() > 2048 || !url.is_ascii() || url.bytes().any(|b| b.is_ascii_control() || b.is_ascii_whitespace() || b"\\#".contains(&b)) { return None; }
    let rest = url.strip_prefix("https://")?;
    let host = rest.split(['/', '?']).next()?;
    if host.len() > 253 || !host.contains('.') || host.bytes().all(|b| b.is_ascii_digit() || b == b'.') { return None; }
    if !host.split('.').all(|label| !label.is_empty() && label.len() <= 63 && !label.starts_with('-') && !label.ends_with('-') && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')) { return None; }
    Some(host)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageAsset {
    pub src: String,
    /// Hex-encoded original PNG/JPEG bytes. Empty means unavailable; don't retry each frame.
    pub data: String,
}
pub fn encode(bytes: &[u8]) -> String {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes { out.push(HEX[(b >> 4) as usize] as char); out.push(HEX[(b & 15) as usize] as char); }
    out
}
pub fn decode(data: &str) -> Option<Vec<u8>> {
    if data.len() > MAX_IMAGE_BYTES * 2 || data.len() % 2 != 0 { return None; }
    fn nibble(b: u8) -> Option<u8> { match b { b'0'..=b'9' => Some(b-b'0'), b'a'..=b'f' => Some(b-b'a'+10), _ => None } }
    data.as_bytes().chunks_exact(2).map(|p| Some(nibble(p[0])? * 16 + nibble(p[1])?)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sources_are_public_https_or_owned_bundle_paths() {
        for src in ["https://images.example.com/photo.jpg?size=small", "/apps/photos/art/cover.png"] {
            assert!(TileImage { src: src.into(), alt: String::new() }.valid_for("photos.album"));
        }
        for src in ["http://example.com/x", "https://127.0.0.1/x", "https://user@example.com/x", "https://example.com:443/x", "/apps/other/x.png", "/apps/photos/../secret", "/apps/photos//x", "/etc/secret", "https://example.com/\nx"] {
            assert!(!TileImage { src: src.into(), alt: String::new() }.valid_for("photos.album"), "{src}");
        }
        assert_eq!(decode(&encode(&[0, 128, 255])), Some(alloc::vec![0, 128, 255]));
        assert!(decode("ffz0").is_none());
    }
}
