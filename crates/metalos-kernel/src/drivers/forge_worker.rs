//! Forge execution context. The UI only submits bounded messages and reads replies.
use crate::sync::IrqMutex;
use alloc::{
    boxed::Box,
    collections::{BTreeMap, VecDeque},
    string::{String, ToString},
    sync::Arc,
};
use core::task::Poll;
use metalos_forge::{ForgeError, Generation, GenerationRequest, InferenceBackend};
const QUEUE_LIMIT: usize = 16;
enum Command {
    Attach(String, Box<dyn InferenceBackend>),
    Generate(String, GenerationRequest, Arc<IrqMutex<Reply>>),
}
#[derive(Default)]
struct Reply {
    done: Option<Result<Generation, ForgeError>>,
    partial: String,
}
static COMMANDS: IrqMutex<VecDeque<Command>> = IrqMutex::new(VecDeque::new());
struct Proxy {
    id: String,
    resident: u64,
    registered: bool,
    reply: Option<Arc<IrqMutex<Reply>>>,
    partial: String,
}
pub fn wrap(id: &str, backend: Box<dyn InferenceBackend>) -> Box<dyn InferenceBackend> {
    let resident = backend.resident_bytes();
    let registered = {
        let mut queue = COMMANDS.lock();
        if queue.len() < QUEUE_LIMIT {
            queue.push_back(Command::Attach(id.to_string(), backend));
            true
        } else {
            false
        }
    };
    crate::task::scheduler::wake(2);
    Box::new(Proxy {
        id: id.to_string(),
        resident,
        registered,
        reply: None,
        partial: String::new(),
    })
}
impl InferenceBackend for Proxy {
    fn resident_bytes(&self) -> u64 {
        self.resident
    }
    fn partial_text(&self) -> Option<&str> {
        if self.partial.is_empty() {
            None
        } else {
            Some(&self.partial)
        }
    }
    fn start_generation(
        &mut self,
        request: &GenerationRequest,
    ) -> Result<Option<Generation>, ForgeError> {
        if !self.registered {
            return Err(ForgeError::BackendUnavailable);
        }
        if self.reply.is_some() {
            return Err(ForgeError::Busy);
        }
        let reply = Arc::new(IrqMutex::new(Reply::default()));
        let mut queue = COMMANDS.lock();
        if queue.len() >= QUEUE_LIMIT {
            return Err(ForgeError::Busy);
        }
        queue.push_back(Command::Generate(
            self.id.clone(),
            request.clone(),
            reply.clone(),
        ));
        self.reply = Some(reply);
        self.partial.clear();
        crate::task::scheduler::wake(2);
        Ok(None)
    }
    fn poll_generation(&mut self) -> Poll<Result<Generation, ForgeError>> {
        let Some(reply) = self.reply.as_ref() else {
            return Poll::Ready(Err(ForgeError::InvalidRequest));
        };
        let result = {
            let mut reply = reply.lock();
            if self.partial != reply.partial {
                self.partial.clone_from(&reply.partial);
            }
            reply.done.take()
        };
        if let Some(result) = result {
            self.reply = None;
            Poll::Ready(result)
        } else {
            Poll::Pending
        }
    }
    fn generate(&mut self, request: &GenerationRequest) -> Result<Generation, ForgeError> {
        self.start_generation(request)?;
        loop {
            if let Poll::Ready(result) = self.poll_generation() {
                return result;
            }
        }
    }
}
pub extern "C" fn run() -> ! {
    let mut backends = BTreeMap::new();
    loop {
        let command = {
            let mut queue = COMMANDS.lock();
            let command = queue.pop_front();
            if command.is_none() {
                crate::task::scheduler::block_current();
            }
            command
        };
        match command {
            Some(Command::Attach(id, backend)) => {
                backends.insert(id, backend);
            }
            Some(Command::Generate(id, request, reply)) => {
                let Some(backend) = backends.get_mut(&id) else {
                    reply.lock().done = Some(Err(ForgeError::BackendUnavailable));
                    continue;
                };
                let result = match backend.start_generation(&request) {
                    Ok(Some(result)) => Ok(result),
                    Err(error) => Err(error),
                    Ok(None) => loop {
                        let result = backend.poll_generation();
                        if let Some(partial) = backend.partial_text() {
                            let mut state = reply.lock();
                            if state.partial != partial {
                                state.partial = partial.to_string();
                            }
                        }
                        if let Poll::Ready(result) = result {
                            break result;
                        }
                    },
                };
                reply.lock().done = Some(result);
            }
            None => {
                #[cfg(all(target_arch = "aarch64", target_os = "none"))]
                unsafe {
                    core::arch::asm!("wfi");
                }
                #[cfg(not(all(target_arch = "aarch64", target_os = "none")))]
                core::hint::spin_loop();
            }
        }
    }
}
