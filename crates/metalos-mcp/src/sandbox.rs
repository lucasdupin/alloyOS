use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use metalos_proto::SyscallNo;

use crate::protocol::McpError;

/// Canonical app identifier for the `Tasks` application, which is the ONLY app
/// permitted to hold `AppCapability::ScriptSchedulerAccess`.
pub const TASKS_APP_ID: &str = "tasks";

/// Capabilities that an application manifest may attempt to declare or invoke.
///
/// In metalOS, developer applications expose:
/// - **Allowed**: `ExposeMcpTools`, `ExposeMcpResources`, `ExposeMcpPrompts`,
///   `ExposeSkills`, `ExposeTiles`, `ScopedStorage(String)`, `ScopedNetwork`,
///   and `ScriptSchedulerAccess` (strictly exclusive to `app_id = "tasks"`).
/// - **Forbidden**:
///   - Direct window/framebuffer/input ownership (`CreateWindow`, `DirectFramebuffer`, `RawInputCapture`)
///   - Shell execution (`ShellExecution`)
///   - Cross-app access (`CrossAppAccess` — only Welder centralizes app access and orchestration)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppCapability {
    // Allowed app exposure & sandboxed capabilities
    ExposeMcpTools,
    ExposeMcpResources,
    ExposeMcpPrompts,
    ExposeSkills,
    ExposeTiles,
    /// Exclusive capability allowing ONLY the `tasks` app (`app_id == "tasks"`) to access `ScriptScheduler`.
    ScriptSchedulerAccess,
    ScopedStorage(String),
    ScopedNetwork,

    // Strictly forbidden capabilities
    CreateWindow,
    DirectFramebuffer,
    RawInputCapture,
    ShellExecution,
    CrossAppAccess,
}

impl AppCapability {
    /// Returns true if this capability is unconditionally forbidden for all apps.
    pub fn is_forbidden(&self) -> bool {
        matches!(
            self,
            AppCapability::CreateWindow
                | AppCapability::DirectFramebuffer
                | AppCapability::RawInputCapture
                | AppCapability::ShellExecution
                | AppCapability::CrossAppAccess
        )
    }

    /// Returns true if this capability is an MCP, Skill, or Tile exposure capability.
    pub fn is_mcp_surface(&self) -> bool {
        matches!(
            self,
            AppCapability::ExposeMcpTools
                | AppCapability::ExposeMcpResources
                | AppCapability::ExposeMcpPrompts
                | AppCapability::ExposeSkills
                | AppCapability::ExposeTiles
        )
    }

    /// Parse a single capability token from a `/apps/*.mcp` manifest file.
    pub fn parse(raw: &str) -> Result<Self, McpSandboxError> {
        let token = raw.trim();
        match token {
            "ExposeMcpTools" | "mcp.tools" => Ok(AppCapability::ExposeMcpTools),
            "ExposeMcpResources" | "mcp.resources" => Ok(AppCapability::ExposeMcpResources),
            "ExposeMcpPrompts" | "mcp.prompts" => Ok(AppCapability::ExposeMcpPrompts),
            "ExposeSkills" | "app.skills" => Ok(AppCapability::ExposeSkills),
            "ExposeTiles" | "app.tiles" => Ok(AppCapability::ExposeTiles),
            "ScriptSchedulerAccess" | "scheduler.access" => {
                Ok(AppCapability::ScriptSchedulerAccess)
            }
            "ScopedNetwork" | "net.scoped" => Ok(AppCapability::ScopedNetwork),
            "CreateWindow" | "ui.window" => Ok(AppCapability::CreateWindow),
            "DirectFramebuffer" | "ui.framebuffer" => Ok(AppCapability::DirectFramebuffer),
            "RawInputCapture" | "ui.raw_input" => Ok(AppCapability::RawInputCapture),
            "ShellExecution" | "sys.shell" => Ok(AppCapability::ShellExecution),
            "CrossAppAccess" | "app.cross_access" => Ok(AppCapability::CrossAppAccess),
            _ => {
                if let Some(rest) = token
                    .strip_prefix("ScopedStorage(")
                    .and_then(|s| s.strip_suffix(')'))
                {
                    let path = rest.trim().trim_matches('"');
                    if path.is_empty() {
                        return Err(McpSandboxError::ManifestParseError(
                            "ScopedStorage path cannot be empty".to_string(),
                        ));
                    }
                    Ok(AppCapability::ScopedStorage(path.to_string()))
                } else if let Some(path) = token.strip_prefix("storage:") {
                    let path = path.trim();
                    if path.is_empty() {
                        return Err(McpSandboxError::ManifestParseError(
                            "storage: path cannot be empty".to_string(),
                        ));
                    }
                    Ok(AppCapability::ScopedStorage(path.to_string()))
                } else {
                    Err(McpSandboxError::ManifestParseError(format!(
                        "unknown capability token: {}",
                        token
                    )))
                }
            }
        }
    }

