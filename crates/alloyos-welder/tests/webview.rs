use alloyos_welder::webview::{
    WebContentCapabilities, WebViewBroker, WebViewCaller, WebViewCommand, WebViewError,
    LOCAL_FORM_FIXTURE_PATH, LOCAL_FORM_FIXTURE_URL,
};

#[test]
fn enforces_strict_welder_and_ferrofluid_only_access_boundary() {
    let mut broker = WebViewBroker::new();

    // Headless apps (anonymous or by app ID) must never open a WebView.
    assert_eq!(
        broker
            .open_session(&WebViewCaller::App, 1280, 800, 2000)
            .unwrap_err(),
        WebViewError::AppAccessForbidden
    );
    let malicious_app = WebViewCaller::app("app.notes.headless");
    assert_eq!(
        broker
            .open_session(&malicious_app, 1280, 800, 2000)
            .unwrap_err(),
        WebViewError::AppAccessForbidden
    );

    // Welder and Ferrofluid are authorized to open and manage sessions.
    let mut session = broker
        .open_session(&WebViewCaller::Welder, 1280, 800, 2000)
        .expect("Welder should open WebViewSession");
    assert_eq!(session.view_id(), 1);
    assert_eq!(session.generation(), 1);

    // Apps cannot hijack an existing session to navigate, inject markup/script, or push frames.
    assert_eq!(
        broker.navigate(&WebViewCaller::App, &mut session, "https://example.org"),
        Err(WebViewError::AppAccessForbidden)
    );
    assert_eq!(
        broker.navigate(&malicious_app, &mut session, "https://example.org"),
        Err(WebViewError::AppAccessForbidden)
    );
    assert_eq!(
        broker.supply_markup_or_script(
            &malicious_app,
            &mut session,
            "<script>fetch('http://127.0.0.1')</script>"
        ),
        Err(WebViewError::AppAccessForbidden)
    );
    assert_eq!(
        broker.app_push_frame(&malicious_app, &[0u8; 64]),
        Err(WebViewError::AppAccessForbidden)
    );
    assert_eq!(
        broker.focus_editable(&malicious_app, &mut session, true, true),
        Err(WebViewError::AppAccessForbidden)
    );
    assert_eq!(
        broker.commit_text(&malicious_app, &mut session, "injected"),
        Err(WebViewError::AppAccessForbidden)
    );
    assert_eq!(
        broker.submit_form(&malicious_app, &mut session),
        Err(WebViewError::AppAccessForbidden)
    );
}

#[test]
fn web_content_receives_zero_implicit_host_or_agent_capabilities() {
    let mut broker = WebViewBroker::new();
    let session = broker
        .open_session(&WebViewCaller::Ferrofluid, 1024, 768, 1000)
        .unwrap();

    assert_eq!(session.capabilities(), WebContentCapabilities::NONE);
    assert!(!session.capabilities().has_any_privilege());

    for forbidden_cap in [
        "mcp.tools.list",
        "mcp.tools.call",
        "knowledge_search",
        "knowledge_upsert",
        "fs_read",
        "fs_write",
        "oauth.token",
        "agent.credentials",
    ] {
        assert_eq!(
            broker.request_host_capability_from_web_content(&session, forbidden_cap),
            Err(WebViewError::CapabilityDenied),
            "web content must be denied capability {forbidden_cap}"
        );
    }
}

#[test]
fn executes_local_acceptance_fixture_form_flow_via_welder_and_ferrofluid() {
    assert_eq!(LOCAL_FORM_FIXTURE_PATH, "tests/fixtures/web/local-form.html");

    let mut broker = WebViewBroker::new();
    let mut session = broker
        .open_session(&WebViewCaller::Welder, 800, 600, 2000)
        .unwrap();

    broker
        .load_local_form_fixture(&WebViewCaller::Welder, &mut session)
        .unwrap();
    assert_eq!(session.committed_url(), Some(LOCAL_FORM_FIXTURE_URL));
    assert_eq!(session.trusted_origin(), Some("https://fixture.alloyos.local"));
    assert_eq!(session.untrusted_title(), "alloyOS Local Acceptance Form");
    assert!(!session.soft_keyboard_visible());

    // Ferrofluid focuses the editable <input id="q"> element, showing the soft keyboard.
    broker
        .focus_editable(&WebViewCaller::Ferrofluid, &mut session, true, true)
        .unwrap();
    assert!(session.soft_keyboard_visible());

    // Ferrofluid commits text from the soft keyboard and submits the form.
    broker
        .commit_text(&WebViewCaller::Ferrofluid, &mut session, "  ferrofluid webview  ")
        .unwrap();
    let output = broker
        .submit_form(&WebViewCaller::Ferrofluid, &mut session)
        .unwrap();
    assert_eq!(output, "submitted:ferrofluid webview");
    assert_eq!(session.submitted_output(), Some("submitted:ferrofluid webview"));

    // Leaving the editable field hides the soft keyboard.
    broker
        .focus_editable(&WebViewCaller::Ferrofluid, &mut session, false, false)
        .unwrap();
    assert!(!session.soft_keyboard_visible());
}

