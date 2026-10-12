use alloc::vec;
use alloc::vec::Vec;
use metalos_proto::{DirtyRect, FramebufferInfo};

use crate::theme::{blend_argb, liquid_mercury_border_color, BG_OBSIDIAN};

/// Double-buffered 32-bpp ARGB8888 canvas with `DirtyRect` damage tracking,
/// fast page-flipping, and vsync frame counter.
#[derive(Debug, Clone)]
pub struct MercuryFramebuffer {
    pub width: u32,
    pub height: u32,
    pub back_buffer: Vec<u32>,
    pub front_buffer: Vec<u32>,
    pub dirty_rects: Vec<DirtyRect>,
    pub vsync_counter: u64,
}

impl MercuryFramebuffer {
    /// Creates a new double-buffered ARGB8888 framebuffer initialized to `BG_OBSIDIAN`.
    pub fn new(width: u32, height: u32) -> Self {
        let len = (width as usize).saturating_mul(height as usize);
        Self {
            width,
            height,
            back_buffer: vec![BG_OBSIDIAN; len],
            front_buffer: vec![BG_OBSIDIAN; len],
            dirty_rects: Vec::new(),
            vsync_counter: 0,
        }
    }

    /// Returns the `metalos_proto::FramebufferInfo` geometry descriptor.
    pub const fn info(&self) -> FramebufferInfo {
        FramebufferInfo {
            width: self.width,
            height: self.height,
            stride_bytes: self.width * 4,
            bytes_per_pixel: 4,
        }
    }

    /// Returns the total number of completed vsync page-flips.
    pub const fn vsync_count(&self) -> u64 {
        self.vsync_counter
    }

    /// Clips a rectangle to the framebuffer boundaries.
    pub fn clip_rect(&self, rect: DirtyRect) -> Option<DirtyRect> {
        if rect.width == 0 || rect.height == 0 || rect.x >= self.width || rect.y >= self.height {
            return None;
        }
        let x2 = rect.x.saturating_add(rect.width).min(self.width);
        let y2 = rect.y.saturating_add(rect.height).min(self.height);
        let w = x2.saturating_sub(rect.x);
        let h = y2.saturating_sub(rect.y);
        if w == 0 || h == 0 {
            None
        } else {
            Some(DirtyRect::new(rect.x, rect.y, w, h))
        }
    }

    /// Records a damaged rectangular region and coalesces overlapping or
    /// edge-adjacent rectangles.
    pub fn mark_dirty(&mut self, rect: DirtyRect) {
        let Some(mut candidate) = self.clip_rect(rect) else {
            return;
        };

        let mut i = 0;
        while i < self.dirty_rects.len() {
            if rects_overlap_or_touch(self.dirty_rects[i], candidate) {
                let existing = self.dirty_rects.swap_remove(i);
                candidate = rect_union(existing, candidate);
                i = 0;
            } else {
                i += 1;
            }
        }
        self.dirty_rects.push(candidate);
    }

    /// Writes a single pixel into the back buffer without recording a dirty rect.
    #[inline]
    pub fn put_pixel_raw(&mut self, x: u32, y: u32, color: u32) {
        if x < self.width && y < self.height {
            let idx = (y as usize) * (self.width as usize) + (x as usize);
            self.back_buffer[idx] = color;
        }
    }

    /// Writes a single pixel into the back buffer and marks `(x, y, 1, 1)` dirty.
    #[inline]
    pub fn put_pixel(&mut self, x: u32, y: u32, color: u32) {
        if x < self.width && y < self.height {
            let idx = (y as usize) * (self.width as usize) + (x as usize);
            self.back_buffer[idx] = color;
            self.mark_dirty(DirtyRect::new(x, y, 1, 1));
        }
    }

    /// Alpha-blends a pixel onto the existing back-buffer pixel at `(x, y)`.
    #[inline]
    pub fn put_pixel_blended_raw(&mut self, x: u32, y: u32, fg: u32, alpha: u8) {
        if x < self.width && y < self.height {
            let idx = (y as usize) * (self.width as usize) + (x as usize);
            let bg = self.back_buffer[idx];
            self.back_buffer[idx] = blend_argb(bg, fg, alpha);
        }
    }

    /// Reads a pixel from the back buffer.
    #[inline]
    pub fn get_back_pixel(&self, x: u32, y: u32) -> Option<u32> {
        if x < self.width && y < self.height {
            Some(self.back_buffer[(y as usize) * (self.width as usize) + (x as usize)])
        } else {
            None
        }
    }

    /// Reads a pixel from the presented front buffer.
    #[inline]
    pub fn get_front_pixel(&self, x: u32, y: u32) -> Option<u32> {
        if x < self.width && y < self.height {
            Some(self.front_buffer[(y as usize) * (self.width as usize) + (x as usize)])
        } else {
            None
        }
    }

    /// Clears the entire back buffer to `color` and marks the full screen dirty.
    pub fn clear(&mut self, color: u32) {
        self.back_buffer.fill(color);
        self.dirty_rects.clear();
        if self.width > 0 && self.height > 0 {
            self.dirty_rects
                .push(DirtyRect::new(0, 0, self.width, self.height));
        }
    }

