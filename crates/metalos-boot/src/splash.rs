//! Phase 3: Boot Splash & Liquid Metal Drip Animation (`crates/metalos-boot/src/splash.rs`).
//!
//! Implements a `#![no_std]` fixed-point 2D metaball and surface-tension scalar field
//! engine recreating the SVG physics of `assets/logo.svg`:
//! - `#crystal-weld` filter (`feGaussianBlur stdDeviation="6"` + `feColorMatrix 28*a - 11`)
//! - `#mercury-type` filter (`feGaussianBlur stdDeviation="4.8"` + `feColorMatrix 30*a - 12`)
//! - **Ferrofluid Crystal Emblem** (octagonal core + welded diamond cones + razor-sharp tips)
//! - **metalOS** viscous meniscus wordmark (glyph strokes + inter-letter meniscus bridges)
//! - **Viscous Drip Physics Engine** (`LiquidSplashAnimation`) animating molten mercury
//!   droplets detaching under gravity and surface tension before coalescing into the
//!   initial **Ferrofluid (`ferrofluidd`)** terminal window border rectangle.

use alloc::vec::Vec;
use metalos_proto::DirtyRect;

/// Pure obsidian background pixel in ARGB8888 (`#000000`).
pub const OBSIDIAN_BLACK_ARGB: u32 = 0xFF00_0000;

/// Specular chrome highlight pixel in ARGB8888 (`#F5F7FA`).
pub const CHROME_HIGHLIGHT_ARGB: u32 = 0xFFF5_F7FA;

/// Liquid mercury midtone pixel in ARGB8888 (`#B8C2CC`).
pub const MERCURY_MIDTONE_ARGB: u32 = 0xFFB8_C2CC;

/// Deep metallic shadow pixel in ARGB8888 (`#1A2028`).
pub const OBSIDIAN_METALLIC_ARGB: u32 = 0xFF1A_2028;

/// Fixed-point shift (Q16.16 format: 16 fractional bits).
pub const Q16_SHIFT: u32 = 16;

/// One pixel in Q16.16 fixed-point representation.
pub const Q16_ONE: i32 = 1 << Q16_SHIFT;

/// 2D Metaball particle in Q16.16 fixed-point coordinates for `#![no_std]` physics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Metaball {
    /// Horizontal center in Q16.16 pixels.
    pub x_q16: i32,
    /// Vertical center in Q16.16 pixels.
    pub y_q16: i32,
    /// Horizontal velocity in Q16.16 pixels per second.
    pub vx_q16: i32,
    /// Vertical velocity in Q16.16 pixels per second.
    pub vy_q16: i32,
    /// Characteristic radius in Q16.16 pixels.
    pub radius_q16: i32,
}

impl Metaball {
    /// Creates a stationary metaball at integer pixel coordinates `(x_px, y_px)` with `radius_px`.
    #[inline]
    pub const fn new(x_px: i32, y_px: i32, radius_px: i32) -> Self {
        Self {
            x_q16: x_px << Q16_SHIFT,
            y_q16: y_px << Q16_SHIFT,
            vx_q16: 0,
            vy_q16: 0,
            radius_q16: radius_px << Q16_SHIFT,
        }
    }

    /// Creates a metaball with explicit initial velocity in pixels/sec.
    #[inline]
    pub const fn with_velocity(
        x_px: i32,
        y_px: i32,
        vx_px_s: i32,
        vy_px_s: i32,
        radius_px: i32,
    ) -> Self {
        Self {
            x_q16: x_px << Q16_SHIFT,
            y_q16: y_px << Q16_SHIFT,
            vx_q16: vx_px_s << Q16_SHIFT,
            vy_q16: vy_px_s << Q16_SHIFT,
            radius_q16: radius_px << Q16_SHIFT,
        }
    }

    /// Integer pixel X coordinate (rounded down).
    #[inline]
    pub const fn x_px(&self) -> i32 {
        self.x_q16 >> Q16_SHIFT
    }

    /// Integer pixel Y coordinate (rounded down).
    #[inline]
    pub const fn y_px(&self) -> i32 {
        self.y_q16 >> Q16_SHIFT
    }

    /// Integer pixel radius (rounded down, minimum 1).
    #[inline]
    pub const fn radius_px(&self) -> i32 {
        let r = self.radius_q16 >> Q16_SHIFT;
        if r < 1 {
            1
        } else {
            r
        }
    }

