//! Fresh, bounded handler invocation for the headless app host.
//!
//! This interpreter boundary does not provide OS process or memory isolation.
use crate::{ast::Stmt, parse_program, AlloyOsHost, JsError, JsIsolate, JsValue};
use alloc::{format, string::String};

pub const MAX_APP_SOURCE_BYTES: usize = 256 * 1024;
pub const MAX_APP_RESULT_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppHandler {
    CallTool,
    ReadResource,
    GetPrompt,
    RenderTile,
}
impl AppHandler {
    pub fn name(self) -> &'static str {
        match self {
            Self::CallTool => "callTool",
            Self::ReadResource => "readResource",
            Self::GetPrompt => "getPrompt",
            Self::RenderTile => "renderTile",
        }
    }
    pub fn arity(self) -> usize {
        match self {
            Self::CallTool | Self::GetPrompt => 2,
            Self::ReadResource | Self::RenderTile => 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppHandlerResult {
    pub text: String,
    pub steps_executed: usize,
}

/// Validate the supported handler contract without executing package source.
/// Runtime validation remains necessary: top-level code may overwrite bindings.
pub fn validate_app_handlers(source: &str, required: &[AppHandler]) -> Result<(), JsError> {
    if source.len() > MAX_APP_SOURCE_BYTES {
        return Err(JsError::SyntaxError("App source exceeds 256 KiB".into()));
    }
    let program = parse_program(source)?;
    for handler in required {
        let mut declarations = program.iter().filter_map(|stmt| match stmt {
            Stmt::FunctionDecl { name, params, .. } if name == handler.name() => Some(params.len()),
            _ => None,
        });
        let Some(arity) = declarations.next() else {
            return Err(JsError::ReferenceError(format!(
                "Missing top-level handler {}",
                handler.name()
            )));
        };
        if declarations.next().is_some() || arity != handler.arity() {
            return Err(JsError::TypeError(format!(
                "Handler {} must be declared once with {} parameters",
                handler.name(),
                handler.arity()
            )));
        }
    }
    Ok(())
}

/// Invoke one declared app handler with data arguments and a fresh global scope.
/// Fuel covers package initialization and handler execution together. The host
/// remains responsible for capability enforcement and MCP schema validation.
pub fn invoke_app_handler(
    source: &str,
    handler: AppHandler,
    args: &[JsValue],
    max_steps: usize,
    host: &mut dyn AlloyOsHost,
) -> Result<AppHandlerResult, JsError> {
    validate_app_handlers(source, &[handler])?;
    if args.len() != handler.arity() {
        return Err(JsError::TypeError(format!(
            "{} expects {} arguments",
            handler.name(),
            handler.arity()
        )));
    }
    let mut budget = 16_384;
    let mut byte_budget = MAX_APP_RESULT_BYTES;
    for value in args {
        validate_data(value, 0, &mut budget, &mut byte_budget)?;
    }
    let mut isolate = JsIsolate::with_max_steps(max_steps);
    let value = isolate.call_function(source, handler.name(), args, host)?;
    match value {
        JsValue::String(text) if text.len() <= MAX_APP_RESULT_BYTES => Ok(AppHandlerResult {
            text,
            steps_executed: isolate.steps_executed(),
        }),
        JsValue::String(_) => Err(JsError::TypeError(
            "App handler result exceeds 1 MiB".into(),
        )),
        _ => Err(JsError::TypeError(format!(
            "{} must return text",
            handler.name()
        ))),
    }
}
fn validate_data(
    value: &JsValue,
    depth: usize,
    budget: &mut usize,
    byte_budget: &mut usize,
) -> Result<(), JsError> {
    if depth > 32 || *budget == 0 {
        return Err(JsError::TypeError(
            "App arguments exceed data limits".into(),
        ));
    }
    *budget -= 1;
    match value {
        JsValue::Function(_) | JsValue::Undefined => {
            return Err(JsError::TypeError("App arguments must be JSON data".into()))
        }
        JsValue::String(text) => {
            *byte_budget = byte_budget
                .checked_sub(text.len())
                .ok_or_else(|| JsError::TypeError("App argument data exceeds 1 MiB".into()))?;
        }
        JsValue::Array(values) => {
            for value in values {
                validate_data(value, depth + 1, budget, byte_budget)?;
            }
        }
        JsValue::Object(values) => {
            for (key, value) in values {
                *byte_budget = byte_budget
                    .checked_sub(key.len())
                    .ok_or_else(|| JsError::TypeError("App argument data exceeds 1 MiB".into()))?;
                validate_data(value, depth + 1, budget, byte_budget)?;
            }
        }
        _ => (),
    }
    Ok(())
}
