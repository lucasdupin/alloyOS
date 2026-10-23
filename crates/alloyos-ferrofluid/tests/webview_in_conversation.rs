//! Separate executable: Slint permits only one System UI window per process.
//! Exercises the Ferrofluid-owned WebView embedded inline in the conversation stream.

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
fn webview_in_conversation_screenshot() {
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
    for frame in 0..=4 {
        ui.render(Duration::from_millis(frame * 16));
    }
    assert_eq!(
        count_mark_blue_pixels(&ui),
        0,
        "before attaching webview, no acceptance mark pixels exist"
    );

    let webview = webview_fixture::build_inline_acceptance_webview();
    assert_eq!(
        webview.trusted_origin().unwrap().as_str(),
        "https://local.alloyos.test"
    );
    assert_eq!(webview.untrusted_title(), webview_fixture::ACCEPTANCE_TITLE);
    assert!(webview.committed_frame().is_some());

    ui.set_webview(Some(&webview));
    ui.set_webview_expanded(false);
    for frame in 1..=6 {
        ui.render(Duration::from_millis(100 + frame * 16));
    }

    assert!(!ui.webview_expanded());
    assert!(
        count_mark_blue_pixels(&ui) > 1_000,
        "committed BGRA8 browser frame must render inside the conversation WebView card"
    );

    snapshot(&mut suite, "webview-in-conversation", &ui);

    // Tapping the Pin icon button on the inline conversation WebView card pins it.
    assert!(!ui.webview_pinned());
    assert!(!ui.dashboard_state().pinned_ids().contains(&"web.card".to_string()));
    tap(&mut ui, 1035, 976);
    ui.render(Duration::from_millis(100));
    assert!(
        ui.webview_pinned(),
        "tapping Pin on the conversation WebView card must pin the webview"
    );
    assert!(
        ui.dashboard_state().pinned_ids().contains(&"web.card".to_string()),
        "dashboard state must include web.card when pinned"
    );

    // Opening the sidebar with pinned webview renders drawer content without blank placeholder.
    ui.set_sidebar_open(true);
    for frame in 1..=4 {
        ui.render(Duration::from_millis(100 + frame * 16));
    }
    assert!(ui.sidebar_open());
    assert!(ui.webview_pinned());

    // Close sidebar and toggle pin off.
    ui.set_sidebar_open(false);
    ui.render(Duration::from_millis(100));
    tap(&mut ui, 1035, 976);
    ui.render(Duration::from_millis(100));
    assert!(
        !ui.webview_pinned(),
        "tapping Pin again on the conversation WebView card must unpin the webview"
    );
    assert!(
        !ui.dashboard_state().pinned_ids().contains(&"web.card".to_string()),
        "dashboard state must remove web.card when unpinned"
    );

    // Restoring dashboard pins with web.card repins the webview.
    ui.restore_dashboard(&["web.card".into()]);
    assert!(ui.webview_pinned());

    // Tapping the Expand icon button on the inline conversation WebView card promotes it.
    tap(&mut ui, 930, 976);
    ui.render(Duration::from_millis(250));
    assert!(
        ui.webview_expanded(),
        "tapping Expand on the conversation WebView card must enter expanded mode"
    );

    suite.finish();
}
