//! Software 2D layout and rasterization pipeline for WebKit page rendering.

use alloc::string::String;
use alloc::vec::Vec;
use super::dom::{Color, DomElement, DomNode, HtmlDocument, SvgCommand, SvgGraphic};

/// 2D Software surface with drawing primitives and vector path rasterization.
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

impl Canvas {
    pub fn new(width: u32, height: u32, bg: Color) -> Self {
        let count = (width * height * 4) as usize;
        let mut bgra = Vec::with_capacity(count);
        for _ in 0..(width * height) {
            bgra.push(bg.2);
            bgra.push(bg.1);
            bgra.push(bg.0);
            bgra.push(255);
        }
        Self { width, height, bgra }
    }

    #[inline]
    pub fn set_pixel(&mut self, x: i32, y: i32, rgb: Color) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let idx = ((y as u32 * self.width + x as u32) * 4) as usize;
        self.bgra[idx] = rgb.2;
        self.bgra[idx + 1] = rgb.1;
        self.bgra[idx + 2] = rgb.0;
        self.bgra[idx + 3] = 255;
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, rgb: Color) {
        for py in y.max(0)..(y + h).min(self.height as i32) {
            for px in x.max(0)..(x + w).min(self.width as i32) {
                self.set_pixel(px, py, rgb);
            }
        }
    }

    pub fn fill_rounded_rect(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, rgb: Color) {
        let r = r.min(w / 2).min(h / 2);
        for py in y.max(0)..(y + h).min(self.height as i32) {
            for px in x.max(0)..(x + w).min(self.width as i32) {
                let dx = if px < x + r {
                    x + r - 1 - px
                } else if px >= x + w - r {
                    px - (x + w - r)
                } else {
                    0
                };
                let dy = if py < y + r {
                    y + r - 1 - py
                } else if py >= y + h - r {
                    py - (y + h - r)
                } else {
                    0
                };
                if dx * dx + dy * dy <= r * r {
                    self.set_pixel(px, py, rgb);
                }
            }
        }
    }

    pub fn stroke_rounded_rect(
        &mut self,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        r: i32,
        thickness: i32,
        rgb: Color,
    ) {
        let inner_r = (r - thickness).max(0);
        for py in y.max(0)..(y + h).min(self.height as i32) {
            for px in x.max(0)..(x + w).min(self.width as i32) {
                let odx = if px < x + r {
                    x + r - 1 - px
                } else if px >= x + w - r {
                    px - (x + w - r)
                } else {
                    0
                };
                let ody = if py < y + r {
                    y + r - 1 - py
                } else if py >= y + h - r {
                    py - (y + h - r)
                } else {
                    0
                };
                if odx * odx + ody * ody > r * r {
                    continue;
                }
                let ix = x + thickness;
                let iy = y + thickness;
                let iw = w - thickness * 2;
                let ih = h - thickness * 2;
                if px >= ix && px < ix + iw && py >= iy && py < iy + ih {
                    let idx = if px < ix + inner_r {
                        ix + inner_r - 1 - px
                    } else if px >= ix + iw - inner_r {
                        px - (ix + iw - inner_r)
                    } else {
                        0
                    };
                    let idy = if py < iy + inner_r {
                        iy + inner_r - 1 - py
                    } else if py >= iy + ih - inner_r {
                        py - (iy + ih - inner_r)
                    } else {
                        0
                    };
                    if idx * idx + idy * idy <= inner_r * inner_r {
                        continue;
                    }
                }
                self.set_pixel(px, py, rgb);
            }
        }
    }

    pub fn draw_thick_line(
        &mut self,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        radius: f32,
        rgb: Color,
    ) {
        let min_x = (x0.min(x1) - radius - 1.0) as i32;
        let max_x = (x0.max(x1) + radius + 1.0) as i32;
        let min_y = (y0.min(y1) - radius - 1.0) as i32;
        let max_y = (y0.max(y1) + radius + 1.0) as i32;
        let vx = x1 - x0;
        let vy = y1 - y0;
        let len_sq = (vx * vx + vy * vy).max(1.0);
        for py in min_y..=max_y {
            for px in min_x..=max_x {
                let wx = px as f32 - x0;
                let wy = py as f32 - y0;
                let t = ((wx * vx + wy * vy) / len_sq).clamp(0.0, 1.0);
                let cx = x0 + t * vx;
                let cy = y0 + t * vy;
                let dx = px as f32 - cx;
                let dy = py as f32 - cy;
                if dx * dx + dy * dy <= radius * radius {
                    self.set_pixel(px, py, rgb);
                }
            }
        }
    }

    pub fn draw_text(&mut self, mut x: i32, y: i32, text: &str, scale: i32, rgb: Color) {
        for raw_ch in text.chars() {
            let ch = to_ascii_folding(raw_ch);
            if ch == ' ' {
                x += 4 * scale;
                continue;
            }
            let rows = glyph_5x7(ch);
            for (row_idx, &row_bits) in rows.iter().enumerate() {
                for col in 0..5 {
                    if (row_bits & (1 << (4 - col))) != 0 {
                        self.fill_rect(
                            x + col * scale,
                            y + row_idx as i32 * scale,
                            scale,
                            scale,
                            rgb,
                        );
                    }
                }
            }
            x += 6 * scale;
        }
    }

    pub fn draw_svg_graphic(&mut self, x: i32, y: i32, graphic: &SvgGraphic, scale: f32) {
        for rect in &graphic.rects {
            let rx = (rect.x * scale) as i32 + x;
            let ry = (rect.y * scale) as i32 + y;
            let rw = (rect.width * scale) as i32;
            let rh = (rect.height * scale) as i32;
            let r_radius = (rect.rx * scale) as i32;
            let fill = rect.fill.unwrap_or((137, 207, 255));
            self.fill_rounded_rect(rx, ry, rw, rh, r_radius, fill);
        }

        for path in &graphic.paths {
            let stroke_col = path.stroke.unwrap_or((16, 20, 24));
            let radius = (path.stroke_width * scale / 2.0).max(1.0);

            let mut cur_x = 0.0f32;
            let mut cur_y = 0.0f32;
            let mut start_x = 0.0f32;
            let mut start_y = 0.0f32;

            for cmd in &path.commands {
                match *cmd {
                    SvgCommand::MoveTo(tx, ty) => {
                        cur_x = tx * scale + x as f32;
                        cur_y = ty * scale + y as f32;
                        start_x = cur_x;
                        start_y = cur_y;
                    }
                    SvgCommand::LineTo(tx, ty) => {
                        let nx = tx * scale + x as f32;
                        let ny = ty * scale + y as f32;
                        self.draw_thick_line(cur_x, cur_y, nx, ny, radius, stroke_col);
                        cur_x = nx;
                        cur_y = ny;
                    }
                    SvgCommand::HorizontalTo(tx) => {
                        let nx = tx * scale + x as f32;
                        self.draw_thick_line(cur_x, cur_y, nx, cur_y, radius, stroke_col);
                        cur_x = nx;
                    }
                    SvgCommand::VerticalTo(ty) => {
                        let ny = ty * scale + y as f32;
                        self.draw_thick_line(cur_x, cur_y, cur_x, ny, radius, stroke_col);
                        cur_y = ny;
                    }
                    SvgCommand::Close => {
                        self.draw_thick_line(cur_x, cur_y, start_x, start_y, radius, stroke_col);
                        cur_x = start_x;
                        cur_y = start_y;
                    }
                }
            }
        }
    }
}