    /// Format this capability for `/apps/*.mcp` serialization.
    pub fn to_manifest_token(&self) -> String {
        match self {
            AppCapability::ExposeMcpTools => "ExposeMcpTools".to_string(),
            AppCapability::ExposeMcpResources => "ExposeMcpResources".to_string(),
            AppCapability::ExposeMcpPrompts => "ExposeMcpPrompts".to_string(),
            AppCapability::ExposeSkills => "ExposeSkills".to_string(),
            AppCapability::ExposeTiles => "ExposeTiles".to_string(),
            AppCapability::ScriptSchedulerAccess => "ScriptSchedulerAccess".to_string(),
            AppCapability::ScopedStorage(path) => format!("ScopedStorage({})", path),
            AppCapability::ScopedNetwork => "ScopedNetwork".to_string(),
            AppCapability::CreateWindow => "CreateWindow".to_string(),
            AppCapability::DirectFramebuffer => "DirectFramebuffer".to_string(),
            AppCapability::RawInputCapture => "RawInputCapture".to_string(),
            AppCapability::ShellExecution => "ShellExecution".to_string(),
            AppCapability::CrossAppAccess => "CrossAppAccess".to_string(),
        }
    }
}

/// Sandbox and manifest validation errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpSandboxError {
    /// App attempted to declare or invoke a raw UI capability (`CreateWindow`, `DirectFramebuffer`, `RawInputCapture`).
    /// Apps may only expose declarative `AppTile` descriptors (`ExposeTiles`); Ferrofluid owns rendering.
    UiCapabilityForbidden {
        app_id: String,
        capability: AppCapability,
    },
    /// App attempted to declare or invoke shell execution (`ShellExecution`).
    ShellCapabilityForbidden {
        app_id: String,
        capability: AppCapability,
    },
    /// Non-`tasks` app attempted to declare or invoke `ScriptSchedulerAccess` or access scheduler storage.
    /// Only the `Tasks` app (`app_id = "tasks"`) is permitted to access `ScriptScheduler`.
    SchedulerAccessForbidden { app_id: String },
    /// An app attempted to access or invoke another app directly.
    /// No app can access another app; only Welder centralizes app access and orchestration.
    CrossAppAccessForbidden {
        caller_app_id: String,
        target_app_id: String,
    },
    /// App attempted an operation at runtime that was not declared in its manifest.
    MissingCapability {
        app_id: String,
        capability: AppCapability,
    },
    /// App manifest did not declare any exposure surface.
    EmptyMcpSurface { app_id: String },
    /// App manifest has an invalid `app_id`, `name`, or `version`, or attempts path traversal in `ScopedStorage`.
    InvalidManifest { app_id: String, reason: String },
    /// Syntax error while parsing a `/apps/*.mcp` manifest file.
    ManifestParseError(String),
}

