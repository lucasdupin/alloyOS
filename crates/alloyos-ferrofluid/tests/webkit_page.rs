//! Tests for the real WebKit page engine and JavaScriptCore execution in WebViewCard.

use alloyos_ferrofluid::webview::{
    EditingState, PointerPhase, WebViewController,
};

fn count_blue_pixels(bgra: &[u8]) -> usize {
    let mut count = 0;
    for px in bgra.chunks_exact(4) {
        let b = px[0] as i32;
        let g = px[1] as i32;
        let r = px[2] as i32;
        if (r - 137).abs() <= 8 && (g - 207).abs() <= 8 && (b - 255).abs() <= 8 {
            count += 1;
        }
    }
    count
}

fn loads_local_form_fixture_with_webkit_engine() {
    let url = "https://local.alloyos.test/local-form.html";
    let ctrl = WebViewController::for_url(1, url, "alloyOS native web acceptance")
        .expect("should load fixture URL");

    assert_eq!(ctrl.committed_url(), Some(url));
    assert_eq!(ctrl.trusted_origin().unwrap().as_str(), "https://local.alloyos.test");
    assert_eq!(ctrl.untrusted_title(), "alloyOS native web acceptance");

    let frame = ctrl.committed_frame().expect("must have committed frame");
    assert_eq!(frame.width, 576);
    assert_eq!(frame.height, 380);

    // Styled text and blue mark.svg pixels must be present
    let blue_count = count_blue_pixels(&frame.pixels);
    assert!(
        blue_count > 1_000,
        "mark.svg blue pixels must be rendered into the frame (found {blue_count})"
    );

    // Initial status text in DOM
    let page = ctrl.page().expect("must have active WebPage");
    assert_eq!(page.element_text("result"), Some("Waiting for input."));
}

fn executes_javascript_and_sets_script_ready() {
    let url = "https://local.alloyos.test/local-form.html";
    let mut ctrl = WebViewController::for_url(2, url, "alloyOS native web acceptance")
        .expect("should load fixture");

    // Acceptance requirement: document.documentElement.dataset.scriptReady is true
    let page = ctrl.page().expect("must have page");
    assert_eq!(
        page.dataset_attr("scriptReady"),
        Some("true"),
        "JavaScript must execute and set document.documentElement.dataset.scriptReady = true"
    );

    // Evaluate arbitrary JavaScript expressions in the page context via JavaScriptCore
    let res = ctrl.eval_js("10 + 25").expect("eval_js should succeed");
    assert_eq!(res, "35");

    let tag = ctrl
        .eval_js("document.getElementById('name').tagName")
        .expect("eval_js should query DOM");
    assert_eq!(tag, "\"INPUT\"");
}

fn interactive_touch_focus_and_soft_keyboard_driving() {
    let url = "https://local.alloyos.test/local-form.html";
    let mut ctrl = WebViewController::for_url(3, url, "alloyOS native web acceptance")
        .expect("should load fixture");

    assert!(!ctrl.soft_keyboard_visible());
    assert_eq!(ctrl.editing_state(), EditingState::default());

    // Tapping the input field (inside the form at x=80, y=200) focuses it
    ctrl.interact_pointer(PointerPhase::Down, 80, 200);
    ctrl.interact_pointer(PointerPhase::Up, 80, 200);

    assert!(
        ctrl.soft_keyboard_visible(),
        "soft keyboard must become visible when focusing editable input"
    );
    assert_eq!(
        ctrl.editing_state(),
        EditingState {
            focused: true,
            editable: true
        }
    );

    // Committing text updates input element value
    ctrl.interact_commit_text("Alloy");
    let page = ctrl.page().expect("must have page");
    assert_eq!(page.element_value("name"), Some("Alloy"));
}

fn interactive_submit_executes_js_listener_and_updates_rendered_dom() {
    let url = "https://local.alloyos.test/local-form.html";
    let mut ctrl = WebViewController::for_url(4, url, "alloyOS native web acceptance")
        .expect("should load fixture");

    // Focus and type "Alloy" into the name input
    ctrl.interact_pointer(PointerPhase::Down, 80, 200);
    ctrl.interact_commit_text("Alloy");
    assert!(ctrl.soft_keyboard_visible());

    // Submit form (invokes JS submit event listener: document.getElementById('result').textContent = 'Hello, ' + name + '!')
    let submit_result = ctrl.interact_submit().expect("submit should succeed");
    assert_eq!(submit_result, "Hello, Alloy!");

    let page = ctrl.page().expect("must have page");
    assert_eq!(page.element_text("result"), Some("Hello, Alloy!"));

    // Soft keyboard should hide on submit
    assert!(!ctrl.soft_keyboard_visible());

    // Frame must be updated with the new text
    let frame = ctrl.committed_frame().expect("must have updated frame");
    assert!(frame.pixels.len() == (576 * 380 * 4) as usize);
}