pub fn to_ascii_folding(ch: char) -> char {
    match ch {
        'á' | 'à' | 'ã' | 'â' | 'ä' | 'Á' | 'À' | 'Ã' | 'Â' | 'Ä' => 'A',
        'é' | 'è' | 'ê' | 'ë' | 'É' | 'È' | 'Ê' | 'Ë' => 'E',
        'í' | 'ì' | 'î' | 'ï' | 'Í' | 'Ì' | 'Î' | 'Ï' => 'I',
        'ó' | 'ò' | 'õ' | 'ô' | 'ö' | 'Ó' | 'Ò' | 'Õ' | 'Ô' | 'Ö' => 'O',
        'ú' | 'ù' | 'û' | 'ü' | 'Ú' | 'Ù' | 'Û' | 'Ü' => 'U',
        'ç' | 'Ç' => 'C',
        'ñ' | 'Ñ' => 'N',
        other => other,
    }
}

pub fn glyph_5x7(ch: char) -> [u8; 7] {
    match ch.to_ascii_uppercase() {
        'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        'C' => [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
        'D' => [0b11100, 0b10010, 0b10001, 0b10001, 0b10001, 0b10010, 0b11100],
        'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        'G' => [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01110],
        'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'I' => [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100],
        'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        'N' => [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001],
        'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
        'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010],
        'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        'Y' => [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
        'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
        '0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
        '3' => [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110],
        '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => [0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110],
        '6' => [0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110],
        ':' => [0b00000, 0b00100, 0b00100, 0b00000, 0b00100, 0b00100, 0b00000],
        '.' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00100, 0b00100],
        ',' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00100, 0b00100, 0b01000],
        '!' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00100],
        '/' => [0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000],
        '-' => [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000],
        '+' => [0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000],
        '=' => [0b00000, 0b00000, 0b11111, 0b00000, 0b11111, 0b00000, 0b00000],
        '&' => [0b01100, 0b10010, 0b10100, 0b01000, 0b10101, 0b10010, 0b01101],
        '#' => [0b01010, 0b11111, 0b01010, 0b01010, 0b11111, 0b01010, 0b00000],
        '[' | '(' => [0b00110, 0b01000, 0b01000, 0b01000, 0b01000, 0b01000, 0b00110],
        ']' | ')' => [0b01100, 0b00010, 0b00010, 0b00010, 0b00010, 0b00010, 0b01100],
        '\'' => [0b00100, 0b00100, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000],
        '?' => [0b01110, 0b10001, 0b00010, 0b00100, 0b00100, 0b00000, 0b00100],
        '_' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b11111],
        _ => [0; 7],
    }
}

