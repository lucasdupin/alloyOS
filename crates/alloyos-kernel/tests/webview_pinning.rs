use std::{sync::Mutex, time::Duration};

use alloyos_ferrofluid::{
    webview::WebViewController,
    FerrofluidSystemUi,
};
use alloyos_proto::{
    display::{PHONE_HEIGHT, PHONE_SCALE, PHONE_WIDTH},
    InputEvent, InputEventKind,
};
use alloyos_welder::{AgentRuntime, SharedMapVfsBackend, ToolVfsBackend};

static TEST_LOCK: Mutex<()> = Mutex::new(());

const TEST_WEB_URL: &str = "https://local.alloyos.test/local-form.html";
const TEST_WEB_TITLE: &str = "alloyOS native web acceptance";

fn tap_ui(ui: &mut FerrofluidSystemUi, x: i32, y: i32) {
    ui.handle_input(InputEvent {
        kind: InputEventKind::PointerMove,
        code: 0,
        x,
        y,
    });
    ui.handle_input(InputEvent {
        kind: InputEventKind::PointerButtonDown,
        code: 0x110,
        x,
        y,
    });
    ui.handle_input(InputEvent {
        kind: InputEventKind::PointerButtonUp,
        code: 0x110,
        x,
        y,
    });
}

#[test]
fn test_webview_card_pinning_and_url_persistence() {
    let _guard = TEST_LOCK.lock().unwrap();

    let vfs = SharedMapVfsBackend::shared();
    vfs.borrow_mut()
        .write_file(AgentRuntime::DASHBOARD_CONFIG_PATH, "")
        .expect("dashboard config init");

    let mut agent = AgentRuntime::with_vfs_backend(vfs.clone());
    let mut ui = FerrofluidSystemUi::new(PHONE_WIDTH, PHONE_HEIGHT, PHONE_SCALE as f32)
        .expect("FerrofluidSystemUi creation");
    ui.set_booting(false);

    // Initial state: no pins
    ui.restore_dashboard(agent.pinned_tile_ids());
    assert!(!ui.webview_pinned(), "webview should not be pinned initially");

    // Open webview
    let chat = [
        alloyos_ferrofluid::ChatItem::Text(
            "Welcome to alloyOS.\nYour workspace takes shape around what you need.".into(),
        ),
        alloyos_ferrofluid::ChatItem::Text("You: Open the local web acceptance form.".into()),
        alloyos_ferrofluid::ChatItem::Text(
            "Welder: Opening the local acceptance page in a Ferrofluid WebView.".into(),
        ),
    ];
    ui.set_chat_items(&chat);

    let controller = WebViewController::for_url(1, TEST_WEB_URL, TEST_WEB_TITLE)
        .expect("controller creation");
    ui.set_webview(Some(&controller));
    for frame in 0..6 {
        ui.render(Duration::from_millis(frame * 16));
    }

    assert!(!ui.webview_pinned());

    // 1. Click the Pin button on the WebView card (pin button position: ~1035, 976 at scale)
    tap_ui(&mut ui, 1035, 976);
    ui.render(Duration::from_millis(50));

    // Pinning must be active
    assert!(
        ui.webview_pinned(),
        "webview must be pinned after tapping pin button"
    );

    // The pinned ID must consistently contain the URL: `web.<url>`
    let expected_pin_id = format!("web.{}", TEST_WEB_URL);
    let pinned_ids = ui.dashboard_state().pinned_ids().to_vec();
    assert!(
        pinned_ids.contains(&expected_pin_id),
        "dashboard state must contain pin id '{expected_pin_id}', got: {pinned_ids:?}"
    );
    assert!(
        !pinned_ids.contains(&"web.card".to_string()),
        "dashboard state must not use generic 'web.card' without URL"
    );

    // 2. Persist to agent /etc/dashboard.conf
    agent
        .set_pinned_tile_ids(ui.dashboard_state().pinned_ids())
        .expect("set_pinned_tile_ids must succeed for web.<url>");

    let saved_conf = vfs
        .borrow_mut()
        .read_file(AgentRuntime::DASHBOARD_CONFIG_PATH)
        .expect("dashboard.conf must exist");
    assert!(
        saved_conf.lines().any(|line| line == expected_pin_id),
        "/etc/dashboard.conf must persist '{expected_pin_id}', found:\n{saved_conf}"
    );

    // 3. Kernel / Ferrofluid sync cycle must NOT unpin the webview
    // Simulate what kernel system_ui driver does on tick:
    ui.restore_dashboard(agent.pinned_tile_ids());
    ui.render(Duration::from_millis(50));

    assert!(
        ui.webview_pinned(),
        "webview must remain pinned across dashboard restoration / sync"
    );

    // 4. Persistence across reboot: a new AgentRuntime restores the pin with the URL
    let restored_agent = AgentRuntime::with_vfs_backend(vfs.clone());
    let restored_pins = restored_agent.pinned_tile_ids();
    assert!(
        restored_pins.contains(&expected_pin_id),
        "restored agent must have '{expected_pin_id}' in pinned_tile_ids, got: {restored_pins:?}"
    );

    // Simulate reboot restore in UI from persisted agent pins
    ui.restore_dashboard(restored_agent.pinned_tile_ids());
    ui.render(Duration::from_millis(50));

    assert!(
        ui.webview_pinned(),
        "restored webview must be pinned when restored from persistent storage with '{expected_pin_id}'"
    );

    // 5. Tapping Pin again unpins the webview
    tap_ui(&mut ui, 1035, 976);
    ui.render(Duration::from_millis(50));

    assert!(!ui.webview_pinned(), "webview should be unpinned after second tap");
    assert!(
        !ui.dashboard_state().pinned_ids().contains(&expected_pin_id),
        "dashboard state must no longer contain '{expected_pin_id}'"
    );

    agent
        .set_pinned_tile_ids(ui.dashboard_state().pinned_ids())
        .expect("update pins after unpin");
    let unpinned_conf = vfs
        .borrow_mut()
        .read_file(AgentRuntime::DASHBOARD_CONFIG_PATH)
        .unwrap_or_default();
    assert!(
        !unpinned_conf.lines().any(|line| line == expected_pin_id),
        "/etc/dashboard.conf must not contain '{expected_pin_id}' after unpin"
    );
}