    /// Evaluates the smooth Gaussian-like surface-tension scalar field contribution
    /// and spatial gradient `(field_u8, grad_x, grad_y)` at pixel `(px, py)`.
    #[inline]
    pub fn field_contribution(&self, px: i32, py: i32) -> (u32, i32, i32) {
        let cx = self.x_px();
        let cy = self.y_px();
        let r = self.radius_px().max(1);
        let dx = px - cx;
        let dy = py - cy;

        // Cutoff at 2*r for compact support matching feGaussianBlur kernel radius.
        let max_r = r * 2;
        if dx.abs() >= max_r || dy.abs() >= max_r {
            return (0, 0, 0);
        }

        let dist_sq = dx * dx + dy * dy;
        let max_r_sq = max_r * max_r;
        if dist_sq >= max_r_sq {
            return (0, 0, 0);
        }

        // Smooth quartic falloff (1 - d^2 / (2r)^2)^2 scaled to [0..=255]
        let rem = max_r_sq - dist_sq;
        let num = (rem as i64) * (rem as i64) * 255;
        let den = (max_r_sq as i64) * (max_r_sq as i64);
        let val = (num / den.max(1)) as u32;

        // Spatial gradient pointing outward/inward for specular normal calculation
        let grad_scale = (rem * 128) / max_r_sq.max(1);
        let gx = (dx * grad_scale) / r;
        let gy = (dy * grad_scale) / r;
        (val, gx, gy)
    }
}

/// Recreates the `#crystal-weld` SVG filter alpha thresholding (`28 * a - 11`, clamped to `[0, 255]`).
///
/// In `assets/logo.svg`, `<feColorMatrix>` transforms normalized alpha `a in [0.0, 1.0]` via:
/// `a_out = clamp(28 * a - 11, 0.0, 1.0)`.
/// In 8-bit alpha `[0..=255]` space (`a = alpha_u8 / 255`), the threshold lies at
/// `11 / 28 * 255 ≈ 100.18`, steeply ramping from `0` at `alpha_u8 <= 100` to `255` at `alpha_u8 >= 110`.
#[inline]
pub const fn crystal_weld_alpha(alpha_u8: u8) -> u8 {
    let scaled = 28 * (alpha_u8 as i32) - 11 * 255;
    if scaled <= 0 {
        0
    } else if scaled >= 255 {
        255
    } else {
        scaled as u8
    }
}

/// Recreates the `#mercury-type` SVG filter alpha thresholding (`30 * a - 12`, clamped to `[0, 255]`).
#[inline]
pub const fn mercury_type_alpha(alpha_u8: u8) -> u8 {
    let scaled = 30 * (alpha_u8 as i32) - 12 * 255;
    if scaled <= 0 {
        0
    } else if scaled >= 255 {
        255
    } else {
        scaled as u8
    }
}

/// Fast integer square root for `#![no_std]` surface normal normalization.
#[inline]
const fn isqrt_u32(n: u32) -> u32 {
    if n < 2 {
        return n;
    }
    let mut x = n;
    let mut y = (x + 1) >> 1;
    while y < x {
        x = y;
        y = (x + n / x) >> 1;
    }
    x
}

/// Shades a pixel using liquid-mercury specular physics on an obsidian `#000000` background.
///
/// - When `crystal_weld_alpha` evaluates to `0`, returns pure `#000000` (`0xFF00_0000`).
/// - Inside the liquid boundary, computes a 3D surface normal from `(grad_x, grad_y)` and
///   evaluates a dual-lobe studio reflection producing:
///   - Specular chrome highlights (`#F5F7FA`)
///   - Liquid silver midtones (`#B8C2CC`)
///   - Deep obsidian metallic gradients (`#1A2028` .. `#4A5568`)
#[inline]
pub fn shade_mercury_pixel(field_val: u32, grad_x: i32, grad_y: i32) -> u32 {
    let clamped_field = if field_val > 255 {
        255
    } else {
        field_val as u8
    };
    let weld_a = crystal_weld_alpha(clamped_field);
    if weld_a == 0 {
        return OBSIDIAN_BLACK_ARGB;
    }

    // Approximate 3D surface normal (nx, ny, nz) from 2D field gradient (grad_x, grad_y)
    let nz: i32 = 96;
    let len_sq = (grad_x * grad_x + grad_y * grad_y + nz * nz) as u32;
    let len = isqrt_u32(len_sq).max(1) as i32;

    let nx_q8 = (grad_x * 256) / len;
    let ny_q8 = (grad_y * 256) / len;
    let nz_q8 = (nz * 256) / len;

    // Studio key light from top-left-front: L = (-110, -140, 184) / 256
    let dot_key = ((-nx_q8 * 110 - ny_q8 * 140 + nz_q8 * 184) >> 8).clamp(0, 255);
    // Fresnel rim highlight near the meniscus edge
    let fresnel = (255 - nz_q8.clamp(0, 255)).clamp(0, 255);
    // Specular lobe (dot_key^4 approximation)
    let spec2 = (dot_key * dot_key) >> 8;
    let spec4 = (spec2 * spec2) >> 8;

    // Base metallic tone interpolates between deep obsidian (#1A2028) and mercury silver (#B8C2CC)
    // with high-intensity chrome specular peaks (#F5F7FA).
    let metallic_mix = ((dot_key * 180 + fresnel * 75) >> 8).clamp(0, 255);

    let base_r = 0x1A + ((0xB8 - 0x1A) * metallic_mix >> 8);
    let base_g = 0x20 + ((0xC2 - 0x20) * metallic_mix >> 8);
    let base_b = 0x28 + ((0xCC - 0x28) * metallic_mix >> 8);

    // Add specular chrome peak toward #F5F7FA
    let r = (base_r + ((0xF5 - base_r) * spec4 >> 8)).clamp(0, 255);
    let g = (base_g + ((0xF7 - base_g) * spec4 >> 8)).clamp(0, 255);
    let b = (base_b + ((0xFA - base_b) * spec4 >> 8)).clamp(0, 255);

    // Premultiply against #000000 background by crystal_weld_alpha for razor-clean meniscus anti-aliasing
    let a = weld_a as i32;
    let out_r = ((r * a) / 255) as u32;
    let out_g = ((g * a) / 255) as u32;
    let out_b = ((b * a) / 255) as u32;

    0xFF00_0000 | (out_r << 16) | (out_g << 8) | out_b
}

