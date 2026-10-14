//! Protected application supervisor. Package code only executes in an EL0
//! executable. Host calls are serviced by the initiating host with its policy.
use super::{
    app_process::AddressSpace,
    pcb::{Task, TaskState},
};
use crate::sync::IrqMutex;
use alloc::{
    collections::BTreeMap,
    format,
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use alloyos_js::{AlloyOsHost, AppHandler, AppHandlerResult, JsError, JsValue};
const MAX_HOST_CALLS: usize = 256;
const MAX_HOST_BYTES: usize = 4 * 1024 * 1024;
struct Channel {
    host_calls: usize,
    host_request_bytes: usize,
    host_response_bytes: usize,
    active: bool,
    host_pending: bool,
    request: Option<String>,
    host_request: Option<String>,
    host_response: Option<Result<String, String>>,
    result: Option<Result<String, String>>,
}
impl Channel {
    fn allow_request(&mut self, bytes: usize) -> bool {
        if self.host_calls >= MAX_HOST_CALLS
            || bytes > MAX_HOST_BYTES.saturating_sub(self.host_request_bytes)
        {
            return false;
        }
        self.host_calls += 1;
        self.host_request_bytes += bytes;
        true
    }
    fn limit_response(&mut self, result: Result<String, String>) -> Result<String, String> {
        let bytes = result.as_ref().map(|s| s.len()).unwrap_or(0);
        if bytes > MAX_HOST_BYTES.saturating_sub(self.host_response_bytes) {
            return Err("App host response budget exhausted".into());
        }
        self.host_response_bytes += bytes;
        result
    }
}
static CHANNELS: IrqMutex<BTreeMap<u64, Channel>> = IrqMutex::new(BTreeMap::new());
static APPS: IrqMutex<BTreeMap<String, Arc<Task>>> = IrqMutex::new(BTreeMap::new());
fn error(s: impl Into<String>) -> JsError {
    JsError::TypeError(s.into())
}
fn executable() -> Result<Vec<u8>, JsError> {
    let vfs = crate::fs::vfs::VFS.lock();
    let vfs = vfs.as_ref().ok_or_else(|| error("VFS unavailable"))?;
    let inode = vfs
        .lookup("/bin/app-runner")
        .map_err(|_| error("Missing /bin/app-runner"))?;
    if inode.size > 16 * 1024 * 1024 {
        return Err(error("App executable too large"));
    }
    let mut bytes = alloc::vec![0;inode.size];
    let n = vfs
        .root_fs
        .read(inode.ino, 0, &mut bytes)
        .map_err(|_| error("Cannot read app executable"))?;
    if n != bytes.len() {
        return Err(error("Incomplete app executable"));
    }
    Ok(bytes)
}
pub fn install() {
    alloyos_mcp::process::install(invoke);
    alloyos_mcp::process::install_lifecycle(stop);
}
pub fn stop(app_id: &str) {
    if let Some(task) = APPS.lock().remove(app_id) {
        task.set_exit_code(-125);
        CHANNELS.lock().remove(&task.pid);
    }
}
pub fn invoke(
    app_id: &str,
    source: &str,
    handler: AppHandler,
    args: &[JsValue],
    steps: usize,
    host: &mut dyn AlloyOsHost,
) -> Result<AppHandlerResult, JsError> {
    let request =
        alloyos_js::process::encode_request(source, handler, args, steps).map_err(error)?;
    let existing = APPS.lock().get(app_id).cloned();
    if let Some(task) = existing.as_ref().filter(|t| t.state() == TaskState::Zombie) {
        CHANNELS.lock().remove(&task.pid);
    }
    let task = if let Some(task) = existing.filter(|t| t.state() != TaskState::Zombie) {
        task
    } else {
        let retired = {
            let mut apps = APPS.lock();
            if apps
                .values()
                .filter(|task| task.state() != TaskState::Zombie)
                .count()
                >= 4
            {
                let channels = CHANNELS.lock();
                let idle = apps
                    .iter()
                    .find(|(_, task)| channels.get(&task.pid).is_some_and(|c| !c.active))
                    .map(|(id, _)| id.clone());
                drop(channels);
                let Some(id) = idle else {
                    return Err(error(
                        "Protected app process limit reached (4 active handlers)",
                    ));
                };
                apps.remove(&id)
            } else {
                None
            }
        };
        if let Some(task) = retired {
            task.set_exit_code(-125);
            CHANNELS.lock().remove(&task.pid);
            #[cfg(all(target_arch = "aarch64", target_os = "none"))]
            super::scheduler::sleep_until(
                crate::drivers::rng::monotonic_time_ns().saturating_add(1_000_000),
            );
        }
        let bytes = executable()?;
        let task = Arc::new(
            Task::user_executable(&bytes, 4096, 16384)
                .map_err(|e| error(format!("App executable load: {e:?}")))?,
        );
        CHANNELS.lock().insert(
            task.pid,
            Channel {
                host_calls: 0,
                host_request_bytes: 0,
                host_response_bytes: 0,
                active: false,
                host_pending: false,
                request: None,
                host_request: None,
                host_response: None,
                result: None,
            },
        );
        APPS.lock().insert(app_id.to_string(), task.clone());
        super::scheduler::add_task(task.clone());
        task
    };
    {
        let mut channels = CHANNELS.lock();
        let channel = channels
            .get_mut(&task.pid)
            .ok_or_else(|| error("Missing process channel"))?;
        if channel.active {
            return Err(error("App already has an active request"));
        }
        channel.host_calls = 0;
        channel.host_request_bytes = 0;
        channel.host_response_bytes = 0;
        channel.active = true;
        channel.request = Some(request);
        channel.result = None;
        channel.host_response = None;
    }
    let deadline = crate::drivers::rng::monotonic_time_ns().saturating_add(5_000_000_000);
    *task.execution_deadline.lock() = Some(deadline);
    loop {
        let (operation, result) = {
            let mut channels = CHANNELS.lock();
            let c = channels
                .get_mut(&task.pid)
                .ok_or_else(|| error("App channel closed"))?;
            (c.host_request.take(), c.result.take())
        };
        if let Some(result) = result {
            *task.execution_deadline.lock() = None;
            if let Some(c) = CHANNELS.lock().get_mut(&task.pid) {
                c.active = false;
            }
            return result
                .map(|text| AppHandlerResult {
                    text,
                    steps_executed: 0,
                })
                .map_err(error);
        }
        if let Some(operation) = operation {
            let result = alloyos_js::process::dispatch_host(&operation, host);
            if let Some(c) = CHANNELS.lock().get_mut(&task.pid) {
                c.host_response = Some(c.limit_response(result))
            }
        }
        if task.state() == TaskState::Zombie || crate::drivers::rng::monotonic_time_ns() >= deadline
        {
            task.set_exit_code(-110);
            CHANNELS.lock().remove(&task.pid);
            APPS.lock().remove(app_id);
            return Err(error("App faulted or exceeded its request deadline"));
        }
        #[cfg(all(target_arch = "aarch64", target_os = "none"))]
        unsafe {
            core::arch::asm!("wfi", options(nomem, nostack));
        }
        #[cfg(not(all(target_arch = "aarch64", target_os = "none")))]
        return Err(error("EL0 app invocation requires guest execution"));
    }
}
/// Explicit cancellation retires only the selected app and releases its IPC.
pub fn cancel(app_id: &str) -> bool {
    let Some(task) = APPS.lock().remove(app_id) else {
        return false;
    };
    task.set_exit_code(-125);
    CHANNELS.lock().remove(&task.pid);
    true
}
pub fn dispatch(pid: u64, space: &mut AddressSpace, number: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    use alloyos_js::process::*;
    let mut channels = CHANNELS.lock();
    let Some(c) = channels.get_mut(&pid) else {
        return -1;
    };
    match number {
        REQUEST_SYSCALL => {
            let Some(request) = c.request.as_ref() else {
                return -11;
            };
            if a2 < request.len() as u64 {
                return -90;
            }
            if space.copy_out(a1, request.as_bytes()).is_err() {
                return -14;
            }
            let n = request.len();
            c.request = None;
            n as i64
        }
        HOST_SYSCALL => {
            if !c.active || c.request.is_some() || c.result.is_some() {
                return -1;
            }
            if space.validate_range(a3, MAX_WIRE_BYTES, true).is_err() {
                return -14;
            }
            if let Some(response) = c.host_response.as_ref() {
                let n = match response {
                    Ok(text) => {
                        if space.copy_out(a3, text.as_bytes()).is_err() {
                            return -14;
                        }
                        text.len() as i64
                    }
                    Err(_) => -1,
                };
                c.host_response = None;
                c.host_pending = false;
                return n;
            }
            if c.host_pending {
                return -11;
            }
            if a2 > MAX_WIRE_BYTES as u64 {
                return -90;
            }
            if !c.allow_request(a2 as usize) {
                return -7;
            }
            let mut bytes = alloc::vec![0;a2 as usize];
            if space.copy_in(a1, &mut bytes).is_err() {
                return -14;
            }
            let Ok(text) = String::from_utf8(bytes) else {
                return -22;
            };
            c.host_request = Some(text);
            c.host_pending = true;
            -11
        }
        RESULT_SYSCALL => {
            if !c.active || c.request.is_some() || c.host_pending || c.result.is_some() {
                return -1;
            }
            if a2 > MAX_WIRE_BYTES as u64 {
                return -90;
            }
            let mut bytes = alloc::vec![0;a2 as usize];
            if space.copy_in(a1, &mut bytes).is_err() {
                return -14;
            }
            let Ok(text) = String::from_utf8(bytes) else {
                return -22;
            };
            c.result = Some(if a3 == 0 { Ok(text) } else { Err(text) });
            0
        }
        _ => -38,
    }
}

/// Guest-only acceptance: uses the installed executable and private address
/// space, not a mock transport or a host interpreter.
#[cfg(any(feature = "process-selftest", feature = "process-runtime-selftest"))]
pub fn acceptance() {
    use alloyos_js::{ScriptCapabilities, VirtualHostEnv};
    let mut package_service = alloyos_mcp::PackageService::new();
    let mut package = alloyos_mcp::PackageSnapshot::default();
    package.files.insert(
        "/apps/parser-probe/parser-probe.mcp".into(),
        "name=Parser Probe\nversion=1\ncapabilities=ExposeMcpTools\n".into(),
    );
    for expression in [
        format!("{}true", "!".repeat(10000)),
        format!("{}1{}", "(".repeat(1000), ")".repeat(1000)),
        format!("{}1", "1+".repeat(4000)),
        format!("a{}", "()".repeat(1000)),
        format!("{}1", "function() { return 1; } + ".repeat(200)),
    ] {
        package.files.insert(
            "/apps/parser-probe/main.js".into(),
            format!("function callTool(name,args) {{return {};}}", expression),
        );
        package_service.apply_snapshot(package.clone());
        assert!(!package_service.status("parser-probe").unwrap().ready);
    }
    package.files.insert(
        "/apps/parser-probe/main.js".into(),
        "function callTool(name,args) {return 'alive';}".into(),
    );
    package_service.apply_snapshot(package);
    assert!(package_service.status("parser-probe").unwrap().ready);
    crate::serial_println!("APP-PROCESS: hostile package parser limits and host recovery PASS");

    let mut host =
        VirtualHostEnv::with_capabilities(ScriptCapabilities::for_mcp_app("process-test"));
    let args = [
        JsValue::String("test".into()),
        JsValue::Object(BTreeMap::new()),
    ];
    let source = r#"function callTool(name,args) { AlloyOS.fs.write('/var/process-test/data','protected'); return AlloyOS.fs.read('/var/process-test/data'); }"#;
    assert_eq!(
        invoke(
            "process-test",
            source,
            AppHandler::CallTool,
            &args,
            50000,
            &mut host
        )
        .unwrap()
        .text,
        "protected"
    );
    let pid = APPS.lock().get("process-test").unwrap().pid;
    assert_eq!(
        invoke(
            "process-test",
            source,
            AppHandler::CallTool,
            &args,
            50000,
            &mut host
        )
        .unwrap()
        .text,
        "protected"
    );
    assert_eq!(APPS.lock().get("process-test").unwrap().pid, pid);
    crate::serial_println!("APP-PROCESS: persistent EL0 handler and filesystem IPC PASS");
    let denied="function callTool(name,args) { AlloyOS.fs.write('/var/peer/secret','bad'); return 'bad'; }";
    assert!(invoke(
        "process-test",
        denied,
        AppHandler::CallTool,
        &args,
        50000,
        &mut host
    )
    .is_err());
    crate::serial_println!("APP-PROCESS: denied peer filesystem capability PASS");
    let loop_source = "function callTool(name,args) { while (true) {} return 'bad'; }";
    assert!(invoke(
        "process-test",
        loop_source,
        AppHandler::CallTool,
        &args,
        100,
        &mut host
    )
    .is_err());
    assert_eq!(
        invoke(
            "peer-test",
            "function callTool(name,args) { return 'peer'; }",
            AppHandler::CallTool,
            &args,
            50000,
            &mut host
        )
        .unwrap()
        .text,
        "peer"
    );
    crate::serial_println!("APP-PROCESS: bounded infinite loop and independent peer PASS");
    APPS.lock().get("process-test").unwrap().set_exit_code(-11);
    assert_eq!(
        invoke(
            "process-test",
            source,
            AppHandler::CallTool,
            &args,
            50000,
            &mut host
        )
        .unwrap()
        .text,
        "protected"
    );
    assert_ne!(APPS.lock().get("process-test").unwrap().pid, pid);
    crate::serial_println!("APP-PROCESS: process restart PASS");
    let scheduler =
        alloc::rc::Rc::new(core::cell::RefCell::new(alloyos_js::ScriptScheduler::new()));
    let mut caps = ScriptCapabilities::for_mcp_app("tasks");
    caps.allow_scheduler = true;
    let mut tasks = VirtualHostEnv::with_capabilities(caps);
    tasks.scheduler = Some(scheduler.clone());
    let source = r#"function callTool(name,args) { AlloyOS.scheduler.create('probe','/scripts/probe.js','interval:1000','1;'); return JSON.stringify(AlloyOS.scheduler.list()); }"#;
    let result = invoke(
        "tasks",
        source,
        AppHandler::CallTool,
        &args,
        50000,
        &mut tasks,
    )
    .unwrap();
    assert!(result.text.contains("probe"));
    let result = invoke(
        "tasks",
        "function callTool(name,args) { return JSON.stringify(AlloyOS.scheduler.list()); }",
        AppHandler::CallTool,
        &args,
        50000,
        &mut tasks,
    )
    .unwrap();
    assert!(result.text.contains("probe"));
    crate::serial_println!("APP-PROCESS: Tasks scheduler IPC persistence PASS");
    let before = host.logs.len();
    let flood="function callTool(name,args) { let i=0; while(i<300) { AlloyOS.log('bounded'); i++; } return AlloyOS.fs.read('/var/process-test/data'); }";
    assert!(invoke(
        "process-test",
        flood,
        AppHandler::CallTool,
        &args,
        50000,
        &mut host
    )
    .is_err());
    assert!(host.logs.len() - before <= MAX_HOST_CALLS);
    assert_eq!(
        invoke(
            "process-test",
            "function callTool(name,args) { return AlloyOS.fs.read('/var/process-test/data'); }",
            AppHandler::CallTool,
            &args,
            50000,
            &mut host
        )
        .unwrap()
        .text,
        "protected"
    );
    crate::serial_println!("APP-PROCESS: bounded host-call flood and budget reset PASS");

    let allocation = "function callTool(name,args) { let data='x'; while (data.length < 32000000) { data=data+data; } return 'bad'; }";
    assert!(invoke(
        "memory-test",
        allocation,
        AppHandler::CallTool,
        &args,
        50000,
        &mut host
    )
    .is_err());
    assert!(
        !APPS.lock().contains_key("memory-test"),
        "Memory limit must fault the protected process, not merely consume interpreter fuel"
    );
    assert_eq!(
        invoke(
            "memory-test",
            "function callTool(name,args) { return 'recovered'; }",
            AppHandler::CallTool,
            &args,
            50000,
            &mut host
        )
        .unwrap()
        .text,
        "recovered"
    );
    crate::serial_println!("APP-PROCESS: private heap exhaustion and restart PASS");
    // These execute real privileged syscall attempts in an ungranted process.
    for number in [130u32, 131, 132, 133, 134, 2] {
        let words = [
            0xd2800008 | (number << 5),
            0xd4000001,
            0xd2800788,
            0xd4000001,
        ];
        let bytes: Vec<u8> = words.iter().flat_map(|v| v.to_le_bytes()).collect();
        let task = Arc::new(Task::user_process(&bytes, 2, 3).unwrap());
        super::scheduler::add_task(task.clone());
        let until = crate::drivers::rng::monotonic_time_ns() + 1_000_000_000;
        while task.exit_code().is_none() {
            assert!(crate::drivers::rng::monotonic_time_ns() < until);
            super::scheduler::sleep_until(crate::drivers::rng::monotonic_time_ns() + 1_000_000);
        }
        assert_eq!(task.exit_code(), Some(-1), "Ungrant syscall {number}");
    }
    crate::serial_println!("APP-PROCESS: display input and shared kernel operation denial PASS");
    let spinner = Arc::new(Task::user_process(&0x14000000u32.to_le_bytes(), 2, 3).unwrap());
    *spinner.execution_deadline.lock() =
        Some(crate::drivers::rng::monotonic_time_ns() + 50_000_000);
    super::scheduler::add_task(spinner.clone());
    let until = crate::drivers::rng::monotonic_time_ns() + 1_000_000_000;
    while spinner.exit_code().is_none() {
        assert!(
            crate::drivers::rng::monotonic_time_ns() < until,
            "Timer failed to cancel busy process"
        );
        super::scheduler::sleep_until(crate::drivers::rng::monotonic_time_ns() + 1_000_000);
    }
    assert_eq!(spinner.exit_code(), Some(-110));
    assert_eq!(
        invoke(
            "peer-test",
            "function callTool(name,args) { return 'peer'; }",
            AppHandler::CallTool,
            &args,
            50000,
            &mut host
        )
        .unwrap()
        .text,
        "peer"
    );
    crate::serial_println!(
        "APP-PROCESS: timer cancels nonyielding process and peer continues PASS"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn broker_budget_limits_calls_and_aggregate_bytes() {
        let mut c = Channel {
            host_calls: 0,
            host_request_bytes: 0,
            host_response_bytes: 0,
            active: true,
            host_pending: false,
            request: None,
            host_request: None,
            host_response: None,
            result: None,
        };
        for _ in 0..MAX_HOST_CALLS {
            assert!(c.allow_request(1));
        }
        assert!(!c.allow_request(1));
        c.host_calls = 0;
        c.host_request_bytes = 0;
        for _ in 0..4 {
            assert!(c.allow_request(1024 * 1024));
        }
        assert!(!c.allow_request(1));
        c.host_response_bytes = MAX_HOST_BYTES - 2;
        assert_eq!(c.limit_response(Ok("ok".into())).unwrap(), "ok");
        assert!(c.limit_response(Ok("x".into())).is_err());
    }
    #[test]
    fn host_protocol_requires_active_invocation_and_full_output_buffer() {
        let mut space = AddressSpace::prepare(&[0; 4], 260, 261).unwrap();
        let pid = u64::MAX - 4;
        CHANNELS.lock().insert(
            pid,
            Channel {
                host_calls: 0,
                host_request_bytes: 0,
                host_response_bytes: 0,
                active: false,
                host_pending: false,
                request: None,
                host_request: None,
                host_response: None,
                result: None,
            },
        );
        assert_eq!(dispatch(pid, &mut space, 121, 0, 0, 0), -1);
        assert_eq!(dispatch(pid, &mut space, 122, 0, 0, 0), -1);
        CHANNELS.lock().get_mut(&pid).unwrap().active = true;
        let end = space.stack_top() - 8;
        assert_eq!(dispatch(pid, &mut space, 121, 0, 0, end), -14);
        CHANNELS.lock().remove(&pid);
    }
}
