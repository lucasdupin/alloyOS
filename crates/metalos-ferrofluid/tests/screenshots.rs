#[path = "../../../tests/support/screenshots.rs"]
mod golden;
use metalos_ferrofluid::FerrofluidSystemUi;
use metalos_proto::{
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
    pointer(&mut ui, InputEventKind::PointerButtonDown, 75, 2000);
    for frame in 62..=69 {
        ui.render(Duration::from_millis(frame * 16));
    }
    snapshot(&mut suite, "keyboard-pressed", &ui);
    pointer(&mut ui, InputEventKind::PointerButtonUp, 75, 2000);
    for frame in 70..=100 {
        ui.render(Duration::from_millis(frame * 16));
    }
    pointer(&mut ui, InputEventKind::PointerButtonDown, 80, 2480);
    pointer(&mut ui, InputEventKind::PointerButtonUp, 80, 2480);
    for frame in 101..=160 {
        ui.render(Duration::from_millis(frame * 16));
    }
    snapshot(&mut suite, "keyboard-dismissed", &ui);
    suite.finish();
}