    /// Fills a solid rectangle in the back buffer and marks the clipped region dirty.
    pub fn fill_rect(
        &mut self,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        color: u32,
    ) -> Option<DirtyRect> {
        let clipped = self.clip_rect(DirtyRect::new(x, y, width, height))?;
        let stride = self.width as usize;
        for row in clipped.y..(clipped.y + clipped.height) {
            let row_start = (row as usize) * stride + (clipped.x as usize);
            let row_end = row_start + (clipped.width as usize);
            self.back_buffer[row_start..row_end].fill(color);
        }
        self.mark_dirty(clipped);
        Some(clipped)
    }

    /// Draws a 1px horizontal line in the back buffer.
    pub fn draw_horizontal_line(
        &mut self,
        x: u32,
        y: u32,
        width: u32,
        color: u32,
    ) -> Option<DirtyRect> {
        self.fill_rect(x, y, width, 1, color)
    }

    /// Draws a 1px vertical line in the back buffer.
    pub fn draw_vertical_line(
        &mut self,
        x: u32,
        y: u32,
        height: u32,
        color: u32,
    ) -> Option<DirtyRect> {
        self.fill_rect(x, y, 1, height, color)
    }

    /// Draws an outlined rectangle border of given `thickness` and `color`.
    pub fn draw_rect_border(
        &mut self,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        thickness: u32,
        color: u32,
    ) -> Option<DirtyRect> {
        let clipped = self.clip_rect(DirtyRect::new(x, y, width, height))?;
        let t = thickness.min(clipped.width / 2).min(clipped.height / 2).max(1);
        // Top & bottom
        self.fill_rect(clipped.x, clipped.y, clipped.width, t, color);
        self.fill_rect(
            clipped.x,
            clipped.y + clipped.height - t,
            clipped.width,
            t,
            color,
        );
        // Left & right
        self.fill_rect(clipped.x, clipped.y, t, clipped.height, color);
        self.fill_rect(
            clipped.x + clipped.width - t,
            clipped.y,
            t,
            clipped.height,
            color,
        );
        self.mark_dirty(clipped);
        Some(clipped)
    }

    /// Draws the signature metalOS liquid-mercury drip perimeter border around a
    /// rectangle, seamlessly connecting the kernel boot splash aesthetic to the
    /// Mercury AI Console frame.
    pub fn draw_liquid_mercury_border(
        &mut self,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Option<DirtyRect> {
        let clipped = self.clip_rect(DirtyRect::new(x, y, width, height))?;
        let perim = (clipped.width * 2 + clipped.height * 2).max(1);
        let mut step = 0u32;

        for px in clipped.x..(clipped.x + clipped.width) {
            let c = liquid_mercury_border_color(step, perim);
            self.put_pixel_raw(px, clipped.y, c);
            step += 1;
        }
        for py in clipped.y..(clipped.y + clipped.height) {
            let c = liquid_mercury_border_color(step, perim);
            self.put_pixel_raw(clipped.x + clipped.width - 1, py, c);
            step += 1;
        }
        for px in (clipped.x..(clipped.x + clipped.width)).rev() {
            let c = liquid_mercury_border_color(step, perim);
            self.put_pixel_raw(px, clipped.y + clipped.height - 1, c);
            step += 1;
        }
        for py in (clipped.y..(clipped.y + clipped.height)).rev() {
            let c = liquid_mercury_border_color(step, perim);
            self.put_pixel_raw(clipped.x, py, c);
            step += 1;
        }

        self.mark_dirty(clipped);
        Some(clipped)
    }

    /// Copies damaged regions from `back_buffer` into `front_buffer`, increments
    /// the vsync frame counter, and returns the presented `DirtyRect` list.
    pub fn present(&mut self) -> Vec<DirtyRect> {
        let stride = self.width as usize;
        for rect in &self.dirty_rects {
            for row in rect.y..(rect.y + rect.height) {
                let start = (row as usize) * stride + (rect.x as usize);
                let end = start + (rect.width as usize);
                self.front_buffer[start..end].copy_from_slice(&self.back_buffer[start..end]);
            }
        }
        self.vsync_counter = self.vsync_counter.wrapping_add(1);
        core::mem::take(&mut self.dirty_rects)
    }
}

#[inline]
fn rects_overlap_or_touch(a: DirtyRect, b: DirtyRect) -> bool {
    let a_right = a.x.saturating_add(a.width);
    let a_bottom = a.y.saturating_add(a.height);
    let b_right = b.x.saturating_add(b.width);
    let b_bottom = b.y.saturating_add(b.height);

    a.x <= b_right && b.x <= a_right && a.y <= b_bottom && b.y <= a_bottom
}

#[inline]
pub(crate) fn rect_union(a: DirtyRect, b: DirtyRect) -> DirtyRect {
    if a.width == 0 || a.height == 0 {
        return b;
    }
    if b.width == 0 || b.height == 0 {
        return a;
    }
    let min_x = a.x.min(b.x);
    let min_y = a.y.min(b.y);
    let max_x = a.x.saturating_add(a.width).max(b.x.saturating_add(b.width));
    let max_y = a
        .y
        .saturating_add(a.height)
        .max(b.y.saturating_add(b.height));
    DirtyRect::new(min_x, min_y, max_x - min_x, max_y - min_y)
}
