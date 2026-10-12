#[path = "../../../tests/support/screenshots.rs"]
mod golden;
use metalos_keyboard::{preview::KeyboardPreview, LiquidDisplacement};
use metalos_proto::display::{PHONE_HEIGHT, PHONE_SCALE, PHONE_WIDTH};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize};
use std::{cell::Cell, rc::Rc, time::Duration};
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

#[test]
fn keyboard_screenshots() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    let clock = Rc::new(Cell::new(Duration::ZERO));
    slint::platform::set_platform(Box::new(PreviewPlatform {
        window: window.clone(),
        clock: clock.clone(),
    }))
    .unwrap();
    let ui = KeyboardPreview::new().unwrap();
    window.dispatch_event(WindowEvent::ScaleFactorChanged {
        scale_factor: PHONE_SCALE as f32,
    });
    window.set_size(PhysicalSize::new(PHONE_WIDTH, PHONE_HEIGHT));
    ui.show().unwrap();
    let mut rgb = vec![slint::Rgb8Pixel::default(); (PHONE_WIDTH * PHONE_HEIGHT) as usize];
    let mut map = LiquidDisplacement::new(PHONE_WIDTH, PHONE_HEIGHT, PHONE_SCALE as f32);
    let mut tick = |frame: u64, capture: bool| {
        clock.set(Duration::from_millis(frame * 16));
        slint::platform::update_timers_and_animations();
        map.advance(clock.get(), ui.get_reveal(), false);
        window.draw_if_needed(|renderer| {
            renderer.render(&mut rgb, PHONE_WIDTH as usize);
        });
        if !capture {
            return Vec::new();
        }
        let mut pixels = Vec::with_capacity(rgb.len() * 3);
        for y in 0..PHONE_HEIGHT {
            for x in 0..PHONE_WIDTH {
                let p = map.pixel(&rgb, x, y);
                pixels.extend_from_slice(&[p.r, p.g, p.b]);
            }
        }
        pixels
    };
    let mut screenshots = golden::Screenshots::new(env!("CARGO_MANIFEST_DIR"));
    tick(0, false);
    assert_eq!(
        ui.get_reveal(),
        0.0,
        "initialize the reactive binding before requesting a slide"
    );
    ui.set_requested(true);
    for frame in 1..=60 {
        let pixels = tick(frame, [10, 60, 68, 100, 130, 190].contains(&frame));
        if frame == 10 {
            let reveal = ui.get_reveal();
            assert!(
                reveal > 0.0 && reveal < 260.0,
                "capture must be mid-animation"
            );
            // Extra reactive evaluations and redraws at the same virtual time
            // must not move the keyboard or alter the velocity-driven map.
            for _ in 0..3 {
                window.request_redraw();
                assert_eq!(
                    tick(frame, true),
                    pixels,
                    "rising frame drifted without clock advancement"
                );
                assert_eq!(ui.get_reveal(), reveal);
            }
            screenshots.check("rising", PHONE_WIDTH, PHONE_HEIGHT, &pixels);
        }
        if frame == 60 {
            screenshots.check("resting", PHONE_WIDTH, PHONE_HEIGHT, &pixels);
        }
    }
    let point = LogicalPosition::new(75.0 / PHONE_SCALE as f32, 2000.0 / PHONE_SCALE as f32);
    window.dispatch_event(WindowEvent::PointerPressed {
        position: point,
        button: PointerEventButton::Left,
    });
    for frame in 61..=68 {
        let pixels = tick(frame, [10, 60, 68, 100, 130, 190].contains(&frame));
        if frame == 68 {
            screenshots.check("pressed", PHONE_WIDTH, PHONE_HEIGHT, &pixels);
        }
    }
    window.dispatch_event(WindowEvent::PointerReleased {
        position: point,
        button: PointerEventButton::Left,
    });
    for frame in 69..=100 {
        let pixels = tick(frame, [10, 60, 68, 100, 130, 190].contains(&frame));
        if frame == 100 {
            screenshots.check("released", PHONE_WIDTH, PHONE_HEIGHT, &pixels);
        }
    }
    // Switch to the symbol page through its actual touch target.
    let point = LogicalPosition::new(100.0 / PHONE_SCALE as f32, 2480.0 / PHONE_SCALE as f32);
    window.dispatch_event(WindowEvent::PointerPressed {
        position: point,
        button: PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position: point,
        button: PointerEventButton::Left,
    });
    for frame in 101..=130 {
        let pixels = tick(frame, [10, 60, 68, 100, 130, 190].contains(&frame));
        if frame == 130 {
            screenshots.check("symbols", PHONE_WIDTH, PHONE_HEIGHT, &pixels);
        }
    }
    ui.set_scroll_offset(156.0);
    for frame in 131..=190 {
        let pixels = tick(frame, [10, 60, 68, 100, 130, 190].contains(&frame));
        if frame == 190 {
            screenshots.check("scrolled", PHONE_WIDTH, PHONE_HEIGHT, &pixels);
        }
    }
    screenshots.finish();
}