/// Minimum distance squared from point `(px, py)` to line segment `(x0, y0) -> (x1, y1)`,
/// plus the displacement vector `(dx, dy)` from the closest point on the segment.
#[inline]
fn segment_displacement(px: i32, py: i32, x0: i32, y0: i32, x1: i32, y1: i32) -> (i32, i32) {
    let vx = x1 - x0;
    let vy = y1 - y0;
    let wx = px - x0;
    let wy = py - y0;
    let len_sq = vx * vx + vy * vy;
    if len_sq <= 0 {
        return (px - x0, py - y0);
    }
    let t_num = (wx * vx + wy * vy).clamp(0, len_sq);
    let proj_x = x0 + (vx * t_num) / len_sq;
    let proj_y = y0 + (vy * t_num) / len_sq;
    (px - proj_x, py - proj_y)
}

/// Evaluates a tapered ferrofluid spike cone from `(0, 0)` to `(tx, ty)` with base radius `r_base`
/// and razor tip radius `r_tip`. Returns `(field_val, grad_x, grad_y)`.
#[inline]
fn cone_spike_field(
    lx: i32,
    ly: i32,
    tx: i32,
    ty: i32,
    r_base: i32,
    r_tip: i32,
) -> (u32, i32, i32) {
    let len_sq = tx * tx + ty * ty;
    if len_sq <= 0 {
        return (0, 0, 0);
    }
    let t_q8 = (((lx * tx + ly * ty) << 8) / len_sq).clamp(0, 256);
    let proj_x = (tx * t_q8) >> 8;
    let proj_y = (ty * t_q8) >> 8;
    let dx = lx - proj_x;
    let dy = ly - proj_y;

    let radius = (r_base * (256 - t_q8) + r_tip * t_q8) >> 8;
    let eff_r = radius.max(2);
    let dist_sq = dx * dx + dy * dy;
    let max_d_sq = eff_r * eff_r;
    if dist_sq >= max_d_sq {
        return (0, 0, 0);
    }

    let rem = max_d_sq - dist_sq;
    let val = ((rem as i64 * rem as i64 * 255) / (max_d_sq as i64 * max_d_sq as i64).max(1)) as u32;
    let gx = (dx * 96) / eff_r;
    let gy = (dy * 96) / eff_r;
    (val, gx, gy)
}

/// Evaluates the **Ferrofluid Crystal Emblem** scalar field and gradient at local offset `(lx, ly)`
/// relative to the emblem center, scaled by `scale_q8` (`256 == 1.0x` SVG units).
///
/// Combines:
/// - Central octagonal/metaball core
/// - 16 radial ferrofluid spikes matching `assets/logo.svg` (`(0, ±152)`, `(±106, 0)`,
///   `(±59, ±102)`, `(±29, ±120)`, `(±84, ±43)`) welded by the `#crystal-weld` scalar field
/// - Crisp razor spike tips
pub fn ferrofluid_emblem_field(lx: i32, ly: i32, scale_q8: i32) -> (u32, i32, i32) {
    let s = scale_q8.max(32);
    // Normalize into SVG emblem space (where spike tips reach ±152)
    let sx = (lx << 8) / s;
    let sy = (ly << 8) / s;

    if sx.abs() > 165 || sy.abs() > 165 {
        return (0, 0, 0);
    }

    // 8 bidirectional spike axes from assets/logo.svg
    const SPIKES: [(i32, i32, i32, i32); 16] = [
        (0, -152, 24, 3),
        (0, 152, 24, 3),
        (106, 0, 20, 3),
        (-106, 0, 20, 3),
        (59, -102, 18, 3),
        (-59, -102, 18, 3),
        (59, 102, 18, 3),
        (-59, 102, 18, 3),
        (29, -120, 16, 2),
        (-29, -120, 16, 2),
        (29, 120, 16, 2),
        (-29, 120, 16, 2),
        (84, -43, 16, 2),
        (-84, -43, 16, 2),
        (84, 43, 16, 2),
        (-84, 43, 16, 2),
    ];

    // Central octagonal ferrofluid core
    let core_ball = Metaball::new(0, 0, 34);
    let (mut total_field, mut gx, mut gy) = core_ball.field_contribution(sx, sy);

    for &(tx, ty, r_base, r_tip) in &SPIKES {
        let (f, sgx, sgy) = cone_spike_field(sx, sy, tx, ty, r_base, r_tip);
        if f > 0 {
            total_field = total_field.saturating_add(f);
            gx += sgx;
            gy += sgy;
        }
    }

    (total_field.min(320), gx, gy)
}

