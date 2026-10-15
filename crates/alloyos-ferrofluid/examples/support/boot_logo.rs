pub fn render() -> Vec<u8> {
    // Use the actual Slint software renderer, fonts, layout and initial liquid
    // state. A separately rasterized SVG cannot guarantee the same pixels.
    use alloyos_ferrofluid::system_ui::FerrofluidSystemUi;
    use alloyos_proto::display::{PHONE_HEIGHT, PHONE_SCALE, PHONE_WIDTH};
    let mut ui = FerrofluidSystemUi::new(PHONE_WIDTH, PHONE_HEIGHT, PHONE_SCALE as f32)
        .expect("boot frame renderer");
    ui.set_booting(true);
    ui.render(std::time::Duration::ZERO)
        .expect("initial boot frame");
    let frame = ui.pixels();
    let lit: Vec<_> = frame
        .iter()
        .enumerate()
        .filter(|(_, pixel)| **pixel != 0xff000000)
        .map(|(i, _)| i)
        .collect();
    assert!(!lit.is_empty(), "boot logo must be visible");
    let x = lit.iter().map(|i| *i % PHONE_WIDTH as usize).min().unwrap();
    let y = lit.iter().map(|i| *i / PHONE_WIDTH as usize).min().unwrap();
    let width = lit.iter().map(|i| *i % PHONE_WIDTH as usize).max().unwrap() - x + 1;
    let height = lit.iter().map(|i| *i / PHONE_WIDTH as usize).max().unwrap() - y + 1;
    let mut pixels = Vec::with_capacity(width * height);
    for row in y..y + height {
        for col in x..x + width {
            let pixel = frame[row * PHONE_WIDTH as usize + col];
            let gray = pixel as u8;
            assert_eq!(pixel, 0xff000000 | gray as u32 * 0x010101);
            pixels.push(gray);
        }
    }
    // Verify the kernel's cropped blit reproduces every pixel of the UI frame.
    for row in 0..PHONE_HEIGHT as usize {
        for col in 0..PHONE_WIDTH as usize {
            let gray = if (y..y + height).contains(&row) && (x..x + width).contains(&col) {
                pixels[(row - y) * width + col - x]
            } else {
                0
            };
            assert_eq!(
                frame[row * PHONE_WIDTH as usize + col],
                0xff000000 | gray as u32 * 0x010101,
                "boot handoff pixel mismatch"
            );
        }
    }
    let mut asset = Vec::new();
    for value in [x, y, width, height] {
        asset.extend_from_slice(&(value as u32).to_le_bytes());
    }
    asset.extend_from_slice(&pixels);
    asset
}
