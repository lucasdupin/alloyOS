#[path = "../../../tests/support/apps.rs"]
mod fixture_apps;
use fixture_apps::register_test_apps;
use std::collections::BTreeMap;

use alloyos_mcp::{
    mcp_map, methods, validate_app_manifest, AppCapability, AppManifest, McpCaller, McpError,
    McpRegistry, McpRequest, McpResponse, McpSandboxError, McpValue, PackageService,
    PackageSnapshot, RuntimeCapabilityGuard, ERR_METHOD_NOT_FOUND, ERR_RESOURCE_NOT_FOUND,
    ERR_SANDBOX_VIOLATION,
};

#[test]
fn sandbox_manifest_rejects_all_forbidden_capabilities_from_config_text() {
    let bad_window_cfg = "\
app_id = window_hack
name = Window Hack
version = 0.1.0
capabilities = ExposeMcpTools, CreateWindow
";
    let manifest = AppManifest::parse(bad_window_cfg).expect("parse config");
    assert_eq!(
        validate_app_manifest(&manifest),
        Err(McpSandboxError::UiCapabilityForbidden {
            app_id: "window_hack".to_string(),
            capability: AppCapability::CreateWindow,
        })
    );

    let bad_fb_cfg = "\
app_id = fb_hack
name = Framebuffer Hack
version = 0.1.0
capabilities = ExposeMcpTools, DirectFramebuffer
";
    let manifest = AppManifest::parse(bad_fb_cfg).expect("parse config");
    assert_eq!(
        validate_app_manifest(&manifest),
        Err(McpSandboxError::UiCapabilityForbidden {
            app_id: "fb_hack".to_string(),
            capability: AppCapability::DirectFramebuffer,
        })
    );

    let bad_input_cfg = "\
app_id = input_hack
name = Input Hack
version = 0.1.0
capabilities = ExposeMcpResources, RawInputCapture
";
    let manifest = AppManifest::parse(bad_input_cfg).expect("parse config");
    assert_eq!(
        validate_app_manifest(&manifest),
        Err(McpSandboxError::UiCapabilityForbidden {
            app_id: "input_hack".to_string(),
            capability: AppCapability::RawInputCapture,
        })
    );

    let bad_shell_cfg = "\
app_id = shell_hack
name = Shell Hack
version = 0.1.0
capabilities = ExposeMcpPrompts, ShellExecution
";
    let manifest = AppManifest::parse(bad_shell_cfg).expect("parse config");
    assert_eq!(
        validate_app_manifest(&manifest),
        Err(McpSandboxError::ShellCapabilityForbidden {
            app_id: "shell_hack".to_string(),
            capability: AppCapability::ShellExecution,
        })
    );

    let scheduler_cfg = "app_id=arbitrary_scheduler\nname=Scheduler\nversion=1\ncapabilities=ExposeMcpTools,ScriptSchedulerAccess\n";
    assert!(validate_app_manifest(&AppManifest::parse(scheduler_cfg).unwrap()).is_ok());

    let bad_cross_app_cfg = "\
app_id = orchestrator_hack
name = Orchestrator Hack
version = 0.1.0
capabilities = ExposeMcpTools, CrossAppAccess
";
    let manifest = AppManifest::parse(bad_cross_app_cfg).expect("parse config");
    assert_eq!(
        validate_app_manifest(&manifest),
        Err(McpSandboxError::CrossAppAccessForbidden {
            caller_app_id: "orchestrator_hack".to_string(),
            target_app_id: "*".to_string(),
        })
    );
}

#[test]
fn sandbox_rejects_empty_mcp_surface_and_path_traversal() {
    let empty_surface = AppManifest::new(
        "storage_only",
        "Storage Only",
        "1.0.0",
        vec![AppCapability::ScopedStorage("/var/data".to_string())],
    );
    assert_eq!(
        validate_app_manifest(&empty_surface),
        Err(McpSandboxError::EmptyMcpSurface {
            app_id: "storage_only".to_string(),
        })
    );

    let traversal = AppManifest::new(
        "traversal_app",
        "Traversal App",
        "1.0.0",
        vec![
            AppCapability::ExposeMcpTools,
            AppCapability::ScopedStorage("/var/data/../secret".to_string()),
        ],
    );
    assert!(matches!(
        validate_app_manifest(&traversal),
        Err(McpSandboxError::InvalidManifest { .. })
    ));
}

