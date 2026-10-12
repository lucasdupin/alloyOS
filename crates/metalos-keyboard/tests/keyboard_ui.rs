//! Exercise the keyboard's public component without linking Ferrofluid.
use metalos_keyboard::preview::KeyboardPreview;
use metalos_proto::display::{PHONE_HEIGHT, PHONE_SCALE, PHONE_WIDTH};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

struct TestPlatform {
    window: Rc<MinimalSoftwareWindow>,
    now: Rc<Cell<Duration>>,
}

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
    fn duration_since_start(&self) -> Duration {
        self.now.get()
    }
}

#[test]
fn standalone_keyboard_types_switches_pages_and_dismisses() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    let now = Rc::new(Cell::new(Duration::ZERO));
    slint::platform::set_platform(Box::new(TestPlatform {
        window: window.clone(),
        now: now.clone(),
    }))
    .unwrap();
    let ui = KeyboardPreview::new().unwrap();
    window.dispatch_event(WindowEvent::ScaleFactorChanged {
        scale_factor: PHONE_SCALE as f32,
    });
    window.set_size(PhysicalSize::new(PHONE_WIDTH, PHONE_HEIGHT));
    window.dispatch_event(WindowEvent::WindowActiveChanged(true));
    ui.show().unwrap();
    let actions = Rc::new(RefCell::new(Vec::new()));
    let captured = actions.clone();
    ui.on_key(move |value| captured.borrow_mut().push(value.to_string()));
    let mut rgb = vec![slint::Rgb8Pixel::default(); (PHONE_WIDTH * PHONE_HEIGHT) as usize];
    let ink = |rgb: &[slint::Rgb8Pixel]| {
        (1940..2070)
            .flat_map(|y| (0..140).map(move |x| (x, y)))
            .filter(|&(x, y)| rgb[(y * PHONE_WIDTH + x) as usize].r > 128)
            .count()
    };
    let preview_ink = Cell::new(0);
    let mut render = |millis| {
        now.set(Duration::from_millis(millis));
        slint::platform::update_timers_and_animations();
        window.draw_if_needed(|renderer| {
            renderer.render(&mut rgb, PHONE_WIDTH as usize);
        });
        preview_ink.set(
            (1780..1940)
                .flat_map(|y| (0..160).map(move |x| (x, y)))
                .filter(|&(x, y)| rgb[(y * PHONE_WIDTH + x) as usize].r > 128)
                .count(),
        );
        ink(&rgb)
    };
    let tap = |x: f32, y: f32| {
        let position = LogicalPosition::new(x / PHONE_SCALE as f32, y / PHONE_SCALE as f32);
        window.dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
        window.dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
    };
    render(0);
    assert_eq!(ui.get_reveal(), 0.0);
    ui.set_requested(true);
    for frame in 1..=35 {
        render(frame * 16);
    }
    assert_eq!(ui.get_reveal(), 260.0);
    let resting_ink = render(560);
    assert_eq!(preview_ink.get(), 0);
    tap(75.0, 2000.0); // q; press and release in one frame
    render(576);
    let expanded_ink = render(672);
    assert!(
        preview_ink.get() > 100,
        "quick tap shows a large character above the key"
    );
    assert!(
        expanded_ink > resting_ink,
        "quick tap must visibly enlarge rendered q: {resting_ink} -> {expanded_ink}"
    );
    render(752);
    let settled_ink = render(960);
    assert_eq!(preview_ink.get(), 0, "raised preview fades after release");
    assert_eq!(
        settled_ink, resting_ink,
        "label returns to its original size"
    );
    tap(100.0, 2290.0); // Shift
    render(992);
    tap(75.0, 2000.0); // Q
    render(1008);
    tap(220.0, 2480.0); // 123
    render(1024);
    tap(75.0, 2000.0); // 1
    assert_eq!(*actions.borrow(), vec!["q", "Q", "1"]);
    tap(80.0, 2480.0); // Hide
    render(1040);
    render(1520);
    assert_eq!(ui.get_reveal(), 0.0);
    ui.set_requested(false);
    render(1536);
    ui.set_requested(true);
    render(1552);
    render(2080);
    assert_eq!(ui.get_reveal(), 260.0, "new input request reopens keyboard");
    ui.set_scroll_offset(1.0);
    render(2096);
    render(2640);
    assert_eq!(
        ui.get_reveal(),
        0.0,
        "the first scroll movement fully dismisses the keyboard"
    );
    ui.set_scroll_offset(0.0);
    render(2656);
    render(3200);
    assert_eq!(ui.get_reveal(), 0.0, "scroll reversal keeps keyboard hidden");
    ui.set_requested(false);
    render(3216);
    ui.set_requested(true);
    render(3232);
    render(3776);
    assert_eq!(ui.get_reveal(), 260.0, "refocusing reopens the keyboard");
}
