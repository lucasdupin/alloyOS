use alloyos_forge::provider::ToolCall;
use alloyos_welder::{AgentUiSynthesizer, NativeToolRegistry, SharedMapVfsBackend, ToolVfsBackend};
use serde_json::{json, Value};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

fn execute(registry: &mut NativeToolRegistry, name: &str, args: Value) -> Result<Value, String> {
    let output = registry
        .execute_tool(
            &ToolCall::new("review", name, args.to_string()),
            &mut AgentUiSynthesizer::new(),
        )
        .map_err(|error| format!("{error:?}"))?;
    serde_json::from_str(&output.output_json).map_err(|error| error.to_string())
}

fn approve_install(registry: &mut NativeToolRegistry, args:Value) -> Result<Value,String> {
    let proposal = execute(registry,"smithery_install",args)?;
    assert_eq!(proposal["awaiting_user_approval"],true);
    let id = proposal["proposal"]["id"].as_str().unwrap();
    let output = registry.confirm_remote_install(id,true).map_err(|error|error.to_string())?;
    serde_json::from_str(&output.output_json).map_err(|error|error.to_string())
}

struct NoCredentialsBroker;
impl alloyos_welder::accounts::AccountBroker for NoCredentialsBroker {
    fn start(&mut self, request:&alloyos_mcp::account::AccountRequest)->std::task::Poll<Result<alloyos_mcp::account::AccountResponse,alloyos_mcp::account::AccountError>> {
        std::task::Poll::Ready(Ok(alloyos_mcp::account::AccountResponse {request_id:request.request_id,app_id:request.app_id.clone(),endpoint:request.endpoint.clone(),outcome:alloyos_mcp::account::AccountOutcome::Disconnected{remote_revoked:false}}))
    }
    fn poll(&mut self)->std::task::Poll<Result<alloyos_mcp::account::AccountResponse,alloyos_mcp::account::AccountError>> {panic!("fixture disconnect is immediate")}
    fn cancel(&mut self){}
}
fn approve_removal(registry:&mut NativeToolRegistry,args:Value)->Result<Value,String>{
    registry.set_account_broker(Box::new(NoCredentialsBroker)).unwrap();
    let proposal=execute(registry,"smithery_uninstall",args)?;
    let output=registry.confirm_remote_install(proposal["proposal"]["id"].as_str().unwrap(),true).map_err(|error|error.to_string())?;
    serde_json::from_str(&output.output_json).map_err(|error|error.to_string())
}

#[test]
fn search_preserves_empty_results_and_encodes_query() {
    let mut registry = NativeToolRegistry::new();
    registry.register_http_response(
        "https://api.smithery.ai/servers?q=github",
        r#"{"servers":[],"pagination":{"totalCount":0}}"#,
    );
    let result = execute(&mut registry, "smithery_search", json!({"query":"github"})).unwrap();
    assert_eq!(result["count"], 0);
    registry.register_http_response(
        "https://api.smithery.ai/servers?q=a%26b%20%2F%E9%9B%AA",
        r#"{"servers":[]}"#,
    );
    assert!(execute(&mut registry, "smithery_search", json!({"query":"a&b /雪"})).is_ok());
}

#[test]
fn offline_and_malformed_registry_responses_are_errors() {
    let mut registry = NativeToolRegistry::new();
    assert!(execute(&mut registry, "smithery_search", json!({"query":"github"})).is_err());
    registry.register_http_response(
        "https://api.smithery.ai/servers?q=github",
        r#"{"error":"Unauthorized"}"#,
    );
    assert!(execute(&mut registry, "smithery_search", json!({"query":"github"})).is_err());
    assert!(execute(
        &mut registry,
        "smithery_install",
        json!({"server":"unknown"})
    )
    .is_err());
    assert!(registry.vfs_read("/apps/unknown/unknown.mcp").is_none());
}

#[test]
fn invalid_ids_cannot_install_or_uninstall_packages() {
    let backend = SharedMapVfsBackend::shared();
    backend
        .borrow_mut()
        .files
        .insert("/apps/keep/keep.mcp".into(), "keep me".into());
    let mut registry = NativeToolRegistry::new();
    registry.set_vfs_backend(backend.clone());
    for id in ["../keep", "x/../keep", "", "bad\nname"] {
        assert!(execute(
            &mut registry,
            "smithery_install",
            json!({"server":"github","app_id":id})
        )
        .is_err());
        assert!(execute(&mut registry, "smithery_uninstall", json!({"app_id":id})).is_err());
    }
    assert_eq!(backend.borrow().files["/apps/keep/keep.mcp"], "keep me");
}

#[test]
fn install_never_overwrites_an_existing_package() {
    let backend = SharedMapVfsBackend::shared();
    backend
        .borrow_mut()
        .files
        .insert("/apps/github/github.mcp".into(), "keep me".into());
    let mut registry = NativeToolRegistry::new();
    registry.set_vfs_backend(backend.clone());
    assert!(execute(
        &mut registry,
        "smithery_install",
        json!({"server":"github"})
    )
    .is_err());
    assert_eq!(backend.borrow().files["/apps/github/github.mcp"], "keep me");
}