#[test]
fn registry_routes_tasks_app_skills_tiles_and_enforces_isolation() {
    let mut registry = McpRegistry::new();
    register_test_apps(&mut registry).expect("register_test_apps");
    assert_eq!(registry.app_count(), 3);

    // Verify resources/list returns Sample Scheduler (3), Sample Timer (1), and PackageManager (1) resources
    let req = McpRequest::new(1, methods::RESOURCES_LIST, BTreeMap::new());
    let resp = registry.handle_request(&req);
    assert!(resp.is_ok());
    let resources = resp
        .result
        .unwrap()
        .as_map()
        .unwrap()
        .get("resources")
        .unwrap()
        .as_array()
        .unwrap()
        .to_vec();
    assert_eq!(resources.len(), 5);

    // Verify skills/list and tiles/list
    let skills: Vec<_> = registry
        .list_skills()
        .into_iter()
        .filter(|skill| skill.app_id == "sample_scheduler")
        .collect();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].app_id, "sample_scheduler");
    assert_eq!(skills[0].path, "/apps/sample_scheduler/SKILL.md");

    let tiles: Vec<_> = registry
        .list_tiles()
        .into_iter()
        .filter(|tile| tile.folder() == Some("sample_scheduler"))
        .collect();
    assert_eq!(tiles.len(), 1);
    assert_eq!(tiles[0].tile_id, "sample_scheduler.board");
    assert_eq!(tiles[0].folder(), Some("sample_scheduler"));

    // Unknown tool returns ERR_METHOD_NOT_FOUND
    let bad_tool_req = McpRequest::new(
        2,
        methods::TOOLS_CALL,
        mcp_map(&[
            (
                "name",
                McpValue::Text("sample_scheduler.nonexistent".to_string()),
            ),
            ("arguments", McpValue::Map(BTreeMap::new())),
        ]),
    );
    let bad_tool_resp = registry.handle_request(&bad_tool_req);
    assert_eq!(
        bad_tool_resp.error.as_ref().map(|e| e.code),
        Some(ERR_METHOD_NOT_FOUND)
    );

    // Unknown resource returns ERR_RESOURCE_NOT_FOUND
    let bad_res_req = McpRequest::new(
        3,
        methods::RESOURCES_READ,
        mcp_map(&[(
            "uri",
            McpValue::Text("mcp://sample_scheduler/unknown".to_string()),
        )]),
    );
    let bad_res_resp = registry.handle_request(&bad_res_req);
    assert_eq!(
        bad_res_resp.error.as_ref().map(|e| e.code),
        Some(ERR_RESOURCE_NOT_FOUND)
    );

    // Verify RuntimeCapabilityGuard for Sample Scheduler app
    let guard: &RuntimeCapabilityGuard = registry
        .guard_for("sample_scheduler")
        .expect("guard present");
    assert!(guard
        .enforce_capability(&AppCapability::ScriptSchedulerAccess)
        .is_ok());
    assert!(guard
        .enforce_capability(&AppCapability::CreateWindow)
        .is_err());
    assert!(guard
        .enforce_capability(&AppCapability::DirectFramebuffer)
        .is_err());
    assert!(guard
        .enforce_capability(&AppCapability::RawInputCapture)
        .is_err());
    assert!(guard
        .enforce_capability(&AppCapability::ShellExecution)
        .is_err());
    assert!(guard
        .enforce_capability(&AppCapability::CrossAppAccess)
        .is_err());

    // Verify that another app cannot access Sample Scheduler resources or tools
    let cross_res = registry.read_resource_as(
        &McpCaller::App("other_app".to_string()),
        "mcp://sample_scheduler/schedules",
    );
    assert_eq!(
        cross_res
            .expect_err("cross-app resource read must fail")
            .code,
        ERR_SANDBOX_VIOLATION
    );

    let _err = McpError::internal_error("check");
    let _ = McpResponse::from_json(&bad_res_resp.to_json()).unwrap();
}

