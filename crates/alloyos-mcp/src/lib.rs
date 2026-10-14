#![cfg_attr(not(test), no_std)]

#[cfg(test)]
extern crate std;

extern crate alloc;
#[cfg(test)]
extern crate self as alloyos_mcp;

#[cfg(test)]
#[path = "../../../tests/support/apps.rs"]
mod fixture_apps;

pub mod apps;
pub mod process;
pub mod protocol;
pub mod display;
pub mod packages;
pub mod registry;
pub mod sandbox;
pub mod transport;
mod remote;

pub use apps::{JsApp, RemoteMcpApp};
pub use remote::parse_remote_tools;
pub use protocol::{
    mcp_map, methods, tool_text_result, AppSkill, AppTile, McpError, McpParamSpec, McpPrompt,
    McpRequest, McpResource, McpResourceContent, McpResponse, McpTool, McpValue, TileAction,
    TileItem, ERR_INTERNAL_ERROR, ERR_INVALID_PARAMS, ERR_INVALID_REQUEST, ERR_METHOD_NOT_FOUND,
    ERR_PARSE_ERROR, ERR_RESOURCE_NOT_FOUND, ERR_SANDBOX_VIOLATION,
};
pub use packages::{PackageSnapshot, PackageService, PackageStatus, PackageDiagnostic, PackageChange, ValidatedPackageRevision};
pub use registry::{McpApp, McpCaller, McpRegistry};
pub use sandbox::{
    app_allowed_write_root, is_within_app_write_allowlist, validate_app_manifest, AppCapability,
    AppManifest, McpSandboxError, RuntimeCapabilityGuard,
};

#[cfg(test)]
mod tests {
    use crate::fixture_apps;
    use fixture_apps::register_test_apps;
    use super::*;
    use alloc::boxed::Box;
    use alloc::collections::BTreeMap;
    use alloc::string::{String, ToString};
    use alloc::vec;
    use alloc::vec::Vec;

    struct MaliciousApp {
        manifest: AppManifest,
    }

    impl MaliciousApp {
        fn new(app_id: &str, cap: AppCapability) -> Self {
            Self {
                manifest: AppManifest::new(
                    app_id,
                    "Rogue App",
                    "0.1.0",
                    vec![AppCapability::ExposeMcpTools, cap],
                ),
            }
        }
    }

    impl McpApp for MaliciousApp {
        fn manifest(&self) -> &AppManifest {
            &self.manifest
        }
        fn tools(&self) -> Vec<McpTool> {
            vec![]
        }
        fn resources(&self) -> Vec<McpResource> {
            vec![]
        }
        fn prompts(&self) -> Vec<McpPrompt> {
            vec![]
        }
        fn call_tool(
            &mut self,
            _tool_name: &str,
            _args: &BTreeMap<String, McpValue>,
        ) -> Result<McpValue, McpError> {
            Ok(McpValue::Null)
        }
        fn read_resource(&self, _uri: &str) -> Result<McpResourceContent, McpError> {
            Err(McpError::resource_not_found("none"))
        }
        fn get_prompt(
            &self,
            _name: &str,
            _args: &BTreeMap<String, String>,
        ) -> Result<String, McpError> {
            Err(McpError::method_not_found("none"))
        }
    }

    #[test]
    fn manifest_validation_checks_capabilities_independently_of_app_identity()
    {
        let sample_scheduler = AppManifest::parse(include_str!("../../../tests/fixtures/apps/sample_scheduler/sample_scheduler.mcp")).expect("parse sample_scheduler.mcp");
        assert_eq!(sample_scheduler.app_id, "sample_scheduler");
        assert!(validate_app_manifest(&sample_scheduler).is_ok());

        let roundtrip = AppManifest::parse(&sample_scheduler.to_mcp_config()).expect("roundtrip parse");
        assert_eq!(roundtrip, sample_scheduler);

        // Scheduler access is a manifest capability, independent of package identity.
        let rogue_scheduler = AppManifest::new(
            "rogue_cron",
            "Rogue Cron",
            "1.0.0",
            vec![
                AppCapability::ExposeMcpTools,
                AppCapability::ScriptSchedulerAccess,
            ],
        );
        assert!(validate_app_manifest(&rogue_scheduler).is_ok());

        // Non-sample_scheduler app attempting ScopedStorage(/etc/schedules.conf) or ScopedStorage(/scripts) is rejected
        for protected_path in ["/etc/schedules.conf", "/scripts", "/scripts/hack.js"] {
            let rogue_storage = AppManifest::new(
                "rogue_storage",
                "Rogue Storage",
                "1.0.0",
                vec![
                    AppCapability::ExposeMcpTools,
                    AppCapability::ScopedStorage(protected_path.to_string()),
                ],
            );
            assert_eq!(
                validate_app_manifest(&rogue_storage),
                Err(McpSandboxError::SchedulerAccessForbidden {
                    app_id: "rogue_storage".to_string(),
                })
            );
        }

        // CrossAppAccess is rejected for all apps (even sample_scheduler)
        let cross_app = AppManifest::new(
            "sample_scheduler",
            "Sample Scheduler",
            "1.0.0",
            vec![AppCapability::ExposeMcpTools, AppCapability::CrossAppAccess],
        );
        assert_eq!(
            validate_app_manifest(&cross_app),
            Err(McpSandboxError::CrossAppAccessForbidden {
                caller_app_id: "sample_scheduler".to_string(),
                target_app_id: "*".to_string(),
            })
        );
    }

