//! Registry-independent remote registration, gated by a real display decision.
//! Model tools can propose an immutable package snapshot; only the trusted UI
//! event route can commit it. Proposals are deliberately not persisted/replayed.
use super::*;
use alloyos_mcp::install::{PackageChangeAction, PendingRemoteInstall};

#[derive(Clone)]
struct Candidate {
    review: PendingRemoteInstall,
    files: BTreeMap<String, String>,
    directories: BTreeSet<String>,
}
#[derive(Default)]
pub(super) struct InstallState {
    next_id: u64,
    pending: Option<Candidate>,
    approved_removal: Option<Candidate>,
    completed: Option<Result<ToolExecutionOutput, AiError>>,
}

impl NativeToolRegistry {
    pub(super) fn clear_pending_remote_install(&mut self) {
        let mut state = self.remote_installs.borrow_mut();
        state.pending = None;
        state.approved_removal = None;
        state.completed = None;
    }
    pub fn pending_remote_install(&self) -> Option<PendingRemoteInstall> {
        self.remote_installs
            .borrow()
            .pending
            .as_ref()
            .map(|candidate| candidate.review.clone())
    }
    fn remote_package_available(&mut self, app_id: &str) -> Result<(), String> {
        validate_package_id(app_id)?;
        let root = format!("/apps/{app_id}/");
        let snapshot = self.app_snapshot()?;
        if snapshot.files.keys().any(|path| path.starts_with(&root))
            || snapshot
                .directories
                .iter()
                .any(|path| path == root.trim_end_matches('/') || path.starts_with(&root))
            || self.mcp_registry.borrow().guard_for(app_id).is_some()
        {
            return Err(format!("package '{app_id}' already exists"));
        }
        Ok(())
    }
    pub(super) fn propose_remote_install(
        &mut self,
        app_id: &str,
        name: &str,
        endpoint: &str,
        source: &str,
        publisher: &str,
        tools: &str,
    ) -> Result<serde_json::Value, String> {
        if self.remote_installs.borrow().pending.is_some()
            || self.remote_installs.borrow().approved_removal.is_some()
        {
            return Err("another installation is awaiting user approval".into());
        }
        self.remote_package_available(app_id)?;
        if name.trim().is_empty()
            || name.len() > 128
            || name.chars().any(char::is_control)
            || source.len() > 256
            || source.chars().any(char::is_control)
            || publisher.len() > 256
            || publisher.chars().any(char::is_control)
            || !crate::remote_discovery::hosted_endpoint(endpoint)
        {
            return Err("invalid remote app metadata or unsupported hosted HTTPS endpoint".into());
        }
        alloyos_mcp::parse_remote_tools(tools)?;
        let manifest = alloyos_mcp::AppManifest::new_remote(
            app_id,
            name,
            "1.0.0",
            vec![
                alloyos_mcp::AppCapability::ExposeMcpTools,
                alloyos_mcp::AppCapability::ExposeSkills,
                alloyos_mcp::AppCapability::ScopedNetwork,
            ],
            endpoint,
            Some("streamable-http".into()),
        );
        alloyos_mcp::validate_app_manifest(&manifest)
            .map_err(|error| error.to_mcp_error().message)?;
        let root = format!("/apps/{app_id}/");
        let files = BTreeMap::from([
            (format!("{root}{app_id}.mcp"), manifest.to_mcp_config()),
            (format!("{root}tools.json"), tools.into()),
            (format!("{root}SKILL.md"), format!("# {name}\n\nWeb-hosted remote MCP application. Installation only registers its endpoint; use the account connection tools to verify connectivity, discover current tool schemas, and authorize an account when required. Never claim readiness from installation alone.\n")),
        ]);
        let mut validation = alloyos_mcp::PackageService::new();
        validation.apply_snapshot(alloyos_mcp::PackageSnapshot {
            files: files.clone(),
            directories: Vec::new(),
        });
        if !validation.status(app_id).is_some_and(|status| status.ready) {
            return Err("remote package failed validation".into());
        }
        let mut state = self.remote_installs.borrow_mut();
        state.next_id = state
            .next_id
            .checked_add(1)
            .ok_or("installation proposal ID exhausted")?;
        let review = PendingRemoteInstall {
            action: PackageChangeAction::Install,
            id: format!("install-{}", state.next_id),
            app_id: app_id.into(),
            name: name.into(),
            endpoint: endpoint.into(),
            source: source.into(),
            publisher: publisher.into(),
        };
        if !review.is_valid() {
            return Err("package review metadata is invalid".into());
        }
        state.pending = Some(Candidate {
            review: review.clone(),
            files,
            directories: BTreeSet::new(),
        });
        Ok(
            serde_json::json!({"ok":true,"installed":false,"awaiting_user_approval":true,"proposal":review,"connection_status":"not_connected","connectivity_verified":false}),
        )
    }
    pub(super) fn direct_remote_install(
        &mut self,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let args_map = args
            .as_object()
            .ok_or("installation arguments must be an object")?;
        if args_map
            .keys()
            .any(|key| !matches!(key.as_str(), "app_id" | "name" | "endpoint"))
        {
            return Err(
                "unsupported installation argument; approval requires the native Install control"
                    .into(),
            );
        }
        self.propose_remote_install(
            required_string(args, "app_id")?,
            required_string(args, "name")?,
            required_string(args, "endpoint")?,
            "Direct hosted endpoint",
            "Publisher not verified",
            "[]",
        )
    }
    /// Called only by the kernel's authenticated display-event route, never by a
    /// model tool, app binding, text command, or app-supplied approval boolean.
    pub fn confirm_remote_install(
        &mut self,
        id: &str,
        approved: bool,
    ) -> Result<ToolExecutionOutput, AiError> {
        let candidate = {
            let mut state = self.remote_installs.borrow_mut();
            if state
                .pending
                .as_ref()
                .is_none_or(|candidate| candidate.review.id != id)
            {
                return Err(AiError::PolicyViolation(
                    "installation proposal is no longer current".into(),
                ));
            }
            state.pending.take().unwrap()
        };
        if !approved {
            return Ok(ToolExecutionOutput {tool_name:if candidate.review.action == PackageChangeAction::Remove {"mcp_uninstall_app"}else{"mcp_install_remote"}.into(),output_json:serde_json::json!({"ok":true,"installed":false,"removed":false,"cancelled":true,"app_id":candidate.review.app_id,"name":candidate.review.name}).to_string(),synthesized_view:None});
        }
        if candidate.review.action == PackageChangeAction::Remove {
            return self.start_approved_package_removal(candidate);
        }
        self.commit_remote_install(candidate)
            .map_err(AiError::ConfigError)
    }
    fn package_snapshot_for(
        &mut self,
        app_id: &str,
    ) -> Result<(BTreeMap<String, String>, BTreeSet<String>), String> {
        validate_package_id(app_id)?;
        let root = format!("/apps/{app_id}/");
        let snapshot = self.app_snapshot()?;
        let files: BTreeMap<_, _> = snapshot
            .files
            .into_iter()
            .filter(|(path, _)| path.starts_with(&root))
            .collect();
        let mut directories: BTreeSet<_> = snapshot
            .directories
            .into_iter()
            .filter(|path| path == root.trim_end_matches('/') || path.starts_with(&root))
            .collect();
        if files.is_empty() && directories.is_empty() {
            return Err(format!("package '{app_id}' is not installed"));
        }
        if files
            .keys()
            .chain(directories.iter())
            .any(|path| ScriptCapabilities::normalize_path(path) != *path)
        {
            return Err("package contains noncanonical paths".into());
        }
        directories.insert(root.trim_end_matches('/').into());
        for path in files.keys() {
            let mut parent = path.as_str();
            while let Some((directory, _)) = parent.rsplit_once('/') {
                if directory.len() < root.len() {
                    break;
                }
                directories.insert(directory.into());
                parent = directory;
            }
        }
        Ok((files, directories))
    }
    pub(super) fn propose_app_removal(
        &mut self,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        if self.remote_installs.borrow().pending.is_some()
            || self.remote_installs.borrow().approved_removal.is_some()
        {
            return Err("another package change is awaiting completion".into());
        }
        let map = args
            .as_object()
            .ok_or("removal arguments must be an object")?;
        if map.keys().any(|key| key != "app_id") {
            return Err(
                "removal requires the native Remove control, not a model approval argument".into(),
            );
        }
        let app_id = required_string(args, "app_id")?;
        let (files, directories) = self.package_snapshot_for(app_id)?;
        let manifest = files
            .get(&format!("/apps/{app_id}/{app_id}.mcp"))
            .and_then(|raw| alloyos_mcp::AppManifest::parse_in_folder(app_id, raw).ok());
        let name = manifest
            .as_ref()
            .map(|manifest| manifest.name.as_str())
            .filter(|name| {
                !name.is_empty() && name.len() <= 128 && !name.chars().any(char::is_control)
            })
            .unwrap_or(app_id);
        let endpoint = manifest
            .as_ref()
            .and_then(|manifest| manifest.endpoint.clone())
            .or_else(|| {
                self.accounts
                    .borrow()
                    .status(app_id)
                    .map(|record| record.endpoint.clone())
            })
            .unwrap_or_default();
        if !endpoint.is_empty() && !crate::remote_discovery::hosted_endpoint(&endpoint) {
            return Err(
                "cannot safely identify this package's account endpoint for removal".into(),
            );
        }
        let mut state = self.remote_installs.borrow_mut();
        state.next_id = state
            .next_id
            .checked_add(1)
            .ok_or("package change proposal ID exhausted")?;
        let review = PendingRemoteInstall {
            action: PackageChangeAction::Remove,
            id: format!("install-{}", state.next_id),
            app_id: app_id.into(),
            name: name.into(),
            endpoint: endpoint.clone(),
            source: if endpoint.is_empty() {
                "Installed alloyOS JavaScript SDK package"
            } else {
                "Installed web-hosted remote MCP package"
            }
            .into(),
            publisher: "Installed package publisher not verified".into(),
        };
        if !review.is_valid() {
            return Err("package review metadata is invalid".into());
        }
        state.pending = Some(Candidate {
            review: review.clone(),
            files,
            directories,
        });
        Ok(
            serde_json::json!({"ok":true,"removed":false,"awaiting_user_approval":true,"proposal":review}),
        )
    }
    fn start_approved_package_removal(
        &mut self,
        candidate: Candidate,
    ) -> Result<ToolExecutionOutput, AiError> {
        let current = self
            .package_snapshot_for(&candidate.review.app_id)
            .map_err(AiError::ConfigError)?;
        if current.0 != candidate.files || current.1 != candidate.directories {
            return Err(AiError::PolicyViolation(
                "package changed after review; review removal again".into(),
            ));
        }
        if candidate.review.endpoint.is_empty() {
            return self
                .commit_package_removal(candidate, None)
                .map_err(AiError::ConfigError);
        }
        self.accounts
            .borrow_mut()
            .register_installed(&candidate.review.app_id, &candidate.review.endpoint)
            .map_err(|error| AiError::ConfigError(error.to_string()))?;
        let _ = self
            .mcp_registry
            .borrow_mut()
            .set_remote_credential_reference(&candidate.review.app_id, None);
        let _ = self.mcp_registry.borrow_mut().set_remote_trusted_bearer(&candidate.review.app_id, None);
        let cleanup = self
            .accounts
            .borrow_mut()
            .disconnect(&candidate.review.app_id);
        match cleanup {
            Poll::Ready(Ok(record)) if record.remote_revocation_confirmed.is_some() => self
                .commit_package_removal(candidate, record.remote_revocation_confirmed)
                .map_err(AiError::ConfigError),
            Poll::Ready(Err(error)) => Err(AiError::ConfigError(format!(
                "Account cleanup failed; package was preserved. {error}"
            ))),
            _ => {
                let output = ToolExecutionOutput {tool_name:"mcp_uninstall_app".into(),output_json:serde_json::json!({"ok":true,"removed":false,"pending_account_cleanup":true,"app_id":candidate.review.app_id,"name":candidate.review.name}).to_string(),synthesized_view:None};
                self.remote_installs.borrow_mut().approved_removal = Some(candidate);
                Ok(output)
            }
        }
    }
    pub(super) fn poll_approved_package_removal(&mut self) {
        let app = self
            .remote_installs
            .borrow()
            .approved_removal
            .as_ref()
            .map(|candidate| candidate.review.app_id.clone());
        let Some(app) = app else {
            return;
        };
        if self.accounts.borrow().has_pending() {
            return;
        }
        let status = self.accounts.borrow().status(&app).cloned();
        let candidate = self
            .remote_installs
            .borrow_mut()
            .approved_removal
            .take()
            .unwrap();
        let outcome = if let Some(record) = status.filter(|record| {
            record.phase == crate::accounts::ConnectionPhase::Disconnected
                && record.remote_revocation_confirmed.is_some()
        }) {
            self.commit_package_removal(candidate, record.remote_revocation_confirmed)
                .map_err(AiError::ConfigError)
        } else {
            Err(AiError::ConfigError(
                "Account cleanup did not complete; package was preserved.".into(),
            ))
        };
        self.remote_installs.borrow_mut().completed = Some(outcome);
    }
    pub fn take_package_change_result(&mut self) -> Option<Result<ToolExecutionOutput, AiError>> {
        self.remote_installs.borrow_mut().completed.take()
    }
    fn commit_package_removal(
        &mut self,
        candidate: Candidate,
        remote_revoked: Option<bool>,
    ) -> Result<ToolExecutionOutput, String> {
        let current = self.package_snapshot_for(&candidate.review.app_id)?;
        if current.0 != candidate.files || current.1 != candidate.directories {
            return Err("package changed after review; package was preserved".into());
        }
        let result = (|| {
            for path in candidate.files.keys() {
                self.delete_vfs_raw(path)?;
            }
            for directory in candidate.directories.iter().rev() {
                self.delete_vfs_raw(directory)?;
            }
            self.refresh_app_packages()?;
            Ok(())
        })();
        if let Err(error) = result {
            let mut rollback = Vec::new();
            for (path, content) in &candidate.files {
                if let Err(error) = self.write_vfs_raw(path, content) {
                    rollback.push(error);
                }
            }
            if let Err(error) = self.refresh_app_packages() {
                rollback.push(error);
            }
            return Err(if rollback.is_empty() {
                error
            } else {
                format!("{error}; rollback failed: {}", rollback.join(", "))
            });
        }
        Ok(ToolExecutionOutput {tool_name:"mcp_uninstall_app".into(),output_json:serde_json::json!({"ok":true,"removed":true,"app_id":candidate.review.app_id,"name":candidate.review.name,"deleted_files":candidate.files.len(),"remote_revocation_confirmed":remote_revoked,"account_credentials_removed":remote_revoked.is_some()}).to_string(),synthesized_view:None})
    }
    fn commit_remote_install(
        &mut self,
        candidate: Candidate,
    ) -> Result<ToolExecutionOutput, String> {
        let review = candidate.review;
        self.remote_package_available(&review.app_id)?;
        // The exact captured files/endpoint are approved; registry data is never
        // fetched again between review and commit.
        let manifest_path = format!("/apps/{0}/{0}.mcp", review.app_id);
        let mut paths: Vec<_> = candidate
            .files
            .keys()
            .filter(|path| **path != manifest_path)
            .cloned()
            .collect();
        paths.push(manifest_path);
        let result = (|| {
            for path in &paths {
                self.write_vfs_raw(path, &candidate.files[path])?;
            }
            self.refresh_app_packages()?;
            if self
                .mcp_registry
                .borrow()
                .guard_for(&review.app_id)
                .is_none()
            {
                return Err("installed package was not registered".into());
            }
            Ok(())
        })();
        if let Err(error) = result {
            let mut rollback = Vec::new();
            // Also remove a possibly partially written failing file, but only
            // within this previously absent package owned by this transaction.
            for path in &paths {
                if let Err(error) = self.delete_vfs_raw(path) {
                    rollback.push(error);
                }
            }
            if let Err(error) = self.refresh_app_packages() {
                rollback.push(error);
            }
            return Err(if rollback.is_empty() {
                error
            } else {
                format!("{error}; rollback failed: {}", rollback.join(", "))
            });
        }
        let _ = self
            .accounts
            .borrow_mut()
            .register_installed(&review.app_id, &review.endpoint);
        Ok(ToolExecutionOutput {tool_name:"mcp_install_remote".into(),output_json:serde_json::json!({"ok":true,"installed":true,"app_id":review.app_id,"name":review.name,"endpoint":review.endpoint,"path":format!("/apps/{}/",review.app_id),"connection_status":"not_connected","connectivity_verified":false,"account_authorized":false}).to_string(),synthesized_view:None})
    }
}
