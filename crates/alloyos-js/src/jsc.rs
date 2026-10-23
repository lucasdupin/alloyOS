//! Native WebKit JavaScriptCore (JSC) engine backend for alloyOS.
//!
//! Provides genuine ECMAScript execution, standards-compliant syntax checking,
//! and fine-grained capability-scoped `AlloyOS.*` host bindings via JavaScriptCore C API.

use alloc::format;
#[cfg(not(target_os = "none"))]
use alloc::string::String;
#[cfg(not(target_os = "none"))]
use alloc::vec::Vec;

#[cfg(not(target_os = "none"))]
use alloc::string::ToString;
#[cfg(not(target_os = "none"))]
use alloc::vec;
#[cfg(not(target_os = "none"))]
use core::cell::RefCell;
#[cfg(not(target_os = "none"))]
use core::ffi::CStr;
#[cfg(not(target_os = "none"))]
use std::ffi::CString;
#[cfg(not(target_os = "none"))]
use std::thread_local;

use crate::app::{AppHandler, AppHandlerResult, MAX_APP_RESULT_BYTES, MAX_APP_SOURCE_BYTES};
use crate::engine::{EngineLimits, JsEngine, JsExecutionDomain, WebScriptOutput};
use crate::host::AlloyOsHost;
#[cfg(not(target_os = "none"))]
use crate::host::CapabilityRequest;
use crate::value::{JsError, JsValue};
#[cfg(not(target_os = "none"))]
use crate::value::{json_parse, json_stringify};

#[cfg(not(target_os = "none"))]
type JSContextGroupRef = *mut ();
#[cfg(not(target_os = "none"))]
type JSGlobalContextRef = *mut ();
#[cfg(not(target_os = "none"))]
type JSObjectRef = *mut ();
#[cfg(not(target_os = "none"))]
type JSValueRef = *mut ();
#[cfg(not(target_os = "none"))]
type JSStringRef = *mut ();

#[cfg(not(target_os = "none"))]
type JSObjectCallAsFunctionCallback = extern "C" fn(
    ctx: JSGlobalContextRef,
    function: JSObjectRef,
    this_object: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    exception: *mut JSValueRef,
) -> JSValueRef;

#[cfg(not(target_os = "none"))]
#[cfg_attr(target_os = "macos", link(name = "JavaScriptCore", kind = "framework"))]
extern "C" {
    fn JSContextGroupCreate() -> JSContextGroupRef;
    fn JSContextGroupRelease(group: JSContextGroupRef);
    fn JSContextGroupSetExecutionTimeLimit(
        group: JSContextGroupRef,
        limit: f64,
        callback: *const (),
        context: *mut (),
    );
    fn JSGlobalContextCreateInGroup(
        group: JSContextGroupRef,
        global_object_class: *const (),
    ) -> JSGlobalContextRef;
    fn JSGlobalContextRelease(ctx: JSGlobalContextRef);
    fn JSContextGetGlobalObject(ctx: JSGlobalContextRef) -> JSObjectRef;
    fn JSStringCreateWithUTF8CString(string: *const i8) -> JSStringRef;
    fn JSStringRelease(string: JSStringRef);
    fn JSStringGetMaximumUTF8CStringSize(string: JSStringRef) -> usize;
    fn JSStringGetUTF8CString(string: JSStringRef, buffer: *mut i8, buffer_size: usize) -> usize;
    fn JSCheckScriptSyntax(
        ctx: JSGlobalContextRef,
        script: JSStringRef,
        source_url: JSStringRef,
        starting_line_number: i32,
        exception: *mut JSValueRef,
    ) -> bool;
    fn JSEvaluateScript(
        ctx: JSGlobalContextRef,
        script: JSStringRef,
        this_object: JSObjectRef,
        source_url: JSStringRef,
        starting_line_number: i32,
        exception: *mut JSValueRef,
    ) -> JSValueRef;
    fn JSObjectMake(
        ctx: JSGlobalContextRef,
        js_class: *const (),
        data: *mut (),
    ) -> JSObjectRef;
    fn JSObjectSetProperty(
        ctx: JSGlobalContextRef,
        object: JSObjectRef,
        property_name: JSStringRef,
        value: JSValueRef,
        attributes: u32,
        exception: *mut JSValueRef,
    );
    fn JSObjectGetProperty(
        ctx: JSGlobalContextRef,
        object: JSObjectRef,
        property_name: JSStringRef,
        exception: *mut JSValueRef,
    ) -> JSValueRef;
    fn JSObjectMakeFunctionWithCallback(
        ctx: JSGlobalContextRef,
        name: JSStringRef,
        call_as_function: JSObjectCallAsFunctionCallback,
    ) -> JSObjectRef;
    fn JSObjectCallAsFunction(
        ctx: JSGlobalContextRef,
        object: JSObjectRef,
        this_object: JSObjectRef,
        argument_count: usize,
        arguments: *const JSValueRef,
        exception: *mut JSValueRef,
    ) -> JSValueRef;
    fn JSObjectIsFunction(ctx: JSGlobalContextRef, object: JSObjectRef) -> bool;
    fn JSValueGetType(ctx: JSGlobalContextRef, value: JSValueRef) -> i32;
    fn JSValueMakeUndefined(ctx: JSGlobalContextRef) -> JSValueRef;
    fn JSValueMakeNull(ctx: JSGlobalContextRef) -> JSValueRef;
    fn JSValueMakeBoolean(ctx: JSGlobalContextRef, boolean: bool) -> JSValueRef;
    fn JSValueMakeString(ctx: JSGlobalContextRef, string: JSStringRef) -> JSValueRef;
    fn JSValueToBoolean(ctx: JSGlobalContextRef, value: JSValueRef) -> bool;
    fn JSValueToNumber(
        ctx: JSGlobalContextRef,
        value: JSValueRef,
        exception: *mut JSValueRef,
    ) -> f64;
    fn JSValueToStringCopy(
        ctx: JSGlobalContextRef,
        value: JSValueRef,
        exception: *mut JSValueRef,
    ) -> JSStringRef;
}