/// Evaluates the **metalOS** viscous meniscus wordmark scalar field at local coordinates `(wx, wy)`
/// in the `960x320` reference space of `assets/logo.svg` (scaled by `scale_q8`).
///
/// Includes the 7 vector-stroke glyphs (`m`, `e`, `t`, `a`, `l`, `O`, `S`) plus the 6 horizontal
/// viscous meniscus bridges (`<rect>` elements in `assets/logo.svg`) that weld adjacent glyphs together.
pub fn wordmark_meniscus_field(wx: i32, wy: i32) -> (u32, i32, i32) {
    if !(40..=650).contains(&wx) || !(95..=225).contains(&wy) {
        return (0, 0, 0);
    }

    // Stroke segments `(x0, y0, x1, y1, radius)` in SVG coordinates for "metalOS"
    // plus the 6 viscous meniscus bridges from assets/logo.svg lines 37-42.
    const STROKES: [(i32, i32, i32, i32, i32); 35] = [
        // 'm' (x: 58..162, y: 140..204)
        (64, 140, 64, 204, 11),
        (64, 146, 110, 146, 10),
        (110, 146, 110, 204, 11),
        (110, 146, 156, 146, 10),
        (156, 146, 156, 204, 11),
        // 'e' (x: 184..248, y: 140..204)
        (188, 146, 242, 146, 10),
        (188, 172, 240, 172, 9),
        (188, 198, 242, 198, 10),
        (188, 146, 188, 198, 10),
        (242, 146, 242, 172, 10),
        // 't' (x: 268..310, y: 118..204)
        (286, 118, 286, 200, 11),
        (266, 146, 308, 146, 9),
        (286, 200, 310, 200, 9),
        // 'a' (x: 328..396, y: 140..204)
        (334, 146, 390, 146, 10),
        (390, 146, 390, 204, 11),
        (334, 172, 390, 172, 9),
        (334, 198, 390, 198, 10),
        (334, 172, 334, 198, 10),
        // 'l' (x: 416..430, y: 112..204)
        (420, 112, 420, 204, 11),
        // 'O' (x: 448..524, y: 114..204)
        (454, 118, 518, 118, 11),
        (454, 198, 518, 198, 11),
        (454, 118, 454, 198, 11),
        (518, 118, 518, 198, 11),
        // 'S' (x: 548..618, y: 114..204)
        (552, 118, 612, 118, 11),
        (552, 118, 552, 158, 11),
        (552, 158, 612, 158, 11),
        (612, 158, 612, 198, 11),
        (552, 198, 612, 198, 11),
        // 6 Viscous Meniscus Bridges between neighboring letters (assets/logo.svg lines 37-42)
        (162, 157, 190, 157, 8),
        (246, 151, 274, 151, 8),
        (306, 155, 330, 155, 8),
        (396, 195, 420, 195, 8),
        (424, 157, 450, 157, 8),
        (522, 164, 552, 164, 9),
        // Bottom meniscus drip reservoir along baseline
        (160, 202, 556, 202, 5),
    ];

    let mut field = 0u32;
    let mut gx = 0i32;
    let mut gy = 0i32;

    for &(x0, y0, x1, y1, r) in &STROKES {
        let max_r = r * 2;
        let min_x = x0.min(x1) - max_r;
        let max_x = x0.max(x1) + max_r;
        let min_y = y0.min(y1) - max_r;
        let max_y = y0.max(y1) + max_r;
        if wx < min_x || wx > max_x || wy < min_y || wy > max_y {
            continue;
        }
        let (dx, dy) = segment_displacement(wx, wy, x0, y0, x1, y1);
        let dist_sq = dx * dx + dy * dy;
        let max_r_sq = max_r * max_r;
        if dist_sq < max_r_sq {
            let rem = max_r_sq - dist_sq;
            let contrib = ((rem as i64 * rem as i64 * 255)
                / (max_r_sq as i64 * max_r_sq as i64).max(1)) as u32;
            field = field.saturating_add(contrib);
            gx += (dx * 96) / r.max(1);
            gy += (dy * 96) / r.max(1);
        }
    }

    (field.min(320), gx, gy)
}