#[test]
fn enforces_https_only_navigation_and_prevents_origin_spoofing() {
    let mut broker = WebViewBroker::new();
    let mut session = broker
        .open_session(&WebViewCaller::Welder, 1200, 900, 2000)
        .unwrap();

    for unsafe_url in [
        "http://example.org/insecure",
        "file:///Users/secret/local-form.html",
        "javascript:alert(1)",
        "data:text/html,<h1>spoof</h1>",
        "https://user:pass@example.org/login",
        "https://trusted.example.org@evil.example/phish",
        "https://example.org:0/badport",
        "https://example.org:99999/badport",
        "/relative/path",
    ] {
        assert_eq!(
            broker.navigate(&WebViewCaller::Welder, &mut session, unsafe_url),
            Err(WebViewError::InvalidUrl),
            "expected {unsafe_url} to be rejected"
        );
    }

    // Requesting a navigation does not immediately update the trusted origin.
    broker
        .navigate(
            &WebViewCaller::Welder,
            &mut session,
            "https://Docs.Example.ORG:8443/guide?page=1#intro",
        )
        .unwrap();
    assert!(session.trusted_origin().is_none());

    // Only the browser's committed URL updates the trusted origin.
    let gen = session.generation();
    broker
        .commit_browser_url(
            &mut session,
            gen,
            "https://Docs.Example.ORG:8443/guide?page=1#intro",
        )
        .unwrap();
    assert_eq!(session.trusted_origin(), Some("https://docs.example.org:8443"));

    // Untrusted page title containing another origin never alters trusted_origin.
    broker
        .receive_untrusted_title(&mut session, gen, "https://bank.example.com")
        .unwrap();
    assert_eq!(session.untrusted_title(), "https://bank.example.com");
    assert_eq!(session.trusted_origin(), Some("https://docs.example.org:8443"));
}

#[test]
fn crash_advances_generation_rejects_stale_replies_and_recovers_on_reload() {
    let mut broker = WebViewBroker::new();
    let mut session = broker
        .open_session(&WebViewCaller::Ferrofluid, 1024, 768, 2000)
        .unwrap();
    broker
        .navigate(
            &WebViewCaller::Ferrofluid,
            &mut session,
            "https://portal.example.org/app",
        )
        .unwrap();
    let old_gen = session.generation();
    broker
        .commit_browser_url(&mut session, old_gen, "https://portal.example.org/app")
        .unwrap();
    broker
        .focus_editable(&WebViewCaller::Ferrofluid, &mut session, true, true)
        .unwrap();
    assert!(session.soft_keyboard_visible());
    let _ = session.take_commands();

    // Simulate browser process crash.
    broker.handle_crash(&mut session, old_gen, 42).unwrap();
    assert!(session.is_crashed());
    assert_eq!(session.crash_reason(), Some(42));
    assert!(!session.soft_keyboard_visible());
    assert_eq!(session.generation(), old_gen + 1);

    // Late replies from the dead generation are deterministically rejected.
    assert_eq!(
        broker.commit_browser_url(&mut session, old_gen, "https://evil.example.org/"),
        Err(WebViewError::StaleGeneration)
    );

    // Explicit recovery re-creates the view under the new generation and reloads the committed URL.
    broker
        .recover_and_reload(&WebViewCaller::Ferrofluid, &mut session)
        .unwrap();
    assert!(!session.is_crashed());
    let commands = session.take_commands();
    assert_eq!(
        commands,
        vec![
            WebViewCommand::Create {
                view_id: session.view_id(),
                generation: old_gen + 1,
                width: 1024,
                height: 768,
                scale_thousandths: 2000,
            },
            WebViewCommand::Navigate {
                view_id: session.view_id(),
                generation: old_gen + 1,
                url: String::from("https://portal.example.org/app"),
            },
        ]
    );
}
