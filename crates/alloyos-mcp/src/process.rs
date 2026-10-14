//! Supervisor transport. Bare-metal MCP handlers fail closed until a protected
//! executable supervisor is installed; host tests keep their local interpreter.
use alloyos_js::{AlloyOsHost, AppHandler, AppHandlerResult, JsError, JsValue};
use core::sync::atomic::{AtomicUsize, Ordering};
pub type Invoke = fn(
    &str,
    &str,
    AppHandler,
    &[JsValue],
    usize,
    &mut dyn AlloyOsHost,
) -> Result<AppHandlerResult, JsError>;
static STOP: AtomicUsize = AtomicUsize::new(0);
pub fn install_lifecycle(stop: fn(&str)) {
    STOP.store(stop as usize, Ordering::Release);
}
pub fn stop(app_id: &str) {
    let pointer = STOP.load(Ordering::Acquire);
    if pointer != 0 {
        let stop: fn(&str) = unsafe { core::mem::transmute(pointer) };
        stop(app_id);
    }
}
static TRANSPORT: AtomicUsize = AtomicUsize::new(0);
pub fn install(transport: Invoke) {
    TRANSPORT.store(transport as usize, Ordering::Release);
}
pub fn invoke(
    app_id: &str,
    source: &str,
    handler: AppHandler,
    args: &[JsValue],
    steps: usize,
    host: &mut dyn AlloyOsHost,
) -> Result<AppHandlerResult, JsError> {
    let pointer = TRANSPORT.load(Ordering::Acquire);
    if pointer != 0 {
        let transport: Invoke = unsafe { core::mem::transmute(pointer) };
        return transport(app_id, source, handler, args, steps, host);
    }
    #[cfg(not(target_os = "none"))]
    {
        let _ = app_id;
        alloyos_js::invoke_app_handler(source, handler, args, steps, host)
    }
    #[cfg(target_os = "none")]
    {
        let _ = (app_id, source, handler, args, steps, host);
        Err(JsError::TypeError(
            "Protected app supervisor unavailable".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn structured_results_reject_excessive_depth_and_plain_text_remains_text() {
        let deep = alloc::format!("{}0{}", "[".repeat(100), "]".repeat(100));
        assert!(crate::McpValue::from_json(&deep).is_err());
        // The same fallback used by JsApp::call_tool preserves opaque text.
        let parsed = crate::McpValue::from_json(&deep)
            .unwrap_or_else(|_| crate::McpValue::Text(deep.clone()));
        assert_eq!(parsed, crate::McpValue::Text(deep));
        assert!(crate::McpValue::from_json("{\"result\":42}").is_ok());
    }
}
