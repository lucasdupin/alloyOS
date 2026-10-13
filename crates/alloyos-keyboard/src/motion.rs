use alloc::{vec, vec::Vec};
use core::time::Duration;
use slint::PhysicalSize;

/// Separable displacement map in scanout pixels. Slint's retained RGB buffer
/// stays pristine: every frame samples it, never the previously warped output.
/// The panel drags a tapered band of the workspace; viscosity trails velocity
/// and relaxes after the slide, including when scrolling changes direction.
pub struct LiquidDisplacement {
    rows: Vec<f32>,
    columns: Vec<f32>,
    width: u32,
    height: u32,
    scale: f32,
    pull: f32,
    reveal: f32,
    time: Duration,
    band: Option<(u32, u32)>,
}

impl LiquidDisplacement {
    pub fn new(width: u32, height: u32, scale: f32) -> Self {
        assert!(width > 0 && height > 0 && scale.is_finite() && scale > 0.0);
        let columns = (0..width)
            .map(|x| {
                let u = 2.0 * x as f32 / width.saturating_sub(1).max(1) as f32 - 1.0;
                (1.0 - u * u).max(0.0)
            })
            .collect();
        Self {
            rows: vec![0.0; height as usize],
            columns,
            width,
            height,
            scale,
            pull: 0.0,
            reveal: 0.0,
            time: Duration::ZERO,
            band: None,
        }
    }

    pub fn advance(&mut self, time: Duration, reveal: f32, suspended: bool) -> Option<(u32, u32)> {
        let time = time.max(self.time);
        let old_band = self.band;
        let dt = time.saturating_sub(self.time).as_secs_f32();
        if dt > 0.0 {
            let velocity = (reveal - self.reveal) / dt;
            let target = (-velocity * 0.045).clamp(-22.0, 22.0) * self.scale;
            self.pull += (target - self.pull) * (dt / (0.085 + dt));
        }
        if suspended || self.pull.abs() < 0.08 * self.scale {
            self.pull = 0.0;
        }
        self.time = time;
        self.reveal = reveal;
        let edge = self.height as f32 - reveal * self.scale;
        let reach = 200.0 * self.scale;
        self.band = if self.pull != 0.0 {
            Some((
                (edge - reach).max(0.0) as u32,
                ((edge as u32).saturating_add(1)).min(self.height),
            ))
        } else {
            None
        };
        for (y, row) in self.rows.iter_mut().enumerate() {
            let t = ((edge - y as f32) / reach).clamp(0.0, 1.0);
            // Smooth bell: zero value and slope at both band boundaries.
            *row = self.pull * 16.0 * t * t * (1.0 - t) * (1.0 - t);
        }
        match (old_band, self.band) {
            (Some(a), Some(b)) => Some((a.0.min(b.0), a.1.max(b.1))),
            (a, b) => a.or(b),
        }
    }

    /// Sample the pristine scene through the current map; the keyboard itself
    /// is outside the field and returns an unchanged pixel.
    pub fn pixel(&self, rgb: &[slint::Rgb8Pixel], x: u32, y: u32) -> slint::Rgb8Pixel {
        assert_eq!(rgb.len(), self.width as usize * self.height as usize);
        if self.rows[y as usize] == 0.0 {
            rgb[(y * self.width + x) as usize]
        } else {
            let (sx, sy) = self.source(x as f32, y as f32);
            sample_rgb(rgb, PhysicalSize::new(self.width, self.height), sx, sy)
        }
    }

    pub fn source(&self, x: f32, y: f32) -> (f32, f32) {
        let x = x.clamp(0.0, self.width.saturating_sub(1) as f32);
        let y = y.clamp(0.0, self.height.saturating_sub(1) as f32);
        let drag = self.rows[y as usize];
        let weight = self.columns[x as usize];
        let radial = (x - self.width as f32 * 0.5) / self.width as f32;
        (
            (x + radial * drag * weight * 0.22).clamp(0.0, self.width.saturating_sub(1) as f32),
            (y - drag * weight).clamp(0.0, self.height.saturating_sub(1) as f32),
        )
    }
}

/// Bilinear sampling keeps text and rounded edges smooth during subpixel pulls.
fn sample_rgb(rgb: &[slint::Rgb8Pixel], size: PhysicalSize, x: f32, y: f32) -> slint::Rgb8Pixel {
    let x0 = x as u32;
    let y0 = y as u32;
    let x1 = (x0 + 1).min(size.width - 1);
    let y1 = (y0 + 1).min(size.height - 1);
    let a = rgb[(y0 * size.width + x0) as usize];
    if x == x0 as f32 && y == y0 as f32 {
        return a;
    }
    let b = rgb[(y0 * size.width + x1) as usize];
    let c = rgb[(y1 * size.width + x0) as usize];
    let d = rgb[(y1 * size.width + x1) as usize];
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let blend = |a: u8, b: u8, c: u8, d: u8| {
        let top = a as f32 + (b as f32 - a as f32) * fx;
        let bottom = c as f32 + (d as f32 - c as f32) * fx;
        (top + (bottom - top) * fy + 0.5) as u8
    };
    slint::Rgb8Pixel {
        r: blend(a.r, b.r, c.r, d.r),
        g: blend(a.g, b.g, c.g, d.g),
        b: blend(a.b, b.b, c.b, d.b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn displacement_tracks_motion_and_restores_pristine_pixels() {
        let mut map = LiquidDisplacement::new(402, 874, 1.0);
        assert_eq!(map.source(201.0, 500.0), (201.0, 500.0));
        let band = map
            .advance(Duration::from_millis(16), 100.0, false)
            .unwrap();
        assert_eq!(band, (574, 775));
        // Rising keyboard pulls the workspace upwards: inverse sample looks down.
        assert!(map.source(201.0, 674.0).1 > 674.0);
        assert_eq!(map.source(0.0, 674.0), (0.0, 674.0));
        assert_eq!(
            map.source(201.0, 800.0),
            (201.0, 800.0),
            "keys remain unwarped"
        );
        assert_eq!(
            map.source(201.0, 300.0),
            (201.0, 300.0),
            "header stays anchored"
        );
        map.advance(Duration::from_millis(116), 30.0, false);
        assert!(
            map.source(201.0, 744.0).1 < 744.0,
            "downward slide reverses pull"
        );
        let mut restored = false;
        for frame in 1..=120 {
            let damage = map.advance(Duration::from_millis(116 + frame * 16), 30.0, false);
            if map.pull == 0.0 && damage.is_some() {
                restored = true;
            }
        }
        assert!(
            restored,
            "last warped band is repainted when liquid settles"
        );
        assert!(map
            .advance(Duration::from_millis(2200), 30.0, false)
            .is_none());
        assert_eq!(map.source(201.0, 744.0), (201.0, 744.0));
        let rgb = [
            slint::Rgb8Pixel { r: 0, g: 0, b: 0 },
            slint::Rgb8Pixel { r: 100, g: 0, b: 0 },
            slint::Rgb8Pixel { r: 0, g: 100, b: 0 },
            slint::Rgb8Pixel {
                r: 100,
                g: 100,
                b: 0,
            },
        ];
        let pixel = sample_rgb(&rgb, PhysicalSize::new(2, 2), 0.5, 0.5);
        assert_eq!((pixel.r, pixel.g, pixel.b), (50, 50, 0));
    }
}
