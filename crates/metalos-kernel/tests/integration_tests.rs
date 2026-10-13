use metalos_kernel::syscall::handlers::{sys_close, sys_open, sys_read, sys_write};
use metalos_kernel::task::pcb::{Task, TaskState};
use metalos_kernel::task::scheduler::Scheduler;
use std::sync::{Arc, Mutex};

static VFS_TEST_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn integration_vfs_and_syscall_io() {
    let _guard = VFS_TEST_LOCK.lock().unwrap();
    // Initialize VFS and FD table
    metalos_kernel::fs::init();

    let test_filename = b"/tmp_integration.txt\0";
    let fd = sys_open(test_filename.as_ptr() as u64, 0, 0);
    assert!(fd >= 3, "Opened FD should be >= 3, got: {}", fd);

    let message = b"metalOS integration test message";
    let written = sys_write(fd as u64, message.as_ptr() as u64, message.len() as u64);
    assert_eq!(written, message.len() as i64);

    // Close and reopen to verify persistence in RamFS
    assert_eq!(sys_close(fd as u64), 0);

    let reopened_fd = sys_open(test_filename.as_ptr() as u64, 0, 0);
    assert!(reopened_fd >= 3);

    let mut read_buf = [0u8; 64];
    let read_bytes = sys_read(
        reopened_fd as u64,
        read_buf.as_mut_ptr() as u64,
        read_buf.len() as u64,
    );
    assert_eq!(read_bytes, message.len() as i64);
    assert_eq!(&read_buf[..read_bytes as usize], message);

    assert_eq!(sys_close(reopened_fd as u64), 0);
}

#[test]
fn integration_devfs_devices() {
    let _guard = VFS_TEST_LOCK.lock().unwrap();
    metalos_kernel::fs::init();

    // /dev/zero
    let zero_path = b"/dev/zero\0";
    let fd_zero = sys_open(zero_path.as_ptr() as u64, 0, 0);
    assert!(fd_zero >= 3);

    let mut buf = [0xAAu8; 32];
    let read_zero = sys_read(fd_zero as u64, buf.as_mut_ptr() as u64, 32);
    assert_eq!(read_zero, 32);
    for b in buf {
        assert_eq!(b, 0x00);
    }
    assert_eq!(sys_close(fd_zero as u64), 0);

    // /dev/null
    let null_path = b"/dev/null\0";
    let fd_null = sys_open(null_path.as_ptr() as u64, 0, 0);
    assert!(fd_null >= 3);
    let read_null = sys_read(fd_null as u64, buf.as_mut_ptr() as u64, 32);
    assert_eq!(read_null, 0); // EOF
    assert_eq!(sys_close(fd_null as u64), 0);
}

#[test]
fn integration_scheduler_lifecycle() {
    let mut sched = Scheduler::new();
    let t1 = Arc::new(Task::new(10, None));
    let t2 = Arc::new(Task::new(20, None));

    sched.add_task(t1.clone());
    sched.add_task(t2.clone());

    let next1 = sched.next_task().unwrap();
    assert_eq!(next1.pid, 10);
    assert_eq!(next1.state(), TaskState::Running);

    sched.tick();
    assert_eq!(sched.current_task().unwrap().pid, 20);

    // Terminate task 2
    let curr = sched.current_task().unwrap();
    curr.set_exit_code(42);
    assert_eq!(curr.state(), TaskState::Zombie);

    // Next tick will only have t1
    sched.tick();
    assert_eq!(sched.current_task().unwrap().pid, 10);
}

