use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use metalos_proto::SyscallNo;

use crate::protocol::McpError;

/// Capabilities that an application manifest may attempt to declare or invoke.
///
/// In metalOS, developer applications are strictly headless MCP servers:
/// - **Allowed**: `ExposeMcpTools`, `ExposeMcpResources`, `ExposeMcpPrompts`,
///   `ScopedStorage(String)`, `ScopedNetwork`
/// - **Forbidden (Zero App-Owned UI & Zero Shell)**: `CreateWindow`,
///   `DirectFramebuffer`, `RawInputCapture`, `ShellExecution`
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppCapability {
    // Allowed headless MCP & sandboxed capabilities
    ExposeMcpTools,
    ExposeMcpResources,
    ExposeMcpPrompts,
    ScopedStorage(String),
    ScopedNetwork,

    // Strictly forbidden capabilities — UI & shell belong exclusively to the AI agent / kernel
    CreateWindow,
    DirectFramebuffer,
    RawInputCapture,
    ShellExecution,
}

impl AppCapability {
    /// Returns true if this capability violates metalOS's Zero App-Owned UI or sandbox rules.
    pub fn is_forbidden(&self) -> bool {
        matches!(
            self,
            AppCapability::CreateWindow
                | AppCapability::DirectFramebuffer
                | AppCapability::RawInputCapture
                | AppCapability::ShellExecution
        )
    }

    /// Returns true if this capability is an MCP exposure capability.
    pub fn is_mcp_surface(&self) -> bool {
        matches!(
            self,
            AppCapability::ExposeMcpTools
                | AppCapability::ExposeMcpResources
                | AppCapability::ExposeMcpPrompts
        )
    }

    /// Parse a single capability token from a `/apps/*.mcp` manifest file.
    pub fn parse(raw: &str) -> Result<Self, McpSandboxError> {
        let token = raw.trim();
        match token {
            "ExposeMcpTools" | "mcp.tools" => Ok(AppCapability::ExposeMcpTools),
            "ExposeMcpResources" | "mcp.resources" => Ok(AppCapability::ExposeMcpResources),
            "ExposeMcpPrompts" | "mcp.prompts" => Ok(AppCapability::ExposeMcpPrompts),
            "ScopedNetwork" | "net.scoped" => Ok(AppCapability::ScopedNetwork),
            "CreateWindow" | "ui.window" => Ok(AppCapability::CreateWindow),
            "DirectFramebuffer" | "ui.framebuffer" => Ok(AppCapability::DirectFramebuffer),
            "RawInputCapture" | "ui.raw_input" => Ok(AppCapability::RawInputCapture),
            "ShellExecution" | "sys.shell" => Ok(AppCapability::ShellExecution),
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
            AppCapability::ScopedStorage(path) => format!("ScopedStorage({})", path),
            AppCapability::ScopedNetwork => "ScopedNetwork".to_string(),
            AppCapability::CreateWindow => "CreateWindow".to_string(),
            AppCapability::DirectFramebuffer => "DirectFramebuffer".to_string(),
            AppCapability::RawInputCapture => "RawInputCapture".to_string(),
            AppCapability::ShellExecution => "ShellExecution".to_string(),
        }
    }
}