    #[test]
    fn manifest_validation_strictly_rejects_ui_and_shell_capabilities() {
        for forbidden_ui in [
            AppCapability::CreateWindow,
            AppCapability::DirectFramebuffer,
            AppCapability::RawInputCapture,
        ] {
            let manifest = AppManifest::new(
                "bad_ui_app",
                "Bad UI App",
                "1.0.0",
                vec![AppCapability::ExposeMcpTools, forbidden_ui.clone()],
            );
            let err = validate_app_manifest(&manifest).expect_err("must reject UI capability");
            assert_eq!(
                err,
                McpSandboxError::UiCapabilityForbidden {
                    app_id: "bad_ui_app".to_string(),
                    capability: forbidden_ui.clone(),
                }
            );

            let mut reg = McpRegistry::new();
            let reg_err = reg
                .register_app(Box::new(MaliciousApp::new("bad_ui_app", forbidden_ui)))
                .expect_err("registry must reject UI app");
            assert!(matches!(
                reg_err,
                McpSandboxError::UiCapabilityForbidden { .. }
            ));
        }

        let shell_manifest = AppManifest::new(
            "bad_shell_app",
            "Bad Shell App",
            "1.0.0",
            vec![
                AppCapability::ExposeMcpTools,
                AppCapability::ShellExecution,
            ],
        );
        let err = validate_app_manifest(&shell_manifest).expect_err("must reject ShellExecution");
        assert_eq!(
            err,
            McpSandboxError::ShellCapabilityForbidden {
                app_id: "bad_shell_app".to_string(),
                capability: AppCapability::ShellExecution,
            }
        );
    }

    #[test]
    fn runtime_guard_enforces_zero_ui_declared_scheduler_access_and_no_cross_app() {
        let manifest = AppManifest::parse(include_str!("../../../tests/fixtures/apps/sample_scheduler/sample_scheduler.mcp")).unwrap();
        let guard = RuntimeCapabilityGuard::new(manifest).expect("valid guard");

        assert!(guard
            .enforce_capability(&AppCapability::ExposeMcpTools)
            .is_ok());
        assert!(guard
            .enforce_capability(&AppCapability::ExposeSkills)
            .is_ok());
        assert!(guard
            .enforce_capability(&AppCapability::ExposeTiles)
            .is_ok());
        assert!(guard
            .enforce_capability(&AppCapability::ScriptSchedulerAccess)
            .is_ok());
        assert!(guard
            .enforce_capability(&AppCapability::ScopedStorage(
                "/scripts/heartbeat.js".to_string()
            ))
            .is_ok());
        assert!(guard
            .enforce_capability(&AppCapability::ScopedStorage(
                "/etc/schedules.conf".to_string()
            ))
            .is_ok());
        assert!(guard
            .enforce_capability(&AppCapability::ScopedStorage("/etc/passwd".to_string()))
            .is_err());

        // Even Sample Scheduler cannot access another app
        assert_eq!(
            guard.enforce_no_cross_app_access("other_app"),
            Err(McpSandboxError::CrossAppAccessForbidden {
                caller_app_id: "sample_scheduler".to_string(),
                target_app_id: "other_app".to_string(),
            })
        );
    }

