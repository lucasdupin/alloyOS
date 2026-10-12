//! Render the actual bare-metal Slint adapter without a host windowing backend.
use metalos_ferrofluid::system_ui::FerrofluidSystemUi;
use metalos_proto::display::{PHONE_HEIGHT, PHONE_SCALE, PHONE_WIDTH};
use std::{io::Write, time::Duration};

fn main() {
    let mut ui =
        FerrofluidSystemUi::new(PHONE_WIDTH, PHONE_HEIGHT, PHONE_SCALE as f32).expect("System UI");
    ui.set_booting(std::env::args().any(|arg| arg == "--boot"));
    ui.render(Duration::ZERO).expect("initial frame");
    if std::env::args().any(|arg| arg == "--boot") {
        ui.render(Duration::from_millis(1200));
    }
    if std::env::args().any(|arg| arg == "--keyboard") {
        for kind in [
            metalos_proto::InputEventKind::PointerButtonDown,
            metalos_proto::InputEventKind::PointerButtonUp,
        ] {
            ui.handle_input(metalos_proto::InputEvent {
                kind,
                code: 0x110,
                x: 300,
                y: 2500,
            });
        }
        let sliding = std::env::args().any(|arg| arg == "--liquid");
        for frame in 1..=if sliding { 10 } else { 60 } {
            ui.render(Duration::from_millis(frame * 16));
        }
        if std::env::args().any(|arg| arg == "--pressed") {
            ui.handle_input(metalos_proto::InputEvent {
                kind: metalos_proto::InputEventKind::PointerButtonDown,
                code: 0x110,
                x: 75,
                y: 2000,
            });
            for frame in 61..=68 {
                ui.render(Duration::from_millis(frame * 16));
            }
        }
    }
    let path = std::env::args().nth(1).expect("Output PPM path");
    let mut file = std::fs::File::create(path).unwrap();
    writeln!(file, "P6\n{} {}\n255", PHONE_WIDTH, PHONE_HEIGHT).unwrap();
    for pixel in ui.pixels() {
        file.write_all(&[(pixel >> 16) as u8, (pixel >> 8) as u8, *pixel as u8])
            .unwrap();
    }
}