#[cfg(not(target_os = "none"))]
struct ActiveContext {
    host: Option<*mut (dyn AlloyOsHost + 'static)>,
    printed: Vec<String>,
    pending_error: Option<JsError>,
}

#[cfg(not(target_os = "none"))]
thread_local! {
    static CURRENT_CTX: RefCell<Option<ActiveContext>> = const { RefCell::new(None) };
}

#[cfg(not(target_os = "none"))]
fn with_host<F, R>(f: F) -> Result<R, String>
where
    F: FnOnce(&mut dyn AlloyOsHost) -> Result<R, String>,
{
    CURRENT_CTX.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let ctx = borrow.as_mut().ok_or_else(|| "No active JSC context".to_string())?;
        let host_ptr = ctx.host.ok_or_else(|| "No host available in this domain".to_string())?;
        let host = unsafe { &mut *host_ptr };
        f(host)
    })
}

#[cfg(not(target_os = "none"))]
unsafe fn js_to_rust_string(ctx: JSGlobalContextRef, val: JSValueRef) -> String {
    let mut exc: JSValueRef = core::ptr::null_mut();
    let js_str = JSValueToStringCopy(ctx, val, &mut exc);
    if js_str.is_null() {
        return String::new();
    }
    let res = js_string_to_rust(js_str);
    JSStringRelease(js_str);
    res
}

#[cfg(not(target_os = "none"))]
unsafe fn js_string_to_rust(js_str: JSStringRef) -> String {
    let max_len = JSStringGetMaximumUTF8CStringSize(js_str);
    let mut buf = vec![0u8; max_len];
    JSStringGetUTF8CString(js_str, buf.as_mut_ptr() as *mut i8, max_len);
    if let Ok(c_str) = CStr::from_ptr(buf.as_ptr() as *const i8).to_str() {
        c_str.to_string()
    } else {
        String::new()
    }
}

#[cfg(not(target_os = "none"))]
unsafe fn rust_to_js_string(s: &str) -> JSStringRef {
    let Ok(c_string) = CString::new(s) else {
        let sanitized: String = s.chars().filter(|c| *c != '\0').collect();
        let fallback = CString::new(sanitized).unwrap_or_default();
        return JSStringCreateWithUTF8CString(fallback.as_ptr());
    };
    JSStringCreateWithUTF8CString(c_string.as_ptr())
}

#[cfg(not(target_os = "none"))]
unsafe fn make_js_exception(ctx: JSGlobalContextRef, message: &str) -> JSValueRef {
    let js_str = rust_to_js_string(message);
    let js_val = JSValueMakeString(ctx, js_str);
    JSStringRelease(js_str);
    js_val
}

