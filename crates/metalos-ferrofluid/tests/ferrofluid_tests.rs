use metalos_ferrofluid::{
    draw_char, draw_markdown, draw_str, AgentUiToken, AiConsoleView, CallerPrincipal,
    FerrofluidError, FerrofluidFramebuffer, FerrofluidIpcCommand, FerrofluidServer,
    FerrofluidWidget, ACCENT_LIQUID_SILVER, BG_OBSIDIAN, BORDER_CHROME, SURFACE_CARBON,
    TEXT_MERCURY_BRIGHT,
};
use metalos_proto::{DirtyRect, InputEvent, InputEventKind};

#[test]
fn phone_widgets_stack_at_full_width() {
    let mut server = FerrofluidServer::new(480, 864);
    let token = server.agent_token();
    let agent = server.agent_principal();
    let card = FerrofluidWidget::MetricCard {
        title: "CPU".into(),
        value: "12".into(),
        unit: "%".into(),
        subtitle: "Usage".into(),
    };
    server.submit_widgets(&agent, Some(&token), vec![card.clone(), card]).unwrap();
    let surface = &server.surfaces[0];
    let deck_y = surface.y + surface.height * 3 / 5 + 4;
    let slot_h = (surface.height - surface.height * 3 / 5 - 4) / 2;
    // Both cards span the phone width and occupy separate vertical rows.
    for y in [deck_y, deck_y + slot_h] {
        assert_eq!(server.framebuffer.get_back_pixel(surface.x, y), Some(BORDER_CHROME));
        assert_eq!(server.framebuffer.get_back_pixel(surface.x + surface.width - 1, y), Some(BORDER_CHROME));
    }
}

#[test]
fn double_buffered_rendering_dirty_rect_coalescing_and_vsync() {
    let mut fb = FerrofluidFramebuffer::new(320, 200);
    assert_eq!(fb.vsync_count(), 0);
    assert!(fb.dirty_rects.is_empty());

    // Two disjoint rectangles remain separate
    fb.fill_rect(10, 10, 20, 20, BORDER_CHROME);
    fb.fill_rect(100, 100, 30, 30, TEXT_MERCURY_BRIGHT);
    assert_eq!(fb.dirty_rects.len(), 2);

    // Back buffer has new pixels, front buffer still has initial BG_OBSIDIAN before present()
    assert_eq!(fb.get_back_pixel(15, 15), Some(BORDER_CHROME));
    assert_eq!(fb.get_front_pixel(15, 15), Some(BG_OBSIDIAN));

    // Overlapping or touching rectangle coalesces with the first dirty rect
    fb.fill_rect(25, 25, 20, 20, ACCENT_LIQUID_SILVER);
    assert_eq!(fb.dirty_rects.len(), 2);
    assert!(fb.dirty_rects.contains(&DirtyRect::new(10, 10, 35, 35)));

    // Present flips dirty regions to front buffer and increments vsync counter
    let presented = fb.present();
    assert_eq!(presented.len(), 2);
    assert_eq!(fb.vsync_count(), 1);
    assert!(fb.dirty_rects.is_empty());
    assert_eq!(fb.get_front_pixel(15, 15), Some(BORDER_CHROME));
    assert_eq!(fb.get_front_pixel(30, 30), Some(ACCENT_LIQUID_SILVER));
    assert_eq!(fb.get_front_pixel(110, 110), Some(TEXT_MERCURY_BRIGHT));
}

