/// Crisp monochrome & liquid-mercury visual palette for `ferrofluidd` (ARGB8888).

/// Pure obsidian background (`#000000`).
pub const BG_OBSIDIAN: u32 = 0xFF00_0000;

/// Carbon surface fill for cards, code blocks, and input bars (`#0A0A0C`).
pub const SURFACE_CARBON: u32 = 0xFF0A_0A0C;

/// High-contrast metallic chrome border (`#D8DCE4`).
pub const BORDER_CHROME: u32 = 0xFFD8_DCE4;

/// Dimmed silver border for inner dividers, cells, and subtle framing (`#6E7681`).
pub const BORDER_SILVER_DIM: u32 = 0xFF6E_7681;

/// Bright mercury white-silver for primary text and active highlights (`#F0F4F8`).
pub const TEXT_MERCURY_BRIGHT: u32 = 0xFFF0_F4F8;

/// Liquid silver accent for badges, sparklines, and anti-aliased glyph edges (`#A8B5C4`).
pub const ACCENT_LIQUID_SILVER: u32 = 0xFFA8_B5C4;

/// Alpha-blends `fg` over `bg` using an 8-bit `alpha` (`0 = bg`, `255 = fg`).
/// Returns an opaque `0xFFRRGGBB` pixel.
#[inline]
pub const fn blend_argb(bg: u32, fg: u32, alpha: u8) -> u32 {
    let a = alpha as u32;
    let inv_a = 255 - a;

    let bg_r = (bg >> 16) & 0xFF;
    let bg_g = (bg >> 8) & 0xFF;
    let bg_b = bg & 0xFF;

    let fg_r = (fg >> 16) & 0xFF;
    let fg_g = (fg >> 8) & 0xFF;
    let fg_b = fg & 0xFF;

    let r = (fg_r * a + bg_r * inv_a) / 255;
    let g = (fg_g * a + bg_g * inv_a) / 255;
    let b = (fg_b * a + bg_b * inv_a) / 255;

    0xFF00_0000 | (r << 16) | (g << 8) | b
}

/// Computes a liquid-mercury specular border color along a normalized perimeter
/// position `pos / total` so the boot drip border transitions smoothly into the
/// single-window AI console frame.
#[inline]
pub const fn liquid_mercury_border_color(pos: u32, total: u32) -> u32 {
    if total == 0 {
        return BORDER_CHROME;
    }
    let half = total / 2;
    let dist = if pos <= half { pos } else { total - pos };
    let alpha = if half == 0 {
        255
    } else {
        let v = (dist * 255) / half;
        if v > 255 {
            255
        } else {
            v as u8
        }
    };
    blend_argb(BORDER_SILVER_DIM, BORDER_CHROME, alpha)
}
