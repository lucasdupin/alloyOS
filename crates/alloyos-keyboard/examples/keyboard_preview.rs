//! Render the keyboard in isolation using the same no_std software backend.
use alloyos_keyboard::{preview::KeyboardPreview, LiquidDisplacement, SoftKey};
use alloyos_proto::display::{PHONE_HEIGHT, PHONE_SCALE, PHONE_WIDTH};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize};
use std::{cell::Cell, io::Write, rc::Rc, time::Duration};

struct PreviewPlatform {
    window: Rc<MinimalSoftwareWindow>,
    clock: Rc<Cell<Duration>>,
}

impl Platform for PreviewPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
    fn duration_since_start(&self) -> Duration {
        self.clock.get()
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("output PPM path");
    let sliding = std::env::args().any(|arg| arg == "--liquid");
    let pressed = std::env::args().any(|arg| arg == "--pressed");
    let scrolled = std::env::args().any(|arg| arg == "--scroll");
    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    let clock = Rc::new(Cell::new(Duration::ZERO));
    slint::platform::set_platform(Box::new(PreviewPlatform {
        window: window.clone(),
        clock: clock.clone(),
    }))
    .unwrap();
    let ui = KeyboardPreview::new().unwrap();
    let size = PhysicalSize::new(PHONE_WIDTH, PHONE_HEIGHT);
    window.dispatch_event(WindowEvent::ScaleFactorChanged {
        scale_factor: PHONE_SCALE as f32,
    });
    window.set_size(size);
    ui.on_key(|value| println!("{:?}", SoftKey::from(value)));
    ui.show().unwrap();
    let mut rgb = vec![slint::Rgb8Pixel::default(); (PHONE_WIDTH * PHONE_HEIGHT) as usize];
    let mut map = LiquidDisplacement::new(PHONE_WIDTH, PHONE_HEIGHT, PHONE_SCALE as f32);
    window.draw_if_needed(|renderer| {
        renderer.render(&mut rgb, PHONE_WIDTH as usize);
    });
    map.advance(Duration::ZERO, ui.get_reveal(), false);
    ui.set_requested(true);
    let frames = if sliding { 10 } else { 70 };
    for frame in 0..=frames {
        clock.set(Duration::from_millis(frame * 16));
        if frame == 35 && scrolled {
            ui.set_scroll_offset(156.0);
        }
        if frame == 60 && pressed {
            let (x, y) = map.source(75.0, 2000.0);
            window.dispatch_event(WindowEvent::PointerPressed {
                position: LogicalPosition::new(x / PHONE_SCALE as f32, y / PHONE_SCALE as f32),
                button: PointerEventButton::Left,
            });
        }
        slint::platform::update_timers_and_animations();
        map.advance(clock.get(), ui.get_reveal(), false);
        window.draw_if_needed(|renderer| {
            renderer.render(&mut rgb, PHONE_WIDTH as usize);
        });
    }
    let mut file = std::fs::File::create(path).unwrap();
    writeln!(file, "P6\n{} {}\n255", PHONE_WIDTH, PHONE_HEIGHT).unwrap();
    for y in 0..PHONE_HEIGHT {
        for x in 0..PHONE_WIDTH {
            let pixel = map.pixel(&rgb, x, y);
            file.write_all(&[pixel.r, pixel.g, pixel.b]).unwrap();
        }
    }
}