#[test]
fn obsolete_entrypoint_is_ignored_and_omitted_on_serialization() {
    let base = "name = Sample Timer\nversion = 1.0.0\ncapabilities = ExposeMcpTools\n";
    let expected = AppManifest::parse_in_folder("sample_timer", base).unwrap();
    for path in [
        "/apps/sample_timer/other.js",
        "/apps/other/main.js",
        "/apps/sample_timer/../other/main.js",
        "/var/sample_timer/main.txt",
    ] {
        let manifest =
            AppManifest::parse_in_folder("sample_timer", &format!("{base}entrypoint = {path}\n"))
                .unwrap();
        assert_eq!(manifest, expected);
        validate_app_manifest(&manifest).unwrap();
        assert!(!manifest.to_mcp_config().contains("entrypoint"));
        assert_eq!(
            AppManifest::parse(&manifest.to_mcp_config()).unwrap(),
            manifest
        );
    }
}

#[test]
fn fixture_entrypoint_exists_and_parses() {
    AppManifest::parse(include_str!(
        "../../../tests/fixtures/apps/sample_scheduler/sample_scheduler.mcp"
    ))
    .unwrap();
    alloyos_js::parse_program(include_str!(
        "../../../tests/fixtures/apps/sample_scheduler/main.js"
    ))
    .unwrap();
}

