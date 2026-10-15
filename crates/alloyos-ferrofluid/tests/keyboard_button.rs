//! Separate executable: Slint permits only one System UI window per process.
use alloyos_ferrofluid::system_ui::FerrofluidSystemUi;
use alloyos_proto::{InputEvent, InputEventKind, display::{PHONE_WIDTH, PHONE_HEIGHT, PHONE_SCALE}};
use std::time::Duration;

fn tap(ui: &mut FerrofluidSystemUi, x: i32, y: i32) {
    for kind in [InputEventKind::PointerButtonDown, InputEventKind::PointerButtonUp] {
        ui.handle_input(InputEvent { kind, code: 0x110, x, y });
    }
}

#[test]
fn keyboard_button_opens_without_hardware_key_input() {
    let mut ui = FerrofluidSystemUi::new(PHONE_WIDTH, PHONE_HEIGHT, PHONE_SCALE as f32).unwrap();
    ui.set_booting(false);
    ui.set_transcript("What should I call you?");
    ui.render(Duration::ZERO);
    let closed = ui.pixels().to_vec();
    ui.handle_input(InputEvent { kind: InputEventKind::PointerButtonDown, code: 0x110, x: 747, y: 2481 });
    ui.render(Duration::from_millis(120));
    ui.handle_input(InputEvent { kind: InputEventKind::PointerButtonUp, code: 0x110, x: 747, y: 2481 });
    ui.render(Duration::from_millis(136));
    ui.render(Duration::from_millis(680));
    assert_ne!(ui.pixels(), closed.as_slice(), "tap reveals keyboard");
    tap(&mut ui, 75, 2073); // Q
    tap(&mut ui, 1070, 2480); // Send
    assert_eq!(ui.take_submission().as_deref(), Some("q"));
    tap(&mut ui, 80, 2480); // Hide
    ui.render(Duration::from_millis(696));
    ui.render(Duration::from_millis(1240));
    tap(&mut ui, 747, 2481); // Reopen
    ui.render(Duration::from_millis(1256));
    ui.render(Duration::from_millis(1800));
    tap(&mut ui, 75, 2073);
    tap(&mut ui, 1070, 2480);
    assert_eq!(ui.take_submission().as_deref(), Some("q"), "reopened keyboard accepts taps");
}
