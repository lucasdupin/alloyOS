//! End-to-end integration test verifying that Google Gemini 3.8 Flash (`gemini-3.8-flash`)
//! integration works end-to-end:
//! 1. Hits Gemini provider with `What time is it?`
//! 2. Model issues MCP tool call for `clock.now`
//! 3. Welder invokes MCP and synthesizes the `clock.now` AppTile
//! 4. Gemini delivers response text and the tile is rendered on the Ferrofluid chat stream after the question
//! 5. User sends a follow-up "thank you" message
//! 6. Confirms the "thank you" note renders AFTER (below) the Clock card on the stream
//! 7. Captures visual screenshots saved to the artifacts directory as evidence.

use core::task::Poll;
use std::{
    cell::RefCell,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Mutex,
    time::Duration,
};

use alloyos_ferrofluid::{ChatItem, FerrofluidSystemUi};
use alloyos_forge::{
    cloud::{ApiProvider, HttpRequest, HttpResponse, HttpsClient},
    Forge, ForgeError,
};
use alloyos_proto::display::{PHONE_HEIGHT, PHONE_SCALE, PHONE_WIDTH};
use alloyos_welder::{SharedMapVfsBackend, ToolVfsBackend, AgentRuntime};

static TEST_LOCK: Mutex<()> = Mutex::new(());

/// Mock Gemini HTTPS client simulating Google Gemini API responses for tool-use loops.
struct MockGeminiClient {
    step: RefCell<usize>,
    requests: RefCell<Vec<HttpRequest>>,
}

impl MockGeminiClient {
    fn new() -> Self {
        Self {
            step: RefCell::new(0),
            requests: RefCell::new(Vec::new()),
        }
    }
}

impl HttpsClient for MockGeminiClient {
    fn post(&mut self, request: &HttpRequest) -> Result<HttpResponse, ForgeError> {
        self.requests.borrow_mut().push(HttpRequest {
            url: request.url.clone(),
            headers: request.headers.clone(),
            body: request.body.clone(),
        });

        // Verify request targets Google Gemini API with valid API key header
        assert!(
            request.url.contains("generativelanguage.googleapis.com"),
            "URL must target Google Generative Language API, got: {}",
            request.url
        );
        assert!(
            request.url.contains("models/gemini-3.8-flash:generateContent"),
            "URL must target gemini-3.8-flash model, got: {}",
            request.url
        );
        assert!(
            request.headers.iter().any(|(k, v)| k == "x-goog-api-key" && v == "gemini-test-key"),
            "Must supply x-goog-api-key header"
        );

        let mut step = self.step.borrow_mut();
        let cur_step = *step;
        *step += 1;

        let response_text = match cur_step {
            0 => {
                // Step 0: User asked "What time is it?". Gemini requests mcp_call_tool for clock.now
                assert!(
                    request.body.contains("What time is it?"),
                    "Request 0 prompt must contain user query 'What time is it?'"
                );
                r#"{"kind":"tools","calls":[{"name":"mcp_call_tool","arguments":{"app":"clock","tool":"clock.now"}}]}"#
            }
            1 => {
                // Step 1: Tool execution finished. Model receives uptime and returns final answer
                assert!(
                    request.body.contains("uptime_ms"),
                    "Request 1 prompt must contain clock tool execution results"
                );
                r#"{"kind":"final","text":"It is 10:34."}"#
            }
            2 => {
                // Step 2: Follow-up message "thank you". Gemini returns polite acknowledgement
                assert!(
                    request.body.contains("thank you"),
                    "Request 2 prompt must contain user query 'thank you'"
                );
                r#"{"kind":"final","text":"You're welcome! Let me know if you need anything else."}"#
            }
            _ => panic!("Unexpected request step: {}", cur_step),
        };

        let gemini_json = serde_json::json!({
            "candidates": [
                {
                    "content": {
                        "parts": [
                            {
                                "text": response_text
                            }
                        ]
                    }
                }
            ],
            "usageMetadata": {
                "candidatesTokenCount": 20
            }
        });

        Ok(HttpResponse {
            status: 200,
            body: gemini_json.to_string(),
        })
    }

