#[path = "../../../tests/support/apps.rs"]
mod fixture_apps;
use fixture_apps::register_test_apps;
#[path = "../../../tests/support/screenshots.rs"]
mod golden;
use alloyos_ferrofluid::FerrofluidSystemUi;
use alloyos_mcp::McpRegistry;
use alloyos_proto::{
    display::{PHONE_HEIGHT, PHONE_SCALE, PHONE_WIDTH},
    InputEvent, InputEventKind,
};
use std::time::Duration;

fn snapshot(suite: &mut golden::Screenshots, name: &str, ui: &FerrofluidSystemUi) {
    let rgb: Vec<u8> = ui
        .pixels()
        .iter()
        .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8])
        .collect();
    suite.check(name, PHONE_WIDTH, PHONE_HEIGHT, &rgb);
}

fn pointer(ui: &mut FerrofluidSystemUi, kind: InputEventKind, x: i32, y: i32) {
    ui.handle_input(InputEvent {
        kind,
        code: 0x110,
        x,
        y,
    });
}

#[test]
fn system_ui_screenshots() {
    let mut suite = golden::Screenshots::new(env!("CARGO_MANIFEST_DIR"));
    let mut ui = FerrofluidSystemUi::new(PHONE_WIDTH, PHONE_HEIGHT, PHONE_SCALE as f32).unwrap();
    ui.set_booting(true);
    ui.render(Duration::ZERO);
    snapshot(&mut suite, "boot", &ui);
    ui.set_booting(false);
    ui.render(Duration::from_millis(16));
    snapshot(&mut suite, "workspace", &ui);
    pointer(&mut ui, InputEventKind::PointerButtonDown, 300, 2500);
    pointer(&mut ui, InputEventKind::PointerButtonUp, 300, 2500);
    for frame in 2..=61 {
        ui.render(Duration::from_millis(frame * 16));
        if frame == 11 {
            let rising = ui.pixels().to_vec();
            for _ in 0..3 {
                ui.render(Duration::from_millis(frame * 16));
                assert_eq!(
                    ui.pixels(),
                    rising,
                    "reactive System UI drifted at fixed animation time"
                );
            }
            snapshot(&mut suite, "keyboard-rising", &ui);
        }
        if frame == 61 {
            snapshot(&mut suite, "keyboard-open", &ui);
        }
    }
    pointer(&mut ui, InputEventKind::PointerButtonDown, 75, 2073);
    for frame in 62..=69 {
        ui.render(Duration::from_millis(frame * 16));
    }
    snapshot(&mut suite, "keyboard-pressed", &ui);
    pointer(&mut ui, InputEventKind::PointerButtonUp, 75, 2073);
    for frame in 70..=100 {
        ui.render(Duration::from_millis(frame * 16));
    }
    pointer(&mut ui, InputEventKind::PointerButtonDown, 80, 2480);
    pointer(&mut ui, InputEventKind::PointerButtonUp, 80, 2480);
    for frame in 101..=160 {
        ui.render(Duration::from_millis(frame * 16));
    }
    snapshot(&mut suite, "keyboard-dismissed", &ui);

    let fixture =
        alloyos_mcp::McpValue::from_json(include_str!("../../../tests/fixtures/dashboard.json"))
            .unwrap();
    let fixture = fixture.as_map().unwrap();
    ui.set_transcript(fixture["transcript"].as_str().unwrap());
    let mut registry = McpRegistry::new();
    register_test_apps(&mut registry).unwrap();
    registry.update_uptime(45_296_000);
    ui.set_tiles(&[registry.render_tile("sample_timer.now").unwrap()]);
    ui.render(Duration::from_millis(2600));
    snapshot(&mut suite, fixture["snapshot"].as_str().unwrap(), &ui);

    ui.set_tiles(&[]);
    ui.set_transcript("You: Help me plan the afternoon.");
    ui.set_reply_pending(true);
    ui.render(Duration::from_millis(2800));
    let thinking = ui.pixels().to_vec();
    ui.render(Duration::from_millis(2800));
    assert!(
        ui.pixels() == thinking.as_slice(),
        "thinking indicator changed at fixed time"
    );
    snapshot(&mut suite, "thinking", &ui);
    ui.render(Duration::from_millis(3200));
    ui.render(Duration::from_millis(3400));
    assert!(
        ui.pixels() != thinking,
        "thinking indicator must animate as time advances"
    );

    let base = "You: Write a detailed plan.";
    let mut reply = String::new();
    for chunk in 1..=36 {
        if chunk > 1 {
            reply.push('\n');
        }
        reply.push_str(&format!(
            "Step {chunk:02}: Take a short walk, pause for a rest, and review the next task."
        ));
        ui.update_streaming_reply(base, &reply);
        ui.render(Duration::from_millis(3400 + chunk * 16));
        assert!(
            (ui.chat_scroll_offset() - ui.chat_scroll_maximum()).abs() < 1.0,
            "streamed chunk {chunk} did not reveal the latest content"
        );
    }
    assert!(
        ui.chat_scroll_maximum() > 0.0,
        "stream fixture must overflow the viewport"
    );
    ui.set_reply_pending(false);
    ui.render(Duration::from_millis(4000));
    assert!((ui.chat_scroll_offset() - ui.chat_scroll_maximum()).abs() < 1.0);
    snapshot(&mut suite, "streaming-bottom", &ui);
    pointer(&mut ui, InputEventKind::PointerButtonDown, 400, 1000);
    for (frame, y) in [1010, 1040, 1100, 1200, 1400, 1600].into_iter().enumerate() {
        pointer(&mut ui, InputEventKind::PointerMove, 400, y);
        ui.render(Duration::from_millis(4016 + frame as u64 * 16));
    }
    pointer(&mut ui, InputEventKind::PointerButtonUp, 400, 1600);
    ui.render(Duration::from_millis(4500));
    assert!(
        ui.chat_scroll_offset() < ui.chat_scroll_maximum() - 1.0,
        "manual drag must move away from the latest content"
    );
    let offset_before_refresh = ui.chat_scroll_offset();
    ui.set_tiles(&[]);
    ui.render(Duration::from_millis(4501));
    assert!(
        (ui.chat_scroll_offset() - offset_before_refresh).abs() < 1.0,
        "descriptor refresh must preserve manual scroll position"
    );
    let height_before = ui.chat_scroll_maximum();
    reply.push('x');
    ui.update_streaming_reply(base, &reply);
    ui.render(Duration::from_millis(4516));
    assert!(
        (ui.chat_scroll_maximum() - height_before).abs() < 1.0,
        "single-character append should preserve the content height"
    );
    assert!(
        (ui.chat_scroll_offset() - ui.chat_scroll_maximum()).abs() < 1.0,
        "same-height streamed update must return to the latest content"
    );
    ui.set_tiles(&[]);
    ui.set_transcript("Your workspace is ready.");
    ui.set_model_selection_status("Model unavailable. Please try again.");
    ui.render(Duration::from_millis(4600));
    snapshot(&mut suite, "model-error", &ui);
    ui.set_model_selection_status("");
    ui.set_transcript("Welder: Your apps are ready.");
    use alloyos_mcp::protocol::{AppTile, TileItem};
    let mut cards = Vec::new();
    for card in fixture["cards"].as_array().unwrap() {
        let fields = card.as_array().unwrap();
        let id = fields[0].as_str().unwrap();
        let title = fields[1].as_str().unwrap();
        let value = fields[2].as_str().unwrap();
        cards.push(AppTile {
            tile_id: id.into(),
            title: title.into(),
            subtitle: String::new(),
            refresh_resource_uri: None,
            items: vec![TileItem {
                id: "value".into(),
                title: value.into(),
                subtitle: String::new(),
                badge: String::new(),
                status: String::new(),
            }],
            actions: Vec::new(),
        });
    }
    let item = cards[2].items[0].clone();
    cards[2].items.push(item.clone());
    cards[2].items.push(item);
    ui.set_tiles(&cards);
    ui.restore_dashboard(
        &cards
            .iter()
            .map(|card| card.tile_id.clone())
            .collect::<Vec<_>>(),
    );
    ui.render(Duration::from_millis(4616));
    // Drag to the dashboard at the start of the shared conversation scroll.
    pointer(&mut ui, InputEventKind::PointerButtonDown, 400, 500);
    for (frame, y) in [600, 800, 1200, 1700, 2200].into_iter().enumerate() {
        pointer(&mut ui, InputEventKind::PointerMove, 400, y);
        ui.render(Duration::from_millis(4632 + frame as u64 * 16));
    }
    pointer(&mut ui, InputEventKind::PointerButtonUp, 400, 2200);
    ui.render(Duration::from_millis(5000));
    snapshot(&mut suite, "dashboard-bento", &ui);
    suite.finish();
}
