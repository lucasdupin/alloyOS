use std::collections::BTreeMap;

use metalos_mcp::{
    mcp_map, methods, register_builtin_apps, validate_app_manifest, AppCapability, AppManifest,
    McpError, McpRegistry, McpRequest, McpResponse, McpSandboxError, McpValue, NotesVaultApp,
    RuntimeCapabilityGuard, SysPulseApp, TaskBoardApp, ERR_METHOD_NOT_FOUND, ERR_RESOURCE_NOT_FOUND,
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
            AppCapability::ScopedStorage("/var/notes/../secret".to_string()),
        ],
    );
    assert!(matches!(
        validate_app_manifest(&traversal),
        Err(McpSandboxError::InvalidManifest { .. })
    ));
}

#[test]
fn registry_routes_all_three_apps_and_handles_errors_cleanly() {
    let mut registry = McpRegistry::new();
    register_builtin_apps(&mut registry).expect("register_builtin_apps");

    // Verify resources/list returns all 4 resources across notes, tasks, syspulse
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

    // Unknown tool returns ERR_METHOD_NOT_FOUND
    let bad_tool_req = McpRequest::new(
        2,
        methods::TOOLS_CALL,
        mcp_map(&[
            ("name", McpValue::Text("notes.nonexistent".to_string())),
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
        mcp_map(&[("uri", McpValue::Text("mcp://notes/unknown".to_string()))]),
    );
    let bad_res_resp = registry.handle_request(&bad_res_req);
    assert_eq!(
        bad_res_resp.error.as_ref().map(|e| e.code),
        Some(ERR_RESOURCE_NOT_FOUND)
    );

    // Verify RuntimeCapabilityGuard is accessible for all 3 apps
    for app_id in ["notes", "tasks", "syspulse"] {
        let guard: &RuntimeCapabilityGuard = registry.guard_for(app_id).expect("guard present");
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
    }

    // Verify direct instantiation of NotesVaultApp, TaskBoardApp, SysPulseApp
    let _n = NotesVaultApp::default();
    let _t = TaskBoardApp::default();
    let _s = SysPulseApp::default();
    let _err = McpError::internal_error("check");
    let _ = McpResponse::from_json(&bad_res_resp.to_json()).unwrap();
}
