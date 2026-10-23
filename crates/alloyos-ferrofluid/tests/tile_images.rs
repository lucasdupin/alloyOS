//! Image delivery must repaint existing chat/pinned cards without breaking input.
use alloyos_ferrofluid::FerrofluidSystemUi;
use alloyos_mcp::{apps::parse_app_tile_from_json, tile_images::{encode, ImageAsset}};
use alloyos_proto::{display::{PHONE_WIDTH, PHONE_HEIGHT, PHONE_SCALE}, InputEvent, InputEventKind};
use std::time::Duration;

fn tap(ui: &mut FerrofluidSystemUi, x: i32, y: i32) {
    for kind in [InputEventKind::PointerButtonDown, InputEventKind::PointerButtonUp] {
        ui.handle_input(InputEvent { kind, code: 0x110, x, y });
    }
}
fn red_pixels(ui: &FerrofluidSystemUi) -> usize {
    ui.pixels().iter().filter(|p| (**p & 0xffffff) == 0xff0000).count()
}
#[test]
fn images_arrive_after_tile_creation_and_keyboard_still_accepts_soft_keys() {
    let src = "/apps/photos/cover.png";
    let tile = parse_app_tile_from_json(r#"{"tile_id":"photos.album","title":"Photos","image":{"src":"/apps/photos/cover.png","alt":"Red test image"},"items":[],"actions":[]}"#).unwrap();
    let mut ui = FerrofluidSystemUi::new(PHONE_WIDTH, PHONE_HEIGHT, PHONE_SCALE as f32).unwrap();
    ui.set_booting(false);
    ui.set_tiles(&[tile]);
    ui.render(Duration::ZERO);
    assert_eq!(red_pixels(&ui), 0);
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, 64, 32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().unwrap().write_image_data(&[255,0,0,255].repeat(64*32)).unwrap();
    }
    let assets = [ImageAsset { src: src.into(), data: encode(&png) }];
    ui.set_image_assets(&assets);
    ui.render(Duration::from_millis(16));
    assert!(red_pixels(&ui) > 10_000, "downloaded image must actually appear in chat pixels");
    std::fs::create_dir_all("../../target/screenshots/tile-images").unwrap();
    let file = std::fs::File::create("../../target/screenshots/tile-images/chat.png").unwrap();
    let mut encoder = png::Encoder::new(file, PHONE_WIDTH, PHONE_HEIGHT);
    encoder.set_color(png::ColorType::Rgb);
    encoder.write_header().unwrap().write_image_data(&ui.pixels().iter().flat_map(|p| [(p>>16) as u8,(p>>8) as u8,*p as u8]).collect::<Vec<_>>()).unwrap();
    // Failed/replaced images must remove stale pixels, while the card remains usable.
    ui.set_image_assets(&[ImageAsset { src: src.into(), data: "000102".into() }]);
    ui.render(Duration::from_millis(32));
    assert_eq!(red_pixels(&ui), 0);
    ui.set_image_assets(&assets);
    ui.render(Duration::from_millis(48));
    assert!(red_pixels(&ui) > 10_000);
    tap(&mut ui, 747, 2481);
    ui.render(Duration::from_millis(64));
    ui.render(Duration::from_millis(700));
    tap(&mut ui, 75, 2073);
    tap(&mut ui, 1070, 2480);
    assert_eq!(ui.take_submission().as_deref(), Some("q"));
}