#[test]
fn security_gate_forbids_developer_mcp_apps_and_allows_ai_agent() {
    let mut server = FerrofluidServer::new(640, 400);
    let dev_app = CallerPrincipal::DeveloperMcpApp {
        app_id: "com.acme.weather-mcp".to_string(),
    };
    let unauth = CallerPrincipal::Unauthenticated;
    let fake_token = AgentUiToken::new(999, 999);

    // 1. Developer MCP app attempting to create a window/surface is strictly forbidden
    let res_surface = server.create_surface(&dev_app, Some(&fake_token), "Weather Popup", 200, 150);
    assert_eq!(res_surface, Err(FerrofluidError::UiCreationForbiddenForApp));

    // 2. Developer MCP app attempting to submit or mutate widgets is strictly forbidden
    let metric = FerrofluidWidget::MetricCard {
        title: "CPU".to_string(),
        value: "12".to_string(),
        unit: "%".to_string(),
        subtitle: "4 cores".to_string(),
    };
    assert_eq!(
        server.submit_widgets(&dev_app, None, vec![metric.clone()]),
        Err(FerrofluidError::UiCreationForbiddenForApp)
    );
    assert_eq!(
        server.mutate_widget(&dev_app, None, 0, metric.clone()),
        Err(FerrofluidError::UiCreationForbiddenForApp)
    );
    assert_eq!(
        server.render_widget(&dev_app, None, &metric, 10, 10, 100, 50),
        Err(FerrofluidError::UiCreationForbiddenForApp)
    );

    // 3. Developer MCP app attempting to send IPC UI commands is strictly forbidden
    assert_eq!(
        server.submit_ipc_command(
            &dev_app,
            Some(&server.agent_token()),
            FerrofluidIpcCommand::StreamChatToken {
                token: "hijack".to_string()
            }
        ),
        Err(FerrofluidError::UiCreationForbiddenForApp)
    );

    // 4. Unauthenticated caller is also rejected with UiCreationForbiddenForApp
    assert_eq!(
        server.create_surface(&unauth, None, "Unauth Window", 100, 100),
        Err(FerrofluidError::UiCreationForbiddenForApp)
    );

    // 5. AI Agent with wrong token is rejected with InvalidAgentToken
    let agent_principal = server.agent_principal();
    assert_eq!(
        server.create_surface(&agent_principal, Some(&fake_token), "Bad Token", 100, 100),
        Err(FerrofluidError::InvalidAgentToken)
    );

    // 6. Authenticated AI Agent with valid AgentUiToken succeeds
    let valid_token = server.agent_token();
    let surface_id = server
        .create_surface(
            &agent_principal,
            Some(&valid_token),
            "Agent Synthesized Surface",
            320,
            200,
        )
        .expect("AI agent should be allowed to create surfaces");
    assert_eq!(surface_id, 2);

    let dirty = server
        .submit_widgets(&agent_principal, Some(&valid_token), vec![metric])
        .expect("AI agent should be allowed to submit generative UI widgets");
    assert!(dirty.width > 0 && dirty.height > 0);
}

#[test]
fn monospace_font_and_markdown_code_block_pixel_rendering() {
    let mut fb = FerrofluidFramebuffer::new(400, 200);
    let char_rect = draw_char(&mut fb, 8, 8, 'M', TEXT_MERCURY_BRIGHT);
    assert_eq!(char_rect, DirtyRect::new(8, 8, 8, 8));

    // Verify bright mercury stroke pixels and anti-aliased edge tint pixels were written
    let mut bright_pixels = 0;
    let mut tinted_pixels = 0;
    for y in 8..16 {
        for x in 8..16 {
            let px = fb.get_back_pixel(x, y).unwrap();
            if px == TEXT_MERCURY_BRIGHT {
                bright_pixels += 1;
            } else if px != BG_OBSIDIAN {
                tinted_pixels += 1;
            }
        }
    }
    assert!(bright_pixels > 10, "Expected glyph stroke pixels for 'M'");
    assert!(
        tinted_pixels > 0,
        "Expected anti-aliased liquid-silver edge tint pixels"
    );

    // Render markdown with **bold**, `code`, and fenced ``` blocks
    fb.clear(BG_OBSIDIAN);
    fb.present();

    let md = "**Bold Title**\nInline `mcp_tool` call\n```rust\nlet x = 42;\n```";
    let md_rect = draw_markdown(&mut fb, 10, 10, 300, md, 0, 10);
    assert!(md_rect.width > 0 && md_rect.height >= 30);

    // Fenced code block row (line index 3 -> visible row 3 at y = 10 + 3 * 10 = 40)
    // has a 2px ACCENT_LIQUID_SILVER bar at x=10..12 and SURFACE_CARBON fill
    assert_eq!(fb.get_back_pixel(10, 42), Some(ACCENT_LIQUID_SILVER));
    assert_eq!(fb.get_back_pixel(250, 42), Some(SURFACE_CARBON));
}

