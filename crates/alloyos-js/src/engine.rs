//! Shared JavaScript engine boundary (`JsEngine` / `AlloyJsEngine`) separating
//! capability-scoped headless app / automation execution from isolated web script execution.

use alloc::string::String;
use alloc::vec::Vec;
use crate::app::{AppHandler, AppHandlerResult, MAX_APP_RESULT_BYTES, MAX_APP_SOURCE_BYTES};
use crate::host::AlloyOsHost;
use crate::runtime::DEFAULT_MAX_STEPS;
use crate::value::{JsError, JsValue};

/// Execution domain separating host-bound alloyOS scripts from untrusted web scripts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsExecutionDomain {
    /// Capability-scoped `AlloyOS.*` host bindings for headless MCP apps; no DOM/WebView/browser globals.
    HeadlessApp,
    /// Capability-scoped `AlloyOS.*` host bindings for scheduled automation scripts; no DOM/WebView/browser globals.
    AutomationScript,
    /// Pure ECMAScript execution context for untrusted web/smoke scripts where `AlloyOS` is not bound.
    IsolatedWebScript,
}

impl JsExecutionDomain {
    /// Returns `true` only for domains permitted to bind the `AlloyOS.*` host namespace.
    pub const fn allows_alloyos_bindings(self) -> bool {
        matches!(self, Self::HeadlessApp | Self::AutomationScript)
    }

    /// Returns `false` across all headless engine domains (DOM globals belong exclusively to the browser surface).
    pub const fn allows_dom_globals(self) -> bool {
        false
    }
}

/// Deterministic resource limits enforced by a `JsEngine` instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineLimits {
    pub max_source_bytes: usize,
    pub max_steps: usize,
    pub max_result_bytes: usize,
}

impl Default for EngineLimits {
    fn default() -> Self {
        Self {
            max_source_bytes: MAX_APP_SOURCE_BYTES,
            max_steps: DEFAULT_MAX_STEPS,
            max_result_bytes: MAX_APP_RESULT_BYTES,
        }
    }
}

/// Output of evaluating an isolated web/smoke script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebScriptOutput {
    pub value: JsValue,
    pub printed: Vec<String>,
    pub steps_executed: usize,
}

/// Shared JavaScript engine trait used across headless apps, automation scripts, and isolated web scripts.
pub trait JsEngine {
    /// Returns the deterministic resource limits configured for this engine.
    fn limits(&self) -> EngineLimits;

    /// Validates JavaScript source syntax and size limits for the target `domain`.
    fn validate_source(&self, domain: JsExecutionDomain, source: &str) -> Result<(), JsError>;

    /// Validates top-level headless app handler declarations and parameter arities.
    fn validate_app_handlers(&self, source: &str, required: &[AppHandler]) -> Result<(), JsError>;

    /// Evaluates an untrusted ECMAScript snippet in `IsolatedWebScript` mode without `AlloyOS.*` bindings.
    fn eval_isolated_web_script(&self, source: &str) -> Result<WebScriptOutput, JsError>;

    /// Invokes a headless MCP app handler in `HeadlessApp` mode with capability-scoped `host` bindings.
    fn invoke_app_handler(
        &self,
        source: &str,
        handler: AppHandler,
        args: &[JsValue],
        host: &mut dyn AlloyOsHost,
    ) -> Result<AppHandlerResult, JsError>;
}

/// Default `alloyos-js` implementation of the shared `JsEngine` boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlloyJsEngine {
    limits: EngineLimits,
}

impl Default for AlloyJsEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl AlloyJsEngine {
    /// Creates a new `AlloyJsEngine` with default resource limits.
    pub const fn new() -> Self {
        Self {
            limits: EngineLimits {
                max_source_bytes: MAX_APP_SOURCE_BYTES,
                max_steps: DEFAULT_MAX_STEPS,
                max_result_bytes: MAX_APP_RESULT_BYTES,
            },
        }
    }

    /// Creates a new `AlloyJsEngine` with custom deterministic `limits`.
    pub const fn with_limits(limits: EngineLimits) -> Self {
        Self { limits }
    }

    /// Returns the configured engine limits.
    pub const fn limits(&self) -> EngineLimits {
        self.limits
    }

    /// Validates JavaScript source syntax and byte limits for `domain`.
    pub fn validate_source(&self, domain: JsExecutionDomain, source: &str) -> Result<(), JsError> {
        crate::jsc::JavaScriptCoreEngine::with_limits(self.limits).validate_source(domain, source)
    }

    /// Validates top-level headless app handler declarations against engine byte limits.
    pub fn validate_app_handlers(
        &self,
        source: &str,
        required: &[AppHandler],
    ) -> Result<(), JsError> {
        crate::jsc::JavaScriptCoreEngine::with_limits(self.limits).validate_app_handlers(source, required)
    }

    /// Evaluates a script in the specified `domain`. In `IsolatedWebScript` domain, `AlloyOS.*`
    /// bindings are absent from the global scope even when `host` is supplied.
    pub fn eval_in_domain(
        &self,
        domain: JsExecutionDomain,
        source: &str,
        host: &mut dyn AlloyOsHost,
    ) -> Result<WebScriptOutput, JsError> {
        crate::jsc::JavaScriptCoreEngine::with_limits(self.limits).eval_in_domain(domain, source, host)
    }

    /// Evaluates an isolated web script with no `AlloyOS.*` host bindings.
    pub fn eval_isolated_web_script(&self, source: &str) -> Result<WebScriptOutput, JsError> {
        crate::jsc::JavaScriptCoreEngine::with_limits(self.limits).eval_isolated_web_script(source)
    }

    /// Invokes a declared headless MCP app handler under this engine's configured limits.
    pub fn invoke_app_handler(
        &self,
        source: &str,
        handler: AppHandler,
        args: &[JsValue],
        host: &mut dyn AlloyOsHost,
    ) -> Result<AppHandlerResult, JsError> {
        crate::jsc::JavaScriptCoreEngine::with_limits(self.limits).invoke_app_handler(source, handler, args, host)
    }
}

impl JsEngine for AlloyJsEngine {
    fn limits(&self) -> EngineLimits {
        AlloyJsEngine::limits(self)
    }

    fn validate_source(&self, domain: JsExecutionDomain, source: &str) -> Result<(), JsError> {
        AlloyJsEngine::validate_source(self, domain, source)
    }

    fn validate_app_handlers(&self, source: &str, required: &[AppHandler]) -> Result<(), JsError> {
        AlloyJsEngine::validate_app_handlers(self, source, required)
    }

    fn eval_isolated_web_script(&self, source: &str) -> Result<WebScriptOutput, JsError> {
        AlloyJsEngine::eval_isolated_web_script(self, source)
    }

    fn invoke_app_handler(
        &self,
        source: &str,
        handler: AppHandler,
        args: &[JsValue],
        host: &mut dyn AlloyOsHost,
    ) -> Result<AppHandlerResult, JsError> {
        AlloyJsEngine::invoke_app_handler(self, source, handler, args, host)
    }
}