/// Viscous Liquid Metal Boot Splash & Drip Animation engine (`LiquidSplashAnimation`).
///
/// Renders the **Ferrofluid Crystal Emblem** and **metalOS** meniscus wordmark on a pure
/// `#000000` background, detaches molten mercury droplets from the bottom meniscus bridges
/// and ferrofluid spikes under simulated gravity (`9.8 m/s²` scaled to screen space), and
/// coalesces them into the initial **Ferrofluid (`ferrofluidd`)** terminal window border rectangle.
#[derive(Debug, Clone)]
pub struct LiquidSplashAnimation {
    /// Viewport width in pixels.
    pub width: u32,
    /// Viewport height in pixels.
    pub height: u32,
    /// Total elapsed animation time in milliseconds.
    pub elapsed_ms: u32,
    /// Coalescence progress (`0..=100`) toward the Ferrofluid (`ferrofluidd`) window frame.
    pub coalesce_progress: u8,
    /// Active molten mercury droplets in Q16.16 fixed-point space.
    pub droplets: Vec<Metaball>,
    /// Anchor coordinates `(x_px, y_px)` where each droplet detaches from the meniscus/emblem.
    pub anchor_points: Vec<(i32, i32)>,
    /// Target rectangle for the initial **Ferrofluid (`ferrofluidd`)** terminal window border.
    pub target_window_rect: DirtyRect,
}

impl LiquidSplashAnimation {
    /// Total animation duration in milliseconds for full coalescence (`coalesce_progress == 100`).
    pub const TOTAL_DURATION_MS: u32 = 1200;

    /// Gravitational acceleration in pixels/sec² for molten mercury droplets.
    pub const GRAVITY_PX_S2: i32 = 520;

    /// Creates a new `LiquidSplashAnimation` for a viewport of `(width, height)`.
    pub fn new(width: u32, height: u32) -> Self {
        let w = width.max(64);
        let h = height.max(64);

        // Initial Ferrofluid (ferrofluidd) terminal window border centered in the lower-middle viewport
        let win_w = ((w * 3) / 4).max(32);
        let win_h = ((h * 3) / 5).max(24);
        let win_x = (w - win_w) / 2;
        let win_y = (h - win_h) / 2 + (h / 10);
        let target_window_rect = DirtyRect::new(win_x, win_y, win_w, win_h);

        // Compute logo placement in viewport coordinates
        let logo_top_y = (h as i32) / 5;
        let baseline_y = logo_top_y + ((h as i32) / 7).max(16);
        let left_x = (w as i32) / 6;
        let span_x = ((w as i32) * 2 / 3).max(32);

        // 6 meniscus bridge drip anchors + 2 ferrofluid crystal spike drip anchors
        let relative_offsets: [i32; 8] = [10, 22, 34, 48, 60, 72, 84, 92];
        let mut droplets = Vec::with_capacity(relative_offsets.len());
        let mut anchor_points = Vec::with_capacity(relative_offsets.len());

        let base_radius = ((w.min(h) as i32) / 48).clamp(3, 18);

        for (i, &pct) in relative_offsets.iter().enumerate() {
            let ax = left_x + (span_x * pct) / 100;
            let ay = baseline_y + ((i as i32 & 1) * 3);
            let r = base_radius + ((i as i32 % 3) - 1).max(0);
            anchor_points.push((ax, ay));
            droplets.push(Metaball::new(ax, ay, r));
        }

        Self {
            width: w,
            height: h,
            elapsed_ms: 0,
            coalesce_progress: 0,
            droplets,
            anchor_points,
            target_window_rect,
        }
    }

