//! Render the actual bare-metal Slint adapter without a host windowing backend.
use metalos_ferrofluid::system_ui::FerrofluidSystemUi;
use metalos_proto::display::{PHONE_HEIGHT, PHONE_SCALE, PHONE_WIDTH};
use std::{io::Write, time::Duration};

fn main() {
    let mut ui =
        FerrofluidSystemUi::new(PHONE_WIDTH, PHONE_HEIGHT, PHONE_SCALE as f32).expect("System UI");
    ui.set_booting(std::env::args().any(|arg| arg == "--boot"));
    ui.set_status("Ready · AI unavailable");
    ui.render(Duration::ZERO).expect("initial frame");
    let path = std::env::args().nth(1).expect("Output PPM path");
    let mut file = std::fs::File::create(path).unwrap();
    writeln!(file, "P6\n{} {}\n255", PHONE_WIDTH, PHONE_HEIGHT).unwrap();
    for pixel in ui.pixels() {
        file.write_all(&[(pixel >> 16) as u8, (pixel >> 8) as u8, *pixel as u8])
            .unwrap();
    }
}