/// Sandbox and manifest validation errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpSandboxError {
    /// App attempted to declare or invoke a UI capability (`CreateWindow`, `DirectFramebuffer`, `RawInputCapture`).
    /// Only the AI agent (`mercuryd`) may own the framebuffer or create visual surfaces.
    UiCapabilityForbidden {
        app_id: String,
        capability: AppCapability,
    },
    /// App attempted to declare or invoke shell execution (`ShellExecution`).
    ShellCapabilityForbidden {
        app_id: String,
        capability: AppCapability,
    },
    /// App attempted an operation at runtime that was not declared in its manifest.
    MissingCapability {
        app_id: String,
        capability: AppCapability,
    },
    /// App manifest did not declare any MCP surface (`ExposeMcpTools`, `ExposeMcpResources`, or `ExposeMcpPrompts`).
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
                    "Zero App-Owned UI violation in app '{}': capability {:?} is strictly forbidden; only the AI agent may render UI",
                    app_id, capability
                ))
            }
            McpSandboxError::ShellCapabilityForbidden { app_id, capability } => {
                McpError::sandbox_violation(format!(
                    "Sandbox violation in app '{}': capability {:?} is strictly forbidden",
                    app_id, capability
                ))
            }
            McpSandboxError::MissingCapability { app_id, capability } => {
                McpError::sandbox_violation(format!(
                    "Sandbox violation in app '{}': undeclared capability {:?}",
                    app_id, capability
                ))
            }
            McpSandboxError::EmptyMcpSurface { app_id } => {
                McpError::sandbox_violation(format!(
                    "Invalid MCP app '{}': must declare at least one MCP capability",
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
            declared_capabilities,
        }
    }

    /// Returns true if the manifest explicitly declares the given capability.
    pub fn has_capability(&self, cap: &AppCapability) -> bool {
        self.declared_capabilities.iter().any(|c| c == cap)
    }

    /// Parse an `AppManifest` from `/apps/*.mcp` key-value configuration text.
    ///
    /// Example `/apps/notes.mcp` format:
    /// ```text
    /// # Headless MCP App Manifest
    /// app_id = notes
    /// name = Notes Vault
    /// version = 1.0.0
    /// capabilities = ExposeMcpTools, ExposeMcpResources, ExposeMcpPrompts, ScopedStorage(/var/notes)
    /// ```
    pub fn parse(config_text: &str) -> Result<Self, McpSandboxError> {
        let mut app_id: Option<String> = None;
        let mut name: Option<String> = None;
        let mut version: Option<String> = None;
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
                "app_id" => app_id = Some(val.to_string()),
                "name" => name = Some(val.to_string()),
                "version" => version = Some(val.to_string()),
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

        let app_id = app_id.ok_or_else(|| {
            McpSandboxError::ManifestParseError("missing required field 'app_id'".to_string())
        })?;
        let name = name.ok_or_else(|| {
            McpSandboxError::ManifestParseError("missing required field 'name'".to_string())
        })?;
        let version = version.ok_or_else(|| {
            McpSandboxError::ManifestParseError("missing required field 'version'".to_string())
        })?;

        Ok(Self {
            app_id,
            name,
            version,
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
            "app_id = {}\nname = {}\nversion = {}\ncapabilities = {}\n",
            self.app_id,
            self.name,
            self.version,
            caps.join(", ")
        )
    }
}

/// Validates an application's manifest before registration or execution.
///
/// Enforces:
/// 1. Non-empty alphanumeric/underscore/hyphen `app_id`, `name`, and `version`.
/// 2. **Zero App-Owned UI**: Rejects `CreateWindow`, `DirectFramebuffer`, and `RawInputCapture`
///    with `McpSandboxError::UiCapabilityForbidden`.
/// 3. **No Shell Execution**: Rejects `ShellExecution` with `McpSandboxError::ShellCapabilityForbidden`.
/// 4. **Scoped Storage Isolation**: Rejects `ScopedStorage` paths containing `..` or not rooted at `/`.
/// 5. **At Least One MCP Surface**: Requires at least one of `ExposeMcpTools`, `ExposeMcpResources`,
///    or `ExposeMcpPrompts`.
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
            }
            AppCapability::ExposeMcpTools
            | AppCapability::ExposeMcpResources
            | AppCapability::ExposeMcpPrompts => {
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

/// Runtime capability guard that enforces Zero App-Owned UI and manifest boundaries
/// on every operation invoked by or against an MCP application.
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
    ///
    /// Immediately rejects any forbidden UI or shell capability with
    /// `UiCapabilityForbidden` or `ShellCapabilityForbidden`, and rejects any
    /// allowed capability that was not declared in the app's manifest.
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
            AppCapability::ScopedStorage(target_path) => {
                if target_path.contains("..") || !target_path.starts_with('/') {
                    return Err(McpSandboxError::MissingCapability {
                        app_id: self.manifest.app_id.clone(),
                        capability: requested.clone(),
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