#[test]
fn integration_full_ai_os_stack_e2e() {
    use metalos_welder::{AgentRuntime, ProviderToolCall as ToolCall, UiCallerOrigin};
    use metalos_boot::splash::LiquidSplashAnimation;
    use metalos_ferrofluid::{AgentUiToken, CallerPrincipal, FerrofluidError, SecurityGate};
    use metalos_js::{
        CalendarTime, CronExpr, EventPattern, MetalOsHost, OsEvent, ScheduleTrigger,
        ScriptScheduler, VirtualHostEnv,
    };
    use metalos_mcp::{
        register_builtin_apps, validate_app_manifest, AppCapability, AppManifest, McpRegistry,
        McpValue,
    };

    // 1. Phase 3: Boot Splash & Liquid Metal Drip Animation coalescing into Ferrofluid border
    let mut splash = LiquidSplashAnimation::new(320, 200);
    for _ in 0..20 {
        splash.step_frame(100);
    }
    assert_eq!(splash.coalesce_progress, 100);

    // 2. Phase 4 & 7: the UI capability boundary rejects headless MCP apps.
    let agent_token = AgentUiToken::system_default();
    let gate = SecurityGate::new(agent_token);
    let rogue_app_caller = CallerPrincipal::DeveloperMcpApp {
        app_id: "notes".to_string(),
    };
    assert_eq!(
        gate.authorize(&rogue_app_caller, None),
        Err(FerrofluidError::UiCreationForbiddenForApp),
    );

    // 3. Phase 7: Headless MCP Apps expose tools, resources, SKILLs & Tiles; validated against UI/Shell/Scheduler/CrossApp rules
    let bad_manifest = AppManifest::new(
        "bad_ui_app",
        "Bad UI App",
        "0.1.0",
        vec![AppCapability::ExposeMcpTools, AppCapability::CreateWindow],
    );
    assert!(validate_app_manifest(&bad_manifest).is_err());

    // Only `tasks` may request ScriptSchedulerAccess
    let bad_sched_manifest = AppManifest::new(
        "unauthorized_sched_app",
        "Unauthorized Scheduler App",
        "0.1.0",
        vec![
            AppCapability::ExposeMcpTools,
            AppCapability::ScriptSchedulerAccess,
        ],
    );
    assert!(validate_app_manifest(&bad_sched_manifest).is_err());

    let mut mcp = McpRegistry::new();
    register_builtin_apps(&mut mcp).expect("builtin Tasks MCP app must pass sandbox check");
    assert_eq!(mcp.app_count(), 1);
    assert_eq!(mcp.list_skills().len(), 1);
    assert_eq!(mcp.list_tiles().len(), 1);

    // Call `tasks.create` via MCP (as Welder)
    let mut args = std::collections::BTreeMap::new();
    args.insert(
        "id".to_string(),
        McpValue::Text("nightly-audit".to_string()),
    );
    args.insert(
        "path".to_string(),
        McpValue::Text("/scripts/heartbeat.js".to_string()),
    );
    args.insert(
        "trigger".to_string(),
        McpValue::Text("every:60s".to_string()),
    );
    let mcp_res = mcp
        .call_tool("tasks.create", &args)
        .expect("MCP tasks.create should succeed");
    assert!(mcp_res.to_json().contains("nightly-audit"));

    // 4. Phase 6: Model writes local JS script & schedules it on both FS events and Cron time
    let mut js_host = VirtualHostEnv::new();
    let mut scheduler = ScriptScheduler::new();

    scheduler.register_script_source(
        "/scripts/on_note_sync.js",
        r#"
            const data = MetalOS.fs.read(EVENT.path);
            MetalOS.fs.write("/synced/latest.txt", "SYNC:" + data);
            MetalOS.ui.notify("Synced " + EVENT.path);
        "#,
    );
    scheduler.schedule_script(
        "evt_sync",
        "/scripts/on_note_sync.js",
        ScheduleTrigger::OnEvent(EventPattern::FsCreated("/notes/*".to_string())),
    );

    scheduler.register_script_source(
        "/scripts/cron_telemetry.js",
        r#"
            const st = MetalOS.sys.status();
            MetalOS.fs.write("/var/heartbeat.txt", "uptime=" + st.uptimeMs);
        "#,
    );
    let cron = CronExpr::parse("*/5 * * * *").expect("valid cron");
    scheduler.schedule_script(
        "cron_5m",
        "/scripts/cron_telemetry.js",
        ScheduleTrigger::Cron(cron),
    );

    // Trigger FS event
    js_host.fs_write("/notes/design.md", "AI-First OS").unwrap();
    let evt_reports = scheduler.dispatch_event(
        &OsEvent::FsCreated {
            path: "/notes/design.md".to_string(),
        },
        &mut js_host,
    );
    assert_eq!(evt_reports.len(), 1);
    assert!(evt_reports[0].result.is_ok());
    assert_eq!(
        js_host.fs_read("/synced/latest.txt").as_deref(),
        Some("SYNC:AI-First OS")
    );

    // Trigger Cron tick at minute=10
    let cal = CalendarTime {
        year: 2026,
        month: 10,
        day: 12,
        hour: 9,
        minute: 10,
        day_of_week: 1,
    };
    let cron_reports = scheduler.tick_time(60_000, Some(cal), &mut js_host);
    assert_eq!(cron_reports.len(), 1);
    assert!(cron_reports[0].result.is_ok());
    assert!(js_host
        .fs_read("/var/heartbeat.txt")
        .unwrap()
        .starts_with("uptime="));

    // 5. Phase 5: Pluggable AI Runtime blocks shell commands and synthesizes UI exclusively
    let mut ai = AgentRuntime::new();
    assert!(ai.handle_console_input("!ls /", &mut |_| {}).is_err());
    let forbidden_call = ToolCall::new("tc_1", "sh_exec", "{\"cmd\":\"ls\"}");
    assert!(ai
        .tool_registry
        .execute_tool(&forbidden_call, &mut ai.ui_synthesizer)
        .is_err());

    ai.handle_console_input("/model claude", &mut |_| {})
        .unwrap();
    assert_eq!(ai.active_provider_name(), "claude");

    // Agent synthesizes UI view with declarative AppTile and renders it to Ferrofluid using its exclusive token
    let tile = mcp.render_tile("tasks.board").ok();
    let view = ai
        .ui_synthesizer
        .synthesize_from_mcp_output_with_tile(
            &UiCallerOrigin::AiAgent,
            "tasks",
            "tasks.create",
            &mcp_res.to_json(),
            tile,
        )
        .expect("Agent is allowed to synthesize UI");
    assert_eq!(view.title, "Tasks");
    assert_eq!(view.source_app.as_deref(), Some("tasks"));
    assert!(view.tile.is_some());


    // The agent retains authorization independently of any rendering toolkit.
    gate.authorize(&agent_token.principal(), Some(&agent_token))
        .expect("Agent with valid token can update System UI");
}