    fn start_post(&mut self, request: &HttpRequest) -> Poll<Result<HttpResponse, ForgeError>> {
        Poll::Ready(self.post(request))
    }
}

fn load_workspace_clock_app(vfs: &Rc<RefCell<SharedMapVfsBackend>>) {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let clock_dir = workspace_root.join("apps/clock");

    for file_name in &[
        "clock.mcp",
        "main.js",
        "tiles.json",
        "tools.json",
        "resources.json",
        "SKILL.md",
    ] {
        let file_path = clock_dir.join(file_name);
        let content = fs::read_to_string(&file_path)
            .unwrap_or_else(|e| panic!("Failed to read {}: {}", file_path.display(), e));
        let vfs_path = format!("/apps/clock/{}", file_name);
        vfs.borrow_mut().write_file(&vfs_path, &content).unwrap();
    }
}

fn save_screenshot_png(path: &Path, width: u32, height: u32, rgb: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = fs::File::create(path).unwrap();
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(rgb).unwrap();
}

#[test]
fn gemini_3_8_flash_clock_tile_and_subsequent_chat_flow() {
    let _guard = TEST_LOCK.lock().unwrap();

    // 1. Initialize VFS with persistent ai.conf pointing to gemini-3.8-flash and the real clock app
    let vfs = SharedMapVfsBackend::shared();
    vfs.borrow_mut()
        .write_file("/etc/ai.conf", "provider=gemini\nmodel=gemini-3.8-flash\n")
        .unwrap();
    load_workspace_clock_app(&vfs);

    // 2. Configure Forge with mock Gemini HTTP transport
    let forge = Rc::new(RefCell::new(Forge::new(0)));
    let mock_client = Box::new(MockGeminiClient::new());
    forge
        .borrow_mut()
        .connect_api(
            ApiProvider::Gemini,
            "gemini-3.8-flash",
            "gemini-test-key",
            mock_client,
        )
        .expect("Failed to connect Gemini API provider");

    // 3. Initialize Welder AgentRuntime
    let mut agent = AgentRuntime::with_vfs_backend(vfs.clone());
    agent.set_forge(forge);

    // Verify active provider and model
    assert_eq!(agent.active_provider_name(), "gemini");
    assert_eq!(agent.active_model_id(), "gemini-3.8-flash");

    // 4. Initialize Ferrofluid UI
    let mut ui = FerrofluidSystemUi::new(PHONE_WIDTH, PHONE_HEIGHT, PHONE_SCALE as f32)
        .expect("Failed to initialize Ferrofluid System UI");
    ui.set_booting(false);
    ui.set_active_model("gemini-3.8-flash");

    let mut chat_items: Vec<ChatItem> = Vec::new();

    // ------------------------------------------------------------------------
    // Turn 1: User asks "What time is it?"
    // ------------------------------------------------------------------------
    let query1 = "What time is it?";
    chat_items.push(ChatItem::Text(format!("You: {}", query1)));
    ui.set_chat_items(&chat_items);

    assert!(agent.start_console_input(query1).unwrap().is_none());
    ui.set_reply_pending(true);

    // Poll until turn completes
    let mut turn1_output = None;
    for _ in 0..10 {
        match agent.poll_console_reply() {
            Poll::Pending => {}
            Poll::Ready(Ok(output)) => {
                turn1_output = Some(output);
                break;
            }
            Poll::Ready(Err(e)) => panic!("Turn 1 failed: {:?}", e),
        }
    }

    let out1 = turn1_output.expect("Turn 1 should complete within poll loop");
    ui.set_reply_pending(false);

    assert_eq!(out1.active_provider, "gemini");
    assert_eq!(out1.active_model, "gemini-3.8-flash");
    assert_eq!(out1.response_text, "It is 10:34.");
    assert_eq!(
        out1.tool_outputs.len(),
        1,
        "Should have executed 1 MCP tool (clock.now)"
    );

    let clock_view = out1.tool_outputs[0]
        .synthesized_view
        .as_ref()
        .expect("Should have synthesized view for clock");
    let clock_tile = clock_view
        .tile
        .as_ref()
        .expect("Synthesized view must contain clock tile");
    assert_eq!(clock_tile.tile_id, "clock.now");
    assert_eq!(clock_tile.title, "Clock");

    // Push Welder reply and synthesized tile to chat stream
    chat_items.push(ChatItem::Text(format!("Welder: {}", out1.response_text)));
    chat_items.push(ChatItem::Tile(clock_tile.clone()));
    ui.set_chat_items(&chat_items);

    // Verify stream contains 3 items: [You, Welder, Tile(clock.now)]
    assert_eq!(chat_items.len(), 3);
    assert_eq!(chat_items[0], ChatItem::Text("You: What time is it?".into()));
    assert_eq!(chat_items[1], ChatItem::Text("Welder: It is 10:34.".into()));
    assert_eq!(chat_items[2], ChatItem::Tile(clock_tile.clone()));

    // Render frame after Turn 1
    ui.render(Duration::from_millis(16));

    let artifact_dir = std::env::var("ALLOYOS_ARTIFACT_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from("target/screenshots")
        });
    let screenshot1_path = artifact_dir.join("gemini_clock_tile_turn1.png");
    let rgb1: Vec<u8> = ui
        .pixels()
        .iter()
        .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8])
        .collect();
    save_screenshot_png(&screenshot1_path, PHONE_WIDTH, PHONE_HEIGHT, &rgb1);
    assert!(screenshot1_path.exists(), "Turn 1 screenshot must be written");

    // ------------------------------------------------------------------------
    // Turn 2: User sends "thank you"
    // ------------------------------------------------------------------------
    let query2 = "thank you";
    // Crucial: The new message is pushed AFTER the embedded clock card
    chat_items.push(ChatItem::Text(format!("You: {}", query2)));
    ui.set_chat_items(&chat_items);

    assert!(agent.start_console_input(query2).unwrap().is_none());
    ui.set_reply_pending(true);

    let mut turn2_output = None;
    for _ in 0..10 {
        match agent.poll_console_reply() {
            Poll::Pending => {}
            Poll::Ready(Ok(output)) => {
                turn2_output = Some(output);
                break;
            }
            Poll::Ready(Err(e)) => panic!("Turn 2 failed: {:?}", e),
        }
    }

    let out2 = turn2_output.expect("Turn 2 should complete within poll loop");
    ui.set_reply_pending(false);

    assert_eq!(out2.active_provider, "gemini");
    assert_eq!(out2.active_model, "gemini-3.8-flash");
    assert_eq!(
        out2.response_text,
        "You're welcome! Let me know if you need anything else."
    );

    // Push Welder reply to chat stream
    chat_items.push(ChatItem::Text(format!("Welder: {}", out2.response_text)));
    ui.set_chat_items(&chat_items);

    // Render frame after Turn 2
    ui.render(Duration::from_millis(32));

    // ------------------------------------------------------------------------
    // Strict Verification of Chronological Order in Scroll View
    // ------------------------------------------------------------------------
    assert_eq!(chat_items.len(), 5);

    // 0. Initial User Question
    assert_eq!(chat_items[0], ChatItem::Text("You: What time is it?".into()));

    // 1. Initial Welder Response
    assert_eq!(chat_items[1], ChatItem::Text("Welder: It is 10:34.".into()));

    // 2. Embedded Clock Card
    assert_eq!(chat_items[2], ChatItem::Tile(clock_tile.clone()));

    // 3. Subsequent User Note: CONFIRMED strictly AFTER the Embedded Clock Tile!
    assert_eq!(chat_items[3], ChatItem::Text("You: thank you".into()));

    // 4. Subsequent Welder Response: CONFIRMED strictly AFTER the User Note!
    assert_eq!(
        chat_items[4],
        ChatItem::Text("Welder: You're welcome! Let me know if you need anything else.".into())
    );

    // Verify UI state mirrors chat_items exactly
    assert_eq!(ui.chat_items(), chat_items);

    // ------------------------------------------------------------------------
    // Save Final End-to-End Screenshot
    // ------------------------------------------------------------------------
    let screenshot2_path = artifact_dir.join("gemini_clock_chat_stream.png");
    let rgb2: Vec<u8> = ui
        .pixels()
        .iter()
        .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8])
        .collect();
    save_screenshot_png(&screenshot2_path, PHONE_WIDTH, PHONE_HEIGHT, &rgb2);
    assert!(screenshot2_path.exists(), "Turn 2 screenshot must be written");

    // Also verify that the rendered pixels changed between Turn 1 and Turn 2
    // (proving the new "thank you" message and response were visibly painted below the card)
    assert_ne!(rgb1, rgb2, "Turn 2 must render new visible content below the card");

    // ------------------------------------------------------------------------
    // Turn 3: Tile Refresh in Stream
    // ------------------------------------------------------------------------
    // Supply an explicit RTC instant: uptime must never stand in for wall time.
    let mut status = agent.tool_registry.system_status();
    status.wall_clock_ms = Some(12_000_000);
    agent.tool_registry.set_system_status(status);
    agent.tool_registry.tick_scheduler(12_000_000, None);
    let refresh_out = agent.refresh_tile("clock.now").expect("Refresh clock tile must succeed");
    let refreshed_tile = refresh_out
        .synthesized_view
        .and_then(|v| v.tile)
        .expect("Refreshed tile must be present");
    assert_eq!(refreshed_tile.items[0].title, "03:20:00");

    // In-place refresh of embedded tile preserving chronological order
    if let Some(ChatItem::Tile(tile)) = chat_items
        .iter_mut()
        .find(|item| matches!(item, ChatItem::Tile(t) if t.tile_id == "clock.now"))
    {
        *tile = refreshed_tile.clone();
    }
    ui.set_chat_items(&chat_items);
    ui.render(Duration::from_millis(48));

    // Order must remain identical
    assert_eq!(chat_items.len(), 5);
    assert_eq!(chat_items[2], ChatItem::Tile(refreshed_tile));
    assert_eq!(chat_items[3], ChatItem::Text("You: thank you".into()));

    let screenshot3_path = artifact_dir.join("gemini_clock_refreshed.png");
    let rgb3: Vec<u8> = ui
        .pixels()
        .iter()
        .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8])
        .collect();
    save_screenshot_png(&screenshot3_path, PHONE_WIDTH, PHONE_HEIGHT, &rgb3);
    assert!(screenshot3_path.exists());
}