impl McpSandboxError {
    pub fn to_mcp_error(&self) -> McpError {
        match self {
            McpSandboxError::UiCapabilityForbidden { app_id, capability } => {
                McpError::sandbox_violation(format!(
                    "Zero App-Owned UI violation in app '{}': capability {:?} is strictly forbidden; apps may only expose declarative Tiles",
                    app_id, capability
                ))
            }
            McpSandboxError::ShellCapabilityForbidden { app_id, capability } => {
                McpError::sandbox_violation(format!(
                    "Sandbox violation in app '{}': capability {:?} is strictly forbidden",
                    app_id, capability
                ))
            }
            McpSandboxError::SchedulerAccessForbidden { app_id } => {
                McpError::sandbox_violation(format!(
                    "Scheduler isolation violation in app '{}': only the 'tasks' app may access ScriptScheduler",
                    app_id
                ))
            }
            McpSandboxError::CrossAppAccessForbidden {
                caller_app_id,
                target_app_id,
            } => McpError::sandbox_violation(format!(
                "Cross-app isolation violation: app '{}' cannot access app '{}'; only Welder centralizes app access and orchestration",
                caller_app_id, target_app_id
            )),
            McpSandboxError::MissingCapability { app_id, capability } => {
                McpError::sandbox_violation(format!(
                    "Sandbox violation in app '{}': undeclared capability {:?}",
                    app_id, capability
                ))
            }
            McpSandboxError::EmptyMcpSurface { app_id } => {
                McpError::sandbox_violation(format!(
                    "Invalid MCP app '{}': must declare at least one MCP/Skill/Tile capability",
                    app_id
                ))
            }
            McpSandboxError::InvalidManifest { app_id, reason } => {
                McpError::sandbox_violation(format!(
                    "Invalid manifest for app '{}': {}",
                    app_id, reason
                ))
            }
            McpSandboxError::ManifestParseError(msg) => {
                McpError::sandbox_violation(format!("Manifest parse error: {}", msg))
            }
        }
    }
}

/// Application manifest parsed from `/apps/*.mcp` config files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppManifest {
    pub app_id: String,
    pub name: String,
    pub version: String,
    pub entrypoint: Option<String>,
    pub declared_capabilities: Vec<AppCapability>,
}

impl AppManifest {
    pub fn new(
        app_id: impl Into<String>,
        name: impl Into<String>,
        version: impl Into<String>,
        declared_capabilities: Vec<AppCapability>,
    ) -> Self {
        Self {
            app_id: app_id.into(),
            name: name.into(),
            version: version.into(),
            entrypoint: None,
            declared_capabilities,
        }
    }

    /// Returns true if the manifest explicitly declares the given capability.
    pub fn has_capability(&self, cap: &AppCapability) -> bool {
        self.declared_capabilities.iter().any(|c| c == cap)
    }

    /// Parse an `AppManifest` for a known `/apps/<folder>/` directory name.
    pub fn parse_in_folder(
        folder: impl Into<String>,
        config_text: &str,
    ) -> Result<Self, McpSandboxError> {
        Self::parse_with_default_folder(Some(folder.into()), config_text)
    }

    /// Parse an `AppManifest` from `/apps/<folder>/<folder>.mcp` key-value configuration text.
    /// If `app_id` is omitted in the manifest, the folder name is used (or derived from `name`).
    pub fn parse(config_text: &str) -> Result<Self, McpSandboxError> {
        Self::parse_with_default_folder(None, config_text)
    }

    fn parse_with_default_folder(
        default_folder: Option<String>,
        config_text: &str,
    ) -> Result<Self, McpSandboxError> {
        let mut app_id: Option<String> = default_folder;
        let mut name: Option<String> = None;
        let mut version: Option<String> = None;
        let mut entrypoint: Option<String> = None;
        let mut capabilities: Vec<AppCapability> = Vec::new();

        for (line_idx, raw_line) in config_text.lines().enumerate() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, val) = line.split_once('=').ok_or_else(|| {
                McpSandboxError::ManifestParseError(format!(
                    "line {}: expected 'key = value'",
                    line_idx + 1
                ))
            })?;
            let key = key.trim();
            let val = val.trim().trim_matches('"');

