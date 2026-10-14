//! Supervisor-granted endpoints. A process can only use handles installed in
//! its task; sender identity is always supplied by the syscall entry path.
use super::app_process::{MAX_MESSAGE, MAX_QUEUE};
use crate::sync::IrqMutex;
use alloc::{collections::VecDeque, sync::Arc, vec::Vec};

pub struct Message {
    pub sender: u64,
    pub payload: Vec<u8>,
}
pub struct Endpoint {
    pub(crate) queue: IrqMutex<VecDeque<Message>>,
}
impl Endpoint {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            queue: IrqMutex::new(VecDeque::new()),
        })
    }
}
pub struct Port {
    pub endpoint: Arc<Endpoint>,
    pub send: bool,
    pub receive: bool,
}
impl Port {
    pub fn send(&self, sender: u64, payload: &[u8]) -> i64 {
        if !self.send {
            return -1;
        }
        if payload.len() > MAX_MESSAGE {
            return -90;
        }
        let mut queue = self.endpoint.queue.lock();
        if queue.len() >= MAX_QUEUE {
            return -11;
        }
        queue.push_back(Message {
            sender,
            payload: payload.to_vec(),
        });
        payload.len() as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn endpoint_grants_and_backpressure_are_enforced() {
        let endpoint = Endpoint::new();
        let sender = Port {
            endpoint: endpoint.clone(),
            send: true,
            receive: false,
        };
        let receiver = Port {
            endpoint,
            send: false,
            receive: true,
        };
        assert_eq!(receiver.send(8, b"denied"), -1);
        assert_eq!(sender.send(8, &[0; MAX_MESSAGE + 1]), -90);
        for _ in 0..MAX_QUEUE {
            assert_eq!(sender.send(8, b"ok"), 2);
        }
        assert_eq!(sender.send(8, b"full"), -11);
        let message = receiver.endpoint.queue.lock().pop_front().unwrap();
        assert_eq!(message.sender, 8);
        assert_eq!(message.payload, b"ok");
    }
}
