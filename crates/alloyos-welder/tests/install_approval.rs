use alloyos_forge::provider::ToolCall;
use alloyos_welder::{AgentUiSynthesizer, NativeToolRegistry, SharedMapVfsBackend};
use serde_json::{json, Value};
use std::{cell::RefCell, rc::Rc};
fn registry() -> (NativeToolRegistry, Rc<RefCell<SharedMapVfsBackend>>) {
    let backend = SharedMapVfsBackend::shared();
    (
        NativeToolRegistry::with_vfs_backend(backend.clone()),
        backend,
    )
}
fn execute(registry: &mut NativeToolRegistry, name: &str, args: Value) -> Result<Value, String> {
    let output = registry
        .execute_tool(
            &ToolCall::new("proposal", name, args.to_string()),
            &mut AgentUiSynthesizer::new(),
        )
        .map_err(|error| error.to_string())?;
    serde_json::from_str(&output.output_json).map_err(|error| error.to_string())
}
fn propose(registry: &mut NativeToolRegistry) -> Value {
    execute(
        registry,
        "mcp_install_remote",
        json!({"app_id":"mail","name":"Mail","endpoint":"https://mail.example/mcp"}),
    )
    .unwrap()
}
#[test]
fn direct_remote_install_writes_nothing_before_native_decision_and_never_authorizes_account() {
    let (mut registry, backend) = registry();
    let before = backend.borrow().files.clone();
    let result = propose(&mut registry);
    assert_eq!(result["awaiting_user_approval"], true);
    assert_eq!(result["installed"], false);
    assert_eq!(backend.borrow().files, before);
    assert!(registry.mcp_registry().guard_for("mail").is_none());
    let review = registry.pending_remote_install().unwrap();
    assert_eq!(review.endpoint, "https://mail.example/mcp");
    assert!(review.publisher.contains("not verified"));
    let output = registry.confirm_remote_install(&review.id, true).unwrap();
    let result: Value = serde_json::from_str(&output.output_json).unwrap();
    assert_eq!(result["installed"], true);
    assert_eq!(result["account_authorized"], false);
    assert_eq!(result["connectivity_verified"], false);
    assert_eq!(result["connection_status"], "not_connected");
    assert_eq!(backend.borrow().files["/apps/mail/tools.json"], "[]");
    assert!(backend.borrow().files["/apps/mail/mail.mcp"].contains("https://mail.example/mcp"));
    assert!(!backend.borrow().files.contains_key("/apps/mail/main.js"));
    assert!(registry.mcp_registry().guard_for("mail").is_some());
    assert!(registry.pending_remote_install().is_none());
    assert!(registry.confirm_remote_install(&review.id, true).is_err());
}
#[test]
fn cancel_and_stale_decisions_never_modify_packages_or_approve_another_request() {
    let (mut registry, backend) = registry();
    let before = backend.borrow().files.clone();
    propose(&mut registry);
    let first = registry.pending_remote_install().unwrap();
    assert!(registry
        .confirm_remote_install("install-stale", true)
        .is_err());
    assert_eq!(registry.pending_remote_install().unwrap().id, first.id);
    registry.confirm_remote_install(&first.id, false).unwrap();
    assert_eq!(backend.borrow().files, before);
    propose(&mut registry);
    let second = registry.pending_remote_install().unwrap();
    assert_ne!(first.id, second.id);
    assert!(registry.confirm_remote_install(&first.id, true).is_err());
    assert_eq!(registry.pending_remote_install().unwrap().id, second.id);
    registry.confirm_remote_install(&second.id, false).unwrap();
    assert_eq!(backend.borrow().files, before);
}
#[test]
fn model_boolean_unknown_approval_tool_and_second_proposal_cannot_self_approve() {
    let (mut registry, backend) = registry();
    assert!(execute(
        &mut registry,
        "mcp_install_remote",
        json!({"app_id":"mail","name":"Mail","endpoint":"https://mail.example/mcp","approved":true})
    )
    .is_err());
    propose(&mut registry);
    let review = registry.pending_remote_install().unwrap();
    assert!(execute(
        &mut registry,
        "mcp_confirm_remote_install",
        json!({"id":review.id,"approved":true})
    )
    .is_err());
    assert!(execute(
        &mut registry,
        "mcp_install_remote",
        json!({"app_id":"other","name":"Other","endpoint":"https://other.example/mcp"})
    )
    .is_err());
    assert_eq!(registry.pending_remote_install().unwrap().app_id, "mail");
    assert!(!backend
        .borrow()
        .files
        .keys()
        .any(|path| path.starts_with("/apps/mail/")));
}
#[test]
fn package_collision_between_review_and_approval_preserves_existing_content() {
    let (mut registry, backend) = registry();
    propose(&mut registry);
    let review = registry.pending_remote_install().unwrap();
    backend
        .borrow_mut()
        .files
        .insert("/apps/mail/private.txt".into(), "existing content".into());
    let before = backend.borrow().files.clone();
    assert!(registry.confirm_remote_install(&review.id, true).is_err());
    assert_eq!(backend.borrow().files, before);
    assert!(registry.pending_remote_install().is_none());
}
#[test]
fn smithery_is_only_a_metadata_adapter_and_approval_uses_the_reviewed_snapshot() {
    let (mut registry, backend) = registry();
    registry.register_http_response("https://api.smithery.ai/servers/vendor%2Fmail",r#"{"qualifiedName":"vendor/mail","displayName":"Mail","remote":true,"deploymentUrl":"https://mail.example/mcp"}"#);
    let result = execute(
        &mut registry,
        "smithery_install",
        json!({"server":"vendor/mail"}),
    )
    .unwrap();
    assert_eq!(result["installed"], false);
    let review = registry.pending_remote_install().unwrap();
    assert!(review.source.contains("Smithery"));
    assert!(backend.borrow().files.get("/apps/mail/mail.mcp").is_none());
    registry.register_http_response("https://api.smithery.ai/servers/vendor%2Fmail",r#"{"qualifiedName":"vendor/mail","displayName":"Changed","remote":true,"deploymentUrl":"https://changed.example/mcp"}"#);
    registry.confirm_remote_install(&review.id, true).unwrap();
    let manifest = &backend.borrow().files["/apps/mail/mail.mcp"];
    assert!(manifest.contains("https://mail.example/mcp"));
    assert!(!manifest.contains("changed.example"));
}
#[test]
fn direct_endpoint_policy_rejects_local_execution_and_unusable_addresses_without_proposals() {
    let (mut registry, backend) = registry();
    for endpoint in [
        "stdio://mail",
        "http://localhost:3000/mcp",
        "http://mail.example/mcp",
        "https://127.0.0.1/mcp",
        "https://host.local/mcp",
        "https://user:secret@mail.example/mcp",
    ] {
        assert!(
            execute(
                &mut registry,
                "mcp_install_remote",
                json!({"app_id":"mail","name":"Mail","endpoint":endpoint})
            )
            .is_err(),
            "{endpoint}"
        );
        assert!(registry.pending_remote_install().is_none());
    }
    assert!(!backend
        .borrow()
        .files
        .keys()
        .any(|path| path.starts_with("/apps/mail/")));
}

#[test]
fn ordinary_file_and_script_tools_cannot_bypass_install_approval_or_edit_existing_packages() {
    let (mut registry, backend) = registry();
    backend
        .borrow_mut()
        .files
        .insert("/apps/existing/keep.txt".into(), "unchanged".into());
    for path in [
        "/apps/mail/mail.mcp",
        "/tmp/../apps/mail/mail.mcp",
        "/apps/existing/keep.txt",
    ] {
        assert!(execute(
            &mut registry,
            "fs_write",
            json!({"path":path,"content":"bypass"})
        )
        .is_err());
        assert!(execute(
            &mut registry,
            "script_create",
            json!({"path":path,"source":"1 + 1;"})
        )
        .is_err());
    }
    for (index, source) in [
        "AlloyOS.fs.write('/apps/mail/mail.mcp', 'bypass');",
        "AlloyOS.fs.write('/tmp/../apps/mail/mail.mcp', 'bypass');",
        "AlloyOS.fs.delete('/apps/existing/keep.txt');",
    ]
    .iter()
    .enumerate()
    {
        let path = format!("/scripts/bypass-{index}.js");
        execute(
            &mut registry,
            "script_create",
            json!({"path":path,"source":source}),
        )
        .unwrap();
        let result = execute(&mut registry, "script_run", json!({"path":path}));
        assert!(result.is_err() || result.unwrap()["ok"] == false);
    }
    assert_eq!(
        backend.borrow().files["/apps/existing/keep.txt"],
        "unchanged"
    );
    assert!(!backend
        .borrow()
        .files
        .keys()
        .any(|path| path.starts_with("/apps/mail/")));
    assert!(registry.pending_remote_install().is_none());
}

#[test]
fn attaching_a_different_volume_invalidates_any_prior_install_review() {
    let (mut registry, original) = registry();
    propose(&mut registry);
    let review = registry.pending_remote_install().unwrap();
    let replacement = SharedMapVfsBackend::shared();
    registry.set_vfs_backend(replacement.clone());
    assert!(registry.pending_remote_install().is_none());
    assert!(registry.confirm_remote_install(&review.id, true).is_err());
    assert!(!original
        .borrow()
        .files
        .keys()
        .any(|path| path.starts_with("/apps/mail/")));
    assert!(!replacement
        .borrow()
        .files
        .keys()
        .any(|path| path.starts_with("/apps/mail/")));
}

fn sdk_app() -> (NativeToolRegistry, Rc<RefCell<SharedMapVfsBackend>>) {
    let backend = SharedMapVfsBackend::shared();
    backend.borrow_mut().files.extend([
        (
            "/apps/notes/notes.mcp".into(),
            "name=Notes\nversion=1\ncapabilities=ExposeMcpTools\n".into(),
        ),
        (
            "/apps/notes/main.js".into(),
            "function callTool(name,args) { return '{}'; }".into(),
        ),
        ("/apps/notes/tools.json".into(), "[]".into()),
        (
            "/apps/notes/assets/nested/readme.txt".into(),
            "asset".into(),
        ),
    ]);
    (
        NativeToolRegistry::with_vfs_backend(backend.clone()),
        backend,
    )
}
#[test]
fn native_sdk_package_removal_requires_real_review_and_preserves_other_apps() {
    let (mut registry, backend) = sdk_app();
    assert!(registry.mcp_registry().guard_for("notes").is_some());
    let before = backend.borrow().files.clone();
    let result = execute(
        &mut registry,
        "mcp_uninstall_app",
        json!({"app_id":"notes"}),
    )
    .unwrap();
    assert_eq!(result["removed"], false);
    assert_eq!(backend.borrow().files, before);
    let review = registry.pending_remote_install().unwrap();
    assert_eq!(
        review.action,
        alloyos_mcp::install::PackageChangeAction::Remove
    );
    assert!(review.endpoint.is_empty());
    assert!(review.source.contains("JavaScript SDK"));
    registry.confirm_remote_install(&review.id, false).unwrap();
    assert_eq!(backend.borrow().files, before);
    execute(
        &mut registry,
        "smithery_uninstall",
        json!({"app_id":"notes"}),
    )
    .unwrap();
    let review = registry.pending_remote_install().unwrap();
    let output = registry.confirm_remote_install(&review.id, true).unwrap();
    let result: Value = serde_json::from_str(&output.output_json).unwrap();
    assert_eq!(result["removed"], true);
    assert_eq!(result["name"], "Notes");
    assert_eq!(result["deleted_files"], 4);
    assert!(!backend
        .borrow()
        .files
        .keys()
        .any(|path| path.starts_with("/apps/notes/")));
    assert!(registry.mcp_registry().guard_for("notes").is_none());
}
#[test]
fn package_changed_after_removal_review_is_not_deleted_and_model_cannot_approve() {
    let (mut registry, backend) = sdk_app();
    assert!(execute(
        &mut registry,
        "mcp_uninstall_app",
        json!({"app_id":"notes","approved":true})
    )
    .is_err());
    execute(
        &mut registry,
        "mcp_uninstall_app",
        json!({"app_id":"notes"}),
    )
    .unwrap();
    let review = registry.pending_remote_install().unwrap();
    backend
        .borrow_mut()
        .files
        .insert("/apps/notes/main.js".into(), "changed after review".into());
    let updated = backend.borrow().files.clone();
    assert!(registry.confirm_remote_install(&review.id, true).is_err());
    assert_eq!(backend.borrow().files, updated);
}
struct RemovalBroker {
    request: Option<alloyos_mcp::account::AccountRequest>,
    fail: bool,
    profile_fail: bool,
}
impl alloyos_welder::accounts::AccountBroker for RemovalBroker {
    fn forget_profile(
        &mut self,
        _: &str,
        _: &str,
    ) -> Result<(), alloyos_mcp::account::AccountError> {
        if self.profile_fail {
            Err(alloyos_mcp::account::AccountError::StorageUnavailable)
        } else {
            Ok(())
        }
    }
    fn start(
        &mut self,
        request: &alloyos_mcp::account::AccountRequest,
    ) -> std::task::Poll<
        Result<alloyos_mcp::account::AccountResponse, alloyos_mcp::account::AccountError>,
    > {
        assert!(matches!(
            request.operation,
            alloyos_mcp::account::AccountOperation::Disconnect { .. }
        ));
        self.request = Some(request.clone());
        std::task::Poll::Pending
    }
    fn poll(
        &mut self,
    ) -> std::task::Poll<
        Result<alloyos_mcp::account::AccountResponse, alloyos_mcp::account::AccountError>,
    > {
        use alloyos_mcp::account::*;
        let request = self.request.take().unwrap();
        std::task::Poll::Ready(Ok(AccountResponse {
            request_id: request.request_id,
            app_id: request.app_id,
            endpoint: request.endpoint,
            outcome: if self.fail {
                AccountOutcome::Error {
                    error: AccountError::StorageUnavailable,
                }
            } else {
                AccountOutcome::Disconnected {
                    remote_revoked: false,
                }
            },
        }))
    }
    fn cancel(&mut self) {
        self.request = None;
    }
}
#[test]
fn remote_removal_waits_for_account_cleanup_and_does_not_claim_provider_revocation() {
    let (mut registry, backend) = registry();
    propose(&mut registry);
    let review = registry.pending_remote_install().unwrap();
    registry.confirm_remote_install(&review.id, true).unwrap();
    registry
        .set_account_broker(Box::new(RemovalBroker {
            request: None,
            fail: false,
            profile_fail: false,
        }))
        .unwrap();
    execute(&mut registry, "mcp_uninstall_app", json!({"app_id":"mail"})).unwrap();
    let review = registry.pending_remote_install().unwrap();
    let result = registry.confirm_remote_install(&review.id, true).unwrap();
    let result: Value = serde_json::from_str(&result.output_json).unwrap();
    assert_eq!(result["pending_account_cleanup"], true);
    assert!(backend.borrow().files.contains_key("/apps/mail/mail.mcp"));
    assert!(registry.take_package_change_result().is_none());
    registry.poll_account_connections();
    let output = registry.take_package_change_result().unwrap().unwrap();
    let result: Value = serde_json::from_str(&output.output_json).unwrap();
    assert_eq!(result["removed"], true);
    assert_eq!(result["remote_revocation_confirmed"], false);
    assert_eq!(result["account_credentials_removed"], true);
    assert!(!backend.borrow().files.contains_key("/apps/mail/mail.mcp"));
}
#[test]
fn failed_account_cleanup_preserves_the_package_and_reports_removal_failure() {
    let (mut registry, backend) = registry();
    propose(&mut registry);
    let review = registry.pending_remote_install().unwrap();
    registry.confirm_remote_install(&review.id, true).unwrap();
    registry
        .set_account_broker(Box::new(RemovalBroker {
            request: None,
            fail: true,
            profile_fail: false,
        }))
        .unwrap();
    execute(&mut registry, "mcp_uninstall_app", json!({"app_id":"mail"})).unwrap();
    let review = registry.pending_remote_install().unwrap();
    registry.confirm_remote_install(&review.id, true).unwrap();
    let before = backend
        .borrow()
        .files
        .iter()
        .filter(|(path, _)| path.starts_with("/apps/mail/"))
        .map(|(path, content)| (path.clone(), content.clone()))
        .collect::<std::collections::BTreeMap<_, _>>();
    registry.poll_account_connections();
    assert!(registry.take_package_change_result().unwrap().is_err());
    for (path, content) in before {
        assert_eq!(backend.borrow().files[&path], content);
    }
    assert!(registry.mcp_registry().guard_for("mail").is_some());
}

#[test]
fn private_profile_cleanup_failure_preserves_package_after_token_disconnect() {
    let (mut registry, backend) = registry();
    propose(&mut registry);
    let review = registry.pending_remote_install().unwrap();
    registry.confirm_remote_install(&review.id, true).unwrap();
    registry
        .set_account_broker(Box::new(RemovalBroker {
            request: None,
            fail: false,
            profile_fail: true,
        }))
        .unwrap();
    execute(&mut registry, "mcp_uninstall_app", json!({"app_id":"mail"})).unwrap();
    let review = registry.pending_remote_install().unwrap();
    registry.confirm_remote_install(&review.id, true).unwrap();
    registry.poll_account_connections();
    let error = registry
        .take_package_change_result()
        .unwrap()
        .unwrap_err()
        .to_string();
    assert!(error.contains("configuration cleanup failed"));
    assert!(backend.borrow().files.contains_key("/apps/mail/mail.mcp"));
}