#[test]
fn generative_ui_widgets_pixel_output_and_single_window_boot() {
    let mut server = FerrofluidServer::new(640, 480);

    // Initial boot presents the single centered/fullscreen Ferrofluid AI Console window
    let boot_dirty = server.present();
    assert!(!boot_dirty.is_empty());
    assert_eq!(server.framebuffer.vsync_count(), 1);
    assert_eq!(server.surfaces.len(), 1);
    assert_eq!(server.surfaces[0].title, "Ferrofluid AI Console");

    let agent = server.agent_principal();
    let token = server.agent_token();

    // Synthesize all 5 generative UI widget variants
    let widgets = vec![
        FerrofluidWidget::MetricCard {
            title: "LATENCY".to_string(),
            value: "1.4".to_string(),
            unit: "ms".to_string(),
            subtitle: "VirtIO-Net".to_string(),
        },
        FerrofluidWidget::DataTable {
            headers: vec!["PID".to_string(), "TASK".to_string()],
            rows: vec![
                vec!["1".to_string(), "ferrofluidd".to_string()],
                vec!["2".to_string(), "agentd".to_string()],
            ],
        },
        FerrofluidWidget::Sparkline {
            label: "TOK/S".to_string(),
            samples: vec![12, 28, 45, 60, 52, 80, 95],
        },
        FerrofluidWidget::StatusBanner {
            status: "OPTIMAL".to_string(),
            active_mcp_apps: 4,
            scheduled_scripts: 2,
        },
        FerrofluidWidget::AiConsoleView(AiConsoleView::new("Claude")),
    ];

    server
        .submit_widgets(&agent, Some(&token), widgets)
        .expect("Widget synthesis should succeed for AI agent");

    // Stream tokens and dispatch keyboard input
    server
        .stream_chat_token(&agent, Some(&token), "Synthesizing **telemetry** view...\n")
        .unwrap();
    server.handle_input(InputEvent {
        kind: InputEventKind::KeyDown,
        code: b'h' as u16,
        x: 0,
        y: 0,
    });
    server.handle_input(InputEvent {
        kind: InputEventKind::KeyDown,
        code: b'i' as u16,
        x: 0,
        y: 0,
    });
    assert_eq!(server.console.input_prompt, "hi");

    let dirty_after = server.present();
    assert!(!dirty_after.is_empty());
    assert_eq!(server.framebuffer.vsync_count(), 2);

    // Verify front buffer contains non-obsidian rendered chrome, carbon surface, and bright text pixels
    let has_chrome = server.framebuffer.front_buffer.contains(&BORDER_CHROME);
    let has_carbon = server.framebuffer.front_buffer.contains(&SURFACE_CARBON);
    let has_bright_text = server
        .framebuffer
        .front_buffer
        .contains(&TEXT_MERCURY_BRIGHT);
    let has_liquid_silver = server
        .framebuffer
        .front_buffer
        .contains(&ACCENT_LIQUID_SILVER);

    assert!(has_chrome, "Expected BORDER_CHROME pixels in front buffer");
    assert!(has_carbon, "Expected SURFACE_CARBON pixels in front buffer");
    assert!(
        has_bright_text,
        "Expected TEXT_MERCURY_BRIGHT pixels in front buffer"
    );
    assert!(
        has_liquid_silver,
        "Expected ACCENT_LIQUID_SILVER pixels in front buffer"
    );

    // Also test standalone draw_str helper
    let mut scratch = FerrofluidFramebuffer::new(128, 32);
    let r = draw_str(&mut scratch, 4, 4, "FERROFLUID", TEXT_MERCURY_BRIGHT);
    assert_eq!(r.width, 10 * 8);
    assert_eq!(r.height, 8);
}