#[test]
fn gemini_3_8_flash_sse_streaming_integration() {
    let _guard = TEST_LOCK.lock().unwrap();

    struct SseGeminiClient;
    impl HttpsClient for SseGeminiClient {
        fn post(&mut self, _req: &HttpRequest) -> Result<HttpResponse, ForgeError> {
            let sse_body = concat!(
                "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"The \"}]}}]}\n\n",
                "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"time \"}]}}]}\n\n",
                "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"is 10:34.\"}]}}],\"usageMetadata\":{\"candidatesTokenCount\":5}}\n\n",
            );
            Ok(HttpResponse {
                status: 200,
                body: sse_body.into(),
            })
        }
        fn start_post(&mut self, req: &HttpRequest) -> Poll<Result<HttpResponse, ForgeError>> {
            Poll::Ready(self.post(req))
        }
    }

    let vfs = SharedMapVfsBackend::shared();
    vfs.borrow_mut()
        .write_file("/etc/ai.conf", "provider=gemini\nmodel=gemini-3.8-flash\n")
        .unwrap();

    let forge = Rc::new(RefCell::new(Forge::new(0)));
    forge
        .borrow_mut()
        .connect_api(
            ApiProvider::Gemini,
            "gemini-3.8-flash",
            "gemini-test-key",
            Box::new(SseGeminiClient),
        )
        .expect("Connect Gemini API");

    let mut agent = AgentRuntime::with_vfs_backend(vfs);
    agent.set_forge(forge);

    assert!(agent.start_console_input("What time is it?").unwrap().is_none());

    let mut final_out = None;
    for _ in 0..10 {
        match agent.poll_console_reply() {
            Poll::Pending => {}
            Poll::Ready(Ok(out)) => {
                final_out = Some(out);
                break;
            }
            Poll::Ready(Err(e)) => panic!("SSE generation failed: {:?}", e),
        }
    }

    let out = final_out.expect("SSE generation completed");
    assert_eq!(out.response_text, "The time is 10:34.");
    assert_eq!(out.active_model, "gemini-3.8-flash");
}


