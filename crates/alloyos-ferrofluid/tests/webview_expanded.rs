//! Separate executable: Slint permits only one System UI window per process.
//! Exercises the Ferrofluid-owned WebView in full expanded workspace mode.

#[path = "../../../tests/support/screenshots.rs"]
mod golden;
#[path = "support/webview_fixture.rs"]
mod webview_fixture;

use alloyos_ferrofluid::{ChatItem, FerrofluidSystemUi};
use alloyos_proto::{
    display::{PHONE_HEIGHT, PHONE_SCALE, PHONE_WIDTH},
    InputEvent, InputEventKind,
};
use std::time::Duration;

fn snapshot(suite: &mut golden::Screenshots, name: &str, ui: &FerrofluidSystemUi) {
    let mut rgb = Vec::with_capacity((PHONE_WIDTH * PHONE_HEIGHT * 3) as usize);
    for &pixel in ui.pixels() {
        rgb.extend_from_slice(&[(pixel >> 16) as u8, (pixel >> 8) as u8, pixel as u8]);
    }
    suite.check(name, PHONE_WIDTH, PHONE_HEIGHT, &rgb);
}

fn tap(ui: &mut FerrofluidSystemUi, x: i32, y: i32) {
    for kind in [
        InputEventKind::PointerButtonDown,
        InputEventKind::PointerButtonUp,
    ] {
        ui.handle_input(InputEvent {
            kind,
            code: 0x110,
            x,
            y,
        });
    }
}

fn count_mark_blue_pixels(ui: &FerrofluidSystemUi) -> usize {
    ui.pixels()
        .iter()
        .filter(|&&p| {
            let r = ((p >> 16) & 0xff) as i32;
            let g = ((p >> 8) & 0xff) as i32;
            let b = (p & 0xff) as i32;
            (r - 137).abs() <= 8 && (g - 207).abs() <= 8 && (b - 255).abs() <= 8
        })
        .count()
}

#[test]
fn webview_expanded_screenshot() {
    let mut suite = golden::Screenshots::new(env!("CARGO_MANIFEST_DIR"));
    let mut ui =
        FerrofluidSystemUi::new(PHONE_WIDTH, PHONE_HEIGHT, PHONE_SCALE as f32).unwrap();
    ui.set_booting(false);

    let chat = [
        ChatItem::Text(
            "Welcome to alloyOS.\nYour workspace takes shape around what you need.".into(),
        ),
        ChatItem::Text("You: Open the local web acceptance form.".into()),
        ChatItem::Text(
            "Welder: Opening the local acceptance page in a Ferrofluid WebView.".into(),
        ),
    ];
    ui.set_chat_items(&chat);

    let mut webview = webview_fixture::build_inline_acceptance_webview();
    ui.set_webview(Some(&webview));
    for frame in 0..=4 {
        ui.render(Duration::from_millis(frame * 16));
    }
    let inline_blue = count_mark_blue_pixels(&ui);
    assert!(inline_blue > 1_000);

    // Expand via the conversation card's Expand icon button, resize viewport, and commit updated frame.
    tap(&mut ui, 950, 976);
    ui.render(Duration::from_millis(100));
    assert!(ui.webview_expanded());

    webview_fixture::promote_to_expanded_acceptance_webview(&mut webview);
    ui.set_webview(Some(&webview));
    for frame in 1..=6 {
        ui.render(Duration::from_millis(100 + frame * 16));
    }

    let expanded_blue = count_mark_blue_pixels(&ui);
    assert!(
        expanded_blue > inline_blue,
        "expanded WebView surface must paint a larger viewport with the updated form state ({expanded_blue} vs {inline_blue})"
    );

    snapshot(&mut suite, "webview-expanded", &ui);

    // Tapping Reload icon in the expanded chrome dispatches a reload request.
    assert!(!ui.take_webview_reload());
    tap(&mut ui, 990, 106);
    ui.render(Duration::from_millis(220));
    assert!(
        ui.take_webview_reload(),
        "Reload icon button in expanded header must request a WebView reload"
    );

    // Tapping Collapse icon in the expanded chrome returns to the conversation view.
    tap(&mut ui, 1098, 106);
    ui.render(Duration::from_millis(250));
    assert!(
        !ui.webview_expanded(),
        "Collapse button in expanded header must return to the conversation stream"
    );

    suite.finish();
}
