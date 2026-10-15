//! Bounded JSON wire protocol shared by the EL0 app runner and its supervisor.
//! No caller identity or capability policy is accepted from the wire.
use crate::{AlloyOsHost, AppHandler, JsError, JsValue, OsEvent, SystemStatus};
use alloc::{
    string::{String, ToString},
    vec::Vec,
};
pub const MAX_WIRE_BYTES: usize = 1024 * 1024;
pub const REQUEST_SYSCALL: u64 = 120;
pub const HOST_SYSCALL: u64 = 121;
pub const RESULT_SYSCALL: u64 = 122;
// Bound nesting before the recursive JSON parser touches untrusted process data.
pub fn validate_wire(input: &str) -> Result<(), String> {
    if input.len() > MAX_WIRE_BYTES {
        return Err("IPC payload exceeds limit".into());
    }
    let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
    for byte in input.bytes() {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else {
            match byte {
                b'"' => quoted = true,
                b'[' | b'{' => {
                    depth += 1;
                    if depth > 40 {
                        return Err("IPC nesting exceeds limit".into());
                    }
                }
                b']' | b'}' => {
                    depth = depth.checked_sub(1).ok_or("Invalid IPC nesting")?;
                }
                _ => (),
            }
        }
    }
    if depth != 0 || quoted {
        return Err("Incomplete IPC payload".into());
    }
    Ok(())
}
fn text(value: &JsValue) -> Result<&str, String> {
    if let JsValue::String(s) = value {
        Ok(s)
    } else {
        Err("Expected string".into())
    }
}
fn arg(args: &[JsValue], i: usize) -> Result<&str, String> {
    text(args.get(i).ok_or("Missing argument")?)
}
pub fn encode_request(
    source: &str,
    handler: AppHandler,
    args: &[JsValue],
    max_steps: usize,
) -> Result<String, String> {
    let value = JsValue::Array(alloc::vec![
        JsValue::String(source.into()),
        JsValue::String(handler.name().into()),
        JsValue::Array(args.to_vec()),
        JsValue::Number(max_steps as i64)
    ]);
    let data = crate::json_stringify(&value);
    if data.len() > MAX_WIRE_BYTES {
        Err("App request exceeds IPC limit".into())
    } else {
        Ok(data)
    }
}
pub fn execute_request(
    request: &str,
    host: &mut dyn AlloyOsHost,
) -> Result<crate::AppHandlerResult, JsError> {
    if request.len() > MAX_WIRE_BYTES {
        return Err(JsError::TypeError("Request too large".into()));
    }
    validate_wire(request).map_err(JsError::TypeError)?;
    let JsValue::Array(v) = crate::json_parse(request)? else {
        return Err(JsError::TypeError("Invalid invocation".into()));
    };
    if v.len() != 4 {
        return Err(JsError::TypeError("Invalid invocation".into()));
    }
    let source = arg(&v, 0).map_err(JsError::TypeError)?;
    let handler = match arg(&v, 1).map_err(JsError::TypeError)? {
        "callTool" => AppHandler::CallTool,
        "readResource" => AppHandler::ReadResource,
        "getPrompt" => AppHandler::GetPrompt,
        "renderTile" => AppHandler::RenderTile,
        _ => return Err(JsError::TypeError("Unknown handler".into())),
    };
    let JsValue::Array(args) = &v[2] else {
        return Err(JsError::TypeError("Invalid arguments".into()));
    };
    let JsValue::Number(steps) = v[3] else {
        return Err(JsError::TypeError("Invalid fuel".into()));
    };
    if !(1..=50_000).contains(&steps) {
        return Err(JsError::TypeError("Invalid fuel".into()));
    }
    crate::invoke_app_handler(source, handler, args, steps as usize, host)
}
/// Run a host request against the supervisor's manifest-bound host. The app
/// cannot substitute an app ID or policy; every privileged operation is checked.
pub fn dispatch_host(request: &str, host: &mut dyn AlloyOsHost) -> Result<String, String> {
    if request.len() > MAX_WIRE_BYTES {
        return Err("Host request exceeds IPC limit".into());
    }
    validate_wire(request)?;
    let JsValue::Array(v) = crate::json_parse(request).map_err(|e| e.to_string())? else {
        return Err("Invalid host request".into());
    };
    let op = arg(&v, 0)?;
    let args = &v[1..];
    use crate::CapabilityRequest as C;
    let result = match op {
        "check" => {
            let name = arg(args, 0)?;
            let value = arg(args, 1)?;
            let cap = match name {
                "read" => C::FsRead(value),
                "write" => C::FsWrite(value),
                "fetch" => C::NetFetch(value),
                "post" => C::NetPost(value),
                "status" => C::SysStatus,
                "notify" => C::UiNotify,
                "forge" => C::ForgeGenerate,
                "scheduler" => C::SchedulerAccess,
                _ => return Err("Unknown capability".into()),
            };
            host.check_capability(&cap)?;
            JsValue::Null
        }
        "read" => {
            let p = arg(args, 0)?;
            host.check_capability(&C::FsRead(p))?;
            host.fs_read(p)
                .map(JsValue::String)
                .unwrap_or(JsValue::Null)
        }
        "write" => {
            let p = arg(args, 0)?;
            host.check_capability(&C::FsWrite(p))?;
            host.fs_write(p, arg(args, 1)?)?;
            JsValue::Null
        }
        "delete" => {
            let p = arg(args, 0)?;
            host.check_capability(&C::FsWrite(p))?;
            JsValue::Bool(host.fs_delete(p)?)
        }
        "list" => {
            let p = arg(args, 0)?;
            host.check_capability(&C::FsRead(p))?;
            JsValue::Array(host.fs_list(p).into_iter().map(JsValue::String).collect())
        }
        "fetch" => {
            let p = arg(args, 0)?;
            host.check_capability(&C::NetFetch(p))?;
            JsValue::String(host.net_fetch(p)?)
        }
        "post" => {
            let p = arg(args, 0)?;
            let body = arg(args, 1)?;
            host.check_capability(&C::NetPost(p))?;
            let mut headers = Vec::new();
            let parsed_headers = match args.get(2) {
                Some(JsValue::Array(h_arr)) => Some(h_arr.clone()),
                Some(JsValue::String(s)) => {
                    if let Ok(JsValue::Array(h_arr)) = crate::json_parse(s) {
                        Some(h_arr)
                    } else {
                        None
                    }
                }
                _ => None,
            };
            if let Some(h_arr) = parsed_headers {
                for item in h_arr {
                    if let JsValue::Array(pair) = item {
                        if pair.len() == 2 {
                            if let (JsValue::String(k), JsValue::String(v)) = (&pair[0], &pair[1]) {
                                headers.push((k.clone(), v.clone()));
                            }
                        }
                    }
                }
            }
            JsValue::String(host.net_post(p, body, &headers)?)
        }
        "status" => {
            host.check_capability(&C::SysStatus)?;
            let s = host.sys_status();
            JsValue::Array(alloc::vec![
                JsValue::Number(s.uptime_ms as i64),
                JsValue::Number(s.mem_used_bytes as i64),
                JsValue::Number(s.mem_free_bytes as i64),
                JsValue::Number(s.active_tasks as i64),
                s.wall_clock_ms.map(|ms| JsValue::Number(ms as i64)).unwrap_or(JsValue::Null)
            ])
        }
        "notify" => {
            host.check_capability(&C::UiNotify)?;
            host.ui_notify(arg(args, 0)?);
            JsValue::Null
        }
        "log" => {
            host.log(arg(args, 0)?);
            JsValue::Null
        }
        "scheduler_list" => {
            host.check_capability(&C::SchedulerAccess)?;
            host.scheduler_list()?
        }
        "scheduler_create" => {
            host.check_capability(&C::SchedulerAccess)?;
            host.scheduler_create(arg(args, 0)?, arg(args, 1)?, arg(args, 2)?, arg(args, 3)?)?
        }
        "scheduler_edit" => {
            host.check_capability(&C::SchedulerAccess)?;
            host.scheduler_edit(arg(args, 0)?, arg(args, 1)?, arg(args, 2)?, arg(args, 3)?)?
        }
        "scheduler_delete" => {
            host.check_capability(&C::SchedulerAccess)?;
            host.scheduler_delete(arg(args, 0)?)?
        }
        "forge" => {
            host.check_capability(&C::ForgeGenerate)?;
            let model = arg(args, 1)?;
            let tokens = arg(args, 2)?
                .parse::<u32>()
                .map_err(|_| "Invalid token limit")?;
            JsValue::String(host.forge(
                arg(args, 0)?,
                if model.is_empty() { None } else { Some(model) },
                tokens,
            )?)
        }
        _ => return Err("Unknown host operation".into()),
    };
    let response = crate::json_stringify(&result);
    if response.len() > MAX_WIRE_BYTES {
        Err("Host response exceeds IPC limit".into())
    } else {
        Ok(response)
    }
}
/// Transport adapter owns no OS resources. All operations round-trip to the
/// process supervisor, which derives identity from the scheduled PID.
pub struct ProcessHost<F: Fn(&str) -> Result<String, String>> {
    pub exchange: F,
}
impl<F: Fn(&str) -> Result<String, String>> ProcessHost<F> {
    fn call(&self, op: &str, args: &[&str]) -> Result<JsValue, String> {
        let mut v = alloc::vec![JsValue::String(op.into())];
        v.extend(args.iter().map(|s| JsValue::String((*s).into())));
        let wire = crate::json_stringify(&JsValue::Array(v));
        if wire.len() > MAX_WIRE_BYTES {
            return Err("Host request exceeds IPC limit".into());
        }
        crate::json_parse(&(self.exchange)(&wire)?).map_err(|e| e.to_string())
    }
}
impl<F: Fn(&str) -> Result<String, String>> AlloyOsHost for ProcessHost<F> {
    fn check_capability(&self, req: &crate::CapabilityRequest<'_>) -> Result<(), String> {
        use crate::CapabilityRequest as C;
        let (op, value) = match req {
            C::FsRead(p) => ("read", *p),
            C::FsWrite(p) => ("write", *p),
            C::NetFetch(p) => ("fetch", *p),
            C::NetPost(p) => ("post", *p),
            C::SysStatus => ("status", ""),
            C::UiNotify => ("notify", ""),
            C::ForgeGenerate => ("forge", ""),
            C::SchedulerAccess => ("scheduler", ""),
        };
        self.call("check", &[op, value]).map(|_| ())
    }
    fn fs_read(&mut self, p: &str) -> Option<String> {
        match self.call("read", &[p]).ok()? {
            JsValue::String(s) => Some(s),
            _ => None,
        }
    }
    fn fs_write(&mut self, p: &str, c: &str) -> Result<(), String> {
        self.call("write", &[p, c]).map(|_| ())
    }
    fn fs_delete(&mut self, p: &str) -> Result<bool, String> {
        self.call("delete", &[p]).map(|v| v == JsValue::Bool(true))
    }
    fn fs_list(&mut self, p: &str) -> Vec<String> {
        match self.call("list", &[p]) {
            Ok(JsValue::Array(v)) => v
                .into_iter()
                .filter_map(|v| {
                    if let JsValue::String(s) = v {
                        Some(s)
                    } else {
                        None
                    }
                })
                .collect(),
            _ => Vec::new(),
        }
    }
    fn net_fetch(&mut self, p: &str) -> Result<String, String> {
        let v = self.call("fetch", &[p])?;
        text(&v).map(ToString::to_string)
    }
    fn net_post(
        &mut self,
        p: &str,
        body: &str,
        headers: &[(String, String)],
    ) -> Result<String, String> {
        let h_val = JsValue::Array(
            headers
                .iter()
                .map(|(k, v)| {
                    JsValue::Array(alloc::vec![
                        JsValue::String(k.clone()),
                        JsValue::String(v.clone()),
                    ])
                })
                .collect(),
        );
        let wire_headers = crate::json_stringify(&h_val);
        let v = self.call("post", &[p, body, &wire_headers])?;
        text(&v).map(ToString::to_string)
    }
    fn sys_status(&self) -> SystemStatus {
        let mut s = SystemStatus {
            wall_clock_ms: None,
            uptime_ms: 0,
            mem_used_bytes: 0,
            mem_free_bytes: 0,
            active_tasks: 0,
        };
        if let Ok(JsValue::Array(v)) = self.call("status", &[]) {
            if let [JsValue::Number(a), JsValue::Number(b), JsValue::Number(c), JsValue::Number(d), wall] =
                v.as_slice()
            {
                s = SystemStatus {
                    wall_clock_ms: match wall { JsValue::Number(ms) if *ms >= 0 => Some(*ms as u64), _ => None },
                    uptime_ms: *a as u64,
                    mem_used_bytes: *b as u64,
                    mem_free_bytes: *c as u64,
                    active_tasks: *d as u32,
                }
            }
        }
        s
    }
    fn ui_notify(&mut self, p: &str) {
        let _ = self.call("notify", &[p]);
    }
    fn log(&mut self, p: &str) {
        let _ = self.call("log", &[p]);
    }
    fn drain_emitted_events(&mut self) -> Vec<OsEvent> {
        Vec::new()
    }
    fn scheduler_list(&mut self) -> Result<JsValue, String> {
        self.call("scheduler_list", &[])
    }
    fn scheduler_create(&mut self, a: &str, b: &str, c: &str, d: &str) -> Result<JsValue, String> {
        self.call("scheduler_create", &[a, b, c, d])
    }
    fn scheduler_edit(&mut self, a: &str, b: &str, c: &str, d: &str) -> Result<JsValue, String> {
        self.call("scheduler_edit", &[a, b, c, d])
    }
    fn scheduler_delete(&mut self, a: &str) -> Result<JsValue, String> {
        self.call("scheduler_delete", &[a])
    }
    fn forge(&mut self, p: &str, m: Option<&str>, n: u32) -> Result<String, String> {
        let v = self.call("forge", &[p, m.unwrap_or(""), &n.to_string()])?;
        text(&v).map(ToString::to_string)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;
    #[test]
    fn malformed_wire_fails_without_panics() {
        let mut host = crate::VirtualHostEnv::with_capabilities(crate::ScriptCapabilities::none());
        for input in [
            "[]",
            "null",
            "{}",
            "[42]",
            "[\"read\"]",
            "[\"write\",\"/var/a\"]",
            "[\"check\"]",
        ] {
            assert!(dispatch_host(input, &mut host).is_err(), "{input}");
        }
    }
    #[test]
    fn wire_enforces_manifest_host_policy() {
        let deeply_nested = alloc::format!("{}0{}", "[".repeat(100), "]".repeat(100));
        assert!(validate_wire(&deeply_nested).is_err());
        let mut host = crate::VirtualHostEnv::with_capabilities(
            crate::ScriptCapabilities::for_mcp_app("sample"),
        );
        assert!(dispatch_host("[\"write\",\"/var/peer/secret\",\"bad\"]", &mut host).is_err());
        assert!(dispatch_host("[\"scheduler_list\"]", &mut host).is_err());
        assert!(dispatch_host("[\"write\",\"/var/sample/data\",\"ok\"]", &mut host).is_ok());
        assert_eq!(
            dispatch_host("[\"read\",\"/var/sample/data\"]", &mut host).unwrap(),
            "\"ok\""
        );
    }
    #[test]
    fn invocation_round_trip_and_fuel_limit() {
        let mut host = crate::VirtualHostEnv::new();
        let source = "function callTool(name, args) { return name; }";
        let args = [
            JsValue::String("hello".into()),
            JsValue::Object(BTreeMap::new()),
        ];
        let request = encode_request(source, AppHandler::CallTool, &args, 50000).unwrap();
        assert_eq!(execute_request(&request, &mut host).unwrap().text, "hello");
        let request = encode_request(source, AppHandler::CallTool, &args, 50001).unwrap();
        assert!(execute_request(&request, &mut host).is_err());
    }
    #[test]
    fn dispatch_host_net_post_round_trip() {
        let mut host = crate::VirtualHostEnv::new();
        host.register_http_post_response("https://api.test/echo", "echoed");
        let wire = "[\"post\",\"https://api.test/echo\",\"body text\",[[\"Content-Type\",\"text/plain\"]]]";
        assert_eq!(
            dispatch_host(wire, &mut host).unwrap(),
            "\"echoed\""
        );
    }
}
