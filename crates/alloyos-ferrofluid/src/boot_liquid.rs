//! Slow, layered turbulence sampled from the pristine Slint boot logotype.
use alloc::{vec, vec::Vec};
use core::time::Duration;
use alloyos_proto::DirtyRect;
use slint::{PhysicalSize, Rgb8Pixel};

pub struct BootLiquid {
    size: PhysicalSize,
    scale: f32,
    rows: Vec<f32>,
    columns: Vec<f32>,
    time: f32,
    drops: [(f32, f32, f32); 4],
}

impl BootLiquid {
    pub fn new(size: PhysicalSize, scale: f32) -> Self {
        Self {
            size,
            scale,
            rows: vec![0.0; size.height as usize],
            columns: vec![0.0; size.width as usize],
            time: 0.0,
            drops: [(0.0, 0.0, 0.0); 4],
        }
    }

    pub fn bounds(&self) -> DirtyRect {
        let half_width = (90.0 * self.scale) as u32;
        let half_height = (28.0 * self.scale) as u32;
        let x = (self.size.width / 2).saturating_sub(half_width);
        let y = (self.size.height / 2).saturating_sub(half_height);
        DirtyRect::new(
            x,
            y,
            (half_width * 2).min(self.size.width - x),
            (half_height + (115.0 * self.scale) as u32).min(self.size.height - y),
        )
    }

    pub fn advance(&mut self, elapsed: Duration) {
        // Reduce time before conversion to preserve sub-frame precision on long boots.
        let t = (elapsed.as_secs_f64() % 3600.0) as f32;
        self.time = t;
        let rect = self.bounds();
        for y in rect.y..rect.y + rect.height {
            let v = (y as f32 - self.size.height as f32 * 0.5) / self.scale;
            self.rows[y as usize] = self.scale
                * (0.65 * libm::sinf(v * 0.13 + t * 0.7) + 0.25 * libm::sinf(v * 0.29 - t * 0.5));
        }
        for x in rect.x..rect.x + rect.width {
            let u = (x as f32 - self.size.width as f32 * 0.5) / self.scale;
            self.columns[x as usize] = self.scale
                * (0.8 * libm::sinf(u * 0.09 - t * 0.6) + 0.3 * libm::sinf(u * 0.21 + t * 0.8));
        }
    }

    /// Anchor each drip to a real white glyph edge, not an assumed text baseline.
    pub fn prepare_drops(&mut self, rgb: &[Rgb8Pixel]) {
        let rect = self.bounds();
        for (index, offset) in [-42.0, -15.0, 16.0, 43.0].iter().enumerate() {
            let target = self.size.width as f32 * 0.5 + offset * self.scale;
            let mut anchor = None;
            let radius = (4.0 * self.scale) as i32;
            for dx in -radius..=radius {
                let x = (target as i32 + dx).clamp(0, self.size.width as i32 - 1) as u32;
                for y in rect.y
                    ..(self.size.height / 2 + (20.0 * self.scale) as u32).min(self.size.height)
                {
                    if rgb[(y * self.size.width + x) as usize].r > 200
                        && anchor.is_none_or(|(_, bottom)| y > bottom)
                    {
                        anchor = Some((x, y));
                    }
                }
            }
            let delay = 0.3 + index as f32 * 0.8;
            self.drops[index] =
                anchor.map_or((0.0, 0.0, delay), |(x, y)| (x as f32, y as f32, delay));
        }
    }