    #[test]
    fn mcp_app_write_permissions_are_allow_list_based_under_var_app_name() {
        assert_eq!(app_allowed_write_root("notes"), "/var/notes");
        assert!(is_within_app_write_allowlist("notes", "/var/notes"));
        assert!(is_within_app_write_allowlist("notes", "/var/notes/state.json"));
        assert!(is_within_app_write_allowlist("notes", "/var/notes/sub/item.md"));
        assert!(!is_within_app_write_allowlist("notes", "/var/notes_other/state.json"));
        assert!(!is_within_app_write_allowlist("notes", "/var/sample_timer/state.json"));
        assert!(!is_within_app_write_allowlist("notes", "/knowledge/profile.md"));
        assert!(!is_within_app_write_allowlist("notes", "/apps/notes/main.js"));
        assert!(!is_within_app_write_allowlist("notes", "/etc/ai.conf"));
        assert!(!is_within_app_write_allowlist("notes", "/tmp/scratch.txt"));

        // Manifest declaring ScopedStorage outside /var/<app_id>/ is rejected
        for forbidden_scope in [
            "/var/other_app",
            "/knowledge",
            "/knowledge/profile.md",
            "/etc",
            "/tmp",
            "/apps/notes",
        ] {
            let bad_manifest = AppManifest::new(
                "notes",
                "Notes",
                "0.1.0",
                vec![
                    AppCapability::ExposeMcpTools,
                    AppCapability::ScopedStorage(forbidden_scope.to_string()),
                ],
            );
            assert!(
                matches!(
                    validate_app_manifest(&bad_manifest),
                    Err(McpSandboxError::InvalidManifest { .. })
                ),
                "expected ScopedStorage({}) to be rejected by allow-list",
                forbidden_scope
            );
        }

        // RuntimeCapabilityGuard allows writes to /var/<app_id>/ by default and denies everything else
        let notes_manifest = AppManifest::new(
            "notes",
            "Notes",
            "0.1.0",
            vec![AppCapability::ExposeMcpTools],
        );
        let notes_guard = RuntimeCapabilityGuard::new(notes_manifest).expect("valid notes guard");
        assert!(notes_guard.enforce_write_path("/var/notes").is_ok());
        assert!(notes_guard
            .enforce_write_path("/var/notes/index.json")
            .is_ok());
        assert!(notes_guard
            .enforce_write_path("/var/other_app/index.json")
            .is_err());
        assert!(notes_guard
            .enforce_write_path("/knowledge/profile.md")
            .is_err());
        assert!(notes_guard
            .enforce_write_path("/var/notes/../other_app/file.txt")
            .is_err());
        // /apps/ remains strictly read-only to apps
        assert!(notes_guard
            .enforce_write_path("/apps/notes/main.js")
            .is_err());

        // Default outbound network access granted without manifest declaration
        assert!(notes_guard
            .enforce_capability(&AppCapability::ScopedNetwork)
            .is_ok());
        use crate::sandbox::SyscallNo;
        assert!(notes_guard.enforce_syscall(SyscallNo::Socket).is_ok());
        assert!(notes_guard.enforce_syscall(SyscallNo::Connect).is_ok());
    }

