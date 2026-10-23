//! Shared deterministic browser-frame generator and IPC packet driver for
//! Ferrofluid WebView screenshot tests (`tests/fixtures/web/local-form.html`).

use alloyos_ferrofluid::webview::{LoadingState, PointerPhase, WebViewController};
use alloyos_proto::web::{Opcode, Packet, View, MAX_PACKET_BYTES, MAX_PAYLOAD_BYTES};

pub const ACCEPTANCE_URL: &str = "https://local.alloyos.test/local-form.html";
pub const ACCEPTANCE_TITLE: &str = "alloyOS native web acceptance";

fn encode_reply(view: View, sequence: u32, opcode: Opcode, payload: &[u8]) -> Vec<u8> {
    let packet = Packet {
        opcode,
        view,
        sequence,
        payload,
    };
    let mut buf = [0u8; MAX_PACKET_BYTES];
    let len = packet.encode(&mut buf).expect("valid reply packet");
    buf[..len].to_vec()
}

pub fn send_frame_packets(
    controller: &mut WebViewController,
    sequence: u32,
    width: u32,
    height: u32,
    bgra: &[u8],
) {
    let stride = width * 4;
    assert_eq!(bgra.len(), (stride * height) as usize);
    let view = controller.view();

    let mut meta = [0u8; 28];
    for (index, val) in [width, height, stride, 0, 0, width, height]
        .into_iter()
        .enumerate()
    {
        meta[index * 4..index * 4 + 4].copy_from_slice(&val.to_le_bytes());
    }
    controller
        .handle_reply_bytes(&encode_reply(view, sequence, Opcode::BeginFrame, &meta))
        .expect("BeginFrame must succeed");

    let max_chunk_data = MAX_PAYLOAD_BYTES - 4;
    let mut offset = 0usize;
    while offset < bgra.len() {
        let end = (offset + max_chunk_data).min(bgra.len());
        let mut payload = Vec::with_capacity(4 + (end - offset));
        payload.extend_from_slice(&(offset as u32).to_le_bytes());
        payload.extend_from_slice(&bgra[offset..end]);
        controller
            .handle_reply_bytes(&encode_reply(view, sequence, Opcode::FrameChunk, &payload))
            .expect("FrameChunk must succeed");
        offset = end;
    }

    controller
        .handle_reply_bytes(&encode_reply(view, sequence, Opcode::EndFrame, &[]))
        .expect("EndFrame must succeed");
}

pub fn build_inline_acceptance_webview() -> WebViewController {
    let width = 576;
    let height = 380;
    let mut controller =
        WebViewController::new(1, width, height, 2000).expect("valid inline viewport");
    let _create_pkt = controller.encode_create().unwrap();
    let _nav_pkt = controller.encode_navigate(ACCEPTANCE_URL).unwrap();
    let view = controller.view();

    controller
        .handle_reply_bytes(&encode_reply(view, 1, Opcode::Created, &[]))
        .unwrap();
    controller
        .handle_reply_bytes(&encode_reply(
            view,
            2,
            Opcode::Url,
            ACCEPTANCE_URL.as_bytes(),
        ))
        .unwrap();
    controller
        .handle_reply_bytes(&encode_reply(
            view,
            3,
            Opcode::Title,
            ACCEPTANCE_TITLE.as_bytes(),
        ))
        .unwrap();
    controller
        .handle_reply_bytes(&encode_reply(
            view,
            4,
            Opcode::Loading,
            &[LoadingState::Idle as u8],
        ))
        .unwrap();

    let pixels = render_local_form_surface(width, height, false);
    send_frame_packets(&mut controller, 5, width, height, &pixels);
    controller
}

#[allow(dead_code)]
pub fn promote_to_expanded_acceptance_webview(controller: &mut WebViewController) {
    let width = 640;
    let height = 1520;
    let _resize_pkt = controller.encode_resize(width, height, 2000).unwrap();
    let _focus_pkt = controller.encode_focus(true).unwrap();
    let _down_pkt = controller
        .encode_pointer(PointerPhase::Down, 120, 210)
        .unwrap();
    let _up_pkt = controller
        .encode_pointer(PointerPhase::Up, 120, 210)
        .unwrap();

    let view = controller.view();
    controller
        .handle_reply_bytes(&encode_reply(view, 6, Opcode::Editing, &[1, 1]))
        .unwrap();
    let _commit_pkt = controller.encode_commit_text("Ada").unwrap();
    controller
        .handle_reply_bytes(&encode_reply(view, 7, Opcode::Editing, &[0, 0]))
        .unwrap();

    let pixels = render_local_form_surface(width, height, true);
    send_frame_packets(controller, 8, width, height, &pixels);
}

struct Canvas {
    width: u32,
    height: u32,
    bgra: Vec<u8>,
}

impl Canvas {
    fn new(width: u32, height: u32, bg: (u8, u8, u8)) -> Self {
        let mut bgra = vec![0u8; (width * height * 4) as usize];
        for px in bgra.chunks_exact_mut(4) {
            px[0] = bg.2;
            px[1] = bg.1;
            px[2] = bg.0;
            px[3] = 255;
        }
        Self {
            width,
            height,
            bgra,
        }
    }

