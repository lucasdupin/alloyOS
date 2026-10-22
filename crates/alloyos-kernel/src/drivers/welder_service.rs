//! Supervised setup execution in a capability-minimal Welder ELF. Existing IPC
//! ports carry only bounded setup snapshots/proposals, never account secrets.
use crate::task::{
    ipc::{Endpoint, Port},
    pcb::{Task, TaskState},
    scheduler,
};
use alloc::{
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use alloyos_welder::setup_service::{self, Assembly, Request, Response, Transport};

#[derive(Default)]
pub struct WelderService {
    task: Option<Arc<Task>>,
    input: Option<Port>,
    output: Option<Arc<Endpoint>>,
    executable: Option<Vec<u8>>,
    last_request: u64,
}
impl Drop for WelderService {
    fn drop(&mut self) {
        self.stop(-125);
    }
}
impl WelderService {
    fn stop(&mut self, status: i32) {
        if let Some(task) = self.task.take() {
            task.set_exit_code(status);
        }
        self.input = None;
        self.output = None;
    }
    fn launch(&mut self) -> Result<(), String> {
        if self
            .task
            .as_ref()
            .is_some_and(|task| task.state() != TaskState::Zombie)
        {
            return Ok(());
        }
        if self.task.is_some() {
            self.stop(-125);
            scheduler::sleep_until(super::rng::monotonic_time_ns().saturating_add(1_000_000));
        }
        if self.executable.is_none() {
            let guard = crate::fs::vfs::VFS.lock();
            let vfs = guard.as_ref().ok_or("Welder filesystem unavailable")?;
            let inode = vfs
                .lookup("/system/welder")
                .map_err(|_| "Missing /system/welder")?;
            if inode.size == 0
                || inode.size > 8 * 1024 * 1024
                || inode.kind != crate::fs::vfs::InodeKind::File
            {
                return Err("Invalid Welder executable size".into());
            }
            let mut bytes = Vec::new();
            bytes
                .try_reserve_exact(inode.size)
                .map_err(|_| "Welder executable allocation failed")?;
            bytes.resize(inode.size, 0);
            let n = vfs
                .root_fs
                .read(inode.ino, 0, &mut bytes)
                .map_err(|_| "Cannot read Welder executable")?;
            if n != bytes.len() {
                return Err("Incomplete Welder executable".into());
            }
            self.executable = Some(bytes);
        }
        let task = Arc::new(
            Task::user_executable(self.executable.as_ref().unwrap(), 1024, 4096)
                .map_err(|_| "Cannot load protected Welder service")?,
        );
        let input = Endpoint::new();
        let output = Endpoint::new();
        task.ports.lock().push(Port {
            endpoint: input.clone(),
            send: false,
            receive: true,
        });
        task.ports.lock().push(Port {
            endpoint: output.clone(),
            send: true,
            receive: false,
        });
        self.input = Some(Port {
            endpoint: input,
            send: true,
            receive: false,
        });
        self.output = Some(output);
        crate::serial_println!(
            "[welderd] protected setup executable loaded pid={}",
            task.pid
        );
        scheduler::add_task(task.clone());
        self.task = Some(task);
        Ok(())
    }
    fn fail<T>(&mut self, message: &str, status: i32) -> Result<T, String> {
        self.stop(status);
        Err(message.into())
    }
}
impl Transport for WelderService {
    fn exchange(&mut self, request: &Request) -> Result<Response, String> {
        request.validate()?;
        if request.id <= self.last_request {
            return Err("Replayed Welder request".into());
        }
        // Consume the ID before starting. A failed request is never replayed;
        // the next caller obtains a new ID and revalidates its current state.
        self.last_request = request.id;
        self.launch()?;
        let bytes = serde_json::to_vec(request).map_err(|_| "Cannot encode Welder request")?;
        if bytes.len() > setup_service::MAX_BYTES {
            return Err("Welder request exceeds limit".into());
        }
        let deadline = super::rng::monotonic_time_ns().saturating_add(1_000_000_000);
        let task = self.task.as_ref().unwrap().clone();
        *task.execution_deadline.lock() = Some(deadline);
        let mut offset = 0;
        let mut response = Assembly::default();
        loop {
            if task.state() == TaskState::Zombie {
                return self.fail(
                    "Welder setup service stopped; retry the current question",
                    -125,
                );
            }
            if super::rng::monotonic_time_ns() >= deadline {
                return self.fail(
                    "Welder setup service timed out; retry the current question",
                    -110,
                );
            }
            if offset < bytes.len() {
                let frame = setup_service::chunk(request.id, &bytes, offset)?;
                match self.input.as_ref().unwrap().send(0, &frame) {
                    n if n == frame.len() as i64 => {
                        offset += setup_service::chunk_payload_bytes(frame.len())
                    }
                    -11 => {}
                    _ => return self.fail("Welder request transport failed", -71),
                }
            }
            let message = self.output.as_ref().unwrap().queue.lock().pop_front();
            if let Some(message) = message {
                if message.sender != task.pid {
                    return self.fail("Invalid Welder reply identity", -71);
                }
                match response.push(&message.payload, Some(request.id)) {
                    Ok(Some((_, bytes))) => {
                        let result = serde_json::from_slice::<Response>(&bytes)
                            .map_err(|_| "Invalid Welder reply encoding".to_string());
                        let result = result.and_then(|response| {
                            response.validate_for(request)?;
                            Ok(response)
                        });
                        *task.execution_deadline.lock() = None;
                        return match result {
                            Ok(response) => {
                                crate::serial_println!(
                                    "[welderd] setup request={} completed pid={}",
                                    request.id,
                                    task.pid
                                );
                                Ok(response)
                            }
                            Err(error) => {
                                self.stop(-71);
                                Err(error)
                            }
                        };
                    }
                    Ok(None) => {}
                    Err(_) => return self.fail("Invalid Welder response framing", -71),
                }
            }
            scheduler::sleep_until(super::rng::monotonic_time_ns().saturating_add(1_000_000));
        }
    }
}

#[cfg(feature = "process-runtime-selftest")]
pub fn acceptance() {
    use alloyos_welder::{
        setup::{SetupStage, SetupState},
        setup_service::{Operation, VERSION},
    };
    let mut service = WelderService::default();
    let mut request = Request {
        version: VERSION,
        id: 1,
        state: SetupState::default(),
        revision: 0,
        operation: Operation::Render,
        clock_report: String::new(),
        persistent: true,
    };
    let result = (|| -> Result<(), String> {
        let response = service.exchange(&request)?;
        if !response
            .view
            .as_ref()
            .is_some_and(|view| view.prompt.contains("What should I call you?"))
        {
            return Err("initial question mismatch".into());
        }
        let first_pid = service.task.as_ref().unwrap().pid;
        request.id = 2;
        request.revision = 1;
        request.operation = Operation::Answer {
            value: "Protected Fixture".into(),
        };
        let response = service.exchange(&request)?;
        let plan = response.plan.ok_or("missing transition")?;
        if plan.next.stage != SetupStage::AgentName || plan.next.user_name != "Protected Fixture" {
            return Err("name transition mismatch".into());
        }
        request.state = plan.next;
        request.id = 3;
        request.operation = Operation::FaultProbe;
        if service.exchange(&request).is_ok() {
            return Err("fault was not contained".into());
        }
        request.id = 4;
        request.operation = Operation::Render;
        let response = service.exchange(&request)?;
        if service.task.as_ref().unwrap().pid == first_pid
            || !response
                .view
                .as_ref()
                .is_some_and(|view| view.prompt.contains("Protected Fixture"))
        {
            return Err("restart snapshot mismatch".into());
        }
        if service.exchange(&request).is_ok() {
            return Err("replayed request accepted".into());
        }
        request.id = 5;
        request.operation = Operation::HangProbe;
        if service.exchange(&request).is_ok() {
            return Err("hung worker was not stopped".into());
        }
        request.id = 6;
        request.operation = Operation::Render;
        if !service
            .exchange(&request)?
            .view
            .is_some_and(|view| view.prompt.contains("Protected Fixture"))
        {
            return Err("timeout recovery mismatch".into());
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            crate::serial_println!("WELDER-SERVICE: protected setup transition, fault/timeout restart and replay rejection PASS");
        }
        Err(error) => panic!("Welder service acceptance failed: {}", error),
    }
}