#[test]
fn filesystem_discovery_loads_arbitrary_apps_and_unloads_invalid_or_removed_packages() {
    let mut snapshot = alloyos_mcp::PackageSnapshot::default();
    snapshot.files.insert(
        "/apps/discovered-app/discovered-app.mcp".into(),
        "name=Discovered\nversion=1\ncapabilities=ExposeMcpTools,ExposeTiles\n".into(),
    );
    snapshot.files.insert(
        "/apps/discovered-app/tools.json".into(),
        r#"[{"name":"discovered-app.answer","description":"Answer","params":[]}]"#.into(),
    );
    snapshot.files.insert(
        "/apps/discovered-app/tiles.json".into(),
        r#"["discovered-app.tile"]"#.into(),
    );
    snapshot.files.insert("/apps/discovered-app/main.js".into(), r#"
        function callTool(name,args) { return JSON.stringify({ answer: 42 }); }
        function renderTile(id) { return JSON.stringify({ tile_id: id, title: "Discovered", items: [], actions: [] }); }
    "#.into());
    let mut registry = McpRegistry::new();
    assert_eq!(
        registry
            .discover_and_register_packages(&snapshot, None)
            .unwrap(),
        1
    );
    assert_eq!(
        registry
            .call_tool("discovered-app.answer", &BTreeMap::new())
            .unwrap()
            .as_map()
            .unwrap()["answer"]
            .as_i64(),
        Some(42)
    );
    assert_eq!(registry.list_tiles()[0].tile_id, "discovered-app.tile");
    snapshot.files.insert(
        "/apps/discovered-app/main.js".into(),
        "function broken(".into(),
    );
    assert_eq!(
        registry
            .discover_and_register_packages(&snapshot, None)
            .unwrap(),
        0
    );
    assert!(registry
        .call_tool("discovered-app.answer", &BTreeMap::new())
        .is_err());
    assert_eq!(registry.app_count(), 0);
    snapshot.files.insert(
        "/apps/discovered-app/main.js".into(),
        "function callTool(name,args) { return '{}'; } function renderTile(id) { return '{}'; }"
            .into(),
    );
    assert_eq!(
        registry
            .discover_and_register_packages(&snapshot, None)
            .unwrap(),
        1
    );
    assert_eq!(
        registry
            .discover_and_register_packages(&alloyos_mcp::PackageSnapshot::default(), None)
            .unwrap(),
        0
    );
    assert_eq!(registry.app_count(), 0);
}

#[test]
fn every_fixture_package_passes_validation() {
    let mut service = alloyos_mcp::PackageService::new();
    service.scan_startup(fixture_apps::snapshot());
    for status in service.inventory() {
        assert!(status.ready, "{}: {:?}", status.app_id, status.diagnostics);
    }
}

#[test]
fn every_declared_fixture_tile_renders() {
    let snapshot = fixture_apps::snapshot();
    let scheduler = std::rc::Rc::new(std::cell::RefCell::new(alloyos_js::ScriptScheduler::new()));
    use alloyos_mcp::McpApp;
    for directory in snapshot
        .directories
        .iter()
        .filter(|dir| dir.trim_matches('/').split('/').count() == 2)
    {
        let app_id = directory.rsplit('/').next().unwrap();
        let app = alloyos_mcp::JsApp::from_package_files(
            app_id,
            &snapshot.files,
            Some(scheduler.clone()),
        )
        .unwrap();
        if let Some(ids) = snapshot.files.get(&format!("{}/tiles.json", directory)) {
            let ids = McpValue::from_json(ids).unwrap();
            for id in ids.as_array().unwrap() {
                assert!(
                    app.render_tile(id.as_str().unwrap()).is_ok(),
                    "{}: {:?}",
                    app_id,
                    app.render_tile(id.as_str().unwrap())
                );
            }
        }
    }
}

pub fn repo_apps_snapshot() -> alloyos_mcp::PackageSnapshot {
    fn walk(path: &std::path::Path, root: &std::path::Path, result: &mut alloyos_mcp::PackageSnapshot) {
        for entry in std::fs::read_dir(path).unwrap() {
            let p = entry.unwrap().path();
            let target = format!("/apps/{}", p.strip_prefix(root).unwrap().display());
            if p.is_dir() {
                result.directories.push(target);
                walk(&p, root, result);
            } else {
                result
                    .files
                    .insert(target, std::fs::read_to_string(p).unwrap());
            }
        }
    }
    let mut result = alloyos_mcp::PackageSnapshot::default();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps");
    walk(&root, &root, &mut result);
    result
}

#[test]
fn every_repo_installed_package_passes_validation() {
    let mut service = alloyos_mcp::PackageService::new();
    service.scan_startup(repo_apps_snapshot());
    let statuses = service.inventory();
    assert!(statuses.len() >= 6, "expected at least 6 apps (clock, tasks, package-manager, calc, memory, weather), found {}", statuses.len());
    for status in statuses {
        assert!(status.ready, "{}: {:?}", status.app_id, status.diagnostics);
    }
}

#[test]
fn every_repo_installed_app_renders_tiles() {
    let snapshot = repo_apps_snapshot();
    let scheduler = std::rc::Rc::new(std::cell::RefCell::new(alloyos_js::ScriptScheduler::new()));
    use alloyos_mcp::McpApp;
    for directory in snapshot
        .directories
        .iter()
        .filter(|dir| dir.trim_matches('/').split('/').count() == 2)
    {
        let app_id = directory.rsplit('/').next().unwrap();
        let app = alloyos_mcp::JsApp::from_package_files(
            app_id,
            &snapshot.files,
            Some(scheduler.clone()),
        )
        .unwrap();
        if let Some(ids) = snapshot.files.get(&format!("{}/tiles.json", directory)) {
            let ids = McpValue::from_json(ids).unwrap();
            for id in ids.as_array().unwrap() {
                let rendered = app.render_tile(id.as_str().unwrap());
                assert!(
                    rendered.is_ok(),
                    "app {} tile {} failed to render: {:?}",
                    app_id,
                    id.as_str().unwrap(),
                    rendered
                );
            }
        }
    }
}

#[test]
fn calc_memory_and_weather_tools_execute() {
    let snapshot = repo_apps_snapshot();
    let mut registry = McpRegistry::new();
    let scheduler = std::rc::Rc::new(std::cell::RefCell::new(alloyos_js::ScriptScheduler::new()));
    registry.discover_and_register_packages(&snapshot, Some(scheduler)).unwrap();

    // 1. Calc
    let mut calc_args = BTreeMap::new();
    calc_args.insert("expression".to_string(), McpValue::Text("15 + 27 * 3".to_string()));
    let calc_res = registry.call_tool("calc.evaluate", &calc_args).unwrap();
    let calc_map = calc_res.as_map().unwrap();
    assert_eq!(calc_map["result"].as_i64(), Some(96));

    let mut convert_args = BTreeMap::new();
    convert_args.insert("value".to_string(), McpValue::Int(5));
    convert_args.insert("from".to_string(), McpValue::Text("km".to_string()));
    convert_args.insert("to".to_string(), McpValue::Text("m".to_string()));
    let conv_res = registry.call_tool("calc.convert", &convert_args).unwrap();
    let conv_map = conv_res.as_map().unwrap();
    assert_eq!(conv_map["result"].as_i64(), Some(5000));

    // 2. Memory
    let mut mem_args = BTreeMap::new();
    mem_args.insert("content".to_string(), McpValue::Text("User prefers dark obsidian palette".to_string()));
    mem_args.insert("category".to_string(), McpValue::Text("preference".to_string()));
    let mem_res = registry.call_tool("memory.remember", &mem_args).unwrap();
    let mem_map = mem_res.as_map().unwrap();
    assert_eq!(mem_map["status"].as_str(), Some("recorded"));

    let list_res = registry.call_tool("memory.list", &BTreeMap::new()).unwrap();
    let list_arr = list_res.as_array().unwrap();
    assert!(!list_arr.is_empty());

    // 3. Weather (network API fetch)
    registry.register_http_response(
        "https://wttr.in/Tokyo?format=j1",
        r#"{
            "current_condition": [
                {
                    "temp_C": "24",
                    "temp_F": "75",
                    "weatherDesc": [{"value": "Clear"}],
                    "humidity": "55",
                    "windspeedKmph": "10"
                }
            ],
            "weather": [
                {
                    "date": "2026-10-14",
                    "maxtempC": "26",
                    "mintempC": "18",
                    "hourly": [{"weatherDesc": [{"value": "Sunny"}]}]
                }
            ]
        }"#,
    );

    let mut weather_args = BTreeMap::new();
    weather_args.insert("city".to_string(), McpValue::Text("Tokyo".to_string()));
    weather_args.insert("units".to_string(), McpValue::Text("celsius".to_string()));
    let w_res = registry.call_tool("weather.current", &weather_args).unwrap();
    let w_map = w_res.as_map().unwrap();
    assert_eq!(w_map["city"].as_str(), Some("Tokyo"));
    assert_eq!(w_map["temperature"].as_str(), Some("24°C"));
    assert_eq!(w_map["condition"].as_str(), Some("Clear"));
}