            match key {
                "app_id" | "folder" => app_id = Some(val.to_string()),
                "name" => name = Some(val.to_string()),
                "version" => version = Some(val.to_string()),
                "entrypoint" => {
                    if entrypoint.replace(val.to_string()).is_some() {
                        return Err(McpSandboxError::ManifestParseError("duplicate entrypoint".to_string()));
                    }
                },
                "capabilities" => {
                    if !val.is_empty() {
                        for item in val.split(',') {
                            let item = item.trim();
                            if !item.is_empty() {
                                capabilities.push(AppCapability::parse(item)?);
                            }
                        }
                    }
                }
                "capability" => {
                    if !val.is_empty() {
                        capabilities.push(AppCapability::parse(val)?);
                    }
                }
                other => {
                    return Err(McpSandboxError::ManifestParseError(format!(
                        "line {}: unknown manifest key '{}'",
                        line_idx + 1,
                        other
                    )))
                }
            }
        }

        let name = name.ok_or_else(|| {
            McpSandboxError::ManifestParseError("missing required field 'name'".to_string())
        })?;
        let app_id = app_id.unwrap_or_else(|| {
            name.trim()
                .to_ascii_lowercase()
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
                .collect()
        });
        let version = version.ok_or_else(|| {
            McpSandboxError::ManifestParseError("missing required field 'version'".to_string())
        })?;

        Ok(Self {
            app_id,
            name,
            version,
            entrypoint,
            declared_capabilities: capabilities,
        })
    }

    /// Serialize this manifest into `/apps/*.mcp` configuration format.
    pub fn to_mcp_config(&self) -> String {
        let caps: Vec<String> = self
            .declared_capabilities
            .iter()
            .map(|c| c.to_manifest_token())
            .collect();
        format!(
            "app_id = {}\nname = {}\nversion = {}\n{}capabilities = {}\n",
            self.app_id,
            self.name,
            self.version,
            self.entrypoint.as_ref().map(|path| format!("entrypoint = {path}\n")).unwrap_or_default(),
            caps.join(", ")
        )
    }
}

fn is_scheduler_protected_path(path: &str) -> bool {
    path == "/etc/schedules.conf"
        || path == "/scripts"
        || path.starts_with("/scripts/")
}