    fn drop_coverage(&self, x: f32, y: f32) -> f32 {
        let mut coverage: f32 = 0.0;
        for &(ax, ay, delay) in &self.drops {
            if ay == 0.0 || self.time < delay {
                continue;
            }
            let age = (self.time - delay) % 4.2;
            if age > 2.25 {
                continue;
            }
            let attached = age < 0.75;
            let growth = (age / 0.75).min(1.0);
            let falling = (age - 0.75).max(0.0);
            let cx = ax - self.rows[(ay as u32).min(self.size.height - 1) as usize];
            let root = ay - self.columns[(ax as u32).min(self.size.width - 1) as usize];
            let cy = root + self.scale * (1.0 + 5.0 * growth + 48.0 * falling * falling);
            let rx = self.scale * (0.6 + 1.5 * growth);
            let ry = rx * if attached { 1.4 } else { 1.15 };
            if (x - cx).abs() > rx + 1.0 || y < root - self.scale - 1.0 || y > cy + ry + 1.0 {
                continue;
            }
            let dx = (x - cx) / rx;
            let dy = (y - cy) / ry;
            let mut alpha = ((1.0 - libm::sqrtf(dx * dx + dy * dy)) * rx + 0.5).clamp(0.0, 1.0);
            if attached && y >= root - self.scale && y < cy {
                let neck = self.scale * (1.2 - 0.9 * growth);
                alpha = alpha.max((neck - (x - cx).abs() + 0.5).clamp(0.0, 1.0));
            }
            let fade = ((2.25 - age) / 0.4).clamp(0.0, 1.0);
            coverage = coverage.max(alpha * fade);
        }
        coverage
    }

    pub fn pixel(&self, rgb: &[Rgb8Pixel], x: u32, y: u32) -> Rgb8Pixel {
        let rect = self.bounds();
        if x < rect.x || x >= rect.x + rect.width || y < rect.y || y >= rect.y + rect.height {
            return rgb[(y * self.size.width + x) as usize];
        }
        let sx = (x as f32 + self.rows[y as usize]).clamp(0.0, (self.size.width - 1) as f32);
        let sy = (y as f32 + self.columns[x as usize]).clamp(0.0, (self.size.height - 1) as f32);
        let x0 = sx as u32;
        let y0 = sy as u32;
        let x1 = (x0 + 1).min(self.size.width - 1);
        let y1 = (y0 + 1).min(self.size.height - 1);
        let at = |x, y| rgb[(y * self.size.width + x) as usize];
        let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
        let blend = |a: u8, b: u8, c: u8, d: u8| {
            let top = a as f32 + (b as f32 - a as f32) * (sx - x0 as f32);
            let bottom = c as f32 + (d as f32 - c as f32) * (sx - x0 as f32);
            (top + (bottom - top) * (sy - y0 as f32) + 0.5) as u8
        };
        let sampled = Rgb8Pixel {
            r: blend(a.r, b.r, c.r, d.r),
            g: blend(a.g, b.g, c.g, d.g),
            b: blend(a.b, b.b, c.b, d.b),
        };
        let alpha = self.drop_coverage(x as f32, y as f32);
        let white = |channel: u8| (channel as f32 + (255.0 - channel as f32) * alpha + 0.5) as u8;
        Rgb8Pixel {
            r: white(sampled.r),
            g: white(sampled.g),
            b: white(sampled.b),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn droplets_detach_fall_and_repaint_without_trails() {
        let size = PhysicalSize::new(240, 240);
        let mut scene = vec![Rgb8Pixel { r: 0, g: 0, b: 0 }; 240 * 240];
        for y in 110..125 {
            for x in 60..180 {
                scene[y * 240 + x] = Rgb8Pixel {
                    r: 255,
                    g: 255,
                    b: 255,
                };
            }
        }
        let mut liquid = BootLiquid::new(size, 1.0);
        liquid.prepare_drops(&scene);
        let drop_rows = |liquid: &BootLiquid| {
            (130..235)
                .filter(|&y| (60..180).any(|x| liquid.pixel(&scene, x, y).r > 200))
                .collect::<Vec<_>>()
        };
        liquid.advance(Duration::ZERO);
        assert!(drop_rows(&liquid).is_empty());
        liquid.advance(Duration::from_millis(1400));
        let first = drop_rows(&liquid);
        assert!(!first.is_empty());
        liquid.advance(Duration::from_millis(1800));
        let later = drop_rows(&liquid);
        assert!(later.last() > first.last(), "detached droplet falls");
        assert_eq!(scene[150 * 240 + 78].r, 0, "source scene stays pristine");
        liquid.advance(Duration::from_millis(4100));
        assert_eq!(
            liquid.pixel(&scene, 78, 140).r,
            0,
            "old drop leaves no trail"
        );
    }
}