    /// Advances the viscous drip physics and window border coalescence by `dt_ms` milliseconds.
    pub fn step_frame(&mut self, dt_ms: u32) {
        if dt_ms == 0 {
            return;
        }
        self.elapsed_ms = self.elapsed_ms.saturating_add(dt_ms);

        // Update coalescence progress (0..=100)
        // Phase A (0..400ms): Meniscus necking & gravity drip detachment
        // Phase B (400..1200ms): Free-fall acceleration & coalescence into target_window_rect
        let raw_progress = if self.elapsed_ms <= 240 {
            (self.elapsed_ms * 15) / 240
        } else {
            15 + ((self.elapsed_ms - 240) * 85) / (Self::TOTAL_DURATION_MS - 240)
        };
        self.coalesce_progress = raw_progress.min(100) as u8;

        let dt_i64 = dt_ms as i64;
        // Gravity impulse in Q16.16 pixels/sec: dv_y = g * dt_ms / 1000
        let dv_y_q16 = (((Self::GRAVITY_PX_S2 as i64) << Q16_SHIFT) * dt_i64 / 1000) as i32;

        let rect = self.target_window_rect;
        let target_top_y = rect.y as i32;
        let target_bottom_y = (rect.y + rect.height.saturating_sub(1)) as i32;
        let target_left_x = rect.x as i32;
        let target_right_x = (rect.x + rect.width.saturating_sub(1)) as i32;

        let num_drops = self.droplets.len().max(1) as i32;

        for (idx, drop) in self.droplets.iter_mut().enumerate() {
            // Apply downward gravity with slight staggered meniscus release viscosity
            let viscosity_factor = if self.elapsed_ms < 180 { 1 } else { 2 };
            drop.vy_q16 = drop
                .vy_q16
                .saturating_add((dv_y_q16 * viscosity_factor) / 2);

            // Integrate position: dx = vx * dt / 1000, dy = vy * dt / 1000
            let dx_q16 = ((drop.vx_q16 as i64 * dt_i64) / 1000) as i32;
            let dy_q16 = ((drop.vy_q16 as i64 * dt_i64) / 1000) as i32;
            drop.x_q16 = drop.x_q16.saturating_add(dx_q16);
            drop.y_q16 = drop.y_q16.saturating_add(dy_q16);

            // Target point along the Ferrofluid terminal window border perimeter
            let perimeter_x =
                target_left_x + ((target_right_x - target_left_x) * (idx as i32)) / (num_drops - 1).max(1);
            let perimeter_y = if idx % 2 == 0 {
                target_top_y
            } else {
                target_bottom_y
            };

            // As coalescence progresses past 50%, surface tension pulls droplets onto the window border
            if self.coalesce_progress >= 50 {
                let pull = (self.coalesce_progress as i32 - 50) * 2; // 0..=100
                let target_x_q16 = perimeter_x << Q16_SHIFT;
                let target_y_q16 = perimeter_y << Q16_SHIFT;
                drop.x_q16 += ((target_x_q16 - drop.x_q16) as i64 * pull as i64 / 100) as i32;
                // Ensure y_q16 continues moving downward toward at least target_top_y
                let clamped_target_y_q16 = target_y_q16.max(drop.y_q16);
                drop.y_q16 +=
                    ((clamped_target_y_q16 - drop.y_q16) as i64 * pull as i64 / 100) as i32;
            }
        }
    }

    /// Clears a scanline slice to pure `#000000` (`0xFF00_0000`) using an ARM64 NEON SIMD fast path
    /// when compiled on `aarch64`, or a portable scalar loop on other architectures.
    #[inline]
    fn clear_scanline_obsidian(scanline: &mut [u32]) {
        #[cfg(target_arch = "aarch64")]
        {
            // SAFETY: `vdupq_n_u32` and `vst1q_u32` are unconditionally available on AArch64 NEON,
            // and `chunks_exact_mut(4)` guarantees 4-element capacity per store.
            unsafe {
                use core::arch::aarch64::{vdupq_n_u32, vst1q_u32};
                let black_vec = vdupq_n_u32(OBSIDIAN_BLACK_ARGB);
                let mut chunks = scanline.chunks_exact_mut(4);
                for chunk in &mut chunks {
                    vst1q_u32(chunk.as_mut_ptr(), black_vec);
                }
                for rem in chunks.into_remainder() {
                    *rem = OBSIDIAN_BLACK_ARGB;
                }
            }
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            for px in scanline.iter_mut() {
                *px = OBSIDIAN_BLACK_ARGB;
            }
        }
    }

    /// Evaluates the coalesced **Ferrofluid (`ferrofluidd`)** terminal window border field at `(px, py)`.
    #[inline]
    fn window_border_field(&self, px: i32, py: i32) -> (u32, i32, i32) {
        if self.coalesce_progress == 0 {
            return (0, 0, 0);
        }
        let r = self.target_window_rect;
        let x0 = r.x as i32;
        let y0 = r.y as i32;
        let x1 = (r.x + r.width.saturating_sub(1)) as i32;
        let y1 = (r.y + r.height.saturating_sub(1)) as i32;

        let border_half_w = 4;
        if px < x0 - border_half_w
            || px > x1 + border_half_w
            || py < y0 - border_half_w
            || py > y1 + border_half_w
        {
            return (0, 0, 0);
        }

        // Distance to nearest of the 4 window border edges
        let dx_left = (px - x0).abs();
        let dx_right = (px - x1).abs();
        let dy_top = (py - y0).abs();
        let dy_bottom = (py - y1).abs();

        let on_horiz_span = px >= x0 - border_half_w && px <= x1 + border_half_w;
        let on_vert_span = py >= y0 - border_half_w && py <= y1 + border_half_w;

        let mut min_dist = i32::MAX;
        let mut gx = 0;
        let mut gy = 0;

        if on_horiz_span && dy_top < min_dist {
            min_dist = dy_top;
            gx = 0;
            gy = (py - y0) * 32;
        }
        if on_horiz_span && dy_bottom < min_dist {
            min_dist = dy_bottom;
            gx = 0;
            gy = (py - y1) * 32;
        }
        if on_vert_span && dx_left < min_dist {
            min_dist = dx_left;
            gx = (px - x0) * 32;
            gy = 0;
        }
        if on_vert_span && dx_right < min_dist {
            min_dist = dx_right;
            gx = (px - x1) * 32;
            gy = 0;
        }

        if min_dist > border_half_w {
            return (0, 0, 0);
        }

        let base_val = ((border_half_w + 1 - min_dist) * 220) / (border_half_w + 1);
        let scaled_val = ((base_val as u32) * (self.coalesce_progress as u32)) / 100;
        (scaled_val, gx, gy)
    }