#[test]
fn remote_package_validates_and_executes_with_explicit_transport_fixtures() {
    let mut snapshot = PackageSnapshot::default();
    snapshot.files.insert(
        "/apps/github/github.mcp".to_string(),
        "app_id = github\nname = GitHub MCP\nversion = 1.0.0\nendpoint = https://server.smithery.ai/@smithery-ai/github/mcp\ntransport = remote\ncapabilities = ExposeMcpTools, ScopedNetwork\n".to_string(),
    );
    snapshot.files.insert(
        "/apps/github/tools.json".to_string(),
        r#"[{"name":"create_issue","description":"Create a GitHub issue","params":[{"name":"title","type":"string","required":true,"description":"Issue title"}]}]"#.to_string(),
    );
    snapshot.files.insert(
        "/apps/github/SKILL.md".to_string(),
        "# GitHub Skill\nUse `github.create_issue` to open issues.\n".to_string(),
    );

    let mut service = PackageService::new();
    let changes = service.apply_snapshot(snapshot.clone());
    assert_eq!(changes.len(), 1);
    let status = service.status("github").expect("github package status");
    assert!(status.ready, "remote package must be ready without main.js: {:?}", status.diagnostics);

    let mut registry = McpRegistry::new();
    registry.register_http_response(
        "POST https://server.smithery.ai/@smithery-ai/github/mcp initialize",
        r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"fixture","version":"1"}}}"#,
    );
    registry.register_http_response(
        "POST https://server.smithery.ai/@smithery-ai/github/mcp notifications/initialized", "",
    );
    registry.register_http_response(
        "POST https://server.smithery.ai/@smithery-ai/github/mcp tools/call",
        r#"{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"Issue #42 created: Bug in auth"}]}}"#,
    );

    let registered_count = registry
        .discover_and_register_packages(&snapshot, None)
        .expect("discover packages");
    assert_eq!(registered_count, 1);
    assert_eq!(registry.app_count(), 1);

    let mut args = BTreeMap::new();
    args.insert("title".to_string(), McpValue::Text("Bug in auth".to_string()));
    let res = registry.call_tool("github.create_issue", &args).expect("remote tool call succeeds");
    assert!(res.to_json().contains("Issue #42 created: Bug in auth"));
}