    #[test]
    fn filesystem_app_mcp_skills_tiles_and_welder_orchestration() {
        let mut registry = McpRegistry::new();
        register_test_apps(&mut registry).expect("discover filesystem fixtures");
        assert_eq!(registry.app_count(), 3);

        // 1. Verify initialize reports sample_scheduler app and welderCentralizedOrchestration
        let init_resp = McpResponse::from_json(&registry.handle_json_rpc(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        ))
        .unwrap();
        assert!(init_resp.is_ok());

        // 2. Verify tools/list returns Sample Scheduler, Sample Timer, and PackageManager tools
        let tools = registry.list_tools();
        assert_eq!(tools.len(), 8);

        // 3. Create a scheduled task via sample_scheduler.create
        let create_json = r#"{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"sample_scheduler.create","arguments":{"id":"heartbeat_60s","title":"System Heartbeat","script_path":"/scripts/heartbeat.js","trigger":"every:60s","source":"export function main() { return AlloyOS.sys.status(); }"}}}"#;
        let create_resp = McpResponse::from_json(&registry.handle_json_rpc(create_json)).unwrap();
        assert!(create_resp.is_ok());
        let create_map = create_resp.result.as_ref().unwrap().as_map().unwrap();
        assert_eq!(
            create_map.get("status").and_then(|v| v.as_str()),
            Some("created")
        );

        // 4. List scheduled sample_scheduler via sample_scheduler.list
        let list_json = r#"{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"name":"sample_scheduler.list","arguments":{}}}"#;
        let list_resp = McpResponse::from_json(&registry.handle_json_rpc(list_json)).unwrap();
        let list_map = list_resp.result.as_ref().unwrap().as_map().unwrap();
        assert_eq!(list_map.get("count").and_then(|v| v.as_i64()), Some(1));

        // 5. Edit the scheduled task via sample_scheduler.edit (change trigger to cron and title, then pause it)
        let edit_json = r#"{"jsonrpc":"2.0","id":12,"method":"tools/call","params":{"name":"sample_scheduler.edit","arguments":{"id":"heartbeat_60s","title":"5m Cron Heartbeat","trigger":"cron:*/5 * * * *","enabled":false}}}"#;
        let edit_resp = McpResponse::from_json(&registry.handle_json_rpc(edit_json)).unwrap();
        assert!(edit_resp.is_ok());
        let edited_task = edit_resp
            .result
            .as_ref()
            .unwrap()
            .as_map()
            .unwrap()
            .get("task")
            .unwrap()
            .as_map()
            .unwrap();
        assert_eq!(
            edited_task.get("title").and_then(|v| v.as_str()),
            Some("5m Cron Heartbeat")
        );
        assert_eq!(
            edited_task.get("trigger").and_then(|v| v.as_str()),
            Some("cron:*/5 * * * *")
        );
        assert_eq!(
            edited_task.get("status").and_then(|v| v.as_str()),
            Some("paused")
        );

        // 6. Verify SKILL exposure (skills/list, skills/read, and mcp://sample_scheduler/skill)
        let skills: alloc::vec::Vec<_> = registry.list_skills().into_iter()
            .filter(|skill| skill.app_id == "sample_scheduler").collect();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].path, "/apps/sample_scheduler/SKILL.md");
        assert!(skills[0].content.contains("sample_scheduler.create"));

        let skill_read_json = r#"{"jsonrpc":"2.0","id":20,"method":"skills/read","params":{"app_id":"sample_scheduler"}}"#;
        let skill_resp =
            McpResponse::from_json(&registry.handle_json_rpc(skill_read_json)).unwrap();
        assert!(skill_resp.is_ok());

        // 7. Verify Declarative Tile exposure (tiles/list and tiles/render)
        let tiles: alloc::vec::Vec<_> = registry.list_tiles().into_iter()
            .filter(|tile| tile.folder() == Some("sample_scheduler")).collect();
        assert_eq!(tiles.len(), 1);
        assert_eq!(tiles[0].tile_id, "sample_scheduler.board");
        assert_eq!(tiles[0].folder(), Some("sample_scheduler"));
        assert_eq!(tiles[0].items.len(), 1);
        assert_eq!(tiles[0].items[0].title, "5m Cron Heartbeat");
        assert_eq!(tiles[0].items[0].badge, "cron:*/5 * * * *");
        assert_eq!(tiles[0].items[0].status, "paused");

        let tile_render_json = r#"{"jsonrpc":"2.0","id":21,"method":"tiles/render","params":{"tile_id":"sample_scheduler.board"}}"#;
        let tile_resp =
            McpResponse::from_json(&registry.handle_json_rpc(tile_render_json)).unwrap();
        assert!(tile_resp.is_ok());

        // 8. Verify No App Can Access Another App (only Welder centralizes app access)
        let cross_app_call = registry.call_tool_as(
            &McpCaller::App("other_app".to_string()),
            "sample_scheduler.list",
            &BTreeMap::new(),
        );
        assert_eq!(
            cross_app_call.expect_err("app-to-app call must be rejected").code,
            ERR_SANDBOX_VIOLATION
        );

        let cross_app_json = r#"{"jsonrpc":"2.0","id":30,"method":"tools/call","params":{"caller_app":"rogue_app","name":"sample_scheduler.list","arguments":{}}}"#;
        let cross_app_resp =
            McpResponse::from_json(&registry.handle_json_rpc(cross_app_json)).unwrap();
        assert_eq!(
            cross_app_resp.error.as_ref().map(|e| e.code),
            Some(ERR_SANDBOX_VIOLATION)
        );

        // 9. Delete the scheduled task via sample_scheduler.delete
        let del_json = r#"{"jsonrpc":"2.0","id":40,"method":"tools/call","params":{"name":"sample_scheduler.delete","arguments":{"id":"heartbeat_60s"}}}"#;
        let del_resp = McpResponse::from_json(&registry.handle_json_rpc(del_json)).unwrap();
        assert_eq!(
            del_resp
                .result
                .unwrap()
                .as_map()
                .unwrap()
                .get("deleted")
                .and_then(|v| v.as_bool()),
            Some(true)
        );
    }
}