    fn set_pixel(&mut self, x: i32, y: i32, rgb: (u8, u8, u8)) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let idx = ((y as u32 * self.width + x as u32) * 4) as usize;
        self.bgra[idx] = rgb.2;
        self.bgra[idx + 1] = rgb.1;
        self.bgra[idx + 2] = rgb.0;
        self.bgra[idx + 3] = 255;
    }

    fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, rgb: (u8, u8, u8)) {
        for py in y.max(0)..(y + h).min(self.height as i32) {
            for px in x.max(0)..(x + w).min(self.width as i32) {
                self.set_pixel(px, py, rgb);
            }
        }
    }

    fn fill_rounded_rect(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, rgb: (u8, u8, u8)) {
        for py in y..(y + h) {
            for px in x..(x + w) {
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

    fn stroke_rounded_rect(
        &mut self,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        r: i32,
        thickness: i32,
        rgb: (u8, u8, u8),
    ) {
        let inner_r = (r - thickness).max(0);
        for py in y..(y + h) {
            for px in x..(x + w) {
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

    fn draw_thick_line(
        &mut self,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        radius: f32,
        rgb: (u8, u8, u8),
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

    fn draw_mark_svg(&mut self, x: i32, y: i32, size: i32) {
        let r = (size * 12) / 64;
        self.fill_rounded_rect(x, y, size, size, r, (137, 207, 255));
        let scale = size as f32 / 64.0;
        let stroke = (2.6 * scale).max(1.5);
        let dark = (16, 20, 24);
        // Path: M16 48 L32 16 L48 48 M23 36 H41
        self.draw_thick_line(
            x as f32 + 16.0 * scale,
            y as f32 + 48.0 * scale,
            x as f32 + 32.0 * scale,
            y as f32 + 16.0 * scale,
            stroke,
            dark,
        );
        self.draw_thick_line(
            x as f32 + 32.0 * scale,
            y as f32 + 16.0 * scale,
            x as f32 + 48.0 * scale,
            y as f32 + 48.0 * scale,
            stroke,
            dark,
        );
        self.draw_thick_line(
            x as f32 + 23.0 * scale,
            y as f32 + 36.0 * scale,
            x as f32 + 41.0 * scale,
            y as f32 + 36.0 * scale,
            stroke,
            dark,
        );
    }

    fn draw_text(&mut self, mut x: i32, y: i32, text: &str, scale: i32, rgb: (u8, u8, u8)) {
        for ch in text.chars() {
            let rows = glyph_5x7(ch);
            for (row_idx, bits) in rows.iter().enumerate() {
                for col_idx in 0..5 {
                    if (bits >> (4 - col_idx)) & 1 != 0 {
                        self.fill_rect(
                            x + col_idx * scale,
                            y + (row_idx as i32) * scale,
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
}

fn render_local_form_surface(width: u32, height: u32, expanded: bool) -> Vec<u8> {
    let mut c = Canvas::new(width, height, (16, 20, 24));
    let pad = 36;

    // <img src="mark.svg" width="64" height="64"> + <h1>Native web acceptance</h1>
    c.draw_mark_svg(pad, 36, 52);
    c.draw_text(pad + 72, 52, "NATIVE WEB ACCEPTANCE", 3, (238, 240, 242));

    // <form id="greeting-form">
    let form_y = 124;
    let form_w = width as i32 - pad * 2;
    c.draw_text(pad, form_y, "YOUR NAME", 2, (165, 173, 183));

    // <input id="name"> + <button type="submit">Greet</button>
    let btn_w = 124;
    let input_x = pad;
    let input_y = form_y + 28;
    let input_w = form_w - btn_w - 14;
    let input_h = 52;
    c.fill_rounded_rect(input_x, input_y, input_w, input_h, 8, (22, 26, 31));
    let input_border = if expanded {
        (137, 207, 255)
    } else {
        (70, 76, 84)
    };
    c.stroke_rounded_rect(input_x, input_y, input_w, input_h, 8, 2, input_border);
    c.draw_text(input_x + 18, input_y + 16, "ADA", 3, (238, 240, 242));

    let btn_x = input_x + input_w + 14;
    c.fill_rounded_rect(btn_x, input_y, btn_w, input_h, 8, (137, 207, 255));
    c.draw_text(btn_x + 22, input_y + 16, "GREET", 3, (16, 20, 24));

    // <p id="result" role="status">
    let status_y = input_y + input_h + 28;
    if expanded {
        c.draw_text(pad, status_y, "HELLO, ADA!", 3, (137, 207, 255));
    } else {
        c.draw_text(pad, status_y, "WAITING FOR INPUT.", 2, (165, 173, 183));
    }

    if expanded {
        // Minimal .scroll-target section at the bottom of the expanded page
        let scroll_y = height as i32 - 140;
        c.fill_rect(pad, scroll_y, form_w, 2, (137, 207, 255));
        c.draw_text(
            pad,
            scroll_y + 24,
            "SCROLL ACCEPTANCE TARGET",
            2,
            (165, 173, 183),
        );
    }

    c.bgra
}

fn glyph_5x7(ch: char) -> [u8; 7] {
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
        '·' => [0b00000, 0b00000, 0b00000, 0b00100, 0b00000, 0b00000, 0b00000],
        _ => [0; 7],
    }
}