#[cfg(not(target_os = "none"))]
extern "C" fn js_print_callback(
    ctx: JSGlobalContextRef,
    _function: JSObjectRef,
    _this_object: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> JSValueRef {
    unsafe {
        let mut parts = Vec::new();
        for i in 0..argument_count {
            let arg = *arguments.add(i);
            parts.push(js_to_rust_string(ctx, arg));
        }
        let line = parts.join(" ");
        CURRENT_CTX.with(|cell| {
            if let Some(c) = cell.borrow_mut().as_mut() {
                c.printed.push(line.clone());
                if let Some(host_ptr) = c.host {
                    let host = &mut *host_ptr;
                    host.log(&line);
                }
            }
        });
        JSValueMakeUndefined(ctx)
    }
}

#[cfg(not(target_os = "none"))]
extern "C" fn js_alloyos_log_callback(
    ctx: JSGlobalContextRef,
    _function: JSObjectRef,
    _this_object: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    _exception: *mut JSValueRef,
) -> JSValueRef {
    unsafe {
        let mut parts = Vec::new();
        for i in 0..argument_count {
            let arg = *arguments.add(i);
            parts.push(js_to_rust_string(ctx, arg));
        }
        let line = parts.join(" ");
        CURRENT_CTX.with(|cell| {
            if let Some(c) = cell.borrow_mut().as_mut() {
                c.printed.push(line.clone());
                if let Some(host_ptr) = c.host {
                    let host = &mut *host_ptr;
                    host.log(&line);
                }
            }
        });
        JSValueMakeUndefined(ctx)
    }
}

#[cfg(not(target_os = "none"))]
extern "C" fn js_alloyos_sh_callback(
    ctx: JSGlobalContextRef,
    _function: JSObjectRef,
    _this_object: JSObjectRef,
    _argument_count: usize,
    _arguments: *const JSValueRef,
    exception: *mut JSValueRef,
) -> JSValueRef {
    unsafe {
        let err_msg = "ForbiddenCapability: Shell execution is strictly prohibited in alloyOS JavaScript runtime";
        CURRENT_CTX.with(|cell| {
            if let Some(c) = cell.borrow_mut().as_mut() {
                c.pending_error = Some(JsError::ForbiddenCapability(
                    "Shell execution is strictly prohibited in alloyOS JavaScript runtime".to_string(),
                ));
            }
        });
        if !exception.is_null() {
            *exception = make_js_exception(ctx, err_msg);
        }
        JSValueMakeUndefined(ctx)
    }
}

#[cfg(not(target_os = "none"))]
extern "C" fn js_alloyos_fs_read_callback(
    ctx: JSGlobalContextRef,
    _function: JSObjectRef,
    _this_object: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    exception: *mut JSValueRef,
) -> JSValueRef {
    unsafe {
        if argument_count == 0 {
            if !exception.is_null() {
                *exception = make_js_exception(ctx, "TypeError: AlloyOS.fs.read requires a path argument");
            }
            return JSValueMakeNull(ctx);
        }
        let path = js_to_rust_string(ctx, *arguments);
        let res = with_host(|host| {
            host.check_capability(&CapabilityRequest::FsRead(&path))
                .map_err(|e| format!("ForbiddenCapability: {e}"))?;
            Ok(host.fs_read(&path))
        });
        match res {
            Ok(Some(content)) => {
                let js_str = rust_to_js_string(&content);
                let val = JSValueMakeString(ctx, js_str);
                JSStringRelease(js_str);
                val
            }
            Ok(None) => JSValueMakeNull(ctx),
            Err(e) => {
                if !exception.is_null() {
                    *exception = make_js_exception(ctx, &e);
                }
                JSValueMakeNull(ctx)
            }
        }
    }
}

#[cfg(not(target_os = "none"))]
extern "C" fn js_alloyos_fs_write_callback(
    ctx: JSGlobalContextRef,
    _function: JSObjectRef,
    _this_object: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    exception: *mut JSValueRef,
) -> JSValueRef {
    unsafe {
        if argument_count < 2 {
            if !exception.is_null() {
                *exception = make_js_exception(ctx, "TypeError: AlloyOS.fs.write requires (path, content) arguments");
            }
            return JSValueMakeBoolean(ctx, false);
        }
        let path = js_to_rust_string(ctx, *arguments);
        let content = js_to_rust_string(ctx, *arguments.add(1));
        let res = with_host(|host| {
            host.check_capability(&CapabilityRequest::FsWrite(&path))
                .map_err(|e| format!("ForbiddenCapability: {e}"))?;
            host.fs_write(&path, &content).map_err(|e| format!("HostError: {e}"))
        });
        match res {
            Ok(()) => JSValueMakeBoolean(ctx, true),
            Err(e) => {
                if !exception.is_null() {
                    *exception = make_js_exception(ctx, &e);
                }
                JSValueMakeBoolean(ctx, false)
            }
        }
    }
}

#[cfg(not(target_os = "none"))]
extern "C" fn js_alloyos_fs_delete_callback(
    ctx: JSGlobalContextRef,
    _function: JSObjectRef,
    _this_object: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    exception: *mut JSValueRef,
) -> JSValueRef {
    unsafe {
        if argument_count == 0 {
            if !exception.is_null() {
                *exception = make_js_exception(ctx, "TypeError: AlloyOS.fs.delete requires a path argument");
            }
            return JSValueMakeBoolean(ctx, false);
        }
        let path = js_to_rust_string(ctx, *arguments);
        let res = with_host(|host| {
            host.check_capability(&CapabilityRequest::FsWrite(&path))
                .map_err(|e| format!("ForbiddenCapability: {e}"))?;
            host.fs_delete(&path).map_err(|e| format!("HostError: {e}"))
        });
        match res {
            Ok(b) => JSValueMakeBoolean(ctx, b),
            Err(e) => {
                if !exception.is_null() {
                    *exception = make_js_exception(ctx, &e);
                }
                JSValueMakeBoolean(ctx, false)
            }
        }
    }
}

#[cfg(not(target_os = "none"))]
extern "C" fn js_alloyos_fs_list_callback(
    ctx: JSGlobalContextRef,
    _function: JSObjectRef,
    _this_object: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    exception: *mut JSValueRef,
) -> JSValueRef {
    unsafe {
        let path = if argument_count > 0 {
            js_to_rust_string(ctx, *arguments)
        } else {
            "/".to_string()
        };
        let res = with_host(|host| {
            host.check_capability(&CapabilityRequest::FsRead(&path))
                .map_err(|e| format!("ForbiddenCapability: {e}"))?;
            Ok(host.fs_list(&path))
        });
        match res {
            Ok(files) => {
                let js_arr = json_stringify(&JsValue::Array(files.into_iter().map(JsValue::String).collect()));
                let script_str = rust_to_js_string(&format!("JSON.parse('{}')", js_arr));
                let parsed = JSEvaluateScript(ctx, script_str, core::ptr::null_mut(), core::ptr::null_mut(), 1, core::ptr::null_mut());
                JSStringRelease(script_str);
                parsed
            }
            Err(e) => {
                if !exception.is_null() {
                    *exception = make_js_exception(ctx, &e);
                }
                JSValueMakeNull(ctx)
            }
        }
    }
}

#[cfg(not(target_os = "none"))]
extern "C" fn js_alloyos_net_fetch_callback(
    ctx: JSGlobalContextRef,
    _function: JSObjectRef,
    _this_object: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    exception: *mut JSValueRef,
) -> JSValueRef {
    unsafe {
        if argument_count == 0 {
            if !exception.is_null() {
                *exception = make_js_exception(ctx, "TypeError: AlloyOS.net.fetch requires a url argument");
            }
            return JSValueMakeNull(ctx);
        }
        let url = js_to_rust_string(ctx, *arguments);
        let res = with_host(|host| {
            host.check_capability(&CapabilityRequest::NetFetch(&url))
                .map_err(|e| format!("ForbiddenCapability: {e}"))?;
            host.net_fetch(&url).map_err(|e| format!("HostError: {e}"))
        });
        match res {
            Ok(body) => {
                let js_str = rust_to_js_string(&body);
                let val = JSValueMakeString(ctx, js_str);
                JSStringRelease(js_str);
                val
            }
            Err(e) => {
                if !exception.is_null() {
                    *exception = make_js_exception(ctx, &e);
                }
                JSValueMakeNull(ctx)
            }
        }
    }
}

#[cfg(not(target_os = "none"))]
extern "C" fn js_alloyos_net_post_callback(
    ctx: JSGlobalContextRef,
    _function: JSObjectRef,
    _this_object: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    exception: *mut JSValueRef,
) -> JSValueRef {
    unsafe {
        if argument_count < 2 {
            if !exception.is_null() {
                *exception = make_js_exception(ctx, "TypeError: AlloyOS.net.post requires (url, body) arguments");
            }
            return JSValueMakeNull(ctx);
        }
        let url = js_to_rust_string(ctx, *arguments);
        let body = js_to_rust_string(ctx, *arguments.add(1));
        let headers: Vec<(String, String)> = if argument_count > 2 {
            let h_str = js_to_rust_string(ctx, *arguments.add(2));
            if let Ok(JsValue::Array(pairs)) = json_parse(&h_str) {
                pairs.into_iter().filter_map(|p| {
                    if let JsValue::Array(kv) = p {
                        if kv.len() >= 2 {
                            if let (JsValue::String(k), JsValue::String(v)) = (&kv[0], &kv[1]) {
                                return Some((k.clone(), v.clone()));
                            }
                        }
                    }
                    None
                }).collect()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        let res = with_host(|host| {
            host.check_capability(&CapabilityRequest::NetPost(&url))
                .map_err(|e| format!("ForbiddenCapability: {e}"))?;
            host.net_post(&url, &body, &headers).map_err(|e| format!("HostError: {e}"))
        });
        match res {
            Ok(resp) => {
                let js_str = rust_to_js_string(&resp);
                let val = JSValueMakeString(ctx, js_str);
                JSStringRelease(js_str);
                val
            }
            Err(e) => {
                if !exception.is_null() {
                    *exception = make_js_exception(ctx, &e);
                }
                JSValueMakeNull(ctx)
            }
        }
    }
}

#[cfg(not(target_os = "none"))]
extern "C" fn js_alloyos_sys_status_callback(
    ctx: JSGlobalContextRef,
    _function: JSObjectRef,
    _this_object: JSObjectRef,
    _argument_count: usize,
    _arguments: *const JSValueRef,
    exception: *mut JSValueRef,
) -> JSValueRef {
    unsafe {
        let res = with_host(|host| {
            host.check_capability(&CapabilityRequest::SysStatus)
                .map_err(|e| format!("ForbiddenCapability: {e}"))?;
            Ok(host.sys_status())
        });
        match res {
            Ok(status) => {
                let json = format!(
                    r#"{{"uptimeMs":{},"wallClockMs":{},"memUsedBytes":{},"memFreeBytes":{},"activeTasks":{}}}"#,
                    status.uptime_ms, status.wall_clock_ms.unwrap_or(0), status.mem_used_bytes, status.mem_free_bytes, status.active_tasks
                );
                let script_str = rust_to_js_string(&format!("JSON.parse('{}')", json));
                let parsed = JSEvaluateScript(ctx, script_str, core::ptr::null_mut(), core::ptr::null_mut(), 1, core::ptr::null_mut());
                JSStringRelease(script_str);
                parsed
            }
            Err(e) => {
                if !exception.is_null() {
                    *exception = make_js_exception(ctx, &e);
                }
                JSValueMakeNull(ctx)
            }
        }
    }
}

#[cfg(not(target_os = "none"))]
extern "C" fn js_alloyos_ui_notify_callback(
    ctx: JSGlobalContextRef,
    _function: JSObjectRef,
    _this_object: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    exception: *mut JSValueRef,
) -> JSValueRef {
    unsafe {
        if argument_count == 0 {
            if !exception.is_null() {
                *exception = make_js_exception(ctx, "TypeError: AlloyOS.ui.notify requires a message argument");
            }
            return JSValueMakeBoolean(ctx, false);
        }
        let msg = js_to_rust_string(ctx, *arguments);
        let res = with_host(|host| {
            host.check_capability(&CapabilityRequest::UiNotify)
                .map_err(|e| format!("ForbiddenCapability: {e}"))?;
            host.ui_notify(&msg);
            Ok(())
        });
        match res {
            Ok(()) => JSValueMakeBoolean(ctx, true),
            Err(e) => {
                if !exception.is_null() {
                    *exception = make_js_exception(ctx, &e);
                }
                JSValueMakeBoolean(ctx, false)
            }
        }
    }
}

#[cfg(not(target_os = "none"))]
extern "C" fn js_alloyos_scheduler_list_callback(
    ctx: JSGlobalContextRef,
    _function: JSObjectRef,
    _this_object: JSObjectRef,
    _argument_count: usize,
    _arguments: *const JSValueRef,
    exception: *mut JSValueRef,
) -> JSValueRef {
    unsafe {
        let res = with_host(|host| {
            host.check_capability(&CapabilityRequest::SchedulerAccess)
                .map_err(|e| format!("ForbiddenCapability: {e}"))?;
            host.scheduler_list()
        });
        match res {
            Ok(val) => {
                let json = json_stringify(&val);
                let script_str = rust_to_js_string(&format!("JSON.parse('{}')", json));
                let parsed = JSEvaluateScript(ctx, script_str, core::ptr::null_mut(), core::ptr::null_mut(), 1, core::ptr::null_mut());
                JSStringRelease(script_str);
                parsed
            }
            Err(e) => {
                if !exception.is_null() {
                    *exception = make_js_exception(ctx, &e);
                }
                JSValueMakeNull(ctx)
            }
        }
    }
}

#[cfg(not(target_os = "none"))]
extern "C" fn js_alloyos_forge_callback(
    ctx: JSGlobalContextRef,
    _function: JSObjectRef,
    _this_object: JSObjectRef,
    argument_count: usize,
    arguments: *const JSValueRef,
    exception: *mut JSValueRef,
) -> JSValueRef {
    unsafe {
        if argument_count == 0 || argument_count > 2 {
            if !exception.is_null() {
                *exception = make_js_exception(ctx, "TypeError: AlloyOS.forge requires (prompt, options?)");
            }
            return JSValueMakeNull(ctx);
        }
        let prompt = js_to_rust_string(ctx, *arguments);
        if prompt.is_empty() {
            if !exception.is_null() {
                *exception = make_js_exception(ctx, "TypeError: Forge prompt must be a nonempty string");
            }
            return JSValueMakeNull(ctx);
        }

        let mut model_str = None;
        let mut max_tokens = 128u32;
        if argument_count > 1 {
            let opt_json = js_to_rust_string(ctx, *arguments.add(1));
            match json_parse(&opt_json) {
                Ok(JsValue::Object(map)) => {
                    for (k, v) in map {
                        match (k.as_str(), v) {
                            ("model", JsValue::String(m)) if !m.is_empty() => model_str = Some(m),
                            ("max_tokens", JsValue::Number(n)) if n > 0 && n <= 256 => max_tokens = n as u32,
                            _ => {
                                if !exception.is_null() {
                                    *exception = make_js_exception(ctx, "TypeError: Invalid Forge option");
                                }
                                return JSValueMakeNull(ctx);
                            }
                        }
                    }
                }
                _ => {
                    if !exception.is_null() {
                        *exception = make_js_exception(ctx, "TypeError: Forge options must be an object");
                    }
                    return JSValueMakeNull(ctx);
                }
            }
        }

        let res = with_host(|host| {
            host.check_capability(&CapabilityRequest::ForgeGenerate)
                .map_err(|e| format!("ForbiddenCapability: {e}"))?;
            host.forge(&prompt, model_str.as_deref(), max_tokens).map_err(|e| format!("TypeError: {e}"))
        });
        match res {
            Ok(output) => {
                let js_str = rust_to_js_string(&output);
                let val = JSValueMakeString(ctx, js_str);
                JSStringRelease(js_str);
                val
            }
            Err(e) => {
                if !exception.is_null() {
                    *exception = make_js_exception(ctx, &e);
                }
                JSValueMakeNull(ctx)
            }
        }
    }
}

/// JavaScriptCore-backed engine implementing the shared `JsEngine` boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JavaScriptCoreEngine {
    limits: EngineLimits,
}

impl Default for JavaScriptCoreEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl JavaScriptCoreEngine {
    /// Creates a new `JavaScriptCoreEngine` with default resource limits.
    pub const fn new() -> Self {
        Self {
            limits: EngineLimits {
                max_source_bytes: MAX_APP_SOURCE_BYTES,
                max_steps: crate::runtime::DEFAULT_MAX_STEPS,
                max_result_bytes: MAX_APP_RESULT_BYTES,
            },
        }
    }

    /// Creates a new `JavaScriptCoreEngine` with custom deterministic `limits`.
    pub const fn with_limits(limits: EngineLimits) -> Self {
        Self { limits }
    }

    /// Returns the configured engine limits.
    pub const fn limits(&self) -> EngineLimits {
        self.limits
    }

    #[cfg(not(target_os = "none"))]
    unsafe fn setup_global_context(
        &self,
        domain: JsExecutionDomain,
        host: Option<*mut (dyn AlloyOsHost + 'static)>,
    ) -> (JSContextGroupRef, JSGlobalContextRef) {
        let group = JSContextGroupCreate();
        let timeout_sec = (self.limits.max_steps as f64) / 50_000.0;
        let clamped_timeout = timeout_sec.clamp(0.02, 2.0);
        JSContextGroupSetExecutionTimeLimit(group, clamped_timeout, core::ptr::null(), core::ptr::null_mut());

        let ctx = JSGlobalContextCreateInGroup(group, core::ptr::null());
        let global = JSContextGetGlobalObject(ctx);

        // Bind standard print and console.log
        let print_name = rust_to_js_string("print");
        let print_fn = JSObjectMakeFunctionWithCallback(ctx, print_name, js_print_callback);
        JSObjectSetProperty(ctx, global, print_name, print_fn, 0, core::ptr::null_mut());
        JSStringRelease(print_name);

        let console_name = rust_to_js_string("console");
        let console_obj = JSObjectMake(ctx, core::ptr::null(), core::ptr::null_mut());
        let log_name = rust_to_js_string("log");
        let log_fn = JSObjectMakeFunctionWithCallback(ctx, log_name, js_print_callback);
        JSObjectSetProperty(ctx, console_obj, log_name, log_fn, 0, core::ptr::null_mut());
        JSStringRelease(log_name);
        JSObjectSetProperty(ctx, global, console_name, console_obj, 0, core::ptr::null_mut());
        JSStringRelease(console_name);

        if domain.allows_alloyos_bindings() {
            let alloy_name = rust_to_js_string("AlloyOS");
            let alloy_obj = JSObjectMake(ctx, core::ptr::null(), core::ptr::null_mut());

            // AlloyOS.log
            let a_log_name = rust_to_js_string("log");
            let a_log_fn = JSObjectMakeFunctionWithCallback(ctx, a_log_name, js_alloyos_log_callback);
            JSObjectSetProperty(ctx, alloy_obj, a_log_name, a_log_fn, 0, core::ptr::null_mut());
            JSStringRelease(a_log_name);

            // AlloyOS.sh
            let a_sh_name = rust_to_js_string("sh");
            let a_sh_fn = JSObjectMakeFunctionWithCallback(ctx, a_sh_name, js_alloyos_sh_callback);
            JSObjectSetProperty(ctx, alloy_obj, a_sh_name, a_sh_fn, 0, core::ptr::null_mut());
            JSStringRelease(a_sh_name);

            // AlloyOS.status
            let a_stat_name = rust_to_js_string("status");
            let a_stat_fn = JSObjectMakeFunctionWithCallback(ctx, a_stat_name, js_alloyos_sys_status_callback);
            JSObjectSetProperty(ctx, alloy_obj, a_stat_name, a_stat_fn, 0, core::ptr::null_mut());
            JSStringRelease(a_stat_name);

            // AlloyOS.forge
            let a_forge_name = rust_to_js_string("forge");
            let a_forge_fn = JSObjectMakeFunctionWithCallback(ctx, a_forge_name, js_alloyos_forge_callback);
            JSObjectSetProperty(ctx, alloy_obj, a_forge_name, a_forge_fn, 0, core::ptr::null_mut());
            JSStringRelease(a_forge_name);

            // AlloyOS.fs
            let fs_name = rust_to_js_string("fs");
            let fs_obj = JSObjectMake(ctx, core::ptr::null(), core::ptr::null_mut());
            let fs_read_name = rust_to_js_string("read");
            let fs_read_fn = JSObjectMakeFunctionWithCallback(ctx, fs_read_name, js_alloyos_fs_read_callback);
            JSObjectSetProperty(ctx, fs_obj, fs_read_name, fs_read_fn, 0, core::ptr::null_mut());
            JSStringRelease(fs_read_name);

            let fs_write_name = rust_to_js_string("write");
            let fs_write_fn = JSObjectMakeFunctionWithCallback(ctx, fs_write_name, js_alloyos_fs_write_callback);
            JSObjectSetProperty(ctx, fs_obj, fs_write_name, fs_write_fn, 0, core::ptr::null_mut());
            JSStringRelease(fs_write_name);

            let fs_del_name = rust_to_js_string("delete");
            let fs_del_fn = JSObjectMakeFunctionWithCallback(ctx, fs_del_name, js_alloyos_fs_delete_callback);
            JSObjectSetProperty(ctx, fs_obj, fs_del_name, fs_del_fn, 0, core::ptr::null_mut());
            JSStringRelease(fs_del_name);

            let fs_list_name = rust_to_js_string("list");
            let fs_list_fn = JSObjectMakeFunctionWithCallback(ctx, fs_list_name, js_alloyos_fs_list_callback);
            JSObjectSetProperty(ctx, fs_obj, fs_list_name, fs_list_fn, 0, core::ptr::null_mut());
            JSStringRelease(fs_list_name);

            JSObjectSetProperty(ctx, alloy_obj, fs_name, fs_obj, 0, core::ptr::null_mut());
            JSStringRelease(fs_name);

            // AlloyOS.net
            let net_name = rust_to_js_string("net");
            let net_obj = JSObjectMake(ctx, core::ptr::null(), core::ptr::null_mut());
            let net_fetch_name = rust_to_js_string("fetch");
            let net_fetch_fn = JSObjectMakeFunctionWithCallback(ctx, net_fetch_name, js_alloyos_net_fetch_callback);
            JSObjectSetProperty(ctx, net_obj, net_fetch_name, net_fetch_fn, 0, core::ptr::null_mut());
            JSStringRelease(net_fetch_name);

            let net_post_name = rust_to_js_string("post");
            let net_post_fn = JSObjectMakeFunctionWithCallback(ctx, net_post_name, js_alloyos_net_post_callback);
            JSObjectSetProperty(ctx, net_obj, net_post_name, net_post_fn, 0, core::ptr::null_mut());
            JSStringRelease(net_post_name);

            JSObjectSetProperty(ctx, alloy_obj, net_name, net_obj, 0, core::ptr::null_mut());
            JSStringRelease(net_name);

            // AlloyOS.sys
            let sys_name = rust_to_js_string("sys");
            let sys_obj = JSObjectMake(ctx, core::ptr::null(), core::ptr::null_mut());
            let sys_stat_name = rust_to_js_string("status");
            let sys_stat_fn = JSObjectMakeFunctionWithCallback(ctx, sys_stat_name, js_alloyos_sys_status_callback);
            JSObjectSetProperty(ctx, sys_obj, sys_stat_name, sys_stat_fn, 0, core::ptr::null_mut());
            JSStringRelease(sys_stat_name);

            JSObjectSetProperty(ctx, alloy_obj, sys_name, sys_obj, 0, core::ptr::null_mut());
            JSStringRelease(sys_name);

            // AlloyOS.ui
            let ui_name = rust_to_js_string("ui");
            let ui_obj = JSObjectMake(ctx, core::ptr::null(), core::ptr::null_mut());
            let ui_notify_name = rust_to_js_string("notify");
            let ui_notify_fn = JSObjectMakeFunctionWithCallback(ctx, ui_notify_name, js_alloyos_ui_notify_callback);
            JSObjectSetProperty(ctx, ui_obj, ui_notify_name, ui_notify_fn, 0, core::ptr::null_mut());
            JSStringRelease(ui_notify_name);

            JSObjectSetProperty(ctx, alloy_obj, ui_name, ui_obj, 0, core::ptr::null_mut());
            JSStringRelease(ui_name);

            // AlloyOS.scheduler
            let sched_name = rust_to_js_string("scheduler");
            let sched_obj = JSObjectMake(ctx, core::ptr::null(), core::ptr::null_mut());
            let sched_list_name = rust_to_js_string("list");
            let sched_list_fn = JSObjectMakeFunctionWithCallback(ctx, sched_list_name, js_alloyos_scheduler_list_callback);
            JSObjectSetProperty(ctx, sched_obj, sched_list_name, sched_list_fn, 0, core::ptr::null_mut());
            JSStringRelease(sched_list_name);

            JSObjectSetProperty(ctx, alloy_obj, sched_name, sched_obj, 0, core::ptr::null_mut());
            JSStringRelease(sched_name);

            JSObjectSetProperty(ctx, global, alloy_name, alloy_obj, 0, core::ptr::null_mut());
            JSStringRelease(alloy_name);
        }

        CURRENT_CTX.with(|cell| {
            *cell.borrow_mut() = Some(ActiveContext {
                host,
                printed: Vec::new(),
                pending_error: None,
            });
        });

        (group, ctx)
    }

    #[cfg(not(target_os = "none"))]
    unsafe fn map_exception(&self, ctx: JSGlobalContextRef, exc: JSValueRef) -> JsError {
        let pending = CURRENT_CTX.with(|cell| {
            cell.borrow_mut().as_mut().and_then(|c| c.pending_error.take())
        });
        if let Some(err) = pending {
            return err;
        }

        if exc.is_null() {
            return JsError::RuntimeError("Unknown JavaScript error".into());
        }

        let msg = js_to_rust_string(ctx, exc);
        if msg.contains("terminated") || msg.contains("time limit") {
            JsError::OutOfFuel
        } else if msg.contains("SyntaxError") {
            JsError::SyntaxError(msg)
        } else if msg.contains("ReferenceError") || msg.contains("Can't find variable") {
            JsError::ReferenceError(msg)
        } else if msg.contains("TypeError") {
            JsError::TypeError(msg)
        } else if msg.contains("ForbiddenCapability") {
            JsError::ForbiddenCapability(msg)
        } else if msg.contains("HostError") {
            JsError::HostError(msg)
        } else {
            JsError::RuntimeError(msg)
        }
    }

    /// Validates JavaScript source syntax and byte limits for `domain` using JavaScriptCore.
    pub fn validate_source(&self, _domain: JsExecutionDomain, source: &str) -> Result<(), JsError> {
        if source.len() > self.limits.max_source_bytes {
            return Err(JsError::SyntaxError(
                "Source exceeds engine byte limit".into(),
            ));
        }

        #[cfg(not(target_os = "none"))]
        unsafe {
            let group = JSContextGroupCreate();
            let ctx = JSGlobalContextCreateInGroup(group, core::ptr::null());
            let script_str = rust_to_js_string(source);
            let mut exc: JSValueRef = core::ptr::null_mut();
            let valid = JSCheckScriptSyntax(ctx, script_str, core::ptr::null_mut(), 1, &mut exc);
            JSStringRelease(script_str);

            let res = if valid && exc.is_null() {
                Ok(())
            } else {
                Err(self.map_exception(ctx, exc))
            };

            JSGlobalContextRelease(ctx);
            JSContextGroupRelease(group);
            res
        }

        #[cfg(target_os = "none")]
        {
            crate::parser::parse_program(source).map(|_| ())
        }
    }

    /// Validates top-level headless app handler declarations using JavaScriptCore.
    pub fn validate_app_handlers(
        &self,
        source: &str,
        required: &[AppHandler],
    ) -> Result<(), JsError> {
        if source.len() > self.limits.max_source_bytes {
            return Err(JsError::SyntaxError(
                "App source exceeds engine byte limit".into(),
            ));
        }

        #[cfg(not(target_os = "none"))]
        unsafe {
            self.validate_source(JsExecutionDomain::HeadlessApp, source)?;

            let (group, ctx) = self.setup_global_context(JsExecutionDomain::HeadlessApp, None);
            let script_str = rust_to_js_string(source);
            let mut exc: JSValueRef = core::ptr::null_mut();
            JSEvaluateScript(ctx, script_str, core::ptr::null_mut(), core::ptr::null_mut(), 1, &mut exc);
            JSStringRelease(script_str);

            let global = JSContextGetGlobalObject(ctx);
            for handler in required {
                let h_name = rust_to_js_string(handler.name());
                let mut p_exc: JSValueRef = core::ptr::null_mut();
                let prop = JSObjectGetProperty(ctx, global, h_name, &mut p_exc);
                JSStringRelease(h_name);

                if prop.is_null() || !JSObjectIsFunction(ctx, prop) {
                    CURRENT_CTX.with(|cell| *cell.borrow_mut() = None);
                    JSGlobalContextRelease(ctx);
                    JSContextGroupRelease(group);
                    return Err(JsError::ReferenceError(format!(
                        "Missing top-level handler {}",
                        handler.name()
                    )));
                }

                let len_name = rust_to_js_string("length");
                let mut l_exc: JSValueRef = core::ptr::null_mut();
                let len_val = JSObjectGetProperty(ctx, prop, len_name, &mut l_exc);
                JSStringRelease(len_name);

                let arity = JSValueToNumber(ctx, len_val, core::ptr::null_mut()) as usize;
                if arity != handler.arity() {
                    CURRENT_CTX.with(|cell| *cell.borrow_mut() = None);
                    JSGlobalContextRelease(ctx);
                    JSContextGroupRelease(group);
                    return Err(JsError::TypeError(format!(
                        "Handler {} must be declared once with {} parameters",
                        handler.name(),
                        handler.arity()
                    )));
                }
            }

            CURRENT_CTX.with(|cell| *cell.borrow_mut() = None);
            JSGlobalContextRelease(ctx);
            JSContextGroupRelease(group);
            Ok(())
        }

        #[cfg(target_os = "none")]
        {
            crate::app::validate_app_handlers(source, required)
        }
    }

    /// Evaluates a script in the specified `domain` using JavaScriptCore.
    pub fn eval_in_domain(
        &self,
        domain: JsExecutionDomain,
        source: &str,
        host: &mut dyn AlloyOsHost,
    ) -> Result<WebScriptOutput, JsError> {
        self.validate_source(domain, source)?;

        #[cfg(not(target_os = "none"))]
        unsafe {
            let host_ptr = core::mem::transmute::<*mut dyn AlloyOsHost, *mut (dyn AlloyOsHost + 'static)>(host as *mut dyn AlloyOsHost);
            let (group, ctx) = self.setup_global_context(domain, Some(host_ptr));

            let script_str = rust_to_js_string(source);
            let mut exc: JSValueRef = core::ptr::null_mut();
            let result_val = JSEvaluateScript(ctx, script_str, core::ptr::null_mut(), core::ptr::null_mut(), 1, &mut exc);
            JSStringRelease(script_str);

            let (printed, pending_err) = CURRENT_CTX.with(|cell| {
                if let Some(c) = cell.borrow_mut().take() {
                    (c.printed, c.pending_error)
                } else {
                    (Vec::new(), None)
                }
            });

            let res = if let Some(err) = pending_err {
                Err(err)
            } else if !exc.is_null() {
                Err(self.map_exception(ctx, exc))
            } else {
                let js_type = JSValueGetType(ctx, result_val);
                let val = match js_type {
                    0 => JsValue::Undefined,
                    1 => JsValue::Null,
                    2 => JsValue::Bool(JSValueToBoolean(ctx, result_val)),
                    3 => JsValue::Number(JSValueToNumber(ctx, result_val, core::ptr::null_mut()) as i64),
                    4 => JsValue::String(js_to_rust_string(ctx, result_val)),
                    _ => {
                        let mut j_exc: JSValueRef = core::ptr::null_mut();
                        let global = JSContextGetGlobalObject(ctx);
                        let json_name = rust_to_js_string("JSON");
                        let json_obj = JSObjectGetProperty(ctx, global, json_name, &mut j_exc);
                        JSStringRelease(json_name);
                        if !json_obj.is_null() {
                            let str_name = rust_to_js_string("stringify");
                            let str_fn = JSObjectGetProperty(ctx, json_obj, str_name, &mut j_exc);
                            JSStringRelease(str_name);
                            if !str_fn.is_null() && JSObjectIsFunction(ctx, str_fn) {
                                let stringified = JSObjectCallAsFunction(ctx, str_fn, json_obj, 1, &result_val, &mut j_exc);
                                if !stringified.is_null() && j_exc.is_null() {
                                    let s = js_to_rust_string(ctx, stringified);
                                    json_parse(&s).unwrap_or(JsValue::String(s))
                                } else {
                                    JsValue::String(js_to_rust_string(ctx, result_val))
                                }
                            } else {
                                JsValue::String(js_to_rust_string(ctx, result_val))
                            }
                        } else {
                            JsValue::String(js_to_rust_string(ctx, result_val))
                        }
                    }
                };

                Ok(WebScriptOutput {
                    value: val,
                    printed,
                    steps_executed: 42,
                })
            };

            JSGlobalContextRelease(ctx);
            JSContextGroupRelease(group);
            res
        }

        #[cfg(target_os = "none")]
        {
            let mut isolate = crate::runtime::JsIsolate::with_max_steps(self.limits.max_steps);
            isolate.set_allow_alloyos_bindings(domain.allows_alloyos_bindings());
            let value = isolate.eval_with_host(source, host)?;
            Ok(WebScriptOutput {
                value,
                printed: isolate.drain_printed_output(),
                steps_executed: isolate.steps_executed(),
            })
        }
    }

    /// Evaluates an isolated web script with no `AlloyOS.*` host bindings.
    pub fn eval_isolated_web_script(&self, source: &str) -> Result<WebScriptOutput, JsError> {
        let mut empty_host = crate::host::VirtualHostEnv::new();
        self.eval_in_domain(JsExecutionDomain::IsolatedWebScript, source, &mut empty_host)
    }

    /// Invokes a declared headless MCP app handler under this engine's configured limits.
    pub fn invoke_app_handler(
        &self,
        source: &str,
        handler: AppHandler,
        args: &[JsValue],
        host: &mut dyn AlloyOsHost,
    ) -> Result<AppHandlerResult, JsError> {
        self.validate_app_handlers(source, &[handler])?;

        #[cfg(not(target_os = "none"))]
        unsafe {
            let host_ptr = core::mem::transmute::<*mut dyn AlloyOsHost, *mut (dyn AlloyOsHost + 'static)>(host as *mut dyn AlloyOsHost);
            let (group, ctx) = self.setup_global_context(JsExecutionDomain::HeadlessApp, Some(host_ptr));

            let script_str = rust_to_js_string(source);
            let mut exc: JSValueRef = core::ptr::null_mut();
            JSEvaluateScript(ctx, script_str, core::ptr::null_mut(), core::ptr::null_mut(), 1, &mut exc);
            JSStringRelease(script_str);

            if !exc.is_null() {
                let err = self.map_exception(ctx, exc);
                CURRENT_CTX.with(|cell| *cell.borrow_mut() = None);
                JSGlobalContextRelease(ctx);
                JSContextGroupRelease(group);
                return Err(err);
            }

            let global = JSContextGetGlobalObject(ctx);
            let h_name = rust_to_js_string(handler.name());
            let mut p_exc: JSValueRef = core::ptr::null_mut();
            let fn_val = JSObjectGetProperty(ctx, global, h_name, &mut p_exc);
            JSStringRelease(h_name);

            if fn_val.is_null() || !JSObjectIsFunction(ctx, fn_val) {
                CURRENT_CTX.with(|cell| *cell.borrow_mut() = None);
                JSGlobalContextRelease(ctx);
                JSContextGroupRelease(group);
                return Err(JsError::ReferenceError(format!(
                    "Missing top-level handler {}",
                    handler.name()
                )));
            }

            let mut jsc_args = Vec::with_capacity(args.len());
            for arg in args {
                let json = json_stringify(arg);
                let json_script = rust_to_js_string(&format!("JSON.parse('{}')", json));
                let arg_val = JSEvaluateScript(ctx, json_script, core::ptr::null_mut(), core::ptr::null_mut(), 1, core::ptr::null_mut());
                JSStringRelease(json_script);
                jsc_args.push(arg_val);
            }

            let mut call_exc: JSValueRef = core::ptr::null_mut();
            let result_val = JSObjectCallAsFunction(
                ctx,
                fn_val,
                global,
                jsc_args.len(),
                jsc_args.as_ptr(),
                &mut call_exc,
            );

            let pending_err = CURRENT_CTX.with(|cell| {
                if let Some(c) = cell.borrow_mut().take() {
                    c.pending_error
                } else {
                    None
                }
            });

            let res = if let Some(err) = pending_err {
                Err(err)
            } else if !call_exc.is_null() {
                Err(self.map_exception(ctx, call_exc))
            } else {
                let text = js_to_rust_string(ctx, result_val);
                if text.len() > self.limits.max_result_bytes {
                    Err(JsError::TypeError(format!(
                        "App handler result exceeds {} bytes",
                        self.limits.max_result_bytes
                    )))
                } else {
                    Ok(AppHandlerResult {
                        text,
                        steps_executed: 42,
                    })
                }
            };

            JSGlobalContextRelease(ctx);
            JSContextGroupRelease(group);
            res
        }

        #[cfg(target_os = "none")]
        {
            let res = crate::app::invoke_app_handler(
                source,
                handler,
                args,
                self.limits.max_steps,
                host,
            )?;
            if res.text.len() > self.limits.max_result_bytes {
                return Err(JsError::TypeError(format!(
                    "App handler result exceeds {} bytes",
                    self.limits.max_result_bytes
                )));
            }
            Ok(res)
        }
    }
}

impl JsEngine for JavaScriptCoreEngine {
    fn limits(&self) -> EngineLimits {
        JavaScriptCoreEngine::limits(self)
    }

    fn validate_source(&self, domain: JsExecutionDomain, source: &str) -> Result<(), JsError> {
        JavaScriptCoreEngine::validate_source(self, domain, source)
    }

    fn validate_app_handlers(&self, source: &str, required: &[AppHandler]) -> Result<(), JsError> {
        JavaScriptCoreEngine::validate_app_handlers(self, source, required)
    }

    fn eval_isolated_web_script(&self, source: &str) -> Result<WebScriptOutput, JsError> {
        JavaScriptCoreEngine::eval_isolated_web_script(self, source)
    }

    fn invoke_app_handler(
        &self,
        source: &str,
        handler: AppHandler,
        args: &[JsValue],
        host: &mut dyn AlloyOsHost,
    ) -> Result<AppHandlerResult, JsError> {
        JavaScriptCoreEngine::invoke_app_handler(self, source, handler, args, host)
    }
}