    /// Renders the current liquid-metal boot splash and drip animation frame into `fb`
    /// (ARGB8888 linear framebuffer with row pitch `stride_pixels`) and returns the
    /// bounding `DirtyRect`.
    pub fn render_frame(&self, fb: &mut [u32], stride_pixels: usize) -> DirtyRect {
        let w = self.width as usize;
        let h = self.height as usize;
        let stride = stride_pixels.max(w);

        // Clear visible scanlines to pure #000000 (0xFF00_0000)
        for y in 0..h {
            let row_start = y * stride;
            if row_start >= fb.len() {
                break;
            }
            let row_end = (row_start + w).min(fb.len());
            Self::clear_scanline_obsidian(&mut fb[row_start..row_end]);
        }

        // Compute coordinate mapping from viewport space to the 960x320 SVG logo reference frame
        let logo_w = ((self.width as i32) * 3 / 4).max(48);
        let logo_h = (logo_w / 3).max(20);
        let logo_x0 = (self.width as i32 - logo_w) / 2;
        let logo_y0 = (self.height as i32) / 8;
        let emblem_scale_q8 = ((logo_w << 8) / 960).max(24);
        let emblem_cx = logo_x0 + (logo_w * 775) / 960;
        let emblem_cy = logo_y0 + (logo_h * 160) / 320;

        let mut min_dirty_x = self.width;
        let mut min_dirty_y = self.height;
        let mut max_dirty_x = 0u32;
        let mut max_dirty_y = 0u32;

        for y in 0..h {
            let row_start = y * stride;
            if row_start >= fb.len() {
                break;
            }
            let py = y as i32;

            for x in 0..w {
                let idx = row_start + x;
                if idx >= fb.len() {
                    break;
                }
                let px = x as i32;

                let mut field = 0u32;
                let mut gx = 0i32;
                let mut gy = 0i32;

                // 1. Ferrofluid Crystal Emblem contribution
                let (ef, egx, egy) =
                    ferrofluid_emblem_field(px - emblem_cx, py - emblem_cy, emblem_scale_q8);
                if ef > 0 {
                    field = field.saturating_add(ef);
                    gx += egx;
                    gy += egy;
                }

                // 2. metalOS Viscous Meniscus Wordmark contribution
                if px >= logo_x0
                    && px <= logo_x0 + logo_w
                    && py >= logo_y0
                    && py <= logo_y0 + logo_h
                {
                    let svg_x = ((px - logo_x0) * 960) / logo_w.max(1);
                    let svg_y = ((py - logo_y0) * 320) / logo_h.max(1);
                    let (wf, wgx, wgy) = wordmark_meniscus_field(svg_x, svg_y);
                    if wf > 0 {
                        field = field.saturating_add(wf);
                        gx += wgx;
                        gy += wgy;
                    }
                }

                // 3. Falling & stretching molten mercury droplets + meniscus neck bridges
                for (d_idx, drop) in self.droplets.iter().enumerate() {
                    let (df, dgx, dgy) = drop.field_contribution(px, py);
                    if df > 0 {
                        field = field.saturating_add(df);
                        gx += dgx;
                        gy += dgy;
                    }

                    // Viscous meniscus stretching neck between anchor and falling droplet
                    if self.coalesce_progress < 75 {
                        if let Some(&(ax, ay)) = self.anchor_points.get(d_idx) {
                            let dx_drop = drop.x_px();
                            let dy_drop = drop.y_px();
                            if py >= ay - 4
                                && py <= dy_drop + 4
                                && (px - ax).abs() <= drop.radius_px() * 2
                            {
                                let (sdx, sdy) =
                                    segment_displacement(px, py, ax, ay, dx_drop, dy_drop);
                                let neck_r = (drop.radius_px() / 2).max(2);
                                let d2 = sdx * sdx + sdy * sdy;
                                let max_d2 = neck_r * neck_r * 2;
                                if d2 < max_d2 {
                                    let neck_f = (((max_d2 - d2) * 180) / max_d2.max(1)) as u32;
                                    let stretch_atten =
                                        (75 - self.coalesce_progress as u32) * neck_f / 75;
                                    field = field.saturating_add(stretch_atten);
                                    gx += sdx * 16;
                                    gy += sdy * 16;
                                }
                            }
                        }
                    }
                }

                // 4. Coalescing Ferrofluid (`ferrofluidd`) terminal window border contribution
                let (bf, bgx, bgy) = self.window_border_field(px, py);
                if bf > 0 {
                    field = field.saturating_add(bf);
                    gx += bgx;
                    gy += bgy;
                }

                if field > 0 {
                    let argb = shade_mercury_pixel(field, gx, gy);
                    fb[idx] = argb;
                    if argb != OBSIDIAN_BLACK_ARGB {
                        let ux = x as u32;
                        let uy = y as u32;
                        if ux < min_dirty_x {
                            min_dirty_x = ux;
                        }
                        if uy < min_dirty_y {
                            min_dirty_y = uy;
                        }
                        if ux > max_dirty_x {
                            max_dirty_x = ux;
                        }
                        if uy > max_dirty_y {
                            max_dirty_y = uy;
                        }
                    }
                }
            }
        }

        if min_dirty_x <= max_dirty_x && min_dirty_y <= max_dirty_y {
            DirtyRect::new(
                min_dirty_x,
                min_dirty_y,
                max_dirty_x - min_dirty_x + 1,
                max_dirty_y - min_dirty_y + 1,
            )
        } else {
            DirtyRect::new(0, 0, self.width, self.height)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crystal_weld_alpha_thresholding_matches_svg_filter() {
        // Below 11/28 * 255 (~100), alpha must be clamped to 0
        assert_eq!(crystal_weld_alpha(0), 0);
        assert_eq!(crystal_weld_alpha(50), 0);
        assert_eq!(crystal_weld_alpha(100), 0);

        // Steep surface-tension ramp between 101 and 109
        let mid_low = crystal_weld_alpha(103);
        let mid_high = crystal_weld_alpha(107);
        assert!(mid_low > 0 && mid_low < 255);
        assert!(mid_high > mid_low && mid_high < 255);

        // Above 12/28 * 255 (~110), alpha must saturate at 255
        assert_eq!(crystal_weld_alpha(110), 255);
        assert_eq!(crystal_weld_alpha(200), 255);
        assert_eq!(crystal_weld_alpha(255), 255);
    }

    #[test]
    fn mercury_type_alpha_thresholding_matches_svg_filter() {
        // 30*a - 12 has cutoff at 12/30 * 255 = 102
        assert_eq!(mercury_type_alpha(0), 0);
        assert_eq!(mercury_type_alpha(102), 0);
        assert!(mercury_type_alpha(106) > 0 && mercury_type_alpha(106) < 255);
        assert_eq!(mercury_type_alpha(111), 255);
        assert_eq!(mercury_type_alpha(255), 255);
    }

    #[test]
    fn droplet_detachment_and_gravity_trajectory() {
        let mut anim = LiquidSplashAnimation::new(320, 200);
        let initial_y: Vec<i32> = anim.droplets.iter().map(|d| d.y_q16).collect();

        anim.step_frame(100);
        let after_100ms_y: Vec<i32> = anim.droplets.iter().map(|d| d.y_q16).collect();
        for (y0, y1) in initial_y.iter().zip(after_100ms_y.iter()) {
            assert!(
                y1 > y0,
                "droplet y_q16 must increase downward under gravity ({y1} <= {y0})"
            );
        }

        anim.step_frame(200);
        for (y1, d) in after_100ms_y.iter().zip(anim.droplets.iter()) {
            assert!(
                d.y_q16 > *y1,
                "droplet y_q16 must continue accelerating downward ({} <= {})",
                d.y_q16,
                y1
            );
        }
    }

    #[test]
    fn coalescence_reaches_ferrofluid_window_border_at_completion() {
        let mut anim = LiquidSplashAnimation::new(240, 160);
        for _ in 0..12 {
            anim.step_frame(100);
        }
        assert_eq!(anim.coalesce_progress, 100);

        let mut fb = alloc::vec![0u32; 240 * 160];
        let dirty = anim.render_frame(&mut fb, 240);
        assert!(dirty.width > 0 && dirty.height > 0);

        // Verify that the top border of target_window_rect has lit metallic pixels
        let rect = anim.target_window_rect;
        let mid_x = (rect.x + rect.width / 2) as usize;
        let top_y = rect.y as usize;
        let border_px = fb[top_y * 240 + mid_x];
        assert_ne!(
            border_px, OBSIDIAN_BLACK_ARGB,
            "Coalesced Ferrofluid terminal window border must be rendered with metallic chrome pixels"
        );
    }

    #[test]
    fn render_frame_produces_chrome_pixels_on_obsidian_black_background() {
        let anim = LiquidSplashAnimation::new(160, 120);
        let mut fb = alloc::vec![0xDEAD_BEEFu32; 160 * 120];
        let dirty = anim.render_frame(&mut fb, 160);

        assert!(dirty.width > 0 && dirty.height > 0);
        // Corner pixel (0, 0) must be cleared to pure #000000 (0xFF00_0000)
        assert_eq!(fb[0], OBSIDIAN_BLACK_ARGB);

        // Count non-black liquid mercury chrome pixels rendered by the emblem, wordmark, and droplets
        let chrome_count = fb
            .iter()
            .filter(|&&px| px != OBSIDIAN_BLACK_ARGB && (px >> 24) == 0xFF)
            .count();
        assert!(
            chrome_count > 50,
            "Expected significant liquid mercury pixels rendered, got {chrome_count}"
        );
    }
}
