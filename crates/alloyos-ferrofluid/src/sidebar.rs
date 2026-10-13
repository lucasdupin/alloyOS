//! Backdrop blur and pointer capture for the edge drawer. Rendering always
//! samples the pristine workspace, so glass never accumulates blur over time.
use alloc::{vec, vec::Vec};
use slint::{Image, PhysicalSize, Rgb8Pixel, SharedPixelBuffer};

pub(crate) struct BackdropBlur {
    size: PhysicalSize,
    step: u32,
    buffer: SharedPixelBuffer<Rgb8Pixel>,
    scratch: Vec<Rgb8Pixel>,
}

impl BackdropBlur {
    pub(crate) fn new(size: PhysicalSize, scale: f32) -> Self {
        // Two logical pixels per sample; the separable blur then operates on
        // roughly 1/36 of the scanout pixels at the current display density.
        let step = (scale * 2.0) as u32;
        let width = size.width.div_ceil(step);
        let height = size.height.div_ceil(step);
        Self {
            size,
            step,
            buffer: SharedPixelBuffer::new(width, height),
            scratch: vec![Rgb8Pixel::default(); (width * height) as usize],
        }
    }

    pub(crate) fn capture(&mut self, scene: &[Rgb8Pixel]) -> Image {
        assert_eq!(scene.len(), (self.size.width * self.size.height) as usize);
        let width = self.buffer.width() as usize;
        let height = self.buffer.height() as usize;
        let pixels = self.buffer.make_mut_slice();
        for y in 0..height {
            for x in 0..width {
                let mut sum = [0u32; 3];
                let mut count = 0;
                for sy in y as u32 * self.step..((y as u32 + 1) * self.step).min(self.size.height) {
                    for sx in
                        x as u32 * self.step..((x as u32 + 1) * self.step).min(self.size.width)
                    {
                        let p = scene[(sy * self.size.width + sx) as usize];
                        sum[0] += p.r as u32;
                        sum[1] += p.g as u32;
                        sum[2] += p.b as u32;
                        count += 1;
                    }
                }
                pixels[y * width + x] = Rgb8Pixel {
                    r: (sum[0] / count) as u8,
                    g: (sum[1] / count) as u8,
                    b: (sum[2] / count) as u8,
                };
            }
        }
        // Sliding windows keep both passes linear, independent of radius.
        blur_axis(pixels, &mut self.scratch, width, height, true);
        blur_axis(&self.scratch, pixels, width, height, false);
        Image::from_rgb8(self.buffer.clone())
    }
}

fn blur_axis(
    source: &[Rgb8Pixel],
    target: &mut [Rgb8Pixel],
    width: usize,
    height: usize,
    horizontal: bool,
) {
    let (lines, length, stride) = if horizontal {
        (height, width, 1)
    } else {
        (width, height, width)
    };
    let radius = 3;
    for line in 0..lines {
        let base = if horizontal { line * width } else { line };
        let mut sums = [0u32; 3];
        for i in 0..=radius.min(length - 1) {
            let p = source[base + i * stride];
            sums[0] += p.r as u32;
            sums[1] += p.g as u32;
            sums[2] += p.b as u32;
        }
        for i in 0..length {
            let count = ((i + radius).min(length - 1) - i.saturating_sub(radius) + 1) as u32;
            target[base + i * stride] = Rgb8Pixel {
                r: (sums[0] / count) as u8,
                g: (sums[1] / count) as u8,
                b: (sums[2] / count) as u8,
            };
            if i >= radius {
                let p = source[base + (i - radius) * stride];
                sums[0] -= p.r as u32;
                sums[1] -= p.g as u32;
                sums[2] -= p.b as u32;
            }
            if i + radius + 1 < length {
                let p = source[base + (i + radius + 1) * stride];
                sums[0] += p.r as u32;
                sums[1] += p.g as u32;
                sums[2] += p.b as u32;
            }
        }
    }
}

pub(crate) struct SidebarDrag {
    pub(crate) start_x: f32,
    pub(crate) start_y: f32,
    pub(crate) initial_reveal: f32,
    pub(crate) last_x: f32,
    pub(crate) direction: f32,
    pub(crate) moved: bool,
    tap: bool,
}

impl SidebarDrag {
    pub(crate) fn new(x: f32, y: f32, reveal: f32) -> Self {
        Self {
            start_x: x,
            start_y: y,
            initial_reveal: reveal,
            last_x: x,
            direction: 0.0,
            moved: false,
            tap: true,
        }
    }

    pub(crate) fn update(&mut self, x: f32, y: f32, width: f32) -> f32 {
        let dx = x - self.start_x;
        let dy = y - self.start_y;
        if dx.abs() > 6.0 || dy.abs() > 6.0 {
            self.tap = false;
        }
        if dx.abs() > 6.0 && (self.moved || dx.abs() > dy.abs()) {
            self.moved = true;
        }
        if self.moved {
            self.direction = x - self.last_x;
        }
        self.last_x = x;
        if self.moved {
            (self.initial_reveal + dx).clamp(0.0, width)
        } else {
            self.initial_reveal
        }
    }

    pub(crate) fn open_on_release(&self, reveal: f32, width: f32) -> bool {
        if !self.moved {
            return if self.tap {
                self.initial_reveal < width * 0.5
            } else {
                self.initial_reveal >= width * 0.5
            };
        }
        if self.direction.abs() > 12.0 {
            self.direction > 0.0
        } else {
            reveal >= width * 0.5
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_preserves_flat_colors_and_does_not_accumulate() {
        let size = PhysicalSize::new(42, 30);
        let color = Rgb8Pixel {
            r: 80,
            g: 120,
            b: 160,
        };
        let mut scene = vec![color; (size.width * size.height) as usize];
        let mut blur = BackdropBlur::new(size, 3.0);
        blur.capture(&scene);
        assert!(blur.buffer.as_slice().iter().all(|p| *p == color));
        // A bright center spreads into neighboring samples and loses contrast.
        for y in 12..18 {
            for x in 18..24 {
                scene[(y * size.width + x) as usize] = Rgb8Pixel {
                    r: 255,
                    g: 255,
                    b: 255,
                };
            }
        }
        blur.capture(&scene);
        let first = blur.buffer.as_slice().to_vec();
        assert!(first[2 * 7 + 3].r < 255);
        assert!(first[2 * 7 + 2].r > color.r);
        blur.capture(&scene);
        assert_eq!(blur.buffer.as_slice(), first);
        blur.capture(&vec![color; scene.len()]);
        assert!(blur.buffer.as_slice().iter().all(|p| *p == color));
    }

    #[test]
    fn drag_tracks_finger_clamps_and_snaps_in_both_directions() {
        let mut drag = SidebarDrag::new(2.0, 400.0, 0.0);
        assert_eq!(drag.update(2.0, 430.0, 340.0), 0.0);
        assert!(!drag.moved, "vertical motion must not pull the drawer");
        assert_eq!(drag.update(202.0, 400.0, 340.0), 200.0);
        assert!(drag.open_on_release(200.0, 340.0));
        assert_eq!(drag.update(600.0, 400.0, 340.0), 340.0);
        let mut close = SidebarDrag::new(340.0, 400.0, 340.0);
        assert_eq!(close.update(300.0, 400.0, 340.0), 300.0);
        assert!(
            !close.open_on_release(300.0, 340.0),
            "a closing flick wins over midpoint"
        );
        assert_eq!(close.update(-30.0, 400.0, 340.0), 0.0);
    }
}