/// Hit test target recorded during layout.
#[derive(Debug, Clone)]
pub enum HitTarget {
    Input(String),
    Button(String),
}

/// Page layout and rendering engine.
pub struct PageRenderer {
    pub targets: Vec<(HitTarget, (i32, i32, i32, i32))>,
}

impl Default for PageRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl PageRenderer {
    pub fn new() -> Self {
        Self {
            targets: Vec::new(),
        }
    }

    /// Renders the document into a BGRA8 pixel buffer.
    pub fn render(
        &mut self,
        doc: &HtmlDocument,
        width: u32,
        height: u32,
        scroll_y: i32,
        focused_id: Option<&str>,
        svg_assets: &[(String, SvgGraphic)],
    ) -> Vec<u8> {
        self.targets.clear();

        let bg = doc.body_style.background.unwrap_or((16, 20, 24));
        let mut canvas = Canvas::new(width, height, bg);

        let pad = 36;
        let mut cur_y = 36 - scroll_y;
        let max_content_w = width as i32 - pad * 2;

        self.render_nodes(
            &doc.root_children,
            &mut canvas,
            pad,
            &mut cur_y,
            max_content_w,
            focused_id,
            svg_assets,
            doc,
        );

        canvas.bgra
    }

    fn render_nodes(
        &mut self,
        nodes: &[DomNode],
        canvas: &mut Canvas,
        x: i32,
        y: &mut i32,
        max_w: i32,
        focused_id: Option<&str>,
        svg_assets: &[(String, SvgGraphic)],
        doc: &HtmlDocument,
    ) {
        for node in nodes {
            match node {
                DomNode::Element(el) => {
                    self.render_element(el, canvas, x, y, max_w, focused_id, svg_assets, doc);
                }
                DomNode::Text(text) => {
                    let col = (238, 240, 242);
                    canvas.draw_text(x, *y, text, 2, col);
                    *y += 20;
                }
            }
        }
    }

