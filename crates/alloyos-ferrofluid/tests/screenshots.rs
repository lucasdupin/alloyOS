#[path = "../../../tests/support/screenshots.rs"]
mod golden;
use alloyos_ferrofluid::FerrofluidSystemUi;
use alloyos_mcp::{register_builtin_apps, McpRegistry};
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

    ui.set_transcript("You: What time is it?\n\nWelder: Here's your clock.");
    let mut registry = McpRegistry::new();
    register_builtin_apps(&mut registry).unwrap();
    registry.update_uptime(45_296_000);
    ui.set_tiles(&[registry.render_tile("clock.now").unwrap()]);
    ui.render(Duration::from_millis(2600));
    snapshot(&mut suite, "clock-tile", &ui);

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
    suite.finish();
}
