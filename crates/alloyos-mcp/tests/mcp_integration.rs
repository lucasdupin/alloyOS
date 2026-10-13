use std::collections::BTreeMap;

use alloyos_mcp::{
    mcp_map, methods, register_builtin_apps, validate_app_manifest, AppCapability, AppManifest,
    McpCaller, McpError, McpRegistry, McpRequest, McpResponse, McpSandboxError, McpValue,
    RuntimeCapabilityGuard, TasksApp, ERR_METHOD_NOT_FOUND, ERR_RESOURCE_NOT_FOUND,
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

    let bad_scheduler_cfg = "\
app_id = fake_tasks
name = Fake Tasks
version = 0.1.0
capabilities = ExposeMcpTools, ScriptSchedulerAccess
";
    let manifest = AppManifest::parse(bad_scheduler_cfg).expect("parse config");
    assert_eq!(
        validate_app_manifest(&manifest),
        Err(McpSandboxError::SchedulerAccessForbidden {
            app_id: "fake_tasks".to_string(),
        })
    );

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
    register_builtin_apps(&mut registry).expect("register_builtin_apps");
    assert_eq!(registry.app_count(), 2);

    // Verify resources/list returns the 3 Tasks resources (schedules, tiles, skill)
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
    assert_eq!(resources.len(), 4);

    // Verify skills/list and tiles/list
    let skills: Vec<_> = registry.list_skills().into_iter().filter(|skill| skill.app_id == "tasks").collect();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].app_id, "tasks");
    assert_eq!(skills[0].path, "/apps/tasks/SKILL.md");

    let tiles: Vec<_> = registry.list_tiles().into_iter().filter(|tile| tile.folder() == Some("tasks")).collect();
    assert_eq!(tiles.len(), 1);
    assert_eq!(tiles[0].tile_id, "tasks.board");
    assert_eq!(tiles[0].folder(), Some("tasks"));

    // Unknown tool returns ERR_METHOD_NOT_FOUND
    let bad_tool_req = McpRequest::new(
        2,
        methods::TOOLS_CALL,
        mcp_map(&[
            ("name", McpValue::Text("tasks.nonexistent".to_string())),
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
        mcp_map(&[("uri", McpValue::Text("mcp://tasks/unknown".to_string()))]),
    );
    let bad_res_resp = registry.handle_request(&bad_res_req);
    assert_eq!(
        bad_res_resp.error.as_ref().map(|e| e.code),
        Some(ERR_RESOURCE_NOT_FOUND)
    );

    // Verify RuntimeCapabilityGuard for Tasks app
    let guard: &RuntimeCapabilityGuard = registry.guard_for("tasks").expect("guard present");
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

    // Verify that another app cannot access Tasks resources or tools
    let cross_res = registry.read_resource_as(
        &McpCaller::App("other_app".to_string()),
        "mcp://tasks/schedules",
    );
    assert_eq!(
        cross_res.expect_err("cross-app resource read must fail").code,
        ERR_SANDBOX_VIOLATION
    );

    let _t = TasksApp::default();
    let _err = McpError::internal_error("check");
    let _ = McpResponse::from_json(&bad_res_resp.to_json()).unwrap();
}

#[test]
fn obsolete_entrypoint_is_ignored_and_omitted_on_serialization() {
    let base = "name = Clock\nversion = 1.0.0\ncapabilities = ExposeMcpTools\n";
    let expected = AppManifest::parse_in_folder("clock", base).unwrap();
    for path in ["/apps/clock/other.js", "/apps/other/main.js", "/apps/clock/../other/main.js", "/var/clock/main.txt"] {
        let manifest = AppManifest::parse_in_folder("clock", &format!("{base}entrypoint = {path}\n")).unwrap();
        assert_eq!(manifest, expected);
        validate_app_manifest(&manifest).unwrap();
        assert!(!manifest.to_mcp_config().contains("entrypoint"));
        assert_eq!(AppManifest::parse(&manifest.to_mcp_config()).unwrap(), manifest);
    }
}

#[test]
fn bundled_tasks_main_js_exists_and_parses() {
    AppManifest::parse(alloyos_mcp::TASKS_MANIFEST_MCP).unwrap();
    alloyos_js::parse_program(alloyos_mcp::TASKS_MAIN_JS).unwrap();
}
