#[path = "../../../tests/support/apps.rs"]
mod fixture_apps;
use fixture_apps::register_test_apps;
use std::collections::BTreeMap;

use alloyos_mcp::{
    mcp_map, methods, validate_app_manifest, AppCapability, AppManifest, McpCaller, McpError,
    McpRegistry, McpRequest, McpResponse, McpSandboxError, McpValue, RuntimeCapabilityGuard,
    ERR_METHOD_NOT_FOUND, ERR_RESOURCE_NOT_FOUND, ERR_SANDBOX_VIOLATION,
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