/// Validates an application's manifest before registration or execution.
///
/// Enforces:
/// 1. Non-empty alphanumeric/underscore/hyphen `app_id`, `name`, and `version`.
/// 2. **Zero App-Owned Window/Framebuffer UI**: Rejects `CreateWindow`, `DirectFramebuffer`, and `RawInputCapture`
///    with `McpSandboxError::UiCapabilityForbidden`.
/// 3. **No Shell Execution**: Rejects `ShellExecution` with `McpSandboxError::ShellCapabilityForbidden`.
/// 4. **No Cross-App Access**: Rejects `CrossAppAccess` with `McpSandboxError::CrossAppAccessForbidden`.
/// 5. **Exclusive ScriptScheduler Access**: Rejects `ScriptSchedulerAccess` (and `/etc/schedules.conf` or `/scripts`
///    storage scopes) for any app other than `tasks` (`app_id == "tasks"`).
/// 6. **Scoped Storage Isolation**: Rejects `ScopedStorage` paths containing `..` or not rooted at `/`.
/// 7. **At Least One Surface**: Requires at least one MCP/Skill/Tile surface capability.
pub fn validate_app_manifest(manifest: &AppManifest) -> Result<(), McpSandboxError> {
    if manifest.app_id.trim().is_empty()
        || !manifest
            .app_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(McpSandboxError::InvalidManifest {
            app_id: manifest.app_id.clone(),
            reason: "app_id must be non-empty ASCII alphanumeric/underscore/hyphen".to_string(),
        });
    }

    if manifest.name.trim().is_empty() {
        return Err(McpSandboxError::InvalidManifest {
            app_id: manifest.app_id.clone(),
            reason: "name must not be empty".to_string(),
        });
    }

    if manifest.version.trim().is_empty() {
        return Err(McpSandboxError::InvalidManifest {
            app_id: manifest.app_id.clone(),
            reason: "version must not be empty".to_string(),
        });
    }

    if let Some(path) = &manifest.entrypoint {
        let prefix = format!("/apps/{}/", manifest.app_id);
        if !path.starts_with(&prefix) || !path.ends_with(".js")
            || path.split('/').skip(1).any(|part| part.is_empty() || part == "." || part == "..") {
            return Err(McpSandboxError::InvalidManifest {
                app_id: manifest.app_id.clone(),
                reason: "entrypoint must be a JavaScript file inside the app package".to_string(),
            });
        }
    }

    let mut has_mcp_surface = false;

    for cap in &manifest.declared_capabilities {
        match cap {
            AppCapability::CreateWindow
            | AppCapability::DirectFramebuffer
            | AppCapability::RawInputCapture => {
                return Err(McpSandboxError::UiCapabilityForbidden {
                    app_id: manifest.app_id.clone(),
                    capability: cap.clone(),
                });
            }
            AppCapability::ShellExecution => {
                return Err(McpSandboxError::ShellCapabilityForbidden {
                    app_id: manifest.app_id.clone(),
                    capability: cap.clone(),
                });
            }
            AppCapability::CrossAppAccess => {
                return Err(McpSandboxError::CrossAppAccessForbidden {
                    caller_app_id: manifest.app_id.clone(),
                    target_app_id: "*".to_string(),
                });
            }
            AppCapability::ScriptSchedulerAccess => {
                if manifest.app_id != TASKS_APP_ID {
                    return Err(McpSandboxError::SchedulerAccessForbidden {
                        app_id: manifest.app_id.clone(),
                    });
                }
            }
            AppCapability::ScopedStorage(path) => {
                if !path.starts_with('/') || path.contains("..") {
                    return Err(McpSandboxError::InvalidManifest {
                        app_id: manifest.app_id.clone(),
                        reason: format!(
                            "ScopedStorage path '{}' must be absolute and not contain '..'",
                            path
                        ),
                    });
                }
                if manifest.app_id != TASKS_APP_ID && is_scheduler_protected_path(path) {
                    return Err(McpSandboxError::SchedulerAccessForbidden {
                        app_id: manifest.app_id.clone(),
                    });
                }
            }
            AppCapability::ExposeMcpTools
            | AppCapability::ExposeMcpResources
            | AppCapability::ExposeMcpPrompts
            | AppCapability::ExposeSkills
            | AppCapability::ExposeTiles => {
                has_mcp_surface = true;
            }
            AppCapability::ScopedNetwork => {}
        }
    }

    if !has_mcp_surface {
        return Err(McpSandboxError::EmptyMcpSurface {
            app_id: manifest.app_id.clone(),
        });
    }

    Ok(())
}

/// Runtime capability guard that enforces Zero App-Owned UI, exclusive `Tasks` scheduler access,
/// no cross-app access, and manifest boundaries on every operation invoked by or against an MCP application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeCapabilityGuard {
    manifest: AppManifest,
}

impl RuntimeCapabilityGuard {
    /// Construct a runtime capability guard after validating the manifest.
    pub fn new(manifest: AppManifest) -> Result<Self, McpSandboxError> {
        validate_app_manifest(&manifest)?;
        Ok(Self { manifest })
    }

    pub fn manifest(&self) -> &AppManifest {
        &self.manifest
    }

