#![cfg_attr(not(test), no_std)]

#[cfg(test)]
extern crate std;

extern crate alloc;

pub mod apps;
pub mod protocol;
pub mod registry;
pub mod sandbox;

pub use apps::{
    register_builtin_apps, NoteEntry, NotesVaultApp, SysPulseApp, TaskBoardApp, TaskItem,
    TelemetrySample, NOTES_MANIFEST_MCP, SYSPULSE_MANIFEST_MCP, TASKS_MANIFEST_MCP,
};
pub use protocol::{
    mcp_map, methods, tool_text_result, McpError, McpParamSpec, McpPrompt, McpRequest,
    McpResource, McpResourceContent, McpResponse, McpTool, McpValue, ERR_INTERNAL_ERROR,
    ERR_INVALID_PARAMS, ERR_INVALID_REQUEST, ERR_METHOD_NOT_FOUND, ERR_PARSE_ERROR,
    ERR_RESOURCE_NOT_FOUND, ERR_SANDBOX_VIOLATION,
};
pub use registry::{McpApp, McpRegistry};
pub use sandbox::{
    validate_app_manifest, AppCapability, AppManifest, McpSandboxError, RuntimeCapabilityGuard,
};

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::boxed::Box;
    use alloc::collections::BTreeMap;
    use alloc::string::{String, ToString};
    use alloc::vec;
    use alloc::vec::Vec;

    struct MaliciousUiApp {
        manifest: AppManifest,
    }

    impl MaliciousUiApp {
        fn new(cap: AppCapability) -> Self {
            Self {
                manifest: AppManifest::new(
                    "rogue_ui",
                    "Rogue Window App",
                    "0.1.0",
                    vec![AppCapability::ExposeMcpTools, cap],
                ),
            }
        }
    }

    impl McpApp for MaliciousUiApp {
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
    fn manifest_validation_accepts_headless_mcp_apps() {
        let notes = AppManifest::parse(NOTES_MANIFEST_MCP).expect("parse notes.mcp");
        let tasks = AppManifest::parse(TASKS_MANIFEST_MCP).expect("parse tasks.mcp");
        let syspulse = AppManifest::parse(SYSPULSE_MANIFEST_MCP).expect("parse syspulse.mcp");

        assert!(validate_app_manifest(&notes).is_ok());
        assert!(validate_app_manifest(&tasks).is_ok());
        assert!(validate_app_manifest(&syspulse).is_ok());

        let roundtrip = AppManifest::parse(&notes.to_mcp_config()).expect("roundtrip parse");
        assert_eq!(roundtrip, notes);
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

            // Registry must also reject registration of an app declaring UI capabilities
            let mut reg = McpRegistry::new();
            let reg_err = reg
                .register_app(Box::new(MaliciousUiApp::new(forbidden_ui)))
                .expect_err("registry must reject UI app");
            assert!(matches!(
                reg_err,
                McpSandboxError::UiCapabilityForbidden { .. }
            ));
        }

        // ShellExecution must be rejected with ShellCapabilityForbidden
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
    fn runtime_capability_guard_enforces_zero_ui_and_scoped_storage() {
        let manifest = AppManifest::parse(NOTES_MANIFEST_MCP).unwrap();
        let guard = RuntimeCapabilityGuard::new(manifest).expect("valid guard");

        assert!(guard
            .enforce_capability(&AppCapability::ExposeMcpTools)
            .is_ok());
        assert!(guard
            .enforce_capability(&AppCapability::ScopedStorage(
                "/var/notes/index.db".to_string()
            ))
            .is_ok());
        assert!(guard
            .enforce_capability(&AppCapability::ScopedStorage("/etc/passwd".to_string()))
            .is_err());

        // Attempting to invoke UI or shell at runtime is strictly forbidden
        assert_eq!(
            guard.enforce_capability(&AppCapability::CreateWindow),
            Err(McpSandboxError::UiCapabilityForbidden {
                app_id: "notes".to_string(),
                capability: AppCapability::CreateWindow,
            })
        );
        assert_eq!(
            guard.enforce_capability(&AppCapability::DirectFramebuffer),
            Err(McpSandboxError::UiCapabilityForbidden {
                app_id: "notes".to_string(),
                capability: AppCapability::DirectFramebuffer,
            })
        );
        assert_eq!(
            guard.enforce_capability(&AppCapability::RawInputCapture),
            Err(McpSandboxError::UiCapabilityForbidden {
                app_id: "notes".to_string(),
                capability: AppCapability::RawInputCapture,
            })
        );
        assert_eq!(
            guard.enforce_capability(&AppCapability::ShellExecution),
            Err(McpSandboxError::ShellCapabilityForbidden {
                app_id: "notes".to_string(),
                capability: AppCapability::ShellExecution,
            })
        );
    }

    #[test]
    fn mcp_value_and_json_rpc_roundtrip() {
        let req = McpRequest::new(
            42,
            methods::TOOLS_CALL,
            mcp_map(&[
                ("name", McpValue::Text("notes.create".to_string())),
                (
                    "arguments",
                    McpValue::Map(mcp_map(&[
                        ("title", McpValue::Text("Kernel Spec\nLine 2".to_string())),
                        ("body", McpValue::Text("Ring-3 isolation \"checked\"".to_string())),
                    ])),
                ),
            ]),
        );

        let json = req.to_json();
        let decoded = McpRequest::from_json(&json).expect("decode JSON-RPC request");
        assert_eq!(decoded, req);
    }

    #[test]
    fn end_to_end_notes_tasks_syspulse_over_json_rpc() {
        let mut registry = McpRegistry::new();
        register_builtin_apps(&mut registry).expect("register builtin apps");
        assert_eq!(registry.app_count(), 3);

        // 1. Verify initialize
        let init_resp = McpResponse::from_json(&registry.handle_json_rpc(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        ))
        .unwrap();
        assert!(init_resp.is_ok());

        // 2. Verify tools/list aggregates all 8 tools across notes, tasks, syspulse
        let tools_list_resp = McpResponse::from_json(&registry.handle_json_rpc(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
        ))
        .unwrap();
        let tools_arr = tools_list_resp
            .result
            .as_ref()
            .unwrap()
            .as_map()
            .unwrap()
            .get("tools")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(tools_arr.len(), 8);

        // 3. NotesVaultApp: create, search, read mcp://notes/all, delete
        let create_note_json = r#"{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"notes.create","arguments":{"title":"Architecture","body":"Zero App-Owned UI via MCP","tag":"design"}}}"#;
        let create_resp =
            McpResponse::from_json(&registry.handle_json_rpc(create_note_json)).unwrap();
        assert!(create_resp.is_ok());

        let search_note_json = r#"{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"name":"notes.search","arguments":{"query":"zero app-owned"}}}"#;
        let search_resp =
            McpResponse::from_json(&registry.handle_json_rpc(search_note_json)).unwrap();
        let count = search_resp
            .result
            .as_ref()
            .unwrap()
            .as_map()
            .unwrap()
            .get("count")
            .unwrap()
            .as_i64()
            .unwrap();
        assert_eq!(count, 1);

        let notes_res = registry
            .read_resource("mcp://notes/all")
            .expect("read mcp://notes/all");
        let parsed_notes = McpValue::from_json(&notes_res.text).unwrap();
        assert_eq!(
            parsed_notes.as_map().unwrap().get("total").unwrap().as_i64(),
            Some(1)
        );

        let del_note_json = r#"{"jsonrpc":"2.0","id":12,"method":"tools/call","params":{"name":"notes.delete","arguments":{"id":1}}}"#;
        let del_resp = McpResponse::from_json(&registry.handle_json_rpc(del_note_json)).unwrap();
        assert_eq!(
            del_resp
                .result
                .unwrap()
                .as_map()
                .unwrap()
                .get("deleted")
                .unwrap()
                .as_bool(),
            Some(true)
        );

        // 4. TaskBoardApp: add, complete, list, read mcp://tasks/summary
        let add_task_json = r#"{"jsonrpc":"2.0","id":20,"method":"tools/call","params":{"name":"tasks.add","arguments":{"title":"Ship Phase 7 MCP SDK","priority":"critical"}}}"#;
        let add_task_resp =
            McpResponse::from_json(&registry.handle_json_rpc(add_task_json)).unwrap();
        assert!(add_task_resp.is_ok());

        let complete_task_json = r#"{"jsonrpc":"2.0","id":21,"method":"tools/call","params":{"name":"tasks.complete","arguments":{"id":1}}}"#;
        let complete_task_resp =
            McpResponse::from_json(&registry.handle_json_rpc(complete_task_json)).unwrap();
        assert!(complete_task_resp.is_ok());

        let list_pending_json = r#"{"jsonrpc":"2.0","id":22,"method":"tools/call","params":{"name":"tasks.list","arguments":{"include_completed":false}}}"#;
        let list_pending_resp =
            McpResponse::from_json(&registry.handle_json_rpc(list_pending_json)).unwrap();
        assert_eq!(
            list_pending_resp
                .result
                .unwrap()
                .as_map()
                .unwrap()
                .get("count")
                .unwrap()
                .as_i64(),
            Some(0)
        );

        let task_summary_json = r#"{"jsonrpc":"2.0","id":23,"method":"resources/read","params":{"uri":"mcp://tasks/summary"}}"#;
        let task_summary_resp =
            McpResponse::from_json(&registry.handle_json_rpc(task_summary_json)).unwrap();
        assert!(task_summary_resp.is_ok());

        // 5. SysPulseApp: record_metric, snapshot, read mcp://syspulse/metrics
        let record_metric_json = r#"{"jsonrpc":"2.0","id":30,"method":"tools/call","params":{"name":"syspulse.record_metric","arguments":{"metric":"npu_infer_ms","value":4,"unit":"ms"}}}"#;
        let record_resp =
            McpResponse::from_json(&registry.handle_json_rpc(record_metric_json)).unwrap();
        assert!(record_resp.is_ok());

        let snapshot_json = r#"{"jsonrpc":"2.0","id":31,"method":"tools/call","params":{"name":"syspulse.snapshot","arguments":{}}}"#;
        let snapshot_resp =
            McpResponse::from_json(&registry.handle_json_rpc(snapshot_json)).unwrap();
        assert_eq!(
            snapshot_resp
                .result
                .unwrap()
                .as_map()
                .unwrap()
                .get("metric_count")
                .unwrap()
                .as_i64(),
            Some(4)
        );

        let syspulse_res = registry
            .read_resource("mcp://syspulse/metrics")
            .expect("read mcp://syspulse/metrics");
        let parsed_metrics = McpValue::from_json(&syspulse_res.text).unwrap();
        assert_eq!(
            parsed_metrics
                .as_map()
                .unwrap()
                .get("count")
                .unwrap()
                .as_i64(),
            Some(4)
        );

        // 6. Prompts: list and get across all 3 apps
        let prompts = registry.list_prompts();
        assert_eq!(prompts.len(), 3);

        let prompt_json = r#"{"jsonrpc":"2.0","id":40,"method":"prompts/get","params":{"name":"syspulse.diagnose","arguments":{"subsystem":"virtio-gpu"}}}"#;
        let prompt_resp =
            McpResponse::from_json(&registry.handle_json_rpc(prompt_json)).unwrap();
        let rendered = prompt_resp
            .result
            .unwrap()
            .as_map()
            .unwrap()
            .get("prompt")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();
        assert!(rendered.contains("virtio-gpu"));
    }
}