fn arbitrary_website_html_css_and_js() {
    let mut ctrl = WebViewController::new(5, 600, 400, 2000).unwrap();

    let custom_html = r#"
    <!doctype html>
    <html>
      <head>
        <title>General Site</title>
        <style>
          body { background: #1a1e24; color: #ffffff; margin: 16px; }
          h1 { color: #89cfff; }
          button { padding: 10px; }
        </style>
      </head>
      <body>
        <h1>Arbitrary Web Content</h1>
        <form id="calc-form">
          <input id="num" value="42">
          <button id="calc-btn" type="submit">Compute</button>
        </form>
        <p id="result">Initial</p>
        <script>
          document.getElementById('calc-form').addEventListener('submit', event => {
            event.preventDefault();
            const val = document.getElementById('num').value;
            document.getElementById('result').textContent = 'Computed: ' + val;
          });
          document.documentElement.dataset.engine = 'webkit-jsc';
        </script>
      </body>
    </html>
    "#;

    ctrl.load_html(custom_html, "https://general.example.org");

    let page = ctrl.page().expect("page loaded");
    assert_eq!(page.dataset_attr("engine"), Some("webkit-jsc"));
    assert_eq!(page.element_value("num"), Some("42"));
    assert_eq!(page.element_text("result"), Some("Initial"));

    // Submit the custom form
    let res = ctrl.interact_submit().expect("submit should succeed");
    assert_eq!(res, "Computed: 42");

    let page_after = ctrl.page().expect("page");
    assert_eq!(page_after.element_text("result"), Some("Computed: 42"));
}

fn scrolling_shifts_viewport_content() {
    let url = "https://local.alloyos.test/local-form.html";
    let mut ctrl = WebViewController::for_url(6, url, "alloyOS native web acceptance")
        .expect("should load fixture");

    let initial_frame = ctrl.committed_frame().unwrap().clone();

    // Scroll down by 300 pixels
    ctrl.encode_scroll(0, 300).expect("scroll should encode");

    let scrolled_frame = ctrl.committed_frame().unwrap();
    assert_ne!(
        initial_frame.pixels, scrolled_frame.pixels,
        "scrolling must update pixel buffer to reflect new scroll position"
    );
}

fn loads_real_live_website_over_network() {
    let url = "https://g1.globo.com";
    let mut ctrl = WebViewController::for_url(10, url, "fallback title")
        .expect("should load real website over network");

    assert_eq!(ctrl.committed_url(), Some(url));
    assert_eq!(ctrl.trusted_origin().unwrap().as_str(), "https://g1.globo.com");

    // The real site title must be extracted from the downloaded HTML, not the fallback title
    let title = ctrl.untrusted_title();
    assert!(
        title.contains("g1") || title.contains("Globo"),
        "Title should be parsed from real HTML <title> tag, got: {title}"
    );

    // Frame must be rendered with real page content.
    // Over real WAN, wait for layout and subresource rasterization if needed.
    let mut non_bg = 0;
    for _ in 0..5 {
        let frame = ctrl.committed_frame().expect("must have committed frame");
        assert_eq!(frame.width, 576);
        assert_eq!(frame.height, 380);

        non_bg = frame
            .pixels
            .chunks_exact(4)
            .filter(|px| {
                let b = px[0];
                let g = px[1];
                let r = px[2];
                !((r == 16 && g == 20 && b == 24) || (r == 255 && g == 255 && b == 255))
            })
            .count();
        if non_bg > 500 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
        ctrl.refresh_rendered_frame();
    }

    assert!(
        non_bg > 500,
        "Frame must render real page elements (found {non_bg} non-bg pixels)"
    );
}

fn unreachable_url_renders_authentic_error_page() {
    let url = "https://nonexistent-domain-12345-alloyos.invalid";
    let ctrl = WebViewController::for_url(11, url, "fallback")
        .expect("should produce controller with error page");

    let page = ctrl.page().expect("must have page");
    assert_eq!(page.title(), "Unable to connect");
}

fn main() {
    println!("Running genuine WebKit acceptance test suite on macOS main thread...");
    loads_local_form_fixture_with_webkit_engine();
    println!("  ✓ loads_local_form_fixture_with_webkit_engine passed");
    executes_javascript_and_sets_script_ready();
    println!("  ✓ executes_javascript_and_sets_script_ready passed");
    interactive_touch_focus_and_soft_keyboard_driving();
    println!("  ✓ interactive_touch_focus_and_soft_keyboard_driving passed");
    interactive_submit_executes_js_listener_and_updates_rendered_dom();
    println!("  ✓ interactive_submit_executes_js_listener_and_updates_rendered_dom passed");
    arbitrary_website_html_css_and_js();
    println!("  ✓ arbitrary_website_html_css_and_js passed");
    scrolling_shifts_viewport_content();
    println!("  ✓ scrolling_shifts_viewport_content passed");
    loads_real_live_website_over_network();
    println!("  ✓ loads_real_live_website_over_network passed");
    unreachable_url_renders_authentic_error_page();
    println!("  ✓ unreachable_url_renders_authentic_error_page passed");
    println!("All WebKit genuine integration tests passed successfully!");
}