fn register_details(registry: &mut NativeToolRegistry) {
    registry.register_http_response("https://api.smithery.ai/servers/acme%2Fservice",
        r#"{"qualifiedName":"acme/service","displayName":"Service","remote":true,"deploymentUrl":"https://service.example/mcp","tools":[{"name":"echo","inputSchema":{"type":"object","properties":{"value":{"type":"number","minimum":0.1}},"required":["value"]}}]}"#);
}

#[derive(Debug, Default)]
struct FailingBackend {
    files: BTreeMap<String, String>,
    fail_write: Option<String>,
    fail_delete: Option<String>,
}
impl ToolVfsBackend for FailingBackend {
    fn read_file(&mut self, path: &str) -> Option<String> {
        self.files.get(path).cloned()
    }
    fn write_file(&mut self, path: &str, content: &str) -> Result<(), String> {
        if self.fail_write.as_deref() == Some(path) {
            self.fail_write = None;
            return Err("disk write failed".into());
        }
        self.files.insert(path.into(), content.into());
        Ok(())
    }
    fn delete_file(&mut self, path: &str) -> Result<bool, String> {
        if self.fail_delete.as_deref() == Some(path) {
            self.fail_delete = None;
            return Err("disk delete failed".into());
        }
        Ok(self.files.remove(path).is_some())
    }
    fn list_dir(&mut self, path: &str) -> Vec<String> {
        let prefix = format!("{path}/");
        self.files
            .keys()
            .filter(|key| key.starts_with(&prefix))
            .cloned()
            .collect()
    }
}

#[test]
fn failed_install_rolls_back_writes_and_registration() {
    for path in ["/apps/service/tools.json", "/proc/app-packages.json"] {
        let backend = Rc::new(RefCell::new(FailingBackend::default()));
        let mut registry = NativeToolRegistry::new();
        registry.set_vfs_backend(backend.clone());
        backend.borrow_mut().fail_write = Some(path.into());
        register_details(&mut registry);
        let result = approve_install(&mut registry, json!({"server":"acme/service"}));
        assert!(result.is_err());
        assert!(!backend
            .borrow()
            .files
            .keys()
            .any(|path| path.starts_with("/apps/service/")));
        assert!(registry.mcp_registry().guard_for("service").is_none());
    }
}

#[test]
fn failed_uninstall_restores_package_and_success_removes_nested_files() {
    let backend = Rc::new(RefCell::new(FailingBackend::default()));
    let mut registry = NativeToolRegistry::new();
    registry.set_vfs_backend(backend.clone());
    register_details(&mut registry);
    approve_install(&mut registry, json!({"server":"acme/service"}))
    .unwrap();
    backend.borrow_mut().files.insert(
        "/apps/service/assets/nested/data.txt".into(),
        "nested".into(),
    );
    let original = backend.borrow().files.clone();
    backend.borrow_mut().fail_delete = Some("/apps/service/service.mcp".into());
    assert!(approve_removal(&mut registry,json!({"app_id":"service"}))
    .is_err());
    for (path, text) in original
        .iter()
        .filter(|(path, _)| path.starts_with("/apps/service/"))
    {
        assert_eq!(backend.borrow().files.get(path), Some(text));
    }
    assert!(registry.mcp_registry().guard_for("service").is_some());
    let result = approve_removal(&mut registry,json!({"app_id":"service"}))
    .unwrap();
    assert_eq!(result["deleted_files"], 4);
    assert!(!backend
        .borrow()
        .files
        .keys()
        .any(|path| path.starts_with("/apps/service/")));
    assert!(registry.mcp_registry().guard_for("service").is_none());
}

#[test]
fn metadata_errors_and_manifest_injection_leave_no_package() {
    for details in [
        r#"{"qualifiedName":"acme/service","displayName":"Service","remote":true,"deploymentUrl":"https://service.example/mcp","tools":null}"#,
        r#"{"qualifiedName":"acme/service","displayName":"Bad\ncapabilities=ShellExecution","remote":true,"deploymentUrl":"https://service.example/mcp","tools":[]}"#,
        r#"{"qualifiedName":"acme/service","displayName":"Service","remote":false,"tools":[]}"#,
    ] {
        let mut registry = NativeToolRegistry::new();
        registry.register_http_response("https://api.smithery.ai/servers/acme%2Fservice", details);
        assert!(execute(
            &mut registry,
            "smithery_install",
            json!({"server":"acme/service"})
        )
        .is_err());
        assert!(registry.vfs_read("/apps/service/service.mcp").is_none());
    }
}

#[test]
fn pagination_is_forwarded_and_returned_without_fabricating_results() {
    let mut registry = NativeToolRegistry::new();
    registry.register_http_response(
        "https://api.smithery.ai/servers?q=github&page=2&pageSize=50",
        r#"{"servers":[],"pagination":{"currentPage":2,"totalPages":2}}"#,
    );
    let result = execute(
        &mut registry,
        "smithery_search",
        json!({"query":"github","page":2,"page_size":50}),
    )
    .unwrap();
    assert_eq!(result["pagination"]["currentPage"], 2);
    assert_eq!(result["count"], 0);
    assert!(execute(
        &mut registry,
        "smithery_search",
        json!({"query":"github","page_size":101})
    )
    .is_err());
}