    /// Check whether a capability can be invoked at runtime by this app.
    pub fn enforce_capability(&self, requested: &AppCapability) -> Result<(), McpSandboxError> {
        match requested {
            AppCapability::CreateWindow
            | AppCapability::DirectFramebuffer
            | AppCapability::RawInputCapture => Err(McpSandboxError::UiCapabilityForbidden {
                app_id: self.manifest.app_id.clone(),
                capability: requested.clone(),
            }),
            AppCapability::ShellExecution => Err(McpSandboxError::ShellCapabilityForbidden {
                app_id: self.manifest.app_id.clone(),
                capability: requested.clone(),
            }),
            AppCapability::CrossAppAccess => Err(McpSandboxError::CrossAppAccessForbidden {
                caller_app_id: self.manifest.app_id.clone(),
                target_app_id: "*".to_string(),
            }),
            AppCapability::ScriptSchedulerAccess => {
                if self.manifest.app_id != TASKS_APP_ID {
                    return Err(McpSandboxError::SchedulerAccessForbidden {
                        app_id: self.manifest.app_id.clone(),
                    });
                }
                if self.manifest.has_capability(requested) {
                    Ok(())
                } else {
                    Err(McpSandboxError::MissingCapability {
                        app_id: self.manifest.app_id.clone(),
                        capability: requested.clone(),
                    })
                }
            }
            AppCapability::ScopedStorage(target_path) => {
                if target_path.contains("..") || !target_path.starts_with('/') {
                    return Err(McpSandboxError::MissingCapability {
                        app_id: self.manifest.app_id.clone(),
                        capability: requested.clone(),
                    });
                }
                if self.manifest.app_id != TASKS_APP_ID && is_scheduler_protected_path(target_path)
                {
                    return Err(McpSandboxError::SchedulerAccessForbidden {
                        app_id: self.manifest.app_id.clone(),
                    });
                }
                let allowed = self.manifest.declared_capabilities.iter().any(|cap| {
                    if let AppCapability::ScopedStorage(prefix) = cap {
                        target_path == prefix
                            || (target_path.starts_with(prefix.as_str())
                                && (prefix.ends_with('/')
                                    || target_path.as_bytes().get(prefix.len()) == Some(&b'/')))
                    } else {
                        false
                    }
                });
                if allowed {
                    Ok(())
                } else {
                    Err(McpSandboxError::MissingCapability {
                        app_id: self.manifest.app_id.clone(),
                        capability: requested.clone(),
                    })
                }
            }
            other => {
                if self.manifest.has_capability(other) {
                    Ok(())
                } else {
                    Err(McpSandboxError::MissingCapability {
                        app_id: self.manifest.app_id.clone(),
                        capability: other.clone(),
                    })
                }
            }
        }
    }

    /// Reject any attempt by this app to access or invoke another app.
    /// Only Welder centralizes app access and orchestration.
    pub fn enforce_no_cross_app_access(&self, target_app_id: &str) -> Result<(), McpSandboxError> {
        Err(McpSandboxError::CrossAppAccessForbidden {
            caller_app_id: self.manifest.app_id.clone(),
            target_app_id: target_app_id.to_string(),
        })
    }

    /// Enforce kernel syscall filtering for this MCP app process:
    /// - Process spawning (`Fork`, `Execve`, `Kill`) is rejected with `ShellCapabilityForbidden`.
    /// - Network socket creation (`Socket`, `Connect`) requires `ScopedNetwork`.
    pub fn enforce_syscall(&self, syscall: SyscallNo) -> Result<(), McpSandboxError> {
        match syscall {
            SyscallNo::Fork | SyscallNo::Execve | SyscallNo::Kill => {
                Err(McpSandboxError::ShellCapabilityForbidden {
                    app_id: self.manifest.app_id.clone(),
                    capability: AppCapability::ShellExecution,
                })
            }
            SyscallNo::Socket | SyscallNo::Connect => {
                self.enforce_capability(&AppCapability::ScopedNetwork)
            }
            _ => Ok(()),
        }
    }
}