    fn render_element(
        &mut self,
        el: &DomElement,
        canvas: &mut Canvas,
        x: i32,
        y: &mut i32,
        max_w: i32,
        focused_id: Option<&str>,
        svg_assets: &[(String, SvgGraphic)],
        doc: &HtmlDocument,
    ) {
        let tag = el.tag.as_str();

        match tag {
            "h1" => {
                let text = el.text_content.to_ascii_uppercase();
                canvas.draw_text(x, *y, &text, 3, (238, 240, 242));
                *y += 42;
            }
            "h2" => {
                let text = el.text_content.to_ascii_uppercase();
                canvas.draw_text(x, *y, &text, 2, (238, 240, 242));
                *y += 28;
            }
            "img" => {
                let src = el.get_attribute("src").unwrap_or("");
                let img_w = el
                    .get_attribute("width")
                    .and_then(|w| w.parse::<f32>().ok())
                    .unwrap_or(52.0);
                let img_h = el
                    .get_attribute("height")
                    .and_then(|h| h.parse::<f32>().ok())
                    .unwrap_or(52.0);

                if let Some((_, svg)) = svg_assets.iter().find(|(name, _)| name == src) {
                    let scale = (img_w / svg.width.max(1.0)).min(img_h / svg.height.max(1.0));
                    canvas.draw_svg_graphic(x, *y, svg, scale);
                } else {
                    // Fallback rounded rect placeholder
                    canvas.fill_rounded_rect(x, *y, img_w as i32, img_h as i32, 12, (137, 207, 255));
                }
                *y += img_h as i32 + 24;
            }
            "form" => {
                // Form container
                let mut form_y = *y;
                let mut label_text = String::new();
                let mut input_el: Option<&DomElement> = None;
                let mut btn_el: Option<&DomElement> = None;

                for child in &el.children {
                    if let DomNode::Element(cel) = child {
                        match cel.tag.as_str() {
                            "label" => {
                                label_text = cel.text_content.to_ascii_uppercase();
                            }
                            "input" => {
                                input_el = Some(cel);
                            }
                            "button" => {
                                btn_el = Some(cel);
                            }
                            _ => {}
                        }
                    }
                }

                if !label_text.is_empty() {
                    canvas.draw_text(x, form_y, &label_text, 2, (165, 173, 183));
                    form_y += 28;
                }

                let btn_w = 124;
                let input_w = max_w - btn_w - 14;
                let input_h = 52;

                if let Some(inp) = input_el {
                    let inp_id = inp.id.as_deref().unwrap_or("input");
                    let is_focused = focused_id == Some(inp_id);
                    let border_col = if is_focused {
                        (137, 207, 255) // Active focus highlight
                    } else {
                        (70, 76, 84)
                    };

                    canvas.fill_rounded_rect(x, form_y, input_w, input_h, 8, (22, 26, 31));
                    canvas.stroke_rounded_rect(x, form_y, input_w, input_h, 8, 2, border_col);

                    let val_upper = inp.value.to_ascii_uppercase();
                    if !val_upper.is_empty() {
                        canvas.draw_text(x + 18, form_y + 16, &val_upper, 3, (238, 240, 242));
                    }

                    self.targets.push((
                        HitTarget::Input(String::from(inp_id)),
                        (x, form_y, input_w, input_h),
                    ));
                }

                if let Some(btn) = btn_el {
                    let btn_x = x + input_w + 14;
                    let btn_id = btn.id.as_deref().unwrap_or("submit");

                    canvas.fill_rounded_rect(btn_x, form_y, btn_w, input_h, 8, (137, 207, 255));
                    let btn_text = if btn.text_content.is_empty() {
                        "GREET"
                    } else {
                        btn.text_content.as_str()
                    }
                    .to_ascii_uppercase();

                    canvas.draw_text(btn_x + 22, form_y + 16, &btn_text, 3, (16, 20, 24));

                    self.targets.push((
                        HitTarget::Button(String::from(btn_id)),
                        (btn_x, form_y, btn_w, input_h),
                    ));
                }

                *y = form_y + input_h + 28;
            }
            "p" => {
                if el.classes.iter().any(|c| c == "scroll-target") || el.style.border_top_width > 0 {
                    if el.style.margin_top > 0 {
                        *y += (el.style.margin_top / 3).max(60);
                    }
                    canvas.fill_rect(x, *y, max_w, 2, (137, 207, 255));
                    *y += 24;
                    let text = el.text_content.to_ascii_uppercase();
                    canvas.draw_text(x, *y, &text, 2, (165, 173, 183));
                    *y += 32;
                } else {
                    let text = el.text_content.to_ascii_uppercase();
                    let col = if text.starts_with("HELLO") {
                        (137, 207, 255)
                    } else {
                        (165, 173, 183)
                    };
                    canvas.draw_text(x, *y, &text, if text.starts_with("HELLO") { 3 } else { 2 }, col);
                    *y += 32;
                }
            }
            "div" | "section" | "article" | "body" => {
                self.render_nodes(
                    &el.children,
                    canvas,
                    x,
                    y,
                    max_w,
                    focused_id,
                    svg_assets,
                    doc,
                );
            }
            _ => {
                if !el.text_content.is_empty() {
                    let text = el.text_content.to_ascii_uppercase();
                    canvas.draw_text(x, *y, &text, 2, (238, 240, 242));
                    *y += 24;
                }
                self.render_nodes(
                    &el.children,
                    canvas,
                    x,
                    y,
                    max_w,
                    focused_id,
                    svg_assets,
                    doc,
                );
            }
        }
    }

    /// Performs hit testing to find interactive targets at `(px, py)`.
    pub fn hit_test(&self, px: i32, py: i32) -> Option<HitTarget> {
        let pad = 6;
        for (target, (x, y, w, h)) in &self.targets {
            if px >= *x - pad && px <= *x + *w + pad && py >= *y - pad && py <= *y + *h + pad {
                return Some(target.clone());
            }
        }
        None
    }
}
